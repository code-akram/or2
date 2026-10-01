//! One mosh terminal on a host connection (contracts.md, "Mosh terminals").
//!
//! The host's SSH connection is only the bootstrap: `mosh::bootstrap` runs `mosh-server` over
//! it (with the target's command, after the herdr pane focus), then the session runs over UDP,
//! pinned to the address the SSH connection reached ([`HostHandle::peer_addr`]). From then on
//! the session does not need the SSH connection, so **losing the host connection leaves the
//! session running**; only a user disconnect of the host ends it (`Disconnected`, with mosh's
//! shutdown handshake so the server exits). Whoever starts the server also owes its cleanup:
//! a start that fails, times out or is disconnected before `Connected` calls `mosh::terminate`
//! before the session reports its end.
//!
//! [`HostHandle::peer_addr`]: crate::host::HostHandle::peer_addr

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Notify, mpsc, oneshot};
use tokio::time::timeout;

use super::connection::{Closing, SshHost, closed_reason};
use super::runtime;
use super::terminal_session::{not_installed, program, remote_failure};
use crate::host::TerminalTarget;
use crate::mosh::{self, MoshParams, Plan, run_session};
use crate::session::{CloseReason, Command, SessionDriver, SessionFailure, SessionState};
use crate::term::TerminalSize;
use crate::transport::DatagramTransport;

/// How much longer the bootstrap may take once the user has disconnected: long enough for the
/// exec that is probably already running to report its pid (so the server can be stopped),
/// short enough for a disconnect not to hang on a wedged host.
const ABANDON_GRACE: Duration = Duration::from_secs(2);

/// What a mosh terminal needs from the host driver besides its [`SessionDriver`].
pub(super) struct Open<D> {
    pub(super) host: Arc<SshHost>,
    pub(super) datagrams: Arc<D>,
    /// The address the host's SSH connection reached; mosh must not run without it.
    pub(super) peer: Option<SocketAddr>,
    pub(super) target: TerminalTarget,
    pub(super) size: TerminalSize,
    pub(super) closing: Closing,
    /// Held while the session depends on the host's closing order (see [`watch_host`]).
    pub(super) tracker: mpsc::Sender<()>,
    /// How long to wait for the server's first datagram.
    pub(super) connect_timeout: Duration,
}

type Prepare<'a> = Pin<Box<dyn Future<Output = Result<MoshParams, SessionFailure>> + Send + 'a>>;

/// Runs one mosh terminal to its close. Closes `driver` exactly once, last.
pub(super) async fn drive<D: DatagramTransport>(open: Open<D>, mut driver: SessionDriver) {
    let Open {
        host,
        datagrams,
        peer,
        target,
        size,
        closing,
        tracker,
        connect_timeout,
    } = open;
    let Some(peer) = peer else {
        driver.close(CloseReason::Failed(SessionFailure::Internal(
            "the host connection cannot say which address it reached, so mosh has no \
             address to pin"
                .into(),
        )));
        return;
    };
    let shutdown = Arc::new(Notify::new());
    let (done, finished) = oneshot::channel::<()>();
    runtime().spawn(watch_host(closing, shutdown.clone(), finished, tracker));
    // Dropped when this function returns, after the session's `Closed`.
    let _done = done;

    let mut size = size;
    let mut prepare: Prepare<'_> = Box::pin(prepare(&host, &target, size));
    let mut abandoned = false;
    let prepared = loop {
        tokio::select! {
            result = &mut prepare => break result,
            command = driver.next_command() => match command {
                Command::Disconnect => {
                    abandoned = true;
                    break abandon(&mut prepare).await;
                }
                Command::Resize(new) => size = new,
                // Nothing to send to yet, and host keys do not exist.
                _ => {}
            },
            () = shutdown.notified() => {
                abandoned = true;
                break abandon(&mut prepare).await;
            }
        }
    };
    let mut params = match prepared {
        Ok(params) => params,
        Err(failure) => {
            driver.close(if abandoned {
                CloseReason::Disconnected
            } else {
                CloseReason::Failed(failure)
            });
            return;
        }
    };
    let pid = params.server_pid;
    if abandoned {
        // The server exists and nobody will ever connect to it.
        cleanup(&host, pid).await;
        driver.close(CloseReason::Disconnected);
        return;
    }
    params.size = size;
    let reason = run_session(
        Plan {
            transport: datagrams,
            peer: SocketAddr::new(peer.ip(), params.port),
            params,
            health: None,
            roam: Arc::new(Notify::new()),
            shutdown,
            connect_timeout,
        },
        &mut driver,
    )
    .await;
    if driver.state() != SessionState::Connected {
        // Timed out (UDP blocked), failed or disconnected before the first datagram:
        // mosh-server would otherwise wait for a client for as long as it lives.
        cleanup(&host, pid).await;
    }
    driver.close(reason);
}

/// The probe, then (for tmux and herdr) the target's command, then `mosh-server new` running
/// it. `NotInstalled { program: "mosh-server" }` is decided before anything else runs, so a
/// host without mosh does not get a pane focus first.
async fn prepare(
    host: &SshHost,
    target: &TerminalTarget,
    size: TerminalSize,
) -> Result<MoshParams, SessionFailure> {
    let capabilities = host.capabilities().await.map_err(remote_failure)?;
    if capabilities.mosh_server.is_none() {
        return Err(not_installed("mosh-server"));
    }
    let argv = program(host, target)
        .await?
        .map(|command| command.argv())
        .unwrap_or_default();
    mosh::bootstrap(host, capabilities, size, &argv)
        .await
        .map_err(mosh::BootstrapError::into_failure)
}

/// After a disconnect: lets the bootstrap finish if it is about to, so the server it started
/// can be stopped. A server whose bootstrap is cut short loses its pid (contracts.md).
async fn abandon(prepare: &mut Prepare<'_>) -> Result<MoshParams, SessionFailure> {
    timeout(ABANDON_GRACE, prepare)
        .await
        .unwrap_or(Err(SessionFailure::TimedOut))
}

/// Stops the server that no session reached. Best effort and bounded: the host connection may
/// be gone (then the server stays until someone stops it, as documented).
async fn cleanup(host: &SshHost, pid: Option<u32>) {
    if let Some(pid) = pid {
        let _ = timeout(host.exec_timeout(), mosh::terminate(host, pid)).await;
    }
}

/// Ties the session to the host's closing. A user disconnect of the host (`Disconnected`) ends
/// the session through `shutdown`, and the watcher keeps its `tracker` until the session has
/// closed, so the host reports `Closed` after it, as for SSH terminals. Any other end of the
/// host is a loss: the session carries on without the SSH connection, and the tracker is
/// released at once so the host does not wait for it.
async fn watch_host(
    mut closing: Closing,
    shutdown: Arc<Notify>,
    mut finished: oneshot::Receiver<()>,
    tracker: mpsc::Sender<()>,
) {
    let _tracker = tracker;
    tokio::select! {
        reason = closed_reason(&mut closing) => {
            if reason == CloseReason::Disconnected {
                shutdown.notify_one();
                // Resolves when the session's task is done (its sender is dropped).
                let _ = (&mut finished).await;
            }
        }
        _ = &mut finished => {}
    }
}
