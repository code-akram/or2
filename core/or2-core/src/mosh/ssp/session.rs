// SPDX-License-Identifier: GPL-3.0-or-later
// Derived from mosh-rs (https://github.com/wilsonglasser/mosh-rs), commit
// 90b37125f5e4a598be91dec37d23921b6865276e, src/session.rs. Upstream: GPL-3.0-or-later, copyright
// Wilson Glasser; the protocol logic follows mosh (Keith Winstein and contributors, GPL-3.0-or-later).
// See THIRD_PARTY_NOTICES.md. or2 changes: the session owns no sockets and never blocks. Datagrams
// come in through `handle_datagram` and go out through `tick`; the driver opens and rebinds
// sockets through `DatagramTransport` (roaming is a `Tick::rebind` request and `note_rebound`).
// Prediction, overlays, rendering to escape bytes and the blocking pump are removed. Every Bytes
// event of a diff is fed in order (upstream fed only the last). The server's resize reports are
// returned as events but not applied: whoever drives the session owns the geometry (`resize`). A state is acknowledged only once its diff is applied (`handle_datagram`).

//! The whole client protocol, assembled, without I/O: the crypto session, the packet clock, the
//! transport state machine and the state sync, around a [`ClientTerminal`].
//!
//! What it does NOT do is touch a socket or a terminal. It hands host output to a [`Screen`] and
//! takes user input as bytes; the caller moves datagrams. [`Session::handle_datagram`] takes one
//! received datagram, [`Session::tick`] returns whatever is due to be sent, and
//! [`Session::wait_time_ms`] says how long the caller may sleep before calling `tick` again.

use tokio::time::Instant;

use super::crypto::{Direction, Session as CryptoSession};
use super::error::MoshError;
use super::key::Base64Key;
use super::packet::{Packet, PacketState};
use super::screen::{Screen, ScreenError};
use super::sender::{Received, TransportReceiver, TransportSender};
use super::statesync::{HostEvent, parse_host_diff};
use super::terminal::ClientTerminal;
use super::transport::{Fragment, FragmentAssembly, Fragmenter, SHUTDOWN_NUM};

/// mosh's own overhead inside a datagram (`Connection::ADDED_BYTES`):
/// the 8-byte nonce prefix and the two timestamps.
const CONNECTION_ADDED_BYTES: usize = 8 + 4;
/// The OCB tag (`Crypto::Session::ADDED_BYTES`).
const CRYPTO_ADDED_BYTES: usize = 16;

/// The datagram size the client builds to, and where it comes from.
///
/// 1280 is not a guess: it is the minimum MTU every IPv6 link is
/// REQUIRED to carry, which makes it the largest size that crosses any
/// path without needing path-MTU discovery to have worked. mosh picked
/// it after finding that VPN traffic over some carrier wifi was dropped
/// at 1320 and above.
const DEFAULT_LINK_MTU: usize = 1280;
/// Headers to leave room for. The IPv6 figure is deliberately generous:
/// two minimum-sized extension headers that may or may not be there.
const IPV4_HEADER_LEN: usize = 20 + 8;
const IPV6_HEADER_LEN: usize = 40 + 16 + 8;

/// The size that works everywhere, and where a send that is refused for
/// being too large falls back to.
const FALLBACK_MTU: usize = 500;

/// How often the client rotates its own source port when the link is not answering. This is the
/// whole of client-side roaming: see [`Session::rebind_due`].
pub const PORT_HOP_INTERVAL_MS: u64 = 10_000;

/// How the link is doing, in the terms a user cares about.
///
/// The two are separate because they fail separately: a link that only
/// works in one direction shows up in `since_ack_ms` while
/// `since_heard_ms` stays small, and saying "no reply" rather than "no
/// contact" is the difference between a useful message and a confusing
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkHealth {
    /// Milliseconds since anything at all arrived from the server.
    pub since_heard_ms: u64,
    /// Milliseconds since the server acknowledged something we sent.
    pub since_ack_ms: u64,
}

/// Why a received datagram did not become host events.
#[derive(Debug, thiserror::Error)]
pub enum Fault {
    /// The datagram was forged, corrupt, a replay or from another protocol version. On a public
    /// UDP port anyone can send bytes, so this is an ordinary event: drop it and carry on.
    #[error("datagram dropped: {0}")]
    Dropped(MoshError),
    /// The screen failed while taking a state. The session cannot continue.
    #[error("screen failed: {0}")]
    Screen(#[from] ScreenError),
}

impl From<MoshError> for Fault {
    fn from(error: MoshError) -> Self {
        Self::Dropped(error)
    }
}

/// What [`Session::tick`] wants done.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Tick {
    /// Datagrams to send, in order, from the newest socket.
    pub datagrams: Vec<Vec<u8>>,
    /// Open a new socket and send from it from now on, keeping the old ones to read from: the
    /// link has stopped answering, or the caller asked for it with [`Session::request_rebind`].
    pub rebind: bool,
}

/// A live mosh session against one server, without its sockets.
pub struct Session<S: Screen> {
    crypto: CryptoSession,
    packets: PacketState,
    sender: TransportSender,
    receiver: TransportReceiver,
    fragmenter: Fragmenter,
    assembly: FragmentAssembly,
    started: Instant,
    /// The client's own copy of the screen, one state per diff the server may still name.
    terminal: ClientTerminal<S>,
    /// The datagram size being built to. Starts from the link MTU and drops to
    /// [`FALLBACK_MTU`] if the path refuses that size.
    mtu: usize,
    /// When the newest socket was opened, which paces the next rotation.
    last_port_choice: u64,
    /// When we sent the newest state the server has acknowledged. How long ago that was is how
    /// long it has been since a round trip actually completed, and that is what decides whether
    /// the client starts looking for a new source port.
    last_roundtrip_success: u64,
    /// Set once the peer's shutdown state has been seen.
    peer_shut_down: bool,
    /// An application that knows the network changed asks for a new socket right away.
    rebind_requested: bool,
}

impl<S: Screen> Session<S> {
    /// Start a session under the key the SSH bootstrap agreed, talking to a server over IPv4 or
    /// IPv6 (`ipv6`, which sizes the datagrams), with `blank` as the screen both sides agree on
    /// before anything is sent. No handshake travels here: the bootstrap already agreed the key,
    /// and the first datagram we send is a normal session packet.
    pub fn new(key: &Base64Key, ipv6: bool, blank: S) -> Self {
        Self {
            crypto: CryptoSession::new(key),
            packets: PacketState::default(),
            sender: TransportSender::new(),
            receiver: TransportReceiver::new(),
            fragmenter: Fragmenter::default(),
            assembly: FragmentAssembly::default(),
            started: Instant::now(),
            terminal: ClientTerminal::new(blank),
            mtu: DEFAULT_LINK_MTU
                - if ipv6 {
                    IPV6_HEADER_LEN
                } else {
                    IPV4_HEADER_LEN
                },
            last_port_choice: 0,
            last_roundtrip_success: 0,
            peer_shut_down: false,
            rebind_requested: false,
        }
    }

    /// Milliseconds since the session began. mosh's timestamps are
    /// monotonic and only ever compared to each other, so any epoch
    /// works as long as it never goes backwards.
    pub fn now(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// Queue user input. It goes out on the next [`Self::tick`] that is due, after the collect
    /// window, so a burst of typing costs one packet.
    pub fn send_input(&mut self, bytes: &[u8]) {
        self.sender.state_mut().push_bytes(bytes);
    }

    /// Queue a terminal resize for the server and resize the local screen to match.
    ///
    /// Unlike upstream mosh-rs, which follows the server's own resize reports, the geometry is
    /// owned by the caller: the renderer asked for this size and frames must keep it. The
    /// server's next diff after a resize repaints every cell, so a diff computed for the old
    /// shape that lands on the new one corrects itself within a round trip.
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<(), ScreenError> {
        self.sender
            .state_mut()
            .push_resize(i32::from(cols), i32::from(rows));
        self.terminal.resize(rows, cols)
    }

    /// Begin the shutdown handshake.
    pub fn shutdown(&mut self) {
        let now = self.now();
        self.sender.start_shutdown(now);
    }

    /// True once the peer has acknowledged our shutdown, announced its own, or stopped answering
    /// long enough that it never will.
    pub fn finished(&self) -> bool {
        self.peer_shut_down
            || self.sender.shutdown_acknowledged()
            || self.sender.shutdown_timed_out(self.now())
    }

    /// True only if the peer itself confirmed the end: it acknowledged our shutdown, or
    /// announced its own. [`Session::finished`] is also true when we merely stopped waiting
    /// (the retries ran out), which proves nothing about the server.
    pub fn shutdown_confirmed(&self) -> bool {
        self.peer_shut_down || self.sender.shutdown_acknowledged()
    }

    /// True once the server has announced the end of the session (its shell exited).
    pub fn peer_shut_down(&self) -> bool {
        self.peer_shut_down
    }

    /// How long it has been since the server was heard from, and since it last acknowledged us.
    pub fn link_health(&self) -> LinkHealth {
        let now = self.now();
        LinkHealth {
            since_heard_ms: now.saturating_sub(self.sender.last_heard()),
            since_ack_ms: now.saturating_sub(self.sender.sent_state_acked_timestamp()),
        }
    }

    /// The client's screen states.
    pub fn terminal(&mut self) -> &mut ClientTerminal<S> {
        &mut self.terminal
    }

    /// How long the caller may wait before it needs to call [`Self::tick`], even with nothing
    /// arriving from the network. mosh's `wait_time`; recalculates before answering, so input
    /// queued a moment ago is accounted for.
    pub fn wait_time_ms(&mut self) -> u64 {
        let now = self.now();
        let (srtt, rto) = (self.packets.rtt.srtt(), self.packets.rtt.rto());
        self.sender.wait_time(now, srtt, rto)
    }

    /// Ask for a new source socket on the next [`Self::tick`], without waiting for the link to
    /// be judged failed.
    ///
    /// The ordinary rotation only fires after ten seconds with no completed round trip, which
    /// is the right default for a client that cannot know WHY answers stopped. An application
    /// that does know, because the operating system just told it the network changed, can say
    /// so and save the session those ten seconds. mosh has no equivalent, having no application
    /// to tell it.
    pub fn request_rebind(&mut self) {
        self.rebind_requested = true;
    }

    /// The caller opened and switched to a new socket: restart the rotation clock.
    pub fn note_rebound(&mut self) {
        self.last_port_choice = self.now();
        self.rebind_requested = false;
    }

    /// The path refused a datagram for being too large: fall back to the size that works
    /// everywhere and stay there, as mosh does. The datagrams just refused are lost, and that
    /// costs nothing: this protocol resends STATE, not packets, so the next tick rebuilds them
    /// smaller.
    pub fn datagram_too_large(&mut self) {
        self.mtu = FALLBACK_MTU;
    }

    /// Rotation is a RECOVERY move, not a habit, and it is the whole of client-side roaming.
    ///
    /// A mosh client NEVER re-targets: it keeps talking to the address it was given. The SERVER
    /// re-targets, onto the source of any datagram that passes its authentication. So when a
    /// laptop changes networks, what tells the server where to answer is simply the next packet
    /// arriving from somewhere new, and a fresh source port is what forces a NAT to mint a fresh
    /// mapping for it. Nothing is negotiated and nothing is announced.
    ///
    /// Both clocks have to have run out: ten seconds since the last rotation, AND ten seconds
    /// since a round trip last completed. A healthy session keeps one source port for its whole
    /// life; one that has stopped getting answers starts trying new ones, which is exactly the
    /// state a laptop is in a moment after it changes networks.
    pub fn rebind_due(&self, now: u64) -> bool {
        now.saturating_sub(self.last_port_choice) > PORT_HOP_INTERVAL_MS
            && now.saturating_sub(self.last_roundtrip_success) > PORT_HOP_INTERVAL_MS
    }

    /// Run the timers: the datagrams due now, and whether to rebind. Cheap when nothing is due.
    ///
    /// Call it after every received datagram, after queueing input, and whenever
    /// [`Self::wait_time_ms`] has elapsed.
    pub fn tick(&mut self) -> Result<Tick, MoshError> {
        let now = self.now();
        let (srtt, rto) = (self.packets.rtt.srtt(), self.packets.rtt.rto());
        let mut out = Tick::default();
        if let Some(inst) = self.sender.tick(now, srtt, rto) {
            let budget = self.mtu - CONNECTION_ADDED_BYTES - CRYPTO_ADDED_BYTES;
            for frag in self.fragmenter.fragment(&inst, budget)? {
                let outgoing = self.packets.new_packet(now, frag.to_bytes());
                out.datagrams.push(self.crypto.encrypt(
                    outgoing.seq,
                    Direction::ToServer,
                    &outgoing.packet.to_plaintext(),
                )?);
            }
            // The rotation decision lives with the send, as it does in mosh's
            // `Connection::send`.
            out.rebind = self.rebind_due(now);
        }
        if self.rebind_requested {
            out.rebind = true;
        }
        Ok(out)
    }

    /// Take one datagram that arrived from the server: decrypt it, run it through the transport
    /// state machine and apply what it carries to the screen. Returns the host events it held
    /// (empty for a bare acknowledgement).
    pub fn handle_datagram(&mut self, datagram: &[u8]) -> Result<Vec<HostEvent>, Fault> {
        let incoming = self.crypto.decrypt(datagram)?;
        // Anti-reflection: a packet marked as ours cannot have come
        // from the server.
        if incoming.direction != Direction::ToClient {
            return Err(MoshError::WrongDirection.into());
        }
        let packet = Packet::from_plaintext(&incoming.plaintext)?;
        let now = self.now();
        self.packets.accept(now, incoming.seq, &packet, false);
        if packet.payload.is_empty() {
            return Ok(Vec::new());
        }

        let frag = Fragment::from_bytes(&packet.payload)?;
        if !self.assembly.add(frag) {
            return Ok(Vec::new());
        }
        let Some(inst) = self.assembly.take()? else {
            return Ok(Vec::new());
        };
        inst.check_version()?;

        if let Some(ack) = inst.ack_num {
            self.sender.process_acknowledgement(ack);
        }
        if inst.new_num == Some(SHUTDOWN_NUM) {
            self.peer_shut_down = true;
        }
        // A completed round trip: the peer is answering what we sent. How long ago the newest
        // ACKNOWLEDGED state went out is what tells the roaming logic whether the link works.
        self.last_roundtrip_success = self.sender.sent_state_acked_timestamp();
        let (num, diff, in_order) = match self.receiver.process(&inst, now) {
            Received::Apply { num, diff } => (num, diff, true),
            Received::ApplyOutOfOrder { num, diff } => (num, diff, false),
            // A duplicate is still evidence the peer is alive, and it
            // must be acknowledged again or the peer keeps resending.
            Received::Duplicate => {
                self.sender.set_ack_num(self.receiver.latest(), false, now);
                return Ok(Vec::new());
            }
            Received::Unresolvable => return Ok(Vec::new()),
        };
        let has_data = !diff.is_empty();
        let parsed = if has_data {
            parse_host_diff(&diff)
        } else {
            Ok(Vec::new())
        };

        // EVERY state gets a screen, including one whose diff changed nothing on it: the server
        // is free to compute a later diff from that state, and a client that only remembered the
        // states that painted something would have to drop it.
        let old_num = inst.old_num.unwrap_or(0);
        let throwaway = inst.throwaway_num.unwrap_or(0);
        let applied = match &parsed {
            Ok(events) => {
                let mut painted = Vec::new();
                for event in events {
                    if let HostEvent::Bytes(bytes) = event {
                        painted.extend_from_slice(bytes);
                    }
                    // Resize and EchoAck are not applied here: see `resize`, and there is no
                    // prediction to retire.
                }
                // The diff belongs to the state it was computed FROM, not to whatever is newest.
                self.terminal
                    .apply_diff(old_num, num, &painted, throwaway)?
            }
            Err(_) => false,
        };
        if !applied {
            // The screen has no state `num` (its base is gone, or the diff is garbled), so it
            // must not be acknowledged: the server would keep diffing from a state we never
            // built and the screen would stay stale. Forget it, and acknowledge what we DO hold
            // so the server's next diff starts from there.
            self.receiver.forget(num);
            self.sender.set_ack_num(self.receiver.latest(), true, now);
            return match parsed {
                Ok(_) => Ok(Vec::new()),
                Err(error) => Err(error.into()),
            };
        }
        // Acknowledge the HIGHEST state held, never this one: an out-of-order arrival is older
        // than something we already have, and naming it would ask the server to resend what is
        // already on screen.
        if in_order {
            self.sender
                .set_ack_num(self.receiver.latest(), has_data, now);
        }
        if let Some(throwaway) = inst.throwaway_num {
            self.terminal.forget_before(throwaway);
        }
        let events = parsed?;
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::super::terminal::tests::TextScreen;
    use super::*;

    fn offline_session() -> Session<TextScreen> {
        let key = Base64Key::from_printable("AAAAAAAAAAAAAAAAAAAAAA").expect("valid key");
        Session::new(&key, false, TextScreen::new(24, 80))
    }

    #[test]
    fn the_datagram_size_follows_the_address_family() {
        let key = Base64Key::from_printable("AAAAAAAAAAAAAAAAAAAAAA").expect("valid key");
        let v4 = Session::new(&key, false, TextScreen::new(24, 80));
        // 1280 minus IP and UDP headers: mosh's own 1252.
        assert_eq!(v4.mtu, 1252);
        let v6 = Session::new(&key, true, TextScreen::new(24, 80));
        // Less, because the IPv6 allowance covers extension headers that may or may not be on
        // the path.
        assert_eq!(v6.mtu, 1216);
        assert!(v6.mtu < v4.mtu);
    }

    #[test]
    fn the_fallback_is_a_size_that_works_everywhere() {
        let mut session = offline_session();
        session.datagram_too_large();
        assert_eq!(session.mtu, FALLBACK_MTU);
        assert_eq!(
            FALLBACK_MTU - CONNECTION_ADDED_BYTES - CRYPTO_ADDED_BYTES,
            472
        );
        // The larger datagram carries far more per fragment, which is the point of asking for
        // it: every fragment a screenful needs is another packet whose loss costs the whole.
        const {
            assert!(
                1252 - CONNECTION_ADDED_BYTES - CRYPTO_ADDED_BYTES
                    > (FALLBACK_MTU - CONNECTION_ADDED_BYTES - CRYPTO_ADDED_BYTES) * 2
            );
        }
    }

    #[test]
    fn nothing_rebinds_before_the_interval() {
        let session = offline_session();
        assert!(!session.rebind_due(PORT_HOP_INTERVAL_MS));
        assert!(session.rebind_due(PORT_HOP_INTERVAL_MS + 1));
    }

    #[test]
    fn a_working_link_never_rebinds() {
        let mut session = offline_session();
        // A round trip completed a moment ago, so the link is fine and there is nothing to
        // recover from. mosh keeps its port for the whole life of a healthy session.
        let now = 5 * PORT_HOP_INTERVAL_MS;
        session.last_roundtrip_success = now - 1;
        assert!(!session.rebind_due(now));
        // Ten seconds of silence later, it starts looking.
        assert!(session.rebind_due(now + PORT_HOP_INTERVAL_MS + 2));
    }

    #[test]
    fn rotations_are_paced_from_the_last_one() {
        let mut session = offline_session();
        let mut now = PORT_HOP_INTERVAL_MS + 1;
        assert!(session.rebind_due(now));
        session.last_port_choice = now;
        // A moment later is not another interval.
        now += 1;
        assert!(!session.rebind_due(now));
        now += PORT_HOP_INTERVAL_MS;
        assert!(session.rebind_due(now));
    }

    #[test]
    fn a_requested_rebind_is_reported_once_and_cleared_by_note_rebound() {
        let mut session = offline_session();
        assert!(!session.tick().unwrap().rebind);
        session.request_rebind();
        // Reported on every tick until the caller has actually switched sockets.
        assert!(session.tick().unwrap().rebind);
        assert!(session.tick().unwrap().rebind);
        session.note_rebound();
        assert!(!session.tick().unwrap().rebind);
    }

    #[test]
    fn a_resize_is_queued_for_the_server_and_applied_locally() {
        let mut session = offline_session();
        session.resize(30, 100).unwrap();
        assert_eq!(
            (
                session.terminal.live().rows(),
                session.terminal.live().cols()
            ),
            (30, 100)
        );
        // The first tick sends it (the collect window is a few milliseconds).
        std::thread::sleep(std::time::Duration::from_millis(30));
        let tick = session.tick().unwrap();
        assert_eq!(tick.datagrams.len(), 1);
    }

    /// The server's half, enough to hand the client host diffs.
    struct Server {
        crypto: CryptoSession,
        packets: PacketState,
        fragmenter: Fragmenter,
    }

    impl Server {
        fn new() -> Self {
            let key = Base64Key::from_printable("AAAAAAAAAAAAAAAAAAAAAA").expect("valid key");
            Self {
                crypto: CryptoSession::new(&key),
                packets: PacketState::default(),
                fragmenter: Fragmenter::default(),
            }
        }

        /// A datagram whose diff appends `text`, from state `old` to state `new`.
        fn say(&mut self, old: u64, new: u64, text: &str) -> Vec<u8> {
            use prost::Message as _;

            use super::super::statesync::{HostBytes, HostInstruction, HostMessage};
            let diff = HostMessage {
                instruction: vec![HostInstruction {
                    hostbytes: Some(HostBytes {
                        hoststring: Some(text.as_bytes().to_vec()),
                    }),
                    ..Default::default()
                }],
            }
            .encode_to_vec();
            self.send(old, new, diff)
        }

        fn send(&mut self, old: u64, new: u64, diff: Vec<u8>) -> Vec<u8> {
            let inst = super::super::transport::Instruction::new(old, new, 0, 0, diff, vec![]);
            let fragments = self.fragmenter.fragment(&inst, 1000).unwrap();
            assert_eq!(fragments.len(), 1);
            let outgoing = self.packets.new_packet(0, fragments[0].to_bytes());
            self.crypto
                .encrypt(
                    outgoing.seq,
                    Direction::ToClient,
                    &outgoing.packet.to_plaintext(),
                )
                .unwrap()
        }
    }

    #[test]
    fn a_state_whose_base_is_gone_is_not_acknowledged() {
        let mut session = offline_session();
        let mut server = Server::new();
        // The server's states arrive in order and its throwaway never moves, as when our
        // acknowledgements are lost on the way back: the client holds more than it may keep.
        for num in 1..=40 {
            let datagram = server.say(num - 1, num, ".");
            session.handle_datagram(&datagram).unwrap();
        }
        assert_eq!(session.sender.ack_num(), 40);
        let gone = (1..40)
            .find(|num| !session.terminal.holds(*num))
            .expect("the cap gave a state up");
        assert!(session.receiver.latest() == 40);

        // A diff from the given-up state names a screen the client no longer has.
        let datagram = server.say(gone, 41, "!");
        assert!(session.handle_datagram(&datagram).unwrap().is_empty());
        // It was not applied, so it is not held and not acknowledged: the server learns what
        // the client really has and diffs from that.
        assert_eq!(session.terminal.latest(), 40);
        assert_eq!(session.receiver.latest(), 40);
        assert_eq!(session.sender.ack_num(), 40);
        assert!(!session.terminal.live().text.contains('!'));

        // Its retransmission from a state that is held is taken as new, not as a duplicate.
        let datagram = server.say(40, 41, "!");
        session.handle_datagram(&datagram).unwrap();
        assert_eq!(session.terminal.latest(), 41);
        assert_eq!(session.sender.ack_num(), 41);
        assert!(session.terminal.live().text.ends_with('!'));
    }

    #[test]
    fn a_garbled_diff_is_not_acknowledged() {
        let mut session = offline_session();
        let mut server = Server::new();
        let datagram = server.say(0, 1, "a");
        session.handle_datagram(&datagram).unwrap();

        let datagram = server.send(1, 2, vec![0xff; 12]);
        assert!(matches!(
            session.handle_datagram(&datagram),
            Err(Fault::Dropped(MoshError::BadInstruction))
        ));
        assert_eq!(session.receiver.latest(), 1);
        assert_eq!(session.sender.ack_num(), 1);

        let datagram = server.say(1, 2, "b");
        session.handle_datagram(&datagram).unwrap();
        assert_eq!(session.sender.ack_num(), 2);
        assert_eq!(session.terminal.live().text, "ab");
    }

    #[test]
    fn a_forged_datagram_is_dropped_not_fatal() {
        let mut session = offline_session();
        let result = session.handle_datagram(&[7u8; 64]);
        assert!(matches!(
            result,
            Err(Fault::Dropped(MoshError::IntegrityCheck))
        ));
        assert!(matches!(
            session.handle_datagram(&[1, 2, 3]),
            Err(Fault::Dropped(MoshError::CiphertextTooShort))
        ));
    }
}
