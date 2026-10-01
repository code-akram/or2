//! The built `or2-pair` binary against a FIFO where `authorized_keys` should be. Review of 6afa42e:
//! a 0600 FIFO hung `--check` (a blocking write open waits for a reader). It must report and exit
//! promptly with nothing changed. Only a `test-support` build can be pointed at a throwaway home.

#![cfg(all(unix, feature = "test-support"))]

use std::os::unix::ffi::OsStrExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn a_fifo_as_authorized_keys_is_reported_and_does_not_hang_the_check() {
    let home = tempfile::tempdir().unwrap();
    let ssh = home.path().join(".ssh");
    std::fs::create_dir(&ssh).unwrap();
    let fifo = ssh.join("authorized_keys");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: `name` is a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let mut child = Command::new(env!("CARGO_BIN_EXE_or2-pair"))
        .args(["--check", "--ssh-port", "1"])
        .env("HOME", home.path())
        .env("USER", "tester")
        .env("OR2_PAIR_TEST_HOME", home.path())
        .env("OR2_PAIR_TEST_USER", "tester")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run or2-pair");
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            panic!("--check hung on a FIFO");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut report = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take().unwrap(), &mut report).unwrap();
    assert_eq!(status.code(), Some(0), "{report}");
    assert!(report.contains("not a regular file"), "{report}");
}
