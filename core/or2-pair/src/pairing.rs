//! The live part of a pairing (Unix only): the bootstrap entry, its state file, the wait for the
//! phone, and the cleanup on every ending.
//!
//! [`Live::start`] writes the state file and then the bootstrap line. From then on **every ending
//! removes both**: [`Live::wait`] cleans up on the phone's success, on the timeout and on a
//! signal (the handler only counts; this loop acts on the count), and dropping a `Live` that has
//! not ended (a panic unwinding through it) does the same. If the entry cannot be removed the
//! exact line to delete is reported and the next run sweeps it: **the security boundary is the
//! state file and its deadline** (see `exchange`), which are removed first.

use std::io;
use std::time::{Duration, Instant};

use crate::account::Account;
use crate::authorized_keys::{self, Backup};
use crate::bootstrap::{self, Dialect, EXPIRY_SLACK, PairingId};
use crate::code::PairCode;
use crate::date::DateTime;
use crate::run::{RunError, Signals};
use crate::state::{Done, State, StateDir};

/// How long a removal that finds the file locked (an `enroll` is writing) keeps trying.
const REMOVAL_PATIENCE: Duration = Duration::from_secs(3);
const REMOVAL_RETRY: Duration = Duration::from_millis(50);

/// How a pairing ended.
#[derive(Debug)]
pub enum Ended {
    /// A phone's key replaced the bootstrap entry (`None`: the details could not be read back).
    Paired(Option<Done>),
    TimedOut,
    Interrupted,
    /// The temporary key could not be removed; the line to delete.
    RemovalFailed(io::Error),
}

enum Reason {
    Done,
    Timeout,
    Interrupted,
}

pub struct Live<'a> {
    account: &'a Account,
    id: PairingId,
    fingerprint: String,
    line: String,
    state: StateDir,
    backup: &'a Backup,
    /// The Unix time after which `enroll` refuses.
    deadline: i64,
    started: Instant,
    armed: bool,
}

/// The ids of runs that are over: reported by `--check`, removed by every real run.
pub fn stale_ids(account: &Account, now_unix: i64) -> Vec<PairingId> {
    let dir = StateDir::open(account, false).ok().flatten();
    let live = |id: &PairingId| is_live(dir.as_ref(), id, now_unix);
    authorized_keys::stale(account, &live).unwrap_or_default()
}

fn is_live(dir: Option<&StateDir>, id: &PairingId, now_unix: i64) -> bool {
    dir.is_some_and(|dir| {
        dir.read_state(id)
            .ok()
            .flatten()
            .is_some_and(|state| state.deadline >= now_unix)
    })
}

/// Removes `or2-pair-bootstrap-*` entries whose state file is missing or past its deadline, and
/// those state files (and any other that is past its deadline). What could not be done is
/// returned as text for the output; none of it stops a pairing that can still proceed.
pub fn sweep(account: &Account, backup: &Backup, now_unix: i64) -> Vec<String> {
    let mut problems = Vec::new();
    let dir = match StateDir::open(account, true) {
        Ok(dir) => dir,
        Err(error) => {
            problems.push(format!("could not look for old pairing state: {error}"));
            None
        }
    };
    let live = |id: &PairingId| is_live(dir.as_ref(), id, now_unix);
    if let Err(error) = authorized_keys::sweep(account, Some(backup), &live) {
        problems.push(format!(
            "could not remove old temporary pairing keys: {error}"
        ));
    }
    if let Some(dir) = &dir {
        for id in dir.ids().unwrap_or_default() {
            if !live(&id) {
                let _ = dir.remove(&id);
            }
        }
    }
    problems
}

impl<'a> Live<'a> {
    /// Derives the bootstrap key from `code`, records the run (state file first) and appends the
    /// bootstrap line. `now` starts the window; `expires` is the host's local time for sshd's
    /// own `expiry-time`.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        account: &'a Account,
        backup: &'a Backup,
        code: &PairCode,
        id: PairingId,
        dialect: Dialect,
        exe: &str,
        now: DateTime,
        window: Duration,
    ) -> Result<Self, RunError> {
        let key = bootstrap::public_key(code, &id);
        let fingerprint = key.fingerprint();
        let seconds = window.as_secs() + u64::from(window.subsec_nanos() > 0);
        let deadline = now.to_unix() + i64::try_from(seconds).unwrap_or(i64::MAX / 4);
        let expires = DateTime::local_from_unix(
            deadline + i64::try_from(EXPIRY_SLACK.as_secs()).unwrap_or(600),
        );
        let line = bootstrap::line(dialect, exe, &id, &key, &expires);

        let state = StateDir::open(account, true)
            .map_err(RunError::State)?
            .ok_or_else(|| RunError::State(io::Error::from(io::ErrorKind::NotFound)))?;
        state
            .write_state(&State {
                id: id.to_string(),
                deadline,
                uid: account.uid,
                fingerprint: fingerprint.clone(),
            })
            .map_err(RunError::State)?;
        if let Err(error) = authorized_keys::append(account, backup, &line) {
            let _ = state.remove(&id);
            return Err(RunError::Install(error));
        }
        Ok(Self {
            account,
            id,
            fingerprint,
            line,
            state,
            backup,
            deadline,
            started: Instant::now(),
            armed: true,
        })
    }

    /// The line that was appended (shown when it cannot be removed).
    pub fn line(&self) -> &str {
        &self.line
    }

    /// When the forced command stops answering, as the host's wall clock.
    pub fn deadline_clock(&self) -> String {
        DateTime::local_from_unix(self.deadline).clock()
    }

    /// Waits for the phone: polls `<id>.done` every `poll`, until `window` has passed since
    /// [`Live::start`] or a signal arrives; then ends the run as the module docs say.
    pub fn wait(&mut self, signals: &dyn Signals, poll: Duration, window: Duration) -> Ended {
        let end = self.started + window;
        let reason = loop {
            if signals.count() > 0 {
                break Reason::Interrupted;
            }
            if self.state.has_done(&self.id) {
                break Reason::Done;
            }
            let now = Instant::now();
            if now >= end {
                break Reason::Timeout;
            }
            std::thread::sleep(poll.min(end - now));
        };
        self.end(reason, signals)
    }

    fn end(&mut self, reason: Reason, signals: &dyn Signals) -> Ended {
        self.armed = false;
        if matches!(reason, Reason::Done) {
            let done = self.state.read_done(&self.id).ok().flatten();
            let _ = self.state.remove(&self.id);
            return Ended::Paired(done);
        }
        // The boundary first: with the state file gone `enroll` refuses. Then the key.
        let _ = self.state.remove_state(&self.id);
        let removal = self.remove_entry(signals);
        // An `enroll` that took the lock just before us has replaced the entry and recorded it.
        let done = self.state.has_done(&self.id);
        let details = if done {
            self.state.read_done(&self.id).ok().flatten()
        } else {
            None
        };
        let _ = self.state.remove(&self.id);
        if done {
            return Ended::Paired(details);
        }
        match (removal, reason) {
            (Err(error), _) => Ended::RemovalFailed(error),
            (Ok(()), Reason::Interrupted) => Ended::Interrupted,
            (Ok(()), _) => Ended::TimedOut,
        }
    }

    /// One locked removal, retried while an `enroll` holds the lock; a second signal ends the
    /// retrying after one more attempt.
    fn remove_entry(&self, signals: &dyn Signals) -> io::Result<()> {
        let first = signals.count();
        let started = Instant::now();
        let mut last_chance = false;
        loop {
            match authorized_keys::remove(self.account, &self.fingerprint, Some(self.backup)) {
                Ok(_) => return Ok(()),
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        && !last_chance
                        && started.elapsed() < REMOVAL_PATIENCE =>
                {
                    if signals.count() > first {
                        last_chance = true;
                    } else {
                        std::thread::sleep(REMOVAL_RETRY);
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }
}

impl Drop for Live<'_> {
    /// A panic (or any early return) with the run still live: one best-effort cleanup.
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let _ = self.state.remove_state(&self.id);
        let removed = authorized_keys::remove(self.account, &self.fingerprint, Some(self.backup));
        let _ = self.state.remove(&self.id);
        if let Err(error) = removed {
            eprintln!(
                "or2-pair: could not remove the temporary pairing key ({error}); delete this line from {}: {}",
                self.account.keys_path().display(),
                self.line
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    const OTHER: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7";
    const ID: &str = "abcdefghijklm";

    #[derive(Default)]
    struct Count(AtomicU32);

    impl Signals for Count {
        fn arm(&self) -> io::Result<()> {
            Ok(())
        }

        fn count(&self) -> u32 {
            self.0.load(Ordering::SeqCst)
        }
    }

    struct Fixture {
        home: tempfile::TempDir,
        account: Account,
        original: String,
    }

    fn fixture() -> Fixture {
        let home = tempfile::tempdir().unwrap();
        fs::set_permissions(home.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let account = Account::new("dev", home.path());
        fs::create_dir(account.ssh_dir()).unwrap();
        fs::set_permissions(account.ssh_dir(), fs::Permissions::from_mode(0o700)).unwrap();
        let original = format!("{OTHER} mine\n");
        fs::write(account.keys_path(), &original).unwrap();
        fs::set_permissions(account.keys_path(), fs::Permissions::from_mode(0o600)).unwrap();
        Fixture {
            home,
            account,
            original,
        }
    }

    fn start<'a>(f: &'a Fixture, backup: &'a Backup, window: Duration) -> Live<'a> {
        let code = PairCode::parse("7KQ4-M2XD-9PTM").unwrap();
        Live::start(
            &f.account,
            backup,
            &code,
            PairingId::parse(ID).unwrap(),
            Dialect::Expiry,
            "/usr/local/bin/or2-pair",
            DateTime::now(),
            window,
        )
        .unwrap()
    }

    fn state_count(f: &Fixture) -> usize {
        fs::read_dir(f.home.path().join(".ssh/or2-pair"))
            .unwrap()
            .count()
    }

    #[test]
    fn starting_records_the_state_and_appends_one_line() {
        let f = fixture();
        let backup = Backup::new(DateTime::now());
        let live = start(&f, &backup, Duration::from_secs(300));
        let keys = fs::read_to_string(f.account.keys_path()).unwrap();
        assert_eq!(keys, format!("{}{}\n", f.original, live.line()));
        assert_eq!(state_count(&f), 1);
        assert!(live.line().contains(
            "restrict,command=\"/usr/local/bin/or2-pair enroll abcdefghijklm\",expiry-time=\""
        ));
        assert_eq!(live.deadline_clock().len(), 5);
        // Dropping a run that never ended is the cleanup of a panic.
        drop(live);
        assert_eq!(
            fs::read_to_string(f.account.keys_path()).unwrap(),
            f.original
        );
        assert_eq!(state_count(&f), 0);
    }

    #[test]
    fn a_panic_with_the_run_live_still_removes_the_key_and_the_state() {
        let f = fixture();
        let backup = Backup::new(DateTime::now());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _live = start(&f, &backup, Duration::from_secs(300));
            assert!(
                fs::read_to_string(f.account.keys_path())
                    .unwrap()
                    .contains("or2-pair-bootstrap-")
            );
            panic!("something went wrong in the middle of a pairing");
        }));
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(f.account.keys_path()).unwrap(),
            f.original
        );
        assert_eq!(state_count(&f), 0);
    }

    #[test]
    fn the_phones_done_file_ends_the_wait_and_cleans_up_the_state() {
        let f = fixture();
        let backup = Backup::new(DateTime::now());
        let mut live = start(&f, &backup, Duration::from_secs(300));
        let id = PairingId::parse(ID).unwrap();
        // What `enroll` leaves behind (here without the replacement in authorized_keys: the
        // wait only looks at the file).
        let dir = StateDir::open(&f.account, false).unwrap().unwrap();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            dir.write_done(
                &id,
                &Done {
                    device: "Pixel-8".into(),
                    fingerprint: "SHA256:abc".into(),
                },
            )
            .unwrap();
        });
        let started = Instant::now();
        let ended = live.wait(
            &Count::default(),
            Duration::from_millis(20),
            Duration::from_secs(60),
        );
        writer.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(10));
        match ended {
            Ended::Paired(Some(done)) => assert_eq!(
                (done.device.as_str(), done.fingerprint.as_str()),
                ("Pixel-8", "SHA256:abc")
            ),
            other => panic!("{other:?}"),
        }
        assert_eq!(state_count(&f), 0);
        drop(live);
        // The entry is not removed by the wait on success: the forced command replaced it.
        assert!(
            fs::read_to_string(f.account.keys_path())
                .unwrap()
                .contains("or2-pair-bootstrap-")
        );
    }

    #[test]
    fn the_timeout_and_a_signal_both_clean_up() {
        for interrupted in [false, true] {
            let f = fixture();
            let backup = Backup::new(DateTime::now());
            let mut live = start(&f, &backup, Duration::from_secs(300));
            let signals = Count::default();
            let window = if interrupted {
                signals.0.store(1, Ordering::SeqCst);
                Duration::from_secs(60)
            } else {
                Duration::from_millis(100)
            };
            let ended = live.wait(&signals, Duration::from_millis(20), window);
            match (interrupted, &ended) {
                (true, Ended::Interrupted) | (false, Ended::TimedOut) => {}
                other => panic!("{other:?}"),
            }
            assert_eq!(
                fs::read_to_string(f.account.keys_path()).unwrap(),
                f.original
            );
            assert_eq!(state_count(&f), 0);
            drop(live);
            assert_eq!(
                fs::read_to_string(f.account.keys_path()).unwrap(),
                f.original
            );
        }
    }

    #[test]
    fn a_locked_file_reports_the_line_and_a_second_signal_stops_the_retrying() {
        let f = fixture();
        let backup = Backup::new(DateTime::now());
        let mut live = start(&f, &backup, Duration::from_secs(300));
        // Another program holds the lock on authorized_keys.
        let holder = fs::File::open(f.account.keys_path()).unwrap();
        // SAFETY: `holder` is an open file for the call.
        assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
        let signals = Arc::new(Count::default());
        signals.0.store(1, Ordering::SeqCst);
        let again = Arc::clone(&signals);
        let second = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            again.0.fetch_add(1, Ordering::SeqCst);
        });
        let started = Instant::now();
        let ended = live.wait(
            &*signals,
            Duration::from_millis(20),
            Duration::from_secs(60),
        );
        second.join().unwrap();
        assert!(
            started.elapsed() < REMOVAL_PATIENCE,
            "the second signal ends the retrying early: {:?}",
            started.elapsed()
        );
        assert!(
            matches!(&ended, Ended::RemovalFailed(error) if error.kind() == io::ErrorKind::WouldBlock),
            "{ended:?}"
        );
        // The boundary is gone regardless: no state, so `enroll` refuses.
        assert_eq!(state_count(&f), 0);
        // And the line to delete is still known.
        assert!(live.line().ends_with("or2-pair-bootstrap-abcdefghijklm"));
        drop(live);
        drop(holder);
    }

    #[test]
    fn stale_ids_are_the_entries_without_a_live_state() {
        let f = fixture();
        let backup = Backup::new(DateTime::now());
        let live = start(&f, &backup, Duration::from_secs(300));
        // Our own run is live; an entry for another id with no state is stale.
        let stale = format!(
            "restrict,command=\"/x enroll bbbbbbbbbbbbb\" {OTHER} or2-pair-bootstrap-bbbbbbbbbbbbb\n"
        );
        let mut keys = fs::read_to_string(f.account.keys_path()).unwrap();
        keys.push_str(&stale);
        fs::write(f.account.keys_path(), keys).unwrap();
        let ids = stale_ids(&f.account, DateTime::now().to_unix());
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].as_str(), "bbbbbbbbbbbbb");
        // Past its deadline our own state counts as dead too.
        let later = DateTime::now().to_unix() + 1000;
        assert_eq!(stale_ids(&f.account, later).len(), 2);
        drop(live);
    }
}
