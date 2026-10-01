//! The built `or2-pair` binary: usage, exit codes, and the refusals that need no host key. Every
//! run gets a throwaway HOME (and USER), never the real one, and none of them listens.

use std::path::Path;
use std::process::{Command, Output, Stdio};

fn run(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_or2-pair"))
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("USER", "tester")
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
            "--no-listen",
            "--bind",
            "--ssh-port",
            "--check",
            "--ascii",
            "--address",
        ] {
            assert!(help.contains(option), "{option}");
        }
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
        &["--bind", "not-an-ip"],
        &["stray"],
    ] {
        let out = run(home.path(), args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(text(&out.stderr).contains("or2-pair:"), "{args:?}");
        assert!(out.stdout.is_empty());
    }
}

#[test]
fn check_reports_and_changes_nothing_even_without_a_terminal() {
    let home = tempfile::tempdir().unwrap();
    // Port 1: nothing answers, so the check warns and the run still exits zero.
    let out = run(home.path(), &["--check", "--ssh-port", "1"]);
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
        !report.contains("or2-pair:1?"),
        "no pairing code in a check"
    );
    assert!(!home.path().join(".ssh").exists());
}

#[test]
fn without_a_terminal_it_refuses_to_listen_so_a_pipe_cannot_answer_for_the_user() {
    let home = tempfile::tempdir().unwrap();
    let out = run(home.path(), &["--ssh-port", "1"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("interactive terminal"),
        "{}",
        text(&out.stderr)
    );
    let shown = text(&out.stdout);
    assert!(
        !shown.contains("or2-pair:1?") && !shown.contains("Listening"),
        "{shown}"
    );
    assert!(!home.path().join(".ssh").exists());
}
