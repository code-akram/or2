//! Real SSH over [`Transport`](crate::transport::Transport). Network futures run on a
//! process-wide runtime; each driver and its callbacks stay on one dedicated thread, which
//! also owns the terminal it drives.
//!
//! [`connect_host`]: one connection per host carrying terminals, exec queries and streamlocal
//! channels. See [`connection`], which builds on [`client`] (host-key handler, relay,
//! authentication) and [`pump`] (the terminal pump of a terminal channel).

mod client;
mod connection;
mod pump;
mod terminal_session;

use std::sync::{Arc, OnceLock};

use tokio::runtime::Runtime;

use crate::host::{HostConnectRequest, HostHandle, HostObserver};
use crate::transport::{DirectTcp, Transport};

pub use connection::HostOptions;
#[cfg(any(test, feature = "test-support"))]
pub use connection::{SshRemote, connect_tapped};

pub(crate) fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("or2-network")
            .build()
            .expect("create SSH runtime")
    })
}

/// The device's network changed: every live SSH connection sends a keepalive at once, so a
/// connection the change silently broke is noticed now. mosh sessions roam through their own
/// handles (`SessionHandle::roam`).
pub fn network_changed() {
    connection::network_changed();
}

/// Connects to a host over TCP: races its addresses, verifies the host key, authenticates and
/// serves terminals, queries and herdr watches until closed. Returns at once; the state
/// changes and the final `Closed` arrive through `observer` on a Rust-owned thread.
pub fn connect_host(request: HostConnectRequest, observer: Arc<dyn HostObserver>) -> HostHandle {
    connect_host_with(
        Arc::new(DirectTcp),
        request,
        observer,
        HostOptions::default(),
    )
}

/// [`connect_host`] over any transport and with explicit timings, for tests.
pub fn connect_host_with<T: Transport>(
    transport: Arc<T>,
    request: HostConnectRequest,
    observer: Arc<dyn HostObserver>,
    options: HostOptions,
) -> HostHandle {
    connection::start(transport, request, observer, options)
}
