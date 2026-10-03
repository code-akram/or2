//! Image upload over SFTP on the host's own SSH connection (contracts.md, "Image paste" and
//! "Upload speed").
//!
//! A connection's uploads share one session channel running the server's `sftp` subsystem
//! ([`Uploads`]): the first upload opens it, later ones reuse it, one upload at a time. Every
//! path is relative to where the server's SFTP starts (the login's home), so
//! `~/.cache/or2/images` needs no knowledge of `$HOME` and no shell. Requests that do not
//! depend on each other go out together (SFTP matches replies by id); a request that checks
//! an earlier step waits for it. The upload:
//!
//! 1. checks each part of [`IMAGE_DIR`] with `lstat` ([`directory_problem`]): a directory, not
//!    a symbolic link (except `~/.cache`, which may link to a directory held to the same
//!    rules), not writable by group or others (`~/.cache`: by others), and the image directory
//!    exactly `0700`. All three are read at once; only when one is missing or wrong does it go
//!    part by part, creating the missing ones (`0700`), making the image directory `0700` if
//!    something else made it, and checking each before going below it;
//! 2. creates `<name>.part` (exclusive, `0600`, then `fsetstat 0600`) and checks with `fstat`
//!    that it is a regular `0600` file. Its owner is the account the upload runs as: the
//!    directories must belong to it (`~/.cache` may also belong to root), read with the
//!    `fstat` and checked after it;
//! 3. writes the bytes, pipelined, and closes the file;
//! 4. checks the directories again (read together with the close) and renames the file to
//!    `or2-<UTC yyyyMMdd-HHmmss>-<6 hex>.<ext>`;
//! 5. answers the absolute path the server resolves for it when that is safe to type into a
//!    terminal ([`is_safe_image_path`]), else one made of the start directory's own resolved
//!    path and the image's relative one when that is, else fails.
//!
//! Once its path is delivered, the host driver removes the directory's `or2-*` files of that
//! owner older than [`SWEEP_AGE`] ([`Sweep`]: best effort, bounded, at most once per session
//! per [`SWEEP_INTERVAL`]), never on the upload's way.
//!
//! Whatever ends an upload early (a failure, a check, the caller's reply dropped) removes what
//! it made, best effort: the temporary file before the rename (its handle closed), the image
//! after it, and an image whose caller did not acknowledge its path ([`deliver`]). One left
//! behind (the connection died) is an `or2-*` file the sweep removes later.
//!
//! SFTP v3 names files by path only (no `openat`, no `O_NOFOLLOW`): the checks hold against
//! other accounts, which cannot change what was checked, but not against a process of the
//! same account racing the upload. The server itself is trusted with the files (it decides
//! where bytes go), never with what reaches a terminal.

use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use russh::ChannelMsg;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::{Config, RawSftpSession};
use russh_sftp::protocol::{Attrs, FileAttributes, OpenFlags, StatusCode};
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use tokio::time::{Instant, timeout, timeout_at};

use super::connection::SshHost;
use crate::host::{HostError, UploadedImage};

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
/// Write permission for others: `~/.cache` may be group-writable (a umask of `002` with a
/// group of the user's own, as Debian and Ubuntu give users, leaves it `0775`).
const OTHER_WRITE: u32 = 0o002;
/// `or2-*` files older than this are removed by a later sweep.
pub(crate) const SWEEP_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// How long the sweep may take before it is left unfinished.
const SWEEP_TIMEOUT: Duration = Duration::from_secs(5);
/// How often one SFTP session sweeps at most.
const SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// How long removing a file after a failure or a cancel may take.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a sent path may wait for its caller's acknowledgement ([`deliver`]). The caller
/// acknowledges as soon as it runs again, or drops the path when it stopped waiting, so this
/// only bounds a caller that is never run again.
const ACK_TIMEOUT: Duration = Duration::from_secs(30);
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

/// A host connection's image uploads: its SFTP session, opened by the first upload and kept
/// for the next ones, which run one at a time. Owned by the host driver, so the session (and
/// its channel) goes with the connection.
#[derive(Default)]
pub(super) struct Uploads {
    session: tokio::sync::Mutex<Option<Arc<Sftp>>>,
}

/// One SFTP session, on a session channel of its own that closes when it is dropped.
pub(super) struct Sftp {
    raw: RawSftpSession,
    /// When this session last started a [`Sweep`].
    swept: Mutex<Option<Instant>>,
}

impl Sftp {
    fn new(raw: RawSftpSession) -> Self {
        Self {
            raw,
            swept: Mutex::new(None),
        }
    }

    /// Whether a sweep may start `now`; if so, it counts as started.
    fn claim_sweep(&self, now: Instant) -> bool {
        let mut swept = self.swept.lock().unwrap_or_else(PoisonError::into_inner);
        let due = sweep_due(*swept, now);
        if due {
            *swept = Some(now);
        }
        due
    }
}

impl Deref for Sftp {
    type Target = RawSftpSession;

    fn deref(&self) -> &RawSftpSession {
        &self.raw
    }
}

/// Whether a session that last swept at `last` may sweep again at `now`.
fn sweep_due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|last| now.saturating_duration_since(last) >= SWEEP_INTERVAL)
}

/// An uploaded image: the absolute path for its caller, and the file on the host, removed if
/// the caller no longer takes it ([`deliver`]).
pub(super) struct Uploaded {
    pub(super) path: String,
    sftp: Arc<Sftp>,
    relative: String,
    /// The account the upload ran as, whose old images the [`Sweep`] removes.
    owner: u32,
}

impl Uploaded {
    /// The sweep to run once this image's path was delivered.
    pub(super) fn sweep(&self) -> Sweep {
        Sweep {
            sftp: Arc::clone(&self.sftp),
            owner: self.owner,
        }
    }
}

/// Removes [`IMAGE_DIR`]'s old `or2-*` files of the account an upload ran as, after that
/// upload, on its session: best effort, at most [`SWEEP_TIMEOUT`], and at most once per
/// session per [`SWEEP_INTERVAL`].
pub(super) struct Sweep {
    sftp: Arc<Sftp>,
    owner: u32,
}

impl Sweep {
    pub(super) async fn run(self) {
        if !self.sftp.claim_sweep(Instant::now()) {
            return;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let listing = Mutex::new(None);
        let _ = timeout(SWEEP_TIMEOUT, sweep(&self.sftp, now, self.owner, &listing)).await;
        // A listing left open by the timeout would stay open as long as the session.
        let left_open = listing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(handle) = left_open {
            let _ = timeout(CLEANUP_TIMEOUT, self.sftp.close(handle)).await;
        }
    }
}

/// Hands an upload's result to its caller, in two steps: the path is sent, then the caller
/// acknowledges it as it receives it ([`UploadedImage`]). An image the caller did not take (its
/// reply was gone, or it stopped waiting with the path already sent) is removed, best effort:
/// nothing would ever name it. A sent path counts as delivered only once acknowledged.
pub(super) async fn deliver(
    reply: oneshot::Sender<Result<UploadedImage, HostError>>,
    result: Result<Uploaded, HostError>,
) {
    match result {
        Ok(uploaded) => {
            let (path, acknowledged) = UploadedImage::new(uploaded.path.clone());
            let taken = reply.send(Ok(path)).is_ok()
                && matches!(timeout(ACK_TIMEOUT, acknowledged).await, Ok(Ok(())));
            if !taken {
                let _ = timeout(CLEANUP_TIMEOUT, uploaded.sftp.remove(uploaded.relative)).await;
            }
        }
        Err(error) => {
            let _ = reply.send(Err(error));
        }
    }
}

/// [`Uploads::upload_image`] on a session of its own, closed when the result is dropped.
#[cfg(test)]
pub(super) async fn upload_image(
    host: &Arc<SshHost>,
    bytes: Vec<u8>,
    extension: &str,
    cancelled: impl Future<Output = ()>,
) -> Result<Uploaded, HostError> {
    Uploads::default()
        .upload_image(host, bytes, extension, cancelled)
        .await
}

impl Uploads {
    /// Uploads `bytes` (validated by the handle: nonempty, at most `MAX_IMAGE_BYTES`) with the
    /// lower-case `extension` (see the module comment), after any upload already running on
    /// this connection. Stops when `cancelled` resolves: the caller's reply was dropped (a
    /// cancelled or timed-out upload). Whatever ends it early removes what it made, best effort.
    ///
    /// The connection's session is opened if there is none. A session that fails at the
    /// transport level (its stream ended, or a request got no answer in time) is dropped; when
    /// it failed before anything was made, the upload starts again on a new one, once.
    pub(super) async fn upload_image(
        &self,
        host: &Arc<SshHost>,
        bytes: Vec<u8>,
        extension: &str,
        cancelled: impl Future<Output = ()>,
    ) -> Result<Uploaded, HostError> {
        tokio::pin!(cancelled);
        let mut session = tokio::select! {
            session = self.session.lock() => session,
            () = &mut cancelled => return Err(cancelled_error()),
        };
        let mut reopened = false;
        loop {
            let failure = Failure::new(host);
            let sftp = match session.as_ref() {
                Some(sftp) => Arc::clone(sftp),
                None => {
                    let sftp = tokio::select! {
                        sftp = open(host, &failure) => Arc::new(Sftp::new(sftp?)),
                        () = &mut cancelled => return Err(cancelled_error()),
                    };
                    *session = Some(Arc::clone(&sftp));
                    sftp
                }
            };
            let tried = attempt(&sftp, &bytes, extension, cancelled.as_mut(), &failure).await;
            let error = match tried.result {
                Ok(uploaded) => return Ok(uploaded),
                Err(error) => error,
            };
            if failure.broken() {
                *session = None;
                if tried.again && !reopened && !failure.closed() {
                    reopened = true;
                    continue;
                }
            }
            return Err(error);
        }
    }
}

/// How one try of an upload ended.
struct Tried {
    result: Result<Uploaded, HostError>,
    /// It was not cancelled, and nothing was made on the host before it failed: it may start
    /// again on a new session when its own failed.
    again: bool,
}

/// One try of an upload on `sftp`. What it made on the host is removed again on a failure,
/// best effort; a cleanup that gets no answer in time leaves the session broken.
async fn attempt<F: Future<Output = ()>>(
    sftp: &Arc<Sftp>,
    bytes: &[u8],
    extension: &str,
    mut cancelled: Pin<&mut F>,
    failure: &Failure,
) -> Tried {
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
    let progress = Progress::default();
    let (result, was_cancelled) = tokio::select! {
        result = upload(sftp, &target, bytes, &progress, failure) => (result, false),
        () = &mut cancelled => (Err(cancelled_error()), true),
    };
    let made = progress.made.load(Ordering::SeqCst);
    match result {
        Ok((resolved, owner)) => Tried {
            result: Ok(Uploaded {
                path: resolved,
                sftp: Arc::clone(sftp),
                relative: path,
                owner,
            }),
            again: false,
        },
        Err(error) => {
            let open = progress
                .handle
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            let cleanup = timeout(CLEANUP_TIMEOUT, async {
                // The session outlives the upload: a handle left open would too.
                let closed = async {
                    if let Some(handle) = open {
                        let _ = sftp.close(handle).await;
                    }
                };
                let removed = async {
                    if matches!(made, MAKING_TEMPORARY | MADE_TEMPORARY | RENAMING) {
                        let _ = sftp.remove(temporary.as_str()).await;
                    }
                    if matches!(made, RENAMING | RENAMED) {
                        let _ = sftp.remove(path.as_str()).await;
                    }
                };
                tokio::join!(closed, removed);
            })
            .await;
            if cleanup.is_err() {
                failure.stale.store(true, Ordering::SeqCst);
            }
            Tried {
                result: Err(error),
                again: !was_cancelled && made == MADE_NOTHING,
            }
        }
    }
}

/// The names of one upload's image, relative to the start directory.
struct Target<'a> {
    name: &'a str,
    path: &'a str,
    temporary: &'a str,
}

/// What an upload has on the host so far: what it made (`MADE_NOTHING` and on), and the
/// temporary file's handle while it is open.
#[derive(Default)]
struct Progress {
    made: AtomicU8,
    handle: Mutex<Option<String>>,
}

impl Progress {
    fn set_handle(&self, handle: Option<String>) {
        *self.handle.lock().unwrap_or_else(PoisonError::into_inner) = handle;
    }
}

/// Steps 1 to 5 of the module comment, recording in `progress` what is on the host. Answers
/// the image's path and its owner.
async fn upload(
    sftp: &Arc<Sftp>,
    target: &Target<'_>,
    bytes: &[u8],
    progress: &Progress,
    failure: &Failure,
) -> Result<(String, u32), HostError> {
    let made = &progress.made;
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
    progress.set_handle(Some(handle.clone()));
    let written = async {
        let owner = private_file(sftp, &handle, failure).await?;
        write(sftp, &handle, bytes, failure).await?;
        Ok(owner)
    }
    .await;
    // Closing needs no answer to the checks before the rename: both go out together.
    let closing = sftp.close(handle);
    let owner = match written {
        Ok(owner) => owner,
        Err(error) => {
            let _ = closing.await;
            progress.set_handle(None);
            return Err(error);
        }
    };
    let (closed, parts) = tokio::join!(closing, read_parts(sftp));
    progress.set_handle(None);
    closed.map_err(|error| failure.of("writing the image", error))?;
    check_parts(parts, Some(owner), failure)?;
    made.store(RENAMING, Ordering::SeqCst);
    if let Err(error) = sftp.rename(target.temporary, target.path).await {
        // Not renamed (a name that exists is not ours to remove).
        made.store(MADE_TEMPORARY, Ordering::SeqCst);
        return Err(failure.of("naming the image", error));
    }
    made.store(RENAMED, Ordering::SeqCst);
    Ok((resolve(sftp, target, failure).await?, owner))
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

/// Makes sure [`IMAGE_DIR`] is there and private. In the common case every part exists and
/// passes its check ([`directory_problem`], its owner not yet known): one round trip reads
/// them all, and nothing is made. Otherwise [`make_directories`] goes part by part. A part the
/// session could not read at all (a broken session) fails here, with nothing more sent on it.
async fn ensure_directory(sftp: &RawSftpSession, failure: &Failure) -> Result<(), HostError> {
    let parts = read_parts(sftp).await;
    if let Some(error) = parts.broken() {
        return Err(failure.of("reading ~/.cache/or2", error));
    }
    match parts.first_problem(None) {
        None => Ok(()),
        Some(_) => make_directories(sftp, failure).await,
    }
}

/// Creates the missing parts of [`IMAGE_DIR`] (`0700`) and makes the image directory `0700`
/// when something else made it. Each part is checked ([`directory_problem`], its owner not yet
/// known) before anything is made below it: a server that will not make the image directory
/// private fails the upload.
async fn make_directories(sftp: &RawSftpSession, failure: &Failure) -> Result<(), HostError> {
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

/// The parts of [`IMAGE_DIR`] as `lstat` saw them, in [`IMAGE_DIR_PARTS`] order, and
/// `~/.cache` as `stat` saw it (what a symbolic link there leads to), all read at once.
struct Parts {
    lstat: [Result<Attrs, SftpError>; 3],
    cache: Result<Attrs, SftpError>,
}

/// One part's problem: it could not be read, or [`directory_problem`] has something to say.
enum PartProblem {
    Unreadable(SftpError),
    Wrong(String),
}

/// Reads every part of [`IMAGE_DIR`] in one round trip: the requests go out together.
async fn read_parts(sftp: &RawSftpSession) -> Parts {
    let [cache, or2, images] = IMAGE_DIR_PARTS;
    let (cache, or2, images, followed) = tokio::join!(
        sftp.lstat(cache),
        sftp.lstat(or2),
        sftp.lstat(images),
        sftp.stat(cache),
    );
    Parts {
        lstat: [cache, or2, images],
        cache: followed,
    }
}

impl Parts {
    /// The first error that is not the server's answer (the session is broken), if any.
    fn broken(&self) -> Option<SftpError> {
        self.lstat
            .iter()
            .chain([&self.cache])
            .find_map(|read| match read {
                Err(error) if !matches!(error, SftpError::Status(_)) => Some(error.clone()),
                _ => None,
            })
    }

    /// The first part's problem, in order, exactly as [`check_part`] of each part in turn
    /// would find it.
    fn first_problem(self, owner: Option<u32>) -> Option<PartProblem> {
        let mut cache = Some(self.cache);
        for (part, read) in IMAGE_DIR_PARTS.into_iter().zip(self.lstat) {
            let mut attributes = match read {
                Ok(found) => found.attrs,
                Err(error) => return Some(PartProblem::Unreadable(error)),
            };
            if part == CACHE_DIR
                && has_type(&attributes, MODE_SYMLINK)
                && let Some(followed) = cache.take()
            {
                attributes = match followed {
                    Ok(found) => found.attrs,
                    Err(error) => return Some(PartProblem::Unreadable(error)),
                };
            }
            if let Some(problem) = directory_problem(part, &attributes, owner) {
                return Some(PartProblem::Wrong(problem));
            }
        }
        None
    }
}

/// Checks every part of [`IMAGE_DIR`] as read, now that the `owner` is known.
fn check_parts(parts: Parts, owner: Option<u32>, failure: &Failure) -> Result<(), HostError> {
    match parts.first_problem(owner) {
        None => Ok(()),
        Some(PartProblem::Unreadable(error)) => Err(failure.of("reading ~/.cache/or2", error)),
        Some(PartProblem::Wrong(problem)) => Err(failed(&problem)),
    }
}

/// What makes `attributes` wrong for `part` of [`IMAGE_DIR`], if anything: not a directory, a
/// symbolic link, writable by group or others (`~/.cache`: by others), the image directory not exactly `0700`, or
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
    let shared = if part == CACHE_DIR {
        OTHER_WRITE
    } else {
        SHARED_WRITE
    };
    if bits & shared != 0 {
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
/// checks that it is: a regular `0600` file. Its owner is the account the upload runs as, and
/// every part of [`IMAGE_DIR`] must belong to it: the parts are read with the `fstat` (they
/// need nothing from it) and checked after the file. Answers the owner.
async fn private_file(
    sftp: &RawSftpSession,
    handle: &str,
    failure: &Failure,
) -> Result<u32, HostError> {
    // The `fstat` checks what this did: it waits for the answer.
    let _ = sftp.fsetstat(handle, mode(FILE_MODE)).await;
    let (file, parts) = tokio::join!(sftp.fstat(handle), read_parts(sftp));
    let attributes = file
        .map_err(|error| failure.of("checking the image", error))?
        .attrs;
    if !is_regular(&attributes)
        || attributes.permissions.map(|bits| bits & PERMISSION_BITS) != Some(FILE_MODE)
    {
        return Err(failed("the image could not be made private"));
    }
    let owner = attributes
        .uid
        .ok_or_else(|| failed("the host does not report file owners"))?;
    check_parts(parts, Some(owner), failure)?;
    Ok(owner)
}

/// Removes [`IMAGE_DIR`]'s regular `or2-*` files of `owner` last modified more than
/// [`SWEEP_AGE`] before `now`. Best effort: whatever fails is left for a later sweep. The
/// listing's handle is in `listing` while it is open.
async fn sweep(sftp: &RawSftpSession, now: Duration, owner: u32, listing: &Mutex<Option<String>>) {
    let Some(cutoff) = now.checked_sub(SWEEP_AGE).map(|cutoff| cutoff.as_secs()) else {
        return;
    };
    let Ok(directory) = sftp.opendir(IMAGE_DIR).await else {
        return;
    };
    *listing.lock().unwrap_or_else(PoisonError::into_inner) = Some(directory.handle.clone());
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
    listing
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    for name in old {
        let _ = sftp.remove(format!("{IMAGE_DIR}/{name}")).await;
    }
}

/// Writes `bytes` to the open file `handle` with up to [`IN_FLIGHT`] writes in flight.
async fn write(
    sftp: &Arc<Sftp>,
    handle: &str,
    bytes: &[u8],
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

/// Maps SFTP and channel failures, telling a closed connection from a failed request, and
/// remembers whether the session itself failed (not just a request on it).
struct Failure {
    host: Arc<SshHost>,
    /// A request failed with no answer from the server: the session is gone.
    transport: AtomicBool,
    /// A request timed out: what the server still does on the session is unknown.
    stale: AtomicBool,
}

impl Failure {
    fn new(host: &Arc<SshHost>) -> Self {
        Self {
            host: Arc::clone(host),
            transport: AtomicBool::new(false),
            stale: AtomicBool::new(false),
        }
    }

    fn closed(&self) -> bool {
        self.host.is_closed()
    }

    /// Whether the session failed at the transport level, and must not be used again.
    fn broken(&self) -> bool {
        self.transport.load(Ordering::SeqCst) || self.stale.load(Ordering::SeqCst)
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
        match &error {
            SftpError::Status(_) | SftpError::Limited(_) => {}
            SftpError::Timeout => self.stale.store(true, Ordering::SeqCst),
            // The session's stream ended, or it was sent something that is not SFTP.
            SftpError::IO(_) | SftpError::UnexpectedPacket | SftpError::UnexpectedBehavior(_) => {
                self.transport.store(true, Ordering::SeqCst);
            }
        }
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
    fn a_session_sweeps_at_most_once_an_hour() {
        let start = Instant::now();
        assert!(sweep_due(None, start), "never swept");
        assert!(!sweep_due(Some(start), start));
        assert!(!sweep_due(
            Some(start),
            start + SWEEP_INTERVAL - Duration::from_secs(1)
        ));
        assert!(sweep_due(Some(start), start + SWEEP_INTERVAL));
        assert_eq!(SWEEP_INTERVAL, Duration::from_secs(3600));
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
            problem(".cache", 0o040_775, Some(5), Some(5)),
            None,
            "a group of the user's own (umask 002)"
        );
        assert_eq!(
            problem(".cache/or2", 0o040_775, Some(5), Some(5)).as_deref(),
            Some("~/.cache/or2 is writable by others"),
            "only ~/.cache"
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
