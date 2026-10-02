//! The built `or2-pair` binary: usage, exit codes, the refusals that need no host key, the
//! internal `enroll` command, and the cleanup of a live pairing on SIGINT, SIGTERM and SIGHUP.
//! Every run gets a throwaway HOME (and USER), never the real one.

use std::path::Path;
use std::process::{Command, Output, Stdio};

fn run(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_or2-pair"))
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("USER", "tester")
        // Only a `test-support` build reads these: it is pointed at a throwaway account, never
        // at the real home.
        .env("OR2_PAIR_TEST_HOME", home)
        .env("OR2_PAIR_TEST_USER", "tester")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .expect("run or2-pair")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn help_names_the_tool_and_every_option_and_exits_zero() {
    let home = tempfile::tempdir().unwrap();
    for flag in ["--help", "-h"] {
        let out = run(home.path(), &[flag]);
        assert_eq!(out.status.code(), Some(0));
        let help = text(&out.stdout);
        assert!(
            help.contains("USAGE:") && help.contains("or2-pair [OPTIONS]"),
            "{help}"
        );
        for option in [
            "--manual",
            "--no-listen",
            "--ssh-port",
            "--check",
            "--ascii",
            "--address",
            "Internal:",
            "or2-pair enroll <id>",
        ] {
            assert!(help.contains(option), "{option}");
        }
        assert!(!help.contains("--bind") && !help.contains("--pair-port"));
    }
    assert!(!home.path().join(".ssh").exists());
}

#[test]
fn version_prints_the_crate_version() {
    let home = tempfile::tempdir().unwrap();
    let out = run(home.path(), &["--version"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        text(&out.stdout).trim(),
        format!("or2-pair {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn a_usage_error_exits_two_and_says_how_to_get_help() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        &["--nope"][..],
        &["--ssh-port", "0"],
        &["stray"],
        &["enroll"],
        &["enroll", "not-an-id"],
        &["enroll", "abcdefghijklm", "extra"],
    ] {
        let out = run(home.path(), args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(text(&out.stderr).contains("or2-pair:"), "{args:?}");
        assert!(out.stdout.is_empty());
    }
}

#[test]
fn the_removed_listener_options_are_refused_with_the_contracts_message() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        &["--bind", "10.0.0.5"][..],
        &["--bind=10.0.0.5"],
        &["--pair-port", "5000"],
        &["--manual", "--pair-port=5000"],
    ] {
        let out = run(home.path(), args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let error = text(&out.stderr);
        assert!(
            error.contains("pairing uses the SSH port now; these options are gone"),
            "{args:?}: {error}"
        );
        assert!(out.stdout.is_empty());
    }
}

#[test]
fn without_a_terminal_it_refuses_so_a_pipe_cannot_answer_for_the_user() {
    let home = tempfile::tempdir().unwrap();
    let out = run(home.path(), &["--ssh-port", "1"]);
    assert_eq!(out.status.code(), Some(1));
    let error = text(&out.stderr);
    assert!(
        error.contains("terminal") && error.contains("--manual"),
        "{error}"
    );
    let shown = text(&out.stdout);
    assert!(
        !shown.contains("or2-pair:2?") && !shown.contains("Waiting"),
        "{shown}"
    );
    assert!(!home.path().join(".ssh").exists());
}

#[test]
fn a_user_flag_that_names_someone_else_is_refused_before_anything_is_touched() {
    let home = tempfile::tempdir().unwrap();
    let out = run(
        home.path(),
        &["--user", "or2-not-the-current-user", "--manual"],
    );
    assert_eq!(out.status.code(), Some(1));
    let error = text(&out.stderr);
    assert!(
        error.contains("or2-not-the-current-user") && error.contains("not the account"),
        "{error}"
    );
    let shown = text(&out.stdout);
    assert!(
        !shown.contains("or2-pair:2?") && !shown.contains("Checks"),
        "{shown}"
    );
    assert!(!home.path().join(".ssh").exists());
}

// Everything below runs the binary against a throwaway account, which only a `test-support`
// build can be pointed at.
#[cfg(all(unix, feature = "test-support"))]
mod live {
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Child, ChildStdin};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::*;

    const HOST_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7";
    const OTHER_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBCJz8goXA2qGjRTNHhOwsljhOuCXG/+2B/zTJH/brc5";
    const CODE: &str = "7KQ4-M2XD-9PTM";
    const TESTHOST: &str = env!("CARGO_BIN_EXE_or2-pair-testhost");

    /// A throwaway home and `/etc/ssh`, and a loopback port that answers like OpenSSH 9.9.
    struct Host {
        home: tempfile::TempDir,
        etc: tempfile::TempDir,
        port: u16,
    }

    impl Host {
        fn new() -> Self {
            let home = tempfile::tempdir().unwrap();
            fs::set_permissions(home.path(), fs::Permissions::from_mode(0o755)).unwrap();
            let etc = tempfile::tempdir().unwrap();
            fs::write(
                etc.path().join("ssh_host_ed25519_key.pub"),
                format!("{HOST_KEY}\n"),
            )
            .unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let mut stream = stream;
                    let _ = stream.write_all(b"SSH-2.0-OpenSSH_9.9\r\n");
                }
            });
            Self { home, etc, port }
        }

        fn keys(&self) -> std::path::PathBuf {
            self.home.path().join(".ssh/authorized_keys")
        }

        fn write_keys(&self, text: &str) {
            fs::create_dir_all(self.home.path().join(".ssh")).unwrap();
            fs::set_permissions(
                self.home.path().join(".ssh"),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            fs::write(self.keys(), text).unwrap();
            fs::set_permissions(self.keys(), fs::Permissions::from_mode(0o600)).unwrap();
        }

        fn command(&self, binary: &str) -> Command {
            let mut command = Command::new(binary);
            command
                .env("HOME", self.home.path())
                .env("USER", "tester")
                .env("OR2_PAIR_TEST_HOME", self.home.path())
                .env("OR2_PAIR_TEST_USER", "tester")
                .env("OR2_PAIR_TEST_ETC_SSH", self.etc.path())
                .env("NO_COLOR", "1")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            command
        }

        fn testhost(&self, extra: &[&str]) -> Command {
            let mut command = self.command(TESTHOST);
            command
                .args([
                    "--ssh-port",
                    &self.port.to_string(),
                    "--address",
                    "127.0.0.1",
                    "--name",
                    "Test",
                ])
                .args(extra)
                .stdin(Stdio::piped());
            command
        }
    }

    /// A running testhost: its output, line by line, and the standard input the code went in on
    /// (kept open: a real terminal does not hang up after the code).
    struct Running {
        child: Child,
        lines: mpsc::Receiver<String>,
        _stdin: ChildStdin,
        seen: String,
    }

    impl Running {
        fn start(mut command: Command) -> Self {
            let mut child = command.spawn().expect("run or2-pair-testhost");
            let mut stdin = child.stdin.take().unwrap();
            writeln!(stdin, "{CODE}").unwrap();
            let stdout = child.stdout.take().unwrap();
            let (sender, lines) = mpsc::channel();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if sender.send(line).is_err() {
                        break;
                    }
                }
            });
            Self {
                child,
                lines,
                _stdin: stdin,
                seen: String::new(),
            }
        }

        /// Reads output until a line contains `needle`.
        fn until(&mut self, needle: &str) {
            let limit = Instant::now() + Duration::from_secs(30);
            loop {
                let left = limit.saturating_duration_since(Instant::now());
                let line = self
                    .lines
                    .recv_timeout(left)
                    .unwrap_or_else(|_| panic!("no {needle:?} in:\n{}", self.seen));
                self.seen.push_str(&line);
                self.seen.push('\n');
                if line.contains(needle) {
                    return;
                }
            }
        }

        fn finish(mut self) -> (Option<i32>, String) {
            let limit = Instant::now() + Duration::from_secs(20);
            let status = loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    break status;
                }
                assert!(
                    Instant::now() < limit,
                    "the testhost did not end:\n{}",
                    self.seen
                );
                std::thread::sleep(Duration::from_millis(20));
            };
            while let Ok(line) = self.lines.recv_timeout(Duration::from_secs(1)) {
                self.seen.push_str(&line);
                self.seen.push('\n');
            }
            (status.code(), self.seen)
        }
    }

    /// Whether this path can be a forced command (see the contract's character set); a checkout
    /// somewhere odd skips these tests rather than failing them.
    fn testhost_path_is_usable() -> bool {
        let usable = fs::canonicalize(TESTHOST)
            .ok()
            .is_some_and(|p| or2_pair::bootstrap::check_exe_path(&p).is_ok());
        if !usable {
            eprintln!("SKIP: the test binary's path has characters a forced command cannot carry");
        }
        usable
    }

    fn kill(child: &Child, signal: libc::c_int) {
        // SAFETY: signalling a child process this test started.
        assert_eq!(unsafe { libc::kill(child.id() as libc::pid_t, signal) }, 0);
    }

    #[test]
    fn check_reports_and_changes_nothing_even_without_a_terminal() {
        let host = Host::new();
        let out = host
            .command(env!("CARGO_BIN_EXE_or2-pair"))
            .args(["--check", "--ssh-port", "1"])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        // Port 1: nothing answers, so the check says so and the run still exits zero.
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        let report = text(&out.stdout);
        assert!(
            report.contains("Checks") && report.contains("Nothing was changed"),
            "{report}"
        );
        assert!(
            report.contains("sshd is not answering on port 1"),
            "{report}"
        );
        assert!(
            !report.contains("or2-pair:2?"),
            "no pairing code in a check"
        );
        assert!(!host.home.path().join(".ssh").exists());
    }

    #[test]
    fn manual_prints_a_code_without_an_id_and_changes_nothing() {
        let host = Host::new();
        let out = host
            .command(env!("CARGO_BIN_EXE_or2-pair"))
            .args(["--manual", "--ssh-port", "1", "--address", "198.51.100.7"])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        let shown = text(&out.stdout);
        let code = shown
            .lines()
            .find(|l| l.starts_with("or2-pair:2?"))
            .unwrap();
        assert!(
            code.contains("a=198.51.100.7") && !code.contains("&id="),
            "{code}"
        );
        assert!(!host.home.path().join(".ssh").exists());
    }

    #[test]
    fn enroll_with_no_pairing_in_progress_answers_expired_and_exits_one() {
        let host = Host::new();
        let mut child = host
            .command(env!("CARGO_BIN_EXE_or2-pair"))
            .args(["enroll", "abcdefghijklm"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        writeln!(
            child.stdin.take().unwrap(),
            "{{\"v\":2,\"key\":\"{OTHER_KEY}\",\"device\":\"p\"}}"
        )
        .unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(1));
        let said = text(&out.stdout);
        let lines: Vec<&str> = said.lines().collect();
        assert_eq!(
            lines[0],
            "{\"v\":2,\"hello\":\"or2-pair\",\"id\":\"abcdefghijklm\"}"
        );
        assert_eq!(lines[1], "{\"v\":2,\"ok\":false,\"reason\":\"expired\"}");
        assert!(!host.home.path().join(".ssh/authorized_keys").exists());
    }

    /// Pairs with nobody and ends the live run with `signal`: the temporary key and the state
    /// are gone, the file is as it was.
    fn cleanup_on(signal: libc::c_int, what: &str) {
        if !testhost_path_is_usable() {
            return;
        }
        let host = Host::new();
        let original = format!("# mine\n{OTHER_KEY} me@laptop\n");
        host.write_keys(&original);
        let mut running = Running::start(host.testhost(&[]));
        running.until("Waiting for the phone");
        let keys = fs::read_to_string(host.keys()).unwrap();
        assert!(keys.contains("or2-pair-bootstrap-"), "{what}: {keys}");
        let state = fs::read_dir(host.home.path().join(".ssh/or2-pair"))
            .unwrap()
            .count();
        assert_eq!(state, 1, "{what}");
        kill(&running.child, signal);
        let (code, seen) = running.finish();
        assert_eq!(code, Some(1), "{what}:\n{seen}");
        assert!(
            seen.contains("The temporary key was removed"),
            "{what}:\n{seen}"
        );
        assert_eq!(fs::read_to_string(host.keys()).unwrap(), original, "{what}");
        assert_eq!(
            fs::read_dir(host.home.path().join(".ssh/or2-pair"))
                .unwrap()
                .count(),
            0,
            "{what}"
        );
    }

    #[test]
    fn sigint_removes_the_temporary_key() {
        cleanup_on(libc::SIGINT, "SIGINT");
    }

    #[test]
    fn sigterm_removes_the_temporary_key() {
        cleanup_on(libc::SIGTERM, "SIGTERM");
    }

    #[test]
    fn sighup_removes_the_temporary_key() {
        cleanup_on(libc::SIGHUP, "SIGHUP");
    }

    #[test]
    fn the_timeout_removes_the_temporary_key() {
        if !testhost_path_is_usable() {
            return;
        }
        let host = Host::new();
        let mut command = host.testhost(&[]);
        command.env("OR2_PAIR_TEST_WINDOW_SECS", "1");
        let running = Running::start(command);
        let (code, seen) = running.finish();
        assert_eq!(code, Some(1), "{seen}");
        assert!(seen.contains("Timed out"), "{seen}");
        assert_eq!(fs::read_to_string(host.keys()).unwrap(), "");
    }

    #[test]
    fn a_typo_is_asked_again_and_the_code_is_never_echoed() {
        if !testhost_path_is_usable() {
            return;
        }
        let host = Host::new();
        let mut command = host.testhost(&[]);
        command.env("OR2_PAIR_TEST_WINDOW_SECS", "1");
        let mut child = command.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        // A wrong check character first, then the right code a moment later.
        writeln!(stdin, "7KQ4-M2XD-9PTK").unwrap();
        stdin.flush().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        writeln!(stdin, "{CODE}").unwrap();
        let out = child.wait_with_output().unwrap();
        let shown = text(&out.stdout);
        assert!(shown.contains("That code has a typo"), "{shown}");
        assert!(shown.contains("Waiting for the phone"), "{shown}");
        assert!(
            !shown.contains(CODE) && !shown.contains("7KQ4M2XD9PT"),
            "{shown}"
        );
        assert!(!text(&out.stderr).contains("7KQ4"), "{}", text(&out.stderr));
    }

    #[test]
    fn an_empty_answer_ends_the_run_with_nothing_changed() {
        let host = Host::new();
        let mut child = host.testhost(&[]).spawn().unwrap();
        writeln!(child.stdin.take().unwrap()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(text(&out.stdout).contains("Nothing was changed"));
        assert!(!host.home.path().join(".ssh").exists());
    }
}
