//! `mosh::terminate` through the real stop script (`LocalHost`, a restricted `PATH`): which
//! outcomes are "no such server runs any more" (`Ok`) and which leave a server that may still be
//! running (`Err`, so the caller keeps its debt). Only processes the tests start are signalled.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use or2_core::mosh::terminate;
use or2_core::remote::{ExecOutput, LocalHost, RemoteError, RemoteHost};

/// Runs every command with a fixed `PATH`, like an sshd session started with that environment.
struct Hermetic {
    inner: LocalHost,
    path: PathBuf,
}

impl RemoteHost for Hermetic {
    type Stream = <LocalHost as RemoteHost>::Stream;

    async fn exec_rendered(&self, line: &str) -> Result<ExecOutput, RemoteError> {
        let wrapped = format!(
            "env 'PATH={}' /bin/sh -c '{}'",
            self.path.display(),
            line.replace('\'', r"'\''")
        );
        self.inner.exec_rendered(&wrapped).await
    }

    async fn open_unix(&self, path: &str) -> Result<Self::Stream, RemoteError> {
        self.inner.open_unix(path).await
    }
}

fn script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A `PATH` directory that holds `sh` and nothing else.
fn host(dir: &Path) -> Hermetic {
    let path = dir.join("pathbin");
    fs::create_dir_all(&path).unwrap();
    std::os::unix::fs::symlink("/bin/sh", path.join("sh")).unwrap();
    Hermetic {
        inner: LocalHost::new(),
        path,
    }
}

fn first_existing(candidates: [&str; 2]) -> &str {
    candidates
        .into_iter()
        .find(|path| Path::new(path).exists())
        .expect("the test host has the program")
}

fn with_real_ps(host: &Hermetic) {
    let ps = first_existing(["/usr/bin/ps", "/bin/ps"]);
    std::os::unix::fs::symlink(ps, host.path.join("ps")).unwrap();
}

/// A long-running process whose name is `name`; killed and reaped on drop.
struct Named(Child);

impl Named {
    fn start(dir: &Path, name: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let own = dir.join(
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .to_string(),
        );
        fs::create_dir_all(&own).unwrap();
        let program = own.join(name);
        // A symlink: the kernel names the process after the path it was started by.
        std::os::unix::fs::symlink(first_existing(["/usr/bin/sleep", "/bin/sleep"]), &program)
            .unwrap();
        Named(
            Command::new(program)
                .arg("60")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }

    /// Whether the process ends within a few seconds.
    fn ends(&mut self) -> bool {
        for _ in 0..200 {
            if self.0.try_wait().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    /// Whether the process is still running after a moment.
    fn survives(&mut self) -> bool {
        std::thread::sleep(Duration::from_millis(300));
        self.0.try_wait().unwrap().is_none()
    }
}

impl Drop for Named {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A pid that belonged to a process that has been reaped.
fn vanished_pid(dir: &Path) -> u32 {
    let mut gone = Named::start(dir, "gone-quickly");
    let pid = gone.pid();
    gone.0.kill().unwrap();
    gone.0.wait().unwrap();
    pid
}

#[tokio::test]
async fn a_mosh_server_is_signalled_and_ends() {
    let dir = tempfile::tempdir().unwrap();
    let host = host(dir.path());
    with_real_ps(&host);
    let mut server = Named::start(dir.path(), "mosh-server");
    terminate(&host, server.pid()).await.unwrap();
    assert!(server.ends(), "the mosh-server was not signalled");
}

#[tokio::test]
async fn an_unrelated_process_is_left_alone_and_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let host = host(dir.path());
    with_real_ps(&host);
    let mut other = Named::start(dir.path(), "not-a-server");
    terminate(&host, other.pid()).await.unwrap();
    assert!(other.survives(), "an unrelated process was signalled");
}

#[tokio::test]
async fn a_server_that_is_already_gone_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let host = host(dir.path());
    with_real_ps(&host);
    terminate(&host, vanished_pid(dir.path())).await.unwrap();
}

#[tokio::test]
async fn a_host_without_ps_cannot_be_inspected_so_the_stop_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let host = host(dir.path());
    // A real, running mosh-server that nothing can tell from any other process.
    let mut server = Named::start(dir.path(), "mosh-server");
    let error = terminate(&host, server.pid()).await.unwrap_err();
    assert!(matches!(error, RemoteError::Failed(_)), "{error:?}");
    assert!(server.survives(), "nothing may be signalled blind");
}

#[tokio::test]
async fn a_ps_that_fails_is_an_error_while_the_process_exists() {
    let dir = tempfile::tempdir().unwrap();
    let host = host(dir.path());
    // `ps` exists but does not understand `-p` (BusyBox-style): usage text, status 1 or 2.
    for status in [1, 2] {
        script(
            &host.path.join("ps"),
            &format!("echo 'usage: ps' >&2; exit {status}"),
        );
        let server = Named::start(dir.path(), "mosh-server");
        assert!(
            terminate(&host, server.pid()).await.is_err(),
            "ps status {status}, live process"
        );
    }
}

#[tokio::test]
async fn a_failed_signal_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let host = host(dir.path());
    // `ps` claims the pid is a mosh-server, but there is no such process, so `kill` fails and the
    // process cannot be shown to be gone either.
    script(&host.path.join("ps"), "echo mosh-server");
    let error = terminate(&host, vanished_pid(dir.path()))
        .await
        .unwrap_err();
    assert!(matches!(error, RemoteError::Failed(_)), "{error:?}");
}

#[tokio::test]
async fn a_server_that_vanishes_between_the_name_check_and_the_signal_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let host = host(dir.path());
    // The first look names a mosh-server, the second (after the failed signal) finds nothing.
    let seen = dir.path().join("seen");
    script(
        &host.path.join("ps"),
        &format!(
            "if [ -e '{0}' ]; then exit 1; fi; : > '{0}'; echo mosh-server",
            seen.display()
        ),
    );
    terminate(&host, vanished_pid(dir.path())).await.unwrap();
}
