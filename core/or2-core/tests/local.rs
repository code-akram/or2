//! The capability probe and tmux commands over `LocalHost`: the same scripts and command
//! lines the SSH host driver runs, executed through `/bin/sh` in a hermetic environment
//! (temporary `$HOME`, restricted `PATH`, a private tmux socket directory). No real herdr or
//! tmux server is ever contacted.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use or2_core::host::{HerdrSessionInfo, HostCapabilities};
use or2_core::probe::{HERDR_LIST_TIMEOUT, probe_programs, probe_within};
use or2_core::remote::{ExecOutput, LocalHost, RemoteError, RemoteHost};
use or2_core::tmux;

/// Runs every command with a fixed `HOME` and `PATH`, like an sshd session that was started
/// with that environment.
struct Hermetic {
    inner: LocalHost,
    home: PathBuf,
    path: String,
}

impl RemoteHost for Hermetic {
    type Stream = <LocalHost as RemoteHost>::Stream;

    async fn exec_rendered(&self, line: &str) -> Result<ExecOutput, RemoteError> {
        let wrapped = format!(
            "env 'HOME={}' 'PATH={}' /bin/sh -c {}",
            self.home.display(),
            self.path,
            sh_quote(line)
        );
        self.inner.exec_rendered(&wrapped).await
    }

    async fn open_unix(&self, path: &str) -> Result<Self::Stream, RemoteError> {
        self.inner.open_unix(path).await
    }
}

fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Writes an executable script. A child `sh` writes it, never this process: tests run in
/// parallel threads, and a writable descriptor of ours could be inherited by a process another
/// thread forks, so running the script at once would fail with "Text file busy" (ETXTBSY).
fn script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut writer = Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    writer
        .stdin
        .take()
        .unwrap()
        .write_all(format!("#!/bin/sh\n{body}\n").as_bytes())
        .unwrap();
    assert!(
        writer.wait().unwrap().success(),
        "writing {}",
        path.display()
    );
}

/// `PATH` is `<dir>/pathbin`, which holds nothing but `sh` (the rendered probe line starts
/// with `sh -c`, which a real host finds through sshd's own `PATH`); tests add programs to it.
fn hermetic(dir: &Path) -> Hermetic {
    let path_dir = dir.join("pathbin");
    fs::create_dir_all(&path_dir).unwrap();
    let sh = path_dir.join("sh");
    if !sh.exists() {
        std::os::unix::fs::symlink("/bin/sh", sh).unwrap();
    }
    Hermetic {
        inner: LocalHost::new(),
        home: dir.join("home"),
        path: path_dir.to_str().unwrap().into(),
    }
}

/// The whole capability probe, as a host connection runs it.
async fn probe<H: RemoteHost>(host: &H) -> Result<HostCapabilities, RemoteError> {
    probe_within(host, HERDR_LIST_TIMEOUT, probe_programs(host))
        .await
        .map(|(caps, _)| caps)
}

#[tokio::test]
async fn probe_searches_the_standard_directories_in_order_when_path_has_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let host = hermetic(dir.path());
    let bin = |sub: &str, name: &str| host.home.join(sub).join(name);
    // Every candidate directory that is under $HOME; the later ones lose to the earlier.
    script(&bin(".local/bin", "mosh-server"), "exit 0");
    script(&bin(".cargo/bin", "mosh-server"), "exit 0");
    script(&bin(".cargo/bin", "tmux"), "exit 0");
    script(
        &bin(".local/bin", "herdr"),
        r#"if [ "$1 $2 $3" = "session list --json" ]; then
  echo '{"sessions":[{"name":"default","running":true,"default":true,"socket_path":"/x","new_field":[1,2]},{"name":"other","running":false,"socket_path":"/y"}]}'
fi"#,
    );
    let caps = probe(&host).await.unwrap();
    assert_eq!(
        caps.mosh_server.as_deref(),
        Some(bin(".local/bin", "mosh-server").to_str().unwrap())
    );
    assert_eq!(
        caps.tmux.as_deref(),
        Some(bin(".cargo/bin", "tmux").to_str().unwrap())
    );
    assert_eq!(
        caps.herdr.as_deref(),
        Some(bin(".local/bin", "herdr").to_str().unwrap())
    );
    assert_eq!(
        caps.herdr_sessions,
        [
            HerdrSessionInfo {
                name: "default".into(),
                running: true,
                is_default: true
            },
            HerdrSessionInfo {
                name: "other".into(),
                running: false,
                is_default: false
            },
        ]
    );
}

#[tokio::test]
async fn probe_prefers_command_v_and_reports_missing_programs_without_failing() {
    let dir = tempfile::tempdir().unwrap();
    let path_dir = dir.path().join("pathbin");
    fs::create_dir_all(&path_dir).unwrap();
    script(&path_dir.join("tmux"), "exit 0");
    let host = hermetic(dir.path());
    // The fixed directories include /usr/bin and /usr/local/bin, which may hold a real
    // herdr or mosh-server on the machine running the tests, so a program missing from
    // `$HOME` and `PATH` cannot be asserted missing. Fakes under `$HOME/.local/bin` (searched
    // before the system directories) keep the answer independent of the machine.
    script(&host.home.join(".local/bin/mosh-server"), "exit 0");
    script(
        &host.home.join(".local/bin/herdr"),
        "echo '{\"sessions\":[]}'",
    );
    let caps = probe(&host).await.unwrap();
    assert_eq!(
        caps.tmux.as_deref(),
        Some(path_dir.join("tmux").to_str().unwrap()),
        "command -v wins over the fixed directories"
    );
    assert_eq!(
        caps.mosh_server.as_deref(),
        Some(host.home.join(".local/bin/mosh-server").to_str().unwrap())
    );
    assert_eq!(
        caps.herdr.as_deref(),
        Some(host.home.join(".local/bin/herdr").to_str().unwrap())
    );
    assert!(caps.herdr_sessions.is_empty());
}

#[tokio::test]
async fn probe_picks_a_utf8_locale_in_the_documented_order() {
    let dir = tempfile::tempdir().unwrap();
    let path_dir = dir.path().join("pathbin");
    fs::create_dir_all(&path_dir).unwrap();
    for (listed, expected) in [
        ("C\nPOSIX\nen_US.utf8\nC.utf8\nde_DE.UTF-8", "C.UTF-8"),
        ("C\nPOSIX\nen_US.utf8\nde_DE.UTF-8", "en_US.utf8"),
        ("C\nPOSIX", "en_US.UTF-8"),
    ] {
        script(
            &path_dir.join("locale"),
            &format!(
                "[ \"$1\" = -a ] && printf '%s\\n' '{}'",
                listed.replace('\n', "' '")
            ),
        );
        let host = hermetic(dir.path());
        assert_eq!(
            probe(&host).await.unwrap().utf8_locale,
            expected,
            "{listed:?}"
        );
    }
}

#[tokio::test]
async fn a_herdr_that_fails_or_prints_garbage_is_found_with_no_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let host = hermetic(dir.path());
    let herdr = host.home.join(".local/bin/herdr");
    for body in [
        "exit 1",
        "echo not json",
        "echo '{\"sessions\":7}'",
        "echo '{\"sessions\":[{\"name\":\"no-socket\"}]}'",
        "sleep 0",
    ] {
        script(&herdr, body);
        let caps = probe(&host).await.unwrap();
        assert_eq!(
            caps.herdr.as_deref(),
            Some(herdr.to_str().unwrap()),
            "{body}"
        );
        assert!(caps.herdr_sessions.is_empty(), "{body}");
    }
}

fn tmux_binary() -> Option<PathBuf> {
    ["/usr/bin/tmux", "/usr/local/bin/tmux", "/bin/tmux"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

/// A `tmux` wrapper that pins the private socket directory.
fn private_tmux(dir: &Path, real: &Path) -> (PathBuf, PathBuf) {
    let sockets = dir.join("sockets");
    fs::create_dir_all(&sockets).unwrap();
    let wrapper = dir.join("tmux");
    script(
        &wrapper,
        &format!(
            "unset TMUX\nexport TMUX_TMPDIR={}\nexport HOME={}\nexec {} \"$@\"",
            sockets.display(),
            dir.display(),
            real.display()
        ),
    );
    (wrapper, sockets)
}

#[tokio::test]
async fn tmux_listing_handles_no_server_odd_names_and_activity_order() {
    let Some(real) = tmux_binary() else {
        // OR2_REQUIRE_TMUX (CI) fails instead of skipping, so the test is never vacuous.
        assert!(
            std::env::var_os("OR2_REQUIRE_TMUX").is_none(),
            "OR2_REQUIRE_TMUX is set but tmux is absent"
        );
        eprintln!("SKIP: tmux is absent");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (wrapper, _sockets) = private_tmux(dir.path(), &real);
    let tmux_path = wrapper.to_str().unwrap();
    let host = LocalHost::new();
    // Always remove the private server, even if an assertion fails.
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = Command::new(&self.0)
                .arg("kill-server")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    let _cleanup = Cleanup(wrapper.clone());

    assert!(
        tmux::list_sessions(&host, tmux_path)
            .await
            .unwrap()
            .is_empty(),
        "no server is an empty list"
    );
    // A new session's activity is its creation second: a second apart gives a strict order.
    let oldest_first = ["plain", "with space", "caf\u{e9} \u{65e5}\u{672c}", "it's"];
    for name in oldest_first {
        let status = Command::new(&wrapper)
            .args(["-u", "new-session", "-d", "-s", name, "sleep", "300"])
            .status()
            .unwrap();
        assert!(status.success(), "{name}");
        std::thread::sleep(std::time::Duration::from_millis(1100));
    }
    let sessions = tmux::list_sessions(&host, tmux_path).await.unwrap();
    let names: Vec<&str> = sessions.iter().map(|s| s.name.as_str()).collect();
    let mut newest_first = oldest_first;
    newest_first.reverse();
    assert_eq!(names, newest_first, "most recently active first");
    assert!(
        sessions
            .iter()
            .all(|s| s.windows == 1 && s.attached_clients == 0)
    );

    // After the last session ends the server exits: still an empty list, not an error.
    assert!(
        Command::new(&wrapper)
            .arg("kill-server")
            .status()
            .unwrap()
            .success()
    );
    assert!(
        tmux::list_sessions(&host, tmux_path)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_failing_tmux_is_a_typed_error_not_an_empty_list() {
    let dir = tempfile::tempdir().unwrap();
    let broken = dir.path().join("tmux");
    script(&broken, "echo 'unknown option -- Z' >&2\nexit 1");
    let error = tmux::list_sessions(&LocalHost::new(), broken.to_str().unwrap())
        .await
        .unwrap_err();
    assert_eq!(error, tmux::TmuxError::Failed("unknown option -- Z".into()));
    // A tmux path that does not exist at all fails the exec itself (status 127).
    let error = tmux::list_sessions(&LocalHost::new(), "/nonexistent/tmux")
        .await
        .unwrap_err();
    assert!(matches!(error, tmux::TmuxError::Failed(_)), "{error:?}");
}

#[tokio::test]
async fn tmux_scroll_enters_copy_mode_scrolls_by_lines_and_returns_to_the_bottom() {
    let Some(real) = tmux_binary() else {
        assert!(
            std::env::var_os("OR2_REQUIRE_TMUX").is_none(),
            "OR2_REQUIRE_TMUX is set but tmux is absent"
        );
        eprintln!("SKIP: tmux is absent");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (wrapper, _sockets) = private_tmux(dir.path(), &real);
    let tmux_path = wrapper.to_str().unwrap();
    let host = LocalHost::new();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = Command::new(&self.0)
                .arg("kill-server")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    let _cleanup = Cleanup(wrapper.clone());
    // Two sessions whose names share a prefix: the scroll reaches only the exact one.
    for name in ["it's work", "it's work too"] {
        let status = Command::new(&wrapper)
            .args([
                "-u",
                "new-session",
                "-d",
                "-x",
                "40",
                "-y",
                "10",
                "-s",
                name,
            ])
            .args(["sh", "-c", "seq 1 200; sleep 300"])
            .status()
            .unwrap();
        assert!(status.success(), "{name}");
    }
    let mode = |name: &str| {
        let output = Command::new(&wrapper)
            .args(["display-message", "-p", "-t", &format!("={name}:")])
            .arg("#{pane_in_mode} #{scroll_position}")
            .output()
            .unwrap();
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    // Let the output reach the history.
    for _ in 0..100 {
        let output = Command::new(&wrapper)
            .args([
                "display-message",
                "-p",
                "-t",
                "=it's work:",
                "#{history_size}",
            ])
            .output()
            .unwrap();
        if String::from_utf8_lossy(&output.stdout).trim() != "0" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let clients = tmux::NavClients::new();
    let scroll = |scroll| tmux::scroll(&host, tmux_path, &clients, "it's work", None, scroll);
    use or2_core::host::TargetScroll::{Bottom, Down, Up};

    // Down and Bottom outside copy mode have nothing to do, and succeed.
    scroll(Down { lines: 3 }).await.unwrap();
    scroll(Bottom).await.unwrap();
    assert_eq!(mode("it's work"), "0");
    scroll(Up { lines: 5 }).await.unwrap();
    assert_eq!(mode("it's work"), "1 5");
    assert_eq!(mode("it's work too"), "0", "never a prefix match");
    // A pane already in copy mode keeps its position and scrolls on.
    scroll(Up { lines: 3 }).await.unwrap();
    assert_eq!(mode("it's work"), "1 8");
    scroll(Down { lines: 2 }).await.unwrap();
    assert_eq!(mode("it's work"), "1 6");
    // Zero lines runs nothing.
    scroll(Up { lines: 0 }).await.unwrap();
    assert_eq!(mode("it's work"), "1 6");
    scroll(Bottom).await.unwrap();
    assert_eq!(mode("it's work"), "0");
    // Scrolling down past the bottom leaves copy mode (`copy-mode -e`).
    scroll(Up { lines: 2 }).await.unwrap();
    scroll(Down { lines: 10 }).await.unwrap();
    assert_eq!(mode("it's work"), "0");

    // A session that does not exist is a failure with tmux's message.
    let error = tmux::scroll(&host, tmux_path, &clients, "gone", None, Up { lines: 1 })
        .await
        .unwrap_err();
    assert!(
        matches!(&error, tmux::TmuxError::Failed(message) if message.contains("gone")),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_tmux_history_past_a_mebibyte_keeps_its_newest_whole_lines() {
    use or2_core::history::MAX_HISTORY_BYTES;
    let Some(real) = tmux_binary() else {
        assert!(
            std::env::var_os("OR2_REQUIRE_TMUX").is_none(),
            "OR2_REQUIRE_TMUX is set but tmux is absent"
        );
        eprintln!("SKIP: tmux is absent");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (wrapper, _sockets) = private_tmux(dir.path(), &real);
    let tmux_path = wrapper.to_str().unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = Command::new(&self.0)
                .arg("kill-server")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    let _cleanup = Cleanup(wrapper.clone());
    // 4000 numbered lines of 400 columns, about 1.6 MB: more than the cap, within 5000 rows.
    // The history limit applies to panes made after it is set: set first, in the same command.
    let status = Command::new(&wrapper)
        .args([
            "-u",
            "start-server",
            ";",
            "set-option",
            "-g",
            "history-limit",
            "10000",
            ";",
            "new-session",
            "-d",
            "-x",
            "420",
            "-y",
            "10",
            "-s",
            "wide",
        ])
        .args([
            "sh",
            "-c",
            "seq -f '%04g' 1 4000 | while read n; do printf '%s %0395d\\n' \"$n\" 0; done; \
             echo done; sleep 300",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let host = LocalHost::new();
    let clients = tmux::NavClients::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let read = loop {
        let read = tmux::read_history(&host, tmux_path, &clients, "wide", None, 5000)
            .await
            .unwrap();
        if read.text.lines().any(|line| line == "done") {
            break read;
        }
        assert!(std::time::Instant::now() < deadline, "no history yet");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert!(read.truncated);
    assert!(read.text.len() <= MAX_HISTORY_BYTES);
    let numbered: Vec<&str> = read.text.lines().filter(|line| line.len() == 400).collect();
    // Whole lines only, consecutive, ending with the last one written.
    assert!(read.text.lines().next().unwrap().len() == 400);
    assert!(numbered.len() > 2000, "{}", numbered.len());
    assert!(numbered.last().unwrap().starts_with("4000 "));
    let first: u32 = numbered[0][..4].parse().unwrap();
    for (index, line) in numbered.iter().enumerate() {
        assert_eq!(line[..4].parse::<u32>().unwrap(), first + index as u32);
    }
}
