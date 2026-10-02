//! The live part of a pairing (Unix only): the bootstrap entry, its state file, the wait for the
//! phone, and the cleanup on every ending.
//!
//! [`Live::start`] takes the `or2-pair` lock, publishes the state file (locked by this run for its
//! whole life: see `state`) and appends the bootstrap line, then lets the lock go. From then on
//! **every ending is decided under the lock**: [`Live::wait`] ends on the phone's `.done`, on the
//! timeout (the window on the monotonic clock, or the wall clock passing the deadline, after a
//! suspend) and on a signal (the handler only counts; this loop acts on the count). Then, holding
//! the lock that `enroll` holds for its whole commit, it looks at `.done`: present, the phone's
//! key must be in `authorized_keys` and the run reports the pairing; absent, it removes the state
//! file and the bootstrap entry. Dropping a `Live` that has not ended (a panic unwinding through
//! it) does the same. A process that dies without any of this (SIGKILL) releases its state file's
//! lock, so `enroll` refuses and the next run sweeps what is left: **the security boundary is the
//! state file, its lock and its deadline** (see `exchange`), not the cleanup.

use std::io;
use std::time::{Duration, Instant};

use crate::account::Account;
use crate::authorized_keys::{self, Backup};
use crate::bootstrap::{self, Dialect, PairingId};
use crate::code::PairCode;
use crate::date::DateTime;
use crate::run::{RunError, Signals};
use crate::state::{Done, Held, Liveness, State, StateDir};

/// How long a run waits for the lock (an `enroll` committing, or another run's sweep, holds it
/// for a moment).
pub const LOCK_PATIENCE: Duration = Duration::from_secs(3);

/// How a pairing ended.
#[derive(Debug)]
pub enum Ended {
    /// A phone's key replaced the bootstrap entry, and it is in `authorized_keys`.
    Paired(Done),
    /// `enroll` recorded a pairing, but the phone's key is not in `authorized_keys` (or the
    /// record could not be read): nothing is reported as paired. `removed`: what became of the
    /// bootstrap entry (`Ok(false)`: it was already gone, so nothing was removed).
    NotInstalled {
        done: Option<Done>,
        removed: io::Result<bool>,
    },
    /// The window passed. `removed`: the bootstrap entry was found and removed (false: it was
    /// already gone, so nothing was removed).
    TimedOut { removed: bool },
    /// Ctrl-C, SIGTERM or SIGHUP; `removed` as for `TimedOut`.
    Interrupted { removed: bool },
    /// The lock could not be taken or the entry not removed.
    RemovalFailed(io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// The run's lock on its state file; `None` once the run has ended.
    liveness: Option<Liveness>,
    /// How long an ending waits for the lock.
    patience: Duration,
    /// What went wrong after a change of `authorized_keys` was made (see
    /// `authorized_keys::Committed`), for the output.
    warnings: Vec<String>,
}

/// The ids of runs that are over: reported by `--check` (no lock is taken), removed by every real
/// run.
pub fn stale_ids(account: &Account, now_unix: i64) -> Vec<PairingId> {
    let dir = StateDir::open(account, false).ok().flatten();
    let live = |id: &PairingId| dir.as_ref().is_some_and(|dir| is_live(dir, id, now_unix));
    authorized_keys::stale(account, &live).unwrap_or_default()
}

/// Whether the run `id` is live: its state file is there and readable, is not past its deadline,
/// and its run still holds it (a run killed in any way, SIGKILL included, does not).
pub fn is_live(dir: &StateDir, id: &PairingId, now_unix: i64) -> bool {
    dir.read_state(id)
        .ok()
        .flatten()
        .is_some_and(|state| state.deadline >= now_unix && state.id == id.as_str())
        && dir.held_by_its_run(id).unwrap_or(false)
}

/// Under the lock: removes `or2-pair-bootstrap-*` entries whose run is not live (see
/// [`is_live`]), those state files (and any other of a run that is not live, or a `.done`
/// without its `.json`), and temporary files a crash left. What could not be done is returned as
/// text for the output; none of it stops a pairing that can still proceed.
pub fn sweep(account: &Account, backup: &Backup, now_unix: i64) -> Vec<String> {
    let mut problems = Vec::new();
    let dir = match StateDir::open(account, true) {
        Ok(Some(dir)) => dir,
        Ok(None) => return problems,
        Err(error) => {
            problems.push(format!("could not look for old pairing state: {error}"));
            return problems;
        }
    };
    let held = match dir.lock(LOCK_PATIENCE, &|| false) {
        Ok(held) => held,
        Err(error) => {
            problems.push(format!(
                "could not remove old temporary pairing keys: {error}"
            ));
            return problems;
        }
    };
    let live = |id: &PairingId| is_live(&dir, id, now_unix);
    match authorized_keys::sweep(account, &held, Some(backup), &live) {
        Ok(swept) => problems.extend(swept.warning),
        Err(error) => problems.push(format!(
            "could not remove old temporary pairing keys: {error}"
        )),
    }
    for id in dir.ids().unwrap_or_default() {
        if !live(&id) {
            let _ = dir.remove(&id);
        }
    }
    let _ = dir.remove_leftovers(&held);
    problems
}

impl<'a> Live<'a> {
    /// Derives the bootstrap key from `code`, records the run (state file first) and appends the
    /// bootstrap line, all under the lock. `now` starts the window.
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
        let line = bootstrap::line(dialect, exe, &id, &key, deadline);

        let state = StateDir::open(account, true)
            .map_err(RunError::State)?
            .ok_or_else(|| RunError::State(io::Error::from(io::ErrorKind::NotFound)))?;
        let held = state
            .lock(LOCK_PATIENCE, &|| false)
            .map_err(RunError::State)?;
        let liveness = state
            .publish_state(&State {
                id: id.to_string(),
                deadline,
                uid: account.uid,
                fingerprint: fingerprint.clone(),
            })
            .map_err(RunError::State)?;
        let appended = match authorized_keys::append(account, &held, backup, &line) {
            Ok(appended) => appended,
            Err(error) => {
                let _ = state.remove(&id);
                return Err(RunError::Install(error));
            }
        };
        drop(held);
        Ok(Self {
            account,
            id,
            fingerprint,
            line,
            state,
            backup,
            deadline,
            started: Instant::now(),
            liveness: Some(liveness),
            patience: LOCK_PATIENCE,
            warnings: appended.warning.into_iter().collect(),
        })
    }

    /// What went wrong after a change was made, since the last call (see
    /// `authorized_keys::Committed`).
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// Removes the bootstrap entry: whether one was there, keeping a warning.
    fn remove_entry(&mut self, held: &Held) -> io::Result<bool> {
        let removed =
            authorized_keys::remove(self.account, held, &self.fingerprint, Some(self.backup))?;
        self.warnings.extend(removed.warning);
        Ok(removed.value > 0)
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
    /// [`Live::start`] on the monotonic clock, `now` (the wall clock) is past the deadline (a
    /// suspended machine wakes up after it), or a signal arrives; then ends the run as the module
    /// docs say.
    pub fn wait(
        &mut self,
        signals: &dyn Signals,
        poll: Duration,
        window: Duration,
        now: &dyn Fn() -> DateTime,
    ) -> Ended {
        let end = self.started + window;
        loop {
            let reason = loop {
                if signals.count() > 0 {
                    break Reason::Interrupted;
                }
                let instant = Instant::now();
                if instant >= end || now().to_unix() > self.deadline {
                    break Reason::Timeout;
                }
                if self.state.has_done(&self.id) {
                    break Reason::Done;
                }
                std::thread::sleep(poll.min(end - instant));
            };
            if let Some(ended) = self.end(reason, signals) {
                return ended;
            }
            // The `.done` went away again (the `enroll` that wrote it could not commit), or the
            // lock was busy: the run goes on.
            std::thread::sleep(poll);
        }
    }

    /// Takes the lock (a second signal ends the waiting for it) and ends the run under it.
    /// `None`: the phone's result was not there after all; the run goes on.
    fn end(&mut self, reason: Reason, signals: &dyn Signals) -> Option<Ended> {
        let first = signals.count();
        match self.state.lock(self.patience, &|| signals.count() > first) {
            Ok(held) => self.finish(&held, reason),
            Err(_) if reason == Reason::Done => None,
            Err(error) => {
                // Without the lock nothing can be decided about `authorized_keys`. The state file
                // goes (an `enroll` that has not taken the lock yet then refuses) and so does this
                // run's hold on it.
                let _ = self.state.remove_state(&self.id);
                self.liveness = None;
                Some(Ended::RemovalFailed(error))
            }
        }
    }

    /// Ends the run while holding the lock, so no `enroll` is between its checks and its commit.
    fn finish(&mut self, held: &Held, reason: Reason) -> Option<Ended> {
        if self.state.has_done(&self.id) {
            // A phone's `enroll` committed (before this run took the lock): a pairing, if its key
            // really is in the file.
            let done = self.state.read_done(&self.id).ok().flatten();
            let installed = done.as_ref().is_some_and(|done| {
                authorized_keys::has_fingerprint(self.account, &done.fingerprint).unwrap_or(false)
            });
            // The bootstrap entry is gone after a commit; if not, it goes now.
            let removed = self.remove_entry(held);
            let _ = self.state.remove(&self.id);
            self.liveness = None;
            return Some(match done {
                Some(done) if installed => Ended::Paired(done),
                done => Ended::NotInstalled { done, removed },
            });
        }
        if reason == Reason::Done {
            return None;
        }
        // The boundary first: with the state file gone `enroll` refuses. Then the key.
        let _ = self.state.remove_state(&self.id);
        let removed = self.remove_entry(held);
        let _ = self.state.remove(&self.id);
        self.liveness = None;
        Some(match (removed, reason) {
            (Err(error), _) => Ended::RemovalFailed(error),
            (Ok(removed), Reason::Interrupted) => Ended::Interrupted { removed },
            (Ok(removed), _) => Ended::TimedOut { removed },
        })
    }
}

impl Drop for Live<'_> {
    /// A panic (or any early return) with the run still live: one best-effort cleanup.
    fn drop(&mut self) {
        if self.liveness.is_none() {
            return;
        }
        let ended = match self.state.lock(self.patience, &|| false) {
            Ok(held) => self.finish(&held, Reason::Interrupted),
            Err(error) => {
                let _ = self.state.remove_state(&self.id);
                Some(Ended::RemovalFailed(error))
            }
        };
        self.liveness = None;
        for warning in self.take_warnings() {
            eprintln!("or2-pair: {warning}");
        }
        if let Some(Ended::RemovalFailed(error)) = ended {
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
    use std::io::Cursor;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Condvar, Mutex};

    use super::*;
    use crate::exchange::{self, Enroll, Outcome, Reason as Refusal, Stage};
    use crate::keyline::KeyLine;

    const OTHER: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7";
    const PHONE: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBCJz8goXA2qGjRTNHhOwsljhOuCXG/+2B/zTJH/brc5";
    const ID: &str = "abcdefghijklm";

    /// A signal counter that also counts how often it was looked at, so a test can wait until
    /// the run is retrying a lock (it looks after every failed try).
    #[derive(Default)]
    struct Count {
        raised: AtomicU32,
        looked: Mutex<u32>,
        changed: Condvar,
    }

    impl Count {
        fn raised(n: u32) -> Self {
            let count = Self::default();
            count.raised.store(n, Ordering::SeqCst);
            count
        }

        /// Blocks until `count` has been called at least `n` times.
        fn wait_for_looks(&self, n: u32) {
            let mut looked = self.looked.lock().unwrap();
            while *looked < n {
                looked = self.changed.wait(looked).unwrap();
            }
        }
    }

    impl Signals for Count {
        fn arm(&self) -> io::Result<()> {
            Ok(())
        }

        fn count(&self) -> u32 {
            *self.looked.lock().unwrap() += 1;
            self.changed.notify_all();
            self.raised.load(Ordering::SeqCst)
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

    fn code() -> PairCode {
        PairCode::parse("7KQ4-M2XD-9PTM").unwrap()
    }

    fn start<'a>(f: &'a Fixture, backup: &'a Backup, window: Duration) -> Live<'a> {
        Live::start(
            &f.account,
            backup,
            &code(),
            PairingId::parse(ID).unwrap(),
            Dialect::ExpiryUtc,
            "/usr/local/bin/or2-pair",
            DateTime::now(),
            window,
        )
        .unwrap()
    }

    /// The files of runs in `~/.ssh/or2-pair` (not the lock file).
    fn state_count(f: &Fixture) -> usize {
        fs::read_dir(f.home.path().join(".ssh/or2-pair"))
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name() != crate::state::LOCK)
            .count()
    }

    fn keys(f: &Fixture) -> String {
        fs::read_to_string(f.account.keys_path()).unwrap()
    }

    /// The phone's side, minus SSH: the forced command's code with this request.
    fn enroll(f: &Fixture, hook: Option<&dyn Fn(Stage)>) -> Outcome {
        let env = Enroll {
            account: &f.account,
            now: &DateTime::now,
            request_timeout: Duration::from_secs(5),
            hook,
        };
        let request = format!("{{\"v\":2,\"key\":\"{PHONE}\",\"device\":\"Pixel 8\"}}\n");
        exchange::enroll(
            &PairingId::parse(ID).unwrap(),
            &env,
            Box::new(Cursor::new(request.into_bytes())),
            &mut Vec::new(),
        )
    }

    fn phone_fingerprint() -> String {
        KeyLine::parse(PHONE).unwrap().fingerprint()
    }

    #[test]
    fn starting_records_the_state_and_appends_one_line() {
        let f = fixture();
        let backup = Backup::new(DateTime::now());
        let live = start(&f, &backup, Duration::from_secs(300));
        assert_eq!(keys(&f), format!("{}{}\n", f.original, live.line()));
        assert_eq!(state_count(&f), 1);
        assert!(live.line().contains(
            "restrict,command=\"/usr/local/bin/or2-pair enroll abcdefghijklm\",expiry-time=\""
        ));
        assert_eq!(live.deadline_clock().len(), 5);
        let dir = StateDir::open(&f.account, false).unwrap().unwrap();
        let id = PairingId::parse(ID).unwrap();
        assert!(is_live(&dir, &id, DateTime::now().to_unix()));
        // Dropping a run that never ended is the cleanup of a panic.
        drop(live);
        assert_eq!(keys(&f), f.original);
        assert_eq!(state_count(&f), 0);
    }

    #[test]
    fn a_panic_with_the_run_live_still_removes_the_key_and_the_state() {
        let f = fixture();
        let backup = Backup::new(DateTime::now());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _live = start(&f, &backup, Duration::from_secs(300));
            assert!(keys(&f).contains("or2-pair-bootstrap-"));
            panic!("something went wrong in the middle of a pairing");
        }));
        assert!(result.is_err());
        assert_eq!(keys(&f), f.original);
        assert_eq!(state_count(&f), 0);
    }

    #[test]
    fn a_phone_that_paired_ends_the_wait_and_the_state_is_cleaned_up() {
        let f = fixture();
        let backup = Backup::new(DateTime::now());
        let mut live = start(&f, &backup, Duration::from_secs(300));
        assert!(matches!(enroll(&f, None), Outcome::Installed { .. }));
        let ended = live.wait(
            &Count::default(),
            Duration::from_millis(20),
            Duration::from_secs(60),
            &DateTime::now,
        );
        match ended {
            Ended::Paired(done) => {
                assert_eq!(done.device, "Pixel-8");
                assert_eq!(done.fingerprint, phone_fingerprint());
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(state_count(&f), 0);
        drop(live);
        let after = keys(&f);
        assert!(!after.contains("or2-pair-bootstrap-") && after.contains(PHONE));
    }

    #[test]
    fn a_done_record_without_the_phones_key_in_the_file_is_not_a_pairing() {
        // The waiting run checks the file, not only the record.
        let f = fixture();
        let backup = Backup::new(DateTime::now());
        let mut live = start(&f, &backup, Duration::from_secs(300));
        let dir = StateDir::open(&f.account, false).unwrap().unwrap();
        dir.write_done(
            &PairingId::parse(ID).unwrap(),
            &Done {
                device: "Pixel-8".into(),
                fingerprint: phone_fingerprint(),
                warning: None,
            },
        )
        .unwrap();
        let ended = live.wait(
            &Count::default(),
            Duration::from_millis(20),
            Duration::from_secs(60),
            &DateTime::now,
        );
        assert!(
            matches!(
                ended,
                Ended::NotInstalled {
                    done: Some(_),
                    removed: Ok(true)
                }
            ),
            "{ended:?}"
        );
        assert_eq!(keys(&f), f.original, "the bootstrap entry was removed");
        assert_eq!(state_count(&f), 0);
    }

    #[test]
    fn a_record_whose_key_and_bootstrap_entry_are_both_gone_removes_nothing() {
        // Fix check of the v2 fixes: this ending was always reported as having removed the
        // temporary key.
        let f = fixture();
        let backup = Backup::new(DateTime::now());
        let mut live = start(&f, &backup, Duration::from_secs(300));
        fs::write(f.account.keys_path(), &f.original).unwrap();
        let dir = StateDir::open(&f.account, false).unwrap().unwrap();
        dir.write_done(
            &PairingId::parse(ID).unwrap(),
            &Done {
                device: "Pixel-8".into(),
                fingerprint: phone_fingerprint(),
                warning: None,
            },
        )
        .unwrap();
        let ended = live.wait(
            &Count::default(),
            Duration::from_millis(20),
            Duration::from_secs(60),
            &DateTime::now,
        );
        assert!(
            matches!(
                ended,
                Ended::NotInstalled {
                    done: Some(_),
                    removed: Ok(false)
                }
            ),
            "{ended:?}"
        );
        assert_eq!(keys(&f), f.original);
        assert_eq!(state_count(&f), 0);
    }

    #[test]
    fn a_pairing_whose_directory_sync_failed_is_reported_with_the_warning() {
        // Fix check of the v2 fixes: the sync after the rename failed, and the installed key was
        // reported as a failure. The pairing stands; the waiting run says what went wrong.
        use crate::safefs::fault::{self, Fault};
        let f = fixture();
        let backup = Backup::new(DateTime::now());
        let mut live = start(&f, &backup, Duration::from_secs(300));
        fault::arm(Fault::SyncAfterRename);
        assert!(matches!(enroll(&f, None), Outcome::Installed { .. }));
        assert!(!fault::fires(Fault::SyncAfterRename), "the fault was used");
        let ended = live.wait(
            &Count::default(),
            Duration::from_millis(20),
            Duration::from_secs(60),
            &DateTime::now,
        );
        match ended {
            Ended::Paired(done) => {
                assert_eq!(done.fingerprint, phone_fingerprint());
                let warning = done.warning.unwrap_or_default();
                assert!(warning.contains("could not be synced to disk"), "{warning}");
            }
            other => panic!("{other:?}"),
        }
        assert!(keys(&f).contains(PHONE));
        assert_eq!(state_count(&f), 0);
    }

    #[test]
    fn the_timeout_and_a_signal_both_clean_up() {
        for interrupted in [false, true] {
            let f = fixture();
            let backup = Backup::new(DateTime::now());
            let mut live = start(&f, &backup, Duration::from_secs(300));
            let signals = Count::raised(u32::from(interrupted));
            let window = if interrupted {
                Duration::from_secs(60)
            } else {
                Duration::from_millis(100)
            };
            let ended = live.wait(&signals, Duration::from_millis(20), window, &DateTime::now);
            match (interrupted, &ended) {
                (true, Ended::Interrupted { removed: true })
                | (false, Ended::TimedOut { removed: true }) => {}
                other => panic!("{other:?}"),
            }
            assert_eq!(keys(&f), f.original);
            assert_eq!(state_count(&f), 0);
            drop(live);
            assert_eq!(keys(&f), f.original);
        }
    }

    #[test]
    fn an_entry_that_was_already_gone_is_not_reported_as_removed() {
        let f = fixture();
        let backup = Backup::new(DateTime::now());
        let mut live = start(&f, &backup, Duration::from_secs(300));
        // Someone edited the file by hand meanwhile.
        fs::write(f.account.keys_path(), &f.original).unwrap();
        let ended = live.wait(
            &Count::raised(1),
            Duration::from_millis(20),
            Duration::from_secs(60),
            &DateTime::now,
        );
        assert!(
            matches!(ended, Ended::Interrupted { removed: false }),
            "{ended:?}"
        );
    }

    #[test]
    fn the_wait_ends_when_the_wall_clock_passes_the_deadline() {
        // A suspended machine: the monotonic clock did not move, the wall clock did.
        let f = fixture();
        let backup = Backup::new(DateTime::now());
        let mut live = start(&f, &backup, Duration::from_secs(300));
        let later = DateTime::from_unix(live.deadline + 1);
        let started = Instant::now();
        let ended = live.wait(
            &Count::default(),
            Duration::from_millis(20),
            Duration::from_secs(300),
            &|| later,
        );
        assert!(
            matches!(ended, Ended::TimedOut { removed: true }),
            "{ended:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(keys(&f), f.original);
    }

    #[test]
    fn a_held_lock_reports_the_line_and_a_second_signal_stops_the_retrying() {
        let f = fixture();
        let backup = Backup::new(DateTime::now());
        let mut live = start(&f, &backup, Duration::from_secs(300));
        // Another program holds the lock.
        let dir = StateDir::open(&f.account, false).unwrap().unwrap();
        let holder = dir.lock(Duration::ZERO, &|| false).unwrap();
        let signals = Count::raised(1);
        let ended = std::thread::scope(|scope| {
            scope.spawn(|| {
                // The run looks once in its wait, once as the cleanup starts and once after its
                // first failed try of the lock: then the second signal comes.
                signals.wait_for_looks(3);
                signals.raised.fetch_add(1, Ordering::SeqCst);
            });
            live.wait(
                &signals,
                Duration::from_millis(20),
                Duration::from_secs(60),
                &DateTime::now,
            )
        });
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
    fn a_cleanup_that_meets_an_enroll_in_its_commit_reports_the_pairing() {
        // Review of the v2 integration: `enroll` checked the state, the cleanup removed it and
        // the bootstrap entry and reported "Cancelled", and then `enroll` installed the phone's
        // key and recorded it. Now the cleanup waits for the lock `enroll` holds over its whole
        // commit, and then sees the record.
        let f = fixture();
        let backup = Backup::new(DateTime::now());
        let mut live = start(&f, &backup, Duration::from_secs(300));
        let signals = Count::raised(1);
        let (locked, go) = (Mutex::new(false), Condvar::new());
        let (released, release) = (Mutex::new(false), Condvar::new());
        let ended = std::thread::scope(|scope| {
            let phone = scope.spawn(|| {
                let hook = |stage: Stage| {
                    if stage == Stage::Locked {
                        *locked.lock().unwrap() = true;
                        go.notify_all();
                        let mut free = released.lock().unwrap();
                        while !*free {
                            free = release.wait(free).unwrap();
                        }
                    }
                };
                enroll(&f, Some(&hook))
            });
            scope.spawn(|| {
                // The cleanup has started and failed to take the lock once: let `enroll` go on.
                signals.wait_for_looks(3);
                *released.lock().unwrap() = true;
                release.notify_all();
            });
            // `enroll` holds the lock and has checked the state again: now the run ends.
            let mut is_locked = locked.lock().unwrap();
            while !*is_locked {
                is_locked = go.wait(is_locked).unwrap();
            }
            drop(is_locked);
            let ended = live.wait(
                &signals,
                Duration::from_millis(20),
                Duration::from_secs(60),
                &DateTime::now,
            );
            (ended, phone.join().unwrap())
        });
        let (ended, outcome) = ended;
        assert!(matches!(outcome, Outcome::Installed { .. }), "{outcome:?}");
        assert!(matches!(ended, Ended::Paired(_)), "{ended:?}");
        assert!(keys(&f).contains(PHONE));
        assert_eq!(state_count(&f), 0);
    }

    #[test]
    fn an_enroll_that_reaches_the_lock_after_the_cleanup_refuses() {
        // The other order: `enroll` read the state, then the whole cleanup ran, then `enroll`
        // takes the lock and checks again.
        let f = fixture();
        let backup = Backup::new(DateTime::now());
        let live = std::cell::RefCell::new(start(&f, &backup, Duration::from_secs(300)));
        let ended = std::cell::RefCell::new(None);
        let hook = |stage: Stage| {
            if stage == Stage::Checked {
                *ended.borrow_mut() = Some(live.borrow_mut().wait(
                    &Count::raised(1),
                    Duration::from_millis(20),
                    Duration::from_secs(60),
                    &DateTime::now,
                ));
            }
        };
        let outcome = enroll(&f, Some(&hook));
        assert_eq!(outcome, Outcome::Refused(Refusal::Expired));
        assert!(
            matches!(
                ended.borrow().as_ref(),
                Some(Ended::Interrupted { removed: true })
            ),
            "{:?}",
            ended.borrow()
        );
        assert_eq!(keys(&f), f.original);
        assert_eq!(state_count(&f), 0);
    }

    #[test]
    fn a_run_that_is_gone_without_cleaning_up_is_dead_to_enroll_and_the_sweep() {
        // What SIGKILL leaves: the state file and the bootstrap entry, nobody holding them.
        let f = fixture();
        let backup = Backup::new(DateTime::now());
        let live = start(&f, &backup, Duration::from_secs(300));
        let left = keys(&f);
        // Drop the hold without any cleanup, as a killed process does.
        let mut live = live;
        drop(live.liveness.take());
        std::mem::forget(live);
        assert_eq!(keys(&f), left);
        assert_eq!(state_count(&f), 1);
        assert_eq!(enroll(&f, None), Outcome::Refused(Refusal::Expired));
        assert_eq!(keys(&f), left, "nothing installed");
        // The next run's sweep removes both, although the deadline is still ahead.
        let problems = sweep(
            &f.account,
            &Backup::new(DateTime::now()),
            DateTime::now().to_unix(),
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(keys(&f), f.original);
        assert_eq!(state_count(&f), 0);
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
        let mut text = keys(&f);
        text.push_str(&stale);
        fs::write(f.account.keys_path(), text).unwrap();
        let ids = stale_ids(&f.account, DateTime::now().to_unix());
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].as_str(), "bbbbbbbbbbbbb");
        // Past its deadline our own state counts as dead too.
        let later = DateTime::now().to_unix() + 1000;
        assert_eq!(stale_ids(&f.account, later).len(), 2);
        drop(live);
    }
}
