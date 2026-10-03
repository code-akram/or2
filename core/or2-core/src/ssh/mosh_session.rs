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
//! reports its end. A stop that cannot run because the SSH server has no free session channel
//! (OpenSSH's `MaxSessions`) is not forgotten: the host keeps it as a [`ServerDebt`] and
//! retries, with bounds, when capacity returns. A start may also carry an absolute deadline
//! (AUTO's budget): when it passes before the session is `Connected`, the same cleanup runs and
//! the session closes `Failed { TimedOut }`.
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::{Notify, mpsc, oneshot};
use tokio::time::{Instant, sleep, sleep_until, timeout};

use super::connection::{Closing, SshHost, closed_reason};
use super::runtime;
use super::terminal_session::{Planned, not_installed, plan, remote_failure};
use crate::host::{TerminalTarget, UserCancel};
use crate::mosh::{self, GOODBYE_TIMEOUT, MoshParams, Plan, run_session};
use crate::remote::{RemoteError, RemoteHost};
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
/// How often an owed stop is tried again while the host has no free channel for it, and how
/// many times: long enough for terminals to close and free a channel, short enough that a
/// host that never frees one is given up on (and reported, see [`ServerDebt::stranded`]).
const DEBT_PACING: Pacing = Pacing {
    interval: Duration::from_secs(1),
    attempts: 20,
};
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
    /// How long to wait for the server's first datagram, from the end of the bootstrap.
    pub(super) connect_timeout: Duration,
    /// An absolute moment by which the session must be `Connected` (bootstrap, socket and first
    /// datagram all spend from it), when the caller gave a budget (AUTO's). Past it the session
    /// stops what it started and closes `Failed { TimedOut }`.
    pub(super) deadline: Option<Instant>,
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
        deadline,
    } = open;
    let Some(peer) = peer else {
        driver.close(CloseReason::Failed(SessionFailure::Internal(
            "the host connection cannot say which address it reached, so mosh has no \
             address to pin"
                .into(),
        )));
        return;
    };
    // Dropped (released) when this returns, after the session's `Closed`.
    let _release = host.tmux_release(driver.client_id(), &closing);
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
    // The server a finished bootstrap started, until `prepare` has handed it on or stopped it
    // itself: the pane focus beside the bootstrap can still be pending when the start is given
    // up, and the dropped `prepare` takes the bootstrap's result with it.
    let started = Started::default();
    let mut prepare: Prepare<'_> = Box::pin(prepare(
        &host,
        &target,
        driver.client_id().map(str::to_owned),
        size,
        &cancelled,
        &started,
    ));
    let mut abandoned = false;
    let mut expired = false;
    // Never fires without a budget.
    let expiry = async {
        match deadline {
            Some(deadline) => sleep_until(deadline).await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(expiry);
    let prepared = loop {
        tokio::select! {
            result = &mut prepare => break result,
            () = &mut expiry, if !expired => {
                // The budget is spent before a server answered: start nothing new, let a
                // running exec finish for the grace so its server can be stopped.
                expired = true;
                cancelled.store(true, Ordering::SeqCst);
                break abandon(&mut prepare).await;
            }
            command = driver.next_command() => match command {
                Command::Disconnect => {
                    abandoned = true;
                    cancelled.store(true, Ordering::SeqCst);
                    break abandon(&mut prepare).await;
                }
                Command::Resize(new) => size = new,
                // Nothing to send to yet.
                _ => {}
            },
            () = shutdown.notified() => {
                abandoned = true;
                cancelled.store(true, Ordering::SeqCst);
                break abandon(&mut prepare).await;
            }
        }
    };
    // A preparation cut short (the grace ran out) while its bootstrap had finished: the server
    // exists and its pid is only here.
    drop(prepare);
    if let Some(pid) = started.take() {
        stop_server(&host, Some(pid), DEBT_PACING).await;
    }
    // Why a start that is given up ends: the user's disconnect, or the spent budget.
    let given_up = if abandoned {
        CloseReason::Disconnected
    } else {
        CloseReason::Failed(SessionFailure::TimedOut)
    };
    let mut params = match prepared {
        Ok(Some(params)) if !abandoned && !expired => params,
        Ok(Some(params)) => {
            // The server exists and nobody will ever connect to it.
            stop_server(&host, params.server_pid, DEBT_PACING).await;
            driver.close(given_up);
            return;
        }
        // Cancelled before a server was started: nothing to stop.
        Ok(None) => {
            driver.close(given_up);
            return;
        }
        Err(failure) => {
            // Whoever won (the user's disconnect, the spent budget) keeps its reason: an
            // overdue bootstrap that fails during the grace is not what the session ends with
            // (AUTO's fallback is chosen by `TimedOut`). Nothing else is owed here: a failed
            // bootstrap that did start a server stopped it itself.
            driver.close(if abandoned || expired {
                given_up
            } else {
                CloseReason::Failed(failure)
            });
            return;
        }
    };
    let pid = params.server_pid;
    driver.set_server_pid(pid);
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
            deadline,
        },
        &mut driver,
    )
    .await;
    if owes_cleanup(
        driver.state() == SessionState::Connected,
        &ended.reason,
        ended.server_gone,
    ) {
        stop_server(&host, pid, DEBT_PACING).await;
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
/// it (beside the pane focus a herdr pane owes). `NotInstalled { program: "mosh-server" }` is decided before anything else runs, so a
/// host without mosh does not get a pane focus first.
///
/// `cancelled` is set when the user disconnects. It is checked before each step with a side
/// effect (the pane focus, the exec that starts the server), and `None` comes back then: a
/// disconnect never focuses a pane or starts a server it would only have to stop. A step
/// already running is not interrupted here (the caller bounds the wait).
async fn prepare(
    shared: &Arc<SshHost>,
    target: &TerminalTarget,
    client_id: Option<String>,
    size: TerminalSize,
    cancelled: &AtomicBool,
    started: &Started,
) -> Result<Option<MoshParams>, SessionFailure> {
    let host: &SshHost = shared;
    let capabilities = host.programs().await.map_err(remote_failure)?;
    if capabilities.mosh_server.is_none() {
        return Err(not_installed("mosh-server"));
    }
    if cancelled.load(Ordering::SeqCst) {
        return Ok(None);
    }
    let Planned { command, focus } = plan(host, target, client_id.as_deref()).await?;
    let argv = match command {
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
    let bootstrap = async {
        let params = mosh::bootstrap(host, capabilities, size, &argv)
            .await
            .map_err(mosh::BootstrapError::into_failure)?;
        started.record(params.server_pid);
        Ok(params)
    };
    let Some(focus) = focus else {
        return bootstrap.await.map(|params| {
            started.take();
            Some(params)
        });
    };
    // The pane focus and the server's start do not depend on each other (the herdr client
    // `mosh-server` runs follows herdr's focus), so they share their round trips. A focus that
    // fails (the pane is gone) leaves a server nobody will use: it is stopped before the
    // failure is reported.
    let (focused, bootstrapped) = tokio::join!(focus.run(shared), bootstrap);
    let result = match (focused, bootstrapped) {
        (Ok(()), bootstrapped) => bootstrapped.map(Some),
        (Err(failure), Ok(params)) => {
            // Still recorded in `started` while the stop runs: a cancellation in the middle of
            // it leaves the pid to the caller, which stops it again (a stop is idempotent).
            stop_server(shared, params.server_pid, DEBT_PACING).await;
            Err(failure)
        }
        (Err(failure), Err(_)) => Err(failure),
    };
    // Handed on (the caller owns the params' pid) or stopped: nothing is left for the caller.
    started.take();
    result
}

/// The pid of a server whose start finished inside a `prepare` that may still be cancelled.
#[derive(Default)]
struct Started(Mutex<Option<u32>>);

impl Started {
    fn record(&self, pid: Option<u32>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = pid;
    }

    fn take(&self) -> Option<u32> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// After a disconnect: lets the bootstrap finish if it is about to, so the server it started
/// can be stopped. A server whose bootstrap is cut short loses its pid (contracts.md).
async fn abandon(prepare: &mut Prepare<'_>) -> Result<Option<MoshParams>, SessionFailure> {
    timeout(ABANDON_GRACE, prepare)
        .await
        .unwrap_or(Err(SessionFailure::TimedOut))
}

/// How an owed stop is retried.
#[derive(Clone, Copy)]
struct Pacing {
    interval: Duration,
    attempts: u32,
}

/// What a mosh cleanup needs from the host: a way to run `terminate`, and the host's ledger of
/// stops it still owes.
pub(super) trait StopHost: RemoteHost {
    /// The servers this host still has to stop, and those it gave up on.
    fn debt(&self) -> &ServerDebt;
    /// The connection is gone: nothing can be stopped over it any more.
    fn is_closed(&self) -> bool;
    /// How long one request to the server may take.
    fn exec_timeout(&self) -> Duration;
}

impl StopHost for SshHost {
    fn debt(&self) -> &ServerDebt {
        &self.servers
    }

    fn is_closed(&self) -> bool {
        SshHost::is_closed(self)
    }

    fn exec_timeout(&self) -> Duration {
        SshHost::exec_timeout(self)
    }
}

/// The servers a host owes a stop. A stop needs a fresh SSH exec channel, and a server that
/// refuses one (OpenSSH's `MaxSessions` of 10, all taken by terminals) must not make the stop
/// disappear with the session that wanted it: the server's pid stays here, and is tried again
/// when capacity returns, until it is stopped, the connection is gone, or the attempts run out.
/// A stop that cannot be done any more is **stranded**: kept for [`ServerDebt::stranded`] so
/// the failure is reported, not silent (the server stays on the host until someone stops it).
#[derive(Default)]
pub(super) struct ServerDebt {
    state: Mutex<DebtState>,
}

#[derive(Default)]
struct DebtState {
    owed: Vec<u32>,
    stranded: Vec<u32>,
}

impl ServerDebt {
    fn state(&self) -> std::sync::MutexGuard<'_, DebtState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn owe(&self, pid: u32) {
        let mut state = self.state();
        if !state.owed.contains(&pid) {
            state.owed.push(pid);
        }
    }

    fn is_owed(&self, pid: u32) -> bool {
        self.state().owed.contains(&pid)
    }

    fn owed(&self) -> Vec<u32> {
        self.state().owed.clone()
    }

    fn settled(&self, pid: u32) {
        self.state().owed.retain(|owed| *owed != pid);
    }

    fn strand(&self, pid: u32) {
        let mut state = self.state();
        state.owed.retain(|owed| *owed != pid);
        if !state.stranded.contains(&pid) {
            state.stranded.push(pid);
        }
    }

    /// The pids of servers whose stop was given up on (the connection was lost, or no channel
    /// came free in time): they are still running on the host.
    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn stranded(&self) -> Vec<u32> {
        self.state().stranded.clone()
    }
}

/// The outcome of one try at stopping a server.
enum Stop {
    Done,
    /// No channel, or the host was too slow: worth another try.
    Later,
    /// The connection is gone: nothing more can be done over it.
    Gone,
}

/// One try, bounded ([`CLEANUP_BUDGET`], or the host's exec timeout when shorter).
async fn try_stop(host: &impl StopHost, pid: u32) -> Stop {
    let budget = CLEANUP_BUDGET.min(host.exec_timeout());
    match timeout(budget, mosh::terminate(host, pid)).await {
        Ok(Ok(())) => Stop::Done,
        Ok(Err(RemoteError::Closed)) => Stop::Gone,
        _ if host.is_closed() => Stop::Gone,
        _ => Stop::Later,
    }
}

/// Stops a server that no client will use, bounded ([`CLEANUP_BUDGET`]). If that fails because
/// the host has no free channel (or answered too slowly) the stop is not dropped: the pid goes
/// into the host's [`ServerDebt`] and a background task retries every `pacing.interval`, up to
/// `pacing.attempts` times. If the host connection is already gone the server stays until
/// someone stops it, as documented, and the pid is recorded as stranded.
async fn stop_server<H: StopHost>(host: &Arc<H>, pid: Option<u32>, pacing: Pacing) {
    let Some(pid) = pid else { return };
    match try_stop(&**host, pid).await {
        Stop::Done => {}
        Stop::Gone => host.debt().strand(pid),
        Stop::Later => {
            host.debt().owe(pid);
            let host = Arc::clone(host);
            runtime().spawn(async move {
                for _ in 0..pacing.attempts {
                    sleep(pacing.interval).await;
                    // Settled meanwhile (the host's closing pass, or another owed stop).
                    if !host.debt().is_owed(pid) {
                        return;
                    }
                    match try_stop(&*host, pid).await {
                        Stop::Done => return host.debt().settled(pid),
                        Stop::Gone => break,
                        Stop::Later => {}
                    }
                }
                host.debt().strand(pid);
            });
        }
    }
}

/// The host's last chance before it closes after a user disconnect: its terminals are closed,
/// so channels are free. One try for every stop still owed, bounded by [`CLEANUP_BUDGET`] in
/// all; what still fails is stranded.
pub(super) async fn settle_debts<H: StopHost>(host: &H) {
    let _ = timeout(CLEANUP_BUDGET, async {
        for pid in host.debt().owed() {
            match try_stop(host, pid).await {
                Stop::Done => host.debt().settled(pid),
                Stop::Later | Stop::Gone => host.debt().strand(pid),
            }
        }
    })
    .await;
    for pid in host.debt().owed() {
        host.debt().strand(pid);
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

    /// A host whose exec channels are refused while `refuse` is above zero (each refusal
    /// counts it down), like an sshd at `MaxSessions`; `closed` makes it a lost connection.
    #[derive(Default)]
    struct FakeHost {
        refuse: std::sync::atomic::AtomicU32,
        closed: AtomicBool,
        runs: std::sync::atomic::AtomicU32,
        /// The exit status the stop command reports (0: it worked).
        status: std::sync::atomic::AtomicU32,
        debt: ServerDebt,
    }

    impl RemoteHost for FakeHost {
        type Stream = tokio::io::DuplexStream;

        async fn exec_rendered(
            &self,
            _line: &str,
        ) -> Result<crate::remote::ExecOutput, RemoteError> {
            if self.closed.load(Ordering::SeqCst) {
                return Err(RemoteError::Closed);
            }
            let left = self.refuse.load(Ordering::SeqCst);
            if left > 0 {
                self.refuse.store(left - 1, Ordering::SeqCst);
                return Err(RemoteError::Rejected("ResourceShortage".into()));
            }
            self.runs.fetch_add(1, Ordering::SeqCst);
            Ok(crate::remote::ExecOutput {
                status: Some(self.status.load(Ordering::SeqCst)),
                stdout: crate::remote::SecretBytes::new(),
                stderr: crate::remote::SecretBytes::new(),
            })
        }

        async fn open_unix(&self, _path: &str) -> Result<Self::Stream, RemoteError> {
            Err(RemoteError::Closed)
        }
    }

    impl StopHost for FakeHost {
        fn debt(&self) -> &ServerDebt {
            &self.debt
        }

        fn is_closed(&self) -> bool {
            self.closed.load(Ordering::SeqCst)
        }

        fn exec_timeout(&self) -> Duration {
            Duration::from_secs(1)
        }
    }

    const FAST: Pacing = Pacing {
        interval: Duration::from_millis(10),
        attempts: 5,
    };

    async fn until(what: &str, mut condition: impl FnMut() -> bool) {
        for _ in 0..300 {
            if condition() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {what}");
    }

    #[tokio::test]
    async fn a_stop_with_a_free_channel_runs_at_once_and_owes_nothing() {
        let host = Arc::new(FakeHost::default());
        stop_server(&host, Some(4242), FAST).await;
        assert_eq!(host.runs.load(Ordering::SeqCst), 1);
        assert!(host.debt.owed().is_empty());
        assert!(host.debt.stranded().is_empty());
        // No pid, nothing to stop.
        stop_server(&host, None, FAST).await;
        assert_eq!(host.runs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_stop_refused_for_want_of_a_channel_is_kept_and_retried_until_it_runs() {
        let host = Arc::new(FakeHost::default());
        host.refuse.store(3, Ordering::SeqCst);
        stop_server(&host, Some(4242), FAST).await;
        // The first try was refused: the debt is recorded, the stop has not run.
        assert_eq!(host.runs.load(Ordering::SeqCst), 0);
        until("the retries to stop the server", || {
            host.runs.load(Ordering::SeqCst) == 1
        })
        .await;
        until("the debt to be settled", || host.debt.owed().is_empty()).await;
        assert!(host.debt.stranded().is_empty());
    }

    #[tokio::test]
    async fn a_stop_that_never_finds_a_channel_is_given_up_on_and_reported() {
        let host = Arc::new(FakeHost::default());
        host.refuse.store(u32::MAX, Ordering::SeqCst);
        stop_server(&host, Some(4242), FAST).await;
        until("the retries to run out", || {
            !host.debt.stranded().is_empty()
        })
        .await;
        assert_eq!(host.debt.stranded(), [4242]);
        assert!(host.debt.owed().is_empty());
        assert_eq!(host.runs.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_stop_command_that_failed_keeps_the_debt_until_it_succeeds() {
        let host = Arc::new(FakeHost::default());
        // The exec ran but the stop did not happen (no `ps`, `kill` refused): still owed.
        host.status.store(3, Ordering::SeqCst);
        stop_server(
            &host,
            Some(4242),
            Pacing {
                interval: Duration::from_millis(30),
                attempts: 100,
            },
        )
        .await;
        assert_eq!(host.runs.load(Ordering::SeqCst), 1);
        assert_eq!(host.debt.owed(), [4242]);
        // The host is fixed: the retry succeeds and settles the debt.
        host.status.store(0, Ordering::SeqCst);
        until("the debt to be settled", || host.debt.owed().is_empty()).await;
        assert!(host.debt.stranded().is_empty());
    }

    #[tokio::test]
    async fn a_stop_command_that_keeps_failing_ends_stranded_not_settled() {
        let host = Arc::new(FakeHost::default());
        host.status.store(4, Ordering::SeqCst);
        stop_server(&host, Some(7), FAST).await;
        until("the retries to run out", || {
            !host.debt.stranded().is_empty()
        })
        .await;
        assert_eq!(host.debt.stranded(), [7]);
        assert!(host.debt.owed().is_empty());
    }

    #[tokio::test]
    async fn a_stop_over_a_lost_connection_is_stranded_at_once() {
        let host = Arc::new(FakeHost::default());
        host.closed.store(true, Ordering::SeqCst);
        stop_server(&host, Some(4242), FAST).await;
        assert_eq!(host.debt.stranded(), [4242]);
        assert!(host.debt.owed().is_empty());
    }

    #[tokio::test]
    async fn a_connection_lost_between_retries_strands_the_stop() {
        let host = Arc::new(FakeHost::default());
        host.refuse.store(u32::MAX, Ordering::SeqCst);
        stop_server(&host, Some(7), FAST).await;
        host.closed.store(true, Ordering::SeqCst);
        until("the stop to be stranded", || {
            !host.debt.stranded().is_empty()
        })
        .await;
        assert_eq!(host.debt.stranded(), [7]);
    }

    #[tokio::test]
    async fn the_closing_pass_settles_what_is_owed_and_strands_what_still_fails() {
        let host = Arc::new(FakeHost::default());
        host.refuse.store(1, Ordering::SeqCst);
        stop_server(
            &host,
            Some(1),
            Pacing {
                interval: Duration::from_secs(60),
                attempts: 1,
            },
        )
        .await;
        assert_eq!(host.debt.owed(), [1]);
        // Channels are free now: the last try before the host closes stops the server.
        settle_debts(&*host).await;
        assert_eq!(host.runs.load(Ordering::SeqCst), 1);
        assert!(host.debt.owed().is_empty() && host.debt.stranded().is_empty());

        host.refuse.store(2, Ordering::SeqCst);
        stop_server(
            &host,
            Some(2),
            Pacing {
                interval: Duration::from_secs(60),
                attempts: 1,
            },
        )
        .await;
        host.refuse.store(u32::MAX, Ordering::SeqCst);
        settle_debts(&*host).await;
        assert_eq!(host.debt.stranded(), [2]);
        assert!(host.debt.owed().is_empty());
    }

    #[test]
    fn the_hosts_wait_for_mosh_sessions_covers_every_step_of_closing_one() {
        assert!(CLOSE_BUDGET >= ABANDON_GRACE + GOODBYE_TIMEOUT + CLEANUP_BUDGET);
        // It outlasts the grace the host gives its other terminals, which is the point.
        assert!(CLOSE_BUDGET > Duration::from_secs(3));
    }
}
