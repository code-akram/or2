//! mosh: a Rust client for mosh's State Synchronization Protocol, interoperating with stock
//! `mosh-server`. M3 serves it as a terminal transport on a host connection (`ssh::mosh_session`).
//!
//! ```text
//! bootstrap ──▶ MoshParams ──▶ start ──▶ SessionHandle (standard lifecycle and frames)
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

pub mod bootstrap;
mod driver;
pub mod ghostty;
mod link;
pub mod ssp;

pub use bootstrap::{BootstrapError, MoshKey, MoshParams, bootstrap, terminate};
pub use driver::{CONNECT_TIMEOUT, HealthObserver, LinkControl, start, start_with};
pub(crate) use driver::{GOODBYE_TIMEOUT, Plan, run_session};
pub use ssp::session::LinkHealth;
