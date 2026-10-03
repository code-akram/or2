// SPDX-License-Identifier: GPL-3.0-or-later
// Vendored from mosh-rs (https://github.com/wilsonglasser/mosh-rs), commit
// 90b37125f5e4a598be91dec37d23921b6865276e, src/sender.rs. Upstream: GPL-3.0-or-later, copyright
// Wilson Glasser; the protocol logic follows mosh (Keith Winstein and contributors, GPL-3.0-or-later).
// See THIRD_PARTY_NOTICES.md. or2 changes: module paths only (comments about prediction kept; or2 has prediction off), `TransportReceiver::forget`, then rustfmt.
//! The transport state machine (`transportsender-impl.h`,
//! `networktransport-impl.h`): what turns instructions into a session
//! that keeps itself alive.
//!
//! Both sides number their states from 0, an initial state neither
//! transmits. Every instruction says which state its diff starts from
//! (`old_num`) and which it produces (`new_num`); the receiver applies
//! it only if it already holds `old_num`, which is what makes the
//! protocol idempotent under duplication and reordering. There is no
//! retransmission of packets, only of STATE: a resend recomputes the
//! diff from whatever the peer is now believed to hold.
//!
//! The timers are mosh's, to the millisecond, because they are what
//! make a session survive a bad link without flooding a good one.

use super::statesync::UserStream;
use super::transport::{Instruction, SHUTDOWN_NUM};

/// Bounds on the gap between frames; the real interval is half the
/// smoothed RTT, so a fast link sends more often than a slow one.
pub const SEND_INTERVAL_MIN_MS: u64 = 20;
/// The slowest a session ever sends, however bad the link.
pub const SEND_INTERVAL_MAX_MS: u64 = 250;
/// Gap between bare acks when nothing else is happening: the heartbeat
/// that tells the peer we are still here.
pub const ACK_INTERVAL_MS: u64 = 3000;
/// How long an ack waits for data to ride along with.
pub const ACK_DELAY_MS: u64 = 100;
/// How long input is collected before a frame goes out, so a burst of
/// typing costs one packet instead of one per key.
pub const SEND_MINDELAY_MS: u64 = 8;
/// After this long with nothing heard, stop retrying at frame rate.
pub const ACTIVE_RETRY_TIMEOUT_MS: u64 = 10000;
/// How many shutdown instructions to send before giving up on ever
/// being acknowledged.
pub const SHUTDOWN_RETRIES: u32 = 16;

/// How many un-thrown-away peer states to hold before refusing more,
/// and how long to refuse for once the limit is hit.
const MAX_RECEIVED_STATES: usize = 1024;
const RECEIVER_QUENCH_MS: u64 = 15000;

/// A prophylactic resend is taken when it is no bigger than the diff
/// we were going to send anyway, or when it stays under this size and
/// costs less than the slack below.
const PROSPECTIVE_RESEND_MAX: usize = 1000;
const PROSPECTIVE_RESEND_SLACK: usize = 100;

/// Random padding appended to every instruction, 0 to this many bytes,
/// so the length of a datagram says less about what was typed.
const CHAFF_MAX: usize = 16;

/// A few bytes of noise. mosh sends this on every instruction and it
/// costs nothing to match: a keystroke and a screenful of output should
/// not be trivially distinguishable by datagram size alone.
fn make_chaff() -> Vec<u8> {
    use ocb3::aead::rand_core::RngCore as _;
    let mut buf = [0u8; CHAFF_MAX + 1];
    ocb3::aead::OsRng.fill_bytes(&mut buf);
    let len = usize::from(buf[CHAFF_MAX]) % (CHAFF_MAX + 1);
    buf[..len].to_vec()
}

/// A state we sent, kept until the peer acknowledges it.
#[derive(Debug, Clone)]
struct SentState {
    timestamp: u64,
    num: u64,
    state: UserStream,
}

/// The client half of the transport: owns the outgoing state history,
/// the acknowledgement bookkeeping and the timers.
#[derive(Debug)]
pub struct TransportSender {
    /// States sent and not yet known-acked. The front is the oldest
    /// the peer might still need; the back is the newest we sent.
    sent_states: Vec<SentState>,
    /// The live state, ahead of everything sent.
    current_state: UserStream,
    /// Index into `sent_states` of the state the peer is assumed to
    /// hold. Diffs are computed from it.
    assumed_receiver: usize,
    /// Highest peer state we hold, echoed back as `ack_num`.
    ack_num: u64,
    /// Set when the peer sent real data, which we should acknowledge
    /// promptly rather than at the next heartbeat.
    pending_data_ack: bool,
    next_ack_time: u64,
    next_send_time: Option<u64>,
    /// When the current state first diverged from the last sent one.
    mindelay_clock: Option<u64>,
    last_heard: u64,
    shutting_down: bool,
    /// How many shutdown instructions have gone out, and when the
    /// handshake began: a peer that never answers must not keep the
    /// client waiting forever.
    shutdown_tries: u32,
    shutdown_start: Option<u64>,
}

impl Default for TransportSender {
    fn default() -> Self {
        Self::new()
    }
}

impl TransportSender {
    /// A sender holding only the empty state both sides start from.
    pub fn new() -> Self {
        Self {
            // State 0 is the empty stream both sides start from.
            sent_states: vec![SentState {
                timestamp: 0,
                num: 0,
                state: UserStream::new(),
            }],
            current_state: UserStream::new(),
            assumed_receiver: 0,
            ack_num: 0,
            pending_data_ack: false,
            next_ack_time: 0,
            next_send_time: None,
            mindelay_clock: None,
            last_heard: 0,
            shutting_down: false,
            shutdown_tries: 0,
            shutdown_start: None,
        }
    }

    /// The live user state, to push keystrokes and resizes into.
    pub fn state_mut(&mut self) -> &mut UserStream {
        &mut self.current_state
    }

    /// The peer state our next instruction will acknowledge.
    #[cfg(test)]
    pub fn ack_num(&self) -> u64 {
        self.ack_num
    }

    /// Record that the peer holds up to `num` of OUR states, dropping
    /// everything older. An ack for a state we already culled is
    /// ignored rather than trusted.
    pub fn process_acknowledgement(&mut self, num: u64) {
        if !self.sent_states.iter().any(|s| s.num == num) {
            return;
        }
        let keep_from = self
            .sent_states
            .iter()
            .position(|s| s.num >= num)
            .unwrap_or(0);
        self.sent_states.drain(..keep_from);
        self.assumed_receiver = self.assumed_receiver.saturating_sub(keep_from);
    }

    /// Note a state of the PEER that we now hold, which our next
    /// instruction will acknowledge. `has_data` marks a diff that
    /// carried real content, which is acked sooner.
    pub fn set_ack_num(&mut self, num: u64, has_data: bool, now: u64) {
        self.ack_num = num;
        self.last_heard = now;
        if has_data {
            self.pending_data_ack = true;
        }
    }

    /// Milliseconds between frames: half the smoothed RTT, clamped.
    /// This is mosh's `send_interval()`, and it is more than a timer:
    /// the prediction engine reads it as its estimate of how slow the
    /// link is, and decides whether to show predictions at all from it.
    pub fn interval(srtt_ms: f64) -> u64 {
        ((srtt_ms / 2.0).ceil() as u64).clamp(SEND_INTERVAL_MIN_MS, SEND_INTERVAL_MAX_MS)
    }

    /// When we last heard anything at all from the peer.
    pub fn last_heard(&self) -> u64 {
        self.last_heard
    }

    /// When we SENT the newest state the peer has acknowledged
    /// (`get_sent_state_acked_timestamp`). How long ago that was is how
    /// long it has been since a round trip actually completed, which is
    /// what tells the network layer the link is failing and it is time
    /// to look for a new source port.
    pub fn sent_state_acked_timestamp(&self) -> u64 {
        self.sent_states.first().expect("never empty").timestamp
    }

    /// Which of our sent states the peer is ASSUMED to hold: the newest
    /// one sent recently enough that its acknowledgement could still be
    /// in flight. Everything older has had its chance, so the
    /// assumption falls back to what was actually acknowledged.
    ///
    /// This is what recovers from loss, and leaving it out does not
    /// show on a link that never drops anything. Without it, a lost
    /// state is assumed held forever: every later diff keeps being
    /// computed from a state the peer never received, and the peer
    /// refuses a diff it has nothing to apply to. The session wedges.
    fn update_assumed_receiver_state(&mut self, now: u64, rto_ms: f64) {
        let window = rto_ms as u64 + ACK_DELAY_MS;
        self.assumed_receiver = 0;
        for (idx, state) in self.sent_states.iter().enumerate().skip(1) {
            if now.saturating_sub(state.timestamp) < window {
                self.assumed_receiver = idx;
            } else {
                return;
            }
        }
    }

    /// Drop the prefix the peer is known to hold from every state we
    /// keep, the live one included.
    ///
    /// Without this the live state accumulates every byte the user has
    /// ever typed: nothing is ever wrong, but each diff walks the whole
    /// history and the session's memory grows without bound.
    fn rationalize_states(&mut self) {
        let known = self.sent_states.first().expect("never empty").state.clone();
        self.current_state.subtract(&known);
        for state in &mut self.sent_states {
            state.state.subtract(&known);
        }
    }

    /// Recompute when the next packet is due. Mirrors
    /// `calculate_timers`: fresh input goes out after a short collect
    /// delay, an unacknowledged send retries at frame rate while the
    /// peer is still answering, and everything falls back to the
    /// heartbeat once it is not.
    pub fn calculate_timers(&mut self, now: u64, srtt_ms: f64, rto_ms: f64) {
        // Both of these run before any timer is looked at, exactly as
        // mosh's `calculate_timers` does: what the peer is assumed to
        // hold decides what the next diff is computed FROM, and the
        // known-held prefix is dead weight in every state we keep.
        self.update_assumed_receiver_state(now, rto_ms);
        self.rationalize_states();

        if self.pending_data_ack && self.next_ack_time > now + ACK_DELAY_MS {
            self.next_ack_time = now + ACK_DELAY_MS;
        }
        let interval = Self::interval(srtt_ms);
        let last_sent = self.sent_states.last().expect("never empty");
        let active = self.last_heard + ACTIVE_RETRY_TIMEOUT_MS > now;

        if self.current_state != last_sent.state {
            // New input: collect for a moment so a burst of typing
            // costs one frame, but never send faster than frame rate.
            let mindelay = *self.mindelay_clock.get_or_insert(now);
            self.next_send_time =
                Some((mindelay + SEND_MINDELAY_MS).max(last_sent.timestamp + interval));
        } else if self.current_state != self.sent_states[self.assumed_receiver].state && active {
            // Sent, not yet believed delivered: retry at frame rate.
            let mut t = last_sent.timestamp + interval;
            if let Some(mindelay) = self.mindelay_clock {
                t = t.max(mindelay + SEND_MINDELAY_MS);
            }
            self.next_send_time = Some(t);
        } else if self.current_state != self.sent_states[0].state && active {
            // Nothing acknowledged at all: back off to the RTO.
            self.next_send_time = Some(last_sent.timestamp + rto_ms as u64 + ACK_DELAY_MS);
        } else {
            self.next_send_time = None;
        }

        if self.shutting_down || self.ack_num == SHUTDOWN_NUM {
            self.next_ack_time = last_sent.timestamp + interval;
        }
    }

    /// When the next packet is due, if any: the earlier of the send
    /// and ack deadlines.
    pub fn next_due(&self) -> Option<u64> {
        match self.next_send_time {
            Some(send) => Some(send.min(self.next_ack_time)),
            None => Some(self.next_ack_time),
        }
    }

    /// How long there is to wait before something needs sending, in
    /// milliseconds from `now`.
    ///
    /// This is mosh's `wait_time`, and like mosh's it RECALCULATES the
    /// timers before reading them. That is the whole point of it. A
    /// keystroke queued a moment ago has not moved any deadline yet, so
    /// asking without recalculating gives the answer from before the
    /// keystroke existed, and a caller that waits that long holds the
    /// keystroke for the length of an idle poll before sending it.
    pub fn wait_time(&mut self, now: u64, srtt_ms: f64, rto_ms: f64) -> u64 {
        self.calculate_timers(now, srtt_ms, rto_ms);
        self.next_due()
            .map_or(u64::MAX, |due| due.saturating_sub(now))
    }

    /// Build the instruction due at `now`, or `None` if nothing is.
    ///
    /// An empty diff still mints a new state number: that is how a
    /// heartbeat both proves liveness and gives the peer something to
    /// acknowledge.
    pub fn tick(&mut self, now: u64, srtt_ms: f64, rto_ms: f64) -> Option<Instruction> {
        self.calculate_timers(now, srtt_ms, rto_ms);
        let ack_due = now >= self.next_ack_time;
        let send_due = self.next_send_time.is_some_and(|t| now >= t);
        if !ack_due && !send_due {
            return None;
        }

        let mut diff = self
            .current_state
            .diff_from(&self.sent_states[self.assumed_receiver].state)?;

        // A prophylactic resend. If the peer is not assumed to hold our
        // OLDEST state either, then a diff computed from THAT one
        // repairs whatever was lost in between, at no extra round trip.
        // mosh takes it whenever it is not much bigger, which on a
        // keystroke-sized stream is nearly always.
        if self.assumed_receiver != 0
            && let Some(resend) = self.current_state.diff_from(&self.sent_states[0].state)
            && (resend.len() <= diff.len()
                || (resend.len() < PROSPECTIVE_RESEND_MAX
                    && resend.len().saturating_sub(diff.len()) < PROSPECTIVE_RESEND_SLACK))
        {
            self.assumed_receiver = 0;
            diff = resend;
        }

        let assumed = &self.sent_states[self.assumed_receiver];
        let empty = diff.is_empty();
        if empty && !ack_due {
            // Nothing to say and no ack owed: clear the send timer and
            // wait for the heartbeat.
            self.next_send_time = None;
            self.mindelay_clock = None;
            return None;
        }

        let old_num = assumed.num;
        let last = self.sent_states.last().expect("never empty");
        let new_num = if self.shutting_down {
            SHUTDOWN_NUM
        } else if empty {
            // A bare ack always mints a new number, even though it
            // carries nothing: that is what gives the peer a state to
            // acknowledge and so proves the link is alive in both
            // directions (`send_empty_ack`).
            last.num + 1
        } else if self.current_state == last.state {
            // A resend of a state already minted, because the peer is
            // not believed to hold it yet.
            last.num
        } else {
            last.num + 1
        };

        if new_num == last.num {
            // Retransmitting the state we already minted.
            self.sent_states.last_mut().expect("never empty").timestamp = now;
        } else {
            self.sent_states.push(SentState {
                timestamp: now,
                num: new_num,
                state: self.current_state.clone(),
            });
            // mosh caps the history so a peer that stops acking cannot
            // grow it without bound, dropping from the middle: the
            // oldest is what a resend still needs.
            if self.sent_states.len() > 32 {
                let drop = self.sent_states.len() - 16;
                self.sent_states.remove(drop);
            }
        }
        if !empty {
            // Only the DATA path moves the assumption forward
            // (`send_to_receiver` does, `send_empty_ack` does not), and
            // the next round recomputes it from the timestamps anyway.
            self.assumed_receiver = self.sent_states.len() - 1;
        }
        if new_num == SHUTDOWN_NUM {
            self.shutdown_tries += 1;
        }
        self.next_ack_time = now + ACK_INTERVAL_MS;
        self.next_send_time = None;
        self.mindelay_clock = None;
        self.pending_data_ack = false;

        Some(Instruction::new(
            old_num,
            new_num,
            self.ack_num,
            self.sent_states[0].num,
            diff,
            make_chaff(),
        ))
    }

    /// Begin the shutdown handshake: from here every instruction
    /// carries the shutdown state number until the peer acknowledges
    /// it (or the retries run out).
    pub fn start_shutdown(&mut self, now: u64) {
        // Guarded, so calling it twice does not restart the clock that
        // decides when to give up.
        if !self.shutting_down {
            self.shutting_down = true;
            self.shutdown_start = Some(now);
        }
    }

    /// Give up on ever being acknowledged: enough tries, or long
    /// enough. Without this a client saying goodbye to a server that
    /// has already gone waits forever.
    pub fn shutdown_timed_out(&self, now: u64) -> bool {
        let Some(start) = self.shutdown_start else {
            return false;
        };
        self.shutdown_tries >= SHUTDOWN_RETRIES
            || now.saturating_sub(start) >= ACTIVE_RETRY_TIMEOUT_MS
    }

    /// True once the peer has acknowledged our shutdown state.
    pub fn shutdown_acknowledged(&self) -> bool {
        self.sent_states[0].num == SHUTDOWN_NUM
    }
}

/// The receiving half: the peer's states, kept so an instruction's
/// `old_num` can be resolved, and so a duplicate is recognized.
#[derive(Debug)]
pub struct TransportReceiver {
    /// State numbers held, ascending. The client applies host diffs to
    /// its terminal rather than keeping copies, so only the numbers
    /// matter here.
    received: Vec<u64>,
    /// While set in the future, a full queue refuses more states.
    quench_until: u64,
}

impl Default for TransportReceiver {
    fn default() -> Self {
        Self::new()
    }
}

/// What a received instruction turned out to be.
#[derive(Debug, PartialEq, Eq)]
pub enum Received {
    /// A new state, newer than anything held: apply its diff and
    /// acknowledge it.
    Apply {
        /// The state number to acknowledge.
        num: u64,
        /// Its diff, to apply.
        diff: Vec<u8>,
    },
    /// A new state that arrived out of order, so something newer is
    /// already held. Apply it, but do NOT acknowledge: mosh returns
    /// before touching the acknowledgement, because naming a number
    /// below the highest we hold would ask the peer to resend what we
    /// already have.
    ApplyOutOfOrder {
        /// The state number, which is BELOW the highest held.
        num: u64,
        /// Its diff, to apply.
        diff: Vec<u8>,
    },
    /// Already seen; acknowledge again but do not re-apply.
    Duplicate,
    /// Its `old_num` is not a state we hold, so the diff cannot be
    /// applied to anything. Dropped, deliberately: this is what makes
    /// replay and reordering harmless.
    Unresolvable,
}

impl TransportReceiver {
    /// A receiver holding only state 0, the blank terminal.
    pub fn new() -> Self {
        // State 0 is the empty terminal both sides start from.
        Self {
            received: vec![0],
            quench_until: 0,
        }
    }

    /// The highest contiguous state held, which is what gets acked.
    pub fn latest(&self) -> u64 {
        *self.received.last().expect("never empty")
    }

    /// Un-receive a state whose diff could not be applied, so it is neither acknowledged nor
    /// treated as a duplicate when the peer sends it again. State 0 never goes.
    pub fn forget(&mut self, num: u64) {
        if num != 0 {
            self.received.retain(|n| *n != num);
            if self.received.is_empty() {
                self.received.push(0);
            }
        }
    }

    /// Decide what an arriving instruction turned out to be.
    pub fn process(&mut self, inst: &Instruction, now: u64) -> Received {
        let (Some(old_num), Some(new_num)) = (inst.old_num, inst.new_num) else {
            return Received::Unresolvable;
        };
        if self.received.contains(&new_num) {
            return Received::Duplicate;
        }
        if !self.received.contains(&old_num) {
            return Received::Unresolvable;
        }
        if let Some(throwaway) = inst.throwaway_num {
            self.received.retain(|n| *n >= throwaway);
            if self.received.is_empty() {
                self.received.push(old_num);
            }
        }

        // A peer that keeps minting states we can never throw away is
        // either malicious or on a link that only works one way. mosh
        // refuses to grow past this, and only reconsiders after a
        // quiet period, so a burst cannot walk the limit upward.
        if self.received.len() > MAX_RECEIVED_STATES {
            if now < self.quench_until {
                return Received::Unresolvable;
            }
            self.quench_until = now + RECEIVER_QUENCH_MS;
        }

        let in_order = new_num > self.latest();
        let idx = self.received.partition_point(|n| *n < new_num);
        self.received.insert(idx, new_num);
        let diff = inst.diff.clone().unwrap_or_default();
        if in_order {
            Received::Apply { num: new_num, diff }
        } else {
            Received::ApplyOutOfOrder { num: new_num, diff }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRTT: f64 = 100.0;
    const RTO: f64 = 300.0;

    #[test]
    fn nothing_to_say_still_heartbeats() {
        let mut s = TransportSender::new();
        // The very first tick is due (next_ack_time starts at 0), and
        // an empty diff still mints a state so the peer has something
        // to acknowledge.
        let inst = s.tick(0, SRTT, RTO).expect("heartbeat");
        assert_eq!(inst.new_num, Some(1));
        assert_eq!(inst.diff.as_deref().map(<[u8]>::len), Some(0));
        // And then it goes quiet until the heartbeat interval.
        assert!(s.tick(10, SRTT, RTO).is_none());
        assert!(s.tick(ACK_INTERVAL_MS - 1, SRTT, RTO).is_none());
        assert!(s.tick(ACK_INTERVAL_MS, SRTT, RTO).is_some());
    }

    #[test]
    fn typing_waits_the_collect_delay_then_goes_out_once() {
        let mut s = TransportSender::new();
        s.tick(0, SRTT, RTO); // clear the initial heartbeat
        s.state_mut().push_bytes(b"l");
        // Not yet: the collect window is still open.
        assert!(s.tick(1, SRTT, RTO).is_none());
        // More keys land inside the window.
        s.state_mut().push_bytes(b"s");
        let inst = s.tick(60, SRTT, RTO).expect("due after the interval");
        // One instruction carries the whole burst.
        let diff = inst.diff.expect("diff");
        let typed = crate::mosh::ssp::statesync::UserMessage::keystrokes_of(&diff).unwrap();
        assert_eq!(typed, b"ls".to_vec());
    }

    #[test]
    fn an_acknowledgement_drops_the_states_before_it() {
        let mut s = TransportSender::new();
        s.state_mut().push_bytes(b"a");
        let first = s.tick(100, SRTT, RTO).expect("send");
        assert_eq!(first.new_num, Some(1));

        s.state_mut().push_bytes(b"b");
        // Inside the collect window: mosh would rather wait 8 ms and
        // send one frame than send a packet per keystroke.
        assert!(s.tick(400, SRTT, RTO).is_none(), "still collecting");
        let second = s.tick(420, SRTT, RTO).expect("send");
        assert_eq!(second.new_num, Some(2));

        s.process_acknowledgement(2);
        // Only state 2 is worth keeping now, and it becomes the
        // throwaway floor we advertise.
        let inst = s.tick(4000, SRTT, RTO).expect("heartbeat");
        assert_eq!(inst.throwaway_num, Some(2));
    }

    #[test]
    fn an_acknowledgement_for_an_unknown_state_is_ignored() {
        let mut s = TransportSender::new();
        s.state_mut().push_bytes(b"a");
        s.tick(100, SRTT, RTO);
        s.process_acknowledgement(99);
        let inst = s.tick(4000, SRTT, RTO).expect("heartbeat");
        assert_eq!(inst.throwaway_num, Some(0), "history untouched");
    }

    #[test]
    fn a_cheap_resend_from_the_oldest_state_is_preferred() {
        let mut s = TransportSender::new();
        s.state_mut().push_bytes(b"ab");
        let first = s.tick(100, SRTT, RTO).expect("send");
        assert_eq!(first.old_num, Some(0));
        s.state_mut().push_bytes(b"cd");
        // 8 ms of collect after the input, and a frame interval after
        // the previous send: 420 clears both.
        assert!(s.tick(400, SRTT, RTO).is_none(), "still collecting");
        let second = s.tick(420, SRTT, RTO).expect("send");
        // State 1 has not been acknowledged, so a diff computed from
        // state 0 instead costs two extra bytes and REPAIRS state 1 if
        // it never arrived. mosh takes that trade whenever it is this
        // cheap, which on a keystroke stream is nearly always.
        assert_eq!(second.old_num, Some(0));
        let typed =
            crate::mosh::ssp::statesync::UserMessage::keystrokes_of(&second.diff.unwrap()).unwrap();
        assert_eq!(typed, b"abcd".to_vec());
    }

    #[test]
    fn an_expensive_resend_is_declined() {
        let mut s = TransportSender::new();
        // Far more history than the prophylactic resend is allowed to
        // carry, so the narrow diff wins instead.
        s.state_mut().push_bytes(&vec![b'x'; 2000]);
        s.tick(100, SRTT, RTO).expect("send");
        s.state_mut().push_bytes(b"cd");
        assert!(s.tick(400, SRTT, RTO).is_none(), "still collecting");
        let second = s.tick(420, SRTT, RTO).expect("send");
        assert_eq!(
            second.old_num,
            Some(1),
            "resending 2 KB to repair is not worth it"
        );
        let typed =
            crate::mosh::ssp::statesync::UserMessage::keystrokes_of(&second.diff.unwrap()).unwrap();
        assert_eq!(typed, b"cd".to_vec());
    }

    #[test]
    fn an_unanswered_state_stops_being_assumed_held() {
        let mut s = TransportSender::new();
        s.state_mut().push_bytes(b"ab");
        s.tick(100, SRTT, RTO).expect("send");
        // Long enough that an acknowledgement for state 1 cannot still
        // be in flight: the assumption has to fall back to state 0, or
        // every later diff is computed from something the peer may
        // never have received.
        s.update_assumed_receiver_state(100 + RTO as u64 + ACK_DELAY_MS + 1, RTO);
        assert_eq!(s.assumed_receiver, 0);
        // While it is still plausibly in flight, it stays assumed.
        s.update_assumed_receiver_state(150, RTO);
        assert_eq!(s.assumed_receiver, 1);
    }

    #[test]
    fn an_acknowledged_prefix_stops_being_carried_around() {
        let mut s = TransportSender::new();
        s.state_mut().push_bytes(b"hello");
        s.tick(100, SRTT, RTO).expect("send");
        assert_eq!(s.current_state.len(), 5);
        // The peer confirms state 1, so those five keystrokes are
        // history nobody needs again.
        s.process_acknowledgement(1);
        s.calculate_timers(200, SRTT, RTO);
        assert_eq!(
            s.current_state.len(),
            0,
            "an acknowledged prefix must not stay in the live state forever"
        );
    }

    #[test]
    fn every_instruction_carries_a_little_noise() {
        let mut s = TransportSender::new();
        s.state_mut().push_bytes(b"a");
        let inst = s.tick(100, SRTT, RTO).expect("send");
        // Length only: the point of chaff is that it varies, so the
        // only thing to assert is that the field is there to vary.
        assert!(inst.chaff.is_some());
        assert!(inst.chaff.unwrap().len() <= CHAFF_MAX);
    }

    #[test]
    fn a_shutdown_nobody_answers_eventually_gives_up() {
        let mut s = TransportSender::new();
        s.start_shutdown(1_000);
        assert!(!s.shutdown_timed_out(1_000));
        // Either enough tries...
        for i in 0..SHUTDOWN_RETRIES {
            s.tick(2_000 + u64::from(i) * ACK_INTERVAL_MS, SRTT, RTO);
        }
        assert!(s.shutdown_timed_out(2_000));
        // ...or long enough.
        let mut s = TransportSender::new();
        s.start_shutdown(1_000);
        assert!(!s.shutdown_timed_out(1_000 + ACTIVE_RETRY_TIMEOUT_MS - 1));
        assert!(s.shutdown_timed_out(1_000 + ACTIVE_RETRY_TIMEOUT_MS));
    }

    #[test]
    fn a_state_that_arrives_late_is_applied_but_not_acknowledged() {
        let mut r = TransportReceiver::new();
        r.process(
            &Instruction::new(0, 1, 0, 0, b"one".to_vec(), Vec::new()),
            0,
        );
        r.process(
            &Instruction::new(1, 5, 0, 0, b"five".to_vec(), Vec::new()),
            0,
        );
        assert_eq!(r.latest(), 5);
        // State 3 turns up after 5. It still has to be applied (the
        // server may diff from it later), but acknowledging it would
        // name a state older than what we hold and ask for a resend of
        // things already on screen.
        let late = r.process(
            &Instruction::new(1, 3, 0, 0, b"three".to_vec(), Vec::new()),
            0,
        );
        assert!(matches!(late, Received::ApplyOutOfOrder { num: 3, .. }));
        assert_eq!(r.latest(), 5, "the highest held has not moved");
    }

    #[test]
    fn shutdown_states_carry_the_sentinel_number() {
        let mut s = TransportSender::new();
        s.tick(0, SRTT, RTO);
        s.start_shutdown(0);
        s.state_mut().push_bytes(b"x");
        let inst = s.tick(100, SRTT, RTO).expect("send");
        assert_eq!(inst.new_num, Some(SHUTDOWN_NUM));
        assert!(!s.shutdown_acknowledged());
    }

    #[test]
    fn the_receiver_applies_a_new_state_once() {
        let mut r = TransportReceiver::new();
        let inst = Instruction::new(0, 1, 0, 0, b"diff".to_vec(), Vec::new());
        assert_eq!(
            r.process(&inst, 0),
            Received::Apply {
                num: 1,
                diff: b"diff".to_vec()
            }
        );
        assert_eq!(r.latest(), 1);
        // The same instruction again is a duplicate, not a re-apply:
        // applying a terminal diff twice would corrupt the screen.
        assert_eq!(r.process(&inst, 0), Received::Duplicate);
    }

    #[test]
    fn the_receiver_drops_a_diff_it_cannot_resolve() {
        let mut r = TransportReceiver::new();
        // Claims to start from state 7, which we never had.
        let inst = Instruction::new(7, 8, 0, 0, b"diff".to_vec(), Vec::new());
        assert_eq!(r.process(&inst, 0), Received::Unresolvable);
        assert_eq!(r.latest(), 0);
    }

    #[test]
    fn throwaway_prunes_the_receiver_history() {
        let mut r = TransportReceiver::new();
        r.process(&Instruction::new(0, 1, 0, 0, vec![], Vec::new()), 0);
        r.process(&Instruction::new(1, 2, 0, 0, vec![], Vec::new()), 0);
        // The peer says it will never diff from anything below 2.
        r.process(&Instruction::new(2, 3, 0, 2, vec![], Vec::new()), 0);
        assert_eq!(r.received, vec![2, 3]);
    }

    #[test]
    fn send_interval_is_half_the_rtt_within_bounds() {
        assert_eq!(TransportSender::interval(100.0), 50);
        // A very fast link still waits the floor.
        assert_eq!(TransportSender::interval(4.0), SEND_INTERVAL_MIN_MS);
        // A very slow one still sends at the ceiling.
        assert_eq!(TransportSender::interval(5000.0), SEND_INTERVAL_MAX_MS);
    }
}
