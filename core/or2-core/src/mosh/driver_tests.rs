//! Driver tests against a fake mosh server: just enough of the server side of the protocol to
//! drive the client, so the lifecycle is covered without a `mosh-server` binary. The live test
//! against the real one is `tests/mosh_live.rs`.

use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Instant as StdInstant;

use prost::Message as _;
use tokio::net::UdpSocket;

use crate::frame::{CellWidth, Frame};
use crate::input::{Key, KeyInput, Modifiers};
use crate::term::TerminalSize;

use super::super::bootstrap::MoshKey;
use super::super::ssp::crypto::{Direction, Session as CryptoSession};
use super::super::ssp::key::Base64Key;
use super::super::ssp::packet::{Packet, PacketState};
use super::super::ssp::statesync::{HostBytes, HostInstruction, HostMessage, UserMessage};
use super::super::ssp::transport::{
    Fragment, FragmentAssembly, Fragmenter, Instruction, SHUTDOWN_NUM,
};
use super::*;

const KEY: &str = "zr0jtuYVKJnfJHP/XOZs7A";
const LOCALHOST: IpAddr = IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
const OTHER_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAA";

/// The server side of the protocol: it decrypts what the client sends and answers with host
/// diffs that append bytes to the screen.
struct FakeServer {
    socket: UdpSocket,
    crypto: CryptoSession,
    packets: PacketState,
    assembly: FragmentAssembly,
    fragmenter: Fragmenter,
    started: StdInstant,
    /// The newest state we sent; the client holds it.
    sent: u64,
    /// The newest client state we have seen, which every reply acknowledges.
    client_num: u64,
    /// Where the client's newest authenticated datagram came from.
    client: Option<SocketAddr>,
}

/// What the client said in one instruction.
struct Heard {
    new_num: u64,
    keys: Vec<u8>,
    resizes: Vec<(i32, i32)>,
    from: SocketAddr,
}

impl FakeServer {
    async fn new(key: &str) -> Self {
        Self {
            socket: UdpSocket::bind("127.0.0.1:0").await.unwrap(),
            crypto: CryptoSession::new(&Base64Key::from_printable(key).unwrap()),
            packets: PacketState::default(),
            assembly: FragmentAssembly::default(),
            fragmenter: Fragmenter::default(),
            started: StdInstant::now(),
            sent: 0,
            client_num: 0,
            client: None,
        }
    }

    fn port(&self) -> u16 {
        self.socket.local_addr().unwrap().port()
    }

    fn now(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// The next instruction from the client, or `None` after `wait`.
    async fn hear(&mut self, wait: Duration) -> Option<Heard> {
        let deadline = Instant::now() + wait;
        let mut buf = [0u8; 2048];
        loop {
            let (n, from) = tokio::time::timeout_at(deadline, self.socket.recv_from(&mut buf))
                .await
                .ok()?
                .unwrap();
            let Ok(incoming) = self.crypto.decrypt(&buf[..n]) else {
                continue;
            };
            assert_eq!(incoming.direction, Direction::ToServer);
            let packet = Packet::from_plaintext(&incoming.plaintext).unwrap();
            let now = self.now();
            self.packets.accept(now, incoming.seq, &packet, false);
            self.client = Some(from);
            if packet.payload.is_empty() {
                continue;
            }
            let frag = Fragment::from_bytes(&packet.payload).unwrap();
            if !self.assembly.add(frag) {
                continue;
            }
            let inst = self.assembly.take().unwrap().unwrap();
            let new_num = inst.new_num.unwrap();
            if new_num != SHUTDOWN_NUM {
                self.client_num = self.client_num.max(new_num);
            }
            let message = UserMessage::decode(inst.diff.unwrap_or_default().as_slice()).unwrap();
            let mut keys = Vec::new();
            let mut resizes = Vec::new();
            for instruction in message.instruction {
                if let Some(keystroke) = instruction.keystroke {
                    keys.extend(keystroke.keys.unwrap_or_default());
                }
                if let Some(resize) = instruction.resize {
                    resizes.push((resize.width.unwrap(), resize.height.unwrap()));
                }
            }
            return Some(Heard {
                new_num,
                keys,
                resizes,
                from,
            });
        }
    }

    /// Hears until `done` is satisfied by everything heard so far.
    async fn hear_until(&mut self, done: impl Fn(&[Heard]) -> bool) -> Vec<Heard> {
        let mut heard = Vec::new();
        while !done(&heard) {
            heard.push(
                self.hear(Duration::from_secs(5))
                    .await
                    .expect("the client stopped talking"),
            );
        }
        heard
    }

    /// Sends a host diff that appends `bytes` to the screen, to wherever the client last spoke
    /// from.
    async fn say(&mut self, bytes: &[u8]) {
        let diff = HostMessage {
            instruction: vec![HostInstruction {
                hostbytes: Some(HostBytes {
                    hoststring: Some(bytes.to_vec()),
                }),
                ..Default::default()
            }],
        }
        .encode_to_vec();
        let inst = Instruction::new(self.sent, self.sent + 1, self.client_num, 0, diff, vec![]);
        self.sent += 1;
        self.send(&inst).await;
    }

    async fn say_goodbye(&mut self) {
        let inst = Instruction::new(self.sent, SHUTDOWN_NUM, self.client_num, 0, vec![], vec![]);
        self.send(&inst).await;
    }

    async fn send(&mut self, inst: &Instruction) {
        let to = self.client.expect("the client has not spoken yet");
        let now = self.now();
        for fragment in self.fragmenter.fragment(inst, 1200).unwrap() {
            let outgoing = self.packets.new_packet(now, fragment.to_bytes());
            let datagram = self
                .crypto
                .encrypt(
                    outgoing.seq,
                    Direction::ToClient,
                    &outgoing.packet.to_plaintext(),
                )
                .unwrap();
            self.socket.send_to(&datagram, to).await.unwrap();
        }
    }
}

struct Recorder(Mutex<mpsc::Sender<SessionState>>);

impl SessionObserver for Recorder {
    fn state_changed(&self, state: &SessionState) {
        let _ = self.0.lock().unwrap().send(state.clone());
    }

    fn frame_ready(&self) {}
}

fn params(port: u16, key: &str, columns: u16, rows: u16) -> MoshParams {
    MoshParams {
        port,
        key: MoshKey::parse(key).unwrap(),
        size: TerminalSize::new(columns, rows).unwrap(),
        server_pid: None,
    }
}

fn start_fake(
    port: u16,
    key: &str,
    connect_timeout: Duration,
) -> (SessionHandle, LinkControl, mpsc::Receiver<SessionState>) {
    let (sender, states) = mpsc::channel();
    let (handle, control) = spawn(
        DirectUdp,
        params(port, key, 20, 5),
        LOCALHOST,
        Arc::new(Recorder(Mutex::new(sender))),
        None,
        connect_timeout,
    )
    .unwrap();
    (handle, control, states)
}

/// The renderer's view: frames merged the way Kotlin merges them.
#[derive(Default)]
struct Grid {
    rows: Vec<String>,
    size: Option<TerminalSize>,
}

impl Grid {
    fn merge(&mut self, frame: &Frame) {
        if frame.is_full() {
            self.rows = vec![String::new(); usize::from(frame.size().rows())];
            self.size = Some(frame.size());
        } else {
            assert_eq!(
                self.size,
                Some(frame.size()),
                "a delta matches the held grid"
            );
        }
        for row in frame.rows() {
            self.rows[usize::from(row.index())] = row
                .cells()
                .iter()
                .filter(|cell| cell.width != CellWidth::SpacerTail)
                .map(|cell| cell.text.as_str())
                .collect::<String>()
                .trim_end()
                .to_owned();
        }
    }

    /// Pulls frames until `done` holds for the merged screen.
    async fn wait(&mut self, handle: &SessionHandle, what: &str, done: impl Fn(&Grid) -> bool) {
        let deadline = StdInstant::now() + Duration::from_secs(5);
        loop {
            if let Some(taken) = handle.take_frame() {
                self.merge(&taken.frame);
            }
            if done(self) {
                return;
            }
            assert!(
                StdInstant::now() < deadline,
                "never saw {what}; screen: {:?}",
                self.rows
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    async fn wait_for(&mut self, handle: &SessionHandle, expected: &str) {
        self.wait(handle, expected, |grid| {
            grid.rows.join("\n").contains(expected)
        })
        .await;
    }
}

async fn state(states: &mpsc::Receiver<SessionState>) -> SessionState {
    // The receiver is std; poll it without blocking the runtime the fake server runs on.
    let deadline = StdInstant::now() + Duration::from_secs(5);
    loop {
        match states.try_recv() {
            Ok(state) => return state,
            Err(mpsc::TryRecvError::Empty) if StdInstant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Err(error) => panic!("no state change: {error:?}"),
        }
    }
}

#[tokio::test]
async fn a_session_connects_shows_output_takes_input_resizes_roams_and_ends() {
    let mut server = FakeServer::new(KEY).await;
    let (handle, control, states) = start_fake(server.port(), KEY, CONNECT_TIMEOUT);
    assert_eq!(handle.state(), SessionState::Connecting);
    let mut grid = Grid::default();

    // The first datagram carries the size the client wants, as the server starts at 80x24.
    let heard = server
        .hear_until(|heard| heard.iter().any(|h| !h.resizes.is_empty()))
        .await;
    assert!(heard.iter().any(|h| h.resizes.contains(&(20, 5))));
    let first_port = heard[0].from.port();

    // Not connected until the server authenticates.
    assert_eq!(handle.state(), SessionState::Connecting);
    assert!(handle.send_text("x".into()).is_err());
    server.say(b"hello\r\n$ ").await;
    assert_eq!(state(&states).await, SessionState::Connected);
    grid.wait_for(&handle, "hello").await;
    assert_eq!(grid.size, Some(TerminalSize::new(20, 5).unwrap()));

    // Text and keys reach the server; the key goes through the terminal's own encoder.
    handle.send_text("ab".into()).unwrap();
    handle
        .send_key(KeyInput::new(Key::ArrowUp, Modifiers::default()).unwrap())
        .unwrap();
    let heard = server
        .hear_until(|heard| heard.iter().map(|h| h.keys.len()).sum::<usize>() >= 5)
        .await;
    let keys: Vec<u8> = heard.iter().flat_map(|h| h.keys.clone()).collect();
    assert_eq!(keys, b"ab\x1b[A");
    server.say(b"ab").await;
    grid.wait_for(&handle, "$ ab").await;

    // A resize is sent to the server and the frames follow it at once.
    handle.resize(TerminalSize::new(30, 6).unwrap()).unwrap();
    server
        .hear_until(|heard| heard.iter().any(|h| h.resizes.contains(&(30, 6))))
        .await;
    let resized = TerminalSize::new(30, 6).unwrap();
    grid.wait(&handle, "a frame at the new size", |grid| {
        grid.size == Some(resized)
    })
    .await;

    // Roaming: the next datagram comes from a new source port and the server follows it.
    control.roam();
    handle.send_text("c".into()).unwrap();
    let heard = server
        .hear_until(|heard| heard.iter().any(|h| h.from.port() != first_port))
        .await;
    let roamed_from = heard.last().unwrap().from;
    assert_ne!(roamed_from.port(), first_port);
    assert_eq!(server.client, Some(roamed_from));
    server.say(b" after-roam").await;
    grid.wait_for(&handle, "after-roam").await;

    // The server ends the session: the client acknowledges and closes with RemoteExited.
    server.say_goodbye().await;
    assert_eq!(
        state(&states).await,
        SessionState::Closed(CloseReason::RemoteExited { exit_status: None })
    );
    // Requests after close are refused.
    assert_eq!(
        handle.send_text("late".into()),
        Err(crate::session::SessionError::Closed)
    );
}

#[tokio::test]
async fn disconnect_says_goodbye_to_the_server_and_closes() {
    let mut server = FakeServer::new(KEY).await;
    let (handle, _control, states) = start_fake(server.port(), KEY, CONNECT_TIMEOUT);
    server.hear(Duration::from_secs(5)).await.unwrap();
    server.say(b"hi").await;
    assert_eq!(state(&states).await, SessionState::Connected);

    handle.disconnect();
    server
        .hear_until(|heard| heard.iter().any(|h| h.new_num == SHUTDOWN_NUM))
        .await;
    // The server acknowledges the shutdown by echoing it; the close is prompt, not a full second.
    let started = StdInstant::now();
    server.say_goodbye().await;
    assert_eq!(
        state(&states).await,
        SessionState::Closed(CloseReason::Disconnected)
    );
    assert!(started.elapsed() < Duration::from_millis(900));
}

#[tokio::test]
async fn disconnect_does_not_wait_for_a_server_that_is_gone() {
    let mut server = FakeServer::new(KEY).await;
    let (handle, _control, states) = start_fake(server.port(), KEY, CONNECT_TIMEOUT);
    server.hear(Duration::from_secs(5)).await.unwrap();
    server.say(b"hi").await;
    assert_eq!(state(&states).await, SessionState::Connected);
    drop(server);

    let started = StdInstant::now();
    handle.disconnect();
    assert_eq!(
        state(&states).await,
        SessionState::Closed(CloseReason::Disconnected)
    );
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn dropping_the_handle_disconnects() {
    let mut server = FakeServer::new(KEY).await;
    let (handle, _control, states) = start_fake(server.port(), KEY, CONNECT_TIMEOUT);
    server.hear(Duration::from_secs(5)).await.unwrap();
    server.say(b"hi").await;
    assert_eq!(state(&states).await, SessionState::Connected);
    drop(handle);
    let heard = server
        .hear_until(|heard| heard.iter().any(|h| h.new_num == SHUTDOWN_NUM))
        .await;
    assert!(!heard.is_empty());
}

#[tokio::test]
async fn a_server_that_never_answers_times_out() {
    let server = FakeServer::new(KEY).await;
    let (_handle, _control, states) = start_fake(server.port(), KEY, Duration::from_millis(300));
    assert_eq!(
        state(&states).await,
        SessionState::Closed(CloseReason::Failed(SessionFailure::TimedOut))
    );
}

#[tokio::test]
async fn a_server_with_another_key_is_never_connected() {
    let mut server = FakeServer::new(OTHER_KEY).await;
    let (handle, _control, states) = start_fake(server.port(), KEY, Duration::from_millis(600));
    // It cannot even read the client's datagrams, so it never learns where to answer.
    assert!(server.hear(Duration::from_millis(300)).await.is_none());
    assert_eq!(
        state(&states).await,
        SessionState::Closed(CloseReason::Failed(SessionFailure::TimedOut))
    );
    assert!(handle.take_frame().is_none());
}

#[tokio::test]
async fn forged_datagrams_do_not_connect_a_session() {
    let mut server = FakeServer::new(KEY).await;
    let (handle, _control, states) = start_fake(server.port(), KEY, Duration::from_millis(600));
    let heard = server.hear(Duration::from_secs(5)).await.unwrap();
    // Garbage from the right address does not authenticate.
    server.socket.send_to(&[9u8; 80], heard.from).await.unwrap();
    assert_eq!(
        state(&states).await,
        SessionState::Closed(CloseReason::Failed(SessionFailure::TimedOut))
    );
    assert!(handle.take_frame().is_none());
}

#[test]
fn an_invalid_endpoint_is_refused_synchronously() {
    let (sender, _states) = mpsc::channel();
    let observer = Arc::new(Recorder(Mutex::new(sender)));
    assert!(start(params(0, KEY, 80, 24), LOCALHOST, observer).is_err());
}

/// A transport whose sockets open only when told to (after the first `free` binds, which are
/// immediate): a name resolver that is slow, or never answers.
struct Gated {
    gate: Arc<tokio::sync::Notify>,
    binds: Arc<AtomicUsize>,
    free: usize,
}

impl Gated {
    fn new(free: usize) -> (Self, Arc<tokio::sync::Notify>, Arc<AtomicUsize>) {
        let gate = Arc::new(tokio::sync::Notify::new());
        let binds = Arc::new(AtomicUsize::new(0));
        let transport = Self {
            gate: gate.clone(),
            binds: binds.clone(),
            free,
        };
        (transport, gate, binds)
    }
}

impl DatagramTransport for Gated {
    type Socket = <DirectUdp as DatagramTransport>::Socket;

    async fn bind(&self, endpoint: &Endpoint) -> std::io::Result<Self::Socket> {
        let attempt = self.binds.fetch_add(1, Ordering::SeqCst);
        if attempt >= self.free {
            self.gate.notified().await;
        }
        DirectUdp.bind(endpoint).await
    }
}

fn start_gated(
    transport: Gated,
    port: u16,
) -> (SessionHandle, LinkControl, mpsc::Receiver<SessionState>) {
    let (sender, states) = mpsc::channel();
    let (handle, control) = spawn(
        transport,
        params(port, KEY, 20, 5),
        LOCALHOST,
        Arc::new(Recorder(Mutex::new(sender))),
        None,
        CONNECT_TIMEOUT,
    )
    .unwrap();
    (handle, control, states)
}

#[tokio::test]
async fn a_disconnect_does_not_wait_for_the_first_socket() {
    // The resolver never answers.
    let (transport, _gate, _binds) = Gated::new(0);
    let (handle, _control, states) = start_gated(transport, 60002);
    handle.disconnect();
    let started = StdInstant::now();
    assert_eq!(
        state(&states).await,
        SessionState::Closed(CloseReason::Disconnected)
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn a_resize_while_the_first_socket_opens_is_not_lost() {
    let mut server = FakeServer::new(KEY).await;
    let (transport, gate, _binds) = Gated::new(0);
    let (handle, _control, _states) = start_gated(transport, server.port());
    handle.resize(TerminalSize::new(30, 6).unwrap()).unwrap();
    // Give the driver time to take the command while the socket is still not open.
    tokio::time::sleep(Duration::from_millis(100)).await;
    gate.notify_one();
    let heard = server
        .hear_until(|heard| heard.iter().any(|h| !h.resizes.is_empty()))
        .await;
    assert!(heard.iter().any(|h| h.resizes.contains(&(30, 6))));
}

#[tokio::test]
async fn a_rebind_stuck_on_the_resolver_does_not_freeze_the_session() {
    let mut server = FakeServer::new(KEY).await;
    // The first socket opens at once; every later one waits for a resolver that never answers.
    let (transport, _gate, binds) = Gated::new(1);
    let (handle, control, states) = start_gated(transport, server.port());
    let mut grid = Grid::default();
    server.hear(Duration::from_secs(5)).await.unwrap();
    server.say(b"hi").await;
    assert_eq!(state(&states).await, SessionState::Connected);

    control.roam();
    let deadline = StdInstant::now() + Duration::from_secs(5);
    while binds.load(Ordering::SeqCst) < 2 {
        assert!(StdInstant::now() < deadline, "the rebind never started");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Stuck: but input still goes out, and frames still come in.
    handle.send_text("x".into()).unwrap();
    server
        .hear_until(|heard| heard.iter().any(|h| h.keys == b"x"))
        .await;
    server.say(b"ok").await;
    grid.wait_for(&handle, "hiok").await;

    // The attempt is given up after a while and tried again.
    let deadline = StdInstant::now() + REBIND_TIMEOUT + REBIND_RETRY + Duration::from_secs(3);
    while binds.load(Ordering::SeqCst) < 3 {
        assert!(
            StdInstant::now() < deadline,
            "a rebind that never finishes was never retried"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // And a disconnect is served in the middle of it.
    handle.disconnect();
    server
        .hear_until(|heard| heard.iter().any(|h| h.new_num == SHUTDOWN_NUM))
        .await;
    server.say_goodbye().await;
    assert_eq!(
        state(&states).await,
        SessionState::Closed(CloseReason::Disconnected)
    );
}

/// A name whose later resolutions land on another address: the first socket reaches the server
/// that was asked for, every later one a different loopback IP.
struct Moving(AtomicUsize);

impl DatagramTransport for Moving {
    type Socket = <DirectUdp as DatagramTransport>::Socket;

    async fn bind(&self, endpoint: &Endpoint) -> std::io::Result<Self::Socket> {
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            DirectUdp.bind(endpoint).await
        } else {
            DirectUdp
                .bind(&Endpoint::new("127.0.0.2", endpoint.port()).unwrap())
                .await
        }
    }
}

#[tokio::test]
async fn a_roam_to_another_address_is_refused_and_the_session_keeps_its_server() {
    let mut server = FakeServer::new(KEY).await;
    let (sender, states) = mpsc::channel();
    let (handle, control) = spawn(
        Moving(AtomicUsize::new(0)),
        params(server.port(), KEY, 20, 5),
        LOCALHOST,
        Arc::new(Recorder(Mutex::new(sender))),
        None,
        CONNECT_TIMEOUT,
    )
    .unwrap();
    let mut grid = Grid::default();
    let first_port = server
        .hear(Duration::from_secs(5))
        .await
        .unwrap()
        .from
        .port();
    server.say(b"hi").await;
    assert_eq!(state(&states).await, SessionState::Connected);

    // The roam opens a socket that resolves elsewhere; it is refused, so the client keeps
    // sending from its old socket to the same server, and replies are still read.
    control.roam();
    tokio::time::sleep(Duration::from_millis(300)).await;
    handle.send_text("x".into()).unwrap();
    let heard = server
        .hear_until(|heard| heard.iter().any(|h| h.keys == b"x"))
        .await;
    assert!(
        heard.iter().all(|h| h.from.port() == first_port),
        "nothing left from a new port"
    );
    server.say(b"ok").await;
    grid.wait_for(&handle, "hiok").await;
    handle.disconnect();
}
