// SPDX-License-Identifier: GPL-3.0-or-later
// Vendored from mosh-rs (https://github.com/wilsonglasser/mosh-rs), commit
// 90b37125f5e4a598be91dec37d23921b6865276e, src/crypto.rs. Upstream: GPL-3.0-or-later, copyright
// Wilson Glasser; the protocol logic follows mosh (Keith Winstein and contributors, GPL-3.0-or-later).
// See THIRD_PARTY_NOTICES.md. or2 changes: module paths only (`crate::` to `super::`), `Incoming` has a redacted `Debug`, then rustfmt.
//! The datagram crypto layer: AES-128-OCB3 exactly as mosh's
//! `Crypto::Session` uses it (`src/crypto/crypto.cc`).
//!
//! Wire datagram: `[ 8 bytes BE64 nonce value ][ OCB3 ciphertext ][ 16 byte tag ]`.
//! The nonce's top bit is the direction (1 = to-client, 0 = to-server)
//! and the low 63 bits are the sequence number; both peers count from 0
//! under the same key, so the direction bit is the only thing keeping
//! the two nonce streams disjoint. OCB is fed a 12-byte nonce: four
//! zero bytes then the eight wire bytes. No associated data.

use aes::Aes128;
use ocb3::aead::consts::{U12, U16};
use ocb3::{AeadInPlace, KeyInit, Ocb3};

use super::error::{MoshError, Result};
use super::key::Base64Key;

/// AES-128-OCB3 with mosh's 96-bit nonce and 128-bit tag.
type MoshOcb = Ocb3<Aes128, U12, U16>;

/// Which way a datagram travels. The value IS the top bit of the nonce
/// (`network.h`: `enum Direction { TO_SERVER = 0, TO_CLIENT = 1 }`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Sent BY the client. Nonce direction bit 0.
    ToServer = 0,
    /// Sent BY the server. Nonce direction bit 1; a client drops
    /// anything arriving with the other value, which is what stops
    /// its own datagrams being reflected back at it.
    ToClient = 1,
}

const DIRECTION_MASK: u64 = 1 << 63;
const SEQUENCE_MASK: u64 = u64::MAX ^ DIRECTION_MASK;

/// mosh's `Session::RECEIVE_MTU`: the plaintext/ciphertext ceiling.
pub const RECEIVE_MTU: usize = 2048;

/// The 96-bit OCB nonce mosh builds from the 64-bit direction+seq
/// value: four zero bytes, then the value big-endian.
fn ocb_nonce(direction_seq: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&direction_seq.to_be_bytes());
    n
}

/// One decrypted datagram: the sequence number and direction recovered
/// from the nonce, plus the plaintext (timestamps + payload, parsed a
/// layer up).
pub struct Incoming {
    /// The sequence number the nonce carried.
    pub seq: u64,
    /// Which side sent it, read off the nonce's top bit.
    pub direction: Direction,
    /// Everything the tag covered.
    pub plaintext: Vec<u8>,
}

impl std::fmt::Debug for Incoming {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Incoming")
            .field("seq", &self.seq)
            .field("direction", &self.direction)
            .field("plaintext", &super::Redacted(self.plaintext.len()))
            .finish()
    }
}

/// The keyed crypto session. Holds the AEAD and the 2^47-block lifetime
/// counter mosh caps every key at (both directions share the key).
pub struct Session {
    ocb: MoshOcb,
    blocks_encrypted: u64,
}

impl Session {
    /// Start a session under the key the SSH bootstrap agreed.
    pub fn new(key: &Base64Key) -> Self {
        Self {
            ocb: MoshOcb::new(key.bytes().into()),
            blocks_encrypted: 0,
        }
    }

    /// Encrypt `plaintext` for `seq`/`direction`, returning the full
    /// wire datagram (nonce ++ ciphertext ++ tag).
    pub fn encrypt(&mut self, seq: u64, direction: Direction, plaintext: &[u8]) -> Result<Vec<u8>> {
        if plaintext.len() > RECEIVE_MTU {
            return Err(MoshError::PlaintextTooLong);
        }
        // mosh's key-lifetime guard: one block per 16 bytes, rounded up,
        // and the session dies once 2^47 blocks have gone out.
        self.blocks_encrypted += (plaintext.len() as u64).div_ceil(16);
        if self.blocks_encrypted >> 47 != 0 {
            return Err(MoshError::KeyExhausted);
        }

        let direction_seq = ((direction as u64) << 63) | (seq & SEQUENCE_MASK);
        let nonce = ocb_nonce(direction_seq);

        let mut buf = plaintext.to_vec();
        let tag = self
            .ocb
            .encrypt_in_place_detached(&nonce.into(), &[], &mut buf)
            .map_err(|_| MoshError::IntegrityCheck)?;

        // Wire order: 8 nonce value bytes, ciphertext, 16-byte tag.
        let mut out = Vec::with_capacity(8 + buf.len() + 16);
        out.extend_from_slice(&direction_seq.to_be_bytes());
        out.extend_from_slice(&buf);
        out.extend_from_slice(&tag);
        Ok(out)
    }

    /// Decrypt one wire datagram. Follows `Session::decrypt`'s
    /// validation order: length floor first, then the AEAD tag.
    pub fn decrypt(&self, datagram: &[u8]) -> Result<Incoming> {
        // 8 nonce + 16 tag is the floor mosh accepts (empty plaintext).
        if datagram.len() < 24 {
            return Err(MoshError::CiphertextTooShort);
        }
        let direction_seq = u64::from_be_bytes(datagram[..8].try_into().unwrap());
        let nonce = ocb_nonce(direction_seq);

        let ct_and_tag = &datagram[8..];
        let (ct, tag) = ct_and_tag.split_at(ct_and_tag.len() - 16);
        let mut buf = ct.to_vec();
        self.ocb
            .decrypt_in_place_detached(&nonce.into(), &[], &mut buf, tag.into())
            .map_err(|_| MoshError::IntegrityCheck)?;

        let direction = if direction_seq & DIRECTION_MASK != 0 {
            Direction::ToClient
        } else {
            Direction::ToServer
        };
        Ok(Incoming {
            seq: direction_seq & SEQUENCE_MASK,
            direction,
            plaintext: buf,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Base64Key {
        Base64Key::from_printable("zr0jtuYVKJnfJHP/XOZs7A").unwrap()
    }

    #[test]
    fn round_trip_recovers_plaintext_seq_and_direction() {
        let mut s = Session::new(&key());
        let pt = b"\x00\x01\xff\xff hello mosh";
        let dg = s.encrypt(42, Direction::ToServer, pt).unwrap();

        let dec = Session::new(&key()).decrypt(&dg).unwrap();
        assert_eq!(dec.plaintext, pt);
        assert_eq!(dec.seq, 42);
        assert_eq!(dec.direction, Direction::ToServer);
    }

    #[test]
    fn direction_bit_is_the_nonce_msb() {
        let mut s = Session::new(&key());
        let dg = s.encrypt(1, Direction::ToClient, b"x").unwrap();
        // Top bit of the first wire byte is the direction.
        assert_eq!(dg[0] & 0x80, 0x80);
        let dec = Session::new(&key()).decrypt(&dg).unwrap();
        assert_eq!(dec.direction, Direction::ToClient);
        assert_eq!(dec.seq, 1);

        let mut s2 = Session::new(&key());
        let dg2 = s2.encrypt(1, Direction::ToServer, b"x").unwrap();
        assert_eq!(dg2[0] & 0x80, 0x00);
    }

    #[test]
    fn tampered_tag_fails_integrity() {
        let mut s = Session::new(&key());
        let mut dg = s.encrypt(7, Direction::ToServer, b"secret").unwrap();
        let last = dg.len() - 1;
        dg[last] ^= 0x01;
        assert!(matches!(
            Session::new(&key()).decrypt(&dg),
            Err(MoshError::IntegrityCheck)
        ));
    }

    #[test]
    fn short_datagram_is_rejected_before_the_aead() {
        assert!(matches!(
            Session::new(&key()).decrypt(&[0u8; 23]),
            Err(MoshError::CiphertextTooShort)
        ));
    }

    #[test]
    fn ciphertext_length_is_plaintext_plus_nonce_and_tag() {
        let mut s = Session::new(&key());
        let dg = s.encrypt(0, Direction::ToServer, b"1234567890").unwrap();
        assert_eq!(dg.len(), 8 + 10 + 16);
    }

    /// RFC 7253 Appendix A known-answer vectors for AES-128-OCB with a
    /// 96-bit nonce, 128-bit tag and empty associated data, exactly
    /// the parametrization mosh's `ae.h` reference code runs. If this
    /// fails, the ocb3 dependency is not the cipher mosh speaks and no
    /// amount of round-trip testing above would have noticed.
    #[test]
    fn matches_rfc_7253_known_answers() {
        use ocb3::aead::Aead as _;
        let key = hex::decode("000102030405060708090A0B0C0D0E0F").unwrap();
        let ocb = MoshOcb::new(key.as_slice().into());

        // N = BBAA99887766554433221100, A = "", P = ""
        let nonce = hex::decode("BBAA99887766554433221100").unwrap();
        let ct = ocb.encrypt(nonce.as_slice().into(), &[][..]).unwrap();
        assert_eq!(hex::encode_upper(&ct), "785407BFFFC8AD9EDCC5520AC9111EE6");

        // N = ...03, A = "", P = 0001020304050607
        let nonce = hex::decode("BBAA99887766554433221103").unwrap();
        let pt = hex::decode("0001020304050607").unwrap();
        let ct = ocb.encrypt(nonce.as_slice().into(), pt.as_slice()).unwrap();
        assert_eq!(
            hex::encode_upper(&ct),
            "45DD69F8F5AAE72414054CD1F35D82760B2CD00D2F99BFA9"
        );
    }

    #[test]
    fn empty_plaintext_still_round_trips() {
        let mut s = Session::new(&key());
        let dg = s.encrypt(0, Direction::ToServer, b"").unwrap();
        assert_eq!(dg.len(), 24);
        let dec = Session::new(&key()).decrypt(&dg).unwrap();
        assert!(dec.plaintext.is_empty());
    }
}
