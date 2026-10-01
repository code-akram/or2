//! The host's side of the one-shot exchange. The phone's side is `or2_core::pair`.
//!
//! ```text
//! host  -> {"v":1,"nonce":"<base64 of 32 random bytes>"}
//! phone -> {"v":1,"key":"<openssh public key>","device":"<label>","mac":"<base64 HMAC-SHA256(otp, nonce || key)>"}
//! host  -> {"ok":true}  |  {"ok":false,"reason":"<code>"}
//! ```
//!
//! Reasons: `authentication` (the MAC did not verify), `key` (not a key this tool authorizes),
//! `declined` (the person typed no), `timeout` (nobody answered in time), `request` (unreadable
//! or oversized request), `busy` (another verified request is already being handled), `failed`
//! (this host could not write `authorized_keys`).
//!
//! # One attempt
//!
//! The listener serves exactly one *attempt*, then stops, whatever its result. An attempt is a
//! request that is well formed, within the size limit, **and whose HMAC verifies**: only the
//! holder of the one-time password can make one. Everything else (a bare newline, junk, an
//! unfinished or oversized line, a wrong proof, silence) is refused or ignored without ending the
//! pairing, so a scanner that probes the port cannot burn the code. The one-time password only
//! ever authenticates a MAC over this connection's fresh nonce, so a recorded request is useless
//! on another connection.
//!
//! # Concurrency and what an unauthenticated peer can cost
//!
//! Every accepted connection is handled on its own thread, so an idle socket occupies only
//! itself: an honest phone is served at once while idle sockets wait out their time. The cost of
//! an unauthenticated peer is bounded ([`Limits`]): each connection has [`PRE_AUTH`] in total to
//! deliver its one request line (at most [`REQUEST_LIMIT`] bytes), at most [`MAX_ACTIVE`]
//! connections are handled at once and [`MAX_ACTIVE_PER_PEER`] per peer address (more are closed
//! unanswered), and a peer address whose requests were refused [`MAX_FAILURES_PER_PEER`] times is
//! no longer greeted. Only one verified request is taken (a second one is told `busy`): the
//! confirmation and the write happen on the listener's own thread.

use std::collections::HashMap;
use std::io;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use sha2::Sha256;

use crate::account::Account;
use crate::authorized_keys::{self, Added};
use crate::confirm::{Answer, Confirm, ConfirmRequest};
use crate::date::DateTime;
use crate::keyline::KeyLine;
use crate::net::{Connection, PairListener};

/// The host's listening window, from the moment it starts listening.
pub const WINDOW: Duration = Duration::from_secs(120);
/// A write of a reply.
pub const IO_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest request line accepted.
pub const REQUEST_LIMIT: usize = 2048;
/// The whole time a connection has to deliver its request line once it was accepted.
pub const PRE_AUTH: Duration = Duration::from_secs(8);
/// Connections handled at once.
pub const MAX_ACTIVE: usize = 16;
/// Connections handled at once from one peer address (the phone races up to four endpoints).
pub const MAX_ACTIVE_PER_PEER: usize = 4;
/// A peer address that has sent this many refused requests is not greeted again.
pub const MAX_FAILURES_PER_PEER: u32 = 5;

/// How the cost of unauthenticated peers is bounded; the defaults are the module's constants.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub pre_auth: Duration,
    pub max_active: usize,
    pub max_active_per_peer: usize,
    pub max_failures_per_peer: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            pre_auth: PRE_AUTH,
            max_active: MAX_ACTIVE,
            max_active_per_peer: MAX_ACTIVE_PER_PEER,
            max_failures_per_peer: MAX_FAILURES_PER_PEER,
        }
    }
}

/// How long a thread waits in one read before it looks at the clock and the stop flag.
const SLICE: Duration = Duration::from_millis(50);

/// `HMAC-SHA256(otp, nonce || key)`.
fn mac_of(otp: &[u8; 16], nonce: &[u8], key: &str) -> Hmac<Sha256> {
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(otp).expect("HMAC accepts a key of any length");
    mac.update(nonce);
    mac.update(key.as_bytes());
    mac
}

/// What the exchange needs from its surroundings.
pub struct Session<'a> {
    /// The account the key is authorized for: its name is what is shown, its home is where the
    /// key goes.
    pub account: &'a Account,
    pub otp: [u8; 16],
    pub confirm: &'a dyn Confirm,
    /// Fills a buffer with random bytes (the nonce).
    pub random: &'a dyn Fn(&mut [u8]),
    pub now: &'a dyn Fn() -> DateTime,
    pub limits: Limits,
}

#[derive(Debug)]
pub enum Outcome {
    /// Confirmed and authorized (or already authorized).
    Authorized {
        added: Added,
        device: String,
        fingerprint: String,
    },
    /// The person at the host said no.
    Declined { device: String, fingerprint: String },
    /// The key was not one this tool authorizes (after the MAC verified).
    KeyRejected,
    /// The window passed with no attempt, or nobody answered the question.
    TimedOut,
    /// Confirmed, but writing `authorized_keys` failed.
    WriteFailed(io::Error),
    /// The listener itself failed.
    ListenFailed(io::Error),
}

/// How a connection that was not the attempt is counted.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Connections that sent nothing.
    pub silent: u32,
    /// Connections refused for an unreadable request or a proof that did not verify.
    pub rejected: u32,
    /// Connections closed unanswered (too many at once, or a peer that failed too often) or told
    /// `busy`.
    pub dropped: u32,
}

/// What `serve` came to.
#[derive(Debug)]
pub struct Served {
    pub outcome: Outcome,
    pub stats: Stats,
}

#[derive(Deserialize)]
struct Request {
    v: u32,
    key: String,
    device: String,
    mac: String,
}

enum Line {
    /// A whole line, without its terminator.
    Complete(Vec<u8>),
    /// Something arrived, but not a bounded whole line.
    Broken,
    /// Nothing arrived at all.
    Silent,
}

/// Reads one line of at most `limit` bytes, giving up at `until` (for the whole line, however
/// slowly it trickles in) or when `stop` is set.
fn read_line(
    connection: &mut dyn Connection,
    limit: usize,
    until: Instant,
    stop: &AtomicBool,
) -> Line {
    let mut line = Vec::new();
    let mut chunk = [0u8; 256];
    let gave_up = |line: &Vec<u8>| {
        if line.is_empty() {
            Line::Silent
        } else {
            Line::Broken
        }
    };
    loop {
        let wait = until.saturating_duration_since(Instant::now()).min(SLICE);
        if wait.is_zero() || stop.load(Ordering::Relaxed) || connection.set_timeout(wait).is_err() {
            return gave_up(&line);
        }
        let count = match connection.read(&mut chunk) {
            Ok(0) => return gave_up(&line),
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(_) => return gave_up(&line),
        };
        let end = chunk[..count].iter().position(|byte| *byte == b'\n');
        line.extend_from_slice(&chunk[..end.unwrap_or(count)]);
        if line.len() > limit {
            return Line::Broken;
        }
        if end.is_some() {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Line::Complete(line);
        }
    }
}

fn reply(connection: &mut dyn Connection, ok: bool, reason: &str) {
    let text = if ok {
        "{\"ok\":true}\n".to_owned()
    } else {
        format!("{{\"ok\":false,\"reason\":\"{reason}\"}}\n")
    };
    let _ = connection.set_timeout(IO_TIMEOUT);
    let _ = connection.write_all(text.as_bytes());
    let _ = connection.flush();
}

/// What a connection came to before anyone was asked anything.
enum Pre {
    /// It sent nothing (or hung up).
    Silent,
    /// It sent something that is not a verified request: unreadable, oversized, another
    /// version, or a proof that did not verify. Refused.
    Rejected,
    /// A verified request, but another one was already taken: told `busy`.
    Busy,
    /// A verified request that is the one taken: the attempt.
    Verified(Request),
}

/// The part of a connection that needs no thread-shared state beyond two flags: greet, read the
/// request, check its proof, and (for the first verified request only) claim the attempt.
fn pre_auth(
    connection: &mut dyn Connection,
    nonce: &[u8; 32],
    otp: &[u8; 16],
    until: Instant,
    stop: &AtomicBool,
    claimed: &AtomicBool,
) -> Pre {
    let hello = format!("{{\"v\":1,\"nonce\":\"{}\"}}\n", STANDARD.encode(nonce));
    let wait = until
        .saturating_duration_since(Instant::now())
        .min(IO_TIMEOUT);
    let greeted = !wait.is_zero()
        && connection.set_timeout(wait).is_ok()
        && connection.write_all(hello.as_bytes()).is_ok()
        && connection.flush().is_ok();
    if !greeted {
        return Pre::Silent;
    }

    let line = match read_line(connection, REQUEST_LIMIT, until, stop) {
        Line::Silent => return Pre::Silent,
        Line::Broken => {
            reply(connection, false, "request");
            return Pre::Rejected;
        }
        Line::Complete(line) => line,
    };
    let request = match serde_json::from_slice::<Request>(&line) {
        Ok(request) if request.v == 1 => request,
        _ => {
            reply(connection, false, "request");
            return Pre::Rejected;
        }
    };

    // The proof comes first, before anything about the key is looked at or shown. `verify_slice`
    // compares in constant time.
    let proven = STANDARD
        .decode(request.mac.as_bytes())
        .ok()
        .is_some_and(|mac| mac_of(otp, nonce, &request.key).verify_slice(&mac).is_ok());
    if !proven {
        reply(connection, false, "authentication");
        return Pre::Rejected;
    }
    // The peer knows the one-time password. Only the first such request is the attempt.
    if claimed.swap(true, Ordering::SeqCst) {
        reply(connection, false, "busy");
        return Pre::Busy;
    }
    Pre::Verified(request)
}

/// One connection, start to finish, on the calling thread: what `serve` does for each
/// connection, minus the threads. `None` when it was not the attempt.
pub fn attempt(
    connection: &mut dyn Connection,
    session: &Session<'_>,
    deadline: Instant,
) -> Attempt {
    let mut nonce = [0u8; 32];
    (session.random)(&mut nonce);
    let until = deadline.min(Instant::now() + session.limits.pre_auth);
    let stop = AtomicBool::new(false);
    let claimed = AtomicBool::new(false);
    match pre_auth(connection, &nonce, &session.otp, until, &stop, &claimed) {
        Pre::Silent => Attempt::Silent,
        Pre::Rejected | Pre::Busy => Attempt::Rejected,
        Pre::Verified(request) => Attempt::Over(authorize(connection, session, deadline, &request)),
    }
}

/// What one connection came to, for [`attempt`].
#[derive(Debug)]
pub enum Attempt {
    Silent,
    Rejected,
    Over(Outcome),
}

/// The authorization of a request whose proof verified: parse the key, ask the person, write.
fn authorize(
    connection: &mut dyn Connection,
    session: &Session<'_>,
    deadline: Instant,
    request: &Request,
) -> Outcome {
    let Ok(key) = KeyLine::parse(&request.key) else {
        reply(connection, false, "key");
        return Outcome::KeyRejected;
    };
    let device =
        authorized_keys::sanitize_device(&request.device).unwrap_or_else(|| "phone".to_owned());
    let fingerprint = key.fingerprint();

    let question = ConfirmRequest {
        user: session.account.name.clone(),
        target: authorized_keys::path(&session.account.home),
        device: device.clone(),
        fingerprint: fingerprint.clone(),
        algorithm: key.algorithm().to_owned(),
        peer: connection.peer(),
    };
    match session.confirm.confirm(&question, deadline) {
        Answer::Yes => {}
        Answer::No => {
            reply(connection, false, "declined");
            return Outcome::Declined {
                device,
                fingerprint,
            };
        }
        Answer::TimedOut => {
            reply(connection, false, "timeout");
            return Outcome::TimedOut;
        }
    }

    match authorized_keys::add(session.account, &key, &device, (session.now)()) {
        Ok(added) => {
            reply(connection, true, "");
            Outcome::Authorized {
                added,
                device,
                fingerprint,
            }
        }
        Err(error) => {
            reply(connection, false, "failed");
            Outcome::WriteFailed(error)
        }
    }
}

/// What a connection's thread reports back.
struct Report {
    peer: Option<IpAddr>,
    pre: Pre,
    /// Kept only for the verified request, which the listener's thread answers.
    connection: Option<Box<dyn Connection>>,
}

/// What the listener's thread knows about the connections it has handed out.
#[derive(Default)]
struct Book {
    stats: Stats,
    /// Refused requests, per peer address.
    failures: HashMap<Option<IpAddr>, u32>,
    /// Connections being handled, per peer address, and in all.
    active: HashMap<Option<IpAddr>, usize>,
    total: usize,
}

impl Book {
    /// Takes in what a finished connection reports; the verified request, if it was one.
    fn absorb(&mut self, report: Report) -> Option<(Box<dyn Connection>, Request)> {
        self.total -= 1;
        if let Some(count) = self.active.get_mut(&report.peer) {
            *count -= 1;
        }
        match report.pre {
            Pre::Silent => self.stats.silent += 1,
            Pre::Busy => self.stats.dropped += 1,
            Pre::Rejected => {
                self.stats.rejected += 1;
                *self.failures.entry(report.peer).or_default() += 1;
            }
            Pre::Verified(request) => return report.connection.map(|c| (c, request)),
        }
        None
    }

    /// Whether a connection from `peer` is not taken on: too many at once, or too many refusals.
    fn refuses(&self, peer: Option<IpAddr>, limits: &Limits) -> bool {
        self.failures.get(&peer).copied().unwrap_or(0) >= limits.max_failures_per_peer
            || self.total >= limits.max_active
            || self.active.get(&peer).copied().unwrap_or(0) >= limits.max_active_per_peer
    }
}

/// Serves connections until one makes the attempt or `deadline` passes.
///
/// Each accepted connection gets its own thread (bounded by `limits`), so an idle peer holds
/// nothing but its own slot. The person is asked, and the file written, on the calling thread.
pub fn serve(listener: &mut dyn PairListener, session: &Session<'_>, deadline: Instant) -> Served {
    let limits = session.limits;
    let (sender, receiver) = mpsc::channel::<Report>();
    let stop = AtomicBool::new(false);
    let claimed = AtomicBool::new(false);
    let mut book = Book::default();

    let outcome = std::thread::scope(|scope| {
        let outcome = loop {
            // The window is checked before every accept and again after it: a socket that was
            // queued before the end but is returned after it is closed, not greeted.
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break Outcome::TimedOut;
            }
            let connection = match listener.accept(Instant::now() + remaining.min(SLICE)) {
                Ok(Some(connection)) => Some(connection),
                Ok(None) => None,
                Err(error) => break Outcome::ListenFailed(error),
            };
            // What finished meanwhile counts before the new connection is judged.
            let mut verified = None;
            while let Ok(report) = receiver.try_recv() {
                verified = verified.or_else(|| book.absorb(report));
            }
            if let Some((mut winner, request)) = verified {
                break authorize(winner.as_mut(), session, deadline, &request);
            }
            let Some(connection) = connection else {
                continue;
            };
            if Instant::now() >= deadline {
                break Outcome::TimedOut;
            }
            let peer = connection.peer_ip();
            if book.refuses(peer, &limits) {
                book.stats.dropped += 1;
                continue;
            }
            book.total += 1;
            *book.active.entry(peer).or_default() += 1;
            let mut nonce = [0u8; 32];
            (session.random)(&mut nonce);
            let until = deadline.min(Instant::now() + limits.pre_auth);
            let otp = session.otp;
            let (sender, stop, claimed) = (sender.clone(), &stop, &claimed);
            scope.spawn(move || {
                let mut connection = connection;
                let pre = pre_auth(connection.as_mut(), &nonce, &otp, until, stop, claimed);
                let keep = matches!(pre, Pre::Verified(_)).then_some(connection);
                let _ = sender.send(Report {
                    peer,
                    pre,
                    connection: keep,
                });
            });
        };
        // Let every connection thread finish: they look at this flag between short reads.
        stop.store(true, Ordering::Relaxed);
        outcome
    });
    Served {
        outcome,
        stats: book.stats,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::SocketAddr;

    const OTP: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
    const PHONE: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBCJz8goXA2qGjRTNHhOwsljhOuCXG/+2B/zTJH/brc5";

    /// A connection that reads from a script and records what was written.
    struct Script {
        input: VecDeque<Vec<u8>>,
        output: Vec<u8>,
        /// After the script runs out: hang up (EOF) or time out.
        then_timeout: bool,
        peer: String,
    }

    impl Script {
        fn new(input: &[&[u8]]) -> Self {
            Self {
                input: input.iter().map(|chunk| chunk.to_vec()).collect(),
                output: Vec::new(),
                then_timeout: false,
                peer: "192.168.1.50:5555".into(),
            }
        }

        /// A peer that connects and then says nothing, as long as it is let.
        fn idle(peer: &str) -> Self {
            Self {
                then_timeout: true,
                peer: peer.into(),
                ..Self::new(&[])
            }
        }
    }

    /// Short per-connection limits, so tests with silent peers do not wait for the real 8 s.
    fn fast() -> Limits {
        Limits {
            pre_auth: Duration::from_millis(300),
            ..Limits::default()
        }
    }

    impl Read for Script {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.input.pop_front() {
                Some(mut chunk) => {
                    let count = chunk.len().min(buf.len());
                    buf[..count].copy_from_slice(&chunk[..count]);
                    if count < chunk.len() {
                        self.input.push_front(chunk.split_off(count));
                    }
                    Ok(count)
                }
                None if self.then_timeout => {
                    // A real socket waits for its timeout before it reports one.
                    std::thread::sleep(Duration::from_millis(5));
                    Err(io::ErrorKind::TimedOut.into())
                }
                None => Ok(0),
            }
        }
    }

    impl Write for Script {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Connection for Script {
        fn peer(&self) -> String {
            self.peer.clone()
        }
        fn set_timeout(&mut self, _: Duration) -> io::Result<()> {
            Ok(())
        }
    }

    struct Answers {
        answer: Answer,
        asked: RefCell<Vec<ConfirmRequest>>,
    }

    impl Confirm for Answers {
        fn confirm(&self, request: &ConfirmRequest, _: Instant) -> Answer {
            self.asked.borrow_mut().push(request.clone());
            self.answer
        }
    }

    fn answers(answer: Answer) -> Answers {
        Answers {
            answer,
            asked: RefCell::default(),
        }
    }

    fn nonce_of(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    /// A request a phone holding `otp` would send for a host that sent `nonce`.
    fn request_for(otp: &[u8; 16], nonce: &[u8; 32], key: &str, device: &str) -> Vec<u8> {
        let mac = mac_of(otp, nonce, key).finalize().into_bytes();
        let mut line = serde_json::json!({
            "v": 1, "key": key, "device": device, "mac": STANDARD.encode(mac),
        })
        .to_string()
        .into_bytes();
        line.push(b'\n');
        line
    }

    struct Fixture {
        home: tempfile::TempDir,
        confirm: Answers,
        nonce: Cell<u8>,
    }

    impl Fixture {
        fn new(answer: Answer) -> Self {
            Self {
                home: tempfile::tempdir().unwrap(),
                confirm: answers(answer),
                nonce: Cell::new(7),
            }
        }

        fn run(&self, connection: &mut Script) -> Attempt {
            let random = |buf: &mut [u8]| buf.fill(self.nonce.get());
            let now = || DateTime::from_unix(1_782_867_661);
            let session = Session {
                account: &Account::new("alice", self.home.path()),
                otp: OTP,
                confirm: &self.confirm,
                random: &random,
                now: &now,
                limits: fast(),
            };
            attempt(
                connection,
                &session,
                Instant::now() + Duration::from_secs(60),
            )
        }

        fn authorized_keys(&self) -> Option<String> {
            fs::read_to_string(authorized_keys::path(self.home.path())).ok()
        }
    }

    fn written(connection: &Script) -> Vec<String> {
        String::from_utf8_lossy(&connection.output)
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn a_good_request_is_confirmed_authorized_and_acknowledged() {
        let fixture = Fixture::new(Answer::Yes);
        let request = request_for(&OTP, &nonce_of(7), PHONE, "Pixel 8");
        let mut connection = Script::new(&[&request]);
        let Attempt::Over(outcome) = fixture.run(&mut connection) else {
            panic!("not an attempt")
        };
        let Outcome::Authorized {
            added,
            device,
            fingerprint,
        } = outcome
        else {
            panic!("{outcome:?}")
        };
        assert!(matches!(added, Added::Added { created: true, .. }));
        assert_eq!(device, "Pixel-8");
        assert!(fingerprint.starts_with("SHA256:"));
        let lines = written(&connection);
        assert_eq!(
            lines[0],
            format!("{{\"v\":1,\"nonce\":\"{}\"}}", STANDARD.encode(nonce_of(7)))
        );
        assert_eq!(lines[1], "{\"ok\":true}");
        let keys = fixture.authorized_keys().unwrap();
        assert_eq!(
            keys,
            format!("no-agent-forwarding,no-X11-forwarding {PHONE} or2-Pixel-8-2026-07-01\n")
        );
        // The person was shown the fingerprint and the (sanitized) device before anything was written.
        let asked = fixture.confirm.asked.borrow();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].user, "alice");
        assert_eq!(asked[0].device, "Pixel-8");
        assert_eq!(asked[0].fingerprint, fingerprint);
        assert_eq!(asked[0].peer, "192.168.1.50:5555");
    }

    #[test]
    fn a_wrong_password_is_refused_before_anything_is_asked_or_written() {
        let fixture = Fixture::new(Answer::Yes);
        let mut wrong = OTP;
        wrong[15] ^= 1;
        let request = request_for(&wrong, &nonce_of(7), PHONE, "phone");
        let mut connection = Script::new(&[&request]);
        assert!(matches!(fixture.run(&mut connection), Attempt::Rejected));
        assert_eq!(
            written(&connection)[1],
            "{\"ok\":false,\"reason\":\"authentication\"}"
        );
        assert!(fixture.confirm.asked.borrow().is_empty());
        assert!(fixture.authorized_keys().is_none());
    }

    #[test]
    fn a_request_recorded_for_another_nonce_is_useless() {
        let fixture = Fixture::new(Answer::Yes);
        // A valid request for the nonce an earlier connection was given...
        let recorded = request_for(&OTP, &nonce_of(1), PHONE, "phone");
        // ...replayed to a connection whose nonce is different.
        let mut connection = Script::new(&[&recorded]);
        assert!(matches!(fixture.run(&mut connection), Attempt::Rejected));
        assert!(fixture.authorized_keys().is_none());
    }

    #[test]
    fn the_mac_covers_the_key_so_a_swapped_key_fails() {
        let fixture = Fixture::new(Answer::Yes);
        let other =
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7";
        let honest = request_for(&OTP, &nonce_of(7), PHONE, "phone");
        let mut value: serde_json::Value = serde_json::from_slice(&honest).unwrap();
        value["key"] = other.into();
        let swapped = format!("{value}\n");
        let mut connection = Script::new(&[swapped.as_bytes()]);
        assert!(matches!(fixture.run(&mut connection), Attempt::Rejected));
    }

    #[test]
    fn a_bad_mac_encoding_is_just_a_failed_proof() {
        let fixture = Fixture::new(Answer::Yes);
        for mac in ["", "!!!", "AAAA", "short"] {
            let line =
                format!("{{\"v\":1,\"key\":\"{PHONE}\",\"device\":\"d\",\"mac\":\"{mac}\"}}\n");
            let mut connection = Script::new(&[line.as_bytes()]);
            assert!(
                matches!(fixture.run(&mut connection), Attempt::Rejected),
                "{mac}"
            );
        }
    }

    #[test]
    fn the_person_can_decline() {
        let fixture = Fixture::new(Answer::No);
        let request = request_for(&OTP, &nonce_of(7), PHONE, "phone");
        let mut connection = Script::new(&[&request]);
        assert!(matches!(
            fixture.run(&mut connection),
            Attempt::Over(Outcome::Declined { .. })
        ));
        assert_eq!(
            written(&connection)[1],
            "{\"ok\":false,\"reason\":\"declined\"}"
        );
        assert!(fixture.authorized_keys().is_none());
    }

    #[test]
    fn nobody_answering_is_a_timeout_reply_and_writes_nothing() {
        let fixture = Fixture::new(Answer::TimedOut);
        let request = request_for(&OTP, &nonce_of(7), PHONE, "phone");
        let mut connection = Script::new(&[&request]);
        assert!(matches!(
            fixture.run(&mut connection),
            Attempt::Over(Outcome::TimedOut)
        ));
        assert_eq!(
            written(&connection)[1],
            "{\"ok\":false,\"reason\":\"timeout\"}"
        );
        assert!(fixture.authorized_keys().is_none());
    }

    #[test]
    fn a_key_that_is_not_authorizable_is_refused_after_the_proof() {
        let fixture = Fixture::new(Answer::Yes);
        for key in ["not a key", "ssh-dss AAAAB3NzaC1kc3M=", "ssh-ed25519 AAAA"] {
            let request = request_for(&OTP, &nonce_of(7), key, "phone");
            let mut connection = Script::new(&[&request]);
            assert!(
                matches!(
                    fixture.run(&mut connection),
                    Attempt::Over(Outcome::KeyRejected)
                ),
                "{key}"
            );
            assert_eq!(written(&connection)[1], "{\"ok\":false,\"reason\":\"key\"}");
        }
        assert!(fixture.confirm.asked.borrow().is_empty());
    }

    #[test]
    fn a_key_with_options_or_a_second_line_smuggles_nothing_into_the_file() {
        let fixture = Fixture::new(Answer::Yes);
        let evil = format!("{PHONE} x\ncommand=\"rm -rf ~\" ssh-rsa AAAA");
        let request = request_for(&OTP, &nonce_of(7), &evil, "ev\nil\u{1b}[31m");
        let mut connection = Script::new(&[&request]);
        assert!(matches!(
            fixture.run(&mut connection),
            Attempt::Over(Outcome::Authorized { .. })
        ));
        let keys = fixture.authorized_keys().unwrap();
        assert_eq!(keys.lines().count(), 1);
        assert!(!keys.contains("rm -rf"));
        assert!(keys.trim_end().ends_with("or2-ev-il-31m-2026-07-01"));
        assert!(!fixture.confirm.asked.borrow()[0].device.contains('\x1b'));
    }

    #[test]
    fn malformed_requests_get_a_request_refusal_and_are_not_the_attempt() {
        let fixture = Fixture::new(Answer::Yes);
        let huge = vec![b'x'; REQUEST_LIMIT + 10];
        let wrong_version = b"{\"v\":2,\"key\":\"k\",\"device\":\"d\",\"mac\":\"m\"}\n".to_vec();
        for input in [
            b"not json\n".to_vec(),
            b"{}\n".to_vec(),
            b"{\"v\":1}\n".to_vec(),
            wrong_version,
            // No newline before EOF.
            b"{\"v\":1".to_vec(),
            // Too long, with and without a newline.
            huge.clone(),
            [huge, b"\n".to_vec()].concat(),
        ] {
            let mut connection = Script::new(&[&input]);
            assert!(matches!(fixture.run(&mut connection), Attempt::Rejected));
            assert_eq!(
                written(&connection)[1],
                "{\"ok\":false,\"reason\":\"request\"}"
            );
        }
        assert!(fixture.authorized_keys().is_none());
    }

    #[test]
    fn a_request_arriving_in_pieces_is_read_whole() {
        let fixture = Fixture::new(Answer::Yes);
        let request = request_for(&OTP, &nonce_of(7), PHONE, "phone");
        let (a, rest) = request.split_at(10);
        let (b, c) = rest.split_at(40);
        let mut connection = Script::new(&[a, b, c]);
        assert!(matches!(
            fixture.run(&mut connection),
            Attempt::Over(Outcome::Authorized { .. })
        ));
    }

    #[test]
    fn a_connection_that_sends_nothing_is_not_an_attempt() {
        let fixture = Fixture::new(Answer::Yes);
        let mut hangs_up = Script::new(&[]);
        assert!(matches!(fixture.run(&mut hangs_up), Attempt::Silent));
        let mut stalls = Script::new(&[]);
        stalls.then_timeout = true;
        assert!(matches!(fixture.run(&mut stalls), Attempt::Silent));
        // It was still greeted, so a phone that connects and waits is not left guessing.
        assert_eq!(written(&hangs_up).len(), 1);
    }

    #[test]
    fn a_write_failure_is_reported_to_the_phone_and_the_person() {
        let fixture = Fixture::new(Answer::Yes);
        // ~/.ssh is a file: authorized_keys cannot be created under it.
        fs::write(fixture.home.path().join(".ssh"), "").unwrap();
        let request = request_for(&OTP, &nonce_of(7), PHONE, "phone");
        let mut connection = Script::new(&[&request]);
        assert!(matches!(
            fixture.run(&mut connection),
            Attempt::Over(Outcome::WriteFailed(_))
        ));
        assert_eq!(
            written(&connection)[1],
            "{\"ok\":false,\"reason\":\"failed\"}"
        );
    }

    /// A listener that hands out scripted connections, then reports the deadline.
    struct Queue(VecDeque<Script>);

    impl PairListener for Queue {
        fn endpoints(&self) -> Vec<SocketAddr> {
            Vec::new()
        }
        fn accept(&mut self, deadline: Instant) -> io::Result<Option<Box<dyn Connection>>> {
            if let Some(next) = self.0.pop_front() {
                // Connections arrive a little apart, so the one before has been dealt with.
                std::thread::sleep(Duration::from_millis(30));
                return Ok(Some(Box::new(next) as Box<dyn Connection>));
            }
            // Nothing queued: wait, as a real listener does, until the deadline it was given.
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            Ok(None)
        }
    }

    #[test]
    fn serve_skips_idle_connections_and_stops_after_the_first_attempt() {
        let fixture = Fixture::new(Answer::Yes);
        let good = request_for(&OTP, &nonce_of(7), PHONE, "phone");
        let second_key =
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7";
        let later = request_for(&OTP, &nonce_of(7), second_key, "other");
        let mut queue = Queue(VecDeque::from([
            Script::new(&[]),
            Script::new(&[&good]),
            // A second verified request: only one is taken, this one is told `busy`.
            Script::new(&[&later]),
        ]));
        let random = |buf: &mut [u8]| buf.fill(7);
        let now = || DateTime::from_unix(1_782_867_661);
        let session = Session {
            account: &Account::new("alice", fixture.home.path()),
            otp: OTP,
            confirm: &fixture.confirm,
            random: &random,
            now: &now,
            limits: fast(),
        };
        let outcome = serve(
            &mut queue,
            &session,
            Instant::now() + Duration::from_secs(60),
        )
        .outcome;
        assert!(matches!(outcome, Outcome::Authorized { .. }));
        let keys = fixture.authorized_keys().unwrap();
        assert_eq!(keys.lines().count(), 1, "one attempt, one line");
        assert!(keys.contains(PHONE), "the first verified request won");
    }

    #[test]
    fn serve_times_out_when_nobody_connects() {
        let fixture = Fixture::new(Answer::Yes);
        let mut queue = Queue(VecDeque::new());
        let random = |buf: &mut [u8]| buf.fill(7);
        let now = || DateTime::from_unix(0);
        let session = Session {
            account: &Account::new("alice", fixture.home.path()),
            otp: OTP,
            confirm: &fixture.confirm,
            random: &random,
            now: &now,
            limits: fast(),
        };
        let outcome = serve(&mut queue, &session, Instant::now()).outcome;
        assert!(matches!(outcome, Outcome::TimedOut));
    }

    // --- Finding 7: nothing is served after the window ---------------------------------------

    /// A script the test can still look at after `serve` took it.
    struct Shared(std::sync::Arc<std::sync::Mutex<Script>>);

    impl Read for Shared {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.lock().unwrap().read(buf)
        }
    }
    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Connection for Shared {
        fn peer(&self) -> String {
            self.0.lock().unwrap().peer.clone()
        }
        fn set_timeout(&mut self, _: Duration) -> io::Result<()> {
            Ok(())
        }
    }

    /// Hands out its connection after `delay`, like a socket that was already queued.
    struct LateQueue {
        connection: Option<Shared>,
        delay: Duration,
    }

    impl PairListener for LateQueue {
        fn endpoints(&self) -> Vec<SocketAddr> {
            Vec::new()
        }
        fn accept(&mut self, _: Instant) -> io::Result<Option<Box<dyn Connection>>> {
            std::thread::sleep(self.delay);
            Ok(self
                .connection
                .take()
                .map(|c| Box::new(c) as Box<dyn Connection>))
        }
    }

    #[test]
    fn a_connection_queued_when_the_window_is_over_is_not_greeted() {
        let fixture = Fixture::new(Answer::Yes);
        let good = request_for(&OTP, &nonce_of(7), PHONE, "phone");
        let script = std::sync::Arc::new(std::sync::Mutex::new(Script::new(&[&good])));
        let mut listener = LateQueue {
            connection: Some(Shared(script.clone())),
            delay: Duration::from_millis(80),
        };
        let random = |buf: &mut [u8]| buf.fill(7);
        let now = || DateTime::from_unix(1_782_867_661);
        let session = Session {
            account: &Account::new("alice", fixture.home.path()),
            otp: OTP,
            confirm: &fixture.confirm,
            random: &random,
            now: &now,
            limits: fast(),
        };
        // The window ends while `accept` is still waiting; the socket it then returns was queued
        // before the end, but the window is over: no greeting, no request read, nothing written.
        let deadline = Instant::now() + Duration::from_millis(20);
        let served = serve(&mut listener, &session, deadline);
        assert!(matches!(served.outcome, Outcome::TimedOut));
        assert!(script.lock().unwrap().output.is_empty(), "no hello");
        assert!(fixture.authorized_keys().is_none());
    }

    #[test]
    fn serve_does_not_accept_at_all_once_the_deadline_has_passed() {
        let fixture = Fixture::new(Answer::Yes);
        let mut queue = Queue(VecDeque::from([Script::new(&[]), Script::new(&[])]));
        let random = |buf: &mut [u8]| buf.fill(7);
        let now = || DateTime::from_unix(1_782_867_661);
        let session = Session {
            account: &Account::new("alice", fixture.home.path()),
            otp: OTP,
            confirm: &fixture.confirm,
            random: &random,
            now: &now,
            limits: fast(),
        };
        let past = Instant::now();
        std::thread::sleep(Duration::from_millis(5));
        assert!(matches!(
            serve(&mut queue, &session, past).outcome,
            Outcome::TimedOut
        ));
        assert_eq!(queue.0.len(), 2, "nothing was taken from the queue");
    }

    /// Serves `scripts` in order and returns what `serve` decided and what was left unserved.
    fn serve_scripts(fixture: &Fixture, scripts: Vec<Script>) -> (Outcome, usize) {
        let mut queue = Queue(VecDeque::from(scripts));
        let random = |buf: &mut [u8]| buf.fill(7);
        let now = || DateTime::from_unix(1_782_867_661);
        let session = Session {
            account: &Account::new("alice", fixture.home.path()),
            otp: OTP,
            confirm: &fixture.confirm,
            random: &random,
            now: &now,
            limits: fast(),
        };
        let outcome = serve(
            &mut queue,
            &session,
            Instant::now() + Duration::from_millis(1500),
        )
        .outcome;
        (outcome, queue.0.len())
    }

    // --- Finding 3: only a verified request is the attempt ---------------------------------

    #[test]
    fn junk_and_wrong_proofs_before_the_phone_do_not_use_up_the_attempt() {
        let fixture = Fixture::new(Answer::Yes);
        let good = request_for(&OTP, &nonce_of(7), PHONE, "phone");
        let mut wrong = OTP;
        wrong[0] ^= 1;
        let bad_mac = request_for(&wrong, &nonce_of(7), PHONE, "phone");
        let (outcome, _) = serve_scripts(
            &fixture,
            vec![
                // A scanner's probes: a bare newline, text, an unfinished request, a wrong proof.
                Script::new(&[b"\n"]),
                Script::new(&[b"GET / HTTP/1.1\r\n\r\n"]),
                Script::new(&[b"{\"v\":1"]),
                Script::new(&[&bad_mac]),
                Script::new(&[&good]),
                Script::new(&[]),
            ],
        );
        assert!(matches!(outcome, Outcome::Authorized { .. }), "{outcome:?}");
        assert_eq!(fixture.authorized_keys().unwrap().lines().count(), 1);
    }

    #[test]
    fn a_peer_that_keeps_failing_is_dropped_after_a_few_tries() {
        let fixture = Fixture::new(Answer::Yes);
        let good = request_for(&OTP, &nonce_of(7), PHONE, "phone");
        let junk = || Script::new(&[b"junk\n"]);
        // Every script is from the same peer address (see `Script::peer`): after
        // MAX_FAILURES_PER_PEER refusals its next connection is closed without a greeting, even
        // a correct one.
        let mut scripts: Vec<Script> = (0..MAX_FAILURES_PER_PEER).map(|_| junk()).collect();
        scripts.push(Script::new(&[&good]));
        let (outcome, left) = serve_scripts(&fixture, scripts);
        assert!(matches!(outcome, Outcome::TimedOut), "{outcome:?}");
        assert_eq!(left, 0);
        assert!(fixture.authorized_keys().is_none());
    }

    // --- Finding 4: idle peers hold nothing but their own slot ---------------------------

    /// Serves `listener` with the given limits for `window`.
    fn serve_with(
        fixture: &Fixture,
        listener: &mut dyn PairListener,
        limits: Limits,
        window: Duration,
    ) -> Served {
        let random = |buf: &mut [u8]| buf.fill(7);
        let now = || DateTime::from_unix(1_782_867_661);
        let session = Session {
            account: &Account::new("alice", fixture.home.path()),
            otp: OTP,
            confirm: &fixture.confirm,
            random: &random,
            now: &now,
            limits,
        };
        serve(listener, &session, Instant::now() + window)
    }

    #[test]
    fn a_phone_is_served_while_idle_peers_wait_out_their_time() {
        let fixture = Fixture::new(Answer::Yes);
        let good = request_for(&OTP, &nonce_of(7), PHONE, "phone");
        let mut queue = Queue(VecDeque::from([
            Script::idle("192.168.1.60:1000"),
            Script::idle("192.168.1.60:1001"),
            Script::idle("192.168.1.61:1000"),
            Script::new(&[&good]),
        ]));
        let limits = Limits {
            pre_auth: Duration::from_secs(3),
            ..Limits::default()
        };
        let started = Instant::now();
        let served = serve_with(&fixture, &mut queue, limits, Duration::from_secs(20));
        assert!(
            matches!(served.outcome, Outcome::Authorized { .. }),
            "{:?}",
            served.outcome
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the idle peers' 3 s did not hold the phone up: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_peer_holds_only_a_few_slots_and_the_rest_are_closed_unanswered() {
        let fixture = Fixture::new(Answer::Yes);
        let late = std::sync::Arc::new(std::sync::Mutex::new(Script::idle("192.168.1.60:9")));
        // Three idle connections from one address with room for two: the third is closed at once.
        let mut scripts = VecDeque::from([
            Script::idle("192.168.1.60:1"),
            Script::idle("192.168.1.60:2"),
        ]);
        let mut feeder = Feed {
            scripts: &mut scripts,
            last: Some(Shared(late.clone())),
        };
        let limits = Limits {
            pre_auth: Duration::from_millis(400),
            max_active_per_peer: 2,
            ..Limits::default()
        };
        let served = serve_with(&fixture, &mut feeder, limits, Duration::from_millis(900));
        assert!(matches!(served.outcome, Outcome::TimedOut));
        assert_eq!(served.stats.dropped, 1, "{:?}", served.stats);
        assert_eq!(served.stats.silent, 2, "{:?}", served.stats);
        assert!(late.lock().unwrap().output.is_empty(), "never greeted");
    }

    /// Hands out scripts, then one more `last`.
    struct Feed<'a> {
        scripts: &'a mut VecDeque<Script>,
        last: Option<Shared>,
    }

    impl PairListener for Feed<'_> {
        fn endpoints(&self) -> Vec<SocketAddr> {
            Vec::new()
        }
        fn accept(&mut self, deadline: Instant) -> io::Result<Option<Box<dyn Connection>>> {
            std::thread::sleep(Duration::from_millis(30));
            if let Some(next) = self.scripts.pop_front() {
                return Ok(Some(Box::new(next)));
            }
            if let Some(last) = self.last.take() {
                return Ok(Some(Box::new(last)));
            }
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            Ok(None)
        }
    }

    #[test]
    fn at_most_so_many_connections_are_handled_at_once() {
        let fixture = Fixture::new(Answer::Yes);
        let late = std::sync::Arc::new(std::sync::Mutex::new(Script::idle("192.168.1.99:9")));
        let mut scripts = VecDeque::from([
            Script::idle("192.168.1.60:1"),
            Script::idle("192.168.1.61:1"),
        ]);
        let mut feeder = Feed {
            scripts: &mut scripts,
            last: Some(Shared(late.clone())),
        };
        let limits = Limits {
            pre_auth: Duration::from_millis(400),
            max_active: 2,
            ..Limits::default()
        };
        let served = serve_with(&fixture, &mut feeder, limits, Duration::from_millis(900));
        assert_eq!(served.stats.dropped, 1, "{:?}", served.stats);
        assert!(late.lock().unwrap().output.is_empty(), "never greeted");
    }
}
