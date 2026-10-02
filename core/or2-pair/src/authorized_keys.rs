//! `~/.ssh/authorized_keys`: the bootstrap entry of a run, its replacement by the phone's key,
//! its removal and the sweep of old ones, and the care taken doing it.
//!
//! - everything goes through the checked handles of [`crate::safefs`] (no links followed, owner
//!   and StrictModes checked, one regular file with one name, an exclusive `flock`),
//! - **only on Unix.** There is no implementation for any other target: the changes always fail
//!   there and [`writable`] always says no, so `or2-pair` never writes a key file where it cannot
//!   check owners, links and permissions with handles (see `run::Env::install_keys`),
//! - `~/.ssh` is created with mode 0700 and the file with 0600 when missing,
//! - the file is backed up before the first change of a run (`authorized_keys.or2-backup-<stamp>`,
//!   mode 0600), once per run, only when it exists and is not empty,
//! - every change is **one locked operation** that reads the file again, computes the whole new
//!   contents and rewrites the file in place (`ftruncate` and write on the same descriptor, so the
//!   inode, mode, owner and SELinux label stay): append the bootstrap line; remove it; replace
//!   it with the phone's key; sweep old bootstrap entries,
//! - entries are matched by their parsed key (key type and key data), whatever options precede
//!   them and whatever comment follows, exactly as sshd reads the file; every other line is kept
//!   byte for byte,
//! - the phone's line is `no-agent-forwarding,no-X11-forwarding <algorithm> <key>
//!   or2-<device>-<date>`, rebuilt from the validated key, so nothing from the phone but the key
//!   itself is written,
//! - a missing final newline is repaired first, so a new line never joins the last one.

#[cfg(unix)]
use std::cell::{Cell, RefCell};
#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use sha2::{Digest, Sha256};

use crate::account::Account;
#[cfg(unix)]
use crate::date::DateTime;
use crate::keyline::KeyLine;

/// The options every key added by or2 carries.
pub const OPTIONS: &str = "no-agent-forwarding,no-X11-forwarding";

/// An `authorized_keys` larger than this is not read into memory (nobody's real one is).
pub const MAX_FILE_BYTES: u64 = 8 << 20;

/// A device label as it appears in the key's comment and on screen: ASCII letters, digits,
/// `.`, `_` and `-` only (anything else becomes `-`, runs collapse), at most 32 characters.
/// `None` when nothing is left.
pub fn sanitize_device(raw: &str) -> Option<String> {
    let mut out = String::new();
    for c in raw.trim().chars() {
        let c = if c.is_ascii_alphanumeric() || matches!(c, '.' | '_') {
            c
        } else {
            '-'
        };
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
    }
    let out: String = out.trim_matches('-').chars().take(32).collect();
    let out = out.trim_matches('-').to_owned();
    (!out.is_empty()).then_some(out)
}

/// The phone's line.
pub fn line_for(key: &KeyLine, device: &str, date: &str) -> String {
    format!("{OPTIONS} {} or2-{device}-{date}", key.openssh())
}

/// `SHA256:…` of key data, as `ssh-keygen -l` prints it.
pub fn fingerprint_of(blob: &[u8]) -> String {
    format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(blob)))
}

/// Whether some entry of `contents` authorizes `key`: its parsed key type and key data equal
/// the key's, whatever options precede it and whatever comment follows. Text elsewhere on a line
/// (a comment that mentions a key, a quoted option) is not an authorization, exactly as sshd
/// reads the file.
pub fn contains(contents: &[u8], key: &KeyLine) -> bool {
    contents
        .split(|byte| *byte == b'\n')
        .filter_map(|line| entry(&String::from_utf8_lossy(line)))
        .any(|entry| entry.algorithm == key.algorithm() && entry.blob == key.blob())
}

/// Whether `token` names a public key type (sshd tells the key from the options this way: an
/// options field never starts like one).
fn is_key_type(token: &str) -> bool {
    token.starts_with("ssh-")
        || token.starts_with("ecdsa-sha2-")
        || token.starts_with("sk-")
        || token.ends_with("-cert-v01@openssh.com")
}

/// One well-formed `authorized_keys` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub algorithm: String,
    /// The decoded key data.
    pub blob: Vec<u8>,
    /// The first word of the comment ("" when there is none).
    pub comment: String,
}

/// The key type, decoded key data and comment of one `authorized_keys` line, or `None` for a
/// blank line, a comment, or a line that is not a well-formed entry (an unterminated quote in
/// the options, no key data, key data that is not base64).
///
/// A line is `[options] keytype base64 [comment]`. The options are comma-separated and may
/// contain whitespace inside double quotes (with `\"` for a quote inside quotes); the field ends
/// at the first whitespace outside quotes.
pub fn entry(line: &str) -> Option<Entry> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let mut rest = line;
    if !is_key_type(rest.split_whitespace().next()?) {
        // An options field: skip it, quotes and all.
        let mut quoted = false;
        let mut escaped = false;
        let mut end = None;
        for (index, c) in rest.char_indices() {
            if escaped {
                escaped = false;
            } else if quoted && c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = !quoted;
            } else if !quoted && c.is_whitespace() {
                end = Some(index);
                break;
            }
        }
        // No whitespace outside quotes: options and nothing after them. An unterminated quote
        // runs to the end of the line, so it ends up here as well.
        rest = rest[end?..].trim_start();
    }
    let mut fields = rest.split_whitespace();
    let algorithm = fields.next().filter(|token| is_key_type(token))?;
    let blob = STANDARD.decode(fields.next()?.as_bytes()).ok()?;
    Some(Entry {
        algorithm: algorithm.to_owned(),
        blob,
        comment: fields.next().unwrap_or_default().to_owned(),
    })
}

/// `contents` without every line for which `drop` is true (lines are kept byte for byte), and how
/// many lines were dropped.
#[cfg(unix)]
fn without(contents: &[u8], drop: impl Fn(&Entry) -> bool) -> (Vec<u8>, usize) {
    let mut kept = Vec::with_capacity(contents.len());
    let mut dropped = 0;
    for line in contents.split_inclusive(|byte| *byte == b'\n') {
        let parsed = entry(&String::from_utf8_lossy(line));
        if parsed.as_ref().is_some_and(&drop) {
            dropped += 1;
        } else {
            kept.extend_from_slice(line);
        }
    }
    (kept, dropped)
}

/// `contents` with `line` appended, after a newline when the file does not end in one.
#[cfg(unix)]
fn with_line(contents: &[u8], line: &str) -> Vec<u8> {
    let mut out = contents.to_vec();
    if out.last().is_some_and(|last| *last != b'\n') {
        out.push(b'\n');
    }
    out.extend_from_slice(line.as_bytes());
    out.push(b'\n');
    out
}

/// The backup of a run: made before the first change, once, and only of a non-empty file.
#[cfg(unix)]
#[derive(Debug)]
pub struct Backup {
    now: DateTime,
    used: Cell<bool>,
    path: RefCell<Option<PathBuf>>,
}

#[cfg(unix)]
impl Backup {
    pub fn new(now: DateTime) -> Self {
        Self {
            now,
            used: Cell::new(false),
            path: RefCell::new(None),
        }
    }

    /// Where the previous file was saved, if it was.
    pub fn path(&self) -> Option<PathBuf> {
        self.path.borrow().clone()
    }
}

/// What a change did besides its own result.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Modified<T> {
    pub value: T,
    /// The file did not exist and was created (mode 0600).
    pub created: bool,
}

/// What [`replace`] came to.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replaced {
    /// The bootstrap entry is replaced; `added` is false when the phone's key was already there.
    Done { added: bool },
    /// There was no bootstrap entry any more (removed, or another phone took it first).
    Gone,
}

/// Whether the key could be added right now, without adding it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Writable {
    Yes,
    /// Not possible; why, for the check's message.
    No(String),
}

#[cfg(unix)]
pub use unix::{append, modify, remove, replace, stale, sweep, writable};

/// Why nothing is written on a target without the Unix implementation.
#[cfg(not(unix))]
pub const UNSUPPORTED: &str = "or2-pair does not write authorized_keys on this platform: it cannot check the file's owner, links and permissions safely here";

/// Never writable where there is no implementation: nothing is opened or looked at.
#[cfg(not(unix))]
pub fn writable(_: &Account) -> Writable {
    Writable::No(UNSUPPORTED.to_owned())
}

#[cfg(unix)]
mod unix {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::fd::{AsRawFd, OwnedFd};

    use super::*;
    use crate::bootstrap::{COMMENT_PREFIX, PairingId};
    use crate::safefs::*;

    fn backup_name(now: DateTime, attempt: u32) -> String {
        let suffix = if attempt == 0 {
            String::new()
        } else {
            format!("-{attempt}")
        };
        format!("authorized_keys.or2-backup-{}{suffix}", now.stamp())
    }

    fn write_backup(
        dir: &OwnedFd,
        dir_path: &Path,
        contents: &[u8],
        now: DateTime,
    ) -> io::Result<PathBuf> {
        for attempt in 0..100 {
            let name = backup_name(now, attempt);
            let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL;
            match openat(dir.as_raw_fd(), &name, flags | libc::O_NOFOLLOW, 0o600) {
                Ok(fd) => {
                    let mut out = File::from(fd);
                    out.write_all(contents)?;
                    out.sync_all()?;
                    return Ok(dir_path.join(name));
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::other("could not find a free backup name"))
    }

    /// Rewrites the file in place: the new contents over the start, then the length cut. A
    /// failure puts the old contents back (the backup holds them too).
    fn rewrite(file: &mut File, old: &[u8], new: &[u8]) -> io::Result<()> {
        let write = |file: &mut File, bytes: &[u8]| -> io::Result<()> {
            file.seek(SeekFrom::Start(0))?;
            file.write_all(bytes)?;
            file.set_len(bytes.len() as u64)?;
            file.sync_all()
        };
        write(file, new).inspect_err(|_| {
            let _ = write(file, old);
        })
    }

    /// The one locked operation: opens the checked handles, locks, reads the file, lets `change`
    /// compute the new contents (`None`: no change) and rewrites the file in place. `create`
    /// makes a missing `~/.ssh` and file; without it a missing file is `NotFound`. `backup` is
    /// the run's backup, made before the first change. `after_open` is called once the file is
    /// read (tests swap paths there).
    pub fn modify<T>(
        account: &Account,
        create: bool,
        backup: Option<&Backup>,
        after_open: &dyn Fn(),
        change: impl FnOnce(&[u8]) -> io::Result<(Option<Vec<u8>>, T)>,
    ) -> io::Result<Modified<T>> {
        let dir_path = account.ssh_dir();
        let file_path = account.keys_path();
        let name = account.keys_name();
        let dir = open_ssh_dir(account, create)?
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;

        // The file: an existing one is opened as it is; a missing one is created exclusively,
        // so a path planted in between is refused rather than written through.
        let flags = libc::O_RDWR;
        let (file, created) = match open_file(dir.as_raw_fd(), &name, &file_path, flags, 0) {
            Ok(fd) => (fd, false),
            Err(error) if error.kind() == io::ErrorKind::NotFound && create => {
                let make = flags | libc::O_CREAT | libc::O_EXCL;
                let fd = open_file(dir.as_raw_fd(), &name, &file_path, make, 0o600)?;
                fchmod(fd.as_raw_fd(), 0o600)?;
                (fd, true)
            }
            Err(error) => return Err(error),
        };
        check_file(&fstat(file.as_raw_fd())?, account.uid, &file_path)?;
        lock(file.as_raw_fd())?;
        let mut file = File::from(file);

        let mut existing = Vec::new();
        (&file)
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut existing)?;
        if existing.len() as u64 > MAX_FILE_BYTES {
            return Err(refuse(format!(
                "{} is larger than {} MiB; not touching it",
                file_path.display(),
                MAX_FILE_BYTES >> 20
            )));
        }
        after_open();
        let (new, value) = change(&existing)?;
        if let Some(new) = new {
            if let Some(backup) = backup
                && !backup.used.replace(true)
                && !existing.is_empty()
            {
                let path = write_backup(&dir, &dir_path, &existing, backup.now)?;
                *backup.path.borrow_mut() = Some(path);
            }
            rewrite(&mut file, &existing, &new)?;
        }
        Ok(Modified { value, created })
    }

    /// Appends the bootstrap line.
    pub fn append(account: &Account, backup: &Backup, line: &str) -> io::Result<Modified<()>> {
        modify(account, true, Some(backup), &|| {}, |existing| {
            Ok((Some(with_line(existing, line)), ()))
        })
    }

    /// Removes every entry whose key has this fingerprint (any options, any comment). A missing
    /// file has none. Returns how many entries were dropped.
    pub fn remove(
        account: &Account,
        fingerprint: &str,
        backup: Option<&Backup>,
    ) -> io::Result<usize> {
        let done = modify(account, false, backup, &|| {}, |existing| {
            let (kept, dropped) = without(existing, |e| fingerprint_of(&e.blob) == fingerprint);
            Ok(((dropped > 0).then_some(kept), dropped))
        });
        match done {
            Ok(done) => Ok(done.value),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
            Err(error) => Err(error),
        }
    }

    /// In one write: drops the bootstrap entry (by fingerprint) and appends the phone's line (a
    /// key already present only loses the bootstrap entry). `Gone` when the entry is not there.
    pub fn replace(
        account: &Account,
        bootstrap: &str,
        phone: &KeyLine,
        device: &str,
        now: DateTime,
    ) -> io::Result<Replaced> {
        let done = modify(account, false, None, &|| {}, |existing| {
            let (kept, dropped) = without(existing, |e| fingerprint_of(&e.blob) == bootstrap);
            if dropped == 0 {
                return Ok((None, Replaced::Gone));
            }
            if contains(&kept, phone) {
                return Ok((Some(kept), Replaced::Done { added: false }));
            }
            let line = line_for(phone, device, &now.date());
            Ok((
                Some(with_line(&kept, &line)),
                Replaced::Done { added: true },
            ))
        });
        match done {
            Ok(done) => Ok(done.value),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Replaced::Gone),
            Err(error) => Err(error),
        }
    }

    /// Removes `or2-pair-bootstrap-<id>` entries for which `live(id)` is false. Returns the ids
    /// of the dropped entries (the caller removes their state files).
    pub fn sweep(
        account: &Account,
        backup: Option<&Backup>,
        live: &dyn Fn(&PairingId) -> bool,
    ) -> io::Result<Vec<PairingId>> {
        let done = modify(account, false, backup, &|| {}, |existing| {
            let ids = dead_ids(existing, live);
            let (kept, dropped) = without(existing, |e| dead_id(e, live).is_some());
            Ok(((dropped > 0).then_some(kept), ids))
        });
        match done {
            Ok(done) => Ok(done.value),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }

    /// What [`sweep`] would remove, changing nothing (for `--check`).
    pub fn stale(
        account: &Account,
        live: &dyn Fn(&PairingId) -> bool,
    ) -> io::Result<Vec<PairingId>> {
        match modify(account, false, None, &|| {}, |existing| {
            Ok((None, dead_ids(existing, live)))
        }) {
            Ok(done) => Ok(done.value),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }

    fn dead_id(entry: &Entry, live: &dyn Fn(&PairingId) -> bool) -> Option<PairingId> {
        entry
            .comment
            .strip_prefix(COMMENT_PREFIX)
            .and_then(|id| PairingId::parse(id).ok())
            .filter(|id| !live(id))
    }

    fn dead_ids(existing: &[u8], live: &dyn Fn(&PairingId) -> bool) -> Vec<PairingId> {
        existing
            .split(|byte| *byte == b'\n')
            .filter_map(|line| entry(&String::from_utf8_lossy(line)))
            .filter_map(|e| dead_id(&e, live))
            .collect()
    }

    /// Checks that `authorized_keys` (or, when it does not exist, `~/.ssh` or `~`) can be
    /// written and that sshd (StrictModes) would honour it: the same checks the changes make,
    /// changing nothing. A refusal says what to fix.
    pub fn writable(account: &Account) -> Writable {
        inspect(account).unwrap_or_else(|error| Writable::No(error.to_string()))
    }

    fn inspect(account: &Account) -> io::Result<Writable> {
        let dir_path = account.ssh_dir();
        let file_path = account.keys_path();
        let Some(dir) = open_ssh_dir(account, false)? else {
            let home = open_dir_path(&account.home)?;
            return Ok(if writable_by_us(home.as_raw_fd()) {
                Writable::Yes
            } else {
                Writable::No(format!("{} is not writable", account.home.display()))
            });
        };
        match open_file(
            dir.as_raw_fd(),
            &account.keys_name(),
            &file_path,
            libc::O_WRONLY,
            0,
        ) {
            Ok(file) => {
                check_file(&fstat(file.as_raw_fd())?, account.uid, &file_path)?;
                Ok(Writable::Yes)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Ok(if writable_by_us(dir.as_raw_fd()) {
                    Writable::Yes
                } else {
                    Writable::No(format!("{} is not writable", dir_path.display()))
                })
            }
            Err(error)
                if error.kind() == io::ErrorKind::PermissionDenied
                    && error.raw_os_error().is_some() =>
            {
                Ok(Writable::No(format!(
                    "{} is not writable ({error})",
                    file_path.display()
                )))
            }
            Err(error) => Err(error),
        }
    }
}

// The key-file implementation, and so its tests, are Unix-only.
#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    use super::*;
    use crate::bootstrap::PairingId;

    const ED25519: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBCJz8goXA2qGjRTNHhOwsljhOuCXG/+2B/zTJH/brc5";
    const OTHER: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7";
    const PHONE: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIALYvXruViE9G83T84ZJqbdJkEImlV0NRg9AC6Yw4NYo";
    const ECDSA: &str = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBJKzE3AhnQL0fsk77dWQaJJj+GHelMX7ge+TZ2xgJ/Y3O5aMMlnH6Hn/tnucj94OezBBvEj2VIcJGu8ABqUkhFA=";

    fn ssh_dir(home: &Path) -> PathBuf {
        home.join(".ssh")
    }

    fn path(home: &Path) -> PathBuf {
        ssh_dir(home).join("authorized_keys")
    }

    fn key(line: &str) -> KeyLine {
        KeyLine::parse(line).unwrap()
    }

    fn at() -> DateTime {
        DateTime::from_unix(1_782_867_661)
    }

    fn account(home: &Path) -> Account {
        Account::new("tester", home)
    }

    /// The outcome of appending a phone's line the way the replace does, for the file-handling
    /// tests below.
    #[derive(Debug, PartialEq, Eq)]
    enum Added {
        Added {
            created: bool,
            backup: Option<PathBuf>,
        },
        AlreadyPresent,
    }

    /// Appends the phone's line (a stand-in for the bootstrap append: the same handles).
    fn add_as(
        account: &Account,
        key: &KeyLine,
        device: &str,
        hook: &dyn Fn(),
    ) -> io::Result<Added> {
        let backup = Backup::new(at());
        let done = modify(account, true, Some(&backup), hook, |existing| {
            if contains(existing, key) {
                return Ok((None, false));
            }
            let line = line_for(key, device, &at().date());
            Ok((Some(with_line(existing, &line)), true))
        })?;
        Ok(if done.value {
            Added::Added {
                created: done.created,
                backup: backup.path(),
            }
        } else {
            Added::AlreadyPresent
        })
    }

    fn add(home: &Path, key: &KeyLine, device: &str) -> io::Result<Added> {
        add_as(&account(home), key, device, &|| {})
    }

    fn writable(home: &Path) -> Writable {
        super::writable(&account(home))
    }

    fn bootstrap_line(id: &str, blob_key: &str) -> String {
        format!("restrict,command=\"/bin/or2-pair enroll {id}\" {blob_key} or2-pair-bootstrap-{id}")
    }

    // --- no symlinks, no hard links, nothing that is not ours ------------------------------

    #[test]
    fn a_symlinked_authorized_keys_is_refused_and_its_target_untouched() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let target = elsewhere.path().join("someone-elses-keys");
        fs::write(&target, format!("{OTHER}\n")).unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        std::os::unix::fs::symlink(&target, path(home.path())).unwrap();
        let error = add(home.path(), &key(ED25519), "phone").unwrap_err();
        assert!(error.to_string().contains("symbolic link"), "{error}");
        assert_eq!(fs::read_to_string(&target).unwrap(), format!("{OTHER}\n"));
        let names: Vec<_> = fs::read_dir(ssh_dir(home.path())).unwrap().collect();
        assert_eq!(names.len(), 1, "no backup either");
    }

    #[test]
    fn a_symlinked_ssh_directory_is_refused_and_its_target_untouched() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), ssh_dir(home.path())).unwrap();
        let error = add(home.path(), &key(ED25519), "phone").unwrap_err();
        assert!(error.to_string().contains("symbolic link"), "{error}");
        assert_eq!(fs::read_dir(elsewhere.path()).unwrap().count(), 0);
    }

    #[test]
    fn an_authorized_keys_with_another_hard_link_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let other_name = elsewhere.path().join("alias");
        fs::create_dir(ssh_dir(home.path())).unwrap();
        fs::write(path(home.path()), format!("{OTHER}\n")).unwrap();
        fs::hard_link(path(home.path()), &other_name).unwrap();
        let error = add(home.path(), &key(ED25519), "phone").unwrap_err();
        assert!(error.to_string().contains("hard link"), "{error}");
        assert_eq!(
            fs::read_to_string(&other_name).unwrap(),
            format!("{OTHER}\n")
        );
    }

    #[test]
    fn files_that_belong_to_someone_else_are_refused() {
        // Pretend the account is another user: everything the test creates is then "foreign".
        let home = tempfile::tempdir().unwrap();
        let account = account(home.path());
        let other = account.uid.wrapping_add(1);
        fs::create_dir(ssh_dir(home.path())).unwrap();
        fs::write(path(home.path()), format!("{OTHER}\n")).unwrap();
        let foreign = Account {
            uid: other,
            ..account
        };
        let error = add_as(&foreign, &key(ED25519), "phone", &|| {}).unwrap_err();
        assert!(
            error.to_string().contains("belongs to another user"),
            "{error}"
        );
        assert_eq!(
            fs::read_to_string(path(home.path())).unwrap(),
            format!("{OTHER}\n")
        );
        // The check reports the same thing without changing anything.
        let result = super::writable(&foreign);
        assert!(matches!(result, Writable::No(why) if why.contains("another user")));
    }

    #[test]
    fn a_path_swapped_in_after_the_checks_is_not_followed() {
        // Between opening the checked handles and writing, `~/.ssh` is replaced by a link to
        // somewhere else. The backup and the write must still go through the handles that were
        // checked.
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        let before = format!("{OTHER}\n");
        fs::write(path(home.path()), &before).unwrap();
        let moved = home.path().join(".ssh-real");
        let swap = || {
            fs::rename(ssh_dir(home.path()), &moved).unwrap();
            std::os::unix::fs::symlink(elsewhere.path(), ssh_dir(home.path())).unwrap();
        };
        let added = add_as(&account(home.path()), &key(ED25519), "phone", &swap).unwrap();
        let Added::Added { backup, .. } = added else {
            panic!("{added:?}")
        };
        assert_eq!(
            fs::read_dir(elsewhere.path()).unwrap().count(),
            0,
            "nothing leaked"
        );
        let real = fs::read_to_string(moved.join("authorized_keys")).unwrap();
        assert!(real.starts_with(&before) && real.contains(ED25519));
        assert_eq!(
            fs::read_to_string(moved.join(backup.unwrap().file_name().unwrap())).unwrap(),
            before
        );
    }

    #[test]
    fn an_authorized_keys_that_is_not_a_regular_file_is_refused() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        fs::create_dir(path(home.path())).unwrap();
        assert!(add(home.path(), &key(ED25519), "phone").is_err());
    }

    /// A 0600 FIFO where `authorized_keys` should be, in a disposable home.
    fn home_with_a_fifo() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        let name =
            std::ffi::CString::new(path(home.path()).as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: `name` is a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        home
    }

    /// Runs `work` on its own thread and fails the test, instead of hanging it, when it blocks.
    fn within_seconds<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = send.send(work());
        });
        receive
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the call blocked on a FIFO instead of refusing it")
    }

    #[test]
    fn a_fifo_as_authorized_keys_is_refused_by_the_check_without_blocking() {
        // A blocking O_WRONLY open of a FIFO with no reader once hung `--check`.
        let home = home_with_a_fifo();
        let root = home.path().to_owned();
        let result = within_seconds(move || writable(&root));
        assert!(
            matches!(&result, Writable::No(why) if why.contains("not a regular file")),
            "{result:?}"
        );
        // With a reader on the other end the open would not have blocked; the answer is the same.
        let reader = {
            use std::os::unix::fs::OpenOptionsExt;
            fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(path(home.path()))
                .unwrap()
        };
        let root = home.path().to_owned();
        let result = within_seconds(move || writable(&root));
        assert!(
            matches!(&result, Writable::No(why) if why.contains("not a regular file")),
            "{result:?}"
        );
        drop(reader);
    }

    #[test]
    fn a_fifo_as_authorized_keys_is_refused_by_a_change_without_blocking_or_writing() {
        let home = home_with_a_fifo();
        let root = home.path().to_owned();
        let error = within_seconds(move || add(&root, &key(ED25519), "phone")).unwrap_err();
        assert!(error.to_string().contains("not a regular file"), "{error}");
        assert_eq!(fs::read_dir(ssh_dir(home.path())).unwrap().count(), 1);
    }

    #[test]
    fn other_special_files_are_refused_too() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        std::os::unix::net::UnixListener::bind(path(home.path())).unwrap();
        let root = home.path().to_owned();
        let result = within_seconds(move || writable(&root));
        assert!(matches!(result, Writable::No(_)), "{result:?}");
        assert!(add(home.path(), &key(ED25519), "phone").is_err());
    }

    #[test]
    fn creates_the_directory_and_file_with_private_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let added = add(home.path(), &key(ED25519), "Pixel-8").unwrap();
        assert_eq!(
            added,
            Added::Added {
                created: true,
                backup: None
            }
        );
        let contents = fs::read_to_string(path(home.path())).unwrap();
        assert_eq!(
            contents,
            format!("no-agent-forwarding,no-X11-forwarding {ED25519} or2-Pixel-8-2026-07-01\n")
        );
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&ssh_dir(home.path())), 0o700);
        assert_eq!(mode(&path(home.path())), 0o600);
    }

    #[test]
    fn appends_after_a_backup_and_keeps_what_was_there() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        let before = format!("# mine\n{OTHER} me@laptop\n");
        fs::write(path(home.path()), &before).unwrap();
        let added = add(home.path(), &key(ED25519), "phone").unwrap();
        let Added::Added { created, backup } = added else {
            panic!("{added:?}")
        };
        assert!(!created);
        let backup = backup.unwrap();
        assert_eq!(
            backup.file_name().unwrap(),
            "authorized_keys.or2-backup-20260701-010101"
        );
        assert_eq!(fs::read_to_string(&backup).unwrap(), before);
        let after = fs::read_to_string(path(home.path())).unwrap();
        assert!(after.starts_with(&before));
        assert_eq!(after.lines().count(), 3);
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn a_missing_final_newline_is_repaired_before_appending() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        fs::write(path(home.path()), OTHER).unwrap();
        add(home.path(), &key(ED25519), "phone").unwrap();
        let after = fs::read_to_string(path(home.path())).unwrap();
        let lines: Vec<_> = after.lines().collect();
        assert_eq!(lines[0], OTHER);
        assert!(lines[1].starts_with(OPTIONS));
    }

    #[test]
    fn a_duplicate_key_changes_nothing_even_with_options_or_a_comment() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        let existing = format!("command=\"/bin/true\",no-pty {ED25519} old-comment\n");
        fs::write(path(home.path()), &existing).unwrap();
        let added = add(home.path(), &key(ED25519), "phone").unwrap();
        assert_eq!(added, Added::AlreadyPresent);
        assert_eq!(fs::read_to_string(path(home.path())).unwrap(), existing);
        let names: Vec<_> = fs::read_dir(ssh_dir(home.path())).unwrap().collect();
        assert_eq!(names.len(), 1, "no backup for a no-op");
    }

    #[test]
    fn a_commented_out_copy_does_not_count() {
        let contents = format!("# {ED25519}\n");
        assert!(!contains(contents.as_bytes(), &key(ED25519)));
        assert!(contains(
            format!("{OPTIONS} {ED25519} c\n").as_bytes(),
            &key(ED25519)
        ));
        assert!(!contains(format!("{OTHER}\n").as_bytes(), &key(ED25519)));
    }

    // --- duplicates are the parsed key of an entry, not text anywhere on a line ---------------

    #[test]
    fn a_key_in_another_keys_comment_or_options_is_not_authorized() {
        let a = ED25519;
        let b = OTHER;
        for (what, line) in [
            ("a comment", format!("{a} migration note: was {b}")),
            ("a comment with options", format!("no-pty {a} also {b} x")),
            ("a quoted option", format!("command=\"echo {b}\" {a}")),
            (
                "an escaped quote in an option",
                format!("command=\"say \\\" {b} \\\"\",no-pty {a} c"),
            ),
            ("tabs between fields", format!("{a}\tc\t{b}")),
            ("a commented-out line", format!("# {b}")),
            ("an indented comment", format!("   # {b}")),
            (
                "a certificate for another key type",
                format!("ssh-ed25519-cert-v01@openssh.com {b}"),
            ),
            (
                "an unterminated quote",
                format!("command=\"never closed {b}"),
            ),
            ("just the algorithm", "ssh-ed25519".to_owned()),
        ] {
            let contents = format!("{line}\n");
            assert!(
                !contains(contents.as_bytes(), &key(b)),
                "{what}: {line:?} was taken for an authorization of the second key"
            );
        }
        // The key that really is the entry's key still counts, however it is dressed.
        assert!(contains(
            format!("{OPTIONS} {a} note: {b}\n").as_bytes(),
            &key(a)
        ));
    }

    #[test]
    fn an_entry_counts_for_its_own_key_whatever_precedes_and_follows_it() {
        let a = ED25519;
        for line in [
            a.to_owned(),
            format!("  {a}"),
            format!("{a} comment with spaces"),
            format!("{a}\r"),
            format!("from=\"10.0.0.0/8,*.lan\" {a} c"),
            format!("command=\"/bin/true a b\",no-pty,environment=\"K=V W\" {a}"),
            format!("command=\"say \\\"hi\\\"\" {a}"),
            format!("restrict,port-forwarding\t{a}"),
        ] {
            assert!(
                contains(format!("x\n{line}\ny\n").as_bytes(), &key(a)),
                "{line:?} is an entry for the key"
            );
        }
    }

    #[test]
    fn an_entrys_comment_is_its_first_word_after_the_key() {
        let line =
            format!("restrict,command=\"/x enroll abc\" {ED25519} or2-pair-bootstrap-abc more");
        assert_eq!(entry(&line).unwrap().comment, "or2-pair-bootstrap-abc");
        assert_eq!(entry(ED25519).unwrap().comment, "");
    }

    #[test]
    fn pairing_a_key_that_only_a_comment_mentions_adds_it() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        let before = format!("{ED25519} migrate to {OTHER} later\n");
        fs::write(path(home.path()), &before).unwrap();
        let added = add(home.path(), &key(OTHER), "phone").unwrap();
        assert!(
            matches!(
                added,
                Added::Added {
                    backup: Some(_),
                    ..
                }
            ),
            "{added:?}"
        );
        let after = fs::read_to_string(path(home.path())).unwrap();
        assert!(after.starts_with(&before));
        assert!(
            after
                .lines()
                .last()
                .unwrap()
                .contains(&format!("{OTHER} or2-phone-"))
        );
    }

    #[test]
    fn an_empty_existing_file_gets_no_backup() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        fs::write(path(home.path()), "").unwrap();
        let added = add(home.path(), &key(ED25519), "phone").unwrap();
        assert_eq!(
            added,
            Added::Added {
                created: false,
                backup: None
            }
        );
    }

    #[test]
    fn two_runs_in_one_second_do_not_overwrite_each_others_backup() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        fs::write(path(home.path()), format!("{OTHER}\n")).unwrap();
        let Added::Added {
            backup: Some(first),
            ..
        } = add(home.path(), &key(ED25519), "a").unwrap()
        else {
            panic!()
        };
        let Added::Added {
            backup: Some(second),
            ..
        } = add(home.path(), &key(ECDSA), "b").unwrap()
        else {
            panic!()
        };
        assert_ne!(first, second);
        assert_eq!(fs::read_to_string(&first).unwrap().lines().count(), 1);
        assert_eq!(fs::read_to_string(&second).unwrap().lines().count(), 2);
    }

    #[test]
    fn a_device_label_is_reduced_to_safe_ascii() {
        for (raw, safe) in [
            ("Pixel 8", Some("Pixel-8")),
            ("  Pixel   8  ", Some("Pixel-8")),
            ("a\nb", Some("a-b")),
            ("a\x1b[31mb", Some("a-31mb")),
            ("Büro-Phone", Some("B-ro-Phone")),
            ("../../etc", Some("..-..-etc")),
            ("---", None),
            ("", None),
            ("\n\t", None),
            ("é", None),
            ("my_phone.1", Some("my_phone.1")),
        ] {
            assert_eq!(sanitize_device(raw).as_deref(), safe, "{raw:?}");
        }
        let long = "x".repeat(80);
        assert_eq!(sanitize_device(&long).unwrap().len(), 32);
    }

    // --- what sshd's StrictModes would ignore is refused, not written to -----------------------

    fn chmod(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn existing_file(home: &Path, mode: u32) -> String {
        fs::create_dir_all(ssh_dir(home)).unwrap();
        let before = format!("{OTHER}\n");
        fs::write(path(home), &before).unwrap();
        chmod(&path(home), mode);
        before
    }

    #[test]
    fn an_authorized_keys_writable_by_group_or_others_is_refused_and_left_alone() {
        for mode in [0o664, 0o666, 0o662, 0o620] {
            let home = tempfile::tempdir().unwrap();
            let before = existing_file(home.path(), mode);
            let error = add(home.path(), &key(ED25519), "phone").unwrap_err();
            let text = error.to_string();
            assert!(
                text.contains("writable by other users")
                    && text.contains("StrictModes")
                    && text.contains("chmod go-w"),
                "{mode:o}: {text}"
            );
            assert_eq!(fs::read_to_string(path(home.path())).unwrap(), before);
            assert_eq!(
                fs::read_dir(ssh_dir(home.path())).unwrap().count(),
                1,
                "no backup of a file that was refused"
            );
            // The check says the same, without changing anything.
            let result = writable(home.path());
            assert!(
                matches!(&result, Writable::No(why) if why.contains("chmod go-w")),
                "{mode:o}: {result:?}"
            );
        }
    }

    #[test]
    fn modes_sshd_accepts_are_written_to_and_keep_their_mode() {
        use std::os::unix::fs::PermissionsExt;
        for mode in [0o600, 0o640, 0o644] {
            let home = tempfile::tempdir().unwrap();
            existing_file(home.path(), mode);
            assert!(matches!(
                add(home.path(), &key(ED25519), "phone"),
                Ok(Added::Added { .. })
            ));
            let now = fs::metadata(path(home.path()))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(now & 0o777, mode);
        }
    }

    #[test]
    fn an_ssh_directory_or_home_writable_by_others_is_refused() {
        for mode in [0o775, 0o777, 0o770 | 0o002] {
            let home = tempfile::tempdir().unwrap();
            existing_file(home.path(), 0o600);
            chmod(&ssh_dir(home.path()), mode);
            let error = add(home.path(), &key(ED25519), "phone").unwrap_err();
            assert!(
                error.to_string().contains("writable by other users")
                    && error.to_string().contains(".ssh"),
                "{mode:o}: {error}"
            );
            chmod(&ssh_dir(home.path()), 0o700);
            // The home directory itself.
            chmod(home.path(), mode);
            let error = add(home.path(), &key(ED25519), "phone").unwrap_err();
            assert!(
                error.to_string().contains("writable by other users"),
                "{mode:o}: {error}"
            );
            chmod(home.path(), 0o755);
            assert!(add(home.path(), &key(ED25519), "phone").is_ok());
        }
    }

    #[test]
    fn checks_report_writability_and_strict_modes_trouble() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        fs::set_permissions(home.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(writable(home.path()), Writable::Yes);
        // A home directory writable by group or others is a StrictModes refusal.
        fs::set_permissions(home.path(), fs::Permissions::from_mode(0o775)).unwrap();
        assert!(matches!(writable(home.path()), Writable::No(why) if why.contains("StrictModes")));
        // A read-only existing file is reported, not changed.
        fs::set_permissions(home.path(), fs::Permissions::from_mode(0o755)).unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        fs::write(path(home.path()), "").unwrap();
        fs::set_permissions(path(home.path()), fs::Permissions::from_mode(0o400)).unwrap();
        let result = writable(home.path());
        // Running as root can write a 0400 file; everyone else cannot.
        if fs::OpenOptions::new()
            .append(true)
            .open(path(home.path()))
            .is_err()
        {
            assert!(matches!(result, Writable::No(_)));
        }
    }

    #[test]
    fn an_explicit_key_file_is_opened_by_its_directory() {
        // The test-support layout: a disposable sshd's AuthorizedKeysFile, outside any ~/.ssh.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("authorized");
        fs::write(&file, format!("{OTHER}\n")).unwrap();
        let account = Account::new("tester", dir.path().join("no-such-home")).with_keys_file(&file);
        let added = add_as(&account, &key(ED25519), "phone", &|| {}).unwrap();
        assert!(matches!(added, Added::Added { .. }));
        assert!(fs::read_to_string(&file).unwrap().contains(ED25519));
        assert_eq!(super::writable(&account), Writable::Yes);
    }

    // --- remove, replace, sweep ----------------------------------------------------------------

    #[test]
    fn remove_drops_the_entry_in_place_and_keeps_every_other_line_byte_for_byte() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        let boot = key(ED25519);
        let mine = format!("# mine\r\n{OTHER}  me@laptop\n\n   {ECDSA}");
        let contents = format!(
            "{mine}\n{}\n{ED25519}\n",
            bootstrap_line("abcdefghijklm", ED25519)
        );
        // Both the bootstrap line and a bare copy of the same key go (any options, any comment).
        fs::write(path(home.path()), &contents).unwrap();
        chmod(&path(home.path()), 0o640);
        let inode = fs::metadata(path(home.path())).unwrap().ino();

        let removed = remove(&account(home.path()), &boot.fingerprint(), None).unwrap();
        assert_eq!(removed, 2);
        assert_eq!(
            fs::read_to_string(path(home.path())).unwrap(),
            format!("{mine}\n")
        );
        let meta = fs::metadata(path(home.path())).unwrap();
        assert_eq!(meta.ino(), inode, "rewritten in place");
        assert_eq!(meta.mode() & 0o777, 0o640, "mode kept");
        // Nothing left to remove: no change, no error; a missing file is the same.
        assert_eq!(
            remove(&account(home.path()), &boot.fingerprint(), None).unwrap(),
            0
        );
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            remove(&account(empty.path()), &boot.fingerprint(), None).unwrap(),
            0
        );
        assert!(
            !ssh_dir(empty.path()).exists(),
            "nothing is created to remove from"
        );
    }

    #[test]
    fn replace_swaps_the_bootstrap_entry_for_the_phones_key_in_one_write() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        let boot = key(ED25519);
        let before = format!(
            "{OTHER} me\n{}\n{ECDSA} other\n",
            bootstrap_line("abcdefghijklm", ED25519)
        );
        fs::write(path(home.path()), &before).unwrap();
        let inode = fs::metadata(path(home.path())).unwrap().ino();
        let phone = key(PHONE);
        let done = replace(
            &account(home.path()),
            &boot.fingerprint(),
            &phone,
            "Pixel-8",
            at(),
        )
        .unwrap();
        assert_eq!(done, Replaced::Done { added: true });
        let after = fs::read_to_string(path(home.path())).unwrap();
        assert_eq!(
            after,
            format!(
                "{OTHER} me\n{ECDSA} other\n{OPTIONS} {} or2-Pixel-8-2026-07-01\n",
                phone.openssh()
            )
        );
        assert_eq!(fs::metadata(path(home.path())).unwrap().ino(), inode);
        // The bootstrap entry is gone, so a second phone finds nothing to replace.
        let again = replace(
            &account(home.path()),
            &boot.fingerprint(),
            &key(ECDSA),
            "b",
            at(),
        )
        .unwrap();
        assert_eq!(again, Replaced::Gone);
        assert_eq!(fs::read_to_string(path(home.path())).unwrap(), after);
        // And a file that does not exist at all is `Gone` too.
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            replace(
                &account(empty.path()),
                &boot.fingerprint(),
                &phone,
                "x",
                at()
            )
            .unwrap(),
            Replaced::Gone
        );
    }

    #[test]
    fn replace_with_a_key_that_is_already_authorized_only_removes_the_bootstrap_entry() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        let boot = key(ED25519);
        let before = format!(
            "{}\n{OTHER} mine\n",
            bootstrap_line("abcdefghijklm", ED25519)
        );
        fs::write(path(home.path()), &before).unwrap();
        let done = replace(
            &account(home.path()),
            &boot.fingerprint(),
            &key(OTHER),
            "p",
            at(),
        )
        .unwrap();
        assert_eq!(done, Replaced::Done { added: false });
        assert_eq!(
            fs::read_to_string(path(home.path())).unwrap(),
            format!("{OTHER} mine\n")
        );
    }

    #[test]
    fn a_run_makes_one_backup_before_its_first_change() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        let original = format!("{OTHER} me\n");
        fs::write(path(home.path()), &original).unwrap();
        let account = account(home.path());
        let backup = Backup::new(at());
        let line = bootstrap_line("abcdefghijklm", ED25519);
        append(&account, &backup, &line).unwrap();
        let saved = backup
            .path()
            .expect("the first change saves the file as it was");
        assert_eq!(fs::read_to_string(&saved).unwrap(), original);
        // The removal of the same run changes the file again, with no second backup.
        remove(&account, &key(ED25519).fingerprint(), Some(&backup)).unwrap();
        assert_eq!(fs::read_to_string(path(home.path())).unwrap(), original);
        let backups = fs::read_dir(ssh_dir(home.path()))
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains("or2-backup")
            })
            .count();
        assert_eq!(backups, 1);
    }

    #[test]
    fn sweep_drops_only_the_bootstrap_entries_that_are_not_live() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        let (dead, live, odd) = ("aaaaaaaaaaaaa", "bbbbbbbbbbbbb", "ccccccccccccc");
        let contents = format!(
            "{}\n{}\n{OTHER} or2-pair-bootstrap-not-an-id\n{}\n{ECDSA} keep\n",
            bootstrap_line(dead, ED25519),
            bootstrap_line(live, OTHER),
            bootstrap_line(odd, ED25519),
        );
        fs::write(path(home.path()), &contents).unwrap();
        let alive = |id: &PairingId| id.as_str() == live;
        let dropped = sweep(&account(home.path()), None, &alive).unwrap();
        let ids: Vec<&str> = dropped.iter().map(PairingId::as_str).collect();
        assert_eq!(ids, [dead, odd]);
        let after = fs::read_to_string(path(home.path())).unwrap();
        assert_eq!(
            after,
            format!(
                "{}\n{OTHER} or2-pair-bootstrap-not-an-id\n{ECDSA} keep\n",
                bootstrap_line(live, OTHER)
            )
        );
        // Nothing dead: no change.
        assert!(
            sweep(&account(home.path()), None, &alive)
                .unwrap()
                .is_empty()
        );
    }
}
