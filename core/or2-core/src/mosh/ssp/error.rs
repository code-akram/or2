// SPDX-License-Identifier: GPL-3.0-or-later
// Vendored from mosh-rs (https://github.com/wilsonglasser/mosh-rs), commit
// 90b37125f5e4a598be91dec37d23921b6865276e, src/error.rs. Upstream: GPL-3.0-or-later, copyright
// Wilson Glasser; the protocol logic follows mosh (Keith Winstein and contributors, GPL-3.0-or-later).
// See THIRD_PARTY_NOTICES.md. or2 changes: removed `BadAddress` (sockets are opened through `DatagramTransport`, not resolved here).
//! Error taxonomy of the wire layers.
//!
//! Mirrors mosh's own split: `CryptoException` for anything the crypto
//! layer refuses, and the transport's `dos_assert` class for packets a
//! peer could forge cheaply (those must never tear the session down,
//! only drop the datagram).

use thiserror::Error;

#[derive(Debug, Error)]
/// Everything that can go wrong below the terminal.
///
/// All of it is recoverable by dropping the datagram: on a public
/// UDP port anyone can send bytes, so a parse failure is an
/// ordinary event rather than a fault.
pub enum MoshError {
    /// The printable key is not the 22-character canonical base64 form
    /// mosh-server prints. The message texts follow crypto.cc verbatim
    /// so a user who has seen mosh's own errors recognizes them.
    #[error("Key must be 22 letters long.")]
    KeyLength,
    #[error("Key must represent 16 octets.")]
    /// The key was not 22 characters of base64 that round-trip.
    KeyDecode,
    /// The 132-bit tail rule: the low 4 bits of the 22nd base64 char
    /// must be zero, which mosh enforces by re-encoding and comparing.
    #[error("Base64 key was not encoded 128-bit key.")]
    KeyNotCanonical,
    /// Datagram shorter than nonce (8) + tag (16).
    #[error("Ciphertext must contain nonce and tag.")]
    CiphertextTooShort,
    /// OCB tag mismatch (or any AEAD failure).
    #[error("Packet failed integrity check.")]
    IntegrityCheck,
    /// mosh's 2^47-block key lifetime guard (both directions share the
    /// key, hence 2^47 and not 2^48).
    #[error("Encrypted 2^47 blocks.")]
    KeyExhausted,
    /// Plaintext exceeds RECEIVE_MTU; mosh asserts, we refuse.
    #[error("Plaintext is too long.")]
    PlaintextTooLong,
    /// Decrypted text shorter than the two timestamp fields.
    #[error("Payload too short for timestamps.")]
    PayloadTooShort,
    /// A packet whose direction bit claims it came from ourselves
    /// (mosh's dos_assert against reflected playback).
    #[error("Packet direction is wrong.")]
    WrongDirection,
    /// A transport payload too short to carry a fragment header.
    #[error("Fragment is shorter than its header.")]
    ShortFragment,
    /// The instruction did not survive zlib + protobuf decoding.
    #[error("Instruction did not decode.")]
    BadInstruction,
    /// The peer speaks a different MOSH_PROTOCOL_VERSION.
    #[error("mosh protocol version mismatch")]
    ProtocolVersion,
}

/// The crate's result type.
pub type Result<T> = std::result::Result<T, MoshError>;
