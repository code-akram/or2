// SPDX-License-Identifier: GPL-3.0-or-later
// Vendored from mosh-rs (https://github.com/wilsonglasser/mosh-rs), commit
// 90b37125f5e4a598be91dec37d23921b6865276e, src/packet.rs. Upstream: GPL-3.0-or-later, copyright
// Wilson Glasser; the protocol logic follows mosh (Keith Winstein and contributors, GPL-3.0-or-later).
// See THIRD_PARTY_NOTICES.md. or2 changes: module paths only, then rustfmt.
//! The packet layer: what sits inside every encrypted datagram, and
//! the connection bookkeeping around it (`network.cc`).
//!
//! Plaintext layout, ahead of the transport payload:
//!
//! ```text
//! [ 2 bytes: timestamp       BE u16 ]
//! [ 2 bytes: timestamp_reply BE u16 ]
//! [ rest:    payload                ]
//! ```
//!
//! Both fields are milliseconds mod 65536, with `0xFFFF` reserved as
//! "no timestamp", which is why `timestamp16` bumps a real clock
//! reading of 65535 to 0 rather than sending the sentinel by accident.
//! The pair is what mosh measures RTT with: a peer echoes the last
//! timestamp it heard, adjusted for how long it sat on it.

use super::error::{MoshError, Result};

/// The "no timestamp" sentinel in both fields.
pub const TIMESTAMP_NONE: u16 = u16::MAX;

/// A received timestamp older than this is not echoed (`new_packet`).
pub const SAVED_TIMESTAMP_FRESHNESS_MS: u64 = 1000;

/// RTT samples at or above this are discarded as outliers (a peer that
/// was stopped, a suspended laptop).
pub const RTT_OUTLIER_MS: f64 = 5000.0;

/// RTO clamp (`network.h`).
pub const MIN_RTO_MS: f64 = 50.0;
/// Ceiling on the retransmission timeout.
pub const MAX_RTO_MS: f64 = 1000.0;

/// mosh's congestion response: when a datagram arrives ECN-marked, the
/// saved timestamp is aged by this much so the echo inflates the
/// peer's measured RTT and slows it down.
pub const CONGESTION_TIMESTAMP_PENALTY_MS: u16 = 500;

/// A packet's plaintext: the two timestamps plus the transport payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    /// Our clock when this was sent, in milliseconds mod 65536.
    pub timestamp: u16,
    /// The peer's timestamp, advanced by however long we sat on it.
    /// [`TIMESTAMP_NONE`] when there is nothing to echo.
    pub timestamp_reply: u16,
    /// One transport fragment.
    pub payload: Vec<u8>,
}

impl Packet {
    /// Serialize to the plaintext that goes under the AEAD.
    pub fn to_plaintext(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + self.payload.len());
        out.extend_from_slice(&self.timestamp.to_be_bytes());
        out.extend_from_slice(&self.timestamp_reply.to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// Parse a decrypted plaintext. Anything shorter than the two
    /// timestamps is a forged or truncated packet, which mosh drops
    /// with `dos_assert` rather than tearing the session down.
    pub fn from_plaintext(text: &[u8]) -> Result<Self> {
        if text.len() < 4 {
            return Err(MoshError::PayloadTooShort);
        }
        Ok(Self {
            timestamp: u16::from_be_bytes([text[0], text[1]]),
            timestamp_reply: u16::from_be_bytes([text[2], text[3]]),
            payload: text[4..].to_vec(),
        })
    }
}

/// Milliseconds mod 65536, never the sentinel (`timestamp16`).
pub fn timestamp16(now_ms: u64) -> u16 {
    let ts = (now_ms % 65536) as u16;
    if ts == TIMESTAMP_NONE { 0 } else { ts }
}

/// Difference of two 16-bit timestamps, wrapping (`timestamp_diff`).
pub fn timestamp_diff(newer: u16, older: u16) -> u16 {
    newer.wrapping_sub(older)
}

/// Round-trip time estimator, RFC 6298 shaped, with mosh's initial
/// values and outlier rule.
#[derive(Debug, Clone)]
pub struct RttEstimator {
    srtt: f64,
    rttvar: f64,
    hit: bool,
}

impl Default for RttEstimator {
    fn default() -> Self {
        // mosh's constructor values: a pessimistic second, halved
        // variance, until the first real sample replaces both.
        Self {
            srtt: 1000.0,
            rttvar: 500.0,
            hit: false,
        }
    }
}

impl RttEstimator {
    /// Fold in one sample. Samples at or above the outlier cutoff are
    /// ignored entirely, as are echoes of the sentinel.
    pub fn sample(&mut self, rtt_ms: f64) {
        if rtt_ms >= RTT_OUTLIER_MS {
            return;
        }
        if !self.hit {
            self.srtt = rtt_ms;
            self.rttvar = rtt_ms / 2.0;
            self.hit = true;
            return;
        }
        const ALPHA: f64 = 1.0 / 8.0;
        const BETA: f64 = 1.0 / 4.0;
        self.rttvar = (1.0 - BETA) * self.rttvar + BETA * (self.srtt - rtt_ms).abs();
        self.srtt = (1.0 - ALPHA) * self.srtt + ALPHA * rtt_ms;
    }

    /// The smoothed round-trip time, which also sets the frame rate.
    pub fn srtt(&self) -> f64 {
        self.srtt
    }

    /// Retransmission timeout, clamped the way `Connection::timeout`
    /// clamps it.
    pub fn rto(&self) -> f64 {
        (self.srtt + 4.0 * self.rttvar)
            .ceil()
            .clamp(MIN_RTO_MS, MAX_RTO_MS)
    }
}

/// The per-connection state a client keeps around its packets:
/// outgoing sequence numbers, the timestamp it owes the peer, the
/// highest sequence it has accepted, and the RTT estimate.
#[derive(Debug, Default)]
pub struct PacketState {
    next_seq: u64,
    saved_timestamp: Option<u16>,
    saved_timestamp_received_at: u64,
    expected_receiver_seq: u64,
    /// What the round trips have measured so far.
    pub rtt: RttEstimator,
}

/// One outgoing packet, ready for the crypto layer.
#[derive(Debug)]
pub struct Outgoing {
    /// The sequence number to encrypt under.
    pub seq: u64,
    /// The packet itself, timestamps already stamped.
    pub packet: Packet,
}

impl PacketState {
    /// Build the next outgoing packet for `payload` (`new_packet`).
    ///
    /// The timestamp is always fresh. The reply is the peer's last
    /// timestamp advanced by however long we sat on it, and only while
    /// that is under a second: an older echo would measure our own idle
    /// time, not the network. Either way the saved value is consumed,
    /// so each received timestamp is echoed at most once.
    pub fn new_packet(&mut self, now_ms: u64, payload: Vec<u8>) -> Outgoing {
        let timestamp_reply = match self.saved_timestamp.take() {
            Some(saved)
                if now_ms.saturating_sub(self.saved_timestamp_received_at)
                    < SAVED_TIMESTAMP_FRESHNESS_MS =>
            {
                let held = (now_ms - self.saved_timestamp_received_at) as u16;
                saved.wrapping_add(held)
            }
            _ => TIMESTAMP_NONE,
        };
        self.saved_timestamp_received_at = 0;

        let seq = self.next_seq;
        self.next_seq += 1;
        Outgoing {
            seq,
            packet: Packet {
                timestamp: timestamp16(now_ms),
                timestamp_reply,
                payload,
            },
        }
    }

    /// Fold in a received packet: remember its timestamp to echo,
    /// sample RTT from its echo of ours, and advance the accepted
    /// sequence. Returns whether the packet is NEW (an older sequence
    /// still has its payload delivered, but must not touch timing or
    /// targeting: a replay could otherwise steer both).
    pub fn accept(
        &mut self,
        now_ms: u64,
        seq: u64,
        packet: &Packet,
        congestion_experienced: bool,
    ) -> bool {
        if seq < self.expected_receiver_seq {
            return false;
        }
        self.expected_receiver_seq = seq + 1;

        if packet.timestamp != TIMESTAMP_NONE {
            let saved = if congestion_experienced {
                packet
                    .timestamp
                    .wrapping_sub(CONGESTION_TIMESTAMP_PENALTY_MS)
            } else {
                packet.timestamp
            };
            self.saved_timestamp = Some(saved);
            self.saved_timestamp_received_at = now_ms;
        }
        if packet.timestamp_reply != TIMESTAMP_NONE {
            let sample = timestamp_diff(timestamp16(now_ms), packet.timestamp_reply);
            self.rtt.sample(sample as f64);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_round_trips() {
        let p = Packet {
            timestamp: 0x1234,
            timestamp_reply: TIMESTAMP_NONE,
            payload: b"fragment".to_vec(),
        };
        let bytes = p.to_plaintext();
        assert_eq!(&bytes[..4], &[0x12, 0x34, 0xff, 0xff]);
        assert_eq!(Packet::from_plaintext(&bytes).unwrap(), p);
    }

    #[test]
    fn a_plaintext_without_both_timestamps_is_refused() {
        assert!(matches!(
            Packet::from_plaintext(&[0, 0, 0]),
            Err(MoshError::PayloadTooShort)
        ));
        // Exactly the two timestamps and no payload is legal.
        assert!(
            Packet::from_plaintext(&[0, 0, 0, 0])
                .unwrap()
                .payload
                .is_empty()
        );
    }

    #[test]
    fn timestamp16_never_emits_the_sentinel() {
        assert_eq!(timestamp16(65535), 0);
        assert_eq!(timestamp16(65536), 0);
        assert_eq!(timestamp16(1234), 1234);
        assert_eq!(timestamp16(65535 + 65536), 0);
    }

    #[test]
    fn timestamp_diff_wraps() {
        assert_eq!(timestamp_diff(10, 5), 5);
        // The clock wrapped between the two readings.
        assert_eq!(timestamp_diff(4, 65530), 10);
    }

    #[test]
    fn sequence_numbers_start_at_zero_and_advance() {
        let mut st = PacketState::default();
        assert_eq!(st.new_packet(0, vec![]).seq, 0);
        assert_eq!(st.new_packet(1, vec![]).seq, 1);
        assert_eq!(st.new_packet(2, vec![]).seq, 2);
    }

    #[test]
    fn a_fresh_timestamp_is_echoed_once_with_the_hold_time_added() {
        let mut st = PacketState::default();
        let heard = Packet {
            timestamp: 1000,
            timestamp_reply: TIMESTAMP_NONE,
            payload: vec![],
        };
        assert!(st.accept(5_000, 0, &heard, false));

        // 40 ms later: the echo carries their timestamp plus our hold.
        let out = st.new_packet(5_040, vec![]);
        assert_eq!(out.packet.timestamp_reply, 1040);
        // Consumed: the next packet has nothing left to echo.
        let out = st.new_packet(5_050, vec![]);
        assert_eq!(out.packet.timestamp_reply, TIMESTAMP_NONE);
    }

    #[test]
    fn a_stale_timestamp_is_not_echoed() {
        let mut st = PacketState::default();
        let heard = Packet {
            timestamp: 1000,
            timestamp_reply: TIMESTAMP_NONE,
            payload: vec![],
        };
        assert!(st.accept(5_000, 0, &heard, false));
        // A second later the echo would measure our idle time, not the
        // network, so mosh sends the sentinel instead.
        let out = st.new_packet(6_000, vec![]);
        assert_eq!(out.packet.timestamp_reply, TIMESTAMP_NONE);
    }

    #[test]
    fn congestion_ages_the_saved_timestamp() {
        let mut st = PacketState::default();
        let heard = Packet {
            timestamp: 1000,
            timestamp_reply: TIMESTAMP_NONE,
            payload: vec![],
        };
        assert!(st.accept(5_000, 0, &heard, true));
        let out = st.new_packet(5_000, vec![]);
        // 1000 - 500 penalty + 0 held: the peer measures 500 ms more.
        assert_eq!(out.packet.timestamp_reply, 500);
    }

    #[test]
    fn old_sequences_are_reported_as_not_new() {
        let mut st = PacketState::default();
        let p = Packet {
            timestamp: 1,
            timestamp_reply: TIMESTAMP_NONE,
            payload: vec![],
        };
        assert!(st.accept(0, 5, &p, false));
        // Replay of an older seq: not new, and it must not have moved
        // the saved timestamp (which the first accept already took).
        assert!(!st.accept(0, 4, &p, false));
        assert!(st.accept(0, 6, &p, false));
    }

    #[test]
    fn rtt_starts_pessimistic_and_converges() {
        let mut r = RttEstimator::default();
        assert_eq!(r.rto(), MAX_RTO_MS);
        r.sample(20.0);
        // First sample replaces both estimates outright.
        assert_eq!(r.srtt(), 20.0);
        for _ in 0..50 {
            r.sample(20.0);
        }
        assert!((r.srtt() - 20.0).abs() < 0.5);
        // Floor holds even when the link is much faster than it.
        assert_eq!(r.rto(), MIN_RTO_MS);
    }

    #[test]
    fn rtt_ignores_outliers() {
        let mut r = RttEstimator::default();
        r.sample(30.0);
        r.sample(9_000.0);
        assert_eq!(r.srtt(), 30.0);
    }
}
