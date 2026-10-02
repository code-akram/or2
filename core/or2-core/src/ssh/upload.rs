//! Image upload over SFTP on the host's own SSH connection (contracts.md, "Image paste").
//!
//! One session channel per upload runs the server's `sftp` subsystem. Every path is relative
//! to where the server's SFTP starts (the login's home), so `~/.cache/or2/images` needs no
//! knowledge of `$HOME` and no shell. The upload:
//!
//! 1. creates the missing parts of [`IMAGE_DIR`] (`0700`), makes the image directory `0700` if
//!    something else made it, and checks each part with `lstat` before going below it
//!    ([`directory_problem`]): a directory, not a symbolic link (except `~/.cache`, which may
//!    link to a directory held to the same rules), not writable by group or others, and the
//!    image directory exactly `0700`;
//! 2. creates `<name>.part` (exclusive, `0600`, then `fsetstat 0600`) and checks with `fstat`
//!    that it is a regular `0600` file. Its owner is the account the upload runs as: the
//!    directories must belong to it (`~/.cache` may also belong to root), checked next;
//! 3. removes the directory's `or2-*` files of that owner older than [`SWEEP_AGE`] (best effort,
//!    bounded);
//! 4. writes the bytes, pipelined, and closes the file;
//! 5. checks the directories again and renames the file to
//!    `or2-<UTC yyyyMMdd-HHmmss>-<6 hex>.<ext>`;
//! 6. answers the absolute path the server resolves for it when that is safe to type into a
//!    terminal ([`is_safe_image_path`]), else one made of the start directory's own resolved
//!    path and the image's relative one when that is, else fails.
//!
//! Whatever ends an upload early (a failure, a check, the caller's reply dropped) removes what
//! it made, best effort: the temporary file before the rename, the image after it, and an
//! image whose caller stopped waiting just as it was done ([`deliver`]). One left behind (the
//! connection died) is an `or2-*` file the sweep removes later.
//!
//! SFTP v3 names files by path only (no `openat`, no `O_NOFOLLOW`): the checks hold against
//! other accounts, which cannot change what was checked, but not against a process of the
//! same account racing the upload. The server itself is trusted with the files (it decides
//! where bytes go), never with what reaches a terminal.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use russh::ChannelMsg;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::{Config, RawSftpSession};
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use tokio::time::{Instant, timeout, timeout_at};

use super::connection::SshHost;
use crate::host::HostError;

/// The image directory, relative to where the server's SFTP starts (the home directory).
pub(crate) const IMAGE_DIR: &str = ".cache/or2/images";
/// [`IMAGE_DIR`] and the directories above it, from the start directory down.
const IMAGE_DIR_PARTS: [&str; 3] = [CACHE_DIR, ".cache/or2", IMAGE_DIR];
/// The one part that may be a symbolic link (the user's own `~/.cache` elsewhere), and may
/// belong to root (made by a `sudo` program).
const CACHE_DIR: &str = ".cache";
/// What [`IMAGE_DIR`] and the directories above it are created with.
const DIRECTORY_MODE: u32 = 0o700;
/// What an image is created with.
const FILE_MODE: u32 = 0o600;
/// The permission bits of a mode.
const PERMISSION_BITS: u32 = 0o7777;
/// Write permission for group or others.
const SHARED_WRITE: u32 = 0o022;
/// `or2-*` files older than this are removed by the next upload.
pub(crate) const SWEEP_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// How long the sweep may take before the upload goes on without finishing it.
const SWEEP_TIMEOUT: Duration = Duration::from_secs(5);
/// How long removing a file after a failure or a cancel may take.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);
/// One SFTP write: OpenSSH takes up to 255 KiB, every server at least 32 KiB.
const CHUNK: usize = 32 * 1024;
/// Writes in flight at once.
const IN_FLIGHT: usize = 16;
/// The longest image path handed back (Linux's `PATH_MAX`).
const MAX_PATH_BYTES: usize = 4096;

const MODE_TYPE_MASK: u32 = 0o170_000;
const MODE_DIRECTORY: u32 = 0o040_000;
const MODE_REGULAR: u32 = 0o100_000;
const MODE_SYMLINK: u32 = 0o120_000;

/// What an upload has made on the host so far, which an early end removes.
const MADE_NOTHING: u8 = 0;
/// The temporary file's exclusive create was sent and has not answered.
const MAKING_TEMPORARY: u8 = 1;
const MADE_TEMPORARY: u8 = 2;
/// The rename was sent and has not answered: the file has either name.
const RENAMING: u8 = 3;
const RENAMED: u8 = 4;

/// An uploaded image: the absolute path for its caller, and the file on the host, removed if
/// the caller no longer takes it ([`deliver`]).
pub(super) struct Uploaded {
    pub(super) path: String,
    sftp: Arc<RawSftpSession>,
    relative: String,
}

/// Hands an upload's result to its caller. An image the caller no longer takes (it stopped
/// waiting just as the upload was done) is removed, best effort: nothing would ever name it.
pub(super) async fn deliver(
    reply: oneshot::Sender<Result<String, HostError>>,
    result: Result<Uploaded, HostError>,
) {
    match result {
        Ok(uploaded) => {
            if reply.send(Ok(uploaded.path.clone())).is_err() {
                let _ = timeout(CLEANUP_TIMEOUT, uploaded.sftp.remove(uploaded.relative)).await;
            }
        }
        Err(error) => {
            let _ = reply.send(Err(error));
        }
    }
}

/// Uploads `bytes` (validated by the handle: nonempty, at most `MAX_IMAGE_BYTES`) with the
/// lower-case `extension` (see the module comment). Stops when `cancelled` resolves: the
/// caller's reply was dropped (a cancelled or timed-out upload). Whatever ends it early
/// removes what it made, best effort.
pub(super) async fn upload_image(
    host: &Arc<SshHost>,
    bytes: Vec<u8>,
    extension: &str,
    cancelled: impl Future<Output = ()>,
) -> Result<Uploaded, HostError> {
    tokio::pin!(cancelled);
    let failure = Failure(Arc::clone(host));
    let sftp = tokio::select! {
        sftp = open(host, &failure) => sftp?,
        () = &mut cancelled => return Err(cancelled_error()),
    };
    let sftp = Arc::new(sftp);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let name = image_name(now.as_secs(), rand::random::<[u8; 3]>(), extension);
    let path = format!("{IMAGE_DIR}/{name}");
    let temporary = format!("{path}.part");
    let target = Target {
        name: &name,
        path: &path,
        temporary: &temporary,
    };
    let made = AtomicU8::new(MADE_NOTHING);
    let result = tokio::select! {
        result = upload(&sftp, &target, now, bytes, &made, &failure) => result,
        () = &mut cancelled => Err(cancelled_error()),
    };
    match result {
        Ok(resolved) => Ok(Uploaded {
            path: resolved,
            sftp,
            relative: path,
        }),
        Err(error) => {
            let made = made.load(Ordering::SeqCst);
            let _ = timeout(CLEANUP_TIMEOUT, async {
                if matches!(made, MAKING_TEMPORARY | MADE_TEMPORARY | RENAMING) {
                    let _ = sftp.remove(temporary.as_str()).await;
                }
                if matches!(made, RENAMING | RENAMED) {
                    let _ = sftp.remove(path.as_str()).await;
                }
            })
            .await;
            Err(error)
        }
    }
}

/// The names of one upload's image, relative to the start directory.
struct Target<'a> {
    name: &'a str,
    path: &'a str,
    temporary: &'a str,
}

/// Steps 1 to 6 of the module comment, recording in `made` what is on the host.
async fn upload(
    sftp: &Arc<RawSftpSession>,
    target: &Target<'_>,
    now: Duration,
    bytes: Vec<u8>,
    made: &AtomicU8,
    failure: &Failure,
) -> Result<String, HostError> {
    ensure_directory(sftp, failure).await?;
    made.store(MAKING_TEMPORARY, Ordering::SeqCst);
    let handle = match sftp
        .open(
            target.temporary,
            OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
            mode(FILE_MODE),
        )
        .await
    {
        Ok(handle) => handle.handle,
        Err(error) => {
            // Not made (a name that exists is not ours to remove).
            made.store(MADE_NOTHING, Ordering::SeqCst);
            return Err(failure.of("creating the image", error));
        }
    };
    made.store(MADE_TEMPORARY, Ordering::SeqCst);
    let written = async {
        let owner = private_file(sftp, &handle, failure).await?;
        check_directories(sftp, Some(owner), failure).await?;
        let _ = timeout(SWEEP_TIMEOUT, sweep(sftp, now, owner)).await;
        write(sftp, &handle, bytes, failure).await?;
        Ok(owner)
    }
    .await;
    let closed = sftp.close(handle).await;
    let owner = written?;
    closed.map_err(|error| failure.of("writing the image", error))?;
    check_directories(sftp, Some(owner), failure).await?;
    made.store(RENAMING, Ordering::SeqCst);
    if let Err(error) = sftp.rename(target.temporary, target.path).await {
        // Not renamed (a name that exists is not ours to remove).
        made.store(MADE_TEMPORARY, Ordering::SeqCst);
        return Err(failure.of("naming the image", error));
    }
    made.store(RENAMED, Ordering::SeqCst);
    resolve(sftp, target, failure).await
}

/// The image's absolute path, for a terminal: the server's `realpath` of it when that is safe
/// to type ([`is_safe_image_path`]); else the start directory's own `realpath` joined with the
/// image's relative path, when that is; else a failure. What the server answers is never
/// passed on unchecked.
async fn resolve(
    sftp: &RawSftpSession,
    target: &Target<'_>,
    failure: &Failure,
) -> Result<String, HostError> {
    let resolved = sftp
        .realpath(target.path)
        .await
        .map_err(|error| failure.of("resolving the image path", error))?;
    if let Some(path) = resolved
        .files
        .into_iter()
        .next()
        .map(|file| file.filename)
        .filter(|path| is_safe_image_path(path, target.name))
    {
        return Ok(path);
    }
    let start = sftp
        .realpath(".")
        .await
        .map_err(|error| failure.of("resolving the image path", error))?;
    start
        .files
        .into_iter()
        .next()
        .map(|file| format!("{}/{}", file.filename.trim_end_matches('/'), target.path))
        .filter(|path| is_safe_image_path(path, target.name))
        .ok_or_else(|| failed("the host did not resolve the image path"))
}

/// Whether `path` may be typed into a terminal as image `name`'s path: absolute, ending in
/// `/<name>`, at most 4096 bytes, and free of control characters (C0, DEL and C1: no NUL, ETX,
/// ESC, CR or LF, so no bracketed paste marker either) and of U+FFFD (a name that was not
/// UTF-8, so not the file's).
pub(crate) fn is_safe_image_path(path: &str, name: &str) -> bool {
    path.len() <= MAX_PATH_BYTES
        && path.starts_with('/')
        && path
            .strip_suffix(name)
            .is_some_and(|directory| directory.ends_with('/'))
        && !path
            .chars()
            .any(|c| c.is_control() || c == char::REPLACEMENT_CHARACTER)
}

/// The file name of an image uploaded at `seconds` since the epoch (UTC):
/// `or2-<yyyyMMdd-HHmmss>-<6 hex>.<extension>`.
pub(crate) fn image_name(seconds: u64, random: [u8; 3], extension: &str) -> String {
    let (year, month, day) = civil_date(seconds / 86_400);
    let time = seconds % 86_400;
    format!(
        "or2-{year:04}{month:02}{day:02}-{:02}{:02}{:02}-{:02x}{:02x}{:02x}.{extension}",
        time / 3600,
        time / 60 % 60,
        time % 60,
        random[0],
        random[1],
        random[2],
    )
}

/// The proleptic Gregorian date of a day count since 1970-01-01 (Howard Hinnant's
/// `civil_from_days`).
fn civil_date(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// Opens a session channel, starts the `sftp` subsystem on it and initialises SFTP, all
/// within the host's exec timeout. A refused subsystem, or one that does not speak SFTP, is
/// `SftpUnavailable`.
async fn open(host: &Arc<SshHost>, failure: &Failure) -> Result<RawSftpSession, HostError> {
    let deadline = Instant::now() + host.exec_timeout();
    let mut pending = host.start_open();
    let mut channel = match timeout_at(deadline, pending.wait()).await {
        Ok(Ok(channel)) => channel,
        Ok(Err(error)) => return Err(failure.channel(error)),
        Err(_) => return Err(timed_out()),
    };
    // Dropped on any failure below, the guard closes the channel through the connection.
    let started = timeout_at(deadline, async {
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|error| failure.channel(error))?;
        loop {
            match channel.wait().await {
                Some(ChannelMsg::Success) => return Ok(()),
                Some(ChannelMsg::Failure) => return Err(HostError::SftpUnavailable),
                Some(ChannelMsg::Close) | None => {
                    return Err(if failure.closed() {
                        HostError::Closed
                    } else {
                        HostError::SftpUnavailable
                    });
                }
                // Window adjusts and the like.
                Some(_) => {}
            }
        }
    })
    .await;
    match started {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(error),
        Err(_) => return Err(timed_out()),
    }
    // The stream closes the channel when the SFTP session ends (it is dropped).
    let stream = channel.into_inner().into_stream();
    let config = Config {
        request_timeout_secs: host.exec_timeout().as_secs().max(1),
        ..Config::default()
    };
    let sftp = RawSftpSession::new_with_config(stream, config);
    match timeout_at(deadline, sftp.init()).await {
        Ok(Ok(_)) => Ok(sftp),
        // A subsystem that started but does not speak SFTP (a missing `sftp-server`).
        Ok(Err(_)) if !failure.closed() => Err(HostError::SftpUnavailable),
        Ok(Err(_)) => Err(HostError::Closed),
        Err(_) => Err(timed_out()),
    }
}

/// Creates the missing parts of [`IMAGE_DIR`] (`0700`) and makes the image directory `0700`
/// when something else made it. Each part is checked ([`directory_problem`], its owner not yet
/// known) before anything is made below it: a server that will not make the image directory
/// private fails the upload.
async fn ensure_directory(sftp: &RawSftpSession, failure: &Failure) -> Result<(), HostError> {
    for part in IMAGE_DIR_PARTS {
        match sftp.lstat(part).await {
            Ok(_) => {}
            Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => {
                if let Err(error) = sftp.mkdir(part, mode(DIRECTORY_MODE)).await {
                    // Another upload may have made it meanwhile.
                    if sftp.lstat(part).await.is_err() {
                        return Err(failure.of("creating ~/.cache/or2/images", error));
                    }
                }
            }
            Err(error) => return Err(failure.of("reading ~/.cache/or2", error)),
        }
        if part == IMAGE_DIR
            && let Ok(found) = sftp.lstat(part).await
            && is_directory(&found.attrs)
            && found.attrs.permissions.map(|bits| bits & PERMISSION_BITS) != Some(DIRECTORY_MODE)
        {
            // Checked to have worked next.
            let _ = sftp.setstat(part, mode(DIRECTORY_MODE)).await;
        }
        check_part(sftp, part, None, failure).await?;
    }
    Ok(())
}

/// Checks every part of [`IMAGE_DIR`] again, now that the `owner` is known.
async fn check_directories(
    sftp: &RawSftpSession,
    owner: Option<u32>,
    failure: &Failure,
) -> Result<(), HostError> {
    for part in IMAGE_DIR_PARTS {
        check_part(sftp, part, owner, failure).await?;
    }
    Ok(())
}

/// Checks `part` of [`IMAGE_DIR`] ([`directory_problem`]) as `lstat` sees it; `~/.cache` as a
/// symbolic link is checked as the directory it leads to.
async fn check_part(
    sftp: &RawSftpSession,
    part: &str,
    owner: Option<u32>,
    failure: &Failure,
) -> Result<(), HostError> {
    let mut attributes = sftp
        .lstat(part)
        .await
        .map_err(|error| failure.of("reading ~/.cache/or2", error))?
        .attrs;
    if part == CACHE_DIR && has_type(&attributes, MODE_SYMLINK) {
        attributes = sftp
            .stat(part)
            .await
            .map_err(|error| failure.of("reading ~/.cache/or2", error))?
            .attrs;
    }
    match directory_problem(part, &attributes, owner) {
        Some(problem) => Err(failed(&problem)),
        None => Ok(()),
    }
}

/// What makes `attributes` wrong for `part` of [`IMAGE_DIR`], if anything: not a directory, a
/// symbolic link, writable by group or others, the image directory not exactly `0700`, or
/// (with the `owner` known) another account's (`~/.cache` may also be root's). Attributes
/// without a mode, or without an owner when one is checked, fail too.
pub(crate) fn directory_problem(
    part: &str,
    attributes: &FileAttributes,
    owner: Option<u32>,
) -> Option<String> {
    let Some(bits) = attributes.permissions else {
        return Some("the host does not report file modes".into());
    };
    if bits & MODE_TYPE_MASK == MODE_SYMLINK {
        return Some(format!("~/{part} is a symbolic link"));
    }
    if bits & MODE_TYPE_MASK != MODE_DIRECTORY {
        return Some(format!("~/{part} is not a directory"));
    }
    if bits & SHARED_WRITE != 0 {
        return Some(format!("~/{part} is writable by others"));
    }
    if part == IMAGE_DIR && bits & PERMISSION_BITS != DIRECTORY_MODE {
        return Some(format!("~/{part} could not be made private"));
    }
    match (owner, attributes.uid) {
        (None, _) => None,
        (Some(_), None) => Some("the host does not report file owners".into()),
        (Some(owner), Some(uid)) if uid == owner || (part == CACHE_DIR && uid == 0) => None,
        (Some(_), Some(_)) => Some(format!("~/{part} belongs to another user")),
    }
}

/// Makes the temporary file `0600` (a server may have ignored the mode it was created with) and
/// checks that it is: a regular `0600` file. Answers its owner, the account the upload runs as.
async fn private_file(
    sftp: &RawSftpSession,
    handle: &str,
    failure: &Failure,
) -> Result<u32, HostError> {
    let _ = sftp.fsetstat(handle, mode(FILE_MODE)).await;
    let attributes = sftp
        .fstat(handle)
        .await
        .map_err(|error| failure.of("checking the image", error))?
        .attrs;
    if !is_regular(&attributes)
        || attributes.permissions.map(|bits| bits & PERMISSION_BITS) != Some(FILE_MODE)
    {
        return Err(failed("the image could not be made private"));
    }
    attributes
        .uid
        .ok_or_else(|| failed("the host does not report file owners"))
}

/// Removes [`IMAGE_DIR`]'s regular `or2-*` files of `owner` last modified more than
/// [`SWEEP_AGE`] before `now`. Best effort: whatever fails is left for the next upload.
async fn sweep(sftp: &RawSftpSession, now: Duration, owner: u32) {
    let Some(cutoff) = now.checked_sub(SWEEP_AGE).map(|cutoff| cutoff.as_secs()) else {
        return;
    };
    let Ok(directory) = sftp.opendir(IMAGE_DIR).await else {
        return;
    };
    let mut old = Vec::new();
    // Ends with the end of the listing (an `Eof` status) or any error.
    while let Ok(listing) = sftp.readdir(directory.handle.as_str()).await {
        if listing.files.is_empty() {
            break;
        }
        old.extend(
            listing
                .files
                .into_iter()
                .filter(|file| {
                    file.filename.starts_with("or2-")
                        && !file.filename.contains('/')
                        && is_regular(&file.attrs)
                        && file.attrs.uid == Some(owner)
                        && file
                            .attrs
                            .mtime
                            .is_some_and(|mtime| u64::from(mtime) < cutoff)
                })
                .map(|file| file.filename),
        );
    }
    let _ = sftp.close(directory.handle).await;
    for name in old {
        let _ = sftp.remove(format!("{IMAGE_DIR}/{name}")).await;
    }
}

/// Writes `bytes` to the open file `handle` with up to [`IN_FLIGHT`] writes in flight.
async fn write(
    sftp: &Arc<RawSftpSession>,
    handle: &str,
    bytes: Vec<u8>,
    failure: &Failure,
) -> Result<(), HostError> {
    let mut writes = JoinSet::new();
    for (index, chunk) in bytes.chunks(CHUNK).enumerate() {
        if writes.len() >= IN_FLIGHT {
            finished(writes.join_next().await, failure)?;
        }
        let (sftp, handle, data) = (Arc::clone(sftp), handle.to_owned(), chunk.to_vec());
        let offset = (index * CHUNK) as u64;
        writes.spawn(async move { sftp.write(handle, offset, data).await });
    }
    while let Some(result) = writes.join_next().await {
        finished(Some(result), failure)?;
    }
    Ok(())
}

fn finished(
    result: Option<Result<Result<russh_sftp::protocol::Status, SftpError>, tokio::task::JoinError>>,
    failure: &Failure,
) -> Result<(), HostError> {
    match result {
        None | Some(Ok(Ok(_))) => Ok(()),
        Some(Ok(Err(error))) => Err(failure.of("writing the image", error)),
        Some(Err(_)) => Err(HostError::CommandFailed {
            message: "writing the image failed".into(),
        }),
    }
}

fn mode(permissions: u32) -> FileAttributes {
    FileAttributes {
        permissions: Some(permissions),
        ..FileAttributes::empty()
    }
}

fn has_type(attributes: &FileAttributes, kind: u32) -> bool {
    attributes
        .permissions
        .is_some_and(|bits| bits & MODE_TYPE_MASK == kind)
}

fn is_directory(attributes: &FileAttributes) -> bool {
    has_type(attributes, MODE_DIRECTORY)
}

fn is_regular(attributes: &FileAttributes) -> bool {
    has_type(attributes, MODE_REGULAR)
}

fn failed(message: &str) -> HostError {
    HostError::CommandFailed {
        message: message.to_owned(),
    }
}

fn timed_out() -> HostError {
    HostError::CommandFailed {
        message: "the host did not answer in time".into(),
    }
}

fn cancelled_error() -> HostError {
    HostError::CommandFailed {
        message: "the upload was cancelled".into(),
    }
}

/// Maps SFTP and channel failures, telling a closed connection from a failed request.
struct Failure(Arc<SshHost>);

impl Failure {
    fn closed(&self) -> bool {
        self.0.is_closed()
    }

    fn channel(&self, error: russh::Error) -> HostError {
        match error {
            russh::Error::ChannelOpenFailure(_) => HostError::CommandFailed {
                message: "the host refused a new channel".into(),
            },
            _ if self.closed() => HostError::Closed,
            error => HostError::CommandFailed {
                message: error.to_string(),
            },
        }
    }

    /// `what` failed with `error`: the server's status in words (never a path with the user's
    /// name in it), a timeout, or `Closed` when the connection is gone.
    fn of(&self, what: &str, error: SftpError) -> HostError {
        if self.closed() {
            return HostError::Closed;
        }
        let reason = match error {
            SftpError::Status(status) => match status.status_code {
                StatusCode::PermissionDenied => "permission denied".to_owned(),
                StatusCode::NoSuchFile => "no such file".to_owned(),
                code => code.to_string().to_lowercase(),
            },
            SftpError::Timeout => return timed_out(),
            SftpError::Limited(limit) => limit,
            error => error.to_string(),
        };
        HostError::CommandFailed {
            message: format!("{what}: {reason}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_carry_the_utc_time_and_six_hex_digits() {
        // 2026-10-02 15:30:12 UTC.
        assert_eq!(
            image_name(1_790_955_012, [0xa1, 0x02, 0xff], "png"),
            "or2-20261002-153012-a102ff.png"
        );
        assert_eq!(
            image_name(0, [0, 0, 0], "jpg"),
            "or2-19700101-000000-000000.jpg"
        );
        // A leap day, and the last second of a year.
        assert_eq!(
            image_name(951_782_400, [1, 2, 3], "gif"),
            "or2-20000229-000000-010203.gif"
        );
        assert_eq!(
            image_name(1_767_225_599, [1, 2, 3], "webp"),
            "or2-20251231-235959-010203.webp"
        );
    }

    #[test]
    fn modes_tell_directories_from_files_and_sockets() {
        let with = |bits| FileAttributes {
            permissions: Some(bits),
            ..FileAttributes::empty()
        };
        assert!(is_directory(&with(0o040_700)));
        assert!(
            !is_directory(&with(0o140_755)),
            "a socket is not a directory"
        );
        assert!(is_regular(&with(0o100_600)));
        assert!(!is_regular(&with(0o120_777)), "a symlink is not a file");
        assert!(!is_directory(&FileAttributes::empty()));
    }

    #[test]
    fn only_an_absolute_control_free_path_of_the_image_is_safe() {
        let name = "or2-20261002-153012-a102ff.png";
        let safe = |path: &str| is_safe_image_path(path, name);
        assert!(safe(&format!("/home/me/.cache/or2/images/{name}")));
        assert!(safe(&format!("/{name}")));
        assert!(safe(&format!("/home/zoë's files/{name}")), "quoted later");
        // Not the image: relative, another name, a name that only ends like it.
        assert!(!safe(&format!("home/{name}")));
        assert!(!safe("/etc/passwd"));
        assert!(!safe(&format!("/home/x{name}")));
        assert!(!safe(&format!("/home/{name}/")));
        // Control characters, whatever they would do in a terminal.
        for control in [
            "\0", "\x03", "\x07", "\x1b", "\r", "\n", "\t", "\x7f", "\u{85}", "\u{9b}",
        ] {
            assert!(!safe(&format!("/home/x{control}y/{name}")), "{control:?}");
        }
        assert!(!safe(&format!("/home/\x1b[201~/{name}")));
        assert!(!safe(&format!("/home/\u{fffd}/{name}")), "not UTF-8");
        assert!(!safe(&format!("/{}/{name}", "a".repeat(MAX_PATH_BYTES))));
    }

    #[test]
    fn image_directory_parts_must_be_private_directories_of_the_user() {
        let attributes = |bits: u32, uid: Option<u32>| FileAttributes {
            permissions: Some(bits),
            uid,
            ..FileAttributes::empty()
        };
        let problem =
            |part, bits, uid, owner| directory_problem(part, &attributes(bits, uid), owner);
        assert_eq!(problem(IMAGE_DIR, 0o040_700, Some(5), Some(5)), None);
        assert_eq!(problem(".cache", 0o040_755, Some(5), Some(5)), None);
        assert_eq!(
            problem(".cache", 0o040_755, Some(0), Some(5)),
            None,
            "root's"
        );
        assert_eq!(
            problem(".cache/or2", 0o040_755, None, None),
            None,
            "owner not known yet"
        );
        assert_eq!(
            problem(".cache/or2", 0o040_755, Some(0), Some(5)).as_deref(),
            Some("~/.cache/or2 belongs to another user")
        );
        assert_eq!(
            problem(IMAGE_DIR, 0o040_700, Some(6), Some(5)).as_deref(),
            Some("~/.cache/or2/images belongs to another user")
        );
        assert_eq!(
            problem(IMAGE_DIR, 0o040_700, None, Some(5)).as_deref(),
            Some("the host does not report file owners")
        );
        assert_eq!(
            problem(IMAGE_DIR, 0o040_750, Some(5), Some(5)).as_deref(),
            Some("~/.cache/or2/images could not be made private")
        );
        assert_eq!(
            problem(".cache", 0o040_757, Some(5), Some(5)).as_deref(),
            Some("~/.cache is writable by others")
        );
        assert_eq!(
            problem(".cache/or2", 0o040_770, Some(5), Some(5)).as_deref(),
            Some("~/.cache/or2 is writable by others")
        );
        assert_eq!(
            problem(IMAGE_DIR, 0o120_777, Some(5), Some(5)).as_deref(),
            Some("~/.cache/or2/images is a symbolic link")
        );
        assert_eq!(
            problem(".cache/or2", 0o100_700, Some(5), Some(5)).as_deref(),
            Some("~/.cache/or2 is not a directory")
        );
        assert_eq!(
            directory_problem(IMAGE_DIR, &FileAttributes::empty(), None).as_deref(),
            Some("the host does not report file modes")
        );
    }
}
