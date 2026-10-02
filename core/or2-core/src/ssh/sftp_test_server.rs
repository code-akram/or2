//! A small SFTP server over a real directory, for the in-process SSH server of
//! `connection_tests`: `russh-sftp`'s server side, mapping every path below `root` (relative
//! paths start there, like OpenSSH's `sftp-server` in the home directory). Only what an image
//! upload uses: stat, mkdir, setstat, open/write/fsetstat/close, opendir/readdir, remove,
//! rename and realpath. Every request is logged by name.

use std::collections::HashMap;
use std::fs::{self, DirBuilder, File as FsFile, OpenOptions, Permissions};
use std::os::unix::fs::{DirBuilderExt, FileExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};

pub(super) struct FsSftp {
    root: PathBuf,
    files: HashMap<String, FsFile>,
    directories: HashMap<String, Option<Vec<File>>>,
    next: u32,
    log: Arc<Mutex<Vec<String>>>,
    /// How long the first write waits before it is done (a slow host, for cancel tests).
    first_write_delay: Option<std::time::Duration>,
}

impl FsSftp {
    pub(super) fn new(
        root: PathBuf,
        log: Arc<Mutex<Vec<String>>>,
        first_write_delay: Option<std::time::Duration>,
    ) -> Self {
        Self {
            root,
            files: HashMap::new(),
            directories: HashMap::new(),
            next: 0,
            log,
            first_write_delay,
        }
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
        Ok(Name {
            id,
            files: vec![File::dummy(canonical.to_string_lossy())],
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.record("stat");
        let metadata = fs::metadata(self.resolve(&path)?).map_err(failure)?;
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
            .mode(attrs.permissions.unwrap_or(0o777) & 0o7777)
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
            .mode(attrs.permissions.unwrap_or(0o666) & 0o7777);
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
        if let Some(delay) = self.first_write_delay.take() {
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
