//! `~/.ssh/authorized_keys`: where a confirmed key is appended, and the care taken doing it.
//!
//! - the home directory is opened once and everything else is done relative to the directory
//!   handles that were checked, never by path again: `~/.ssh` and the file are opened with
//!   `O_NOFOLLOW` (a symbolic link is refused, not followed), their owner must be the account,
//!   the file must be a regular file with no other hard link, and the read, the backup and the
//!   append all go through those same handles, so a path swapped in between changes nothing
//!   (Unix; Windows refuses symbolic links and junctions by path, which is weaker),
//! - `~/.ssh` is created with mode 0700 and the file with 0600 when missing (permissions are
//!   Unix-only; on Windows the inherited ACLs apply),
//! - the file is backed up before it is touched (`authorized_keys.or2-backup-<stamp>`, mode
//!   0600), once per run, only when it exists and is not empty,
//! - a key already present (same algorithm and key data, whatever its options or comment) is
//!   not added again, and then nothing is changed at all, not even a backup,
//! - the line is `no-agent-forwarding,no-X11-forwarding <algorithm> <key> or2-<device>-<date>`,
//!   rebuilt from the validated key, so nothing from the phone but the key itself is written,
//! - a missing final newline is repaired first, so the new line never joins the last one.

use std::io;
use std::path::{Path, PathBuf};

use crate::account::Account;
use crate::date::DateTime;
use crate::keyline::KeyLine;

/// The options every key added by or2 carries.
pub const OPTIONS: &str = "no-agent-forwarding,no-X11-forwarding";

/// An `authorized_keys` larger than this is not read into memory (nobody's real one is).
pub const MAX_FILE_BYTES: u64 = 8 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Added {
    /// The line was appended. `backup` is the copy of the previous file, if there was one.
    Added {
        created: bool,
        backup: Option<PathBuf>,
    },
    /// The key was already authorized; the file is untouched.
    AlreadyPresent,
}

pub fn ssh_dir(home: &Path) -> PathBuf {
    home.join(".ssh")
}

pub fn path(home: &Path) -> PathBuf {
    ssh_dir(home).join("authorized_keys")
}

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

/// The line that is appended.
pub fn line_for(key: &KeyLine, device: &str, date: &str) -> String {
    format!("{OPTIONS} {} or2-{device}-{date}", key.openssh())
}

/// Whether some entry of `contents` authorizes `key`: its parsed key type and key data equal
/// the key's, whatever options precede it and whatever comment follows. Text elsewhere on a line
/// (a comment that mentions a key, a quoted option) is not an authorization, exactly as sshd
/// reads the file.
pub fn contains(contents: &[u8], key: &KeyLine) -> bool {
    contents
        .split(|byte| *byte == b'\n')
        .filter_map(|line| entry_key(&String::from_utf8_lossy(line)))
        .any(|(algorithm, blob)| algorithm == key.algorithm() && blob == key.blob())
}

/// Whether `token` names a public key type (sshd tells the key from the options this way: an
/// options field never starts like one).
fn is_key_type(token: &str) -> bool {
    token.starts_with("ssh-")
        || token.starts_with("ecdsa-sha2-")
        || token.starts_with("sk-")
        || token.ends_with("-cert-v01@openssh.com")
}

/// The key type and decoded key data of one `authorized_keys` line, or `None` for a blank line,
/// a comment, or a line that is not a well-formed entry (an unterminated quote in the options,
/// no key data, key data that is not base64).
///
/// A line is `[options] keytype base64 [comment]`. The options are comma-separated and may
/// contain whitespace inside double quotes (with `\"` for a quote inside quotes); the field ends
/// at the first whitespace outside quotes.
fn entry_key(line: &str) -> Option<(String, Vec<u8>)> {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;

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
    Some((algorithm.to_owned(), blob))
}

fn refuse(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message.into())
}

/// The line to append, with a newline first when the file does not end in one.
fn text_to_append(existing: &[u8], key: &KeyLine, device: &str, now: DateTime) -> String {
    let mut text = String::new();
    if existing.last().is_some_and(|last| *last != b'\n') {
        text.push('\n');
    }
    text.push_str(&line_for(key, device, &now.date()));
    text.push('\n');
    text
}

fn backup_name(now: DateTime, attempt: u32) -> String {
    let suffix = if attempt == 0 {
        String::new()
    } else {
        format!("-{attempt}")
    };
    format!("authorized_keys.or2-backup-{}{suffix}", now.stamp())
}

/// Appends the key to the account's `authorized_keys`, as the module docs describe.
pub fn add(account: &Account, key: &KeyLine, device: &str, now: DateTime) -> io::Result<Added> {
    add_hooked(account, key, device, now, &|| {})
}

/// Whether the key could be added right now, without adding it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Writable {
    Yes,
    /// Not possible; why, for the check's message.
    No(String),
}

#[cfg(unix)]
use unix::add_hooked;
#[cfg(unix)]
pub use unix::writable;

#[cfg(not(unix))]
use portable::add_hooked;
#[cfg(not(unix))]
pub use portable::writable;

#[cfg(unix)]
mod unix {
    use std::ffi::CString;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::ffi::OsStrExt;

    use super::*;

    fn c_name(name: &str) -> CString {
        CString::new(name).expect("a file name without NUL")
    }

    fn retry<T: PartialEq + Copy>(bad: T, mut call: impl FnMut() -> T) -> io::Result<T> {
        loop {
            let value = call();
            if value != bad {
                return Ok(value);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    /// Opens the home directory by its path. The path itself may pass through links (the
    /// account database says where the home is); everything below it never does.
    fn open_home(home: &Path) -> io::Result<OwnedFd> {
        let path = CString::new(home.as_os_str().as_bytes())
            .map_err(|_| refuse("the home directory path contains a NUL byte"))?;
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;
        // SAFETY: `path` is a valid NUL-terminated string.
        let fd = retry(-1, || unsafe { libc::open(path.as_ptr(), flags) })?;
        // SAFETY: `fd` is a freshly opened descriptor that nothing else owns.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn openat(
        dir: RawFd,
        name: &str,
        flags: libc::c_int,
        mode: libc::mode_t,
    ) -> io::Result<OwnedFd> {
        let name = c_name(name);
        let flags = flags | libc::O_CLOEXEC;
        // SAFETY: `name` is a valid NUL-terminated string and `dir` an open directory.
        let fd = retry(-1, || unsafe {
            libc::openat(dir, name.as_ptr(), flags, libc::c_uint::from(mode))
        })?;
        // SAFETY: `fd` is a freshly opened descriptor that nothing else owns.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn fstat(fd: RawFd) -> io::Result<libc::stat> {
        // SAFETY: `stat` is plain old data and `fstat` fills it.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        retry(-1, || unsafe { libc::fstat(fd, &mut stat) })?;
        Ok(stat)
    }

    /// What `name` is inside `dir`, without following it, when it is a symbolic link.
    fn is_symlink_at(dir: RawFd, name: &str) -> bool {
        let name = c_name(name);
        // SAFETY: as above.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        let done =
            unsafe { libc::fstatat(dir, name.as_ptr(), &mut stat, libc::AT_SYMLINK_NOFOLLOW) };
        done == 0 && (stat.st_mode & libc::S_IFMT) == libc::S_IFLNK
    }

    fn mkdirat(dir: RawFd, name: &str, mode: libc::mode_t) -> io::Result<()> {
        let name = c_name(name);
        // SAFETY: as above.
        retry(-1, || unsafe { libc::mkdirat(dir, name.as_ptr(), mode) }).map(|_| ())
    }

    fn fchmod(fd: RawFd, mode: libc::mode_t) -> io::Result<()> {
        // SAFETY: `fd` is open.
        retry(-1, || unsafe { libc::fchmod(fd, mode) }).map(|_| ())
    }

    fn lock(fd: RawFd) -> io::Result<()> {
        // SAFETY: `fd` is open.
        let done = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if done == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::WouldBlock {
            Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "another program has authorized_keys locked; try again in a moment",
            ))
        } else {
            Err(error)
        }
    }

    fn writable_by_us(dir: RawFd) -> bool {
        // SAFETY: `dir` is open and "." is a valid string.
        unsafe { libc::faccessat(dir, c".".as_ptr(), libc::W_OK, 0) == 0 }
    }

    /// Opens `name` below `dir` without following links; a refusal says what was in the way.
    fn open_nofollow(
        dir: RawFd,
        name: &str,
        full: &Path,
        flags: libc::c_int,
        mode: libc::mode_t,
    ) -> io::Result<OwnedFd> {
        openat(dir, name, flags | libc::O_NOFOLLOW, mode).map_err(|error| {
            if is_symlink_at(dir, name) {
                refuse(format!(
                    "{} is a symbolic link; or2-pair will not follow it (replace it with a real {} and run again)",
                    full.display(),
                    if flags & libc::O_DIRECTORY != 0 { "directory" } else { "file" }
                ))
            } else if error.raw_os_error() == Some(libc::ENOTDIR) && flags & libc::O_DIRECTORY != 0 {
                refuse(format!("{} is not a directory", full.display()))
            } else {
                error
            }
        })
    }

    /// Opens an existing or new *file* below `dir` for `check_file` to judge: never following a
    /// link and never blocking on what it finds. A FIFO opened for writing would block until a
    /// reader turned up (and a device or socket can fail oddly), so the open is non-blocking, the
    /// descriptor is `fstat`ed, and anything that is not a regular file is refused *before* the
    /// non-blocking flag is cleared and the handle used. Only the verified descriptor is kept.
    fn open_file(
        dir: RawFd,
        name: &str,
        full: &Path,
        flags: libc::c_int,
        mode: libc::mode_t,
    ) -> io::Result<OwnedFd> {
        let not_regular = || refuse(format!("{} is not a regular file", full.display()));
        let fd = match open_nofollow(dir, name, full, flags | libc::O_NONBLOCK, mode) {
            Ok(fd) => fd,
            // Opening a FIFO for writing with no reader, or a socket or device, fails with ENXIO:
            // say what it is rather than the errno.
            Err(error) if error.raw_os_error() == Some(libc::ENXIO) => return Err(not_regular()),
            Err(error) => return Err(error),
        };
        if kind(&fstat(fd.as_raw_fd())?) != libc::S_IFREG {
            return Err(not_regular());
        }
        // A regular file: ordinary blocking I/O from here.
        // SAFETY: `fd` is open.
        let status = retry(-1, || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) })?;
        retry(-1, || unsafe {
            libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, status & !libc::O_NONBLOCK)
        })?;
        Ok(fd)
    }

    fn kind(stat: &libc::stat) -> libc::mode_t {
        stat.st_mode & libc::S_IFMT
    }

    /// What sshd's StrictModes refuses: the home directory, `~/.ssh` and `authorized_keys`
    /// writable by group or others. Appending to such a file would "work" and the phone would
    /// still be turned away, so it is refused here, with the command that fixes it. Nothing is
    /// changed behind the person's back.
    fn check_strict_modes(stat: &libc::stat, path: &Path, what: &str) -> io::Result<()> {
        // `st_mode` is 16 bits wide on macOS and 32 on Linux.
        #[allow(clippy::useless_conversion)]
        let mode = u32::from(stat.st_mode) & 0o7777;
        if mode & 0o022 != 0 {
            return Err(refuse(format!(
                "{} ({what}) is writable by other users (mode {mode:04o}): sshd (StrictModes) ignores authorized_keys when it, ~/.ssh or the home directory can be written by group or others, so a key added now would still be refused. Run `chmod go-w {}` and pair again",
                path.display(),
                path.display()
            )));
        }
        Ok(())
    }

    /// The home directory must be a directory of the account (or root's, as sshd allows).
    fn check_home(stat: &libc::stat, uid: u32, home: &Path) -> io::Result<()> {
        if stat.st_uid != uid && stat.st_uid != 0 {
            return Err(refuse(format!(
                "{} belongs to another user (user id {}); or2-pair only changes the home directory of the account it runs as",
                home.display(),
                stat.st_uid
            )));
        }
        check_strict_modes(stat, home, "the home directory")
    }

    fn check_ssh_dir(stat: &libc::stat, uid: u32, dir: &Path) -> io::Result<()> {
        if stat.st_uid != uid {
            return Err(refuse(format!(
                "{} belongs to another user (user id {}), not to the account or2-pair runs as; sshd would not use it",
                dir.display(),
                stat.st_uid
            )));
        }
        check_strict_modes(stat, dir, "the SSH directory")
    }

    fn check_file(stat: &libc::stat, uid: u32, file: &Path) -> io::Result<()> {
        if kind(stat) != libc::S_IFREG {
            return Err(refuse(format!("{} is not a regular file", file.display())));
        }
        if stat.st_uid != uid {
            return Err(refuse(format!(
                "{} belongs to another user (user id {}), not to the account or2-pair runs as",
                file.display(),
                stat.st_uid
            )));
        }
        check_strict_modes(stat, file, "the key file")?;
        if stat.st_nlink > 1 {
            return Err(refuse(format!(
                "{} has another hard link ({} names for one file); appending would change the other name too. Replace it with a plain copy and run again",
                file.display(),
                stat.st_nlink
            )));
        }
        Ok(())
    }

    /// Opens `~/.ssh`, creating it (mode 0700) when `create` and it is missing.
    fn open_ssh(
        home: &OwnedFd,
        dir_path: &Path,
        uid: u32,
        create: bool,
    ) -> io::Result<Option<OwnedFd>> {
        let flags = libc::O_RDONLY | libc::O_DIRECTORY;
        let opened = match open_nofollow(home.as_raw_fd(), ".ssh", dir_path, flags, 0) {
            Ok(fd) => fd,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !create {
                    return Ok(None);
                }
                match mkdirat(home.as_raw_fd(), ".ssh", 0o700) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
                let fd = open_nofollow(home.as_raw_fd(), ".ssh", dir_path, flags, 0)?;
                // The mode asked of `mkdirat` is cut by the umask; the directory is ours, set it.
                if fstat(fd.as_raw_fd())?.st_uid == uid {
                    fchmod(fd.as_raw_fd(), 0o700)?;
                }
                fd
            }
            Err(error) => return Err(error),
        };
        check_ssh_dir(&fstat(opened.as_raw_fd())?, uid, dir_path)?;
        Ok(Some(opened))
    }

    pub(super) fn add_hooked(
        account: &Account,
        key: &KeyLine,
        device: &str,
        now: DateTime,
        after_open: &dyn Fn(),
    ) -> io::Result<Added> {
        let uid = account.uid;
        let dir_path = ssh_dir(&account.home);
        let file_path = path(&account.home);
        let home = open_home(&account.home)?;
        check_home(&fstat(home.as_raw_fd())?, uid, &account.home)?;
        let dir = open_ssh(&home, &dir_path, uid, true)?
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;

        // The file: an existing one is opened as it is; a missing one is created exclusively, so
        // a path planted in between is refused rather than written through.
        let append = libc::O_RDWR | libc::O_APPEND;
        let (file, created) =
            match open_file(dir.as_raw_fd(), "authorized_keys", &file_path, append, 0) {
                Ok(fd) => (fd, false),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    let make = append | libc::O_CREAT | libc::O_EXCL;
                    let fd =
                        open_file(dir.as_raw_fd(), "authorized_keys", &file_path, make, 0o600)?;
                    fchmod(fd.as_raw_fd(), 0o600)?;
                    (fd, true)
                }
                Err(error) => return Err(error),
            };
        check_file(&fstat(file.as_raw_fd())?, uid, &file_path)?;
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
        if contains(&existing, key) {
            return Ok(Added::AlreadyPresent);
        }

        let backup = if existing.is_empty() {
            None
        } else {
            Some(write_backup(&dir, &dir_path, &existing, now)?)
        };

        let text = text_to_append(&existing, key, device, now);
        let length = existing.len() as u64;
        if let Err(error) = file
            .write_all(text.as_bytes())
            .and_then(|()| file.sync_all())
        {
            // Leave the file as it was found, not with half a line.
            let _ = file.set_len(length);
            return Err(error);
        }
        Ok(Added::Added { created, backup })
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

    /// Checks that `authorized_keys` (or, when it does not exist, `~/.ssh` or `~`) can be
    /// written and that sshd (StrictModes) would honour it: the same checks `add` makes,
    /// changing nothing. A refusal says what to fix.
    pub fn writable(account: &Account) -> Writable {
        inspect(account)
    }

    fn inspect(account: &Account) -> Writable {
        let uid = account.uid;
        let dir_path = ssh_dir(&account.home);
        let file_path = path(&account.home);
        let result = (|| -> io::Result<Writable> {
            let home = open_home(&account.home).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "{} is not writable: it cannot be opened ({error})",
                        account.home.display()
                    ),
                )
            })?;
            check_home(&fstat(home.as_raw_fd())?, uid, &account.home)?;
            let Some(dir) = open_ssh(&home, &dir_path, uid, false)? else {
                return Ok(if writable_by_us(home.as_raw_fd()) {
                    Writable::Yes
                } else {
                    Writable::No(format!("{} is not writable", account.home.display()))
                });
            };
            match open_file(
                dir.as_raw_fd(),
                "authorized_keys",
                &file_path,
                libc::O_WRONLY | libc::O_APPEND,
                0,
            ) {
                Ok(file) => {
                    check_file(&fstat(file.as_raw_fd())?, uid, &file_path)?;
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
        })();
        result.unwrap_or_else(|error| Writable::No(error.to_string()))
    }

    #[cfg(test)]
    pub(super) fn add_as(
        uid: u32,
        account: &Account,
        key: &KeyLine,
        device: &str,
        now: DateTime,
        after_open: &dyn Fn(),
    ) -> io::Result<Added> {
        let account = Account {
            uid,
            ..account.clone()
        };
        add_hooked(&account, key, device, now, after_open)
    }
}

#[cfg(not(unix))]
mod portable {
    use std::fs::{self, OpenOptions};
    use std::io::Write;

    use super::*;

    fn private_options() -> OpenOptions {
        OpenOptions::new()
    }

    fn not_a_link(path: &Path) -> io::Result<()> {
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => Err(refuse(format!(
                "{} is a symbolic link; or2-pair will not follow it",
                path.display()
            ))),
            Ok(_) | Err(_) => Ok(()),
        }
    }

    pub(super) fn add_hooked(
        account: &Account,
        key: &KeyLine,
        device: &str,
        now: DateTime,
        after_open: &dyn Fn(),
    ) -> io::Result<Added> {
        let dir = ssh_dir(&account.home);
        let file = path(&account.home);
        not_a_link(&dir)?;
        if !dir.exists() {
            fs::create_dir(&dir)?;
        }
        not_a_link(&file)?;
        let existing = match fs::read(&file) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        after_open();
        if existing
            .as_deref()
            .is_some_and(|bytes| contains(bytes, key))
        {
            return Ok(Added::AlreadyPresent);
        }
        let backup = match existing.as_deref() {
            Some(bytes) if !bytes.is_empty() => Some(write_backup(&dir, bytes, now)?),
            _ => None,
        };
        let mut out = private_options().create(true).append(true).open(&file)?;
        let text = text_to_append(existing.as_deref().unwrap_or_default(), key, device, now);
        out.write_all(text.as_bytes())?;
        out.sync_all()?;
        Ok(Added::Added {
            created: existing.is_none(),
            backup,
        })
    }

    fn write_backup(dir: &Path, contents: &[u8], now: DateTime) -> io::Result<PathBuf> {
        for attempt in 0..100 {
            let backup = dir.join(backup_name(now, attempt));
            match private_options().write(true).create_new(true).open(&backup) {
                Ok(mut out) => {
                    out.write_all(contents)?;
                    out.sync_all()?;
                    return Ok(backup);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::other("could not find a free backup name"))
    }

    pub fn writable(account: &Account) -> Writable {
        let home = &account.home;
        let file = path(home);
        if let Err(error) = not_a_link(&ssh_dir(home)).and_then(|()| not_a_link(&file)) {
            Writable::No(error.to_string())
        } else if file.exists() {
            match OpenOptions::new().append(true).open(&file) {
                Ok(_) => Writable::Yes,
                Err(error) => Writable::No(format!("{} is not writable ({error})", file.display())),
            }
        } else if ssh_dir(home).exists() || home.exists() {
            Writable::Yes
        } else {
            Writable::No(format!("{} is not writable", home.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// `add` for the account of a temporary home.
    fn add(home: &Path, key: &KeyLine, device: &str, now: DateTime) -> io::Result<Added> {
        super::add(&Account::new("tester", home), key, device, now)
    }

    fn writable(home: &Path) -> Writable {
        super::writable(&Account::new("tester", home))
    }

    const ED25519: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBCJz8goXA2qGjRTNHhOwsljhOuCXG/+2B/zTJH/brc5";
    const OTHER: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7";

    fn key(line: &str) -> KeyLine {
        KeyLine::parse(line).unwrap()
    }

    fn at() -> DateTime {
        DateTime::from_unix(1_782_867_661)
    }

    // --- Finding 2: no symlinks, no hard links, nothing that is not ours --------------------

    #[cfg(unix)]
    #[test]
    fn a_symlinked_authorized_keys_is_refused_and_its_target_untouched() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let target = elsewhere.path().join("someone-elses-keys");
        fs::write(&target, format!("{OTHER}\n")).unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        std::os::unix::fs::symlink(&target, path(home.path())).unwrap();
        let error = add(home.path(), &key(ED25519), "phone", at()).unwrap_err();
        assert!(error.to_string().contains("symbolic link"), "{error}");
        assert_eq!(fs::read_to_string(&target).unwrap(), format!("{OTHER}\n"));
        let names: Vec<_> = fs::read_dir(ssh_dir(home.path())).unwrap().collect();
        assert_eq!(names.len(), 1, "no backup either");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_ssh_directory_is_refused_and_its_target_untouched() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), ssh_dir(home.path())).unwrap();
        let error = add(home.path(), &key(ED25519), "phone", at()).unwrap_err();
        assert!(error.to_string().contains("symbolic link"), "{error}");
        assert_eq!(fs::read_dir(elsewhere.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn an_authorized_keys_with_another_hard_link_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let other_name = elsewhere.path().join("alias");
        fs::create_dir(ssh_dir(home.path())).unwrap();
        fs::write(path(home.path()), format!("{OTHER}\n")).unwrap();
        fs::hard_link(path(home.path()), &other_name).unwrap();
        let error = add(home.path(), &key(ED25519), "phone", at()).unwrap_err();
        assert!(error.to_string().contains("hard link"), "{error}");
        assert_eq!(
            fs::read_to_string(&other_name).unwrap(),
            format!("{OTHER}\n")
        );
    }

    #[cfg(unix)]
    #[test]
    fn files_that_belong_to_someone_else_are_refused() {
        // Pretend the account is another user: everything the test creates is then "foreign".
        let home = tempfile::tempdir().unwrap();
        let account = Account::new("tester", home.path());
        let other = account.uid.wrapping_add(1);
        fs::create_dir(ssh_dir(home.path())).unwrap();
        fs::write(path(home.path()), format!("{OTHER}\n")).unwrap();
        let error =
            unix::add_as(other, &account, &key(ED25519), "phone", at(), &|| {}).unwrap_err();
        assert!(
            error.to_string().contains("belongs to another user"),
            "{error}"
        );
        assert_eq!(
            fs::read_to_string(path(home.path())).unwrap(),
            format!("{OTHER}\n")
        );
        // The check reports the same thing without changing anything.
        let result = super::writable(&Account {
            uid: other,
            ..account
        });
        assert!(matches!(result, Writable::No(why) if why.contains("another user")));
    }

    #[cfg(unix)]
    #[test]
    fn a_path_swapped_in_after_the_checks_is_not_followed() {
        // The race of finding 2: between opening the checked handles and writing, `~/.ssh` is
        // replaced by a link to somewhere else. The backup and the append must still go through
        // the handles that were checked.
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
        let account = Account::new("tester", home.path());
        let added = unix::add_hooked(&account, &key(ED25519), "phone", at(), &swap).unwrap();
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

    #[cfg(unix)]
    #[test]
    fn an_authorized_keys_that_is_not_a_regular_file_is_refused() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        fs::create_dir(path(home.path())).unwrap();
        assert!(add(home.path(), &key(ED25519), "phone", at()).is_err());
    }

    /// A 0600 FIFO where `authorized_keys` should be, in a disposable home.
    #[cfg(unix)]
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
    #[cfg(unix)]
    fn within_seconds<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = send.send(work());
        });
        receive
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the call blocked on a FIFO instead of refusing it")
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_as_authorized_keys_is_refused_by_the_check_without_blocking() {
        // Review of 6afa42e: a blocking O_WRONLY open of a FIFO with no reader hung `--check`.
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

    #[cfg(unix)]
    #[test]
    fn a_fifo_as_authorized_keys_is_refused_by_add_without_blocking_or_writing() {
        let home = home_with_a_fifo();
        let root = home.path().to_owned();
        let error = within_seconds(move || add(&root, &key(ED25519), "phone", at())).unwrap_err();
        assert!(error.to_string().contains("not a regular file"), "{error}");
        assert_eq!(fs::read_dir(ssh_dir(home.path())).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn other_special_files_are_refused_too() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        std::os::unix::net::UnixListener::bind(path(home.path())).unwrap();
        let root = home.path().to_owned();
        let result = within_seconds(move || writable(&root));
        assert!(matches!(result, Writable::No(_)), "{result:?}");
        assert!(add(home.path(), &key(ED25519), "phone", at()).is_err());
    }

    #[test]
    fn creates_the_directory_and_file_with_private_permissions() {
        let home = tempfile::tempdir().unwrap();
        let added = add(home.path(), &key(ED25519), "Pixel-8", at()).unwrap();
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
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&ssh_dir(home.path())), 0o700);
            assert_eq!(mode(&path(home.path())), 0o600);
        }
    }

    #[test]
    fn appends_after_a_backup_and_keeps_what_was_there() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        let before = format!("# mine\n{OTHER} me@laptop\n");
        fs::write(path(home.path()), &before).unwrap();
        let added = add(home.path(), &key(ED25519), "phone", at()).unwrap();
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
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn a_missing_final_newline_is_repaired_before_appending() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        fs::write(path(home.path()), OTHER).unwrap();
        add(home.path(), &key(ED25519), "phone", at()).unwrap();
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
        let added = add(home.path(), &key(ED25519), "phone", at()).unwrap();
        assert_eq!(added, Added::AlreadyPresent);
        assert_eq!(fs::read_to_string(path(home.path())).unwrap(), existing);
        let names: Vec<_> = fs::read_dir(ssh_dir(home.path())).unwrap().collect();
        assert_eq!(names.len(), 1, "no backup for a no-op");
    }

    #[test]
    fn adding_twice_adds_once() {
        let home = tempfile::tempdir().unwrap();
        add(home.path(), &key(ED25519), "phone", at()).unwrap();
        assert_eq!(
            add(home.path(), &key(ED25519), "phone", at()).unwrap(),
            Added::AlreadyPresent
        );
        assert_eq!(
            fs::read_to_string(path(home.path()))
                .unwrap()
                .lines()
                .count(),
            1
        );
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

    // --- Finding 10: duplicates are the parsed key of an entry, not text anywhere on a line ---

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
    fn pairing_a_key_that_only_a_comment_mentions_adds_it() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        let before = format!("{ED25519} migrate to {OTHER} later\n");
        fs::write(path(home.path()), &before).unwrap();
        let added = add(home.path(), &key(OTHER), "phone", at()).unwrap();
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
        let added = add(home.path(), &key(ED25519), "phone", at()).unwrap();
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
        const ECDSA: &str = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBJKzE3AhnQL0fsk77dWQaJJj+GHelMX7ge+TZ2xgJ/Y3O5aMMlnH6Hn/tnucj94OezBBvEj2VIcJGu8ABqUkhFA=";
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        fs::write(path(home.path()), format!("{OTHER}\n")).unwrap();
        let Added::Added {
            backup: Some(first),
            ..
        } = add(home.path(), &key(ED25519), "a", at()).unwrap()
        else {
            panic!()
        };
        let Added::Added {
            backup: Some(second),
            ..
        } = add(home.path(), &key(ECDSA), "b", at()).unwrap()
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

    // --- Finding 6: what sshd's StrictModes would ignore is refused, not appended to ---------

    #[cfg(unix)]
    fn chmod(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(unix)]
    fn existing_file(home: &Path, mode: u32) -> String {
        fs::create_dir_all(ssh_dir(home)).unwrap();
        let before = format!("{OTHER}\n");
        fs::write(path(home), &before).unwrap();
        chmod(&path(home), mode);
        before
    }

    #[cfg(unix)]
    #[test]
    fn an_authorized_keys_writable_by_group_or_others_is_refused_and_left_alone() {
        for mode in [0o664, 0o666, 0o662, 0o620] {
            let home = tempfile::tempdir().unwrap();
            let before = existing_file(home.path(), mode);
            let error = add(home.path(), &key(ED25519), "phone", at()).unwrap_err();
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

    #[cfg(unix)]
    #[test]
    fn modes_sshd_accepts_are_appended_to_and_keep_their_mode() {
        use std::os::unix::fs::PermissionsExt;
        for mode in [0o600, 0o640, 0o644] {
            let home = tempfile::tempdir().unwrap();
            existing_file(home.path(), mode);
            assert!(matches!(
                add(home.path(), &key(ED25519), "phone", at()),
                Ok(Added::Added { .. })
            ));
            let now = fs::metadata(path(home.path()))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(now & 0o777, mode);
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_ssh_directory_or_home_writable_by_others_is_refused() {
        for mode in [0o775, 0o777, 0o770 | 0o002] {
            let home = tempfile::tempdir().unwrap();
            existing_file(home.path(), 0o600);
            chmod(&ssh_dir(home.path()), mode);
            let error = add(home.path(), &key(ED25519), "phone", at()).unwrap_err();
            assert!(
                error.to_string().contains("writable by other users")
                    && error.to_string().contains(".ssh"),
                "{mode:o}: {error}"
            );
            chmod(&ssh_dir(home.path()), 0o700);
            // The home directory itself.
            chmod(home.path(), mode);
            let error = add(home.path(), &key(ED25519), "phone", at()).unwrap_err();
            assert!(
                error.to_string().contains("writable by other users"),
                "{mode:o}: {error}"
            );
            chmod(home.path(), 0o755);
            assert!(add(home.path(), &key(ED25519), "phone", at()).is_ok());
        }
    }

    #[test]
    fn checks_report_writability_and_strict_modes_trouble() {
        let home = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(home.path(), fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(writable(home.path()), Writable::Yes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // A home directory writable by group or others is a StrictModes refusal.
            fs::set_permissions(home.path(), fs::Permissions::from_mode(0o775)).unwrap();
            assert!(
                matches!(writable(home.path()), Writable::No(why) if why.contains("StrictModes"))
            );
            // A read-only existing file is reported, not changed.
            fs::set_permissions(home.path(), fs::Permissions::from_mode(0o755)).unwrap();
            fs::create_dir(ssh_dir(home.path())).unwrap();
            fs::write(path(home.path()), "").unwrap();
            fs::set_permissions(path(home.path()), fs::Permissions::from_mode(0o400)).unwrap();
            let result = writable(home.path());
            // Running as root can write a 0400 file; everyone else cannot.
            if !fs::OpenOptions::new()
                .append(true)
                .open(path(home.path()))
                .is_ok()
            {
                assert!(matches!(result, Writable::No(_)));
            }
        }
    }
}
