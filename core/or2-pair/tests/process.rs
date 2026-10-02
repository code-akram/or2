//! What needs a process of its own: the umask (one per process), the terminal's job control (a
//! stop signal stops the whole process) and signals that end the process. Each test runs its other half in a child: this test
//! binary again, asked for one test by name, with an environment variable that only the child
//! has. Kept out of the unit tests because a child started while another thread holds a `flock`
//! shares that lock until it runs its program, which the lock tests there would notice.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use or2_pair::account::Account;
use or2_pair::state::{LOCK, StateDir};

/// Set only in a child: what it works on.
const CHILD: &str = "OR2_PAIR_TEST_CHILD";

/// This binary, running only the test `name`, with [`CHILD`] set to `value`.
fn child(name: &str, value: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", name, "--test-threads=1", "--nocapture"])
        .env(CHILD, value);
    command
}

fn shown(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

// --- the lock file under a restrictive umask ---------------------------------------------------

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

/// The child's half: under umask 0777 takes the lock, lets it go and takes it again, as one
/// run's start and its ending do.
#[test]
fn child_locks_twice_under_umask_0777() {
    let Some(home) = std::env::var_os(CHILD) else {
        return;
    };
    // SAFETY: `umask` has no preconditions; this process runs this one test.
    unsafe { libc::umask(0o777) };
    let account = Account::new("tester", &home);
    let dir = StateDir::open(&account, false).unwrap().unwrap();
    let first = dir.lock(Duration::ZERO, &|| false).unwrap();
    let lock = Path::new(&home).join(".ssh/or2-pair").join(LOCK);
    assert_eq!(mode(&lock), 0o600, "a new lock file is 0600 at once");
    drop(first);
    drop(dir.lock(Duration::ZERO, &|| false).unwrap());
}

#[test]
fn a_lock_created_under_a_restrictive_umask_stays_usable() {
    // Fix check of the v2 fixes: under umask 0777 the lock file was created mode 000, and the
    // next lock (the same run's ending, and every later run) failed with EACCES.
    let home = tempfile::tempdir().unwrap();
    fs::set_permissions(home.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let account = Account::new("tester", home.path());
    drop(StateDir::open(&account, true).unwrap().unwrap());
    let output = child("child_locks_twice_under_umask_0777", home.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", shown(&output));
    assert!(
        shown(&output).contains("1 passed"),
        "the child ran the test"
    );
    assert_eq!(mode(&home.path().join(".ssh/or2-pair").join(LOCK)), 0o600);
}

// --- Ctrl-C during the login-shell check ---------------------------------------------------------

#[cfg(target_os = "linux")]
mod shell_check {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::time::Instant;

    use or2_pair::checks::{ShellProbe, SystemShell};

    use super::*;

    /// The child's half: the login-shell check with a shell that takes long, a background job of
    /// its own included; it writes its process group to `<dir>/group` once both run.
    #[test]
    fn child_runs_a_slow_shell_check() {
        let Some(dir) = std::env::var_os(CHILD) else {
            return;
        };
        let dir = Path::new(&dir).display().to_string();
        let command = format!(
            "sleep 30 & echo $$ > '{dir}/group.tmp' && mv '{dir}/group.tmp' '{dir}/group'; exec sleep 30"
        );
        let result = SystemShell.run("/bin/sh", &command);
        println!("NOT ENDED: {result:?}");
    }

    /// The processes of `group` that still run (zombies are over).
    fn running(group: libc::pid_t) -> Vec<libc::pid_t> {
        let mut found = Vec::new();
        for entry in fs::read_dir("/proc").unwrap().flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<libc::pid_t>() else {
                continue;
            };
            let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
                continue;
            };
            // `pid (name) state ppid pgrp …`; the name may hold anything, so after its `)`.
            let Some((_, rest)) = stat.rsplit_once(')') else {
                continue;
            };
            let fields: Vec<&str> = rest.split_whitespace().collect();
            if fields.len() > 2
                && fields[2] == group.to_string()
                && fields[0] != "Z"
                && fields[0] != "X"
            {
                found.push(pid);
            }
        }
        found
    }

    #[test]
    fn ctrl_c_during_the_login_shell_check_leaves_no_shell_behind() {
        // Fix check of the v2 fixes: the check's shell runs in a process group of its own (so
        // that its background jobs can be killed at the limit), which the terminal's Ctrl-C does
        // not reach, and or2-pair's handlers are armed only after the code is typed: Ctrl-C
        // during the check ended or2-pair and left the shell running.
        let dir = tempfile::tempdir().unwrap();
        let mut command = child("shell_check::child_runs_a_slow_shell_check", dir.path());
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let process = command.spawn().unwrap();
        let pid = libc::pid_t::try_from(process.id()).unwrap();
        let file = dir.path().join("group");
        let limit = Instant::now() + Duration::from_secs(10);
        while !file.exists() {
            assert!(Instant::now() < limit, "the check started its shell");
            std::thread::yield_now();
        }
        let group: libc::pid_t = fs::read_to_string(&file).unwrap().trim().parse().unwrap();
        assert!(!running(group).is_empty(), "the shell runs");

        // SAFETY: `kill` has no memory preconditions; `pid` is our child.
        assert_eq!(unsafe { libc::kill(pid, libc::SIGINT) }, 0);
        let output = process.wait_with_output().unwrap();
        // SIGKILL takes effect asynchronously: wait (bounded) for the group to be gone.
        let limit = Instant::now() + Duration::from_secs(10);
        let mut left = running(group);
        while !left.is_empty() && Instant::now() < limit {
            std::thread::yield_now();
            left = running(group);
        }
        // SAFETY: as above; whatever the outcome, the test leaves nothing behind.
        unsafe { libc::kill(-group, libc::SIGKILL) };

        assert_eq!(
            output.status.signal(),
            Some(libc::SIGINT),
            "{}",
            shown(&output)
        );
        assert!(
            left.is_empty(),
            "the shell check's processes outlived or2-pair: {left:?}"
        );
    }
}

// --- Ctrl-Z at the code prompt -----------------------------------------------------------------

#[cfg(target_os = "linux")]
mod stop_and_go_on {
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::process::CommandExt;
    use std::time::Instant;

    use or2_pair::prompt::{CodePrompt, Stdin};

    use super::*;

    /// The child's half: reads the code at the real prompt (descriptor 0, the terminal) and
    /// prints what it read.
    #[test]
    fn child_reads_a_code() {
        if std::env::var_os(CHILD).is_none() {
            return;
        }
        let line = Stdin.read_line().expect("a line");
        println!("LINE:{}", line.as_str());
    }

    /// A pseudo-terminal: the side a person types into, and the terminal the program reads.
    fn pty() -> (File, File) {
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
            (File::from(master), File::from_raw_fd(slave))
        }
    }

    fn echoes(fd: RawFd) -> bool {
        // SAFETY: `termios` is plain old data that `tcgetattr` fills.
        let mut settings: libc::termios = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::tcgetattr(fd, &mut settings) }, 0);
        settings.c_lflag & libc::ECHO != 0
    }

    /// Waits until the terminal's echo is `on` (the child changes it); false after 10 s.
    fn until_echo(fd: RawFd, on: bool) -> bool {
        let limit = Instant::now() + Duration::from_secs(10);
        while echoes(fd) != on {
            if Instant::now() > limit {
                return false;
            }
            std::thread::yield_now();
        }
        true
    }

    /// Whatever the terminal sent back to the typing side so far.
    fn echoed(master: &File) -> Vec<u8> {
        // SAFETY: `master` is open; non-blocking so an empty buffer is not a wait.
        unsafe {
            let flags = libc::fcntl(master.as_raw_fd(), libc::F_GETFL);
            libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
        let mut out = Vec::new();
        let mut buf = [0u8; 256];
        while let Ok(count) = (&*master).read(&mut buf) {
            if count == 0 {
                break;
            }
            out.extend_from_slice(&buf[..count]);
        }
        out
    }

    fn signal(pid: libc::pid_t, signal: libc::c_int) {
        // SAFETY: `kill` has no memory preconditions; `pid` is our child.
        assert_eq!(unsafe { libc::kill(pid, signal) }, 0);
    }

    #[test]
    fn ctrl_z_and_going_on_at_the_code_prompt_keeps_the_rest_of_the_code_unechoed() {
        // Fix check of the v2 fixes: SIGTSTP put the terminal back (right, for the shell) and
        // SIGCONT went on reading the code with echo on, so the rest of it was echoed.
        let (mut master, slave) = pty();
        let fd = slave.as_raw_fd();
        // Its own process group, in this session: a stop signal sent to an orphaned process
        // group is discarded.
        let mut command = child("stop_and_go_on::child_reads_a_code", Path::new("1"));
        command
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let process = command.spawn().unwrap();
        let pid = libc::pid_t::try_from(process.id()).unwrap();

        // Echo goes off once the prompt (and its signal handlers) is up.
        assert!(until_echo(fd, false), "the prompt switched echo off");
        master.write_all(b"7KQ4-M2").unwrap();

        signal(pid, libc::SIGTSTP);
        let mut status = 0;
        // SAFETY: `pid` is our child; `status` is written by `waitpid`.
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) },
            pid
        );
        assert!(libc::WIFSTOPPED(status), "stopped: {status:#x}");
        assert!(echoes(fd), "echo is back while it is stopped");

        signal(pid, libc::SIGCONT);
        let off_again = until_echo(fd, false);
        master.write_all(b"XD-9PTM\n").unwrap();
        let output = process.wait_with_output().unwrap();
        let typed_back = echoed(&master);

        assert!(off_again, "echo went off again when it went on");
        assert!(
            typed_back.is_empty(),
            "echoed: {:?}",
            String::from_utf8_lossy(&typed_back)
        );
        assert!(output.status.success(), "{}", shown(&output));
        assert!(
            shown(&output).contains("LINE:7KQ4-M2XD-9PTM"),
            "{}",
            shown(&output)
        );
        assert!(echoes(fd), "echo is back after the prompt");
    }

    /// The child's half: reads the code at the real prompt, and while the prompt puts the
    /// terminal back an ending signal arrives: raised in the thread that puts it back (`raise`),
    /// or sent to the process (`kill`), which another thread may take.
    #[cfg(feature = "test-support")]
    #[test]
    fn child_is_signalled_while_the_prompt_ends() {
        let Some(how) = std::env::var_os(CHILD) else {
            return;
        };
        if how == "raise" {
            // SAFETY: `raise` has no memory preconditions.
            or2_pair::prompt::hooks::on_teardown(|| unsafe {
                libc::raise(libc::SIGINT);
            });
        } else {
            // SAFETY: `kill` of this process has no memory preconditions.
            or2_pair::prompt::hooks::on_teardown(|| unsafe {
                libc::kill(libc::getpid(), libc::SIGTERM);
            });
        }
        let line = Stdin.read_line();
        println!("NOT ENDED: {}", line.is_some());
    }

    /// Types the code into the child above and checks that echo is back after the signal ended
    /// it.
    #[cfg(feature = "test-support")]
    fn signal_while_the_prompt_ends(how: &str, signal: libc::c_int, always: bool) {
        use std::os::unix::process::ExitStatusExt;

        let (mut master, slave) = pty();
        let fd = slave.as_raw_fd();
        let mut command = child(
            "stop_and_go_on::child_is_signalled_while_the_prompt_ends",
            Path::new(how),
        );
        command
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let process = command.spawn().unwrap();
        assert!(until_echo(fd, false), "the prompt switched echo off");
        master.write_all(b"7KQ4-M2XD-9PTM\n").unwrap();
        let output = process.wait_with_output().unwrap();
        if always {
            assert_eq!(output.status.signal(), Some(signal), "{}", shown(&output));
        } else {
            // Another thread may take the signal while this one finishes: either ending is
            // fine, but the signal must not leave echo off.
            assert!(
                output.status.signal() == Some(signal) || output.status.success(),
                "{:?}: {}",
                output.status,
                shown(&output)
            );
        }
        assert!(
            echoes(fd),
            "echo is back although {how} ended the process while the prompt ended"
        );
        let typed_back = echoed(&master);
        assert!(
            typed_back.is_empty(),
            "echoed: {:?}",
            String::from_utf8_lossy(&typed_back)
        );
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn an_ending_signal_while_the_prompt_puts_the_terminal_back_leaves_echo_on() {
        // Fix check of the v2 fixes: the guard told its handlers there was no terminal, then
        // restored it; SIGINT in between re-raised without restoring and echo stayed off.
        signal_while_the_prompt_ends("raise", libc::SIGINT, true);
        signal_while_the_prompt_ends("kill", libc::SIGTERM, false);
    }
}
