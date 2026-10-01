//! One mosh terminal on a host connection (contracts.md, "Mosh terminals").
//!
//! The host's SSH connection is only the bootstrap: `mosh::bootstrap` runs `mosh-server` over
//! it (with the target's command, after the herdr pane focus), then the session runs over UDP,
//! pinned to the address the SSH connection reached ([`HostHandle::peer_addr`]). From then on
//! the session does not need the SSH connection, so **losing the host connection leaves the
//! session running**; only a user disconnect of the host ends it (`Disconnected`, with mosh's
//! shutdown handshake so the server exits). Whoever starts the server also owes its cleanup:
//! a start that fails, times out or is disconnected before `Connected`, and any session that
//! ends `Failed` (before or after `Connected`), calls `mosh::terminate` before the session
//! reports its end.
//!
//! **Time.** Closing is bounded so the host can wait for it: after a user disconnect of the
//! host a session needs at most [`ABANDON_GRACE`] (a running bootstrap), [`GOODBYE_TIMEOUT`]
//! (the shutdown handshake) and [`CLEANUP_BUDGET`] (`terminate`), [`CLOSE_BUDGET`] in all. That
//! is what the host driver waits for the mosh sessions, beyond the grace of its other
//! terminals.
//!
//! [`HostHandle::peer_addr`]: crate::host::HostHandle::peer_addr

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::{Notify, mpsc, oneshot};
use tokio::time::timeout;

use super::connection::{Closing, SshHost, closed_reason};
use super::runtime;
use super::terminal_session::{not_installed, program, remote_failure};
use crate::host::{TerminalTarget, UserCancel};
use crate::mosh::{self, GOODBYE_TIMEOUT, MoshParams, Plan, run_session};
use crate::session::{CloseReason, Command, SessionDriver, SessionFailure, SessionState};
use crate::term::TerminalSize;
use crate::transport::DatagramTransport;

/// How much longer the bootstrap may take once the user has disconnected: long enough for the
/// exec that is probably already running to report its pid (so the server can be stopped),
/// short enough for a disconnect not to hang on a wedged host.
const ABANDON_GRACE: Duration = Duration::from_secs(2);
/// How long stopping a server nobody reached may take, whatever the host's exec timeout: a few
/// round trips (channel, exec, answer), with room for a slow link, and short enough for a
/// disconnect not to hang on a wedged host.
const CLEANUP_BUDGET: Duration = Duration::from_secs(5);
/// The longest a mosh session takes to close once the user has disconnected the host: a
/// bootstrap abandoned, the shutdown handshake and the cleanup, each at its own bound (they
/// never all apply to one session, so this is generous). The host driver waits this long for
/// mosh sessions; its other terminals get a shorter grace.
pub(super) const CLOSE_BUDGET: Duration = ABANDON_GRACE
    .saturating_add(GOODBYE_TIMEOUT)
    .saturating_add(CLEANUP_BUDGET);

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
    /// The user's disconnect of the host, which outlives the host driver (see [`watch_host`]).
    pub(super) user_cancel: UserCancel,
    /// How long to wait for the server's first datagram.
    pub(super) connect_timeout: Duration,
}

type Prepare<'a> =
    Pin<Box<dyn Future<Output = Result<Option<MoshParams>, SessionFailure>> + Send + 'a>>;

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
        user_cancel,
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
    runtime().spawn(watch_host(
        closing,
        user_cancel,
        shutdown.clone(),
        finished,
        tracker,
    ));
    // Dropped when this function returns, after the session's `Closed`.
    let _done = done;

    let mut size = size;
    // Set once the user has gone: `prepare` then starts nothing it has not started yet.
    let cancelled = AtomicBool::new(false);
    let mut prepare: Prepare<'_> = Box::pin(prepare(&host, &target, size, &cancelled));
    let mut abandoned = false;
    let prepared = loop {
        tokio::select! {
            result = &mut prepare => break result,
            command = driver.next_command() => match command {
                Command::Disconnect => {
                    abandoned = true;
                    cancelled.store(true, Ordering::SeqCst);
                    break abandon(&mut prepare).await;
                }
                Command::Resize(new) => size = new,
                // Nothing to send to yet, and host keys do not exist.
                _ => {}
            },
            () = shutdown.notified() => {
                abandoned = true;
                cancelled.store(true, Ordering::SeqCst);
                break abandon(&mut prepare).await;
            }
        }
    };
    let mut params = match prepared {
        Ok(Some(params)) => params,
        // Cancelled before a server was started: nothing to stop.
        Ok(None) => {
            driver.close(CloseReason::Disconnected);
            return;
        }
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
    // The address the SSH connection reached, with the server's port: an IPv6 scope id and
    // flow label stay (a link-local host is reachable only through its interface).
    let mut address = peer;
    address.set_port(params.port);
    let ended = run_session(
        Plan {
            transport: datagrams,
            peer: address,
            params,
            health: None,
            roam: Arc::new(Notify::new()),
            shutdown,
            connect_timeout,
        },
        &mut driver,
    )
    .await;
    if owes_cleanup(
        driver.state() == SessionState::Connected,
        &ended.reason,
        ended.server_gone,
    ) {
        cleanup(&host, pid).await;
    }
    driver.close(ended.reason);
}

/// Whether a session that ended with `reason` must stop its server over SSH: one that never
/// connected (UDP blocked, disconnected first), one that failed after connecting (an internal
/// error: nobody can reattach, the key lived only in memory), and one the user disconnected
/// whose goodbye the server never confirmed (`server_gone` false: the handshake timed out,
/// say after the outbound UDP path broke, so the server may still be running). `mosh-server`
/// would otherwise wait for a client for as long as it lives. A server the peer itself
/// confirmed gone (it acknowledged the goodbye, or announced its own end) needs nothing.
fn owes_cleanup(connected: bool, reason: &CloseReason, server_gone: bool) -> bool {
    !server_gone
        && (!connected || matches!(reason, CloseReason::Failed(_) | CloseReason::Disconnected))
}

/// The probe, then (for tmux and herdr) the target's command, then `mosh-server new` running
/// it. `NotInstalled { program: "mosh-server" }` is decided before anything else runs, so a
/// host without mosh does not get a pane focus first.
///
/// `cancelled` is set when the user disconnects. It is checked before each step with a side
/// effect (the pane focus, the exec that starts the server), and `None` comes back then: a
/// disconnect never focuses a pane or starts a server it would only have to stop. A step
/// already running is not interrupted here (the caller bounds the wait).
async fn prepare(
    host: &SshHost,
    target: &TerminalTarget,
    size: TerminalSize,
    cancelled: &AtomicBool,
) -> Result<Option<MoshParams>, SessionFailure> {
    let capabilities = host.capabilities().await.map_err(remote_failure)?;
    if capabilities.mosh_server.is_none() {
        return Err(not_installed("mosh-server"));
    }
    if cancelled.load(Ordering::SeqCst) {
        return Ok(None);
    }
    let argv = match program(host, target).await? {
        // The command goes to mosh-server as its arguments, where an environment has no
        // place; SSH would run it. Today's targets have none, and one that did must not
        // silently run without it.
        Some(command) => command.argv().ok_or_else(|| {
            SessionFailure::Internal(
                "the target's command sets environment variables, which mosh-server cannot \
                 pass on"
                    .into(),
            )
        })?,
        None => Vec::new(),
    };
    if cancelled.load(Ordering::SeqCst) {
        return Ok(None);
    }
    mosh::bootstrap(host, capabilities, size, &argv)
        .await
        .map(Some)
        .map_err(mosh::BootstrapError::into_failure)
}

/// After a disconnect: lets the bootstrap finish if it is about to, so the server it started
/// can be stopped. A server whose bootstrap is cut short loses its pid (contracts.md).
async fn abandon(prepare: &mut Prepare<'_>) -> Result<Option<MoshParams>, SessionFailure> {
    timeout(ABANDON_GRACE, prepare)
        .await
        .unwrap_or(Err(SessionFailure::TimedOut))
}

/// Stops a server that no client will use. Best effort and bounded ([`CLEANUP_BUDGET`]): the
/// host connection may be gone (then the server stays until someone stops it, as documented).
async fn cleanup(host: &SshHost, pid: Option<u32>) {
    if let Some(pid) = pid {
        let budget = CLEANUP_BUDGET.min(host.exec_timeout());
        let _ = timeout(budget, mosh::terminate(host, pid)).await;
    }
}

/// Ties the session to the host's end. A user disconnect of the host ends the session through
/// `shutdown`, and the watcher keeps its `tracker` until the session has closed, so the host
/// reports `Closed` after it, as for SSH terminals. Two signals say the user has gone: the
/// host's `closing` reason `Disconnected`, and `user_cancel`, which belongs to the host's
/// shared state and so also works after the host driver has exited, and when the disconnect
/// races the loss and the driver never processes it.
///
/// Any other end of the host (`closing` with a failure) is a loss: the session carries on
/// without the SSH connection and the tracker is released at once, so the host does not wait
/// for it. The watcher then stays subscribed to `user_cancel`: the user can still disconnect
/// (or release) the lost host later, which must close the surviving session. Nothing is held
/// for the host then (its `Closed` was already delivered, once).
async fn watch_host(
    mut closing: Closing,
    mut user_cancel: UserCancel,
    shutdown: Arc<Notify>,
    mut finished: oneshot::Receiver<()>,
    tracker: mpsc::Sender<()>,
) {
    let mut tracker = Some(tracker);
    let user_gone = tokio::select! {
        reason = closed_reason(&mut closing) => {
            if reason == CloseReason::Disconnected {
                true
            } else {
                // A loss: the session outlives the connection; the host need not wait.
                tracker = None;
                tokio::select! {
                    () = cancelled(&mut user_cancel) => true,
                    _ = &mut finished => false,
                }
            }
        }
        () = cancelled(&mut user_cancel) => true,
        _ = &mut finished => false,
    };
    if user_gone {
        shutdown.notify_one();
        // Resolves when the session's task is done (its sender is dropped); the tracker, if
        // still held, keeps the host's `Closed` behind the session's.
        let _ = (&mut finished).await;
    }
    drop(tracker);
}

/// Resolves once the user has ended the host. A host whose state vanished reads as ended.
async fn cancelled(user_cancel: &mut UserCancel) {
    let _ = user_cancel.wait_for(|cancelled| *cancelled).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed() -> CloseReason {
        CloseReason::Failed(SessionFailure::Internal("a screen fault".into()))
    }

    #[test]
    fn a_session_stops_its_server_unless_the_peer_confirmed_its_end() {
        // Never connected: whatever the end.
        for reason in [
            CloseReason::Disconnected,
            CloseReason::Failed(SessionFailure::TimedOut),
            CloseReason::RemoteExited { exit_status: None },
        ] {
            assert!(owes_cleanup(false, &reason, false), "{reason:?}");
        }
        // Connected: a failure leaves a server nobody can reattach to, so it is stopped.
        assert!(owes_cleanup(true, &failed(), false));
        assert!(owes_cleanup(
            true,
            &CloseReason::Failed(SessionFailure::TimedOut),
            false
        ));
        // Connected, disconnected by the user, but the goodbye was never acknowledged (it
        // timed out): the server may be running, so it is stopped over SSH.
        assert!(owes_cleanup(true, &CloseReason::Disconnected, false));
        // The peer confirmed the end: the user's goodbye acknowledged, or the server's own.
        assert!(!owes_cleanup(true, &CloseReason::Disconnected, true));
        assert!(!owes_cleanup(
            true,
            &CloseReason::RemoteExited { exit_status: None },
            true
        ));
    }

    #[test]
    fn the_hosts_wait_for_mosh_sessions_covers_every_step_of_closing_one() {
        assert!(CLOSE_BUDGET >= ABANDON_GRACE + GOODBYE_TIMEOUT + CLEANUP_BUDGET);
        // It outlasts the grace the host gives its other terminals, which is the point.
        assert!(CLOSE_BUDGET > Duration::from_secs(3));
    }
}
