//! `~/.ssh/or2-pair/`: the lock, and the state of the runs that are live (Unix only).
//!
//! - `lock` (mode 0600, kept): every change of `authorized_keys` and every change of a run's
//!   state (publishing it, `enroll`'s commit, the cleanup, the sweep) happens while this process
//!   holds an exclusive `flock` on it ([`StateDir::lock`], [`Held`]). One lock, so `enroll`'s
//!   whole commit and the foreground's cleanup can never interleave.
//! - `<id>.json` (mode 0600) for each live run: the pairing id, the deadline (Unix seconds), the
//!   account's uid and the bootstrap key's fingerprint. Published complete (written under a
//!   temporary name, synced, then linked to its name, which must not exist: an id is used once),
//!   so no reader ever sees part of it. The foreground holds an exclusive `flock` on it for its
//!   whole life ([`Liveness`]): a state file whose lock can be taken belongs to a run that is
//!   gone, even one killed with SIGKILL, and `enroll` refuses it and the sweep removes it.
//! - `<id>.done` (mode 0600, published the same way): written by `enroll`, under the lock, when
//!   it commits a phone's key: the device label and the key's fingerprint.
//!
//! `or2-pair enroll <id>`, the forced command, refuses unless the state file exists, is held by
//! its run, is not past its deadline and names its own uid and id: that, not the cleanup, is what
//! makes the bootstrap key useless after the run.
//!
//! The directory (mode 0700) is opened relative to the checked `~/.ssh` handle and every file
//! below it with the same checks as `authorized_keys`: no links followed, owner, mode, regular
//! file, one name (see [`crate::safefs`]).

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::account::Account;
use crate::bootstrap::PairingId;
use crate::safefs::*;

/// The directory's name inside `~/.ssh`.
pub const DIR: &str = "or2-pair";

/// The lock file's name inside it.
pub const LOCK: &str = "lock";

/// Temporary files in the directory start with this (a crash can leave one; the sweep removes
/// them under the lock).
const TEMP_PREFIX: &str = "tmp.";

/// Neither file is larger than this.
const MAX_BYTES: u64 = 4096;

/// How often a lock that is taken is tried again.
const LOCK_RETRY: Duration = Duration::from_millis(50);

/// What a live run records for its forced command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub id: String,
    /// Unix seconds: after this the forced command refuses.
    pub deadline: i64,
    pub uid: u32,
    /// `SHA256:…` of the bootstrap key.
    pub fingerprint: String,
}

/// What `enroll` records when it has installed a phone's key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Done {
    /// The phone's label, reduced to safe ASCII.
    pub device: String,
    /// `SHA256:…` of the phone's key.
    pub fingerprint: String,
    /// What went wrong after the key file took its new contents (see
    /// `authorized_keys::Committed`): the key is installed, and the waiting run says this too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// A [`Done::warning`] takes at most this many bytes of the record (as JSON, without its
/// quotes), so the record stays well inside [`MAX_BYTES`] whatever the warning says.
pub const WARNING_BYTES: usize = 1024;

/// `text` made fit for [`Done::warning`]: control characters become spaces (JSON would write
/// each as up to six bytes), and it is cut, at a character boundary, where its JSON form would
/// pass [`WARNING_BYTES`]. (Fix check of the v2 fixes: it was cut to 1024 characters, which
/// escaping could grow past the 4 KiB the record is read with.)
pub fn record_warning(text: &str) -> String {
    let mut out = String::new();
    let mut size = 0;
    for c in text.chars() {
        let c = if c.is_control() { ' ' } else { c };
        // What `serde_json` writes for it: `"` and `\` are escaped, the rest is as is.
        let encoded = if matches!(c, '"' | '\\') {
            2
        } else {
            c.len_utf8()
        };
        if size + encoded > WARNING_BYTES {
            break;
        }
        size += encoded;
        out.push(c);
    }
    out
}

/// The `or2-pair/lock` held exclusively: `authorized_keys` and the state may be changed. Dropping
/// it explicitly releases the lock, even if a child inherited the descriptor before exec.
#[derive(Debug)]
pub struct Held {
    _fd: OwnedFd,
}

impl Drop for Held {
    fn drop(&mut self) {
        let _ = unlock(self._fd.as_raw_fd());
    }
}

/// A run's own state file, kept open with an exclusive `flock` for as long as the run lives.
/// Dropping it (or the process ending in any way) releases the lock, and the run counts as gone.
#[derive(Debug)]
pub struct Liveness {
    _fd: OwnedFd,
}

impl Drop for Liveness {
    fn drop(&mut self) {
        // `close` alone leaves the lock alive if a concurrent spawn inherited a descriptor.
        let _ = unlock(self._fd.as_raw_fd());
    }
}

#[derive(Debug)]
pub struct StateDir {
    fd: OwnedFd,
    path: PathBuf,
    uid: u32,
}

fn bad(path: &std::path::Path, what: &str) -> io::Error {
    refuse(format!("{} {what}", path.display()))
}

/// What the lock file must be before its mode is repaired: a regular file of the account with
/// one name. Anything else is refused in words about the lock (not about `authorized_keys`),
/// with the fix: it holds nothing, so removing it is safe.
fn check_lock(stat: &libc::stat, uid: u32, path: &std::path::Path) -> io::Result<()> {
    let why = match kind(stat) {
        libc::S_IFREG if stat.st_uid != uid => format!(
            "it belongs to another user (user id {}), not to the account or2-pair runs as",
            stat.st_uid
        ),
        libc::S_IFREG if stat.st_nlink > 1 => format!(
            "it has another hard link ({} names for one file), so changing its mode would change the other name too",
            stat.st_nlink
        ),
        libc::S_IFREG => return Ok(()),
        libc::S_IFDIR => "it is a directory".to_owned(),
        libc::S_IFLNK => "it is a symbolic link, which or2-pair does not follow".to_owned(),
        _ => "it is not a regular file (a FIFO, a socket or a device)".to_owned(),
    };
    Err(refuse(format!(
        "or2-pair cannot use its lock file {}: {why}. Remove it (it holds nothing) and run again",
        path.display()
    )))
}

impl StateDir {
    /// Opens `~/.ssh/or2-pair`, creating it (mode 0700) when `create`. `None`: it is missing and
    /// was not to be created.
    pub fn open(account: &Account, create: bool) -> io::Result<Option<Self>> {
        let Some(ssh) = open_ssh_dir(account, create)? else {
            return Ok(None);
        };
        let path = account.ssh_dir().join(DIR);
        let flags = libc::O_RDONLY | libc::O_DIRECTORY;
        let fd = match open_nofollow(ssh.as_raw_fd(), DIR, &path, flags, 0) {
            Ok(fd) => fd,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !create {
                    return Ok(None);
                }
                match mkdirat(ssh.as_raw_fd(), DIR, 0o700) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
                let fd = open_nofollow(ssh.as_raw_fd(), DIR, &path, flags, 0)?;
                if fstat(fd.as_raw_fd())?.st_uid == account.uid {
                    fchmod(fd.as_raw_fd(), 0o700)?;
                }
                fd
            }
            Err(error) => return Err(error),
        };
        check_ssh_dir(&fstat(fd.as_raw_fd())?, account.uid, &path)?;
        Ok(Some(Self {
            fd,
            path,
            uid: account.uid,
        }))
    }

    /// Takes the lock, trying again every 50 ms while another process holds it, for up to
    /// `patience`. `interrupted` is asked after each failed try: once it says yes, one more try
    /// is made and then the wait ends. A lock still taken is `WouldBlock`.
    pub fn lock(&self, patience: Duration, interrupted: &dyn Fn() -> bool) -> io::Result<Held> {
        let path = self.path.join(LOCK);
        let fd = self.open_lock(&path)?;
        let started = Instant::now();
        let mut last_chance = false;
        loop {
            if try_lock(fd.as_raw_fd(), true)? {
                return Ok(Held { _fd: fd });
            }
            if last_chance || started.elapsed() >= patience {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!(
                        "another or2-pair holds {} (it is changing authorized_keys); try again in a moment",
                        path.display()
                    ),
                ));
            }
            if interrupted() {
                last_chance = true;
            } else {
                std::thread::sleep(LOCK_RETRY);
            }
        }
    }

    /// Opens the lock file, creating it, readable and writable by the account whatever the
    /// umask: a new one gets 0600 (the umask could leave it 000, and then no later run could open
    /// it), and so does one of the account's own with another mode (it holds nothing, and this
    /// directory is the account's, 0700): one it cannot open, and one that group or others may
    /// write (0666, 0620, 0060). The repair comes before the checks of any other file (StrictModes
    /// included), so they pass. What cannot be repaired is refused with what it is and the advice
    /// to remove it: a symbolic link, a directory, something else that is not a regular file, a
    /// file of another account, a file with another hard link.
    fn open_lock(&self, path: &std::path::Path) -> io::Result<OwnedFd> {
        let dir = self.fd.as_raw_fd();
        let open = || open_file(dir, LOCK, path, libc::O_RDWR | libc::O_CREAT, 0o600);
        let fd = match open() {
            Ok(fd) => fd,
            Err(error) => {
                // Say what is in the way, by name (nothing is opened or followed).
                let Ok(stat) = stat_at(dir, LOCK) else {
                    return Err(error);
                };
                check_lock(&stat, self.uid, path)?;
                if error.raw_os_error() != Some(libc::EACCES) {
                    return Err(error);
                }
                // A file of the account whose mode does not let it open it.
                chmod_at(dir, LOCK, 0o600)?;
                open()?
            }
        };
        let stat = fstat(fd.as_raw_fd())?;
        check_lock(&stat, self.uid, path)?;
        // `st_mode` is 16 bits wide on macOS and 32 on Linux.
        #[allow(clippy::useless_conversion)]
        if u32::from(stat.st_mode) & 0o7777 != 0o600 {
            fchmod(fd.as_raw_fd(), 0o600)?;
        }
        check_file(&fstat(fd.as_raw_fd())?, self.uid, path)?;
        Ok(fd)
    }

    fn file_path(&self, id: &PairingId, extension: &str) -> PathBuf {
        self.path.join(format!("{id}.{extension}"))
    }

    /// Publishes `<id>.<extension>` complete: written to a temporary name, synced, linked to its
    /// name (which must not exist: `AlreadyExists`), the temporary name removed, the directory
    /// synced. With `hold` the file is locked (exclusive `flock`) before it gets its name, and the
    /// locked descriptor is returned. With `replace` it is renamed over the name instead (which
    /// must exist: a reader sees the old file or the new one). `before_link` runs just before the
    /// name appears (tests look there).
    fn publish<T: Serialize>(
        &self,
        id: &PairingId,
        extension: &str,
        value: &T,
        hold: bool,
        replace: bool,
        before_link: &dyn Fn(),
    ) -> io::Result<OwnedFd> {
        let name = format!("{id}.{extension}");
        let temp = temp_name(&format!("{TEMP_PREFIX}{name}."));
        let temp_path = self.path.join(&temp);
        let dir = self.fd.as_raw_fd();
        let flags = libc::O_RDWR | libc::O_CREAT | libc::O_EXCL;
        let fd = open_file(dir, &temp, &temp_path, flags, 0o600)?;
        let result = (|| {
            fchmod(fd.as_raw_fd(), 0o600)?;
            if hold && !try_lock(fd.as_raw_fd(), true)? {
                return Err(io::Error::other(
                    "a new state file was locked by someone else",
                ));
            }
            let mut file = File::from(fd.try_clone()?);
            let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            before_link();
            if replace {
                if !exists_at(dir, &name) {
                    return Err(io::Error::from(io::ErrorKind::NotFound));
                }
                renameat(dir, &temp, &name)
            } else {
                linkat(dir, &temp, &name)
            }
        })();
        let _ = unlinkat(dir, &temp);
        result?;
        fsync_dir(dir)?;
        Ok(fd)
    }

    fn read<T: for<'de> Deserialize<'de>>(
        &self,
        id: &PairingId,
        extension: &str,
    ) -> io::Result<Option<T>> {
        let name = format!("{id}.{extension}");
        let path = self.file_path(id, extension);
        let fd = match open_file(self.fd.as_raw_fd(), &name, &path, libc::O_RDONLY, 0) {
            Ok(fd) => fd,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        check_file(&fstat(fd.as_raw_fd())?, self.uid, &path)?;
        let mut bytes = Vec::new();
        File::from(fd).take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(bad(&path, "is larger than it can be"));
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| bad(&path, "is not a pairing state file"))
    }

    /// Records a live run, complete and created once (an id is used once), and returns its lock:
    /// the run is live for as long as the returned value is kept.
    pub fn publish_state(&self, state: &State) -> io::Result<Liveness> {
        self.publish_state_with(state, &|| {})
    }

    fn publish_state_with(&self, state: &State, before_link: &dyn Fn()) -> io::Result<Liveness> {
        let id = PairingId::parse(&state.id).map_err(io::Error::other)?;
        self.publish(&id, "json", state, true, false, before_link)
            .map(|fd| Liveness { _fd: fd })
    }

    /// The live run `id`, `None` when there is none.
    pub fn read_state(&self, id: &PairingId) -> io::Result<Option<State>> {
        self.read(id, "json")
    }

    /// Whether the run that wrote `<id>.json` still holds it (is still running). A missing file
    /// is `Ok(false)`.
    pub fn held_by_its_run(&self, id: &PairingId) -> io::Result<bool> {
        let name = format!("{id}.json");
        let path = self.file_path(id, "json");
        let fd = match open_file(self.fd.as_raw_fd(), &name, &path, libc::O_RDONLY, 0) {
            Ok(fd) => fd,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        // A shared lock that can be taken means nobody holds the exclusive one; closing the
        // descriptor releases it again.
        Ok(!try_lock(fd.as_raw_fd(), false)?)
    }

    /// Records the phone's key as installed (complete, created once: `AlreadyExists` when it
    /// already was).
    pub fn write_done(&self, id: &PairingId, done: &Done) -> io::Result<()> {
        self.publish(id, "done", done, false, false, &|| {})
            .map(drop)
    }

    /// Replaces the published `<id>.done` with `done` (complete, renamed over it; `NotFound`
    /// when there is none). Under the lock only: `enroll` adds a warning to its own record.
    pub fn rewrite_done(&self, id: &PairingId, done: &Done) -> io::Result<()> {
        self.publish(id, "done", done, false, true, &|| {})
            .map(drop)
    }

    pub fn read_done(&self, id: &PairingId) -> io::Result<Option<Done>> {
        self.read(id, "done")
    }

    /// Whether `<id>.done` exists (the waiting run polls this).
    pub fn has_done(&self, id: &PairingId) -> bool {
        exists_at(self.fd.as_raw_fd(), &format!("{id}.done"))
    }

    fn remove_name(&self, name: &str) -> io::Result<()> {
        match unlinkat(self.fd.as_raw_fd(), name) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// Removes only `<id>.json`, which is what ends `enroll`'s willingness (a missing file is
    /// fine).
    pub fn remove_state(&self, id: &PairingId) -> io::Result<()> {
        self.remove_name(&format!("{id}.json"))
    }

    /// Removes only `<id>.done` (an `enroll` whose commit failed takes its record back).
    pub fn remove_done(&self, id: &PairingId) -> io::Result<()> {
        self.remove_name(&format!("{id}.done"))
    }

    /// Removes `<id>.json` and `<id>.done`; whatever is not there is fine.
    pub fn remove(&self, id: &PairingId) -> io::Result<()> {
        let state = self.remove_state(id);
        let done = self.remove_done(id);
        state.and(done)
    }

    /// Removes the temporary files a crash left behind. Only while holding the lock (every
    /// temporary file is written under it, so none of them is in use then).
    pub fn remove_leftovers(&self, _held: &Held) -> io::Result<()> {
        for name in list(self.fd.as_raw_fd())? {
            if name.starts_with(TEMP_PREFIX) {
                self.remove_name(&name)?;
            }
        }
        Ok(())
    }

    /// The ids that have a `.json` or a `.done` file.
    pub fn ids(&self) -> io::Result<Vec<PairingId>> {
        let mut ids = Vec::new();
        for name in list(self.fd.as_raw_fd())? {
            let stem = name
                .strip_suffix(".json")
                .or_else(|| name.strip_suffix(".done"));
            if let Some(id) = stem.and_then(|stem| PairingId::parse(stem).ok())
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn id(text: &str) -> PairingId {
        PairingId::parse(text).unwrap()
    }

    fn state(text: &str, deadline: i64) -> State {
        State {
            id: text.to_owned(),
            deadline,
            uid: Account::new("t", "/").uid,
            fingerprint: "SHA256:abc".into(),
        }
    }

    fn ssh_home() -> (tempfile::TempDir, Account) {
        let home = tempfile::tempdir().unwrap();
        fs::set_permissions(home.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let account = Account::new("tester", home.path());
        (home, account)
    }

    fn names(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_directory_is_created_private_and_holds_state_and_done_files() {
        let (home, account) = ssh_home();
        assert!(StateDir::open(&account, false).unwrap().is_none());
        let dir = StateDir::open(&account, true).unwrap().unwrap();
        let path = home.path().join(".ssh").join(DIR);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );

        let live = id("aaaaaaaaaaaaa");
        assert!(dir.read_state(&live).unwrap().is_none());
        let held = dir.publish_state(&state("aaaaaaaaaaaaa", 123)).unwrap();
        assert_eq!(
            dir.read_state(&live).unwrap().unwrap(),
            state("aaaaaaaaaaaaa", 123)
        );
        let file = path.join("aaaaaaaaaaaaa.json");
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // One name, no temporary file left.
        assert_eq!(names(&path), ["aaaaaaaaaaaaa.json"]);
        // An id is used once.
        let again = dir.publish_state(&state("aaaaaaaaaaaaa", 456)).unwrap_err();
        assert_eq!(again.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(names(&path), ["aaaaaaaaaaaaa.json"]);
        drop(held);

        assert!(!dir.has_done(&live));
        let done = Done {
            device: "Pixel-8".into(),
            fingerprint: "SHA256:xyz".into(),
            warning: None,
        };
        dir.write_done(&live, &done).unwrap();
        assert!(dir.has_done(&live));
        assert_eq!(dir.read_done(&live).unwrap().unwrap(), done);
        assert_eq!(
            dir.write_done(&live, &done).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            fs::metadata(path.join("aaaaaaaaaaaaa.done"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let _other = dir.publish_state(&state("bbbbbbbbbbbbb", 1)).unwrap();
        let mut ids: Vec<String> = dir.ids().unwrap().iter().map(ToString::to_string).collect();
        ids.sort();
        assert_eq!(ids, ["aaaaaaaaaaaaa", "bbbbbbbbbbbbb"]);

        dir.remove(&live).unwrap();
        assert!(dir.read_state(&live).unwrap().is_none() && !dir.has_done(&live));
        dir.remove(&live).unwrap();
    }

    #[test]
    fn dropping_liveness_unlocks_even_with_a_duplicate_descriptor() {
        let (_home, account) = ssh_home();
        let dir = StateDir::open(&account, true).unwrap().unwrap();
        let live = id("aaaaaaaaaaaaa");
        let liveness = dir.publish_state(&state("aaaaaaaaaaaaa", 123)).unwrap();
        // `dup` and a child between fork and exec share the same open file description.
        // Closing only the guard's descriptor would leave the child's lock alive.
        let inherited = liveness._fd.try_clone().unwrap();
        assert!(dir.held_by_its_run(&live).unwrap());
        drop(liveness);
        assert!(!dir.held_by_its_run(&live).unwrap());
        assert!(dir.read_state(&live).unwrap().is_some());
        drop(inherited);
    }

    #[test]
    fn dropping_the_change_lock_unlocks_even_with_a_duplicate_descriptor() {
        let (_home, account) = ssh_home();
        let dir = StateDir::open(&account, true).unwrap().unwrap();
        let held = dir.lock(Duration::ZERO, &|| false).unwrap();
        let inherited = held._fd.try_clone().unwrap();
        assert_eq!(
            dir.lock(Duration::ZERO, &|| false).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        drop(held);
        let next = dir.lock(Duration::ZERO, &|| false).unwrap();
        drop(inherited);
        // Closing the old duplicate must not release the next owner's independent lock.
        assert_eq!(
            dir.lock(Duration::ZERO, &|| false).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        drop(next);
        dir.lock(Duration::ZERO, &|| false).unwrap();
    }

    #[test]
    fn a_state_file_is_live_exactly_while_its_run_holds_it() {
        let (_home, account) = ssh_home();
        let dir = StateDir::open(&account, true).unwrap().unwrap();
        let live = id("aaaaaaaaaaaaa");
        assert!(!dir.held_by_its_run(&live).unwrap(), "no file: not held");
        let liveness = dir.publish_state(&state("aaaaaaaaaaaaa", 123)).unwrap();
        // Another open of the file (another process, or this one: `flock` is per open file)
        // cannot take it while the run keeps it...
        assert!(dir.held_by_its_run(&live).unwrap());
        assert!(
            dir.held_by_its_run(&live).unwrap(),
            "asking does not take it"
        );
        // ...and can as soon as the run is gone (a crash or a SIGKILL closes it the same way).
        drop(liveness);
        assert!(!dir.held_by_its_run(&live).unwrap());
        assert!(
            dir.read_state(&live).unwrap().is_some(),
            "the file is still there"
        );
    }

    #[test]
    fn a_state_file_is_never_visible_before_it_is_complete() {
        // Review of the v2 integration: the final name used to exist (empty) while the contents
        // were written, and another run's sweep took it for dead and removed it.
        let (home, account) = ssh_home();
        let dir = StateDir::open(&account, true).unwrap().unwrap();
        let live = id("aaaaaaaaaaaaa");
        let path = home.path().join(".ssh").join(DIR);
        let seen = std::cell::RefCell::new(Vec::new());
        let looked = std::cell::Cell::new(false);
        let liveness = dir
            .publish_state_with(&state("aaaaaaaaaaaaa", 123), &|| {
                // Written and synced, not yet named: nobody can see or judge it.
                looked.set(true);
                assert!(dir.read_state(&live).unwrap().is_none());
                assert!(dir.ids().unwrap().is_empty());
                assert!(!dir.held_by_its_run(&live).unwrap());
                *seen.borrow_mut() = names(&path);
            })
            .unwrap();
        assert!(looked.get());
        let during = seen.borrow();
        assert_eq!(during.len(), 1, "{during:?}");
        assert!(during[0].starts_with(TEMP_PREFIX), "{during:?}");
        // Named, complete and held.
        assert_eq!(
            dir.read_state(&live).unwrap().unwrap(),
            state("aaaaaaaaaaaaa", 123)
        );
        assert!(dir.held_by_its_run(&live).unwrap());
        assert_eq!(names(&path), ["aaaaaaaaaaaaa.json"]);
        drop(liveness);
    }

    #[test]
    fn one_lock_for_every_change_and_leftovers_are_removed_under_it() {
        let (home, account) = ssh_home();
        let dir = StateDir::open(&account, true).unwrap().unwrap();
        let held = dir.lock(Duration::ZERO, &|| false).unwrap();
        let lock = home.path().join(".ssh").join(DIR).join(LOCK);
        assert_eq!(
            fs::metadata(&lock).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // A second taker (another open of the file) waits and then gives up.
        let other = StateDir::open(&account, false).unwrap().unwrap();
        let started = Instant::now();
        let error = other
            .lock(Duration::from_millis(200), &|| false)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert!(started.elapsed() >= Duration::from_millis(200));
        // Being interrupted ends the waiting after one more try.
        let started = Instant::now();
        assert!(other.lock(Duration::from_secs(30), &|| true).is_err());
        assert!(started.elapsed() < Duration::from_secs(5));

        let leftover = home.path().join(".ssh").join(DIR).join("tmp.x.json.1-2-3");
        fs::write(&leftover, "{").unwrap();
        dir.remove_leftovers(&held).unwrap();
        assert!(!leftover.exists());
        assert!(lock.exists(), "the lock file stays");
        drop(held);
        assert!(other.lock(Duration::ZERO, &|| false).is_ok());
    }

    #[test]
    fn the_checks_of_authorized_keys_apply_to_the_directory_and_its_files() {
        // A symbolic link in place of the directory, a loose mode, a foreign owner, a link in
        // place of a file and a file that is not state are all refused.
        let (home, account) = ssh_home();
        let ssh = home.path().join(".ssh");
        fs::create_dir(&ssh).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), ssh.join(DIR)).unwrap();
        let error = StateDir::open(&account, true).unwrap_err();
        assert!(error.to_string().contains("symbolic link"), "{error}");
        fs::remove_file(ssh.join(DIR)).unwrap();

        fs::create_dir(ssh.join(DIR)).unwrap();
        fs::set_permissions(ssh.join(DIR), fs::Permissions::from_mode(0o777)).unwrap();
        let error = StateDir::open(&account, true).unwrap_err();
        assert!(
            error.to_string().contains("writable by other users"),
            "{error}"
        );
        fs::set_permissions(ssh.join(DIR), fs::Permissions::from_mode(0o700)).unwrap();

        let foreign = Account {
            uid: account.uid.wrapping_add(1),
            ..account.clone()
        };
        let error = StateDir::open(&foreign, true).unwrap_err();
        assert!(error.to_string().contains("another user"), "{error}");

        let dir = StateDir::open(&account, true).unwrap().unwrap();
        let target = elsewhere.path().join("target.json");
        fs::write(&target, "{}").unwrap();
        std::os::unix::fs::symlink(&target, ssh.join(DIR).join("aaaaaaaaaaaaa.json")).unwrap();
        let error = dir.read_state(&id("aaaaaaaaaaaaa")).unwrap_err();
        assert!(error.to_string().contains("symbolic link"), "{error}");

        fs::write(ssh.join(DIR).join("bbbbbbbbbbbbb.json"), "not json").unwrap();
        let error = dir.read_state(&id("bbbbbbbbbbbbb")).unwrap_err();
        assert!(
            error.to_string().contains("not a pairing state file"),
            "{error}"
        );

        fs::write(ssh.join(DIR).join("ccccccccccccc.json"), "x".repeat(5000)).unwrap();
        assert!(dir.read_state(&id("ccccccccccccc")).is_err());

        let loose = ssh.join(DIR).join("ddddddddddddd.json");
        fs::write(&loose, "{}").unwrap();
        fs::set_permissions(&loose, fs::Permissions::from_mode(0o666)).unwrap();
        let error = dir.read_state(&id("ddddddddddddd")).unwrap_err();
        assert!(
            error.to_string().contains("writable by other users"),
            "{error}"
        );

        // The lock file gets the same checks.
        std::os::unix::fs::symlink(&target, ssh.join(DIR).join(LOCK)).unwrap();
        let error = dir.lock(Duration::ZERO, &|| false).unwrap_err();
        assert!(error.to_string().contains("symbolic link"), "{error}");
    }

    #[test]
    fn a_lock_of_the_account_with_an_unusable_mode_is_repaired() {
        let (home, account) = ssh_home();
        let dir = StateDir::open(&account, true).unwrap().unwrap();
        let lock = home.path().join(".ssh").join(DIR).join(LOCK);
        // Fix check of the v2 fixes: one that group or others may write (0666, 0620, 0060) was
        // refused with the StrictModes message for authorized_keys instead of repaired.
        for mode in [0o000, 0o200, 0o400, 0o644, 0o666, 0o620, 0o060, 0o4777] {
            fs::write(&lock, "").unwrap();
            fs::set_permissions(&lock, fs::Permissions::from_mode(mode)).unwrap();
            let held = dir.lock(Duration::ZERO, &|| false);
            assert!(held.is_ok(), "mode {mode:o}: {held:?}");
            assert_eq!(
                fs::metadata(&lock).unwrap().permissions().mode() & 0o7777,
                0o600,
                "mode {mode:o}"
            );
            drop(held);
            fs::remove_file(&lock).unwrap();
        }
    }

    #[test]
    fn a_rewritten_record_stays_readable_whatever_its_warning_says() {
        // Fix check of the v2 fixes: the warning was cut to 1024 characters, but a control
        // character is six bytes of JSON, so a record could pass the 4 KiB it is read with.
        let (_home, account) = ssh_home();
        let dir = StateDir::open(&account, true).unwrap().unwrap();
        let id = id("aaaaaaaaaaaaa");
        let done = |warning: Option<String>| Done {
            device: "p".repeat(32),
            fingerprint: format!("SHA256:{}", "x".repeat(43)),
            warning,
        };
        // What the cut by characters let through is indeed unreadable.
        dir.write_done(&id, &done(None)).unwrap();
        let by_chars: String = "\u{1}".repeat(5000).chars().take(1024).collect();
        dir.rewrite_done(&id, &done(Some(by_chars))).unwrap();
        assert!(dir.read_done(&id).is_err());

        let nasty = [
            "\u{1}".repeat(5000),
            "\"\\".repeat(3000),
            "é€😀".repeat(1000),
            format!("{}\n{}", "a".repeat(1023), "\u{7f}".repeat(2000)),
        ];
        for text in nasty {
            let warning = record_warning(&text);
            assert!(!warning.chars().any(char::is_control), "{warning:?}");
            let json = serde_json::to_string(&warning).unwrap();
            assert!(json.len() <= WARNING_BYTES + 2, "{} bytes", json.len());
            assert!(text.starts_with(&warning) || text.chars().any(char::is_control));
            dir.rewrite_done(&id, &done(Some(warning.clone()))).unwrap();
            let read = dir.read_done(&id).unwrap().expect("the record is there");
            assert_eq!(read.warning.as_deref(), Some(warning.as_str()));
        }
        // A short warning is kept as it is, control characters aside.
        assert_eq!(
            record_warning("could not be synced\t(Input/output error)"),
            "could not be synced (Input/output error)"
        );
    }

    #[test]
    fn a_lock_that_cannot_be_repaired_is_refused_in_words_about_the_lock() {
        let (home, account) = ssh_home();
        let dir = StateDir::open(&account, true).unwrap().unwrap();
        let lock = home.path().join(".ssh").join(DIR).join(LOCK);
        let refused = |dir: &StateDir, what: &str| {
            let error = dir.lock(Duration::ZERO, &|| false).unwrap_err();
            let text = error.to_string();
            assert!(
                text.contains("cannot use its lock file")
                    && text.contains(what)
                    && text.contains("Remove it")
                    && !text.contains("authorized_keys"),
                "{text}"
            );
        };

        fs::create_dir(&lock).unwrap();
        refused(&dir, "is a directory");
        fs::remove_dir(&lock).unwrap();

        let name = std::ffi::CString::new(lock.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: `name` is a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        refused(&dir, "not a regular file");
        // Also with a mode it cannot open: still named for what it is, not repaired.
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o000)).unwrap();
        refused(&dir, "not a regular file");
        fs::remove_file(&lock).unwrap();

        fs::write(&lock, "").unwrap();
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o666)).unwrap();
        let other = home.path().join(".ssh").join(DIR).join("other");
        fs::hard_link(&lock, &other).unwrap();
        refused(&dir, "another hard link");
        assert_eq!(
            fs::metadata(&lock).unwrap().permissions().mode() & 0o7777,
            0o666,
            "a file with another name is not changed"
        );
        fs::remove_file(&other).unwrap();

        // The same file seen by another account (it cannot be made foreign without root).
        let foreign = StateDir {
            fd: dir.fd.try_clone().unwrap(),
            path: dir.path.clone(),
            uid: dir.uid.wrapping_add(1),
        };
        refused(&foreign, "belongs to another user");
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o000)).unwrap();
        refused(&foreign, "belongs to another user");
        assert_eq!(
            fs::metadata(&lock).unwrap().permissions().mode() & 0o7777,
            0o000,
            "another account's file is not changed"
        );
    }
}
