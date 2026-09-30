use std::sync::mpsc as sync;

use super::*;
use crate::keys::ClientKey;
use crate::session::SessionError;
use russh::server;
use tokio::net::TcpListener;

type ConnectedGate = (
    oneshot::Sender<watch::Receiver<TerminalSize>>,
    oneshot::Receiver<()>,
);
static CONNECTED_GATES: OnceLock<std::sync::Mutex<std::collections::HashMap<u16, ConnectedGate>>> =
    OnceLock::new();

pub(super) async fn before_connected(port: u16, size: watch::Receiver<TerminalSize>) {
    let gate = CONNECTED_GATES
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(&port);
    if let Some((sampled, release)) = gate {
        let _ = sampled.send(size);
        let _ = release.await;
    }
}

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
    StallAuth,
    StallPty,
    StallShell,
    DuplexPressure,
    ReplyFlood,
}

struct Server {
    mode: Mode,
    client_key: russh::keys::PublicKey,
    observed: sync::Sender<String>,
    stop: watch::Receiver<bool>,
    input_bytes: usize,
}

impl server::Handler for Server {
    type Error = russh::Error;

    async fn auth_publickey(
        &mut self,
        _: &str,
        key: &russh::keys::PublicKey,
    ) -> Result<server::Auth, Self::Error> {
        if matches!(self.mode, Mode::StallAuth) {
            let _ = self.stop.changed().await;
            return Ok(server::Auth::reject());
        }
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
        if matches!(self.mode, Mode::StallPty) {
            return Ok(());
        }
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
        if matches!(self.mode, Mode::StallShell) {
            return Ok(());
        }
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
        if matches!(self.mode, Mode::DuplexPressure | Mode::ReplyFlood) {
            if self.input_bytes == 0 {
                if matches!(self.mode, Mode::DuplexPressure) {
                    // More packets than russh's default 100-message client channel buffer.
                    for n in 0..256 {
                        session.data(channel, format!("output-{n:03}\r\n").into_bytes())?;
                    }
                    self.observed.send("output-burst:256".into()).unwrap();
                } else {
                    for _ in 0..4 {
                        session.data(channel, b"\x1b[6n".repeat(8192))?;
                    }
                }
            } else if matches!(self.mode, Mode::ReplyFlood) {
                // Keep the client's large ordered user write stalled ahead of generated replies.
                let _ = self.stop.changed().await;
                return Ok(());
            }
            assert!(bytes.iter().all(|byte| *byte == b'x'));
            self.input_bytes += bytes.len();
            if self.input_bytes == 1024 * 1024 {
                session.data(channel, b"PASTE-DONE\r\n".to_vec())?;
                self.observed.send("paste-complete:1048576".into()).unwrap();
            }
            return Ok(());
        }
        self.observed
            .send(format!("input:{}", String::from_utf8_lossy(bytes)))
            .unwrap();
        match bytes {
            b"exit\r" => {
                session.eof(channel)?;
                session.exit_status_request(channel, 17)?;
                session.close(channel)?;
            }
            b"signal\r" => {
                session.exit_signal_request(channel, russh::Sig::TERM, false, "", "")?;
                session.eof(channel)?;
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
    stop: watch::Sender<bool>,
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
                window_size: if matches!(mode, Mode::DuplexPressure | Mode::ReplyFlood) {
                    1024
                } else {
                    2097152
                },
                ..Default::default()
            });
            let (observed, received) = sync::channel();
            let (send_socket, socket) = sync::channel();
            let (stop, stopping) = watch::channel(false);
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
                        observed: observed.clone(),
                        stop: stopping,
                        input_bytes: 0,
                    },
                )
                .await
                .unwrap();
                let result = session.await;
                let _ = observed.send(format!(
                    "server-end:{}",
                    if result.is_ok() { "ok" } else { "lost" }
                ));
            });
            Fixture {
                request,
                host: host_public,
                observed: received,
                socket,
                task,
                stop,
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
        self.stop.send_replace(true);
        if let Ok(socket) = self.socket.try_recv() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
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
        assert_eq!(fixture.observed(), "resize:93:37");
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
fn live_but_stalled_authentication_and_shell_setup_still_time_out() {
    for mode in [Mode::StallAuth, Mode::StallPty, Mode::StallShell] {
        let mut fixture = Fixture::new(mode);
        let (_handle, states) = fixture.connect(vec![fixture.host.clone()]);
        assert_eq!(state(&states), SessionState::Authenticating);
        assert_eq!(
            closed(&states),
            CloseReason::Failed(SessionFailure::TimedOut)
        );
    }
}

#[test]
fn remote_signal_is_a_shell_exit_not_connection_loss() {
    let mut fixture = Fixture::new(Mode::Accept);
    let (handle, states) = fixture.connect(vec![fixture.host.clone()]);
    connected(&states);
    handle.send_text("signal\n".into()).unwrap();
    assert_eq!(
        closed(&states),
        CloseReason::RemoteExited { exit_status: None }
    );
}

#[test]
fn explicit_disconnect_flushes_an_ssh_disconnect_packet() {
    let mut fixture = Fixture::new(Mode::Accept);
    let (handle, states) = fixture.connect(vec![fixture.host.clone()]);
    connected(&states);
    assert!(fixture.observed().starts_with("pty:"));
    assert_eq!(fixture.observed(), "shell");
    assert_eq!(fixture.observed(), "resize:79:23");
    handle.disconnect();
    assert_eq!(closed(&states), CloseReason::Disconnected);
    // russh server returns success for an SSH disconnect, but an error for raw transport EOF.
    assert_eq!(fixture.observed(), "server-end:ok");
}

#[test]
fn terminal_replies_return_to_the_real_ssh_channel() {
    let mut fixture = Fixture::new(Mode::Accept);
    let (handle, states) = fixture.connect(vec![fixture.host.clone()]);
    connected(&states);
    assert!(fixture.observed().starts_with("pty:"));
    assert_eq!(fixture.observed(), "shell");
    assert_eq!(fixture.observed(), "resize:79:23");
    // The fixture echoes bytes; the DSR must cause Ghostty to send a cursor-position reply.
    handle.send_text("\x1b[6n".into()).unwrap();
    assert_eq!(fixture.observed(), "input:\x1b[6n");
    assert_eq!(fixture.observed(), "input:\x1b[2;1R");
    handle.disconnect();
    assert_eq!(closed(&states), CloseReason::Disconnected);
}

#[test]
fn refused_tcp_connection_keeps_the_io_error_kind_in_the_diagnostic() {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let key = ClientKey::generate_ed25519("");
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
    let (sender, states) = sync::channel();
    let _handle = start(request, Arc::new(Recorder(sender)), Duration::from_secs(2));
    let CloseReason::Failed(SessionFailure::Unreachable(message)) = closed(&states) else {
        panic!("expected refused TCP connection");
    };
    assert!(message.contains("ConnectionRefused"));
    assert!(!message.contains("PRIVATE KEY"));
}

#[test]
fn large_paste_and_more_than_a_channel_buffer_of_output_both_finish() {
    let mut fixture = Fixture::new(Mode::DuplexPressure);
    let (handle, states) = fixture.connect(vec![fixture.host.clone()]);
    connected(&states);
    assert_eq!(fixture.observed(), "pty:xterm-256color:79:23");
    assert_eq!(fixture.observed(), "shell");
    assert_eq!(fixture.observed(), "resize:79:23");
    handle.send_text("x".repeat(1024 * 1024)).unwrap();
    assert_eq!(fixture.observed(), "output-burst:256");
    assert_eq!(fixture.observed(), "paste-complete:1048576");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(frame) = handle.take_frame() {
            let text: String = frame
                .frame
                .rows()
                .iter()
                .flat_map(|row| row.cells())
                .map(|cell| cell.text.as_str())
                .collect();
            if text.contains("PASTE-DONE") {
                break;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "remote output stalled"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(handle.state(), SessionState::Connected);
    handle.disconnect();
    assert_eq!(closed(&states), CloseReason::Disconnected);
}

#[test]
fn slow_writer_query_flood_closes_with_a_reply_budget_diagnostic() {
    let mut fixture = Fixture::new(Mode::ReplyFlood);
    let (handle, states) = fixture.connect(vec![fixture.host.clone()]);
    connected(&states);
    handle.send_text("x".repeat(1024 * 1024)).unwrap();
    assert_eq!(
        closed(&states),
        CloseReason::Failed(SessionFailure::Protocol(
            "generated terminal replies exceed 65536-byte budget".into()
        ))
    );
}

#[test]
fn query_batches_coalesce_and_credit_includes_queued_and_in_flight_bytes() {
    let replies = Rc::new(RefCell::new(GeneratedReplies::new()));
    let collected = replies.clone();
    let mut terminal = TerminalEngine::new(TerminalSize::new(79, 23).unwrap(), move |bytes| {
        collected.borrow_mut().append(bytes)
    })
    .unwrap();
    let (writes, mut queued) = mpsc::unbounded_channel();
    let query = b"\x1b[6n".repeat(1000);
    for _ in 0..10 {
        terminal.write(&query);
        replies.borrow_mut().flush(&writes).unwrap();
    }
    assert_eq!(queued.len(), 10); // One write per vt_write, not per query.
    let Write::Reply(in_flight) = queued.try_recv().unwrap() else {
        panic!("expected reply")
    };
    assert_eq!(in_flight.bytes, b"\x1b[1;1R".repeat(1000));
    let budget = replies.borrow().budget.clone();
    assert_eq!(budget.available_permits(), 65536 - 60000);
    terminal.write(&query);
    assert!(matches!(
        replies.borrow_mut().flush(&writes),
        Err(SessionFailure::Protocol(_))
    ));
    assert_eq!(queued.len(), 9);
    assert_eq!(budget.available_permits(), 4); // Last incomplete batch stays below the cap.
    drop(in_flight);
    drop(queued);
    drop(terminal);
    drop(replies);
    assert_eq!(budget.available_permits(), 65536);
}

#[test]
fn resize_between_final_network_sample_and_connected_reaches_the_pty() {
    let mut fixture = Fixture::new(Mode::Accept);
    let (sampled, arrived) = oneshot::channel();
    let (release, proceed) = oneshot::channel();
    CONNECTED_GATES
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(fixture.request.endpoint.port(), (sampled, proceed));
    let (handle, states) = fixture.connect(vec![fixture.host.clone()]);
    assert_eq!(state(&states), SessionState::Authenticating);
    let mut sampled_size = runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(3), arrived)
            .await
            .unwrap()
            .unwrap()
    });
    assert_eq!(
        *sampled_size.borrow_and_update(),
        TerminalSize::new(79, 23).unwrap()
    );
    let latest = TerminalSize::new(103, 43).unwrap();
    handle.resize(latest).unwrap();
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(3), sampled_size.changed())
            .await
            .unwrap()
            .unwrap();
    });
    release.send(()).unwrap();
    assert_eq!(state(&states), SessionState::Connected);
    handle.send_text("after connected".into()).unwrap();
    assert_eq!(fixture.observed(), "pty:xterm-256color:79:23");
    assert_eq!(fixture.observed(), "shell");
    assert_eq!(fixture.observed(), "resize:103:43");
    assert_eq!(fixture.observed(), "input:after connected");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(frame) = handle.take_frame() {
            assert!(frame.frame.is_full());
            assert_eq!(frame.frame.size(), latest);
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    handle.disconnect();
    assert_eq!(closed(&states), CloseReason::Disconnected);
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
