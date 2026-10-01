// SPDX-License-Identifier: GPL-3.0-or-later
// Vendored from mosh-rs (https://github.com/wilsonglasser/mosh-rs), commit
// 90b37125f5e4a598be91dec37d23921b6865276e, src/transport.rs. Upstream: GPL-3.0-or-later, copyright
// Wilson Glasser; the protocol logic follows mosh (Keith Winstein and contributors, GPL-3.0-or-later).
// See THIRD_PARTY_NOTICES.md. or2 changes: module paths only, then rustfmt. (Fragmentation, zlib and protobuf instructions; unrelated to or2's `Transport`.)
//! The transport layer: instructions, zlib, and the fragments that
//! carry them (`transportfragment.cc`, `transportinstruction.proto`).
//!
//! One `Instruction` describes a step of the state sync: which state
//! the diff was computed from, which one it produces, what the sender
//! has acked, and what the receiver may forget. It is serialized as
//! protobuf, compressed WHOLE with zlib (not just the diff), and cut
//! into fragments that fit the connection MTU.
//!
//! The protobuf is declared here with prost derives rather than
//! generated from the `.proto`: seven scalar fields do not justify a
//! `protoc` in the build, and spelling the field numbers out next to
//! their meaning is what a reader of this crate needs anyway. The
//! numbers are proto2's, verbatim from mosh.

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use prost::Message as _;
use std::io::{Read as _, Write as _};

use super::error::{MoshError, Result};

/// `MOSH_PROTOCOL_VERSION` (`network.h`), bumped for echo-ack. Sent on
/// every instruction and checked on every one received.
pub const MOSH_PROTOCOL_VERSION: u32 = 2;

/// The state number that means "shutting down" (`uint64_t(-1)`).
pub const SHUTDOWN_NUM: u64 = u64::MAX;

/// Ceiling on a decompressed instruction. mosh's compressor has a
/// fixed 4 MiB buffer ("effective limit on terminal size") and treats
/// anything larger as a hostile peer; so does this.
const MAX_INSTRUCTION_BYTES: usize = 2048 * 2048;

/// Fragment header: big-endian u64 id + big-endian u16 flags/num.
pub const FRAGMENT_HEADER_LEN: usize = 10;

/// Longest chaff mosh appends to obscure instruction length.
pub const CHAFF_MAX: usize = 16;

/// One transport instruction. Field numbers are proto2's, from
/// `transportinstruction.proto`.
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct Instruction {
    #[prost(uint32, optional, tag = "1")]
    /// Field 1. Must be [`MOSH_PROTOCOL_VERSION`].
    pub protocol_version: Option<u32>,
    /// The state the diff was computed FROM. The receiver must already
    /// hold it, or the instruction is dropped (that is what makes the
    /// protocol idempotent).
    #[prost(uint64, optional, tag = "2")]
    pub old_num: Option<u64>,
    /// The state the diff produces.
    #[prost(uint64, optional, tag = "3")]
    pub new_num: Option<u64>,
    /// Highest contiguous state of the PEER that this sender holds.
    #[prost(uint64, optional, tag = "4")]
    pub ack_num: Option<u64>,
    /// Earliest of our own states still worth keeping; the peer may
    /// forget everything older.
    #[prost(uint64, optional, tag = "5")]
    pub throwaway_num: Option<u64>,
    #[prost(bytes = "vec", optional, tag = "6")]
    /// Field 6. The state diff; what it means is the state layer's.
    pub diff: Option<Vec<u8>>,
    /// Random padding, ignored on receipt: it only exists so an
    /// observer cannot read keystroke timing off packet lengths.
    #[prost(bytes = "vec", optional, tag = "7")]
    pub chaff: Option<Vec<u8>>,
}

impl Instruction {
    /// A fully populated instruction, the way `send_in_fragments`
    /// builds one: every field set, protocol version included.
    pub fn new(
        old_num: u64,
        new_num: u64,
        ack_num: u64,
        throwaway_num: u64,
        diff: Vec<u8>,
        chaff: Vec<u8>,
    ) -> Self {
        Self {
            protocol_version: Some(MOSH_PROTOCOL_VERSION),
            old_num: Some(old_num),
            new_num: Some(new_num),
            ack_num: Some(ack_num),
            throwaway_num: Some(throwaway_num),
            diff: Some(diff),
            chaff: Some(chaff),
        }
    }

    /// Serialize + zlib. mosh compresses the WHOLE serialized
    /// instruction, so the fragmenter slices compressed bytes.
    pub fn to_compressed(&self) -> Result<Vec<u8>> {
        let raw = self.encode_to_vec();
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&raw).map_err(|_| MoshError::BadInstruction)?;
        enc.finish().map_err(|_| MoshError::BadInstruction)
    }

    /// Inverse of [`Self::to_compressed`].
    ///
    /// Capped, because the input is untrusted and zlib expands: a few
    /// kilobytes of fragments can decompress to gigabytes, and reading
    /// that into a Vec is a denial of service anyone on the network can
    /// mount. mosh sizes its decompression buffer once, at 4 MiB, and
    /// treats an overflow as a hostile peer; this is the same ceiling.
    pub fn from_compressed(bytes: &[u8]) -> Result<Self> {
        let mut raw = Vec::new();
        ZlibDecoder::new(bytes)
            .take(MAX_INSTRUCTION_BYTES as u64 + 1)
            .read_to_end(&mut raw)
            .map_err(|_| MoshError::BadInstruction)?;
        if raw.len() > MAX_INSTRUCTION_BYTES {
            return Err(MoshError::BadInstruction);
        }
        Self::decode(raw.as_slice()).map_err(|_| MoshError::BadInstruction)
    }

    /// Reject an instruction from a peer speaking another protocol
    /// version, the one check `Transport::recv` makes before anything
    /// else touches the payload.
    pub fn check_version(&self) -> Result<()> {
        if self.protocol_version != Some(MOSH_PROTOCOL_VERSION) {
            return Err(MoshError::ProtocolVersion);
        }
        Ok(())
    }
}

/// One fragment of a compressed instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    /// Instruction id. Reused verbatim by a byte-identical retransmit,
    /// which is what lets the peer recognize the repeat.
    pub id: u64,
    /// 15-bit index within the instruction.
    pub num: u16,
    /// Whether this is the last fragment of its instruction.
    pub final_fragment: bool,
    /// A slice of the compressed instruction.
    pub contents: Vec<u8>,
}

impl Fragment {
    /// `[ BE u64 id ][ BE u16 (final << 15) | num ][ contents ]`.
    pub fn to_bytes(&self) -> Vec<u8> {
        debug_assert!(self.num & 0x8000 == 0, "fragment num must fit 15 bits");
        let combined = ((self.final_fragment as u16) << 15) | (self.num & 0x7fff);
        let mut out = Vec::with_capacity(FRAGMENT_HEADER_LEN + self.contents.len());
        out.extend_from_slice(&self.id.to_be_bytes());
        out.extend_from_slice(&combined.to_be_bytes());
        out.extend_from_slice(&self.contents);
        out
    }

    /// Parse a fragment off the wire.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < FRAGMENT_HEADER_LEN {
            return Err(MoshError::ShortFragment);
        }
        let id = u64::from_be_bytes(bytes[..8].try_into().unwrap());
        let combined = u16::from_be_bytes([bytes[8], bytes[9]]);
        Ok(Self {
            id,
            num: combined & 0x7fff,
            final_fragment: combined & 0x8000 != 0,
            contents: bytes[FRAGMENT_HEADER_LEN..].to_vec(),
        })
    }
}

/// Cuts instructions into fragments and hands out instruction ids.
///
/// The id only advances when the instruction actually differs from the
/// last one (or the MTU changed): a retransmission of identical bytes
/// keeps its id, so the peer's reassembly recognizes the repeat
/// instead of tearing down a half-assembled instruction.
#[derive(Debug, Default)]
pub struct Fragmenter {
    next_id: u64,
    last_instruction: Option<Instruction>,
    last_mtu: Option<usize>,
}

impl Fragmenter {
    /// Fragment `inst` for a payload budget of `mtu` bytes, which the
    /// caller has already reduced by the datagram overhead. The header
    /// comes out of that budget here, exactly as `make_fragments` does.
    pub fn fragment(&mut self, inst: &Instruction, mtu: usize) -> Result<Vec<Fragment>> {
        let body_mtu = mtu.saturating_sub(FRAGMENT_HEADER_LEN).max(1);

        // Compare on everything BUT the diff and chaff: mosh's own
        // check is field-by-field over the transport header, and chaff
        // is random per send so it would force a new id every time.
        let same = self.last_instruction.as_ref().is_some_and(|last| {
            last.protocol_version == inst.protocol_version
                && last.old_num == inst.old_num
                && last.new_num == inst.new_num
                && last.ack_num == inst.ack_num
                && last.throwaway_num == inst.throwaway_num
        }) && self.last_mtu == Some(mtu);
        if !same {
            self.next_id += 1;
        }
        self.last_instruction = Some(inst.clone());
        self.last_mtu = Some(mtu);
        let id = self.next_id;

        let payload = inst.to_compressed()?;
        let total = payload.len().div_ceil(body_mtu);
        let mut out = Vec::with_capacity(total);
        for (i, chunk) in payload.chunks(body_mtu).enumerate() {
            out.push(Fragment {
                id,
                num: i as u16,
                final_fragment: i + 1 == total,
                contents: chunk.to_vec(),
            });
        }
        Ok(out)
    }
}

/// Reassembles fragments back into an instruction.
///
/// Keyed by instruction id: a fragment carrying a different id wipes
/// whatever was half-assembled, because mosh never interleaves two
/// instructions on one connection.
#[derive(Debug, Default)]
pub struct FragmentAssembly {
    current_id: Option<u64>,
    fragments: Vec<Option<Vec<u8>>>,
    arrived: usize,
    total: Option<usize>,
}

impl FragmentAssembly {
    /// Add one fragment; `true` when the instruction is complete and
    /// [`Self::take`] will produce it.
    pub fn add(&mut self, frag: Fragment) -> bool {
        if self.current_id != Some(frag.id) {
            self.current_id = Some(frag.id);
            self.fragments.clear();
            self.arrived = 0;
            self.total = None;
        }
        let idx = frag.num as usize;
        if self.fragments.len() <= idx {
            self.fragments.resize(idx + 1, None);
        }
        if self.fragments[idx].is_none() {
            self.arrived += 1;
        }
        let is_final = frag.final_fragment;
        self.fragments[idx] = Some(frag.contents);
        if is_final {
            self.total = Some(idx + 1);
            self.fragments.resize(idx + 1, None);
        }
        self.total == Some(self.arrived)
    }

    /// Concatenate and decode the completed instruction, resetting for
    /// the next one. `None` while fragments are still missing.
    pub fn take(&mut self) -> Result<Option<Instruction>> {
        if self.total != Some(self.arrived) {
            return Ok(None);
        }
        let mut encoded = Vec::new();
        for frag in self.fragments.iter() {
            let Some(bytes) = frag else {
                return Ok(None);
            };
            encoded.extend_from_slice(bytes);
        }
        self.fragments.clear();
        self.arrived = 0;
        self.total = None;
        Ok(Some(Instruction::from_compressed(&encoded)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inst(new_num: u64, diff: &[u8]) -> Instruction {
        Instruction::new(new_num - 1, new_num, 0, 0, diff.to_vec(), Vec::new())
    }

    /// Deterministic bytes that zlib cannot squeeze: a run of one
    /// character compresses to almost nothing, which would keep a
    /// "large" diff inside a single fragment and quietly stop the
    /// fragmentation tests from testing fragmentation.
    fn incompressible(len: usize) -> Vec<u8> {
        let mut x: u32 = 0x1234_5678;
        (0..len)
            .map(|_| {
                // xorshift32: no dependency, same bytes every run.
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x & 0xff) as u8
            })
            .collect()
    }

    #[test]
    fn fragment_header_is_id_then_flagged_num() {
        let f = Fragment {
            id: 0x0102030405060708,
            num: 3,
            final_fragment: true,
            contents: b"body".to_vec(),
        };
        let bytes = f.to_bytes();
        assert_eq!(&bytes[..8], &[1, 2, 3, 4, 5, 6, 7, 8]);
        // final flag is the top bit of the 16-bit field.
        assert_eq!(&bytes[8..10], &[0x80, 0x03]);
        assert_eq!(&bytes[10..], b"body");
        assert_eq!(Fragment::from_bytes(&bytes).unwrap(), f);
    }

    #[test]
    fn a_non_final_fragment_clears_the_top_bit() {
        let f = Fragment {
            id: 1,
            num: 0x7fff,
            final_fragment: false,
            contents: vec![],
        };
        let bytes = f.to_bytes();
        assert_eq!(&bytes[8..10], &[0x7f, 0xff]);
        assert!(!Fragment::from_bytes(&bytes).unwrap().final_fragment);
    }

    #[test]
    fn a_header_less_fragment_is_refused() {
        assert!(matches!(
            Fragment::from_bytes(&[0u8; 9]),
            Err(MoshError::ShortFragment)
        ));
    }

    #[test]
    fn a_decompression_bomb_is_refused_rather_than_allocated() {
        // A few kilobytes of zeros expand to far more than the ceiling.
        // The input here is what an attacker can actually put on the
        // wire; what matters is that the OUTPUT never gets that big.
        let bomb = {
            let mut enc = ZlibEncoder::new(Vec::new(), Compression::best());
            enc.write_all(&vec![0u8; MAX_INSTRUCTION_BYTES + 1024])
                .unwrap();
            enc.finish().unwrap()
        };
        assert!(bomb.len() < 8192, "the point is that the input is small");
        assert!(matches!(
            Instruction::from_compressed(&bomb),
            Err(MoshError::BadInstruction)
        ));
    }

    #[test]
    fn instruction_round_trips_through_zlib_and_protobuf() {
        let i = inst(7, b"a diff");
        let compressed = i.to_compressed().unwrap();
        assert_eq!(Instruction::from_compressed(&compressed).unwrap(), i);
    }

    #[test]
    fn a_foreign_protocol_version_is_refused() {
        let mut i = inst(1, b"");
        assert!(i.check_version().is_ok());
        i.protocol_version = Some(3);
        assert!(matches!(i.check_version(), Err(MoshError::ProtocolVersion)));
        i.protocol_version = None;
        assert!(matches!(i.check_version(), Err(MoshError::ProtocolVersion)));
    }

    #[test]
    fn one_instruction_round_trips_through_fragmentation() {
        let mut f = Fragmenter::default();
        let mut asm = FragmentAssembly::default();
        // A diff far larger than the MTU, so it really splits.
        let i = inst(2, &incompressible(4000));
        let frags = f.fragment(&i, 500).unwrap();
        assert!(frags.len() > 1, "a 4 KB diff must span fragments");
        assert!(frags.last().unwrap().final_fragment);
        assert!(frags.iter().rev().skip(1).all(|f| !f.final_fragment));

        let mut complete = false;
        for frag in frags {
            let bytes = frag.to_bytes();
            assert!(bytes.len() <= 500, "fragment must fit the MTU");
            complete = asm.add(Fragment::from_bytes(&bytes).unwrap());
        }
        assert!(complete);
        assert_eq!(asm.take().unwrap().unwrap(), i);
    }

    #[test]
    fn out_of_order_fragments_still_assemble() {
        let mut f = Fragmenter::default();
        let mut asm = FragmentAssembly::default();
        let i = inst(2, &incompressible(3000));
        let mut frags = f.fragment(&i, 400).unwrap();
        frags.reverse();
        let mut complete = false;
        for frag in frags {
            complete = asm.add(frag);
        }
        assert!(complete);
        assert_eq!(asm.take().unwrap().unwrap(), i);
    }

    #[test]
    fn a_new_id_wipes_a_half_assembled_instruction() {
        let mut asm = FragmentAssembly::default();
        assert!(!asm.add(Fragment {
            id: 1,
            num: 0,
            final_fragment: false,
            contents: vec![1]
        }));
        // The peer moved on: what we held for id 1 is dead weight.
        assert!(!asm.add(Fragment {
            id: 2,
            num: 0,
            final_fragment: false,
            contents: vec![2]
        }));
        assert_eq!(asm.arrived, 1);
        assert_eq!(asm.current_id, Some(2));
    }

    #[test]
    fn a_repeated_instruction_keeps_its_id() {
        let mut f = Fragmenter::default();
        let i = inst(5, b"same");
        let first = f.fragment(&i, 500).unwrap()[0].id;
        // Byte-identical retransmit: same id, so the peer sees a repeat
        // rather than a new instruction.
        let again = f.fragment(&i, 500).unwrap()[0].id;
        assert_eq!(first, again);
        // A different state number is a different instruction.
        let next = f.fragment(&inst(6, b"same"), 500).unwrap()[0].id;
        assert_ne!(first, next);
        // So is the same instruction under a changed MTU.
        let mtu_changed = f.fragment(&inst(6, b"same"), 400).unwrap()[0].id;
        assert_ne!(next, mtu_changed);
    }
}
