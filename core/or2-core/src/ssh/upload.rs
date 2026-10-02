//! Image upload over SFTP on the host's own SSH connection (contracts.md, "Image paste").
//!
//! One session channel per upload runs the server's `sftp` subsystem. Every path is relative
//! to where the server's SFTP starts (the login's home), so `~/.cache/or2/images` needs no
//! knowledge of `$HOME` and no shell. The upload:
//!
//! 1. creates the missing directories of [`IMAGE_DIR`] (`0700`) and makes the image directory
//!    private if something else made it;
//! 2. removes that directory's `or2-*` files older than [`SWEEP_AGE`] (best effort, bounded);
//! 3. writes the bytes to `<name>.part` (`0600`, exclusive create), pipelined;
//! 4. renames it to `or2-<UTC yyyyMMdd-HHmmss>-<6 hex>.<ext>` and returns the absolute path
//!    the server resolves for it.
//!
//! A cancelled upload (the caller's reply dropped) or a failed write removes the temporary
//! file, best effort; one that is left behind (the connection died) is an `or2-*` file the
//! sweep removes later.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use russh::ChannelMsg;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::{Config, RawSftpSession};
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use tokio::task::JoinSet;
use tokio::time::{Instant, timeout, timeout_at};

use super::connection::SshHost;
use crate::host::HostError;

/// The image directory, relative to where the server's SFTP starts (the home directory).
pub(crate) const IMAGE_DIR: &str = ".cache/or2/images";
/// What [`IMAGE_DIR`] and the directories above it are created with.
const DIRECTORY_MODE: u32 = 0o700;
/// What an image is created with.
const FILE_MODE: u32 = 0o600;
/// `or2-*` files older than this are removed by the next upload.
pub(crate) const SWEEP_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// How long the sweep may take before the upload goes on without finishing it.
const SWEEP_TIMEOUT: Duration = Duration::from_secs(5);
/// How long removing a temporary file after a failure or a cancel may take.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);
/// One SFTP write: OpenSSH takes up to 255 KiB, every server at least 32 KiB.
const CHUNK: usize = 32 * 1024;
/// Writes in flight at once.
const IN_FLIGHT: usize = 16;

const MODE_TYPE_MASK: u32 = 0o170_000;
const MODE_DIRECTORY: u32 = 0o040_000;
const MODE_REGULAR: u32 = 0o100_000;

/// Uploads `bytes` (validated by the handle: nonempty, at most `MAX_IMAGE_BYTES`) with the
/// lower-case `extension` and returns the image's absolute path. Stops when `cancelled`
/// resolves: the caller's reply was dropped (a cancelled or timed-out upload), and the
/// temporary file is removed, best effort.
pub(super) async fn upload_image(
    host: &Arc<SshHost>,
    bytes: Vec<u8>,
    extension: &str,
    cancelled: impl Future<Output = ()>,
) -> Result<String, HostError> {
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
    tokio::select! {
        prepared = async {
            ensure_directory(&sftp, &failure).await?;
            let _ = timeout(SWEEP_TIMEOUT, sweep(&sftp, now)).await;
            Ok(())
        } => prepared?,
        () = &mut cancelled => return Err(cancelled_error()),
    }
    let name = image_name(now.as_secs(), rand::random::<[u8; 3]>(), extension);
    let path = format!("{IMAGE_DIR}/{name}");
    let temporary = format!("{path}.part");
    let written = tokio::select! {
        written = write_and_rename(&sftp, &temporary, &path, bytes, &failure) => written,
        () = &mut cancelled => Err(cancelled_error()),
    };
    if let Err(error) = written {
        // The temporary file is ours to remove; after a rename that went through it is gone.
        let _ = timeout(CLEANUP_TIMEOUT, sftp.remove(temporary)).await;
        return Err(error);
    }
    // The server's own absolute path: through symlinks (a `~/.cache` elsewhere), what an agent
    // on the host opens.
    let resolved = tokio::select! {
        resolved = sftp.realpath(path) => resolved.map_err(|error| failure.of("resolving the image path", error))?,
        () = &mut cancelled => return Err(cancelled_error()),
    };
    resolved
        .files
        .into_iter()
        .next()
        .map(|file| file.filename)
        .filter(|path| path.starts_with('/'))
        .ok_or_else(|| HostError::CommandFailed {
            message: "the host did not resolve the image path".into(),
        })
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

/// Creates the missing parts of [`IMAGE_DIR`] (`0700`) and makes the image directory private
/// when it is not (best effort: a server that refuses `chmod` still gets the upload).
async fn ensure_directory(sftp: &RawSftpSession, failure: &Failure) -> Result<(), HostError> {
    let mut path = String::new();
    for part in IMAGE_DIR.split('/') {
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(part);
        match sftp.stat(path.as_str()).await {
            Ok(found) if is_directory(&found.attrs) => continue,
            Ok(_) => {
                return Err(HostError::CommandFailed {
                    message: format!("~/{path} is not a directory"),
                });
            }
            Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => {}
            Err(error) => return Err(failure.of("reading ~/.cache/or2", error)),
        }
        if let Err(error) = sftp.mkdir(path.as_str(), mode(DIRECTORY_MODE)).await {
            // Another upload may have made it meanwhile.
            if !matches!(sftp.stat(path.as_str()).await, Ok(found) if is_directory(&found.attrs)) {
                return Err(failure.of("creating ~/.cache/or2/images", error));
            }
        }
    }
    if let Ok(found) = sftp.stat(IMAGE_DIR).await
        && found.attrs.permissions.map(|bits| bits & 0o7777) != Some(DIRECTORY_MODE)
    {
        let _ = sftp.setstat(IMAGE_DIR, mode(DIRECTORY_MODE)).await;
    }
    Ok(())
}

/// Removes [`IMAGE_DIR`]'s regular `or2-*` files last modified more than [`SWEEP_AGE`] before
/// `now`. Best effort: whatever fails is left for the next upload.
async fn sweep(sftp: &RawSftpSession, now: Duration) {
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

/// Writes `bytes` to `temporary` (created exclusively, `0600`) with up to [`IN_FLIGHT`]
/// writes in flight, closes it and renames it to `path`.
async fn write_and_rename(
    sftp: &Arc<RawSftpSession>,
    temporary: &str,
    path: &str,
    bytes: Vec<u8>,
    failure: &Failure,
) -> Result<(), HostError> {
    let handle = sftp
        .open(
            temporary,
            OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
            mode(FILE_MODE),
        )
        .await
        .map_err(|error| failure.of("creating the image", error))?
        .handle;
    let written = async {
        let mut writes = JoinSet::new();
        for (index, chunk) in bytes.chunks(CHUNK).enumerate() {
            if writes.len() >= IN_FLIGHT {
                finished(writes.join_next().await, failure)?;
            }
            let (sftp, handle, data) = (Arc::clone(sftp), handle.clone(), chunk.to_vec());
            let offset = (index * CHUNK) as u64;
            writes.spawn(async move { sftp.write(handle, offset, data).await });
        }
        while let Some(result) = writes.join_next().await {
            finished(Some(result), failure)?;
        }
        // A server that ignored the mode the file was created with.
        let _ = sftp.fsetstat(handle.as_str(), mode(FILE_MODE)).await;
        Ok(())
    }
    .await;
    let closed = sftp.close(handle).await;
    written?;
    closed.map_err(|error| failure.of("writing the image", error))?;
    sftp.rename(temporary, path)
        .await
        .map_err(|error| failure.of("naming the image", error))?;
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

fn is_directory(attributes: &FileAttributes) -> bool {
    attributes
        .permissions
        .is_some_and(|bits| bits & MODE_TYPE_MASK == MODE_DIRECTORY)
}

fn is_regular(attributes: &FileAttributes) -> bool {
    attributes
        .permissions
        .is_some_and(|bits| bits & MODE_TYPE_MASK == MODE_REGULAR)
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
}
