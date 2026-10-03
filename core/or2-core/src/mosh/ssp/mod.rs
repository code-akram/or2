//! The vendored mosh-rs library: mosh's wire protocol, layer by layer, bottom-up.
//!
//! 1. `key` + `crypto`  : the session key and the AES-128-OCB3 datagram layer.
//! 2. `packet`          : sequence numbers, the direction bit, and the 16-bit RTT timestamps
//!    carried inside every encrypted payload.
//! 3. `transport`       : fragmentation, zlib, protobuf instructions.
//! 4. `sender`          : the ack / retransmit / heartbeat timers.
//! 5. `statesync`       : the client's UserStream out, the host's terminal-state diffs in.
//! 6. `terminal`        : the client's own screen states.
//! 7. `session`         : all of the above driven together, without sockets.
//!
//! Each file names the upstream commit and what or2 changed.

pub mod crypto;
pub mod error;
pub mod key;
pub mod packet;
pub mod screen;
pub mod sender;
pub mod session;
pub mod statesync;
pub mod terminal;
pub mod transport;

/// Stands in for bytes that may be typed secrets or screen text in `Debug` output. Anything
/// that holds decrypted plaintext, keystrokes or host output prints through this, so a stray
/// `{:?}` or log line of a protocol type cannot leak them.
pub(crate) struct Redacted(pub usize);

impl std::fmt::Debug for Redacted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<{} bytes redacted>", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::crypto::{Direction, Incoming};
    use super::packet::Packet;
    use super::sender::TransportSender;
    use super::statesync::{HostBytes, HostEvent, Keystroke, UserEvent};
    use super::transport::{Fragment, FragmentAssembly, Instruction};

    /// A typed password and screen text must not appear in a stray `{:?}`.
    #[test]
    fn debug_output_never_carries_keystrokes_or_screen_text() {
        let secret = b"hunter2-s3cret";
        let mut sender = TransportSender::new();
        sender.state_mut().push_bytes(secret);
        let mut assembly = FragmentAssembly::default();
        let fragment = Fragment {
            id: 1,
            num: 0,
            final_fragment: false,
            contents: secret.to_vec(),
        };
        assembly.add(fragment.clone());
        let printed = [
            format!("{sender:?}"),
            format!("{:?}", UserEvent::Byte(secret[0])),
            format!(
                "{:?}",
                Incoming {
                    seq: 1,
                    direction: Direction::ToClient,
                    plaintext: secret.to_vec(),
                }
            ),
            format!(
                "{:?}",
                Packet {
                    timestamp: 0,
                    timestamp_reply: 0,
                    payload: secret.to_vec(),
                }
            ),
            format!(
                "{:?}",
                Instruction::new(0, 1, 0, 0, secret.to_vec(), secret.to_vec())
            ),
            format!("{fragment:?}"),
            format!("{assembly:?}"),
            format!(
                "{:?}",
                Keystroke {
                    keys: Some(secret.to_vec())
                }
            ),
            format!(
                "{:?}",
                HostBytes {
                    hoststring: Some(secret.to_vec())
                }
            ),
            format!("{:?}", HostEvent::Bytes(secret.to_vec())),
        ];
        for text in printed {
            // Neither as text nor as a list of byte values.
            assert!(!text.contains("hunter2"), "{text}");
            assert!(!text.contains("104"), "{text}");
        }
    }
}
