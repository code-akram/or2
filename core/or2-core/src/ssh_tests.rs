use std::sync::mpsc as sync;

use super::*;
use crate::keys::ClientKey;
use crate::session::SessionError;
use russh::server;
use tokio::net::TcpListener;

struct Recorder(sync::Sender<SessionState>);

impl SessionObserver for Recorder {
    fn state_changed(&self, state: &SessionState) {
        let _ = self.0.send(state.clone());
    }
    fn frame_ready(&self) {}
}

#[derive(Clone, Copy)]
enum Mode {
    Accept,
    RejectAuth,
    RejectPty,
    RejectShell,
}

struct Server {
    mode: Mode,
    client_key: russh::keys::PublicKey,
    observed: sync::Sender<String>,
}

impl server::Handler for Server {
    type Error = russh::Error;

    async fn auth_publickey(
        &mut self,
        _: &str,
        key: &russh::keys::PublicKey,
    ) -> Result<server::Auth, Self::Error> {
        Ok(
            if !matches!(self.mode, Mode::RejectAuth) && key == &self.client_key {
                server::Auth::Accept
            } else {
                server::Auth::reject()
            },
        )
    }

    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        drop(channel);
        reply.accept().await;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: russh::ChannelId,
        term: &str,
        columns: u32,
        rows: u32,
        _: u32,
        _: u32,
        _: &[(russh::Pty, u32)],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.observed
            .send(format!("pty:{term}:{columns}:{rows}"))
            .unwrap();
        if matches!(self.mode, Mode::RejectPty) {
            session.channel_failure(channel)?;
        } else {
            session.channel_success(channel)?;
        }
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: russh::ChannelId,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.observed.send("shell".into()).unwrap();
        if matches!(self.mode, Mode::RejectShell) {
            session.channel_failure(channel)?;
        } else {
            session.channel_success(channel)?;
            session.data(channel, b"ready\r\n".to_vec())?;
        }
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        _: russh::ChannelId,
        columns: u32,
        rows: u32,
        _: u32,
        _: u32,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.observed
            .send(format!("resize:{columns}:{rows}"))
            .unwrap();
        Ok(())
    }

    async fn data(
        &mut self,
        channel: russh::ChannelId,
        bytes: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.observed
            .send(format!("input:{}", String::from_utf8_lossy(bytes)))
            .unwrap();
        match bytes {
            b"exit\r" => {
                session.eof(channel)?;
                session.exit_status_request(channel, 17)?;
                session.close(channel)?;
            }
            b"lose\r" => return Err(russh::Error::Disconnect),
            _ => {
                session.data(channel, bytes.to_vec())?;
            }
        }
        Ok(())
    }
}

struct Fixture {
    request: ConnectRequest,
    host: HostKey,
    observed: sync::Receiver<String>,
    socket: sync::Receiver<std::net::TcpStream>,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    fn new(mode: Mode) -> Self {
        runtime().block_on(async {
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let port = listener.local_addr().unwrap().port();
            let host = ClientKey::generate_ed25519("");
            let host_public = HostKey::from_public_key(host.private_key().public_key().clone());
            let key = ClientKey::generate_ed25519("");
            let client_key = key.private_key().public_key().clone();
            let request = ConnectRequest::new(
                &std::net::Ipv4Addr::LOCALHOST.to_string(),
                port,
                "fixture",
                &key.to_stored(),
                &[],
                79,
                23,
            )
            .unwrap();
            let config = Arc::new(server::Config {
                keys: vec![host.private_key().clone()],
                auth_rejection_time: Duration::ZERO,
                auth_rejection_time_initial: Some(Duration::ZERO),
                ..Default::default()
            });
            let (observed, received) = sync::channel();
            let (send_socket, socket) = sync::channel();
            let task = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let socket = socket.into_std().unwrap();
                send_socket.send(socket.try_clone().unwrap()).unwrap();
                let socket = tokio::net::TcpStream::from_std(socket).unwrap();
                let session = server::run_stream(
                    config,
                    socket,
                    Server {
                        mode,
                        client_key,
                        observed,
                    },
                )
                .await
                .unwrap();
                let _ = session.await;
            });
            Fixture {
                request,
                host: host_public,
                observed: received,
                socket,
                task,
            }
        })
    }

    fn connect(&mut self, trust: Vec<HostKey>) -> (SessionHandle, sync::Receiver<SessionState>) {
        self.request.trusted_host_keys = trust;
        let key = self.request.key.to_stored();
        let request = ConnectRequest::new(
            self.request.endpoint.host(),
            self.request.endpoint.port(),
            &self.request.username,
            &key,
            &[],
            self.request.size.columns(),
            self.request.size.rows(),
        )
        .unwrap();
        let request = ConnectRequest {
            trusted_host_keys: self.request.trusted_host_keys.clone(),
            ..request
        };
        let (sender, receiver) = sync::channel();
        (
            start(request, Arc::new(Recorder(sender)), Duration::from_secs(2)),
            receiver,
        )
    }

    fn observed(&self) -> String {
        self.observed.recv_timeout(Duration::from_secs(3)).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn state(receiver: &sync::Receiver<SessionState>) -> SessionState {
    receiver.recv_timeout(Duration::from_secs(5)).unwrap()
}

fn connected(receiver: &sync::Receiver<SessionState>) {
    assert_eq!(state(receiver), SessionState::Authenticating);
    assert_eq!(state(receiver), SessionState::Connected);
}

fn closed(receiver: &sync::Receiver<SessionState>) -> CloseReason {
    let SessionState::Closed(reason) = state(receiver) else {
        panic!("expected closed")
    };
    assert!(receiver.recv_timeout(Duration::from_millis(30)).is_err());
    reason
}

#[test]
fn first_use_and_changed_key_require_an_exact_decision_and_latest_resize() {
    for changed in [false, true] {
        let mut fixture = Fixture::new(Mode::Accept);
        let trusted = if changed {
            vec![HostKey::from_public_key(
                ClientKey::generate_ed25519("")
                    .private_key()
                    .public_key()
                    .clone(),
            )]
        } else {
            vec![]
        };
        let (handle, states) = fixture.connect(trusted.clone());
        let SessionState::AwaitingHostKey(prompt) = state(&states) else {
            panic!("missing prompt")
        };
        assert_eq!(prompt.presented, fixture.host);
        assert_eq!(prompt.previously_trusted, trusted);
        assert_eq!(
            handle.approve_host_key("wrong"),
            Err(SessionError::HostKeyMismatch)
        );
        handle.resize(TerminalSize::new(93, 37).unwrap()).unwrap();
        // Host-key confirmation must not consume the two-second connection budget.
        if !changed {
            std::thread::sleep(Duration::from_millis(2100));
        }
        assert!(states.try_recv().is_err());
        handle
            .approve_host_key(&prompt.presented.fingerprint())
            .unwrap();
        connected(&states);
        assert_eq!(fixture.observed(), "pty:xterm-256color:93:37");
        assert_eq!(fixture.observed(), "shell");
        handle.send_text("hello\r\n".into()).unwrap();
        assert_eq!(fixture.observed(), "input:hello\r");
        handle.resize(TerminalSize::new(101, 41).unwrap()).unwrap();
        assert_eq!(fixture.observed(), "resize:101:41");
        handle.disconnect();
        assert_eq!(closed(&states), CloseReason::Disconnected);
    }
}

#[test]
fn trusted_key_skips_prompt_remote_exit_and_connection_loss_are_distinct() {
    for (text, exit) in [("exit\n", true), ("lose\n", false)] {
        let mut fixture = Fixture::new(Mode::Accept);
        let (handle, states) = fixture.connect(vec![fixture.host.clone()]);
        connected(&states);
        // Do not retain the fixture's duplicate descriptor when testing server-side loss.
        drop(fixture.socket.recv_timeout(Duration::from_secs(1)).unwrap());
        handle.send_text(text.into()).unwrap();
        let reason = closed(&states);
        if exit {
            assert_eq!(
                reason,
                CloseReason::RemoteExited {
                    exit_status: Some(17)
                }
            );
        } else {
            assert!(matches!(
                reason,
                CloseReason::Failed(SessionFailure::ConnectionLost(_))
            ));
        }
    }
}

#[test]
fn host_key_rejection_and_server_loss_during_prompt() {
    for reject in [false, true] {
        let mut fixture = Fixture::new(Mode::Accept);
        let (handle, states) = fixture.connect(vec![]);
        assert!(matches!(state(&states), SessionState::AwaitingHostKey(_)));
        if reject {
            handle.reject_host_key().unwrap();
        } else {
            fixture
                .socket
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .shutdown(std::net::Shutdown::Both)
                .unwrap();
        }
        let reason = closed(&states);
        if reject {
            assert_eq!(reason, CloseReason::Failed(SessionFailure::HostKeyRejected));
        } else {
            assert!(matches!(
                reason,
                CloseReason::Failed(SessionFailure::ConnectionLost(_))
            ));
        }
    }
}

#[test]
fn authentication_pty_and_shell_refusals_never_connect() {
    for mode in [Mode::RejectAuth, Mode::RejectPty, Mode::RejectShell] {
        let mut fixture = Fixture::new(mode);
        let (_handle, states) = fixture.connect(vec![fixture.host.clone()]);
        assert_eq!(state(&states), SessionState::Authenticating);
        assert_eq!(
            closed(&states),
            CloseReason::Failed(if matches!(mode, Mode::RejectAuth) {
                SessionFailure::AuthenticationRejected
            } else {
                SessionFailure::ShellRejected
            })
        );
    }
}

#[test]
fn timeout_and_disconnect_cancel_a_peer_that_never_speaks_ssh() {
    for disconnect in [false, true] {
        let (endpoint, peer) = runtime().block_on(async {
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let endpoint = crate::transport::Endpoint::new(
                &std::net::Ipv4Addr::LOCALHOST.to_string(),
                listener.local_addr().unwrap().port(),
            )
            .unwrap();
            let peer = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                use tokio::io::AsyncReadExt;
                let mut bytes = Vec::new();
                socket.read_to_end(&mut bytes).await.unwrap();
                bytes
            });
            (endpoint, peer)
        });
        let key = ClientKey::generate_ed25519("");
        let request = ConnectRequest::new(
            endpoint.host(),
            endpoint.port(),
            "fixture",
            &key.to_stored(),
            &[],
            80,
            24,
        )
        .unwrap();
        let (sender, states) = sync::channel();
        let handle = start(
            request,
            Arc::new(Recorder(sender)),
            Duration::from_millis(100),
        );
        if disconnect {
            std::thread::sleep(Duration::from_millis(30));
            handle.disconnect();
        }
        assert_eq!(
            closed(&states),
            if disconnect {
                CloseReason::Disconnected
            } else {
                CloseReason::Failed(SessionFailure::TimedOut)
            }
        );
        let bytes = runtime().block_on(async {
            tokio::time::timeout(Duration::from_secs(1), peer)
                .await
                .unwrap()
                .unwrap()
        });
        assert!(bytes.starts_with(b"SSH-2.0-"));
    }
}
