//! The mosh session driver: one dedicated thread that owns the terminal and the protocol state
//! and moves datagrams, with the standard session lifecycle (`Connecting`, `Connected`,
//! `Closed`) and the same frame, input, resize, scroll and disconnect behaviour as the SSH
//! driver.
//!
//! mosh has no handshake of its own (the SSH bootstrap agreed the key), so the session becomes
//! `Connected` when the first datagram from the server authenticates, and fails with `TimedOut`
//! if none does within [`CONNECT_TIMEOUT`]. After that it never gives up on its own: surviving a
//! dead network is mosh's point. It ends when the server announces the end of the session
//! (`RemoteExited`), the user disconnects, or something internal breaks.
//!
//! [`run_session`] runs a session on a [`SessionDriver`] the caller already holds: the host
//! driver, which bootstrapped the server over its SSH connection and must clean up after a
//! session that never connected.

use std::future::{Future, poll_fn};
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::{Instant, sleep, sleep_until, timeout_at};

use crate::session::{CloseReason, Command, SessionDriver, SessionFailure, SessionState};
use crate::submit::SubmitSequencer;
use crate::transport::DatagramTransport;

use super::bootstrap::MoshParams;
use super::ghostty::GhosttyScreen;
use super::link::{Link, is_too_large};
use super::ssp::crypto::RECEIVE_MTU;
use super::ssp::session::{Fault, LinkHealth, Session};

/// How long to wait for the server's first datagram before failing the session.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a disconnect waits for the server to acknowledge the shutdown before giving up.
pub(crate) const GOODBYE_TIMEOUT: Duration = Duration::from_secs(1);
/// The least time between two link-health reports to the session's observer.
const HEALTH_INTERVAL: Duration = Duration::from_secs(1);
/// Silence from the server past which the link counts as stale (the grey-out threshold the UI
/// uses), in milliseconds.
pub(crate) const STALE_AFTER_MS: u64 = 5000;
/// The least time between two looks at a healthy link that is about to turn stale.
const MIN_HEALTH_CHECK: Duration = Duration::from_millis(100);
/// How long to wait before trying to open another socket after one failed.
const REBIND_RETRY: Duration = Duration::from_secs(1);
/// How long opening another socket may take, name resolution included, before it is given up
/// and retried. It runs beside the rest of the session, so this only bounds how long a rotation
/// can be stuck, not how long the session is unresponsive.
const REBIND_TIMEOUT: Duration = Duration::from_secs(3);
/// How many datagrams are taken in one go before frames and commands get a turn.
const MAX_DATAGRAMS_PER_TURN: usize = 64;
/// The pause after the network refused a receive (ICMP unreachable), which ends that turn.
const REFUSED_PAUSE: Duration = Duration::from_millis(20);

/// Everything one mosh session needs besides its [`SessionDriver`].
pub struct Plan<T: DatagramTransport> {
    pub transport: Arc<T>,
    /// The pinned server address: the IP the SSH connection reached and the bootstrap's port.
    pub peer: SocketAddr,
    pub params: MoshParams,
    /// Ends the session like [`Command::Disconnect`] (with the shutdown handshake); a permit
    /// given before the session reads it is kept. The host driver gives it when the user
    /// disconnects the host.
    pub shutdown: Arc<Notify>,
    /// How long to wait for the server's first datagram, socket open included, counted from
    /// the start of the session. Used when `deadline` is `None`.
    pub connect_timeout: Duration,
    /// An absolute moment by which the session must be `Connected`, set by a caller that has
    /// already spent part of its allowance (the host driver's bootstrap): it replaces
    /// `connect_timeout` for the socket open and the first datagram. A moment already past
    /// fails the session `TimedOut` at once.
    pub deadline: Option<Instant>,
}

/// How a session ended: the reason the user sees, and whether the server is KNOWN to have
/// ended too.
#[derive(Debug)]
pub struct Ended {
    pub reason: CloseReason,
    /// The peer itself confirmed the end (it acknowledged our goodbye, or announced its own).
    /// A goodbye that timed out or failed leaves this false although the reason is still
    /// `Disconnected`: the server may be running, and a caller that can reach it another way
    /// (the SSH connection that started it) must stop it.
    pub server_gone: bool,
}

impl Ended {
    fn unconfirmed(reason: CloseReason) -> Self {
        Self {
            reason,
            server_gone: false,
        }
    }
}

/// Runs the session on `driver` until it ends and says why. Does NOT close the driver: the
/// caller does, after whatever cleanup it owes (`driver.state()` is still `Connected` if the
/// session ever was, so a caller can tell a start that never connected).
pub async fn run_session<T: DatagramTransport>(plan: Plan<T>, driver: &mut SessionDriver) -> Ended {
    match run(plan, driver).await {
        Ok(ended) => ended,
        Err(failure) => Ended::unconfirmed(CloseReason::Failed(failure)),
    }
}

fn internal(error: impl std::fmt::Display) -> SessionFailure {
    SessionFailure::Internal(error.to_string())
}

async fn run<T: DatagramTransport>(
    plan: Plan<T>,
    driver: &mut SessionDriver,
) -> Result<Ended, SessionFailure> {
    let Plan {
        transport,
        peer,
        params,
        shutdown,
        connect_timeout,
        deadline: absolute,
    } = plan;
    let key = params.key.to_base64_key().map_err(internal)?;
    let screen = GhosttyScreen::new(params.size).map_err(internal)?;
    // One allowance for the socket open and the first datagram together, computed once.
    let deadline = absolute.unwrap_or_else(|| Instant::now() + connect_timeout);
    // Opening the first socket resolves the host name, which can take as long as the resolver
    // does: a disconnect (or a dropped handle) must not wait for it. A resize is remembered.
    let mut size = params.size;
    let open = timeout_at(deadline, Link::open(transport, peer));
    tokio::pin!(open);
    let mut link = loop {
        tokio::select! {
            opened = &mut open => match opened {
                Ok(Ok(link)) => break link,
                Ok(Err(error)) => {
                    return Err(SessionFailure::Unreachable(format!(
                        "UDP socket failed ({:?}): {error}",
                        error.kind()
                    )));
                }
                Err(_) => return Err(SessionFailure::TimedOut),
            },
            command = driver.next_command() => match command {
                Command::Disconnect => return Ok(Ended::unconfirmed(CloseReason::Disconnected)),
                Command::Resize(new) => size = new,
                // Nothing to send to yet.
                _ => {}
            },
            () = shutdown.notified() => return Ok(Ended::unconfirmed(CloseReason::Disconnected)),
        }
    };
    let mut session = Session::new(&key, link.peer_is_ipv6(), screen);
    // The server starts at 80x24 whatever we want; say so in the first datagram.
    session
        .resize(size.rows(), size.columns())
        .map_err(internal)?;

    let mut buffer = [0u8; RECEIVE_MTU];
    let mut connected = false;
    let mut published = u64::MAX;
    let mut next_rebind = Instant::now();
    // The session observer's reports start when the session connects (see `HealthThrottle`).
    let mut next_report: Option<Instant> = None;
    let mut throttle = HealthThrottle::default();
    // The socket being opened for a rebind, and when to give up on it.
    let mut opening: Option<Opening<T>> = None;
    let mut submits = SubmitSequencer::new();
    let mut opening_deadline = Instant::now();

    loop {
        let tick = session.tick().map_err(internal)?;
        send(&link, &mut session, &tick.datagrams);
        if tick.rebind && opening.is_none() && Instant::now() >= next_rebind {
            opening = Some(Box::pin(link.next_socket()));
            opening_deadline = Instant::now() + REBIND_TIMEOUT;
        }
        if let Some(due) = next_report
            && Instant::now() >= due
        {
            let now = Instant::now();
            let sample = session.link_health();
            if let Some(report) = throttle.offer(now, sample) {
                driver.publish_link_health(report);
            }
            next_report = Some(throttle.next_check(now, sample));
        }

        // Wake for the next health duty only: an idle, healthy session sleeps until its own
        // protocol timers or the moment its link would turn stale.
        let mut wait = Duration::from_millis(session.wait_time_ms().max(1));
        if let Some(due) = next_report {
            wait = wait.min(due.saturating_duration_since(Instant::now()));
        }
        let wait = wait.max(Duration::from_millis(1));
        tokio::select! {
            received = poll_fn(|cx| link.poll_recv(cx, &mut buffer)) => {
                let mut taken = 0;
                let mut next = Some(received);
                while let Some(result) = next.take() {
                    match result {
                        Ok(length) => {
                            match session.handle_datagram(&buffer[..length]) {
                                Ok(_) => {
                                    link.authenticated(std::time::Instant::now());
                                    if !connected {
                                        connected = true;
                                        next_report = Some(Instant::now());
                                        driver
                                            .transition(SessionState::Connected)
                                            .map_err(internal)?;
                                    }
                                    // Taken per datagram: a later one may replace the live
                                    // screen (a diff from an older state) and its write with it.
                                    if let Some(text) =
                                        session.terminal().live().engine().take_clipboard_write()
                                    {
                                        driver.publish_clipboard(text);
                                    }
                                }
                                Err(Fault::Dropped(_)) => {}
                                Err(Fault::Screen(error)) => return Err(internal(error)),
                            }
                        }
                        // The network says no (a refused port, an unreachable route). mosh
                        // keeps trying, so this is not an end; just do not spin on it, and end
                        // the turn so commands (a disconnect) are not held behind refusals.
                        Err(_) => {
                            sleep(REFUSED_PAUSE).await;
                            break;
                        }
                    }
                    taken += 1;
                    if taken < MAX_DATAGRAMS_PER_TURN {
                        next = link.try_recv(&mut buffer);
                    }
                }
                if connected {
                    publish_if_changed(driver, &mut session, &mut published)?;
                }
                if session.peer_shut_down() {
                    // Acknowledge the server's goodbye, then we are done.
                    let tick = session.tick().map_err(internal)?;
                    send(&link, &mut session, &tick.datagrams);
                    return Ok(Ended {
                        reason: CloseReason::RemoteExited { exit_status: None },
                        server_gone: true,
                    });
                }
            }
            () = submits.due() => {
                submits.disarm();
                let bytes = session.terminal().live().engine().submit_enter_bytes().map_err(internal)?;
                session.send_input(&bytes);
                while let Some(command) = submits.next_deferred() {
                    apply_input(command, driver, &mut session, connected, &mut submits)?;
                }
            }
            command = driver.next_command() => {
                if matches!(command, Command::Disconnect) {
                    let server_gone = goodbye(&mut link, &mut session, &mut buffer).await;
                    return Ok(Ended { reason: CloseReason::Disconnected, server_gone });
                }
                if let Some(command) = submits.admit(command) {
                    apply_input(command, driver, &mut session, connected, &mut submits)?;
                }
            }
            opened = poll_fn(|cx| match opening.as_mut() {
                Some(opening) => opening.as_mut().poll(cx),
                None => std::task::Poll::Pending,
            }), if opening.is_some() => {
                opening = None;
                match opened.and_then(|socket| link.adopt(socket)) {
                    Ok(()) => session.note_rebound(),
                    Err(_) => next_rebind = Instant::now() + REBIND_RETRY,
                }
            }
            () = sleep_until(opening_deadline), if opening.is_some() => {
                // A resolver that never answers: drop the attempt and try again shortly.
                opening = None;
                next_rebind = Instant::now() + REBIND_RETRY;
            }
            () = shutdown.notified() => {
                let server_gone = goodbye(&mut link, &mut session, &mut buffer).await;
                return Ok(Ended { reason: CloseReason::Disconnected, server_gone });
            }
            () = sleep(wait) => {}
            () = sleep_until(deadline), if !connected => {
                return Err(SessionFailure::TimedOut);
            }
        }
    }
}

/// Decides which link-health samples reach the session's observer: at most one a second, and
/// only when what the UI shows changes. That is the first sample, the link turning stale
/// (`since_heard_ms` past [`STALE_AFTER_MS`]), each further whole second of silence while it is
/// stale, and its recovery. A healthy link is never reported again: its millisecond-exact
/// numbers change on every sample and would wake the app once a second per session for
/// nothing. `since_ack_ms` alone does not count.
#[derive(Default)]
pub(crate) struct HealthThrottle {
    /// When the last report went out, and what it said (see [`HealthThrottle::condition`]).
    last: Option<(Instant, Option<u64>)>,
}

impl HealthThrottle {
    /// What a report is about: `None` for a healthy link, else the whole seconds of silence.
    fn condition(sample: LinkHealth) -> Option<u64> {
        (sample.since_heard_ms > STALE_AFTER_MS).then_some(sample.since_heard_ms / 1000)
    }

    /// `sample` if it is due, else `None`. A sample that is suppressed is not remembered: the
    /// next one is compared with what was last reported.
    pub(crate) fn offer(&mut self, now: Instant, sample: LinkHealth) -> Option<LinkHealth> {
        let condition = Self::condition(sample);
        if let Some((at, last)) = &self.last
            && (now.saturating_duration_since(*at) < HEALTH_INTERVAL || *last == condition)
        {
            return None;
        }
        self.last = Some((now, condition));
        Some(sample)
    }

    /// When to offer the next sample: a second on while stale, and for a healthy link when it
    /// would turn stale if nothing more were heard (an earlier datagram only moves that later,
    /// and the look finds it healthy again). Never before the interval since the last report.
    pub(crate) fn next_check(&self, now: Instant, sample: LinkHealth) -> Instant {
        let natural = match Self::condition(sample) {
            Some(_) => now + HEALTH_INTERVAL,
            None => {
                let left = STALE_AFTER_MS + 1 - sample.since_heard_ms.min(STALE_AFTER_MS);
                now + Duration::from_millis(left).max(MIN_HEALTH_CHECK)
            }
        };
        match &self.last {
            Some((at, _)) => natural.max(*at + HEALTH_INTERVAL),
            None => natural,
        }
    }
}

/// Runs every command but `Disconnect`. Input held behind a submit's Enter reaches here later.
fn apply_input(
    command: Command,
    driver: &mut SessionDriver,
    session: &mut Session<GhosttyScreen>,
    connected: bool,
    submits: &mut SubmitSequencer,
) -> Result<(), SessionFailure> {
    if let Some(bytes) = session
        .terminal()
        .live()
        .engine()
        .input_bytes(&command)
        .map_err(internal)?
        && !bytes.is_empty()
    {
        session.send_input(&bytes);
    }
    match command {
        // The network changed: rotate to a new socket now.
        Command::Roam => session.request_rebind(),
        Command::Resize(size) => {
            session
                .resize(size.rows(), size.columns())
                .map_err(internal)?;
            if connected {
                publish(driver, session)?;
            }
        }
        // Its Enter follows, after the delay.
        Command::Submit(_) => submits.arm(),
        Command::Scroll(_) => publish(driver, session)?,
        Command::FullFrame => {
            session.terminal().live().engine().request_full_frame();
            publish(driver, session)?;
        }
        // Input is written above; a disconnect is the caller's, which owns the link.
        _ => {}
    }
    Ok(())
}

/// A socket being opened for a rebind.
type Opening<T> =
    Pin<Box<dyn Future<Output = io::Result<<T as DatagramTransport>::Socket>> + Send>>;

/// Sends datagrams from the newest socket. A refused or dropped send costs nothing the protocol
/// does not already repair, since it resends state; only "too large" changes behaviour.
fn send<T: DatagramTransport>(
    link: &Link<T>,
    session: &mut Session<GhosttyScreen>,
    datagrams: &[Vec<u8>],
) {
    for datagram in datagrams {
        if let Err(error) = link.send(datagram)
            && is_too_large(&error)
        {
            session.datagram_too_large();
        }
    }
}

/// Tells the server the client is leaving, as mosh does, so its session ends instead of lingering
/// on the host. Bounded: a server that is already gone must not hold the disconnect. True only
/// if the peer confirmed the end; a timeout or an error is false (see [`Ended::server_gone`]).
async fn goodbye<T: DatagramTransport>(
    link: &mut Link<T>,
    session: &mut Session<GhosttyScreen>,
    buffer: &mut [u8],
) -> bool {
    session.shutdown();
    let deadline = Instant::now() + GOODBYE_TIMEOUT;
    while !session.finished() && Instant::now() < deadline {
        let Ok(tick) = session.tick() else {
            return false;
        };
        send(link, session, &tick.datagrams);
        let wait = Duration::from_millis(session.wait_time_ms().clamp(1, 50));
        tokio::select! {
            received = poll_fn(|cx| link.poll_recv(cx, buffer)) => match received {
                Ok(length) => {
                    let _ = session.handle_datagram(&buffer[..length]);
                }
                // A refusal is no answer; do not spin on it.
                Err(_) => sleep(wait.min(REFUSED_PAUSE)).await,
            },
            () = sleep(wait) => {}
            () = sleep_until(deadline) => {}
        }
    }
    session.shutdown_confirmed()
}

fn publish_if_changed(
    driver: &mut SessionDriver,
    session: &mut Session<GhosttyScreen>,
    published: &mut u64,
) -> Result<(), SessionFailure> {
    let latest = session.terminal().latest();
    if *published == latest {
        return Ok(());
    }
    *published = latest;
    publish(driver, session)
}

fn publish(
    driver: &mut SessionDriver,
    session: &mut Session<GhosttyScreen>,
) -> Result<(), SessionFailure> {
    let frame = session
        .terminal()
        .live()
        .engine()
        .frame()
        .map_err(internal)?;
    driver.publish(frame).map_err(internal)
}

#[cfg(test)]
#[path = "driver_tests.rs"]
mod tests;
