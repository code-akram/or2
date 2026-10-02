//! A small SFTP server over a real directory, for the in-process SSH server of
//! `connection_tests`: `russh-sftp`'s server side, mapping every path below `root` (relative
//! paths start there, like OpenSSH's `sftp-server` in the home directory). Only what an image
//! upload uses: stat, lstat, mkdir, setstat, open/write/fstat/fsetstat/close,
//! opendir/readdir, remove, rename and realpath. Every request is logged by name. [`Quirks`]
//! make it a slow, careless or hostile server.

use std::collections::HashMap;
use std::fs::{self, DirBuilder, File as FsFile, OpenOptions, Permissions};
use std::os::unix::fs::{DirBuilderExt, FileExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// What `realpath` answers.
#[derive(Clone, Default)]
pub(super) enum Realpath {
    /// The canonical path.
    #[default]
    Real,
    /// A failure status, for every path.
    Fail,
    /// The canonical path, after a pause (the server answers nothing else meanwhile).
    Delay(Duration),
    /// Whatever the function makes of the canonical path (a hostile server).
    Rewrite(Arc<dyn Fn(&str) -> String + Send + Sync>),
}

/// How the server misbehaves. Taken afresh by each SFTP session, except
/// [`Quirks::first_write_delay`], which only the next session gets.
#[derive(Clone, Default)]
pub(super) struct Quirks {
    /// How long the first write waits before it is done (a slow host, for cancel tests).
    pub(super) first_write_delay: Option<Duration>,
    pub(super) realpath: Realpath,
    /// Directories are made with the server's own mode, and `setstat` does nothing.
    pub(super) ignore_directory_modes: bool,
    /// Files are made with the server's own mode, and `fsetstat` does nothing.
    pub(super) ignore_file_modes: bool,
    /// Paths (below the root, relative) reported as owned by another user.
    pub(super) foreign: Vec<PathBuf>,
    /// Handles go out as bytes that are not UTF-8 ([`binary_handles`]).
    pub(super) binary_handles: bool,
}

pub(super) struct FsSftp {
    root: PathBuf,
    files: HashMap<String, FsFile>,
    directories: HashMap<String, Option<Vec<File>>>,
    next: u32,
    log: Arc<Mutex<Vec<String>>>,
    quirks: Quirks,
}

impl FsSftp {
    pub(super) fn new(root: PathBuf, log: Arc<Mutex<Vec<String>>>, quirks: Quirks) -> Self {
        Self {
            root,
            files: HashMap::new(),
            directories: HashMap::new(),
            next: 0,
            log,
            quirks,
        }
    }

    /// The attributes of `metadata` at `path`, with another owner for a foreign path.
    fn attributes(&self, path: &Path, metadata: &fs::Metadata) -> FileAttributes {
        let mut attributes = FileAttributes::from(metadata);
        let foreign = path
            .strip_prefix(&self.root)
            .is_ok_and(|relative| self.quirks.foreign.iter().any(|path| path == relative));
        if foreign {
            attributes.uid = attributes.uid.map(|uid| uid + 1);
        }
        attributes
    }

    fn record(&self, request: &str) {
        self.log.lock().unwrap().push(request.to_owned());
    }

    /// `path` inside `root`: relative paths start at `root`, absolute ones must lie below it.
    fn resolve(&self, path: &str) -> Result<PathBuf, StatusCode> {
        let relative = Path::new(path)
            .strip_prefix(&self.root)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| PathBuf::from(path));
        let mut resolved = self.root.clone();
        for component in relative.components() {
            match component {
                Component::Normal(part) => resolved.push(part),
                Component::CurDir => {}
                _ => return Err(StatusCode::PermissionDenied),
            }
        }
        Ok(resolved)
    }

    fn handle(&mut self) -> String {
        self.next += 1;
        self.next.to_string()
    }

    fn ok(id: u32) -> Status {
        Status {
            id,
            status_code: StatusCode::Ok,
            error_message: "Ok".into(),
            language_tag: "en-US".into(),
        }
    }
}

fn failure(error: std::io::Error) -> StatusCode {
    match error.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

impl russh_sftp::server::Handler for FsSftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(&mut self, _: u32, _: HashMap<String, String>) -> Result<Version, Self::Error> {
        self.record("init");
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        self.record("realpath");
        let resolved = self.resolve(&path)?;
        let canonical = fs::canonicalize(resolved).map_err(failure)?;
        let canonical = canonical.to_string_lossy().into_owned();
        let answer = match &self.quirks.realpath {
            Realpath::Real => canonical,
            Realpath::Fail => return Err(StatusCode::Failure),
            Realpath::Delay(delay) => {
                tokio::time::sleep(*delay).await;
                canonical
            }
            Realpath::Rewrite(rewrite) => rewrite(&canonical),
        };
        Ok(Name {
            id,
            files: vec![File::dummy(answer)],
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.record("stat");
        let path = self.resolve(&path)?;
        let metadata = fs::metadata(&path).map_err(failure)?;
        Ok(Attrs {
            id,
            attrs: self.attributes(&path, &metadata),
        })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.record("lstat");
        let path = self.resolve(&path)?;
        let metadata = fs::symlink_metadata(&path).map_err(failure)?;
        Ok(Attrs {
            id,
            attrs: self.attributes(&path, &metadata),
        })
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        self.record("fstat");
        let file = self.files.get(&handle).ok_or(StatusCode::Failure)?;
        let metadata = file.metadata().map_err(failure)?;
        Ok(Attrs {
            id,
            attrs: FileAttributes::from(&metadata),
        })
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        self.record("mkdir");
        DirBuilder::new()
            .mode(if self.quirks.ignore_directory_modes {
                0o777
            } else {
                attrs.permissions.unwrap_or(0o777) & 0o7777
            })
            .create(self.resolve(&path)?)
            .map_err(failure)?;
        Ok(Self::ok(id))
    }

    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        self.record("setstat");
        if self.quirks.ignore_directory_modes {
            return Ok(Self::ok(id));
        }
        if let Some(mode) = attrs.permissions {
            fs::set_permissions(self.resolve(&path)?, Permissions::from_mode(mode & 0o7777))
                .map_err(failure)?;
        }
        Ok(Self::ok(id))
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        flags: OpenFlags,
        attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        self.record("open");
        let mut options = OpenOptions::new();
        options
            .read(flags.contains(OpenFlags::READ))
            .write(flags.contains(OpenFlags::WRITE))
            .mode(if self.quirks.ignore_file_modes {
                0o666
            } else {
                attrs.permissions.unwrap_or(0o666) & 0o7777
            });
        if flags.contains(OpenFlags::CREATE) && flags.contains(OpenFlags::EXCLUDE) {
            options.create_new(true);
        } else if flags.contains(OpenFlags::CREATE) {
            options.create(true);
        }
        let file = options.open(self.resolve(&filename)?).map_err(failure)?;
        let handle = self.handle();
        self.files.insert(handle.clone(), file);
        Ok(Handle { id, handle })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        if let Some(delay) = self.quirks.first_write_delay.take() {
            tokio::time::sleep(delay).await;
        }
        let file = self.files.get(&handle).ok_or(StatusCode::Failure)?;
        file.write_all_at(&data, offset).map_err(failure)?;
        Ok(Self::ok(id))
    }

    async fn fsetstat(
        &mut self,
        id: u32,
        handle: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        self.record("fsetstat");
        if self.quirks.ignore_file_modes {
            return Ok(Self::ok(id));
        }
        let file = self.files.get(&handle).ok_or(StatusCode::Failure)?;
        if let Some(mode) = attrs.permissions {
            file.set_permissions(Permissions::from_mode(mode & 0o7777))
                .map_err(failure)?;
        }
        Ok(Self::ok(id))
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.record("close");
        let known =
            self.files.remove(&handle).is_some() || self.directories.remove(&handle).is_some();
        if known {
            Ok(Self::ok(id))
        } else {
            Err(StatusCode::Failure)
        }
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        self.record("opendir");
        let mut files = Vec::new();
        for entry in fs::read_dir(self.resolve(&path)?).map_err(failure)? {
            let entry = entry.map_err(failure)?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(failure)?;
            files.push(File::new(
                entry.file_name().to_string_lossy(),
                FileAttributes::from(&metadata),
            ));
        }
        let handle = self.handle();
        self.directories.insert(handle.clone(), Some(files));
        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        self.record("readdir");
        match self.directories.get_mut(&handle) {
            Some(listing) => match listing.take() {
                Some(files) => Ok(Name { id, files }),
                None => Err(StatusCode::Eof),
            },
            None => Err(StatusCode::Failure),
        }
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        self.record("remove");
        fs::remove_file(self.resolve(&filename)?).map_err(failure)?;
        Ok(Self::ok(id))
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        self.record("rename");
        let (from, to) = (self.resolve(&oldpath)?, self.resolve(&newpath)?);
        // SFTP v3 (and OpenSSH) never replaces an existing file.
        if to.exists() {
            return Err(StatusCode::Failure);
        }
        fs::rename(from, to).map_err(failure)?;
        Ok(Self::ok(id))
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let file = self.files.get(&handle).ok_or(StatusCode::Failure)?;
        let mut data = vec![0; len as usize];
        let read = file.read_at(&mut data, offset).map_err(failure)?;
        if read == 0 {
            return Err(StatusCode::Eof);
        }
        data.truncate(read);
        Ok(Data { id, data })
    }
}

/// What a binary handle starts with: two bytes that are not UTF-8.
const BINARY_PREFIX: [u8; 2] = [0xff, 0xfe];

/// Relays SFTP packets between the `client` and an SFTP server on `server`, turning every
/// handle the server gives out into bytes that are not UTF-8 (valid in SFTP, where a handle is
/// an opaque string) and back. A request naming any other handle reaches the server unchanged,
/// so a client that does not send the handle's exact bytes back gets the server's failure.
pub(super) async fn binary_handles<C, S>(client: C, server: S)
where
    C: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut client_read, mut client_write) = tokio::io::split(client);
    let (mut server_read, mut server_write) = tokio::io::split(server);
    let requests = async {
        while let Some(mut packet) = next_packet(&mut client_read).await {
            // CLOSE, READ, WRITE, FSTAT, FSETSTAT, READDIR: the handle follows the request id.
            if matches!(packet.first(), Some(4 | 5 | 6 | 8 | 10 | 12)) {
                replace_handle(&mut packet, |handle| {
                    handle.strip_prefix(&BINARY_PREFIX).map(<[u8]>::to_vec)
                });
            }
            if send_packet(&mut server_write, &packet).await.is_err() {
                break;
            }
        }
        let _ = server_write.shutdown().await;
    };
    let replies = async {
        while let Some(mut packet) = next_packet(&mut server_read).await {
            // HANDLE.
            if packet.first() == Some(&102) {
                replace_handle(&mut packet, |handle| {
                    Some([&BINARY_PREFIX[..], handle].concat())
                });
            }
            if send_packet(&mut client_write, &packet).await.is_err() {
                break;
            }
        }
        let _ = client_write.shutdown().await;
    };
    tokio::join!(requests, replies);
}

async fn next_packet(stream: &mut (impl AsyncRead + Unpin)) -> Option<Vec<u8>> {
    let length = stream.read_u32().await.ok()?;
    let mut packet = vec![0; length as usize];
    stream.read_exact(&mut packet).await.ok()?;
    Some(packet)
}

async fn send_packet(stream: &mut (impl AsyncWrite + Unpin), packet: &[u8]) -> std::io::Result<()> {
    let length = u32::try_from(packet.len()).expect("a small packet");
    stream.write_all(&length.to_be_bytes()).await?;
    stream.write_all(packet).await?;
    stream.flush().await
}

/// Replaces the handle at the start of `packet`'s fields (type, request id, then the handle as
/// an SFTP string) with what `map` makes of it, when it makes anything.
fn replace_handle(packet: &mut Vec<u8>, map: impl Fn(&[u8]) -> Option<Vec<u8>>) {
    let Some(length) = packet.get(5..9) else {
        return;
    };
    let length = u32::from_be_bytes(length.try_into().expect("four bytes")) as usize;
    let Some(handle) = packet.get(9..9 + length) else {
        return;
    };
    let Some(replaced) = map(handle) else {
        return;
    };
    let mut rewritten = packet[..5].to_vec();
    let replaced_length = u32::try_from(replaced.len()).expect("a short handle");
    rewritten.extend_from_slice(&replaced_length.to_be_bytes());
    rewritten.extend_from_slice(&replaced);
    rewritten.extend_from_slice(&packet[9 + length..]);
    *packet = rewritten;
}
