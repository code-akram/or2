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

        /// The files of runs in `~/.ssh/or2-pair` (not its lock file), sorted.
        fn state_files(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(self.home.path().join(".ssh/or2-pair"))
                .map(|dir| {
                    dir.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                        .filter(|name| name != "lock")
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            names
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
        _stdin: Option<ChildStdin>,
        seen: String,
    }

    impl Running {
        fn start(command: Command) -> Self {
            let mut running = Self::spawn(command);
            let mut stdin = running.child.stdin.take().unwrap();
            writeln!(stdin, "{CODE}").unwrap();
            running._stdin = Some(stdin);
            running
        }

        /// Starts it without typing anything.
        fn spawn(mut command: Command) -> Self {
            let mut child = command.spawn().expect("run or2-pair");
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
                _stdin: None,
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
        assert_eq!(host.state_files().len(), 1, "{what}");
        kill(&running.child, signal);
        let (code, seen) = running.finish();
        assert_eq!(code, Some(1), "{what}:\n{seen}");
        assert!(
            seen.contains("The temporary key was removed"),
            "{what}:\n{seen}"
        );
        assert_eq!(fs::read_to_string(host.keys()).unwrap(), original, "{what}");
        assert!(host.state_files().is_empty(), "{what}");
    }

    /// The pairing id in the code a run printed.
    fn printed_id(seen: &str) -> String {
        seen.lines()
            .find(|line| line.starts_with("or2-pair:2?"))
            .and_then(|line| line.split("&id=").nth(1))
            .unwrap_or_else(|| panic!("no pairing code in:\n{seen}"))
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect()
    }

    /// `or2-pair enroll <id>` as sshd would start it, with a phone's request.
    fn enroll(host: &Host, id: &str) -> (Option<i32>, String) {
        let mut child = host
            .command(env!("CARGO_BIN_EXE_or2-pair"))
            .args(["enroll", id])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        writeln!(
            child.stdin.take().unwrap(),
            "{{\"v\":2,\"key\":\"{OTHER_KEY}\",\"device\":\"p\"}}"
        )
        .unwrap();
        let out = child.wait_with_output().unwrap();
        (out.status.code(), text(&out.stdout))
    }

    #[test]
    fn sigkill_leaves_nothing_usable_and_the_next_run_sweeps_it() {
        // Review of the v2 integration: a run killed with SIGKILL (no handler, no destructor)
        // left its state file and the bootstrap entry, and a holder of K could still enrol until
        // the deadline. The run's lock on its state file ends with the process.
        if !testhost_path_is_usable() {
            return;
        }
        let host = Host::new();
        let original = format!("# mine\n{OTHER_KEY} me@laptop\n");
        host.write_keys(&original);
        let mut running = Running::start(host.testhost(&[]));
        running.until("Waiting for the phone");
        let id = printed_id(&running.seen);
        kill(&running.child, libc::SIGKILL);
        let (code, _) = running.finish();
        assert_eq!(code, None, "killed by the signal");

        // What it left: the entry and the state file, before their deadline...
        let left = fs::read_to_string(host.keys()).unwrap();
        assert!(left.contains(&format!("or2-pair-bootstrap-{id}")), "{left}");
        assert_eq!(host.state_files(), [format!("{id}.json")]);
        // ...which the forced command refuses: nobody holds the state any more.
        let (code, said) = enroll(&host, &id);
        assert_eq!(code, Some(1), "{said}");
        assert_eq!(
            said.lines().nth(1),
            Some("{\"v\":2,\"ok\":false,\"reason\":\"expired\"}"),
            "{said}"
        );
        assert_eq!(fs::read_to_string(host.keys()).unwrap(), left);

        // The next run sweeps both, although the old deadline has not passed.
        let mut command = host.testhost(&[]);
        command.env("OR2_PAIR_TEST_WINDOW_SECS", "1");
        let (code, seen) = Running::start(command).finish();
        assert_eq!(code, Some(1), "{seen}");
        assert!(seen.contains("Timed out"), "{seen}");
        assert_eq!(fs::read_to_string(host.keys()).unwrap(), original);
        assert!(host.state_files().is_empty(), "{:?}", host.state_files());
    }

    /// A pseudo-terminal: the side the person types into, and the terminal or2-pair reads.
    #[cfg(target_os = "linux")]
    fn pty() -> (fs::File, fs::File) {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        // SAFETY: plain calls on descriptors this function owns; `ptsname_r` writes at most
        // `name.len()` bytes.
        unsafe {
            let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
            assert!(master >= 0);
            let master = OwnedFd::from_raw_fd(master);
            assert_eq!(libc::grantpt(master.as_raw_fd()), 0);
            assert_eq!(libc::unlockpt(master.as_raw_fd()), 0);
            let mut name = [0 as libc::c_char; 128];
            assert_eq!(
                libc::ptsname_r(master.as_raw_fd(), name.as_mut_ptr(), name.len()),
                0
            );
            let slave = libc::open(
                name.as_ptr(),
                libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
            );
            assert!(slave >= 0);
            (fs::File::from(master), fs::File::from_raw_fd(slave))
        }
    }

    #[cfg(target_os = "linux")]
    fn echoes(terminal: &fs::File) -> bool {
        use std::os::fd::AsRawFd;
        // SAFETY: `termios` is plain old data that `tcgetattr` fills.
        let mut settings: libc::termios = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::tcgetattr(terminal.as_raw_fd(), &mut settings) },
            0
        );
        settings.c_lflag & libc::ECHO != 0
    }

    /// Waits (polling the terminal's settings, up to 20 s) until its echo is `on`.
    #[cfg(target_os = "linux")]
    fn until_echo(terminal: &fs::File, on: bool) {
        let limit = Instant::now() + Duration::from_secs(20);
        while echoes(terminal) != on {
            assert!(Instant::now() < limit, "the echo never became {on}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The real `or2-pair` with a terminal as its standard input.
    #[cfg(target_os = "linux")]
    fn on_a_terminal(host: &Host, terminal: &fs::File) -> Running {
        let mut command = host.command(env!("CARGO_BIN_EXE_or2-pair"));
        command
            .args([
                "--ssh-port",
                &host.port.to_string(),
                "--address",
                "127.0.0.1",
                "--name",
                "Test",
            ])
            .stdin(terminal.try_clone().unwrap());
        Running::spawn(command)
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_code_is_typed_without_echo_and_the_echo_comes_back() {
        // Review of the v2 integration: the terminal echoed K, into scrollback and recordings.
        use std::io::Read;
        use std::os::fd::AsRawFd;
        if !testhost_path_is_usable() {
            return;
        }
        let host = Host::new();
        let (mut master, terminal) = pty();
        assert!(echoes(&terminal));
        let mut running = on_a_terminal(&host, &terminal);
        // While the code is asked for, the terminal does not echo.
        until_echo(&terminal, false);
        master.write_all(format!("{CODE}\n").as_bytes()).unwrap();
        running.until("Waiting for the phone");
        // After it, echo is back, and nothing of the code came back to the person's side.
        assert!(echoes(&terminal));
        // SAFETY: `master` is open; non-blocking, so an empty buffer is not a wait.
        unsafe {
            let flags = libc::fcntl(master.as_raw_fd(), libc::F_GETFL);
            libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
        let mut echoed = Vec::new();
        let mut buf = [0u8; 256];
        while let Ok(count) = master.read(&mut buf) {
            if count == 0 {
                break;
            }
            echoed.extend_from_slice(&buf[..count]);
        }
        let echoed = String::from_utf8_lossy(&echoed);
        assert!(!echoed.contains("7KQ4"), "{echoed:?}");
        assert!(!running.seen.contains(CODE), "{}", running.seen);
        kill(&running.child, libc::SIGINT);
        let (code, seen) = running.finish();
        assert_eq!(code, Some(1), "{seen}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ctrl_c_at_the_code_prompt_puts_the_echo_back() {
        use std::os::unix::process::ExitStatusExt;
        if !testhost_path_is_usable() {
            return;
        }
        let host = Host::new();
        let (mut master, terminal) = pty();
        let running = on_a_terminal(&host, &terminal);
        until_echo(&terminal, false);
        // Half a code typed, then Ctrl-C (the signal, as the terminal would send it).
        master.write_all(b"7KQ4-M2").unwrap();
        kill(&running.child, libc::SIGINT);
        let mut child = running.child;
        let status = child.wait().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGINT), "{status:?}");
        assert!(
            echoes(&terminal),
            "the echo was put back before the process ended"
        );
        assert!(
            !host.home.path().join(".ssh").exists(),
            "nothing was changed"
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
