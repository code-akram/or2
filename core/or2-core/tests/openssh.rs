//! Disposable OpenSSH interop. Only temporary keys/configuration and an ephemeral loopback
//! listener are used; no home keys, system sshd or existing authorization are touched.

use std::fs;
use std::net::{Ipv4Addr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use or2_core::frame::{CellWidth, Row};
use or2_core::input::{Key, KeyInput, Modifiers};
use or2_core::keys::ClientKey;
use or2_core::session::{
    CloseReason, ConnectRequest, SessionHandle, SessionObserver, SessionState,
};
use or2_core::term::TerminalSize;

struct Sshd {
    directory: tempfile::TempDir,
    child: Child,
    port: u16,
    host: String,
}

impl Sshd {
    fn new(certificate_only: bool) -> Self {
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
        let mut config = format!(
            "Port {port}\nListenAddress {}\nHostKey {}\nAuthorizedKeysFile {}\nPidFile {}\nStrictModes no\nUsePAM no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPrintMotd no\nPrintLastLog no\nSetEnv HOME={} HISTFILE=/dev/null ENV=/dev/null BASH_ENV=/dev/null ZDOTDIR={}\nLogLevel VERBOSE\n",
            Ipv4Addr::LOCALHOST,
            path.join("host").display(),
            path.join("authorized").display(),
            path.join("pid").display(),
            path.display(),
            path.display(),
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

    fn request(&self, key: &ClientKey) -> ConnectRequest {
        fs::write(
            self.directory.path().join("authorized"),
            key.public_key().openssh + "\n",
        )
        .unwrap();
        let user = Command::new("id").arg("-un").output().unwrap();
        assert!(user.status.success());
        let username = std::str::from_utf8(&user.stdout).unwrap().trim();
        ConnectRequest::new(
            &Ipv4Addr::LOCALHOST.to_string(),
            self.port,
            username,
            &key.to_stored(),
            std::slice::from_ref(&self.host),
            79,
            23,
        )
        .unwrap()
    }

    fn connect(&self, key: &ClientKey) -> (SessionHandle, mpsc::Receiver<SessionState>) {
        let (sender, states) = mpsc::channel();
        let handle = or2_core::ssh::connect(self.request(key), Arc::new(Observer(sender)));
        assert_eq!(
            states.recv_timeout(Duration::from_secs(5)).unwrap(),
            SessionState::Authenticating
        );
        assert_eq!(
            states.recv_timeout(Duration::from_secs(5)).unwrap(),
            SessionState::Connected
        );
        (handle, states)
    }
}

impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Observer(mpsc::Sender<SessionState>);
impl SessionObserver for Observer {
    fn state_changed(&self, state: &SessionState) {
        let _ = self.0.send(state.clone());
    }
    fn frame_ready(&self) {}
}

#[derive(Default)]
struct Grid {
    rows: Vec<Option<Row>>,
    full_seen: bool,
    size: Option<TerminalSize>,
}

impl Grid {
    fn wait(&mut self, handle: &SessionHandle, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
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
            let text: String = self
                .rows
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
                .collect();
            if text.contains(expected) {
                assert!(self.full_seen);
                return;
            }
            // Do not print the login screen: it may contain the runner's account or hostname.
            assert!(
                Instant::now() < deadline,
                "expected shell output did not arrive in terminal frames"
            );
            assert!(
                !matches!(handle.state(), SessionState::Closed(_)),
                "shell closed unexpectedly"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn openssh_shell_types_resizes_encodes_keys_and_reports_output_in_frames() {
    if !Path::new("/usr/bin/sshd").exists() {
        eprintln!("SKIP: /usr/bin/sshd is absent");
        return;
    }
    let fixture = Sshd::new(false);
    // Ed25519 and RSA exercise separate signature/hash negotiation paths against stock sshd.
    for algorithm in ["ed25519", "rsa"] {
        let key = if algorithm == "ed25519" {
            ClientKey::generate_ed25519("")
        } else {
            let path = fixture.directory.path().join("client");
            let status = Command::new("ssh-keygen")
                .args([
                    "-q", "-t", algorithm, "-b", "2048", "-N", "", "-C", "", "-f",
                ])
                .arg(&path)
                .status()
                .unwrap();
            assert!(status.success());
            ClientKey::from_stored(&fs::read(path).unwrap()).unwrap()
        };
        let (handle, states) = fixture.connect(&key);
        let mut grid = Grid::default();
        handle
            .send_text(
                concat!(
                    r"stty -echo; printf '\033[2J\033[H'; printf 'OR2-%s\n' READY",
                    "\n"
                )
                .into(),
            )
            .unwrap();
        grid.wait(&handle, "OR2-READY");
        handle.resize(TerminalSize::new(93, 37).unwrap()).unwrap();
        handle
            .send_text("printf 'SIZE:'; stty size\n".into())
            .unwrap();
        grid.wait(&handle, "SIZE:37 93");
        assert_eq!(grid.size, Some(TerminalSize::new(93, 37).unwrap()));
        handle
            .send_text("printf 'UTF-%s\\n' 'é界😀'\n".into())
            .unwrap();
        grid.wait(&handle, "UTF-é界😀");
        handle.send_text("printf 'KEY-%s\\n' ".into()).unwrap();
        handle
            .send_key(KeyInput::new(Key::Character("a".into()), Modifiers::default()).unwrap())
            .unwrap();
        handle
            .send_key(KeyInput::new(Key::Enter, Modifiers::default()).unwrap())
            .unwrap();
        grid.wait(&handle, "KEY-a");
        handle.send_text("exit 17\n".into()).unwrap();
        assert_eq!(
            states.recv_timeout(Duration::from_secs(5)).unwrap(),
            SessionState::Closed(CloseReason::RemoteExited {
                exit_status: Some(17)
            })
        );
        assert!(states.recv_timeout(Duration::from_millis(30)).is_err());
    }
}

#[test]
fn certificate_only_host_is_an_unsupported_host_key_not_a_trust_prompt() {
    if !Path::new("/usr/bin/sshd").exists() {
        eprintln!("SKIP: /usr/bin/sshd is absent");
        return;
    }
    let fixture = Sshd::new(true);
    let key = ClientKey::generate_ed25519("");
    let (sender, states) = mpsc::channel();
    let _handle = or2_core::ssh::connect(fixture.request(&key), Arc::new(Observer(sender)));
    assert!(matches!(
        states.recv_timeout(Duration::from_secs(5)).unwrap(),
        SessionState::Closed(CloseReason::Failed(
            or2_core::session::SessionFailure::UnsupportedHostKey(_)
        ))
    ));
}
