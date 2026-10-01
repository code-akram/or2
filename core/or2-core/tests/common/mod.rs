//! Shared by the disposable OpenSSH integration tests (`openssh.rs`, `host.rs`). Only
//! temporary keys/configuration and an ephemeral loopback listener are used; no home keys,
//! system sshd or existing authorization are touched.
#![allow(dead_code)]

use std::fs;
use std::net::{Ipv4Addr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use or2_core::frame::{CellWidth, Row};
use or2_core::keys::ClientKey;
use or2_core::session::{SessionHandle, SessionState};
use or2_core::term::TerminalSize;

pub fn sshd_available() -> bool {
    Path::new("/usr/bin/sshd").exists()
}

/// Whether the sshd tests can run. A missing `/usr/bin/sshd` skips them (printing `SKIP`)
/// unless `OR2_REQUIRE_SSHD` is set, which fails instead: set it in CI so the suite is never
/// vacuously green.
pub fn sshd_ready() -> bool {
    ready("sshd", "OR2_REQUIRE_SSHD", sshd_available())
}

/// Like [`sshd_ready`] for `tmux` (`OR2_REQUIRE_TMUX`).
pub fn tmux_ready() -> bool {
    ready(
        "tmux",
        "OR2_REQUIRE_TMUX",
        Command::new("tmux").arg("-V").output().is_ok(),
    )
}

fn ready(program: &str, require: &str, available: bool) -> bool {
    if !available {
        assert!(
            std::env::var_os(require).is_none(),
            "{require} is set but {program} is absent"
        );
        eprintln!("SKIP: {program} is absent");
    }
    available
}

pub struct Sshd {
    pub directory: tempfile::TempDir,
    child: Child,
    pub port: u16,
    /// The host's public key, as an `authorized_keys`-style line.
    pub host: String,
    /// `TMUX_TMPDIR` of every session this sshd starts: tmux in these tests never touches the
    /// user's default socket.
    pub tmux_dir: PathBuf,
}

impl Sshd {
    pub fn new(certificate_only: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path();
        let host = ClientKey::generate_ed25519("");
        fs::write(path.join("host"), &*host.to_stored()).unwrap();
        fs::set_permissions(path.join("host"), fs::Permissions::from_mode(0o600)).unwrap();
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        fs::write(path.join("authorized"), "").unwrap();
        let tmux_dir = path.join("tmux");
        fs::create_dir(&tmux_dir).unwrap();
        let mut config = format!(
            "Port {port}\nListenAddress {}\nHostKey {}\nAuthorizedKeysFile {}\nPidFile {}\nStrictModes no\nUsePAM no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPrintMotd no\nPrintLastLog no\nSetEnv HOME={} HISTFILE=/dev/null ENV=/dev/null BASH_ENV=/dev/null ZDOTDIR={} TMUX_TMPDIR={}\nLogLevel VERBOSE\n",
            Ipv4Addr::LOCALHOST,
            path.join("host").display(),
            path.join("authorized").display(),
            path.join("pid").display(),
            path.display(),
            path.display(),
            tmux_dir.display(),
        );
        if certificate_only {
            let ca = ClientKey::generate_ed25519("");
            fs::write(path.join("ca"), &*ca.to_stored()).unwrap();
            fs::set_permissions(path.join("ca"), fs::Permissions::from_mode(0o600)).unwrap();
            fs::write(path.join("host.pub"), host.public_key().openssh).unwrap();
            assert!(
                Command::new("ssh-keygen")
                    .args(["-q", "-s"])
                    .arg(path.join("ca"))
                    .args(["-I", "fixture", "-h", "-V", "+1h"])
                    .arg(path.join("host.pub"))
                    .status()
                    .unwrap()
                    .success()
            );
            config.push_str(&format!(
                "HostCertificate {}\nHostKeyAlgorithms ssh-ed25519-cert-v01@openssh.com\n",
                path.join("host-cert.pub").display()
            ));
        }
        fs::write(path.join("config"), config).unwrap();
        let log = fs::File::create(path.join("log")).unwrap();
        let child = Command::new("/usr/bin/sshd")
            .args(["-D", "-e", "-f"])
            .arg(path.join("config"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap();
        let mut fixture = Self {
            directory,
            child,
            port,
            host: host.public_key().openssh,
            tmux_dir,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let log = fs::read_to_string(fixture.directory.path().join("log")).unwrap();
            if log.contains("Server listening on") {
                break;
            }
            assert!(
                fixture.child.try_wait().unwrap().is_none() && Instant::now() < deadline,
                "disposable sshd failed to start: {log}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        fixture
    }

    pub fn username() -> String {
        let user = Command::new("id").arg("-un").output().unwrap();
        assert!(user.status.success());
        std::str::from_utf8(&user.stdout).unwrap().trim().to_owned()
    }

    /// Makes `key` the only key sshd accepts.
    pub fn authorize(&self, key: &ClientKey) {
        fs::write(
            self.directory.path().join("authorized"),
            key.public_key().openssh + "\n",
        )
        .unwrap();
    }

    /// The directory sshd sessions use as `$HOME`.
    pub fn home(&self) -> &Path {
        self.directory.path()
    }

    /// A `tmux` command bound to this sshd's private socket directory and a hermetic
    /// environment, so it can never reach the user's own tmux server.
    pub fn tmux(&self) -> Command {
        let mut command = Command::new("tmux");
        command
            .env("TMUX_TMPDIR", &self.tmux_dir)
            .env("HOME", self.home())
            .env("ZDOTDIR", self.home())
            .env_remove("TMUX")
            .stdin(Stdio::null());
        command
    }
}

impl Drop for Sshd {
    fn drop(&mut self) {
        // Only the private socket's server, if a test started one.
        if self.tmux_dir.starts_with(std::env::temp_dir()) {
            let _ = self
                .tmux()
                .arg("kill-server")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The text of a terminal as the renderer would show it, built from taken frames.
#[derive(Default)]
pub struct Grid {
    pub rows: Vec<Option<Row>>,
    pub full_seen: bool,
    pub size: Option<TerminalSize>,
}

impl Grid {
    /// Applies whatever frame is waiting; returns the screen text.
    pub fn refresh(&mut self, handle: &SessionHandle) -> String {
        if let Some(taken) = handle.take_frame() {
            let frame = taken.frame;
            if frame.is_full() {
                self.full_seen = true;
                self.rows = vec![None; usize::from(frame.size().rows())];
                self.size = Some(frame.size());
            } else {
                assert_eq!(self.size, Some(frame.size()));
            }
            for row in frame.rows() {
                self.rows[usize::from(row.index())] = Some(row.clone());
            }
        }
        self.rows
            .iter()
            .flatten()
            .map(|row| {
                let mut text: String = row
                    .cells()
                    .iter()
                    .map(|cell| {
                        if cell.width == CellWidth::SpacerTail {
                            ""
                        } else if cell.text.is_empty() {
                            " "
                        } else {
                            &cell.text
                        }
                    })
                    .collect();
                text.push('\n');
                text
            })
            .collect()
    }

    pub fn wait(&mut self, handle: &SessionHandle, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let text = self.refresh(handle);
            if text.contains(expected) {
                assert!(self.full_seen);
                return;
            }
            // Do not print the screen: it may contain the runner's account or hostname.
            assert!(
                Instant::now() < deadline,
                "expected output {expected:?} did not arrive in terminal frames"
            );
            assert!(
                !matches!(handle.state(), SessionState::Closed(_)),
                "session closed unexpectedly"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// True if `text` appears in the current screen (after applying any waiting frame).
    pub fn contains(&mut self, handle: &SessionHandle, text: &str) -> bool {
        self.refresh(handle).contains(text)
    }
}
