//! `~/.ssh/or2-pair/`: the state of the runs that are live (Unix only).
//!
//! For each live run, `<id>.json` (mode 0600): the pairing id, the deadline (Unix seconds), the
//! account's uid and the bootstrap key's fingerprint. `or2-pair enroll <id>`, the forced command,
//! refuses unless this file exists, is not past its deadline and names its own uid: that, not the
//! cleanup, is what makes the bootstrap key useless after the window. When a phone's key has
//! been installed, `enroll` writes `<id>.done` (created exclusively, mode 0600: the device label
//! and the key's fingerprint), which the waiting `or2-pair` polls.
//!
//! The directory (mode 0700) is opened relative to the checked `~/.ssh` handle and every file
//! below it with the same checks as `authorized_keys`: no links followed, owner, mode, regular
//! file, one name (see [`crate::safefs`]).

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::account::Account;
use crate::bootstrap::PairingId;
use crate::safefs::*;

/// The directory's name inside `~/.ssh`.
pub const DIR: &str = "or2-pair";

/// Neither file is larger than this.
const MAX_BYTES: u64 = 4096;

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

    fn file_path(&self, id: &PairingId, extension: &str) -> PathBuf {
        self.path.join(format!("{id}.{extension}"))
    }

    fn create<T: Serialize>(&self, id: &PairingId, extension: &str, value: &T) -> io::Result<()> {
        let name = format!("{id}.{extension}");
        let path = self.file_path(id, extension);
        let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL;
        let fd = open_file(self.fd.as_raw_fd(), &name, &path, flags, 0o600)?;
        fchmod(fd.as_raw_fd(), 0o600)?;
        let mut file = File::from(fd);
        let result = serde_json::to_vec(value)
            .map_err(io::Error::other)
            .and_then(|bytes| file.write_all(&bytes))
            .and_then(|()| file.sync_all());
        if result.is_err() {
            let _ = unlinkat(self.fd.as_raw_fd(), &name);
        }
        result
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

    /// Records a live run (created exclusively: an id is used once).
    pub fn write_state(&self, state: &State) -> io::Result<()> {
        let id = PairingId::parse(&state.id).map_err(io::Error::other)?;
        self.create(&id, "json", state)
    }

    /// The live run `id`, `None` when there is none.
    pub fn read_state(&self, id: &PairingId) -> io::Result<Option<State>> {
        self.read(id, "json")
    }

    /// Records the phone's key as installed. `AlreadyExists` when it already was.
    pub fn write_done(&self, id: &PairingId, done: &Done) -> io::Result<()> {
        self.create(id, "done", done)
    }

    pub fn read_done(&self, id: &PairingId) -> io::Result<Option<Done>> {
        self.read(id, "done")
    }

    /// Whether `<id>.done` exists (the waiting run polls this).
    pub fn has_done(&self, id: &PairingId) -> bool {
        exists_at(self.fd.as_raw_fd(), &format!("{id}.done"))
    }

    /// Removes only `<id>.json`, which is what ends `enroll`'s willingness (a missing file is
    /// fine).
    pub fn remove_state(&self, id: &PairingId) -> io::Result<()> {
        match unlinkat(self.fd.as_raw_fd(), &format!("{id}.json")) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// Removes `<id>.json` and `<id>.done`; whatever is not there is fine.
    pub fn remove(&self, id: &PairingId) -> io::Result<()> {
        let mut result = Ok(());
        for extension in ["json", "done"] {
            match unlinkat(self.fd.as_raw_fd(), &format!("{id}.{extension}")) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => result = Err(error),
            }
        }
        result
    }

    /// The ids that have a `.json` or a `.done` file.
    pub fn ids(&self) -> io::Result<Vec<PairingId>> {
        // A new open file description of this directory, so reading it never disturbs the
        // handle that is kept.
        let listing = openat(
            self.fd.as_raw_fd(),
            ".",
            libc::O_RDONLY | libc::O_DIRECTORY,
            0,
        )?;
        // SAFETY: `fdopendir` takes ownership of the descriptor on success.
        let dir = unsafe { libc::fdopendir(listing.as_raw_fd()) };
        if dir.is_null() {
            return Err(io::Error::last_os_error());
        }
        std::mem::forget(listing);
        let mut ids = Vec::new();
        loop {
            // SAFETY: `dir` is an open directory stream until `closedir` below.
            let entry = unsafe { libc::readdir(dir) };
            if entry.is_null() {
                break;
            }
            // SAFETY: `d_name` is a NUL-terminated name inside the entry.
            let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
            let name = name.to_string_lossy();
            let stem = name
                .strip_suffix(".json")
                .or_else(|| name.strip_suffix(".done"));
            if let Some(id) = stem.and_then(|stem| PairingId::parse(stem).ok())
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
        // SAFETY: `dir` came from `fdopendir` and is closed once.
        unsafe { libc::closedir(dir) };
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
        dir.write_state(&state("aaaaaaaaaaaaa", 123)).unwrap();
        assert_eq!(
            dir.read_state(&live).unwrap().unwrap(),
            state("aaaaaaaaaaaaa", 123)
        );
        let file = path.join("aaaaaaaaaaaaa.json");
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // An id is used once.
        let again = dir.write_state(&state("aaaaaaaaaaaaa", 456)).unwrap_err();
        assert_eq!(again.kind(), io::ErrorKind::AlreadyExists);

        assert!(!dir.has_done(&live));
        let done = Done {
            device: "Pixel-8".into(),
            fingerprint: "SHA256:xyz".into(),
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

        dir.write_state(&state("bbbbbbbbbbbbb", 1)).unwrap();
        let mut ids: Vec<String> = dir.ids().unwrap().iter().map(ToString::to_string).collect();
        ids.sort();
        assert_eq!(ids, ["aaaaaaaaaaaaa", "bbbbbbbbbbbbb"]);

        dir.remove(&live).unwrap();
        assert!(dir.read_state(&live).unwrap().is_none() && !dir.has_done(&live));
        dir.remove(&live).unwrap();
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
    }
}
