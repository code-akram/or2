//! The capability probe and tmux commands over `LocalHost`: the same scripts and command
//! lines the SSH host driver runs, executed through `/bin/sh` in a hermetic environment
//! (temporary `$HOME`, restricted `PATH`, a private tmux socket directory). No real herdr or
//! tmux server is ever contacted.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use or2_core::host::HerdrSessionInfo;
use or2_core::probe::probe;
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

fn script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
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
