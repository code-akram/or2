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

pub use error::{MoshError, Result};
pub use key::Base64Key;
pub use screen::{Screen, ScreenError};
pub use session::{Fault, LinkHealth, Session, Tick};
pub use terminal::ClientTerminal;
