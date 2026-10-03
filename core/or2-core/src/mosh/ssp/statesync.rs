// SPDX-License-Identifier: GPL-3.0-or-later
// Vendored from mosh-rs (https://github.com/wilsonglasser/mosh-rs), commit
// 90b37125f5e4a598be91dec37d23921b6865276e, src/statesync.rs. Upstream: GPL-3.0-or-later, copyright
// Wilson Glasser; the protocol logic follows mosh (Keith Winstein and contributors, GPL-3.0-or-later).
// See THIRD_PARTY_NOTICES.md. or2 changes: module paths only, redacted `Debug` for the types that hold keystrokes or host output, then rustfmt.
//! State sync: what each side's diffs actually contain
//! (`user.cc`, `completeterminal.cc`, `userinput.proto`,
//! `hostinput.proto`).
//!
//! The two directions are not symmetric, and that asymmetry is the
//! whole design of mosh:
//!
//! - **Client to server** is a [`UserStream`]: an append-only log of
//!   keystrokes and resizes. A diff is literally the suffix the peer
//!   has not seen yet, which is why it can be recomputed from any
//!   older state at no cost.
//! - **Server to client** is a diff of the TERMINAL, already rendered
//!   as ECMA-48 escape bytes by the server's own emulator, plus
//!   resizes and echo acknowledgements. The client never receives raw
//!   host output and never diffs terminal states itself: it feeds
//!   `hoststring` into its emulator exactly as it would feed a PTY.
//!
//! The protobufs are declared with prost derives rather than generated
//! from the `.proto` files. mosh declares its per-instruction fields
//! as proto2 *extensions*, which are ordinary wire fields with the
//! same numbers, so a plain message with those tags is byte-identical
//! on the wire and needs no `protoc`.

// These types mirror mosh's `.proto` files field for field, and the
// tag on each field is its documentation: `Keystroke.keys` is field 4
// because `userinput.proto` says so, and prose restating the name would
// be noise on top of the mapping the module docs above already give.
#![allow(missing_docs)]

use prost::Message as _;

use super::error::{MoshError, Result};

// ---------------------------------------------------------------- //
// client -> server: userinput.proto (package ClientBuffers)
// ---------------------------------------------------------------- //

#[derive(Clone, PartialEq, Eq, prost::Message)]
#[prost(skip_debug)]
pub struct Keystroke {
    #[prost(bytes = "vec", optional, tag = "4")]
    pub keys: Option<Vec<u8>>,
}

impl std::fmt::Debug for Keystroke {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keystroke")
            .field(
                "keys",
                &self.keys.as_ref().map(|k| super::Redacted(k.len())),
            )
            .finish()
    }
}

/// Also the host direction's resize; mosh declares one per package
/// with the same field numbers.
#[derive(Clone, Copy, PartialEq, Eq, prost::Message)]
pub struct ResizeMessage {
    #[prost(int32, optional, tag = "5")]
    pub width: Option<i32>,
    #[prost(int32, optional, tag = "6")]
    pub height: Option<i32>,
}

/// One user instruction. In the `.proto` these two are extensions of
/// an empty message; on the wire they are just fields 2 and 3.
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct UserInstruction {
    #[prost(message, optional, tag = "2")]
    pub keystroke: Option<Keystroke>,
    #[prost(message, optional, tag = "3")]
    pub resize: Option<ResizeMessage>,
}

#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct UserMessage {
    #[prost(message, repeated, tag = "1")]
    pub instruction: Vec<UserInstruction>,
}

impl UserMessage {
    /// Every keystroke byte in a serialized user diff, concatenated.
    /// Resizes are skipped, so this answers "what did the user type",
    /// which is what a caller checking a diff usually means.
    #[cfg(test)]
    pub fn keystrokes_of(diff: &[u8]) -> Result<Vec<u8>> {
        let msg = Self::decode(diff).map_err(|_| MoshError::BadInstruction)?;
        Ok(msg
            .instruction
            .into_iter()
            .filter_map(|i| i.keystroke)
            .filter_map(|k| k.keys)
            .flatten()
            .collect())
    }
}

/// One thing the user did.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UserEvent {
    /// A single byte of input. mosh keeps input byte-by-byte and
    /// coalesces only when serializing, so a diff can start anywhere.
    Byte(u8),
    Resize {
        width: i32,
        height: i32,
    },
}

impl std::fmt::Debug for UserEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Byte(_) => f.write_str("Byte(<redacted>)"),
            Self::Resize { width, height } => f
                .debug_struct("Resize")
                .field("width", width)
                .field("height", height)
                .finish(),
        }
    }
}

/// The client's state: everything the user has done, in order.
///
/// Diffs are pure suffixes, so a state is "older" than another exactly
/// when it is a prefix of it. That is what lets the sender recompute a
/// diff against any state the receiver might still be holding.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct UserStream {
    events: Vec<UserEvent>,
}

/// Typed input can be a password: only the length is printed.
impl std::fmt::Debug for UserStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserStream")
            .field("events", &super::Redacted(self.events.len()))
            .finish()
    }
}

impl UserStream {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_bytes(&mut self, bytes: &[u8]) {
        self.events
            .extend(bytes.iter().copied().map(UserEvent::Byte));
    }

    pub fn push_resize(&mut self, width: i32, height: i32) {
        self.events.push(UserEvent::Resize { width, height });
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// The diff that takes `existing` to `self`: the events `existing`
    /// does not have yet, serialized.
    ///
    /// Contiguous bytes are coalesced into one `Keystroke`, which is
    /// what keeps a burst of typing to a single instruction; a resize
    /// breaks the run because it is its own instruction.
    ///
    /// Returns `None` when `existing` is not a prefix of `self`. mosh
    /// asserts here, because its sender only ever diffs against states
    /// it sent, and a violation means the sender lost track of what the
    /// receiver holds.
    pub fn diff_from(&self, existing: &UserStream) -> Option<Vec<u8>> {
        if !self.events.starts_with(&existing.events) {
            return None;
        }
        let mut msg = UserMessage::default();
        for event in &self.events[existing.events.len()..] {
            match event {
                UserEvent::Byte(b) => {
                    // Append to the run in progress, if the last
                    // instruction is one.
                    match msg
                        .instruction
                        .last_mut()
                        .and_then(|i| i.keystroke.as_mut())
                        .and_then(|k| k.keys.as_mut())
                    {
                        Some(keys) => keys.push(*b),
                        None => msg.instruction.push(UserInstruction {
                            keystroke: Some(Keystroke {
                                keys: Some(vec![*b]),
                            }),
                            resize: None,
                        }),
                    }
                }
                UserEvent::Resize { width, height } => {
                    msg.instruction.push(UserInstruction {
                        keystroke: None,
                        resize: Some(ResizeMessage {
                            width: Some(*width),
                            height: Some(*height),
                        }),
                    });
                }
            }
        }
        Some(msg.encode_to_vec())
    }

    /// The diff from nothing: what a fresh peer needs to catch up.
    #[cfg(test)]
    pub fn init_diff(&self) -> Vec<u8> {
        self.diff_from(&UserStream::new()).unwrap_or_default()
    }

    /// Apply a diff produced by [`Self::diff_from`], appending its
    /// events. This is the server's job in a real session; the client
    /// runs it only to verify its own encoding.
    #[cfg(test)]
    pub fn apply_string(&mut self, diff: &[u8]) -> Result<()> {
        let msg = UserMessage::decode(diff).map_err(|_| MoshError::BadInstruction)?;
        for inst in msg.instruction {
            if let Some(k) = inst.keystroke
                && let Some(keys) = k.keys
            {
                self.push_bytes(&keys);
            }
            if let Some(r) = inst.resize
                && let (Some(width), Some(height)) = (r.width, r.height)
            {
                self.push_resize(width, height);
            }
        }
        Ok(())
    }

    /// Drop the prefix the receiver has confirmed, so a long session
    /// does not keep every keystroke it ever sent
    /// (`UserStream::subtract`). `false` when `prefix` is not one.
    pub fn subtract(&mut self, prefix: &UserStream) -> bool {
        if !self.events.starts_with(&prefix.events) {
            return false;
        }
        self.events.drain(..prefix.events.len());
        true
    }
}

// ---------------------------------------------------------------- //
// server -> client: hostinput.proto (package HostBuffers)
// ---------------------------------------------------------------- //

#[derive(Clone, PartialEq, Eq, prost::Message)]
#[prost(skip_debug)]
pub struct HostBytes {
    /// ECMA-48 escape output, already rendered by the SERVER's
    /// terminal emulator as the difference between two framebuffers.
    #[prost(bytes = "vec", optional, tag = "4")]
    pub hoststring: Option<Vec<u8>>,
}

impl std::fmt::Debug for HostBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostBytes")
            .field(
                "hoststring",
                &self.hoststring.as_ref().map(|s| super::Redacted(s.len())),
            )
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, prost::Message)]
pub struct EchoAck {
    /// The latest client state whose keystrokes the server has echoed.
    /// A client retires predictive local echo with this; one that does
    /// not predict may ignore it.
    #[prost(uint64, optional, tag = "8")]
    pub echo_ack_num: Option<u64>,
}

#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct HostInstruction {
    #[prost(message, optional, tag = "2")]
    pub hostbytes: Option<HostBytes>,
    #[prost(message, optional, tag = "3")]
    pub resize: Option<ResizeMessage>,
    #[prost(message, optional, tag = "7")]
    pub echoack: Option<EchoAck>,
}

#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct HostMessage {
    #[prost(message, repeated, tag = "1")]
    pub instruction: Vec<HostInstruction>,
}

/// What one host diff asks the client to do, flattened out of the
/// protobuf in arrival order.
#[derive(Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// Feed these bytes to the terminal emulator.
    Bytes(Vec<u8>),
    Resize {
        width: i32,
        height: i32,
    },
    EchoAck(u64),
}

impl std::fmt::Debug for HostEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bytes(bytes) => f
                .debug_tuple("Bytes")
                .field(&super::Redacted(bytes.len()))
                .finish(),
            Self::Resize { width, height } => f
                .debug_struct("Resize")
                .field("width", width)
                .field("height", height)
                .finish(),
            Self::EchoAck(num) => f.debug_tuple("EchoAck").field(num).finish(),
        }
    }
}

/// Decode a host diff into the events it carries.
pub fn parse_host_diff(diff: &[u8]) -> Result<Vec<HostEvent>> {
    let msg = HostMessage::decode(diff).map_err(|_| MoshError::BadInstruction)?;
    let mut out = Vec::new();
    for inst in msg.instruction {
        if let Some(e) = inst.echoack
            && let Some(num) = e.echo_ack_num
        {
            out.push(HostEvent::EchoAck(num));
        }
        if let Some(r) = inst.resize
            && let (Some(width), Some(height)) = (r.width, r.height)
        {
            out.push(HostEvent::Resize { width, height });
        }
        if let Some(h) = inst.hostbytes
            && let Some(bytes) = h.hoststring
            && !bytes.is_empty()
        {
            out.push(HostEvent::Bytes(bytes));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diff_carries_only_the_suffix() {
        let mut old = UserStream::new();
        old.push_bytes(b"ls");
        let mut new = old.clone();
        new.push_bytes(b" -l\r");

        let diff = new.diff_from(&old).expect("old is a prefix");
        let mut rebuilt = old.clone();
        rebuilt.apply_string(&diff).unwrap();
        assert_eq!(rebuilt, new);
    }

    #[test]
    fn contiguous_bytes_coalesce_into_one_keystroke() {
        let mut s = UserStream::new();
        s.push_bytes(b"echo hi");
        let diff = s.init_diff();
        let msg = UserMessage::decode(diff.as_slice()).unwrap();
        assert_eq!(
            msg.instruction.len(),
            1,
            "one run of typing, one instruction"
        );
        assert_eq!(
            msg.instruction[0]
                .keystroke
                .as_ref()
                .unwrap()
                .keys
                .as_deref(),
            Some(&b"echo hi"[..])
        );
    }

    #[test]
    fn a_resize_breaks_the_run() {
        let mut s = UserStream::new();
        s.push_bytes(b"ab");
        s.push_resize(80, 24);
        s.push_bytes(b"cd");
        let msg = UserMessage::decode(s.init_diff().as_slice()).unwrap();
        assert_eq!(msg.instruction.len(), 3);
        assert!(msg.instruction[0].keystroke.is_some());
        assert_eq!(
            msg.instruction[1].resize,
            Some(ResizeMessage {
                width: Some(80),
                height: Some(24)
            })
        );
        assert!(msg.instruction[2].keystroke.is_some());
    }

    #[test]
    fn a_state_that_is_not_a_prefix_has_no_diff() {
        let mut a = UserStream::new();
        a.push_bytes(b"abc");
        let mut b = UserStream::new();
        b.push_bytes(b"xyz");
        assert!(b.diff_from(&a).is_none());
    }

    #[test]
    fn subtract_drops_the_confirmed_prefix() {
        let mut sent = UserStream::new();
        sent.push_bytes(b"hello");
        let mut acked = UserStream::new();
        acked.push_bytes(b"hel");

        assert!(sent.subtract(&acked));
        assert_eq!(sent.len(), 2);
        // What remains still diffs correctly against nothing.
        let msg = UserMessage::decode(sent.init_diff().as_slice()).unwrap();
        assert_eq!(
            msg.instruction[0]
                .keystroke
                .as_ref()
                .unwrap()
                .keys
                .as_deref(),
            Some(&b"lo"[..])
        );
    }

    #[test]
    fn subtract_refuses_a_state_that_is_not_a_prefix() {
        let mut sent = UserStream::new();
        sent.push_bytes(b"hello");
        let mut other = UserStream::new();
        other.push_bytes(b"world");
        assert!(!sent.subtract(&other));
        assert_eq!(sent.len(), 5, "a refused subtract changes nothing");
    }

    #[test]
    fn host_diffs_decode_in_arrival_order() {
        let msg = HostMessage {
            instruction: vec![
                HostInstruction {
                    hostbytes: None,
                    resize: None,
                    echoack: Some(EchoAck {
                        echo_ack_num: Some(7),
                    }),
                },
                HostInstruction {
                    hostbytes: Some(HostBytes {
                        hoststring: Some(b"\x1b[2Jhello".to_vec()),
                    }),
                    resize: Some(ResizeMessage {
                        width: Some(100),
                        height: Some(30),
                    }),
                    echoack: None,
                },
            ],
        };
        let events = parse_host_diff(&msg.encode_to_vec()).unwrap();
        assert_eq!(
            events,
            vec![
                HostEvent::EchoAck(7),
                // Within one instruction the resize is applied before
                // the bytes it reflows.
                HostEvent::Resize {
                    width: 100,
                    height: 30
                },
                HostEvent::Bytes(b"\x1b[2Jhello".to_vec()),
            ]
        );
    }

    #[test]
    fn an_empty_hoststring_is_not_an_event() {
        let msg = HostMessage {
            instruction: vec![HostInstruction {
                hostbytes: Some(HostBytes {
                    hoststring: Some(Vec::new()),
                }),
                resize: None,
                echoack: None,
            }],
        };
        assert!(parse_host_diff(&msg.encode_to_vec()).unwrap().is_empty());
    }
}
