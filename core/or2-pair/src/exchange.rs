//! The exchange between the phone and `or2-pair enroll <id>`, the command sshd runs for the
//! bootstrap key (Unix only).
//!
//! Newline-delimited JSON over the SSH session channel (the forced command's standard input and
//! output, no terminal):
//!
//! ```text
//! host  -> {"v":2,"hello":"or2-pair","id":"<id>"}
//! phone -> {"v":2,"key":"<algo> <base64>","device":"<label>"}
//! host  -> {"v":2,"ok":true,"user":"<account>","fingerprint":"SHA256:…"}
//!        | {"v":2,"ok":false,"reason":"expired|gone|key|failed|request"}
//! ```
//!
//! There are no MACs: SSH authenticates both ends (the host by the pinned `hk`, the phone by the
//! bootstrap key only a holder of `K` can derive) and encrypts the channel. The hello is sent
//! first and the request is read before any refusal, so the phone always receives a verdict
//! rather than a closed channel.
//!
//! What the forced command does, in order: send the hello; read one bounded request line
//! (10 s); check the state file (`expired` when it is missing, past its deadline, or names
//! another uid); validate the key (`key`); in one locked write replace the bootstrap entry with
//! the phone's key (`gone` when the entry is not there: another phone was first); record
//! `<id>.done`; answer. **The security boundary is the state file and its deadline, not the
//! cleanup.**

use std::io::{self, Read, Write};
use std::sync::mpsc;
use std::time::Duration;

use serde::Deserialize;

use crate::account::Account;
use crate::authorized_keys::{self, Replaced};
use crate::bootstrap::PairingId;
use crate::date::DateTime;
use crate::keyline::KeyLine;
use crate::state::{Done, StateDir};

pub const VERSION: u32 = 2;
/// The request line is at most this many bytes.
pub const REQUEST_LIMIT: usize = 2048;
/// The phone has this long to send it.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a second phone waits for the first one's write before giving up.
const LOCK_PATIENCE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// No live run: missing, past its deadline, or another account's state.
    Expired,
    /// The bootstrap entry is not there any more: another device was first.
    Gone,
    /// Not a key this tool authorizes.
    Key,
    /// This host could not write `authorized_keys`.
    Failed,
    /// An unreadable, oversized, late or foreign-version request.
    Request,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::Gone => "gone",
            Self::Key => "key",
            Self::Failed => "failed",
            Self::Request => "request",
        }
    }
}

/// What `enroll` came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The phone's key is in `authorized_keys` (`added` is false when it already was).
    Installed {
        device: String,
        fingerprint: String,
        added: bool,
    },
    Refused(Reason),
    /// The channel went away before there was anything to say.
    Aborted,
}

pub fn hello(id: &PairingId) -> String {
    format!("{{\"v\":{VERSION},\"hello\":\"or2-pair\",\"id\":\"{id}\"}}\n")
}

fn verdict_ok(user: &str, fingerprint: &str) -> String {
    format!(
        "{{\"v\":{VERSION},\"ok\":true,\"user\":{},\"fingerprint\":{}}}\n",
        serde_json::Value::from(user),
        serde_json::Value::from(fingerprint)
    )
}

fn verdict_refused(reason: Reason) -> String {
    format!(
        "{{\"v\":{VERSION},\"ok\":false,\"reason\":\"{}\"}}\n",
        reason.as_str()
    )
}

#[derive(Deserialize)]
struct Request {
    v: u32,
    key: String,
    device: String,
}

/// One line of at most `limit` bytes, from `input`, within `timeout` (for the whole line,
/// however slowly it trickles in). The read happens on its own thread, which ends with the
/// process if the peer never sends.
fn read_line(
    input: Box<dyn Read + Send>,
    limit: usize,
    timeout: Duration,
) -> Result<Vec<u8>, &'static str> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut input = input;
        let mut line = Vec::new();
        let mut chunk = [0u8; 256];
        let result = loop {
            let count = match input.read(&mut chunk) {
                Ok(0) => break Err("the channel closed before a whole line"),
                Ok(count) => count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break Err("the channel failed"),
            };
            let end = chunk[..count].iter().position(|byte| *byte == b'\n');
            line.extend_from_slice(&chunk[..end.unwrap_or(count)]);
            if line.len() > limit {
                break Err("the line is too long");
            }
            if end.is_some() {
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                break Ok(line);
            }
        };
        let _ = sender.send(result);
    });
    receiver
        .recv_timeout(timeout)
        .unwrap_or(Err("no line in time"))
}

pub struct Enroll<'a> {
    pub account: &'a Account,
    pub now: &'a dyn Fn() -> DateTime,
    pub request_timeout: Duration,
}

fn say(output: &mut dyn Write, text: &str) -> bool {
    output
        .write_all(text.as_bytes())
        .and_then(|()| output.flush())
        .is_ok()
}

fn refuse(output: &mut dyn Write, reason: Reason) -> Outcome {
    if say(output, &verdict_refused(reason)) {
        Outcome::Refused(reason)
    } else {
        Outcome::Aborted
    }
}

/// Runs the host's side for the run `id`; see the module docs.
pub fn enroll(
    id: &PairingId,
    env: &Enroll<'_>,
    input: Box<dyn Read + Send>,
    output: &mut dyn Write,
) -> Outcome {
    if !say(output, &hello(id)) {
        return Outcome::Aborted;
    }
    let line = match read_line(input, REQUEST_LIMIT, env.request_timeout) {
        Ok(line) => line,
        Err(_) => return refuse(output, Reason::Request),
    };
    let request = match serde_json::from_slice::<Request>(&line) {
        Ok(request) if request.v == VERSION => request,
        _ => return refuse(output, Reason::Request),
    };

    // The state file is the authority: a run that is over, past its deadline or not this
    // account's has nothing to enrol into.
    let state = match StateDir::open(env.account, false)
        .ok()
        .flatten()
        .and_then(|dir| dir.read_state(id).ok().flatten())
    {
        Some(state)
            if state.id == id.as_str()
                && state.uid == env.account.uid
                && (env.now)().to_unix() <= state.deadline =>
        {
            state
        }
        _ => return refuse(output, Reason::Expired),
    };

    let Ok(key) = KeyLine::parse(&request.key) else {
        return refuse(output, Reason::Key);
    };
    let fingerprint = key.fingerprint();
    // The bootstrap key itself must never become the phone's permanent key.
    if fingerprint == state.fingerprint {
        return refuse(output, Reason::Key);
    }
    let device =
        authorized_keys::sanitize_device(&request.device).unwrap_or_else(|| "phone".into());

    let now = (env.now)();
    let mut waited = Duration::ZERO;
    let replaced = loop {
        match authorized_keys::replace(env.account, &state.fingerprint, &key, &device, now) {
            // Another `enroll` holds the lock: wait for it, then find the entry gone.
            Err(error) if error.kind() == io::ErrorKind::WouldBlock && waited < LOCK_PATIENCE => {
                std::thread::sleep(Duration::from_millis(50));
                waited += Duration::from_millis(50);
            }
            other => break other,
        }
    };
    let added = match replaced {
        Ok(Replaced::Done { added }) => added,
        Ok(Replaced::Gone) => return refuse(output, Reason::Gone),
        Err(_) => return refuse(output, Reason::Failed),
    };

    // The waiting `or2-pair` polls for this. Exclusive: only the one write that replaced the
    // bootstrap entry gets here.
    if let Ok(Some(dir)) = StateDir::open(env.account, false) {
        let _ = dir.write_done(
            id,
            &Done {
                device: device.clone(),
                fingerprint: fingerprint.clone(),
            },
        );
    }
    // The key is installed whether or not the phone hears this; the waiting run reports it.
    let _ = say(output, &verdict_ok(&env.account.name, &fingerprint));
    Outcome::Installed {
        device,
        fingerprint,
        added,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Cursor;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;

    use super::*;
    use crate::state::State;

    const BOOT: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIALYvXruViE9G83T84ZJqbdJkEImlV0NRg9AC6Yw4NYo";
    const PHONE: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBCJz8goXA2qGjRTNHhOwsljhOuCXG/+2B/zTJH/brc5";
    const OTHER: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7";
    const ID: &str = "abcdefghijklm";
    const NOW: i64 = 1_782_867_661;

    struct Fixture {
        home: tempfile::TempDir,
        account: Account,
    }

    impl Fixture {
        /// A home with a live run: the state file and the bootstrap entry.
        fn live(deadline: i64) -> Self {
            let home = tempfile::tempdir().unwrap();
            fs::set_permissions(home.path(), fs::Permissions::from_mode(0o755)).unwrap();
            let account = Account::new("dev", home.path());
            let fixture = Self { home, account };
            let dir = StateDir::open(&fixture.account, true).unwrap().unwrap();
            dir.write_state(&State {
                id: ID.into(),
                deadline,
                uid: fixture.account.uid,
                fingerprint: KeyLine::parse(BOOT).unwrap().fingerprint(),
            })
            .unwrap();
            let line = format!(
                "restrict,command=\"/x enroll {ID}\" {BOOT} or2-pair-bootstrap-{ID}\n{OTHER} mine\n"
            );
            fs::write(fixture.keys(), line).unwrap();
            fs::set_permissions(fixture.keys(), fs::Permissions::from_mode(0o600)).unwrap();
            fixture
        }

        fn keys(&self) -> std::path::PathBuf {
            self.account.keys_path()
        }

        fn run(&self, request: &str) -> (Outcome, String) {
            self.run_bytes(request.as_bytes().to_vec())
        }

        fn run_bytes(&self, request: Vec<u8>) -> (Outcome, String) {
            let now = || DateTime::from_unix(NOW);
            let env = Enroll {
                account: &self.account,
                now: &now,
                request_timeout: Duration::from_secs(5),
            };
            let mut out = Vec::new();
            let outcome = enroll(
                &PairingId::parse(ID).unwrap(),
                &env,
                Box::new(Cursor::new(request)),
                &mut out,
            );
            (outcome, String::from_utf8(out).unwrap())
        }
    }

    fn request(key: &str, device: &str) -> String {
        format!("{{\"v\":2,\"key\":\"{key}\",\"device\":\"{device}\"}}\n")
    }

    fn lines(text: &str) -> Vec<&str> {
        text.lines().collect()
    }

    #[test]
    fn a_phone_replaces_the_bootstrap_entry_and_the_run_is_told() {
        let f = Fixture::live(NOW + 300);
        let (outcome, out) = f.run(&request(PHONE, "Pixel 8"));
        let fingerprint = KeyLine::parse(PHONE).unwrap().fingerprint();
        assert_eq!(
            outcome,
            Outcome::Installed {
                device: "Pixel-8".into(),
                fingerprint: fingerprint.clone(),
                added: true
            }
        );
        let said = lines(&out);
        assert_eq!(
            said[0],
            format!("{{\"v\":2,\"hello\":\"or2-pair\",\"id\":\"{ID}\"}}")
        );
        assert_eq!(
            said[1],
            format!("{{\"v\":2,\"ok\":true,\"user\":\"dev\",\"fingerprint\":\"{fingerprint}\"}}")
        );
        let keys = fs::read_to_string(f.keys()).unwrap();
        assert_eq!(
            keys,
            format!(
                "{OTHER} mine\nno-agent-forwarding,no-X11-forwarding {PHONE} or2-Pixel-8-2026-07-01\n"
            )
        );
        let dir = StateDir::open(&f.account, false).unwrap().unwrap();
        let id = PairingId::parse(ID).unwrap();
        assert_eq!(
            dir.read_done(&id).unwrap().unwrap(),
            Done {
                device: "Pixel-8".into(),
                fingerprint
            }
        );
    }

    #[test]
    fn a_second_phone_is_told_gone_and_changes_nothing() {
        let f = Fixture::live(NOW + 300);
        assert!(matches!(
            f.run(&request(PHONE, "a")).0,
            Outcome::Installed { .. }
        ));
        let before = fs::read_to_string(f.keys()).unwrap();
        let (outcome, out) = f.run(&request(OTHER, "b"));
        assert_eq!(outcome, Outcome::Refused(Reason::Gone));
        assert_eq!(lines(&out)[1], "{\"v\":2,\"ok\":false,\"reason\":\"gone\"}");
        assert_eq!(fs::read_to_string(f.keys()).unwrap(), before);
    }

    #[test]
    fn a_key_that_is_already_authorized_only_removes_the_bootstrap_entry() {
        let f = Fixture::live(NOW + 300);
        let (outcome, _) = f.run(&request(OTHER, "p"));
        assert!(
            matches!(outcome, Outcome::Installed { added: false, .. }),
            "{outcome:?}"
        );
        assert_eq!(
            fs::read_to_string(f.keys()).unwrap(),
            format!("{OTHER} mine\n")
        );
    }

    #[test]
    fn no_state_an_expired_state_or_another_accounts_state_is_expired() {
        // Missing: no `~/.ssh/or2-pair` at all.
        let home = tempfile::tempdir().unwrap();
        fs::set_permissions(home.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let none = Fixture {
            account: Account::new("dev", home.path()),
            home,
        };
        let (outcome, out) = none.run(&request(PHONE, "p"));
        assert_eq!(outcome, Outcome::Refused(Reason::Expired));
        assert_eq!(
            lines(&out)[1],
            "{\"v\":2,\"ok\":false,\"reason\":\"expired\"}"
        );

        // Past its deadline by a second.
        let late = Fixture::live(NOW - 1);
        let before = fs::read_to_string(late.keys()).unwrap();
        assert_eq!(
            late.run(&request(PHONE, "p")).0,
            Outcome::Refused(Reason::Expired)
        );
        assert_eq!(
            fs::read_to_string(late.keys()).unwrap(),
            before,
            "nothing changed"
        );
        // Exactly at the deadline still counts.
        assert!(matches!(
            Fixture::live(NOW).run(&request(PHONE, "p")).0,
            Outcome::Installed { .. }
        ));

        // State written for another uid.
        let f = Fixture::live(NOW + 300);
        let dir = StateDir::open(&f.account, false).unwrap().unwrap();
        let id = PairingId::parse(ID).unwrap();
        dir.remove(&id).unwrap();
        dir.write_state(&State {
            id: ID.into(),
            deadline: NOW + 300,
            uid: f.account.uid.wrapping_add(1),
            fingerprint: KeyLine::parse(BOOT).unwrap().fingerprint(),
        })
        .unwrap();
        assert_eq!(
            f.run(&request(PHONE, "p")).0,
            Outcome::Refused(Reason::Expired)
        );

        // State whose id is another run's.
        dir.remove(&id).unwrap();
        fs::write(
            f.home.path().join(".ssh/or2-pair").join(format!("{ID}.json")),
            format!(
                "{{\"id\":\"bbbbbbbbbbbbb\",\"deadline\":{},\"uid\":{},\"fingerprint\":\"SHA256:x\"}}",
                NOW + 300,
                f.account.uid
            ),
        )
        .unwrap();
        fs::set_permissions(
            f.home
                .path()
                .join(".ssh/or2-pair")
                .join(format!("{ID}.json")),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert_eq!(
            f.run(&request(PHONE, "p")).0,
            Outcome::Refused(Reason::Expired)
        );
    }

    #[test]
    fn the_bootstrap_key_cannot_become_the_phones_key_and_bad_keys_are_refused() {
        let f = Fixture::live(NOW + 300);
        let before = fs::read_to_string(f.keys()).unwrap();
        for bad in [
            BOOT,
            "ssh-ed25519 AAAA",
            "not a key",
            "",
            "ssh-dss AAAAB3NzaC1kc3M=",
        ] {
            let (outcome, out) = f.run(&request(bad, "p"));
            assert_eq!(outcome, Outcome::Refused(Reason::Key), "{bad}");
            assert_eq!(lines(&out)[1], "{\"v\":2,\"ok\":false,\"reason\":\"key\"}");
        }
        assert_eq!(fs::read_to_string(f.keys()).unwrap(), before);
    }

    #[test]
    fn unreadable_oversized_or_foreign_requests_are_refused_as_request() {
        let f = Fixture::live(NOW + 300);
        let long = format!(
            "{{\"v\":2,\"key\":\"{}\",\"device\":\"p\"}}\n",
            "A".repeat(2100)
        );
        let at_limit = format!(
            "{{\"v\":2,\"key\":\"{PHONE}\",\"device\":\"{}\"}}\n",
            "d".repeat(1900)
        );
        for bad in [
            "\n".to_owned(),
            "garbage\n".to_owned(),
            "{\"v\":1,\"key\":\"x\",\"device\":\"p\"}\n".to_owned(),
            "{\"v\":3,\"key\":\"x\",\"device\":\"p\"}\n".to_owned(),
            "{\"v\":2,\"device\":\"p\"}\n".to_owned(),
            long,
            // No newline before the channel closes.
            format!("{{\"v\":2,\"key\":\"{PHONE}\",\"device\":\"p\"}}"),
        ] {
            let (outcome, out) = f.run(&bad);
            assert_eq!(
                outcome,
                Outcome::Refused(Reason::Request),
                "{:?}",
                &bad[..bad.len().min(40)]
            );
            assert_eq!(
                lines(&out)[1],
                "{\"v\":2,\"ok\":false,\"reason\":\"request\"}"
            );
        }
        // A long but bounded line (the device label is trimmed to 32 characters) is fine.
        assert!(matches!(f.run(&at_limit).0, Outcome::Installed { .. }));
        let keys = fs::read_to_string(f.keys()).unwrap();
        assert!(
            keys.contains(&format!("or2-{}-2026-07-01", "d".repeat(32))),
            "{keys}"
        );
    }

    #[test]
    fn a_silent_phone_is_refused_after_the_request_timeout() {
        let f = Fixture::live(NOW + 300);
        let (ours, theirs) = UnixStream::pair().unwrap();
        let now = || DateTime::from_unix(NOW);
        let env = Enroll {
            account: &f.account,
            now: &now,
            request_timeout: Duration::from_millis(200),
        };
        let started = std::time::Instant::now();
        let mut out = Vec::new();
        let outcome = enroll(
            &PairingId::parse(ID).unwrap(),
            &env,
            Box::new(theirs),
            &mut out,
        );
        assert_eq!(outcome, Outcome::Refused(Reason::Request));
        assert!(started.elapsed() < Duration::from_secs(3));
        drop(ours);
    }

    #[test]
    fn a_device_without_a_usable_label_is_called_phone() {
        let f = Fixture::live(NOW + 300);
        let (outcome, _) = f.run(&request(PHONE, "---"));
        assert!(matches!(outcome, Outcome::Installed { ref device, .. } if device == "phone"));
    }

    #[test]
    fn a_failed_write_to_the_channel_before_anything_ends_the_run_quietly() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let f = Fixture::live(NOW + 300);
        let now = || DateTime::from_unix(NOW);
        let env = Enroll {
            account: &f.account,
            now: &now,
            request_timeout: Duration::from_secs(1),
        };
        let outcome = enroll(
            &PairingId::parse(ID).unwrap(),
            &env,
            Box::new(Cursor::new(request(PHONE, "p").into_bytes())),
            &mut Broken,
        );
        assert_eq!(outcome, Outcome::Aborted);
        assert!(
            fs::read_to_string(f.keys()).unwrap().contains(BOOT),
            "nothing was changed"
        );
    }
}
