//! Careful file access below the account's home (Unix only): every file or directory `or2-pair`
//! writes is opened relative to a directory handle that was checked, never by path again.
//!
//! - the home directory is opened once; `~/.ssh` and everything below are opened with
//!   `O_NOFOLLOW` (a symbolic link is refused, not followed), the owner must be the account, a
//!   file must be a regular file with no other hard link, and a path swapped in between the
//!   check and the write changes nothing because the handles stay,
//! - `O_NONBLOCK` before `fstat`: a FIFO opened for writing would block until a reader turned up,
//!   so anything that is not a regular file is refused before it is used,
//! - sshd's StrictModes: a home, `~/.ssh` or file writable by group or others is refused, with
//!   the command that fixes it, rather than written to (sshd would ignore it).
//!
//! There is no implementation for any other target: `or2-pair` writes no key file where it cannot
//! check owners, links and permissions with handles (see `run::Env::install_keys`).

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::account::Account;

pub fn refuse(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message.into())
}

fn c_name(name: &str) -> CString {
    CString::new(name).expect("a file name without NUL")
}

pub fn retry<T: PartialEq + Copy>(bad: T, mut call: impl FnMut() -> T) -> io::Result<T> {
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

/// Opens a directory by its path. The path itself may pass through links (the account database
/// says where the home is); everything below it never does.
pub fn open_dir_path(dir: &Path) -> io::Result<OwnedFd> {
    let path = CString::new(dir.as_os_str().as_bytes())
        .map_err(|_| refuse("a directory path contains a NUL byte"))?;
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;
    // SAFETY: `path` is a valid NUL-terminated string.
    let fd = retry(-1, || unsafe { libc::open(path.as_ptr(), flags) })?;
    // SAFETY: `fd` is a freshly opened descriptor that nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

pub fn openat(
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

pub fn fstat(fd: RawFd) -> io::Result<libc::stat> {
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
    let done = unsafe { libc::fstatat(dir, name.as_ptr(), &mut stat, libc::AT_SYMLINK_NOFOLLOW) };
    done == 0 && (stat.st_mode & libc::S_IFMT) == libc::S_IFLNK
}

/// Whether `name` exists in `dir` (as anything, a link included), without following it.
pub fn exists_at(dir: RawFd, name: &str) -> bool {
    let name = c_name(name);
    // SAFETY: as above.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    unsafe { libc::fstatat(dir, name.as_ptr(), &mut stat, libc::AT_SYMLINK_NOFOLLOW) == 0 }
}

pub fn mkdirat(dir: RawFd, name: &str, mode: libc::mode_t) -> io::Result<()> {
    let name = c_name(name);
    // SAFETY: as above.
    retry(-1, || unsafe { libc::mkdirat(dir, name.as_ptr(), mode) }).map(|_| ())
}

/// Removes the file `name` of `dir` (not following anything: a link is removed itself).
pub fn unlinkat(dir: RawFd, name: &str) -> io::Result<()> {
    let name = c_name(name);
    // SAFETY: as above.
    retry(-1, || unsafe { libc::unlinkat(dir, name.as_ptr(), 0) }).map(|_| ())
}

pub fn fchmod(fd: RawFd, mode: libc::mode_t) -> io::Result<()> {
    // SAFETY: `fd` is open.
    retry(-1, || unsafe { libc::fchmod(fd, mode) }).map(|_| ())
}

/// An exclusive, non-blocking `flock`: another program holding it is an error to retry later.
pub fn lock(fd: RawFd) -> io::Result<()> {
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

pub fn writable_by_us(dir: RawFd) -> bool {
    // SAFETY: `dir` is open and "." is a valid string.
    unsafe { libc::faccessat(dir, c".".as_ptr(), libc::W_OK, 0) == 0 }
}

/// Opens `name` below `dir` without following links; a refusal says what was in the way.
pub fn open_nofollow(
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
                if flags & libc::O_DIRECTORY != 0 {
                    "directory"
                } else {
                    "file"
                }
            ))
        } else if error.raw_os_error() == Some(libc::ENOTDIR) && flags & libc::O_DIRECTORY != 0 {
            refuse(format!("{} is not a directory", full.display()))
        } else {
            error
        }
    })
}

/// Opens an existing or new *file* below `dir` for [`check_file`] to judge: never following a
/// link and never blocking on what it finds. A FIFO opened for writing would block until a
/// reader turned up (and a device or socket can fail oddly), so the open is non-blocking, the
/// descriptor is `fstat`ed, and anything that is not a regular file is refused *before* the
/// non-blocking flag is cleared and the handle used. Only the verified descriptor is kept.
pub fn open_file(
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

pub fn kind(stat: &libc::stat) -> libc::mode_t {
    stat.st_mode & libc::S_IFMT
}

/// What sshd's StrictModes refuses: the home directory, `~/.ssh` and `authorized_keys`
/// writable by group or others. Writing to such a file would "work" and the phone would still
/// be turned away, so it is refused here, with the command that fixes it. Nothing is changed
/// behind the person's back.
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
pub fn check_home(stat: &libc::stat, uid: u32, home: &Path) -> io::Result<()> {
    if stat.st_uid != uid && stat.st_uid != 0 {
        return Err(refuse(format!(
            "{} belongs to another user (user id {}); or2-pair only changes the home directory of the account it runs as",
            home.display(),
            stat.st_uid
        )));
    }
    check_strict_modes(stat, home, "the home directory")
}

pub fn check_ssh_dir(stat: &libc::stat, uid: u32, dir: &Path) -> io::Result<()> {
    if stat.st_uid != uid {
        return Err(refuse(format!(
            "{} belongs to another user (user id {}), not to the account or2-pair runs as; sshd would not use it",
            dir.display(),
            stat.st_uid
        )));
    }
    check_strict_modes(stat, dir, "the SSH directory")
}

pub fn check_file(stat: &libc::stat, uid: u32, file: &Path) -> io::Result<()> {
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
            "{} has another hard link ({} names for one file); changing it would change the other name too. Replace it with a plain copy and run again",
            file.display(),
            stat.st_nlink
        )));
    }
    Ok(())
}

/// Opens the account's `~/.ssh`, creating it (mode 0700) when `create` and it is missing.
/// `None`: it does not exist and was not to be created. With an explicit key file (the
/// `test-support` build only) its directory is opened by path and checked the same way.
pub fn open_ssh_dir(account: &Account, create: bool) -> io::Result<Option<OwnedFd>> {
    let uid = account.uid;
    let dir_path = account.ssh_dir();
    if account.keys_file.is_some() {
        let dir = open_dir_path(&dir_path)?;
        check_ssh_dir(&fstat(dir.as_raw_fd())?, uid, &dir_path)?;
        return Ok(Some(dir));
    }
    // An account whose home is unknown (`Account::login_only`) has nothing to write to.
    if account.home.as_os_str().is_empty() {
        return Err(refuse("the account has no known home directory"));
    }
    let home = open_dir_path(&account.home).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "{} is not writable: it cannot be opened ({error})",
                account.home.display()
            ),
        )
    })?;
    check_home(&fstat(home.as_raw_fd())?, uid, &account.home)?;
    let flags = libc::O_RDONLY | libc::O_DIRECTORY;
    let opened = match open_nofollow(home.as_raw_fd(), ".ssh", &dir_path, flags, 0) {
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
            let fd = open_nofollow(home.as_raw_fd(), ".ssh", &dir_path, flags, 0)?;
            // The mode asked of `mkdirat` is cut by the umask; the directory is ours, set it.
            if fstat(fd.as_raw_fd())?.st_uid == uid {
                fchmod(fd.as_raw_fd(), 0o700)?;
            }
            fd
        }
        Err(error) => return Err(error),
    };
    check_ssh_dir(&fstat(opened.as_raw_fd())?, uid, &dir_path)?;
    Ok(Some(opened))
}
