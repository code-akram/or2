//! Host driver tests that need no sshd: connection failures, and an in-process russh server
//! that answers exec requests so the query paths (probe caching, output cap, timeout,
//! refusal, loss) can be driven through the public `HostHandle`.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc as sync;

use russh::server;
use tokio::net::TcpListener;

use super::*;
use crate::herdr::{HerdrObserver, HerdrState, HerdrUnavailable};
use crate::host::{TerminalTarget, TerminalTransport, TmuxSession};
use crate::keys::ClientKey;
use crate::session::{SessionFailure, SessionObserver, SessionState};
use crate::ssh::connect_host;
use crate::term::TerminalSize;
use crate::transport::DirectTcp;

struct Recorder(sync::Sender<(HostState, std::thread::ThreadId)>);

impl HostObserver for Recorder {
    fn state_changed(&self, state: &HostState) {
        let _ = self.0.send((state.clone(), std::thread::current().id()));
    }
}

fn recorder() -> (
    Arc<Recorder>,
    sync::Receiver<(HostState, std::thread::ThreadId)>,
) {
    let (sender, states) = sync::channel();
    (Arc::new(Recorder(sender)), states)
}

fn next(states: &sync::Receiver<(HostState, std::thread::ThreadId)>) -> HostState {
    let (state, thread) = states.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_ne!(
        thread,
        std::thread::current().id(),
        "callbacks are Rust-owned"
    );
    state
}

fn closed(states: &sync::Receiver<(HostState, std::thread::ThreadId)>) -> CloseReason {
    let HostState::Closed(reason) = next(states) else {
        panic!("expected closed")
    };
    assert!(
        states.recv_timeout(Duration::from_millis(100)).is_err(),
        "Closed is delivered once, last"
    );
    reason
}

/// A port with nothing listening.
fn dead_port() -> u16 {
    std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn request(addresses: &[(&str, u16)], trusted: &[String]) -> HostConnectRequest {
    let key = ClientKey::generate_ed25519("k").to_stored();
    HostConnectRequest::new(addresses, "fixture", &key, trusted).unwrap()
}

fn fast() -> HostOptions {
    HostOptions {
        connect_timeout: Duration::from_millis(150),
        ..HostOptions::default()
    }
}

#[test]
fn unreachable_addresses_close_with_each_error_listed_from_a_rust_thread() {
    let (observer, states) = recorder();
    let request = request(
        &[("127.0.0.1", dead_port()), ("127.0.0.1", dead_port())],
        &[],
    );
    let handle = connect_host(request, observer);
    let CloseReason::Failed(SessionFailure::Unreachable(message)) = closed(&states) else {
        panic!("expected Unreachable")
    };
    assert!(
        message.contains("address 0: connection refused"),
        "{message}"
    );
    assert!(
        message.contains("address 1: connection refused"),
        "{message}"
    );
    assert!(
        !message.contains("127.0.0.1"),
        "no host names in diagnostics"
    );
    assert!(matches!(handle.state(), HostState::Closed(_)));
}

/// A transport whose connections never complete: a host that silently drops every packet.
struct Blackhole;

impl Transport for Blackhole {
    type Stream = tokio::io::DuplexStream;

    async fn connect(&self, _: &crate::transport::Endpoint) -> std::io::Result<Self::Stream> {
        std::future::pending().await
    }
}

fn connect_over_blackhole(
    options: HostOptions,
    addresses: &[(&str, u16)],
) -> (
    HostHandle,
    sync::Receiver<(HostState, std::thread::ThreadId)>,
) {
    let (observer, states) = recorder();
    let handle = crate::ssh::connect_host_with(
        Arc::new(Blackhole),
        request(addresses, &[]),
        observer,
        options,
    );
    (handle, states)
}

#[test]
fn a_connect_timeout_that_fires_while_the_race_runs_says_what_each_address_did() {
    // The per-address allowance is longer than the connect timeout here, so the overall timer
    // ends it: the host is unreachable (no address ever connected), not "timed out".
    let (_handle, states) = connect_over_blackhole(
        HostOptions {
            connect_timeout: Duration::from_millis(400),
            address_timeout: Duration::from_secs(60),
            ..HostOptions::default()
        },
        &[("a.invalid", 22), ("b.invalid", 22)],
    );
    let CloseReason::Failed(SessionFailure::Unreachable(message)) = closed(&states) else {
        panic!("expected Unreachable")
    };
    assert!(
        message.contains("address 0: still trying after"),
        "{message}"
    );
    assert!(
        message.contains("address 1: still trying after"),
        "{message}"
    );
    assert!(!message.contains("invalid"), "no host names: {message}");
}

#[test]
fn each_address_gets_its_own_timeout_and_the_failure_lists_them_all() {
    let started = std::time::Instant::now();
    let (_handle, states) = connect_over_blackhole(
        HostOptions {
            connect_timeout: Duration::from_secs(30),
            address_timeout: Duration::from_millis(300),
            stagger: Duration::from_millis(100),
            ..HostOptions::default()
        },
        &[("a.invalid", 22), ("b.invalid", 22)],
    );
    let CloseReason::Failed(SessionFailure::Unreachable(message)) = closed(&states) else {
        panic!("expected Unreachable")
    };
    assert!(
        message.contains("address 0: no answer within 0.3 s")
            && message.contains("address 1: no answer within 0.3 s"),
        "{message}"
    );
    // Far inside the overall timeout: one blackholed address cannot consume the whole budget.
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_peer_that_never_speaks_ssh_times_out_and_disconnect_cancels_it() {
    for disconnect in [false, true] {
        let (listener, port) = runtime().block_on(async {
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let port = listener.local_addr().unwrap().port();
            (listener, port)
        });
        // Accept and read everything the client sends, never answer; ends when it hangs up.
        let peer = runtime().spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(&mut socket, &mut bytes)
                .await
                .unwrap();
            bytes
        });
        let (observer, states) = recorder();
        let handle = connect_host_with_options(port, observer, fast());
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
            tokio::time::timeout(Duration::from_secs(2), peer)
                .await
                .unwrap()
                .unwrap()
        });
        assert!(bytes.starts_with(b"SSH-2.0-"));
    }
}

fn connect_host_with_options(
    port: u16,
    observer: Arc<Recorder>,
    options: HostOptions,
) -> HostHandle {
    start(
        Arc::new(DirectTcp),
        request(&[("127.0.0.1", port)], &[]),
        observer,
        options,
    )
}

// ---------------------------------------------------------------------------------------
// An in-process SSH server that answers exec requests.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Listing {
    Sessions,
    NoServer,
    Huge,
    Hang,
    Refuse,
    Lose,
    /// Prints the marker (a secret, as `mosh-server new` would), then: the command finishes.
    SecretThenFinish(&'static str),
    /// Prints the marker, then never finishes.
    SecretThenHang(&'static str),
    /// Prints the marker, then more than the output cap.
    SecretThenHuge(&'static str),
}

const PROBE_WITH_TMUX: &str = "or2:tmux:/fake/tmux\nor2:locale:C.UTF-8\nor2:end\n";
const PROBE_WITH_HERDR: &str =
    "or2:tmux:/fake/tmux\nor2:herdr:/fake/herdr\nor2:locale:C.UTF-8\nor2:end\n";
const PROBE_WITHOUT_PROGRAMS: &str = "or2:tmux:\nor2:herdr:\nor2:locale:C.UTF-8\nor2:end\n";

/// What `herdr session list --json` prints.
#[derive(Clone, Copy)]
enum HerdrList {
    /// The captured listing of the herdr client's tests.
    Fixture,
    Json(&'static str),
    /// herdr exits with an error.
    Fails,
    /// herdr never answers (a wedged herdr server).
    Hang,
}

/// What the server does with a `direct-streamlocal@openssh.com` open.
#[derive(Clone, Debug)]
enum Streamlocal {
    /// Accepts and plays a herdr server: acknowledges `events.subscribe` and keeps the stream
    /// open, answers `session.snapshot`.
    Herdr,
    Refuse(russh::ChannelOpenFailure),
}

/// herdr's answers, from the sanitized captures the herdr client's own tests use.
const HERDR_LISTING: &str = include_str!("../herdr/fixtures/session_list.json");
const HERDR_ACK: &str = include_str!("../herdr/fixtures/ack.json");
const HERDR_SNAPSHOT: &str = include_str!("../herdr/fixtures/snapshot_two_panes.json");

struct Shared {
    streamlocal: Mutex<Streamlocal>,
    listing: Mutex<Listing>,
    probes: AtomicUsize,
    /// What the capability probe prints.
    probe: Mutex<&'static str>,
    herdr_list: Mutex<HerdrList>,
    /// The panes herdr was asked to focus, in order (`w9:p9` does not exist).
    focused: Mutex<Vec<String>>,
    /// The capability probe starts (and is counted) but never finishes.
    probe_hangs: AtomicBool,
    /// Refuse session channels like an sshd at `MaxSessions`.
    refuse_channels: AtomicBool,
    /// Never answer public key authentication.
    stall_auth: AtomicBool,
    /// Channels the client closed.
    closes: AtomicUsize,
    /// A handle on the accepted socket, so a test can hang up on the client. Kept only when
    /// asked: a duplicate descriptor would keep the connection open after the server ends.
    socket: Mutex<Option<std::net::TcpStream>>,
    keep_socket: bool,
    channel_events: Mutex<Option<sync::Sender<(&'static str, russh::ChannelId)>>>,
    focus_hangs: AtomicBool,
    open_gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    /// Holds back the confirmation of the next streamlocal open.
    unix_gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    /// Whether the server answers `pty-req` and `shell` (it never does by default: the setup
    /// timeout tests need a server that stays silent).
    shell_answers: AtomicBool,
    withheld_stage: Mutex<Option<&'static str>>,
}

struct Server {
    client_key: russh::keys::PublicKey,
    shared: Arc<Shared>,
}

impl server::Handler for Server {
    type Error = russh::Error;

    async fn auth_publickey(
        &mut self,
        _: &str,
        key: &russh::keys::PublicKey,
    ) -> Result<server::Auth, Self::Error> {
        if self.shared.stall_auth.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        Ok(if key == &self.client_key {
            server::Auth::Accept
        } else {
            server::Auth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let channel_id = channel.id();
        drop(channel);
        let gate = self.shared.open_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            let shared = self.shared.clone();
            if let Some(tx) = shared.channel_events.lock().unwrap().as_ref() {
                let _ = tx.send(("pending", channel_id));
            }
            tokio::spawn(async move {
                let _ = gate.await;
                reply.accept().await;
                if let Some(tx) = shared.channel_events.lock().unwrap().as_ref() {
                    let _ = tx.send(("accepted", channel_id));
                }
            });
            return Ok(());
        }
        if self.shared.refuse_channels.load(Ordering::SeqCst) {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
        } else {
            reply.accept().await;
            if let Some(tx) = self.shared.channel_events.lock().unwrap().as_ref() {
                let _ = tx.send(("session", channel_id));
            }
        }
        Ok(())
    }

    async fn channel_open_direct_streamlocal(
        &mut self,
        channel: russh::Channel<server::Msg>,
        _: &str,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let channel_id = channel.id();
        drop(channel);
        let gate = self.shared.unix_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            if let Some(tx) = self.shared.channel_events.lock().unwrap().as_ref() {
                let _ = tx.send(("pending", channel_id));
            }
            tokio::spawn(async move {
                let _ = gate.await;
                reply.accept().await;
            });
            return Ok(());
        }
        let behaviour = self.shared.streamlocal.lock().unwrap().clone();
        match behaviour {
            Streamlocal::Herdr => reply.accept().await,
            Streamlocal::Refuse(reason) => reply.reject(reason).await,
        }
        Ok(())
    }

    /// A herdr server for the streamlocal channels: one request per line.
    async fn data(
        &mut self,
        channel: russh::ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let request = String::from_utf8_lossy(data);
        if request.contains("events.subscribe") {
            session.data(channel, HERDR_ACK.as_bytes().to_vec())?;
        } else if request.contains("pane.focus") {
            let pane = request
                .split("\"pane_id\":\"")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .unwrap_or("")
                .to_owned();
            let reply = if pane == "w9:p9" {
                "{\"id\":\"or2_focus\",\"error\":{\"code\":\"pane_not_found\",\"message\":\"no such pane\"}}\n"
            } else {
                "{\"id\":\"or2_focus\",\"result\":{\"type\":\"ok\"}}\n"
            };
            self.shared.focused.lock().unwrap().push(pane);
            if let Some(tx) = self.shared.channel_events.lock().unwrap().as_ref() {
                let _ = tx.send(("focus", channel));
            }
            if self.shared.focus_hangs.load(Ordering::SeqCst) {
                return Ok(());
            }
            session.data(channel, reply.as_bytes().to_vec())?;
        } else if request.contains("session.snapshot") {
            session.data(
                channel,
                format!("{}\n", HERDR_SNAPSHOT.trim_end()).into_bytes(),
            )?;
        }
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel_id: russh::ChannelId,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if let Some(tx) = self.shared.channel_events.lock().unwrap().as_ref() {
            let _ = tx.send(("close", channel_id));
        }
        self.shared.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: russh::ChannelId,
        _: &str,
        _: u32,
        _: u32,
        _: u32,
        _: u32,
        _: &[(russh::Pty, u32)],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if let Some(tx) = self.shared.channel_events.lock().unwrap().as_ref() {
            let _ = tx.send(("pty", channel));
        }
        if *self.shared.withheld_stage.lock().unwrap() == Some("pty") {
            return Ok(());
        }
        if self.shared.shell_answers.load(Ordering::SeqCst) {
            session.channel_success(channel)?;
        }
        Ok(())
    }

    /// A login shell that just stays open.
    async fn shell_request(
        &mut self,
        channel: russh::ChannelId,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if let Some(tx) = self.shared.channel_events.lock().unwrap().as_ref() {
            let _ = tx.send(("shell", channel));
        }
        if *self.shared.withheld_stage.lock().unwrap() == Some("shell") {
            return Ok(());
        }
        if self.shared.shell_answers.load(Ordering::SeqCst) {
            session.channel_success(channel)?;
        }
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: russh::ChannelId,
        command: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(command).into_owned();
        if command.contains("'session' 'list' '--json'") {
            session.channel_success(channel)?;
            let listing = *self.shared.herdr_list.lock().unwrap();
            return match listing {
                HerdrList::Fixture => {
                    session.data(channel, HERDR_LISTING.as_bytes().to_vec())?;
                    finish(session, channel, 0)
                }
                HerdrList::Json(json) => {
                    session.data(channel, json.as_bytes().to_vec())?;
                    finish(session, channel, 0)
                }
                HerdrList::Fails => {
                    session.extended_data(channel, 1, b"herdr: boom\n".to_vec())?;
                    finish(session, channel, 1)
                }
                HerdrList::Hang => Ok(()),
            };
        }
        if command.starts_with("sh -c") && command.contains("or2:list-begin") {
            // The probe's second script: finds herdr and lists its sessions in one exec. It
            // hangs with the first script when the test says the probe hangs.
            session.channel_success(channel)?;
            if self.shared.probe_hangs.load(Ordering::SeqCst) {
                return Ok(());
            }
            let probe = *self.shared.probe.lock().unwrap();
            let path = probe
                .lines()
                .find_map(|line| line.strip_prefix("or2:herdr:"))
                .unwrap_or("");
            let mut output = format!("or2:herdr:{path}\n");
            if !path.is_empty() {
                let listing = *self.shared.herdr_list.lock().unwrap();
                let (json, status) = match listing {
                    HerdrList::Fixture => (HERDR_LISTING, 0),
                    HerdrList::Json(json) => (json, 0),
                    HerdrList::Fails => ("", 1),
                    HerdrList::Hang => {
                        // herdr's path, then a listing that never ends.
                        output.push_str("or2:list-begin\n");
                        session.data(channel, output.into_bytes())?;
                        return Ok(());
                    }
                };
                output.push_str(&format!("or2:list-begin\n{json}\nor2:list-end:{status}\n"));
            }
            session.data(channel, output.into_bytes())?;
            return finish(session, channel, 0);
        }
        if command.starts_with("sh -c") {
            self.shared.probes.fetch_add(1, Ordering::SeqCst);
            session.channel_success(channel)?;
            if self.shared.probe_hangs.load(Ordering::SeqCst) {
                return Ok(());
            }
            let output = *self.shared.probe.lock().unwrap();
            session.data(channel, output.as_bytes().to_vec())?;
            return finish(session, channel, 0);
        }
        assert!(command.contains("list-sessions"), "{command}");
        let listing = *self.shared.listing.lock().unwrap();
        match listing {
            Listing::Sessions => {
                session.channel_success(channel)?;
                session.data(channel, b"1:0:10:20:older\n2:1:11:90:newer\n".to_vec())?;
                finish(session, channel, 0)
            }
            Listing::NoServer => {
                session.channel_success(channel)?;
                session.extended_data(
                    channel,
                    1,
                    b"no server running on /tmp/tmux-1000/default\n".to_vec(),
                )?;
                finish(session, channel, 1)
            }
            Listing::Huge => {
                session.channel_success(channel)?;
                for _ in 0..65 {
                    session.data(channel, vec![b'x'; 16 * 1024])?;
                }
                finish(session, channel, 0)
            }
            Listing::Hang => Ok(()),
            Listing::Refuse => {
                session.channel_failure(channel)?;
                Ok(())
            }
            Listing::Lose => Err(russh::Error::Disconnect),
            Listing::SecretThenFinish(secret) => {
                session.channel_success(channel)?;
                session.data(channel, secret.as_bytes().to_vec())?;
                finish(session, channel, 0)
            }
            Listing::SecretThenHang(secret) => {
                session.channel_success(channel)?;
                session.data(channel, secret.as_bytes().to_vec())?;
                Ok(())
            }
            Listing::SecretThenHuge(secret) => {
                session.channel_success(channel)?;
                session.data(channel, secret.as_bytes().to_vec())?;
                for _ in 0..65 {
                    session.data(channel, vec![b'x'; 16 * 1024])?;
                }
                finish(session, channel, 0)
            }
        }
    }
}

fn finish(
    session: &mut server::Session,
    channel: russh::ChannelId,
    status: u32,
) -> Result<(), russh::Error> {
    session.exit_status_request(channel, status)?;
    session.eof(channel)?;
    session.close(channel)?;
    Ok(())
}

struct Fixture {
    port: u16,
    handle: HostHandle,
    states: sync::Receiver<(HostState, std::thread::ThreadId)>,
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
    tapped: Option<oneshot::Receiver<Arc<SshHost>>>,
}

impl Fixture {
    /// A host key trusted up front, so the connection goes straight to `Connected`.
    fn connected(exec_timeout: Duration) -> Self {
        Self::connected_with(exec_timeout, PROBE_WITH_TMUX)
    }

    fn connected_with(exec_timeout: Duration, probe: &'static str) -> Self {
        Self::connected_keeping(exec_timeout, probe, false)
    }

    /// [`Self::connected_with`], optionally keeping a handle on the server's socket so the
    /// test can hang up on the client (`hang_up`).
    fn connected_keeping(exec_timeout: Duration, probe: &'static str, keep_socket: bool) -> Self {
        let fixture = Self::start_keeping(
            HostOptions {
                exec_timeout,
                ..HostOptions::default()
            },
            probe,
            true,
            keep_socket,
        );
        assert_eq!(next(&fixture.states), HostState::Authenticating);
        assert_eq!(
            next(&fixture.states),
            HostState::Connected { address_index: 0 }
        );
        fixture
    }

    /// Connects without waiting for any state. With `trusted` false the first state is the
    /// host-key prompt.
    fn start(options: HostOptions, probe: &'static str, trusted: bool) -> Self {
        // The prompt tests hang up on the client; the others let the server end it.
        Self::start_keeping(options, probe, trusted, !trusted)
    }

    fn start_keeping(
        options: HostOptions,
        probe: &'static str,
        trusted: bool,
        keep_socket: bool,
    ) -> Self {
        Self::start_over(Arc::new(DirectTcp), options, probe, trusted, keep_socket)
    }

    /// [`Self::start_keeping`] over any transport.
    fn start_over<T: Transport>(
        transport: Arc<T>,
        options: HostOptions,
        probe: &'static str,
        trusted: bool,
        keep_socket: bool,
    ) -> Self {
        let host = ClientKey::generate_ed25519("");
        let host_openssh = host.public_key().openssh;
        let key = ClientKey::generate_ed25519("");
        let client_key = key.private_key().public_key().clone();
        let shared = Arc::new(Shared {
            streamlocal: Mutex::new(Streamlocal::Herdr),
            listing: Mutex::new(Listing::Sessions),
            probes: AtomicUsize::new(0),
            probe: Mutex::new(probe),
            herdr_list: Mutex::new(HerdrList::Fixture),
            focused: Mutex::new(Vec::new()),
            probe_hangs: AtomicBool::new(false),
            refuse_channels: AtomicBool::new(false),
            stall_auth: AtomicBool::new(false),
            closes: AtomicUsize::new(0),
            socket: Mutex::new(None),
            keep_socket,
            channel_events: Mutex::new(None),
            focus_hangs: AtomicBool::new(false),
            open_gate: Mutex::new(None),
            unix_gate: Mutex::new(None),
            shell_answers: AtomicBool::new(false),
            withheld_stage: Mutex::new(None),
        });
        let (listener, port) = runtime().block_on(async {
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let port = listener.local_addr().unwrap().port();
            (listener, port)
        });
        let config = Arc::new(server::Config {
            keys: vec![host.private_key().clone()],
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });
        let handler = Server {
            client_key,
            shared: shared.clone(),
        };
        let accepted = shared.clone();
        let task = runtime().spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let socket = socket.into_std().unwrap();
            if accepted.keep_socket {
                *accepted.socket.lock().unwrap() = Some(socket.try_clone().unwrap());
            }
            let socket = tokio::net::TcpStream::from_std(socket).unwrap();
            let session = server::run_stream(config, socket, handler).await.unwrap();
            let _ = session.await;
        });
        let (observer, states) = recorder();
        let trusted = if trusted { vec![host_openssh] } else { vec![] };
        let request = HostConnectRequest::new(
            &[("127.0.0.1", port)],
            "fixture",
            &key.to_stored(),
            &trusted,
        )
        .unwrap();
        let (tap, tapped) = oneshot::channel();
        let handle = start_tapped(transport, request, observer, options, Some(tap));
        Self {
            port,
            handle,
            states,
            shared,
            task,
            tapped: Some(tapped),
        }
    }

    /// A trusted fixture over `transport`, waited until `Connected`.
    fn connected_over<T: Transport>(transport: Arc<T>, exec_timeout: Duration) -> Self {
        let fixture = Self::start_over(
            transport,
            HostOptions {
                exec_timeout,
                ..HostOptions::default()
            },
            PROBE_WITH_TMUX,
            true,
            false,
        );
        assert_eq!(next(&fixture.states), HostState::Authenticating);
        assert_eq!(
            next(&fixture.states),
            HostState::Connected { address_index: 0 }
        );
        fixture
    }

    /// The established connection, as the host driver's own tasks use it.
    fn ssh(&mut self) -> Arc<SshHost> {
        let tapped = self.tapped.take().expect("asked once");
        runtime().block_on(tapped).expect("the host connected")
    }

    /// Cuts the TCP connection under the client (needs `keep_socket`).
    fn hang_up(&self) {
        let socket = self
            .shared
            .socket
            .lock()
            .unwrap()
            .take()
            .expect("kept socket");
        socket.shutdown(std::net::Shutdown::Both).unwrap();
    }

    fn list(&self, listing: Listing) -> Result<Vec<TmuxSession>, HostError> {
        *self.shared.listing.lock().unwrap() = listing;
        runtime().block_on(self.handle.list_tmux_sessions())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[test]
fn capabilities_are_probed_once_and_tmux_listings_sort_and_survive_no_server() {
    let fixture = Fixture::connected(Duration::from_secs(5));
    for _ in 0..2 {
        let caps = runtime().block_on(fixture.handle.capabilities()).unwrap();
        assert_eq!(caps.tmux.as_deref(), Some("/fake/tmux"));
        assert_eq!(caps.herdr, None);
        assert_eq!(caps.utf8_locale, "C.UTF-8");
    }
    let names = |sessions: Vec<TmuxSession>| -> Vec<String> {
        sessions.into_iter().map(|s| s.name).collect()
    };
    assert_eq!(
        names(fixture.list(Listing::Sessions).unwrap()),
        ["newer", "older"]
    );
    // The listing needed the cached probe, not a second one.
    assert_eq!(fixture.shared.probes.load(Ordering::SeqCst), 1);
    assert!(fixture.list(Listing::NoServer).unwrap().is_empty());
    fixture.handle.disconnect();
}

#[test]
fn exec_output_over_the_cap_a_hung_command_and_a_refused_one_fail_typed() {
    let fixture = Fixture::connected(Duration::from_millis(400));
    let Err(HostError::CommandFailed { message }) = fixture.list(Listing::Huge) else {
        panic!("expected CommandFailed")
    };
    assert!(message.contains("1 MiB"), "{message}");
    let Err(HostError::CommandFailed { message }) = fixture.list(Listing::Hang) else {
        panic!("expected CommandFailed")
    };
    assert!(message.contains("did not finish in time"), "{message}");
    let Err(HostError::CommandFailed { message }) = fixture.list(Listing::Refuse) else {
        panic!("expected CommandFailed")
    };
    assert!(message.contains("refused"), "{message}");
    // The connection survives every failed exec.
    assert_eq!(fixture.list(Listing::Sessions).unwrap().len(), 2);
    assert!(matches!(
        fixture.handle.state(),
        HostState::Connected { .. }
    ));
    fixture.handle.disconnect();
    assert_eq!(closed(&fixture.states), CloseReason::Disconnected);
}

#[test]
fn losing_the_connection_closes_the_host_with_connection_lost_and_fails_the_query() {
    let fixture = Fixture::connected(Duration::from_secs(5));
    assert_eq!(fixture.list(Listing::Lose), Err(HostError::Closed));
    let CloseReason::Failed(SessionFailure::ConnectionLost(message)) = closed(&fixture.states)
    else {
        panic!("expected ConnectionLost")
    };
    assert!(!message.is_empty());
    assert_eq!(
        runtime().block_on(fixture.handle.list_tmux_sessions()),
        Err(HostError::Closed)
    );
}

struct SessionRecorder(sync::Sender<SessionState>);

impl SessionObserver for SessionRecorder {
    fn state_changed(&self, state: &SessionState) {
        let _ = self.0.send(state.clone());
    }

    fn frame_ready(&self) {}
}

struct WatchRecorder(sync::Sender<HerdrState>);

impl HerdrObserver for WatchRecorder {
    fn state_changed(&self, state: &HerdrState) {
        let _ = self.0.send(state.clone());
    }
}

#[test]
fn a_host_without_tmux_or_herdr_reports_not_installed_everywhere_without_opening_channels() {
    let fixture = Fixture::connected_with(Duration::from_secs(5), PROBE_WITHOUT_PROGRAMS);
    let caps = runtime().block_on(fixture.handle.capabilities()).unwrap();
    assert_eq!((caps.tmux, caps.herdr), (None, None));
    assert_eq!(
        runtime().block_on(fixture.handle.list_tmux_sessions()),
        Err(HostError::NotInstalled {
            program: "tmux".into()
        })
    );
    for (target, program) in [
        (
            TerminalTarget::Tmux {
                session_name: "work".into(),
            },
            "tmux",
        ),
        (
            TerminalTarget::Herdr {
                session: None,
                pane_id: Some("w1:p1".into()),
            },
            "herdr",
        ),
    ] {
        let (tx, states) = sync::channel();
        let _session = fixture
            .handle
            .open_terminal(
                target,
                TerminalSize::new(80, 24).unwrap(),
                Arc::new(SessionRecorder(tx)),
            )
            .unwrap();
        // Closed from `Connecting`: no channel was ever needed (this server has no PTY
        // support at all, so reaching one would fail differently).
        assert_eq!(
            states.recv_timeout(Duration::from_secs(5)).unwrap(),
            SessionState::Closed(CloseReason::Failed(SessionFailure::NotInstalled {
                program: program.into()
            }))
        );
        assert!(states.recv_timeout(Duration::from_millis(100)).is_err());
    }

    let (tx, watch_states) = sync::channel();
    let watch = fixture
        .handle
        .watch_herdr(None, Arc::new(WatchRecorder(tx)))
        .unwrap();
    assert_eq!(
        watch_states.recv_timeout(Duration::from_secs(5)).unwrap(),
        HerdrState::Unavailable {
            reason: HerdrUnavailable::NotInstalled,
            message: "herdr is not installed on the host".into()
        }
    );
    watch.stop();
    assert_eq!(
        watch_states.recv_timeout(Duration::from_secs(5)).unwrap(),
        HerdrState::Closed
    );
    assert!(
        watch_states
            .recv_timeout(Duration::from_millis(100))
            .is_err()
    );
    fixture.handle.disconnect();
}

#[test]
fn a_mosh_terminal_on_a_host_without_mosh_server_is_not_installed_before_anything_runs() {
    // tmux and herdr may be there; mosh-server decides first, so no pane is focused and no
    // exec channel is opened (this server has no exec support for a mosh-server command, and
    // no PTY at all).
    for probe in [PROBE_WITH_TMUX, PROBE_WITH_HERDR, PROBE_WITHOUT_PROGRAMS] {
        let fixture = Fixture::connected_with(Duration::from_secs(5), probe);
        for target in [
            TerminalTarget::Shell,
            TerminalTarget::Tmux {
                session_name: "work".into(),
            },
            TerminalTarget::Herdr {
                session: None,
                pane_id: Some("w1:p1".into()),
            },
        ] {
            let (tx, states) = sync::channel();
            let _session = fixture
                .handle
                .open_terminal_with(
                    target,
                    TerminalTransport::Mosh,
                    TerminalSize::new(80, 24).unwrap(),
                    Arc::new(SessionRecorder(tx)),
                )
                .unwrap();
            assert_eq!(
                states.recv_timeout(Duration::from_secs(5)).unwrap(),
                SessionState::Closed(CloseReason::Failed(SessionFailure::NotInstalled {
                    program: "mosh-server".into()
                }))
            );
            assert!(states.recv_timeout(Duration::from_millis(100)).is_err());
        }
        assert!(
            fixture.shared.focused.lock().unwrap().is_empty(),
            "no pane was focused"
        );
        fixture.handle.disconnect();
    }
}

/// A terminal on a fixture host: its callback states.
fn open_shell(fixture: &Fixture) -> (crate::session::SessionHandle, sync::Receiver<SessionState>) {
    let (tx, states) = sync::channel();
    let handle = fixture
        .handle
        .open_terminal(
            TerminalTarget::Shell,
            TerminalSize::new(80, 24).unwrap(),
            Arc::new(SessionRecorder(tx)),
        )
        .unwrap();
    (handle, states)
}

fn session_closed(states: &sync::Receiver<SessionState>) -> CloseReason {
    let SessionState::Closed(reason) = states.recv_timeout(Duration::from_secs(5)).unwrap() else {
        panic!("expected the session to close")
    };
    reason
}

fn wait_for(condition: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(std::time::Instant::now() < deadline, "condition not met");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_refused_session_channel_is_shell_rejected_and_the_host_stays_connected() {
    // An sshd at `MaxSessions` refuses the channel open; the connection is healthy.
    let fixture = Fixture::connected(Duration::from_secs(5));
    fixture.shared.refuse_channels.store(true, Ordering::SeqCst);
    let (_terminal, states) = open_shell(&fixture);
    assert_eq!(
        session_closed(&states),
        CloseReason::Failed(SessionFailure::ShellRejected)
    );
    // Execs hit the same limit and say so as a command failure; then it clears.
    let Err(HostError::CommandFailed { message }) = fixture.list(Listing::Sessions) else {
        panic!("expected CommandFailed")
    };
    assert!(message.contains("refused"), "{message}");
    assert!(matches!(
        fixture.handle.state(),
        HostState::Connected { .. }
    ));
    fixture
        .shared
        .refuse_channels
        .store(false, Ordering::SeqCst);
    assert_eq!(fixture.list(Listing::Sessions).unwrap().len(), 2);
    fixture.handle.disconnect();
    assert_eq!(closed(&fixture.states), CloseReason::Disconnected);
}

#[test]
fn terminal_setup_that_the_server_never_answers_times_out_and_closes_its_channel() {
    // This server accepts the channel but never answers `pty-req`.
    let fixture = Fixture::connected(Duration::from_millis(300));
    let (_terminal, states) = open_shell(&fixture);
    let started = std::time::Instant::now();
    assert_eq!(
        session_closed(&states),
        CloseReason::Failed(SessionFailure::TimedOut)
    );
    assert!(started.elapsed() < Duration::from_secs(3));
    wait_for(|| fixture.shared.closes.load(Ordering::SeqCst) == 1);
    assert!(matches!(
        fixture.handle.state(),
        HostState::Connected { .. }
    ));
    fixture.handle.disconnect();
}

#[test]
fn a_cancelled_exec_closes_its_channel_on_the_server() {
    let mut fixture = Fixture::connected(Duration::from_secs(30));
    let ssh = fixture.ssh();
    *fixture.shared.listing.lock().unwrap() = Listing::Hang;
    let outcome = runtime().block_on(async {
        timeout(
            Duration::from_millis(150),
            ssh.exec_rendered("tmux list-sessions"),
        )
        .await
    });
    assert!(outcome.is_err(), "the server never answers");
    wait_for(|| fixture.shared.closes.load(Ordering::SeqCst) == 1);
    // The exec's own failure paths close the channel too (once each, not twice).
    *fixture.shared.listing.lock().unwrap() = Listing::Refuse;
    assert!(
        runtime()
            .block_on(ssh.exec_rendered("tmux list-sessions"))
            .is_err()
    );
    wait_for(|| fixture.shared.closes.load(Ordering::SeqCst) == 2);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(fixture.shared.closes.load(Ordering::SeqCst), 2);
    fixture.handle.disconnect();
}

fn untrusted(options: HostOptions) -> Fixture {
    Fixture::start(options, PROBE_WITH_TMUX, false)
}

fn prompt(fixture: &Fixture) -> crate::session::HostKeyPrompt {
    let HostState::AwaitingHostKey(prompt) = next(&fixture.states) else {
        panic!("expected the host-key prompt")
    };
    prompt
}

#[test]
fn the_connect_timer_is_paused_while_the_host_key_prompt_is_open() {
    let fixture = untrusted(HostOptions {
        connect_timeout: Duration::from_millis(300),
        ..HostOptions::default()
    });
    let prompt = prompt(&fixture);
    // Held for more than twice the connect timeout: still waiting for the user.
    std::thread::sleep(Duration::from_millis(700));
    assert!(matches!(
        fixture.handle.state(),
        HostState::AwaitingHostKey(_)
    ));
    fixture
        .handle
        .approve_host_key(&prompt.presented.fingerprint())
        .unwrap();
    assert_eq!(next(&fixture.states), HostState::Authenticating);
    assert_eq!(
        next(&fixture.states),
        HostState::Connected { address_index: 0 }
    );
    fixture.handle.disconnect();
}

#[test]
fn the_connect_timer_resumes_after_approval() {
    let fixture = untrusted(HostOptions {
        connect_timeout: Duration::from_millis(400),
        ..HostOptions::default()
    });
    fixture.shared.stall_auth.store(true, Ordering::SeqCst);
    let prompt = prompt(&fixture);
    std::thread::sleep(Duration::from_millis(600));
    fixture
        .handle
        .approve_host_key(&prompt.presented.fingerprint())
        .unwrap();
    let approved = std::time::Instant::now();
    assert_eq!(next(&fixture.states), HostState::Authenticating);
    assert_eq!(
        closed(&fixture.states),
        CloseReason::Failed(SessionFailure::TimedOut)
    );
    // The remaining time, not a fresh 400 ms and not forever.
    assert!(approved.elapsed() < Duration::from_millis(1500));
}

#[test]
fn the_peer_hanging_up_during_the_prompt_closes_the_host_without_waiting_for_the_user() {
    let fixture = untrusted(HostOptions::default());
    let _prompt = prompt(&fixture);
    let socket = fixture.shared.socket.lock().unwrap().take().unwrap();
    socket.shutdown(std::net::Shutdown::Both).unwrap();
    let CloseReason::Failed(SessionFailure::ConnectionLost(_)) = closed(&fixture.states) else {
        panic!("expected ConnectionLost")
    };
}

/// A transport whose `connect` is buggy.
struct Panicking;

impl Transport for Panicking {
    type Stream = tokio::io::DuplexStream;

    async fn connect(&self, _: &crate::transport::Endpoint) -> std::io::Result<Self::Stream> {
        panic!("a transport bug");
    }
}

#[test]
fn a_connection_task_that_dies_without_reporting_closes_the_host_with_internal() {
    let (observer, states) = recorder();
    // The default 20 s connect timeout: the host must not need it to notice.
    let handle = start(
        Arc::new(Panicking),
        request(&[("127.0.0.1", 1)], &[]),
        observer,
        HostOptions::default(),
    );
    let CloseReason::Failed(SessionFailure::Internal(_)) = closed(&states) else {
        panic!("expected Internal")
    };
    assert!(matches!(handle.state(), HostState::Closed(_)));
}

// ---------------------------------------------------------------------------------------
// Streamlocal refusals and herdr watches against the host's lifetime.

#[test]
fn a_streamlocal_refusal_is_io_when_the_connect_failed_and_rejected_for_every_other_reason() {
    use russh::ChannelOpenFailure as Failure;
    let mut fixture = Fixture::connected(Duration::from_secs(5));
    let host = fixture.ssh();
    let open = |behaviour| {
        *fixture.shared.streamlocal.lock().unwrap() = behaviour;
        runtime().block_on(async { host.open_unix("/run/herdr.sock").await.map(drop) })
    };
    // The socket is missing, or nothing listens on it: herdr reports NotRunning and retries.
    assert!(matches!(
        open(Streamlocal::Refuse(Failure::ConnectFailed)),
        Err(RemoteError::Io(_))
    ));
    // Forwarding is forbidden, or the server is out of resources: not a missing socket.
    for reason in [
        Failure::AdministrativelyProhibited,
        Failure::UnknownChannelType,
        Failure::ResourceShortage,
        Failure::Other {
            code: 99,
            reason: "odd".into(),
        },
    ] {
        let result = open(Streamlocal::Refuse(reason.clone()));
        assert!(
            matches!(result, Err(RemoteError::Rejected(_))),
            "{reason:?}: {:?}",
            result
        );
    }
    // The refusals left the connection healthy.
    assert!(open(Streamlocal::Herdr).is_ok());
    fixture.handle.disconnect();
}

/// How a watch test ends the host.
#[derive(Clone, Copy, Debug)]
enum End {
    Disconnect,
    /// The TCP connection is cut under the client.
    Loss,
}

/// Starts a watch on a connected fixture host, waits until `parked` accepts its state, ends
/// the host and checks the watch got exactly one `Closed`, last and before the host's.
fn watch_ends_with_the_host(probe: &'static str, parked: fn(&HerdrState) -> bool, end: End) {
    let fixture = Fixture::connected_keeping(Duration::from_secs(5), probe, true);
    let (tx, watch_states) = sync::channel();
    let watch = fixture
        .handle
        .watch_herdr(None, Arc::new(WatchRecorder(tx)))
        .unwrap();
    let state = loop {
        let state = watch_states.recv_timeout(Duration::from_secs(5)).unwrap();
        if parked(&state) {
            break state;
        }
        assert!(
            !matches!(state, HerdrState::Closed),
            "closed before parking: {state:?}"
        );
    };
    // Parked: no call is pending that could notice the host going away.
    assert!(
        watch_states
            .recv_timeout(Duration::from_millis(300))
            .is_err(),
        "still parked in {state:?}"
    );
    match end {
        End::Disconnect => {
            fixture.handle.disconnect();
            assert_eq!(closed(&fixture.states), CloseReason::Disconnected);
        }
        End::Loss => {
            fixture.hang_up();
            assert!(matches!(
                closed(&fixture.states),
                CloseReason::Failed(SessionFailure::ConnectionLost(_))
            ));
        }
    }
    // The host closes last, so the watch's `Closed` is already delivered.
    assert_eq!(
        watch_states.try_recv().ok(),
        Some(HerdrState::Closed),
        "{end:?}"
    );
    assert!(
        watch_states
            .recv_timeout(Duration::from_millis(200))
            .is_err(),
        "Closed is delivered once, last"
    );
    assert_eq!(watch.state(), HerdrState::Closed);
    // Stopping a closed watch is quiet.
    watch.stop();
    assert!(
        watch_states
            .recv_timeout(Duration::from_millis(100))
            .is_err()
    );
}

#[test]
fn a_watch_parked_in_not_installed_gets_closed_once_when_the_host_ends() {
    for end in [End::Disconnect, End::Loss] {
        watch_ends_with_the_host(
            PROBE_WITH_TMUX,
            |state| {
                matches!(
                    state,
                    HerdrState::Unavailable {
                        reason: HerdrUnavailable::NotInstalled,
                        ..
                    }
                )
            },
            end,
        );
    }
}

#[test]
fn a_live_watch_gets_closed_once_when_the_host_ends() {
    for end in [End::Disconnect, End::Loss] {
        watch_ends_with_the_host(
            PROBE_WITH_HERDR,
            |state| matches!(state, HerdrState::Live { .. }),
            end,
        );
    }
}

#[test]
fn the_capability_probe_reads_herdr_sessions_through_the_herdr_client() {
    let fixture = Fixture::connected_with(Duration::from_secs(5), PROBE_WITH_HERDR);
    let caps = runtime().block_on(fixture.handle.capabilities()).unwrap();
    assert_eq!(caps.herdr.as_deref(), Some("/fake/herdr"));
    let summary: Vec<_> = caps
        .herdr_sessions
        .iter()
        .map(|session| (session.name.as_str(), session.is_default, session.running))
        .collect();
    assert_eq!(
        summary,
        [
            ("default", true, true),
            ("work", false, true),
            ("idle", false, false)
        ]
    );
    fixture.handle.disconnect();
}

#[test]
fn mosh_server_and_tmux_resolve_from_the_program_probe_while_the_herdr_listing_hangs() {
    const PROBE_WITH_MOSH: &str = "or2:tmux:/fake/tmux\nor2:herdr:/fake/herdr\n\
        or2:mosh-server:/fake/mosh-server\nor2:locale:C.UTF-8\nor2:end\n";
    let exec_timeout = Duration::from_secs(3);
    let fixture = Fixture::connected_with(exec_timeout, PROBE_WITH_MOSH);
    *fixture.shared.herdr_list.lock().unwrap() = HerdrList::Hang;
    let handle = &fixture.handle;
    std::thread::scope(|scope| {
        // The whole probe starts first and is held up by the listing until its bound.
        let started = std::time::Instant::now();
        let whole = scope.spawn(|| runtime().block_on(handle.capabilities()));
        wait_for(|| fixture.shared.probes.load(Ordering::SeqCst) == 1);

        let asked = std::time::Instant::now();
        assert_eq!(
            runtime().block_on(handle.mosh_server()),
            Ok(Some("/fake/mosh-server".into()))
        );
        // tmux needs only the program probe's path too.
        *fixture.shared.listing.lock().unwrap() = Listing::Sessions;
        assert_eq!(
            runtime()
                .block_on(handle.list_tmux_sessions())
                .unwrap()
                .len(),
            2
        );
        assert!(
            asked.elapsed() < Duration::from_secs(1),
            "waited for herdr's listing: {:?}",
            asked.elapsed()
        );
        assert!(!whole.is_finished(), "the listing still hangs");

        let caps = whole.join().unwrap().unwrap();
        assert!(started.elapsed() >= Duration::from_secs(2));
        assert_eq!(caps.herdr.as_deref(), Some("/fake/herdr"));
        assert_eq!(caps.mosh_server.as_deref(), Some("/fake/mosh-server"));
        assert!(caps.herdr_sessions.is_empty(), "a hung herdr lists nothing");
    });
    // One program probe served the whole probe, mosh_server() and the tmux listing.
    assert_eq!(fixture.shared.probes.load(Ordering::SeqCst), 1);
    fixture.handle.disconnect();
}

#[test]
fn mosh_server_is_none_without_it_and_runs_only_the_program_probe() {
    let fixture = Fixture::connected_with(Duration::from_secs(5), PROBE_WITH_HERDR);
    *fixture.shared.herdr_list.lock().unwrap() = HerdrList::Hang;
    let asked = std::time::Instant::now();
    assert_eq!(runtime().block_on(fixture.handle.mosh_server()), Ok(None));
    assert!(asked.elapsed() < Duration::from_secs(1));
    assert_eq!(runtime().block_on(fixture.handle.mosh_server()), Ok(None));
    assert_eq!(fixture.shared.probes.load(Ordering::SeqCst), 1, "cached");
    fixture.handle.disconnect();
    wait_for(|| matches!(fixture.handle.state(), HostState::Closed(_)));
    assert_eq!(
        runtime().block_on(fixture.handle.mosh_server()),
        Err(HostError::Closed)
    );
}

// ---------------------------------------------------------------------------------------
// Secret-bearing exec output is wiped on every path (`remote::SecretBytes`).

/// Waits until the collector has wiped a buffer holding `marker` (bounded).
fn wiped(marker: &str) -> usize {
    crate::remote::wipe_log::wiped(marker.as_bytes())
}

#[test]
fn exec_output_is_wiped_when_the_exec_succeeds_fails_times_out_or_is_cancelled() {
    let mut fixture = Fixture::connected(Duration::from_millis(500));
    let ssh = fixture.ssh();
    let set = |listing| *fixture.shared.listing.lock().unwrap() = listing;
    let exec = || runtime().block_on(ssh.exec_rendered("tmux list-sessions"));

    // Success: the caller owns the output and wipes it by dropping it.
    set(Listing::SecretThenFinish("or2-secret-success"));
    let output = exec().unwrap();
    assert_eq!(&output.stdout[..], b"or2-secret-success");
    assert_eq!(wiped("or2-secret-success"), 0, "still held by the caller");
    drop(output);
    assert_eq!(wiped("or2-secret-success"), 1);

    // The command never finishes: the timeout drops the partial collection.
    set(Listing::SecretThenHang("or2-secret-timeout"));
    assert_eq!(exec().unwrap_err(), RemoteError::TimedOut);
    assert_eq!(wiped("or2-secret-timeout"), 1);

    // Output over the cap.
    set(Listing::SecretThenHuge("or2-secret-cap"));
    assert_eq!(exec().unwrap_err(), RemoteError::OutputTooLarge);
    // Every block given up on growth was wiped too, so more than one holds the marker.
    assert!(wiped("or2-secret-cap") >= 1);
}

#[test]
fn exec_output_is_wiped_when_the_connection_dies_mid_command() {
    let mut fixture = Fixture::connected_keeping(Duration::from_secs(10), PROBE_WITH_TMUX, true);
    let ssh = fixture.ssh();
    *fixture.shared.listing.lock().unwrap() = Listing::SecretThenHang("or2-secret-lost");
    let result = runtime().block_on(async {
        let cut = async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            fixture.hang_up();
        };
        tokio::join!(ssh.exec_rendered("tmux list-sessions"), cut).0
    });
    assert!(result.is_err());
    assert_eq!(wiped("or2-secret-lost"), 1);
}

#[test]
fn a_cancelled_exec_wipes_what_it_had_collected() {
    let mut fixture = Fixture::connected(Duration::from_secs(30));
    let ssh = fixture.ssh();
    *fixture.shared.listing.lock().unwrap() = Listing::SecretThenHang("or2-secret-cancelled");
    let outcome = runtime().block_on(async {
        timeout(
            Duration::from_millis(300),
            ssh.exec_rendered("tmux list-sessions"),
        )
        .await
    });
    assert!(outcome.is_err(), "the caller gave up first");
    assert_eq!(wiped("or2-secret-cancelled"), 1);
    fixture.handle.disconnect();
}

// ---------------------------------------------------------------------------------------
// A stalled outbound path.

/// Lets a test stop the client's writes to the wire (reads keep working), like a peer that
/// stopped reading until the kernel buffers and russh's queues are full.
#[derive(Default)]
struct Stall {
    stalled: AtomicBool,
    waker: Mutex<Option<std::task::Waker>>,
}

impl Stall {
    fn stall(&self) {
        self.stalled.store(true, Ordering::SeqCst);
    }

    fn release(&self) {
        self.stalled.store(false, Ordering::SeqCst);
        if let Some(waker) = self.waker.lock().unwrap().take() {
            waker.wake();
        }
    }

    /// `Pending` (with the waker kept for `release`) while stalled.
    fn poll_open(&self, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        if !self.stalled.load(Ordering::SeqCst) {
            return std::task::Poll::Ready(());
        }
        *self.waker.lock().unwrap() = Some(cx.waker().clone());
        // `release` may have run between the check and storing the waker.
        if self.stalled.load(Ordering::SeqCst) {
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(())
        }
    }
}

struct Stalling<S> {
    inner: S,
    stall: Arc<Stall>,
}

impl<S: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for Stalling<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for Stalling<S> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::task::ready!(self.stall.poll_open(cx));
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::ready!(self.stall.poll_open(cx));
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

struct StallingTcp(Arc<Stall>);

impl Transport for StallingTcp {
    type Stream = Stalling<tokio::net::TcpStream>;

    async fn connect(
        &self,
        endpoint: &crate::transport::Endpoint,
    ) -> std::io::Result<Self::Stream> {
        Ok(Stalling {
            inner: DirectTcp.connect(endpoint).await?,
            stall: Arc::clone(&self.0),
        })
    }
}

#[test]
fn a_stalled_outbound_path_cannot_hold_an_exec_or_the_probe_past_the_exec_timeout() {
    let stall = Arc::new(Stall::default());
    let mut fixture = Fixture::connected_over(
        Arc::new(StallingTcp(Arc::clone(&stall))),
        Duration::from_millis(400),
    );
    let ssh = fixture.ssh();
    // Two channels confirmed while the path still works: one for the exec under test, one to
    // fill russh's outbound queue.
    let exec_channel = runtime().block_on(ssh.start_open().wait()).unwrap();
    let filler_channel = runtime().block_on(ssh.start_open().wait()).unwrap();

    stall.stall();
    let filler = runtime().spawn(async move {
        for _ in 0..400 {
            if filler_channel.data(&[b'x'; 16 * 1024][..]).await.is_err() {
                break;
            }
        }
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(!filler.is_finished(), "the queue is full: the sender waits");

    // The exec request itself waits for queue space (the channel is already open, so only the
    // request and the EOF are at stake): the whole exchange is bounded by the deadline.
    let started = std::time::Instant::now();
    let result = runtime().block_on(ssh.run_exec(
        exec_channel,
        "tmux list-sessions",
        Instant::now() + Duration::from_millis(400),
    ));
    assert_eq!(result.unwrap_err(), RemoteError::TimedOut);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );

    // The probe holds the cache's initialization while it runs; it is bounded, so a second
    // caller waiting behind it is not stuck either, and a timed-out probe is not cached.
    let started = std::time::Instant::now();
    let (first, second) =
        runtime().block_on(async { tokio::join!(ssh.capabilities(), ssh.capabilities()) });
    assert_eq!(first.unwrap_err(), RemoteError::TimedOut);
    assert_eq!(second.unwrap_err(), RemoteError::TimedOut);
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(fixture.shared.probes.load(Ordering::SeqCst), 0);

    // The path recovers: the next caller probes afresh and succeeds.
    stall.release();
    runtime()
        .block_on(async { timeout(Duration::from_secs(5), filler).await })
        .unwrap()
        .unwrap();
    let caps = runtime().block_on(ssh.capabilities()).unwrap();
    assert_eq!(caps.tmux.as_deref(), Some("/fake/tmux"));
    assert_eq!(fixture.shared.probes.load(Ordering::SeqCst), 1);
    fixture.handle.disconnect();
}

#[test]
fn stopping_a_watch_during_the_capability_probe_closes_it_at_once_without_unavailable() {
    // The exec timeout is long: only the stop can end the probe in time.
    let fixture = Fixture::connected(Duration::from_secs(30));
    fixture.shared.probe_hangs.store(true, Ordering::SeqCst);
    let (tx, watch_states) = sync::channel();
    let watch = fixture
        .handle
        .watch_herdr(None, Arc::new(WatchRecorder(tx)))
        .unwrap();
    wait_for(|| fixture.shared.probes.load(Ordering::SeqCst) == 1);
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        watch_states.try_recv().is_err(),
        "nothing is reported while the probe runs"
    );
    let stopped = std::time::Instant::now();
    watch.stop();
    assert_eq!(
        watch_states.recv_timeout(Duration::from_secs(2)).unwrap(),
        HerdrState::Closed,
        "Closed, with no Unavailable before it"
    );
    assert!(stopped.elapsed() < Duration::from_secs(2));
    assert!(
        watch_states
            .recv_timeout(Duration::from_millis(200))
            .is_err(),
        "Closed is delivered once, last"
    );
    assert_eq!(watch.state(), HerdrState::Closed);
    // The host is unaffected: a later probe still runs (and is not cached from the stop).
    fixture.shared.probe_hangs.store(false, Ordering::SeqCst);
    let caps = runtime().block_on(fixture.handle.capabilities()).unwrap();
    assert_eq!(caps.tmux.as_deref(), Some("/fake/tmux"));
    fixture.handle.disconnect();
    assert_eq!(closed(&fixture.states), CloseReason::Disconnected);
}

#[test]
fn a_failed_session_listing_reports_the_last_list_read_not_the_one_from_connect_time() {
    const WITH_FRESH: &str = r#"{"sessions":[{"name":"default","running":true,"default":true,"socket_path":"/s/d.sock"},{"name":"fresh","running":true,"socket_path":"/s/f.sock"}]}"#;
    const ONLY_DEFAULT: &str = r#"{"sessions":[{"name":"default","running":true,"default":true,"socket_path":"/s/d.sock"}]}"#;
    let fixture = Fixture::connected_with(Duration::from_secs(5), PROBE_WITH_HERDR);
    let sessions = |fixture: &Fixture| -> Vec<String> {
        runtime()
            .block_on(fixture.handle.capabilities())
            .unwrap()
            .herdr_sessions
            .into_iter()
            .map(|session| session.name)
            .collect()
    };
    let set = |list| *fixture.shared.herdr_list.lock().unwrap() = list;
    // Connected while herdr lists the captured sessions.
    assert_eq!(sessions(&fixture), ["default", "work", "idle"]);
    // A session appears, then the next listing times out or fails: the list stays.
    set(HerdrList::Json(WITH_FRESH));
    assert_eq!(sessions(&fixture), ["default", "fresh"]);
    set(HerdrList::Fails);
    assert_eq!(sessions(&fixture), ["default", "fresh"]);
    set(HerdrList::Json("not json"));
    assert_eq!(sessions(&fixture), ["default", "fresh"]);
    // A session that went away does not come back from the connect-time list.
    set(HerdrList::Json(ONLY_DEFAULT));
    assert_eq!(sessions(&fixture), ["default"]);
    set(HerdrList::Fails);
    assert_eq!(sessions(&fixture), ["default"]);
    // The probe itself ran once.
    assert_eq!(fixture.shared.probes.load(Ordering::SeqCst), 1);
    fixture.handle.disconnect();
}

#[test]
fn the_host_reports_the_address_its_tcp_connection_reached_once_connected() {
    let fixture = Fixture::start(HostOptions::default(), PROBE_WITH_TMUX, true);
    assert_eq!(next(&fixture.states), HostState::Authenticating);
    assert_eq!(
        next(&fixture.states),
        HostState::Connected { address_index: 0 }
    );
    // Set before `Connected` is reported, so a caller that saw it can read it (mosh pins its
    // UDP traffic to this IP instead of resolving the host name again).
    assert_eq!(
        fixture.handle.peer_addr(),
        Some(std::net::SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            fixture.port
        )))
    );
    fixture.handle.disconnect();
    assert_eq!(closed(&fixture.states), CloseReason::Disconnected);
}

#[test]
fn a_host_that_never_connected_has_no_peer_address() {
    let (observer, states) = recorder();
    let handle = connect_host(request(&[("127.0.0.1", dead_port())], &[]), observer);
    let _ = closed(&states);
    assert_eq!(handle.peer_addr(), None);
}

#[test]
fn focusing_a_herdr_pane_goes_through_the_probed_herdr_and_reports_a_vanished_pane() {
    let fixture = Fixture::connected_with(Duration::from_secs(5), PROBE_WITH_HERDR);
    let focus = |session: Option<&str>, pane: &str| {
        runtime().block_on(
            fixture
                .handle
                .focus_herdr_pane(session.map(str::to_owned), pane.to_owned()),
        )
    };
    assert_eq!(focus(None, "w1:p2"), Ok(()));
    assert_eq!(focus(Some("work"), "w1:p1"), Ok(()));
    assert_eq!(
        *fixture.shared.focused.lock().unwrap(),
        ["w1:p2", "w1:p1"],
        "herdr was asked to focus exactly these panes, in order"
    );
    // A pane that has gone is its own error, not a generic failure.
    assert_eq!(focus(None, "w9:p9"), Err(HostError::PaneNotFound));
    // A session herdr lists as not running cannot be focused.
    assert!(matches!(
        focus(Some("idle"), "w1:p1"),
        Err(HostError::CommandFailed { .. })
    ));
    // Malformed names never reach the host.
    let before = fixture.shared.focused.lock().unwrap().len();
    assert_eq!(focus(Some("a b"), "w1:p1"), Err(HostError::InvalidName));
    assert_eq!(focus(None, "w1 p1"), Err(HostError::InvalidName));
    assert_eq!(fixture.shared.focused.lock().unwrap().len(), before);
    fixture.handle.disconnect();
    assert_eq!(closed(&fixture.states), CloseReason::Disconnected);
    assert_eq!(focus(None, "w1:p1"), Err(HostError::Closed));
}

#[test]
fn focusing_a_herdr_pane_needs_herdr() {
    let fixture = Fixture::connected(Duration::from_secs(5));
    assert_eq!(
        runtime().block_on(fixture.handle.focus_herdr_pane(None, "w1:p1".into())),
        Err(HostError::NotInstalled {
            program: "herdr".into()
        })
    );
    assert!(fixture.shared.focused.lock().unwrap().is_empty());
    fixture.handle.disconnect();
}

/// The session channel opened beside a pane focus that is still pending is closed, exactly once,
/// when the terminal is given up: the join holding it must not drop it unclosed.
#[test]
fn a_terminal_given_up_while_its_focus_waits_closes_the_channel_opened_beside_it() {
    let fixture = Fixture::connected_with(Duration::from_secs(5), PROBE_WITH_HERDR);
    runtime().block_on(fixture.handle.capabilities()).unwrap();
    let (tx, events) = sync::channel();
    *fixture.shared.channel_events.lock().unwrap() = Some(tx);
    fixture.shared.focus_hangs.store(true, Ordering::SeqCst);
    let (tx, states) = sync::channel();
    let terminal = fixture
        .handle
        .open_terminal_with(
            TerminalTarget::Herdr {
                session: None,
                pane_id: Some("w2:p1".into()),
            },
            TerminalTransport::Ssh,
            TerminalSize::new(80, 24).unwrap(),
            Arc::new(SessionRecorder(tx)),
        )
        .unwrap();
    // The session channel is accepted and the focus request has arrived (and is held).
    let mut session = None;
    let mut focused = false;
    while session.is_none() || !focused {
        match events.recv_timeout(Duration::from_secs(5)).unwrap() {
            ("session", id) => session = Some(id),
            ("focus", _) => focused = true,
            _ => {}
        }
    }
    let session = session.unwrap();
    terminal.disconnect();
    assert_eq!(session_closed(&states), CloseReason::Disconnected);
    let mut closes = 0;
    while let Ok(event) = events.recv_timeout(Duration::from_millis(500)) {
        if event == ("close", session) {
            closes += 1;
        }
    }
    assert_eq!(closes, 1, "the session channel must be closed exactly once");
    fixture.handle.disconnect();
}

fn late_open_confirmation_closes(within_grace: bool) {
    let fixture = Fixture::connected_with(Duration::from_secs(5), PROBE_WITH_HERDR);
    runtime().block_on(fixture.handle.capabilities()).unwrap();
    let (tx, events) = sync::channel();
    *fixture.shared.channel_events.lock().unwrap() = Some(tx);
    fixture.shared.focus_hangs.store(true, Ordering::SeqCst);
    let (release, gate) = tokio::sync::oneshot::channel();
    *fixture.shared.open_gate.lock().unwrap() = Some(gate);
    let (tx, states) = sync::channel();
    let terminal = fixture
        .handle
        .open_terminal_with(
            TerminalTarget::Herdr {
                session: None,
                pane_id: Some("w2:p1".into()),
            },
            TerminalTransport::Ssh,
            TerminalSize::new(80, 24).unwrap(),
            Arc::new(SessionRecorder(tx)),
        )
        .unwrap();
    let mut pending = None;
    let mut focus = false;
    while pending.is_none() || !focus {
        match events.recv_timeout(Duration::from_secs(5)).unwrap() {
            ("pending", id) => pending = Some(id),
            ("focus", _) => focus = true,
            _ => {}
        }
    }
    let id = pending.unwrap();
    terminal.disconnect();
    if within_grace {
        release.send(()).unwrap();
        assert_eq!(session_closed(&states), CloseReason::Disconnected);
    } else {
        // The terminal's Closed event proves its bounded cleanup has finished. Only now
        // deliver the confirmation, on a host connection that remains alive.
        assert_eq!(session_closed(&states), CloseReason::Disconnected);
        release.send(()).unwrap();
    }
    let mut closed_channel = false;
    while !closed_channel {
        match events.recv_timeout(Duration::from_millis(700)) {
            Ok(("close", closed)) if closed == id => closed_channel = true,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    // A fresh public query demonstrates that the shared host is usable after cancellation.
    runtime()
        .block_on(fixture.handle.list_tmux_sessions())
        .unwrap();
    fixture.handle.disconnect();
    assert!(
        closed_channel,
        "confirmation after cleanup left session channel {id:?} unclosed (within_grace={within_grace})"
    );
}

#[test]
fn an_open_confirmed_within_the_grace_of_a_given_up_terminal_is_closed() {
    late_open_confirmation_closes(true);
}

#[test]
fn an_open_confirmed_after_a_given_up_terminal_closed_is_still_closed() {
    late_open_confirmation_closes(false);
}

#[test]
fn an_open_the_server_never_confirms_ends_with_the_connection() {
    let mut fixture = Fixture::connected_with(Duration::from_secs(5), PROBE_WITH_HERDR);
    let ssh = fixture.ssh();
    runtime().block_on(fixture.handle.capabilities()).unwrap();
    let (tx, events) = sync::channel();
    *fixture.shared.channel_events.lock().unwrap() = Some(tx);
    fixture.shared.focus_hangs.store(true, Ordering::SeqCst);
    // The confirmation is held back for good.
    let (_release, gate) = tokio::sync::oneshot::channel::<()>();
    *fixture.shared.open_gate.lock().unwrap() = Some(gate);
    let (tx, states) = sync::channel();
    let terminal = fixture
        .handle
        .open_terminal_with(
            TerminalTarget::Herdr {
                session: None,
                pane_id: Some("w2:p1".into()),
            },
            TerminalTransport::Ssh,
            TerminalSize::new(80, 24).unwrap(),
            Arc::new(SessionRecorder(tx)),
        )
        .unwrap();
    while events.recv_timeout(Duration::from_secs(5)).unwrap().0 != "pending" {}
    terminal.disconnect();
    assert_eq!(session_closed(&states), CloseReason::Disconnected);
    // The terminal is gone, the connection owns the open that is still outstanding.
    assert_eq!(ssh.outstanding_opens(), 1);
    fixture.handle.disconnect();
    wait_for(|| ssh.outstanding_opens() == 0);
    // And nothing is started on a connection that is over.
    let mut late = ssh.start_open();
    assert!(runtime().block_on(late.wait()).is_err());
    assert_eq!(ssh.outstanding_opens(), 0);
}

#[test]
fn an_exec_whose_open_is_confirmed_after_its_deadline_has_the_channel_closed() {
    let mut fixture = Fixture::connected_with(Duration::from_millis(300), PROBE_WITH_TMUX);
    let ssh = fixture.ssh();
    let (tx, events) = sync::channel();
    *fixture.shared.channel_events.lock().unwrap() = Some(tx);
    let (release, gate) = tokio::sync::oneshot::channel();
    *fixture.shared.open_gate.lock().unwrap() = Some(gate);
    // The server holds the confirmation back past the exec's deadline.
    let error = runtime()
        .block_on(ssh.exec_rendered("tmux list-sessions"))
        .expect_err("the open outlasts the deadline");
    assert_eq!(error, RemoteError::TimedOut);
    let id = loop {
        if let ("pending", id) = events.recv_timeout(Duration::from_secs(5)).unwrap() {
            break id;
        }
    };
    release.send(()).unwrap();
    let mut closed = false;
    while let Ok((kind, channel)) = events.recv_timeout(Duration::from_secs(2)) {
        if kind == "close" && channel == id {
            closed = true;
            break;
        }
    }
    assert!(
        closed,
        "the late-confirmed exec channel {id:?} was never closed"
    );
    // The connection is healthy and the open task is gone.
    wait_for(|| ssh.outstanding_opens() == 0);
    assert!(
        runtime()
            .block_on(ssh.exec_rendered("tmux list-sessions"))
            .is_ok()
    );
    fixture.handle.disconnect();
}

#[test]
fn a_streamlocal_open_confirmed_after_its_deadline_has_the_channel_closed() {
    let mut fixture = Fixture::connected_with(Duration::from_millis(300), PROBE_WITH_HERDR);
    let ssh = fixture.ssh();
    let (tx, events) = sync::channel();
    *fixture.shared.channel_events.lock().unwrap() = Some(tx);
    let (release, gate) = tokio::sync::oneshot::channel();
    *fixture.shared.unix_gate.lock().unwrap() = Some(gate);
    // The server holds the confirmation back past the open's deadline.
    let error = runtime()
        .block_on(ssh.open_unix("/run/herdr.sock"))
        .err()
        .expect("the open outlasts the deadline");
    assert_eq!(error, RemoteError::TimedOut);
    let id = loop {
        if let ("pending", id) = events.recv_timeout(Duration::from_secs(5)).unwrap() {
            break id;
        }
    };
    release.send(()).unwrap();
    let mut closed = false;
    while let Ok((kind, channel)) = events.recv_timeout(Duration::from_secs(2)) {
        if kind == "close" && channel == id {
            closed = true;
            break;
        }
    }
    assert!(
        closed,
        "the late-confirmed streamlocal channel {id:?} was never closed"
    );
    // The connection is healthy and the open task is gone.
    wait_for(|| ssh.outstanding_opens() == 0);
    // The stream must drop inside the runtime.
    assert!(runtime().block_on(async { ssh.open_unix("/run/herdr.sock").await.is_ok() }));
    fixture.handle.disconnect();
}

// Complete the producer without consuming its oneshot answer. Joining is an event wait,
// so this deterministically reaches the delivery/cancellation race without sleeps or polling.
fn dropped_or_consumed_delivered_open(kind: OpenKind, consume: bool) {
    let mut fixture = Fixture::connected(Duration::from_secs(5));
    let ssh = fixture.ssh();
    let (tx, events) = sync::channel();
    *fixture.shared.channel_events.lock().unwrap() = Some(tx);
    let mut pending = ssh.start_open_of(kind);
    let mut tasks = ssh.opens.lock().unwrap().take().unwrap();
    runtime().block_on(async {
        timeout(Duration::from_secs(5), tasks.join_next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    });
    *ssh.opens.lock().unwrap() = Some(tasks);
    if consume {
        runtime().block_on(async {
            pending.wait().await.unwrap().close().await.unwrap();
        });
    }
    drop(pending);
    let until = std::time::Instant::now() + Duration::from_millis(700);
    let closed_channel = loop {
        match events.recv_timeout(until.saturating_duration_since(std::time::Instant::now())) {
            Ok(("close", _)) => break true,
            Ok(_) => {}
            Err(_) => break false,
        }
    };
    // Still a live shared connection, so disconnecting cannot hide the leak.
    runtime()
        .block_on(ssh.exec_rendered("tmux list-sessions"))
        .unwrap();
    fixture.handle.disconnect();
    assert_eq!(closed(&fixture.states), CloseReason::Disconnected);
    assert!(
        closed_channel,
        "a delivered but unconsumed confirmation was dropped without closing"
    );
}

#[test]
fn a_delivered_session_open_dropped_unconsumed_is_closed() {
    dropped_or_consumed_delivered_open(OpenKind::Session, false);
}

#[test]
fn a_delivered_streamlocal_open_dropped_unconsumed_is_closed() {
    dropped_or_consumed_delivered_open(OpenKind::Streamlocal("/run/herdr.sock".into()), false);
}

#[test]
fn a_delivered_session_open_taken_and_closed_by_the_caller_is_closed() {
    dropped_or_consumed_delivered_open(OpenKind::Session, true);
}

#[test]
fn a_delivered_streamlocal_open_taken_and_closed_by_the_caller_is_closed() {
    dropped_or_consumed_delivered_open(OpenKind::Streamlocal("/run/herdr.sock".into()), true);
}

#[test]
fn pending_opens_end_on_host_close_without_retaining_the_host() {
    for unix in [false, true] {
        let mut fixture = Fixture::connected(Duration::from_secs(5));
        let ssh = fixture.ssh();
        let weak = Arc::downgrade(&ssh);
        let (tx, events) = sync::channel();
        *fixture.shared.channel_events.lock().unwrap() = Some(tx);
        let (_release, gate) = oneshot::channel::<()>();
        let mut pending = if unix {
            *fixture.shared.unix_gate.lock().unwrap() = Some(gate);
            ssh.start_open_of(OpenKind::Streamlocal("/run/herdr.sock".into()))
        } else {
            *fixture.shared.open_gate.lock().unwrap() = Some(gate);
            ssh.start_open()
        };
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)).unwrap().0,
            "pending"
        );
        fixture.handle.disconnect();
        assert_eq!(closed(&fixture.states), CloseReason::Disconnected);
        assert!(runtime().block_on(async {
            timeout(Duration::from_secs(2), pending.wait())
                .await
                .unwrap()
                .is_err()
        }));
        assert_eq!(ssh.outstanding_opens(), 0);
        assert!(ssh.opens.lock().unwrap().is_none());
        assert!(ssh.me.upgrade().is_some());
        assert_eq!(
            runtime().block_on(ssh.exec_rendered("tmux list-sessions")),
            Err(RemoteError::Closed)
        );
        assert!(matches!(
            runtime().block_on(ssh.open_unix("/run/herdr.sock")),
            Err(RemoteError::Closed)
        ));
        drop(pending);
        drop(ssh);
        drop(fixture);
        // The aborted connection task releases its reference a moment after `Closed`.
        wait_for(|| weak.upgrade().is_none());
    }
}

fn cancel_public_open_after_delivery(unix: bool) {
    let mut fixture = Fixture::connected(Duration::from_secs(5));
    let ssh = fixture.ssh();
    let (tx, events) = sync::channel();
    *fixture.shared.channel_events.lock().unwrap() = Some(tx);
    let (release, gate) = oneshot::channel();
    if unix {
        *fixture.shared.unix_gate.lock().unwrap() = Some(gate);
    } else {
        *fixture.shared.open_gate.lock().unwrap() = Some(gate);
    }
    runtime().block_on(async {
        let mut operation = Box::pin(async {
            if unix {
                ssh.open_unix("/run/herdr.sock").await.map(|_| ())
            } else {
                ssh.exec_rendered("tmux list-sessions").await.map(|_| ())
            }
        });
        // Drive the public caller once, to start its channel open; the gate prevents an answer.
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(operation.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)).unwrap().0,
            "pending"
        );
        let mut tasks = ssh.opens.lock().unwrap().take().unwrap();
        release.send(()).unwrap();
        timeout(Duration::from_secs(5), tasks.join_next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        *ssh.opens.lock().unwrap() = Some(tasks);
        // The producer delivered successfully, but the caller has not resumed to receive it.
        drop(operation);
    });
    let until = std::time::Instant::now() + Duration::from_millis(700);
    let closed_channel = loop {
        match events.recv_timeout(until.saturating_duration_since(std::time::Instant::now())) {
            Ok(("close", _)) => break true,
            Ok(_) => {}
            Err(_) => break false,
        }
    };
    runtime()
        .block_on(ssh.exec_rendered("tmux list-sessions"))
        .unwrap();
    fixture.handle.disconnect();
    assert_eq!(closed(&fixture.states), CloseReason::Disconnected);
    assert!(
        closed_channel,
        "cancelled public open leaked delivered channel (unix={unix})"
    );
}

#[test]
fn an_exec_cancelled_after_its_open_was_delivered_has_the_channel_closed() {
    cancel_public_open_after_delivery(false);
}

#[test]
fn a_streamlocal_open_cancelled_after_delivery_has_the_channel_closed() {
    cancel_public_open_after_delivery(true);
}

/// Runs `terminal_session::channel_task` on its own and aborts it once its channel is open and
/// either running (a shell) or still waiting for the herdr pane focus, as the host does with a
/// terminal task that outlasts its close grace; then counts the closes the server sees for the
/// session channel.
fn abort_a_terminal_task_and_count_closes(target: TerminalTarget, focus_waits: bool) {
    let mut fixture = Fixture::connected_with(Duration::from_secs(5), PROBE_WITH_HERDR);
    let ssh = fixture.ssh();
    fixture.shared.shell_answers.store(true, Ordering::SeqCst);
    runtime().block_on(fixture.handle.capabilities()).unwrap();
    let (tx, events) = sync::channel();
    *fixture.shared.channel_events.lock().unwrap() = Some(tx);
    fixture
        .shared
        .focus_hangs
        .store(focus_waits, Ordering::SeqCst);
    let (session_events, mut incoming) = mpsc::channel(32);
    let (_writes, outgoing) = mpsc::unbounded_channel();
    let (_size, latest_size) = watch::channel(TerminalSize::new(80, 24).unwrap());
    let (_stop_signal, stop) = watch::channel(false);
    let task = runtime().spawn(async move {
        super::terminal_session::channel_task(
            ssh,
            target,
            None,
            &session_events,
            outgoing,
            latest_size,
            stop,
        )
        .await
    });
    let mut session = None;
    let mut focus = false;
    while session.is_none() || (focus_waits && !focus) {
        match events.recv_timeout(Duration::from_secs(5)).unwrap() {
            ("session", id) => session = Some(id),
            ("focus", _) => focus = true,
            _ => {}
        }
    }
    if !focus_waits {
        // A shell: wait until it runs (`Connected`), so the abort lands in the pump.
        runtime().block_on(async {
            while !matches!(
                incoming.recv().await,
                Some(crate::ssh::pump::Event::Connected)
            ) {}
        });
    }
    let session = session.unwrap();
    task.abort();
    assert!(runtime().block_on(task).unwrap_err().is_cancelled());
    let mut closes = 0;
    while let Ok(event) = events.recv_timeout(Duration::from_millis(700)) {
        if event == ("close", session) {
            closes += 1;
        }
    }
    assert_eq!(
        closes, 1,
        "an aborted terminal task must close its channel once"
    );
    fixture.handle.disconnect();
}

#[test]
fn an_aborted_terminal_task_closes_a_running_channel_exactly_once() {
    abort_a_terminal_task_and_count_closes(TerminalTarget::Shell, false);
}

#[test]
fn an_aborted_terminal_task_closes_a_channel_still_waiting_for_its_focus_exactly_once() {
    abort_a_terminal_task_and_count_closes(
        TerminalTarget::Herdr {
            session: None,
            pane_id: Some("w2:p1".into()),
        },
        true,
    );
}

// A diagnostic-only gate freezes russh's shared loop after delivering the open confirmation.
// The ten-slot command queue is filled with keepalives: no sleep or status polling.
fn close_with_a_full_command_queue(mode: &str) {
    let mut fixture = Fixture::connected(Duration::from_secs(5));
    let ssh = fixture.ssh();
    let (tx, events) = sync::channel();
    *fixture.shared.channel_events.lock().unwrap() = Some(tx);
    let (entered, reader_entered) = oneshot::channel();
    let (release, reader_release) = oneshot::channel();
    *ssh.reader_gate.lock().unwrap() = Some((entered, reader_release));
    let channel = runtime().block_on(async {
        let mut pending = ssh.start_open();
        let channel = timeout(Duration::from_secs(5), pending.wait())
            .await
            .unwrap()
            .unwrap();
        timeout(Duration::from_secs(5), reader_entered)
            .await
            .unwrap()
            .unwrap();
        // russh::client::connect_stream creates channel(10). The loop is at the gate and
        // cannot consume these messages; each enqueue is bounded in case the setup is wrong.
        for _ in 0..10 {
            timeout(Duration::from_secs(1), ssh.handle.send_keepalive(false))
                .await
                .unwrap()
                .unwrap();
        }
        channel
    });
    let id = events.recv_timeout(Duration::from_secs(5)).unwrap().1;
    runtime().block_on(async {
        match mode {
            "guard_cancel" => {
                let mut closing = Box::pin(channel.close());
                std::future::poll_fn(|cx| {
                    assert!(std::future::Future::poll(closing.as_mut(), cx).is_pending());
                    std::task::Poll::Ready(())
                })
                .await;
                drop(closing);
            }
            "guard_timeout" => {
                assert!(timeout(CHANNEL_CLOSE_GRACE, channel.close()).await.is_err());
            }
            "guard_drop_timeout" => {
                drop(channel);
                // Past the old 250 ms deadline the close task still owns the channel.
                tokio::time::sleep(CHANNEL_CLOSE_GRACE * 2).await;
                assert_eq!(ssh.outstanding_opens(), 1);
            }
            "pump_cancel" | "pump_timeout" => {
                let (tx, _rx) = mpsc::channel(32);
                let (_writes, mut outgoing) = mpsc::unbounded_channel();
                let mut pump = Box::pin(crate::ssh::pump::pump_channel(
                    channel,
                    &tx,
                    &mut outgoing,
                    std::future::ready(()),
                ));
                if mode == "pump_cancel" {
                    std::future::poll_fn(|cx| {
                        assert!(std::future::Future::poll(pump.as_mut(), cx).is_pending());
                        std::task::Poll::Ready(())
                    })
                    .await;
                    drop(pump);
                } else {
                    assert_eq!(
                        timeout(Duration::from_secs(2), pump)
                            .await
                            .unwrap()
                            .unwrap(),
                        CloseReason::Disconnected
                    );
                }
            }
            "exec_cancel" => {
                let mut exec = Box::pin(ExecChannel(Some(channel)).close());
                std::future::poll_fn(|cx| {
                    assert!(std::future::Future::poll(exec.as_mut(), cx).is_pending());
                    std::task::Poll::Ready(())
                })
                .await;
                drop(exec);
            }
            "guard_cancel_control" => {
                let mut closing = Box::pin(channel.close());
                std::future::poll_fn(|cx| {
                    assert!(std::future::Future::poll(closing.as_mut(), cx).is_pending());
                    std::task::Poll::Ready(())
                })
                .await;
                release.send(()).unwrap();
                timeout(Duration::from_secs(2), closing)
                    .await
                    .unwrap()
                    .unwrap();
                return;
            }
            _ => unreachable!(),
        }
        release.send(()).unwrap();
    });
    // A completed fresh exec is a barrier: the reader recovered and processed the full queue.
    runtime()
        .block_on(ssh.exec_rendered("tmux list-sessions"))
        .unwrap();
    let until = std::time::Instant::now() + Duration::from_millis(700);
    let mut closes = 0;
    while let Ok(event) =
        events.recv_timeout(until.saturating_duration_since(std::time::Instant::now()))
    {
        if event == ("close", id) {
            closes += 1;
        }
    }
    fixture.handle.disconnect();
    assert_eq!(closed(&fixture.states), CloseReason::Disconnected);
    assert_eq!(
        closes, 1,
        "{mode}: close lost while the shared SSH connection recovered"
    );
}

#[test]
fn a_cancelled_guard_close_with_a_full_queue_still_closes_the_channel() {
    close_with_a_full_command_queue("guard_cancel");
}
#[test]
fn a_timed_out_guard_close_with_a_full_queue_still_closes_the_channel() {
    close_with_a_full_command_queue("guard_timeout");
}
#[test]
fn a_dropped_guard_with_a_full_queue_still_closes_the_channel() {
    close_with_a_full_command_queue("guard_drop_timeout");
}
#[test]
fn a_cancelled_pump_with_a_full_queue_still_closes_the_channel() {
    close_with_a_full_command_queue("pump_cancel");
}
#[test]
fn a_pump_stop_with_a_full_queue_still_closes_the_channel() {
    close_with_a_full_command_queue("pump_timeout");
}
#[test]
fn a_cancelled_exec_close_with_a_full_queue_still_closes_the_channel() {
    close_with_a_full_command_queue("exec_cancel");
}
#[test]
fn a_guard_close_the_queue_lets_finish_closes_the_channel_once() {
    close_with_a_full_command_queue("guard_cancel_control");
}

#[test]
fn a_guard_dropped_after_the_host_is_gone_does_nothing() {
    for unix in [false, true] {
        let mut fixture = Fixture::connected(Duration::from_secs(5));
        let ssh = fixture.ssh();
        let weak = Arc::downgrade(&ssh);
        let guard = runtime().block_on(async {
            ssh.start_open_of(if unix {
                OpenKind::Streamlocal("/run/herdr.sock".into())
            } else {
                OpenKind::Session
            })
            .wait()
            .await
            .unwrap()
        });
        fixture.handle.disconnect();
        assert_eq!(closed(&fixture.states), CloseReason::Disconnected);
        drop(ssh);
        drop(fixture);
        wait_for(|| weak.upgrade().is_none());
        drop(guard); // Weak cannot upgrade: no panic, no retained host.
    }
}

#[test]
fn a_terminal_disconnect_with_a_full_queue_still_closes_its_channel_once_the_queue_drains() {
    let mut fixture = Fixture::connected(Duration::from_secs(5));
    let ssh = fixture.ssh();
    fixture.shared.shell_answers.store(true, Ordering::SeqCst);
    let (tx, events) = sync::channel();
    *fixture.shared.channel_events.lock().unwrap() = Some(tx);
    let (tx, states) = sync::channel();
    let terminal = fixture
        .handle
        .open_terminal_with(
            TerminalTarget::Shell,
            TerminalTransport::Ssh,
            TerminalSize::new(80, 24).unwrap(),
            Arc::new(SessionRecorder(tx)),
        )
        .unwrap();
    while states.recv_timeout(Duration::from_secs(5)).unwrap() != SessionState::Connected {}
    let id = loop {
        if let ("session", id) = events.recv_timeout(Duration::from_secs(5)).unwrap() {
            break id;
        }
    };
    let (entered, reader_entered) = oneshot::channel();
    let (release, reader_release) = oneshot::channel();
    *ssh.reader_gate.lock().unwrap() = Some((entered, reader_release));
    let blocker = runtime().block_on(async {
        let guard = ssh
            .start_open_of(OpenKind::Streamlocal("/run/herdr.sock".into()))
            .wait()
            .await
            .unwrap();
        timeout(Duration::from_secs(5), reader_entered)
            .await
            .unwrap()
            .unwrap();
        for _ in 0..10 {
            ssh.handle.send_keepalive(false).await.unwrap();
        }
        guard
    });
    terminal.disconnect();
    assert_eq!(session_closed(&states), CloseReason::Disconnected);
    release.send(()).unwrap();
    drop(blocker);
    runtime()
        .block_on(ssh.exec_rendered("tmux list-sessions"))
        .unwrap();
    let until = std::time::Instant::now() + Duration::from_millis(700);
    let mut closes = 0;
    while let Ok(event) =
        events.recv_timeout(until.saturating_duration_since(std::time::Instant::now()))
    {
        if event == ("close", id) {
            closes += 1;
        }
    }
    fixture.handle.disconnect();
    assert_eq!(closed(&fixture.states), CloseReason::Disconnected);
    assert_eq!(
        closes, 1,
        "public Disconnect reported Closed but left its terminal channel open on the recovered host"
    );
}

#[test]
fn a_terminal_task_aborted_while_pty_or_shell_reply_is_pending_closes_its_channel_once() {
    for stage in ["pty", "shell"] {
        let mut fixture = Fixture::connected(Duration::from_secs(5));
        let ssh = fixture.ssh();
        fixture.shared.shell_answers.store(true, Ordering::SeqCst);
        *fixture.shared.withheld_stage.lock().unwrap() = Some(stage);
        let (tx, events) = sync::channel();
        *fixture.shared.channel_events.lock().unwrap() = Some(tx);
        let (sender, _incoming) = mpsc::channel(32);
        let (_writes, outgoing) = mpsc::unbounded_channel();
        let (_size, latest_size) = watch::channel(TerminalSize::new(80, 24).unwrap());
        let (_stop, stop) = watch::channel(false);
        let task = runtime().spawn(async move {
            super::terminal_session::channel_task(
                ssh,
                TerminalTarget::Shell,
                None,
                &sender,
                outgoing,
                latest_size,
                stop,
            )
            .await
        });
        let id = loop {
            let (kind, id) = events.recv_timeout(Duration::from_secs(5)).unwrap();
            if kind == stage {
                break id;
            }
        };
        task.abort();
        assert!(runtime().block_on(task).unwrap_err().is_cancelled());
        let until = std::time::Instant::now() + Duration::from_millis(700);
        let mut closes = 0;
        while let Ok(event) =
            events.recv_timeout(until.saturating_duration_since(std::time::Instant::now()))
        {
            if event == ("close", id) {
                closes += 1;
            }
        }
        assert_eq!(
            closes, 1,
            "abort while {stage} reply pending must close once"
        );
        runtime()
            .block_on(fixture.handle.list_tmux_sessions())
            .unwrap();
        fixture.handle.disconnect();
        assert_eq!(closed(&fixture.states), CloseReason::Disconnected);
    }
}

#[test]
fn a_running_pump_cancelled_under_a_full_queue_still_closes_the_channel() {
    let mut fixture = Fixture::connected(Duration::from_secs(5));
    let ssh = fixture.ssh();
    let (tx, events) = sync::channel();
    *fixture.shared.channel_events.lock().unwrap() = Some(tx);
    let (entered, reader_entered) = oneshot::channel();
    let (release, reader_release) = oneshot::channel();
    *ssh.reader_gate.lock().unwrap() = Some((entered, reader_release));
    runtime().block_on(async {
        let channel = ssh.start_open().wait().await.unwrap();
        timeout(Duration::from_secs(5), reader_entered)
            .await
            .unwrap()
            .unwrap();
        for _ in 0..10 {
            timeout(Duration::from_secs(1), ssh.handle.send_keepalive(false))
                .await
                .unwrap()
                .unwrap();
        }
        let (tx, _rx) = mpsc::channel(32);
        let (_writes, mut outgoing) = mpsc::unbounded_channel();
        let mut pump = Box::pin(crate::ssh::pump::pump_channel(
            channel,
            &tx,
            &mut outgoing,
            std::future::pending::<()>(),
        ));
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(pump.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        drop(pump);
        release.send(()).unwrap();
    });
    let id = events.recv_timeout(Duration::from_secs(5)).unwrap().1;
    runtime()
        .block_on(ssh.exec_rendered("tmux list-sessions"))
        .unwrap();
    let until = std::time::Instant::now() + Duration::from_millis(700);
    let mut closes = 0;
    while let Ok(event) =
        events.recv_timeout(until.saturating_duration_since(std::time::Instant::now()))
    {
        if event == ("close", id) {
            closes += 1;
        }
    }
    fixture.handle.disconnect();
    assert_eq!(closed(&fixture.states), CloseReason::Disconnected);
    assert_eq!(closes, 1);
}

#[test]
fn a_pump_whose_local_input_ended_closes_its_channel_on_the_healthy_connection() {
    let mut fixture = Fixture::connected(Duration::from_secs(5));
    let ssh = fixture.ssh();
    fixture.shared.shell_answers.store(true, Ordering::SeqCst);
    let (tx, events) = sync::channel();
    *fixture.shared.channel_events.lock().unwrap() = Some(tx);
    let (sender, mut incoming) = mpsc::channel(32);
    let (writes, outgoing) = mpsc::unbounded_channel();
    let (_size, size) = watch::channel(TerminalSize::new(80, 24).unwrap());
    let (_stop, stop) = watch::channel(false);
    let host = ssh.clone();
    let task = runtime().spawn(async move {
        super::terminal_session::channel_task(
            host,
            TerminalTarget::Shell,
            None,
            &sender,
            outgoing,
            size,
            stop,
        )
        .await
    });
    runtime().block_on(async {
        loop {
            if matches!(
                timeout(Duration::from_secs(5), incoming.recv())
                    .await
                    .unwrap(),
                Some(crate::ssh::pump::Event::Connected)
            ) {
                break;
            }
        }
    });
    let id = loop {
        if let ("session", id) = events.recv_timeout(Duration::from_secs(5)).unwrap() {
            break id;
        }
    };
    drop(writes); // Local terminal producer ended; no server Close or connection loss.
    assert!(matches!(
        runtime().block_on(task).unwrap(),
        CloseReason::Failed(SessionFailure::ConnectionLost(_))
    ));
    runtime()
        .block_on(ssh.exec_rendered("tmux list-sessions"))
        .unwrap();
    assert!(!ssh.is_closed());
    let until = std::time::Instant::now() + Duration::from_millis(700);
    let mut closes = 0;
    while let Ok(event) =
        events.recv_timeout(until.saturating_duration_since(std::time::Instant::now()))
    {
        if event == ("close", id) {
            closes += 1;
        }
    }
    fixture.handle.disconnect();
    assert_eq!(closed(&fixture.states), CloseReason::Disconnected);
    assert_eq!(closes, 1, "local input EOF must not abandon a live channel");
}
