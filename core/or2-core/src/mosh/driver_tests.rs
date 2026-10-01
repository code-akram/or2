//! Driver tests against a fake mosh server: just enough of the server side of the protocol to
//! drive the client, so the lifecycle is covered without a `mosh-server` binary. The live test
//! against the real one is `tests/mosh_live.rs`.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::task::{Context, Poll};
use std::time::Instant as StdInstant;

use prost::Message as _;
use tokio::net::UdpSocket;
use tokio::sync::mpsc as async_mpsc;

use crate::frame::{CellWidth, Frame};
use crate::input::{Key, KeyInput, Modifiers};
use crate::term::TerminalSize;
use crate::transport::DatagramSocket;

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
    wire: Wire,
    crypto: CryptoSession,
    packets: PacketState,
    assembly: FragmentAssembly,
    fragmenter: Fragmenter,
    started: Instant,
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

/// Where the fake server's datagrams travel: a real loopback UDP socket, or a pair of in-memory
/// queues (see [`MemTransport`]), which need no I/O driver, so a paused clock is the only clock
/// and a test never waits on real time.
enum Wire {
    Udp(UdpSocket),
    Mem {
        from_client: async_mpsc::UnboundedReceiver<Vec<u8>>,
        to_client: async_mpsc::UnboundedSender<Vec<u8>>,
    },
}

/// The source address an in-memory client appears to send from.
const MEM_CLIENT: SocketAddr = SocketAddr::new(LOCALHOST, 40_000);

impl Wire {
    async fn recv_from(&mut self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        match self {
            Wire::Udp(socket) => socket.recv_from(buf).await,
            Wire::Mem { from_client, .. } => {
                // A dropped client is silence, like a UDP socket nobody writes to.
                let Some(datagram) = from_client.recv().await else {
                    return std::future::pending().await;
                };
                buf[..datagram.len()].copy_from_slice(&datagram);
                Ok((datagram.len(), MEM_CLIENT))
            }
        }
    }

    async fn send_to(&self, datagram: &[u8], to: SocketAddr) -> io::Result<usize> {
        match self {
            Wire::Udp(socket) => socket.send_to(datagram, to).await,
            Wire::Mem { to_client, .. } => {
                let _ = to_client.send(datagram.to_vec());
                Ok(datagram.len())
            }
        }
    }
}

/// The client's end of an in-memory link: one socket, handed out by the first `bind`.
struct MemTransport(Mutex<Option<MemSocket>>);

struct MemSocket {
    peer: SocketAddr,
    to_server: async_mpsc::UnboundedSender<Vec<u8>>,
    from_server: Mutex<async_mpsc::UnboundedReceiver<Vec<u8>>>,
}

impl DatagramSocket for MemSocket {
    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(MEM_CLIENT)
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer)
    }

    fn try_send(&self, datagram: &[u8]) -> io::Result<usize> {
        let _ = self.to_server.send(datagram.to_vec());
        Ok(datagram.len())
    }

    fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let mut from_server = self.from_server.lock().unwrap();
        match from_server.poll_recv(cx) {
            Poll::Ready(Some(datagram)) => {
                buf[..datagram.len()].copy_from_slice(&datagram);
                Poll::Ready(Ok(datagram.len()))
            }
            // A closed link never delivers again.
            Poll::Ready(None) | Poll::Pending => Poll::Pending,
        }
    }
}

impl DatagramTransport for MemTransport {
    type Socket = MemSocket;

    async fn bind(&self, endpoint: &Endpoint) -> io::Result<MemSocket> {
        let mut socket = self
            .0
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| io::Error::other("the in-memory link has one socket"))?;
        socket.peer = SocketAddr::new(endpoint.host().parse().unwrap(), endpoint.port());
        Ok(socket)
    }
}

impl FakeServer {
    async fn new(key: &str) -> Self {
        Self::with_wire(
            key,
            Wire::Udp(UdpSocket::bind("127.0.0.1:0").await.unwrap()),
        )
    }

    /// A server on an in-memory link, with the transport that reaches it.
    fn in_memory(key: &str) -> (Self, MemTransport) {
        let (to_server, from_client) = async_mpsc::unbounded_channel();
        let (to_client, from_server) = async_mpsc::unbounded_channel();
        let server = Self::with_wire(
            key,
            Wire::Mem {
                from_client,
                to_client,
            },
        );
        let socket = MemSocket {
            peer: SocketAddr::new(LOCALHOST, 0),
            to_server,
            from_server: Mutex::new(from_server),
        };
        (server, MemTransport(Mutex::new(Some(socket))))
    }

    fn with_wire(key: &str, wire: Wire) -> Self {
        Self {
            wire,
            crypto: CryptoSession::new(&Base64Key::from_printable(key).unwrap()),
            packets: PacketState::default(),
            assembly: FragmentAssembly::default(),
            fragmenter: Fragmenter::default(),
            started: Instant::now(),
            sent: 0,
            client_num: 0,
            client: None,
        }
    }

    fn port(&self) -> u16 {
        match &self.wire {
            Wire::Udp(socket) => socket.local_addr().unwrap().port(),
            Wire::Mem { .. } => 0,
        }
    }

    fn now(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// The next instruction from the client, or `None` after `wait`.
    async fn hear(&mut self, wait: Duration) -> Option<Heard> {
        let deadline = Instant::now() + wait;
        let mut buf = [0u8; 2048];
        loop {
            let (n, from) = tokio::time::timeout_at(deadline, self.wire.recv_from(&mut buf))
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
            self.wire.send_to(&datagram, to).await.unwrap();
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

/// Runs a session as a task of the current runtime instead of on a thread of its own, so a test
/// on a paused clock (`start_paused`) drives the protocol timers, the submit delay and the fake
/// server on the one virtual clock. Pair it with [`FakeServer::in_memory`].
fn start_here(transport: MemTransport, key: &str) -> (SessionHandle, mpsc::Receiver<SessionState>) {
    let (sender, states) = mpsc::channel();
    let (handle, mut driver) = crate::session::channel(Arc::new(Recorder(Mutex::new(sender))));
    let params = params(1, key, 20, 5);
    let plan = Plan {
        transport: Arc::new(transport),
        peer: SocketAddr::new(LOCALHOST, params.port),
        params,
        health: None,
        roam: Arc::new(Notify::new()),
        shutdown: Arc::new(Notify::new()),
        connect_timeout: CONNECT_TIMEOUT,
        deadline: None,
    };
    tokio::task::spawn_local(async move {
        let ended = run_session(plan, &mut driver).await;
        driver.close(ended.reason);
    });
    (handle, states)
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
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(taken) = handle.take_frame() {
                self.merge(&taken.frame);
            }
            if done(self) {
                return;
            }
            assert!(
                Instant::now() < deadline,
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
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match states.try_recv() {
            Ok(state) => return state,
            Err(mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
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

/// Runs on a paused clock with the session and the fake server in one runtime and no socket
/// between them: the protocol's send interval, the 100 ms before the Enter and every wait in the
/// test are virtual, so how busy the machine is cannot change what the server hears (it used to
/// depend on the driver thread being scheduled within the delay).
#[tokio::test(start_paused = true)]
async fn submit_sends_the_text_then_a_separate_enter_after_the_delay_in_order() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut server, transport) = FakeServer::in_memory(KEY);
            let (handle, states) = start_here(transport, KEY);
            server
                .hear_until(|heard| heard.iter().any(|h| !h.resizes.is_empty()))
                .await;
            server.say(b"$ ").await;
            assert_eq!(state(&states).await, SessionState::Connected);

            // Bracketed paste off: typed text, then Enter alone. The user stream is cumulative until the
            // server acknowledges, so the first instruction with the text must not yet hold the Enter.
            let started = Instant::now();
            handle.submit_text("ab\ncd".into()).unwrap();
            let heard = server
                .hear_until(|heard| heard.last().is_some_and(|h| h.keys.ends_with(b"\r")))
                .await;
            assert!(started.elapsed() >= crate::submit::SUBMIT_ENTER_DELAY);
            assert!(
                heard.iter().any(|h| h.keys == b"ab\rcd"),
                "text alone first"
            );
            assert_eq!(heard.last().unwrap().keys, b"ab\rcd\r");
            server.say(b"x").await; // acknowledges everything heard

            // Bracketed paste on: one paste, a marker inside the text removed, then Enter; input sent
            // straight after the submit lands after its Enter. The mode is on once a frame shows the
            // text that followed it in the same host diff (the engine applied the diff whole).
            server.say(b"\x1b[?2004hM").await;
            Grid::default().wait_for(&handle, "M").await;
            let tab = KeyInput::new(Key::Tab, Modifiers::default()).unwrap();
            handle.submit_text("ab\x1b[201~\ncd".into()).unwrap();
            handle.send_text("z".into()).unwrap();
            handle.send_key(tab).unwrap();
            let heard = server
                .hear_until(|heard| heard.last().is_some_and(|h| h.keys.ends_with(b"\t")))
                .await;
            assert!(
                heard.iter().any(|h| h.keys == b"\x1b[200~ab\ncd\x1b[201~"),
                "the paste alone first"
            );
            assert_eq!(heard.last().unwrap().keys, b"\x1b[200~ab\ncd\x1b[201~\rz\t");
            handle.disconnect();
        })
        .await;
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
    server.wire.send_to(&[9u8; 80], heard.from).await.unwrap();
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

fn sample(heard: u64) -> LinkHealth {
    LinkHealth {
        since_heard_ms: heard,
        since_ack_ms: heard + 10,
    }
}

#[test]
fn health_is_reported_when_what_the_ui_shows_changes_and_at_most_once_a_second() {
    let mut throttle = HealthThrottle::default();
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    // The first sample always goes out.
    assert_eq!(throttle.offer(at(0), sample(300)), Some(sample(300)));
    // A healthy link says nothing more, however its numbers move.
    for (ms, heard) in [(1000, 301), (2000, 900), (30_000, 4999), (31_000, 5000)] {
        assert_eq!(throttle.offer(at(ms), sample(heard)), None, "{heard}");
    }
    // Turning stale is reported, once the second since the last report has passed; and a
    // suppressed sample is not remembered.
    assert_eq!(throttle.offer(at(31_500), sample(5001)), Some(sample(5001)));
    assert_eq!(throttle.offer(at(32_000), sample(6000)), None, "too soon");
    // Stale: one report per further whole second of silence.
    assert_eq!(
        throttle.offer(at(32_500), sample(5900)),
        None,
        "same second"
    );
    assert_eq!(throttle.offer(at(32_600), sample(6100)), Some(sample(6100)));
    assert_eq!(
        throttle.offer(at(33_700), sample(6400)),
        None,
        "same second"
    );
    assert_eq!(throttle.offer(at(34_000), sample(7000)), Some(sample(7000)));
    // Recovery is reported, and then silence again.
    assert_eq!(throttle.offer(at(35_000), sample(400)), Some(sample(400)));
    assert_eq!(throttle.offer(at(36_000), sample(1400)), None);
    // The acknowledgement alone is not what the UI shows.
    let ack_only = LinkHealth {
        since_heard_ms: 400,
        since_ack_ms: 9000,
    };
    assert_eq!(throttle.offer(at(40_000), ack_only), None);
}

#[test]
fn a_healthy_link_is_looked_at_again_when_it_would_turn_stale() {
    let mut throttle = HealthThrottle::default();
    let now = Instant::now();
    assert!(throttle.offer(now, sample(300)).is_some());
    // 300 ms of silence: stale after 4.7 s more, not a wake-up a second.
    assert_eq!(
        throttle.next_check(now, sample(300)),
        now + Duration::from_millis(4701)
    );
    // Close to the edge it waits at least the minimum, and never less than the report
    // interval from the last report.
    let later = now + Duration::from_millis(2000);
    assert_eq!(
        throttle.next_check(later, sample(4990)),
        later + Duration::from_millis(100)
    );
    let soon = now + Duration::from_millis(200);
    assert_eq!(
        throttle.next_check(soon, sample(4990)),
        now + Duration::from_secs(1)
    );
    // Stale: every second.
    let stale = now + Duration::from_secs(10);
    assert_eq!(
        throttle.next_check(stale, sample(9000)),
        stale + Duration::from_secs(1)
    );
}

/// Observes health as the session's own observer sees it.
struct HealthRecorder {
    states: Mutex<mpsc::Sender<SessionState>>,
    health: Mutex<Vec<(StdInstant, LinkHealth, bool)>>,
    connected: std::sync::atomic::AtomicBool,
}

impl SessionObserver for HealthRecorder {
    fn state_changed(&self, state: &SessionState) {
        if *state == SessionState::Connected {
            self.connected.store(true, Ordering::SeqCst);
        }
        let _ = self.states.lock().unwrap().send(state.clone());
    }

    fn frame_ready(&self) {}

    fn link_health(&self, health: LinkHealth) {
        let connected = self.connected.load(Ordering::SeqCst);
        self.health
            .lock()
            .unwrap()
            .push((StdInstant::now(), health, connected));
    }
}

#[tokio::test]
async fn a_connected_session_reports_link_health_only_when_it_turns_stale_and_while_it_is() {
    let mut server = FakeServer::new(KEY).await;
    let (sender, states) = mpsc::channel();
    let recorder = Arc::new(HealthRecorder {
        states: Mutex::new(sender),
        health: Mutex::new(Vec::new()),
        connected: std::sync::atomic::AtomicBool::new(false),
    });
    let (handle, _control) = spawn(
        DirectUdp,
        params(server.port(), KEY, 20, 5),
        LOCALHOST,
        recorder.clone(),
        None,
        CONNECT_TIMEOUT,
    )
    .unwrap();
    server.hear(Duration::from_secs(5)).await.unwrap();
    // Silent until the server authenticates.
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert!(
        recorder.health.lock().unwrap().is_empty(),
        "no health before Connected"
    );
    server.say(b"hi").await;
    assert_eq!(state(&states).await, SessionState::Connected);
    // The first report is the healthy link; then nothing while it stays healthy.
    tokio::time::sleep(Duration::from_millis(500)).await;
    {
        let reports = recorder.health.lock().unwrap();
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert!(reports[0].1.since_heard_ms < STALE_AFTER_MS);
    }
    tokio::time::sleep(Duration::from_millis(3500)).await;
    assert_eq!(
        recorder.health.lock().unwrap().len(),
        1,
        "a healthy link is not reported again"
    );
    // The server stays silent: the link turns stale (past five seconds) and is then reported
    // once a second with the growing silence.
    tokio::time::sleep(Duration::from_millis(3400)).await;
    let reports = recorder.health.lock().unwrap().clone();
    assert!((3..=5).contains(&reports.len()), "{reports:?}");
    assert!(reports.iter().all(|(_, _, connected)| *connected));
    for pair in reports[1..].windows(2) {
        assert!(
            pair[1].0.duration_since(pair[0].0) >= Duration::from_millis(900),
            "at most once a second"
        );
        assert!(pair[1].1.since_heard_ms / 1000 > pair[0].1.since_heard_ms / 1000);
    }
    assert!(reports[1].1.since_heard_ms > STALE_AFTER_MS);
    // Hearing the server again is a recovery, reported.
    server.say(b"back").await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let reports = recorder.health.lock().unwrap().clone();
    assert!(
        reports.last().unwrap().1.since_heard_ms < STALE_AFTER_MS,
        "{reports:?}"
    );
    handle.disconnect();
}

/// The `start_with` hook is not the session observer's throttled report: it sees every
/// sample, about once a second, before the server has been heard as well.
#[tokio::test]
async fn the_health_observer_hook_sees_every_sample_including_before_connected() {
    struct Hook(Mutex<Vec<StdInstant>>);
    impl HealthObserver for Hook {
        fn link_health(&self, _health: LinkHealth) {
            self.0.lock().unwrap().push(StdInstant::now());
        }
    }
    let mut server = FakeServer::new(KEY).await;
    let hook = Arc::new(Hook(Mutex::new(Vec::new())));
    let (sender, states) = mpsc::channel();
    let recorder = Arc::new(HealthRecorder {
        states: Mutex::new(sender),
        health: Mutex::new(Vec::new()),
        connected: std::sync::atomic::AtomicBool::new(false),
    });
    let (handle, _control) = spawn(
        DirectUdp,
        params(server.port(), KEY, 20, 5),
        LOCALHOST,
        recorder.clone(),
        Some(hook.clone()),
        CONNECT_TIMEOUT,
    )
    .unwrap();
    server.hear(Duration::from_secs(5)).await.unwrap();
    // Not connected yet: the hook still hears about the link every second.
    tokio::time::sleep(Duration::from_millis(2400)).await;
    assert!(
        (1..=3).contains(&hook.0.lock().unwrap().len()),
        "{} samples before Connected",
        hook.0.lock().unwrap().len()
    );
    assert!(recorder.health.lock().unwrap().is_empty());
    server.say(b"hi").await;
    assert_eq!(state(&states).await, SessionState::Connected);
    tokio::time::sleep(Duration::from_millis(2000)).await;
    assert!(hook.0.lock().unwrap().len() >= 3);
    handle.disconnect();
}

#[tokio::test]
async fn roam_on_the_session_handle_opens_a_new_socket_like_the_link_control() {
    let mut server = FakeServer::new(KEY).await;
    let (handle, _control, states) = start_fake(server.port(), KEY, CONNECT_TIMEOUT);
    let first_port = server
        .hear(Duration::from_secs(5))
        .await
        .unwrap()
        .from
        .port();
    server.say(b"hi").await;
    assert_eq!(state(&states).await, SessionState::Connected);
    handle.roam();
    handle.send_text("c".into()).unwrap();
    let heard = server
        .hear_until(|heard| heard.iter().any(|h| h.from.port() != first_port))
        .await;
    assert_ne!(heard.last().unwrap().from.port(), first_port);
    handle.disconnect();
}

/// `Plan::shutdown` ends a session like a disconnect (the host driver uses it when the user
/// disconnects the host): the server is told, even if the permit was given before the
/// session ever looked, and a session still connecting closes `Disconnected` too.
#[tokio::test]
async fn the_shutdown_signal_disconnects_with_the_goodbye_handshake() {
    let mut server = FakeServer::new(KEY).await;
    let (sender, states) = mpsc::channel();
    let (_handle, mut driver) = channel(Arc::new(Recorder(Mutex::new(sender))));
    let shutdown = Arc::new(Notify::new());
    let plan = Plan {
        transport: Arc::new(DirectUdp),
        peer: SocketAddr::new(LOCALHOST, server.port()),
        params: params(server.port(), KEY, 20, 5),
        health: None,
        roam: Arc::new(Notify::new()),
        shutdown: shutdown.clone(),
        connect_timeout: CONNECT_TIMEOUT,
        deadline: None,
    };
    // The session future is not `Send` (libghostty): its own thread, as in production.
    let session = std::thread::spawn(move || {
        crate::ssh::runtime().block_on(async move {
            let ended = run_session(plan, &mut driver).await;
            (ended, driver.state())
        })
    });
    server.hear(Duration::from_secs(5)).await.unwrap();
    server.say(b"hi").await;
    assert_eq!(state(&states).await, SessionState::Connected);
    shutdown.notify_one();
    let goodbye = server
        .hear_until(|heard| heard.iter().any(|h| h.new_num == SHUTDOWN_NUM))
        .await;
    assert!(!goodbye.is_empty());
    server.say_goodbye().await;
    let (ended, state_then) = tokio::task::spawn_blocking(move || session.join().unwrap())
        .await
        .unwrap();
    assert_eq!(ended.reason, CloseReason::Disconnected);
    assert!(ended.server_gone, "the server acknowledged the goodbye");
    assert_eq!(
        state_then,
        SessionState::Connected,
        "the caller can tell it connected"
    );

    // Before any datagram arrives: closed at once, `Connecting` tells the caller to clean up.
    let server = FakeServer::new(KEY).await;
    let (_handle, mut driver) = channel(Arc::new(Recorder(Mutex::new(mpsc::channel().0))));
    let shutdown = Arc::new(Notify::new());
    shutdown.notify_one();
    let ended = run_session(
        Plan {
            transport: Arc::new(DirectUdp),
            peer: SocketAddr::new(LOCALHOST, server.port()),
            params: params(server.port(), KEY, 20, 5),
            health: None,
            roam: Arc::new(Notify::new()),
            shutdown,
            connect_timeout: CONNECT_TIMEOUT,
            deadline: None,
        },
        &mut driver,
    )
    .await;
    assert_eq!(ended.reason, CloseReason::Disconnected);
    assert!(!ended.server_gone, "no server was ever reached");
    assert_eq!(driver.state(), SessionState::Connecting);
}

/// A goodbye the server never acknowledges is still a user `Disconnected`, but it is not
/// confirmed: the caller must stop the server another way.
#[tokio::test]
async fn an_unanswered_goodbye_closes_disconnected_without_confirming_the_server_is_gone() {
    let mut server = FakeServer::new(KEY).await;
    let (sender, states) = mpsc::channel();
    let (_handle, mut driver) = channel(Arc::new(Recorder(Mutex::new(sender))));
    let shutdown = Arc::new(Notify::new());
    let plan = Plan {
        transport: Arc::new(DirectUdp),
        peer: SocketAddr::new(LOCALHOST, server.port()),
        params: params(server.port(), KEY, 20, 5),
        health: None,
        roam: Arc::new(Notify::new()),
        shutdown: shutdown.clone(),
        connect_timeout: CONNECT_TIMEOUT,
        deadline: None,
    };
    let session = std::thread::spawn(move || {
        crate::ssh::runtime().block_on(async move { run_session(plan, &mut driver).await })
    });
    server.hear(Duration::from_secs(5)).await.unwrap();
    server.say(b"hi").await;
    assert_eq!(state(&states).await, SessionState::Connected);
    shutdown.notify_one();
    // The server hears the goodbye and stays silent.
    let goodbye = server
        .hear_until(|heard| heard.iter().any(|h| h.new_num == SHUTDOWN_NUM))
        .await;
    assert!(!goodbye.is_empty());
    let ended = tokio::task::spawn_blocking(move || session.join().unwrap())
        .await
        .unwrap();
    assert_eq!(ended.reason, CloseReason::Disconnected);
    assert!(!ended.server_gone, "an unanswered goodbye proves nothing");
}

/// An absolute deadline replaces the connect timeout: a server that never answers fails the
/// session `TimedOut` at the deadline (not after the 15 s timeout), still `Connecting` so the
/// caller owes a cleanup, and a deadline already past fails it at once.
#[tokio::test]
async fn an_absolute_deadline_ends_a_session_the_server_never_answers() {
    for (deadline, longest) in [
        (Duration::from_millis(400), Duration::from_secs(5)),
        (Duration::ZERO, Duration::from_secs(2)),
    ] {
        let server = FakeServer::new(KEY).await;
        let (_handle, mut driver) = channel(Arc::new(Recorder(Mutex::new(mpsc::channel().0))));
        let plan = Plan {
            transport: Arc::new(DirectUdp),
            peer: SocketAddr::new(LOCALHOST, server.port()),
            params: params(server.port(), KEY, 20, 5),
            health: None,
            roam: Arc::new(Notify::new()),
            shutdown: Arc::new(Notify::new()),
            connect_timeout: CONNECT_TIMEOUT,
            deadline: Some(Instant::now() + deadline),
        };
        let started = StdInstant::now();
        let session = std::thread::spawn(move || {
            crate::ssh::runtime().block_on(async move {
                let ended = run_session(plan, &mut driver).await;
                (ended, driver.state())
            })
        });
        let (ended, state_then) = tokio::task::spawn_blocking(move || session.join().unwrap())
            .await
            .unwrap();
        assert_eq!(ended.reason, CloseReason::Failed(SessionFailure::TimedOut));
        assert!(!ended.server_gone);
        assert_eq!(state_then, SessionState::Connecting);
        assert!(started.elapsed() >= deadline, "{:?}", started.elapsed());
        assert!(started.elapsed() < longest, "{:?}", started.elapsed());
    }
}
