//! mosh: a Rust client for mosh's State Synchronization Protocol, interoperating with stock
//! `mosh-server`. M3 serves it as a terminal transport on a host connection (`ssh::mosh_session`).
//!
//! ```text
//! bootstrap ──▶ MoshParams ──▶ run_session on a SessionDriver (standard lifecycle and frames)
//!   (RemoteHost exec:                │
//!    mosh-server new)                ▼
//!                       driver thread ── ssp::Session (crypto, timers, state sync; no I/O)
//!                              │            │
//!                              │            └─ ClientTerminal<GhosttyScreen> (libghostty)
//!                              └─ Link ── DatagramTransport (DirectUdp, or Android later)
//! ```
//!
//! [`ssp`] is the vendored mosh-rs library (see `THIRD_PARTY_NOTICES.md`), changed so that it
//! never opens a socket: [`ssp::session::Session`] takes datagrams in and hands datagrams out,
//! and `link` owns the sockets, which it opens only through
//! [`DatagramTransport`](crate::transport::DatagramTransport). Roaming is a rebind through that
//! trait. Terminal state lives in libghostty ([`ghostty::GhosttyScreen`]) so frames reach the
//! renderer exactly as they do for SSH. Local-echo prediction is off, and the vendored
//! prediction engine is not carried.

#![forbid(unsafe_code)]

pub(crate) mod bootstrap;
mod driver;
pub(crate) mod ghostty;
mod link;
pub(crate) mod ssp;

pub use bootstrap::{BootstrapError, MoshKey, MoshParams, bootstrap, terminate};
pub use driver::CONNECT_TIMEOUT;
pub(crate) use driver::GOODBYE_TIMEOUT;
/// The session driver itself, for the live test against a real `mosh-server`.
#[cfg(feature = "test-support")]
pub use driver::{Ended, Plan, run_session};
#[cfg(not(feature = "test-support"))]
pub(crate) use driver::{Plan, run_session};
pub use ssp::session::LinkHealth;
