//! Read only temporary agent histories; execute the same commands as SSH, with no real home.
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use or2_core::directories::{recent, shell_command};
use or2_core::remote::{ExecOutput, LocalHost, RemoteError, RemoteHost};

struct Histories {
    root: tempfile::TempDir,
    shell: PathBuf,
    stdout_len: AtomicUsize,
}

impl Histories {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let shell = root.path().join("shell");
        // Written by a child: no writable descriptor inherited by a parallel test's spawn.
        let mut child = Command::new("sh")
            .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
            .arg(&shell)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"#!/bin/sh\npwd\nprintf 'login-arg:%s\\n' \"$1\"\n")
            .unwrap();
        assert!(child.wait().unwrap().success());
        Self {
            root,
            shell,
            stdout_len: AtomicUsize::new(0),
        }
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.root.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
}

impl RemoteHost for Histories {
    type Stream = <LocalHost as RemoteHost>::Stream;

    async fn exec_rendered(&self, line: &str) -> Result<ExecOutput, RemoteError> {
        let home = self.root.path();
        let wrapped = format!(
            "env 'HOME={}' 'CLAUDE_CONFIG_DIR={}/claude' 'CODEX_HOME={}/codex' 'SHELL={}' sh -c '{}'",
            home.display(),
            home.display(),
            home.display(),
            self.shell.display(),
            line.replace('\'', "'\\''")
        );
        let output = LocalHost::new().exec_rendered(&wrapped).await?;
        self.stdout_len
            .store(output.stdout.len(), Ordering::Relaxed);
        Ok(output)
    }

    async fn open_unix(&self, _path: &str) -> Result<Self::Stream, RemoteError> {
        unreachable!()
    }
}

#[tokio::test]
async fn missing_histories_are_empty_and_nothing_is_created() {
    let host = Histories::new();
    assert!(recent(&host).await.unwrap().is_empty());
    assert!(!host.root.path().join("claude").exists());
    assert!(!host.root.path().join("codex").exists());
}

#[tokio::test]
async fn custom_config_roots_and_jsonl_metadata_are_read_without_transcripts() {
    let host = Histories::new();
    host.write(
        "claude/history.jsonl",
        "partial\n{\"project\":\"/old\",\"timestamp\":1000,\"display\":\"private prompt\"}\n",
    );
    host.write("codex/sessions/2026/10/04/rollout-2026-10-04-abc.jsonl", concat!(
        "{\"type\":\"session_meta\",\"timestamp\":\"2026-10-04T01:00:00.123Z\",\"payload\":{\"cwd\":\"/new\",\"id\":\"made-up\"}}\n",
        "{\"type\":\"turn_context\",\"timestamp\":\"2026-10-04T02:00:00Z\",\"payload\":{\"cwd\":\"/do-not-read\"}}\n"
    ));
    host.write("codex/sessions/2026/10/04/other.jsonl", "{\"type\":\"session_meta\",\"timestamp\":\"2026-10-04T02:00:00Z\",\"payload\":{\"cwd\":\"/do-not-read\"}}\n");
    assert_eq!(recent(&host).await.unwrap(), ["/new", "/old"]);
    assert!(
        fs::read_to_string(host.root.path().join("claude/history.jsonl"))
            .unwrap()
            .contains("private prompt")
    );
}

#[tokio::test]
async fn realistic_large_metadata_and_claude_entries_keep_projects_not_prompts() {
    let host = Histories::new();
    // Real Codex headers include base instructions and exceed 18–22 KiB. Put cwd AFTER
    // the large unknown field too: reading metadata must not depend on field order.
    host.write("codex/sessions/2026/10/04/rollout-large.jsonl", &format!(
        "{{\"payload\":{{\"base_instructions\":{{\"text\":\"{}\"}},\"cwd\":\"/work/codex\"}},\"timestamp\":\"2026-10-04T01:00:00Z\",\"type\":\"session_meta\"}}\n{{\"type\":\"session_meta\",\"timestamp\":\"2026-10-04T03:00:00Z\",\"payload\":{{\"cwd\":\"/transcript-not-metadata\"}}}}\n",
        "private instructions ".repeat(2500)
    ));
    host.write(
        "claude/history.jsonl",
        &format!(
            "{{\"display\":\"{}\",\"timestamp\":1791072000000,\"project\":\"/work/claude\"}}\n",
            "private prompt ".repeat(2000)
        ),
    );
    assert_eq!(
        recent(&host).await.unwrap(),
        ["/work/codex", "/work/claude"]
    );
}

#[tokio::test]
async fn only_the_latest_64_codex_files_and_bounded_first_records_are_read() {
    let host = Histories::new();
    for index in 0..70 {
        host.write(&format!("codex/sessions/2026/10/04/rollout-{index:03}.jsonl"),
            &format!("{{\"type\":\"session_meta\",\"timestamp\":\"1970-01-01T00:00:01Z\",\"payload\":{{\"cwd\":\"/project/{index:03}\"}}}}\n"));
    }
    let paths = recent(&host).await.unwrap();
    assert_eq!(paths.len(), 20);
    assert_eq!(paths[0], "/project/006"); // Equal timestamps sorted by path; files 000..005 not read.
    host.write(
        "codex/sessions/2026/10/04/rollout-zzz.jsonl",
        &format!("{}\n", "x".repeat(10_000)),
    );
    assert!(
        !recent(&host)
            .await
            .unwrap()
            .iter()
            .any(|path| path.contains("zzz"))
    );
}

#[tokio::test]
async fn large_codex_headers_have_an_aggregate_budget_without_losing_claude() {
    let host = Histories::new();
    host.write(
        "claude/history.jsonl",
        &format!(
            "{}\n{{\"project\":\"/work/claude\",\"timestamp\":0}}\n",
            "x".repeat(262144)
        ),
    );
    for index in 0..64 {
        host.write(&format!("codex/sessions/2026/10/04/rollout-{index:03}.jsonl"),
            &format!("{{\"type\":\"session_meta\",\"timestamp\":\"1970-01-01T00:00:01Z\",\"payload\":{{\"cwd\":\"/project/{index:03}\",\"instructions\":\"{}\"}}}}\n", "x".repeat(60_000)));
    }
    let paths = recent(&host).await.unwrap();
    assert!(paths.iter().any(|path| path == "/project/063"));
    assert!(paths.iter().any(|path| path == "/work/claude"));
    assert!(!paths.iter().any(|path| path == "/project/000"));
    // 256 KiB Claude + 512 KiB Codex + format markers. A larger per-file limit alone
    // would exceed RemoteHost's 1 MiB output cap and lose even the Claude result.
    assert_eq!(
        host.stdout_len.load(Ordering::Relaxed),
        262144 + 524288 + 14
    );
    assert!(host.stdout_len.load(Ordering::Relaxed) < or2_core::remote::OUTPUT_CAP);
}

#[tokio::test]
async fn codex_record_at_the_limit_is_accepted_but_one_byte_over_is_rejected() {
    let host = Histories::new();
    for (name, size, path) in [
        ("valid", 65536, "/boundary"),
        ("over", 65537, "/over-limit"),
    ] {
        let prefix = format!(
            "{{\"type\":\"session_meta\",\"timestamp\":\"1970-01-01T00:00:01Z\",\"payload\":{{\"cwd\":\"{path}\",\"instructions\":\""
        );
        let record = format!("{prefix}{}\"}}}}", "x".repeat(size - prefix.len() - 3));
        assert_eq!(record.len(), size);
        host.write(
            &format!("codex/sessions/2026/10/04/rollout-{name}.jsonl"),
            &format!("{record}\n"),
        );
    }
    assert_eq!(recent(&host).await.unwrap(), ["/boundary"]);
}

#[tokio::test]
async fn overlong_or_partial_metadata_is_skipped_and_never_reads_later_records() {
    let host = Histories::new();
    let prefix = "{\"type\":\"session_meta\",\"timestamp\":\"1970-01-01T00:00:01Z\",\"payload\":{\"cwd\":\"/too-large\",\"instructions\":\"";
    host.write("codex/sessions/2026/10/04/rollout-large.jsonl", &format!(
        "{prefix}{}\"}}}}\n{{\"type\":\"session_meta\",\"timestamp\":\"1970-01-01T00:00:02Z\",\"payload\":{{\"cwd\":\"/later-record\"}}}}\n", "x".repeat(70_000)));
    host.write("codex/sessions/2026/10/04/rollout-partial.jsonl", prefix);
    assert!(recent(&host).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_path_is_an_argument_never_shell_code_and_the_login_shell_starts_there() {
    let host = Histories::new();
    let path = host.root.path().join("it's a $(touch INJECTED); project");
    fs::create_dir(&path).unwrap();
    let output = host
        .exec(&shell_command(path.to_str().unwrap()))
        .await
        .unwrap();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{}\nlogin-arg:-l\n", path.display())
    );
    assert!(!host.root.path().join("INJECTED").exists());
    assert!(!path.join("INJECTED").exists());
}

#[tokio::test]
async fn claude_history_is_a_bounded_tail_and_a_partial_first_line_is_ignored() {
    let host = Histories::new();
    host.write("claude/history.jsonl", &format!(
        "{{\"project\":\"/outside-the-tail\",\"timestamp\":9999}}\n{}\n{{\"project\":\"/inside-the-tail\",\"timestamp\":1}}\n",
        "x".repeat(270_000)
    ));
    assert_eq!(recent(&host).await.unwrap(), ["/inside-the-tail"]);
}

#[tokio::test]
async fn newline_file_names_cannot_redirect_the_read_outside_codex_sessions() {
    let host = Histories::new();
    host.write("rollout-private.jsonl", "{\"type\":\"session_meta\",\"timestamp\":\"2026-10-04T00:00:00Z\",\"payload\":{\"cwd\":\"/must-not-read\"}}\n");
    // find prints a path containing a newline. Its second line looks like another absolute
    // file path; the script must reject that line rather than read an unrelated file.
    let trap = format!(
        "codex/sessions/2026/10/04/rollout-cut\n{}/rollout-private.jsonl",
        host.root.path().display()
    );
    host.write(&trap, "malformed\n");
    assert!(recent(&host).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_vanished_directory_fails_instead_of_opening_at_home() {
    let host = Histories::new();
    let path = host.root.path().join("vanished");
    let output = host
        .exec(&shell_command(path.to_str().unwrap()))
        .await
        .unwrap();
    assert_eq!(output.status, Some(1));
    assert!(output.stdout.is_empty());
    assert!(!path.exists());
}
