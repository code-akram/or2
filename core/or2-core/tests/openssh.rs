//! Disposable OpenSSH interop for the M1 single-session path (`ssh::connect`). Only temporary
//! keys/configuration and an ephemeral loopback listener are used; no home keys, system sshd
//! or existing authorization are touched. Host connections are tested in `host.rs`.

mod common;

use std::fs;
use std::net::Ipv4Addr;
use std::process::Command;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use common::{Grid, Sshd, sshd_ready};
use or2_core::input::{Key, KeyInput, Modifiers};
use or2_core::keys::ClientKey;
use or2_core::session::{
    CloseReason, ConnectRequest, SessionHandle, SessionObserver, SessionState,
};
use or2_core::term::TerminalSize;

impl Sshd {
    fn request(&self, key: &ClientKey) -> ConnectRequest {
        self.authorize(key);
        ConnectRequest::new(
            &Ipv4Addr::LOCALHOST.to_string(),
            self.port,
            &Sshd::username(),
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

struct Observer(mpsc::Sender<SessionState>);
impl SessionObserver for Observer {
    fn state_changed(&self, state: &SessionState) {
        let _ = self.0.send(state.clone());
    }
    fn frame_ready(&self) {}
}

#[test]
fn openssh_shell_types_resizes_encodes_keys_and_reports_output_in_frames() {
    if !sshd_ready() {
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
    if !sshd_ready() {
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
