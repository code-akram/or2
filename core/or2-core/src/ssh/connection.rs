//! The host driver: one SSH connection per host, shared by terminals, exec queries and herdr
//! watches (contracts.md, "Host connection").
//!
//! ```text
//! host thread (or2-host)            network task                 per terminal
//! HostDriver, commands,   events    race, relay, handshake,      thread (or2-terminal) with
//! callbacks               ◀───────  authentication, Arc<SshHost> its own engine and channel task
//!                         stop ───▶ liveness of the SSH session
//!                 closing (reason) ──▶ every terminal and watch
//! ```
//!
//! **Closing.** Whatever ends the host (user disconnect, release of the last handle, loss, a
//! failed handshake) is one `CloseReason`. The driver publishes it to every terminal and
//! watch, waits (bounded) until each has delivered its own `Closed`, tears the connection
//! down, and only then reports the host's `Closed`: terminals and watches close before the
//! host, matching the contract probe. The wait is bounded ([`SESSIONS_CLOSE_GRACE`]) because
//! a terminal thread stuck in a slow observer callback must not hold the host open forever;
//! a session that outlasts it can still deliver its `Closed` after the host's. Mosh sessions
//! have their own, longer bound ([`mosh_session::CLOSE_BUDGET`]): closing one can mean stopping
//! its server over this very connection, which takes round trips, and the connection must
//! stay up until that is done. A user
//! disconnect closes terminals with
//! `Disconnected` and sends each channel's close first; loss closes them with the failure the
//! connection ended with. Queries still running are dropped, so their callers see `Closed`.
//! Mosh terminals ([`mosh_session`]) are the exception: they need the connection only to
//! start, so a loss leaves them running; a user disconnect closes them like the others.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use russh::client::{self as russh_client, Handle};
use tokio::io::DuplexStream;
use tokio::sync::{OnceCell, mpsc, oneshot, watch};
use tokio::time::{Instant, sleep_until, timeout, timeout_at};

use super::client::{
    Client, HostKeyRequest, TransportEnd, authenticate, config, handshake_failure, relay,
};
use super::mosh_session;
use super::pump::{CHANNEL_CLOSE_GRACE, connection_error, internal, lost};
use super::runtime;
use super::terminal_session;
use crate::herdr::{self, HerdrState, HerdrUnavailable, HerdrWatchDriver};
use crate::host::{
    HostCapabilities, HostCommand, HostConnectRequest, HostDriver, HostError, HostHandle,
    HostObserver, HostState, TerminalTarget, TerminalTransport, TmuxSession, UserCancel,
};
use crate::probe;
use crate::remote::{ExecOutput, OUTPUT_CAP, RemoteError, RemoteHost, SecretBytes};
use crate::session::{CloseReason, HostKeyPrompt, SessionDriver, SessionFailure};
use crate::term::TerminalSize;
use crate::tmux::{self, TmuxError};
use crate::transport::{DatagramTransport, RACE_STAGGER, Transport, race};

/// Timings, adjustable so tests need not wait for the production values.
#[derive(Debug, Clone, Copy)]
pub struct HostOptions {
    /// TCP, handshake and authentication together. Paused while the user decides on a host
    /// key. Production: 20 s.
    pub connect_timeout: Duration,
    /// Each exec and each socket open. Production: `remote::EXEC_TIMEOUT`.
    pub exec_timeout: Duration,
    /// Delay between address attempts. Production: `transport::RACE_STAGGER`.
    pub stagger: Duration,
    /// How long a mosh terminal waits for the server's first datagram before it closes
    /// `TimedOut` (UDP blocked) and stops the server. Production: `mosh::CONNECT_TIMEOUT`.
    pub mosh_connect_timeout: Duration,
}

impl Default for HostOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(20),
            exec_timeout: crate::remote::EXEC_TIMEOUT,
            stagger: RACE_STAGGER,
            mosh_connect_timeout: crate::mosh::CONNECT_TIMEOUT,
        }
    }
}

/// How long closing waits for terminals and watches to deliver their `Closed`. Past it the
/// host closes anyway: the "sessions before host" order holds unless a session is wedged.
const SESSIONS_CLOSE_GRACE: Duration = Duration::from_secs(3);
/// How long a keepalive that [`network_changed`] queues may wait for the connection to take it.
/// Past it the connection is not draining (a blocked write): the keepalive is dropped, so calls
/// during a flapping network do not pile up tasks.
const KEEPALIVE_QUEUE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a watch whose capability probe failed waits before probing again.
const WATCH_RETRY: Duration = Duration::from_secs(10);

/// From the network task to the host thread.
enum HostEvent {
    HostKey(HostKeyPrompt, oneshot::Sender<bool>),
    Authenticating,
    Connected {
        address_index: usize,
        peer: Option<SocketAddr>,
        host: Arc<SshHost>,
    },
    TransportEnded(SessionFailure),
    Closed(CloseReason),
}

impl From<HostKeyRequest> for HostEvent {
    fn from(request: HostKeyRequest) -> Self {
        HostEvent::HostKey(request.prompt, request.reply)
    }
}

impl From<TransportEnd> for HostEvent {
    fn from(end: TransportEnd) -> Self {
        HostEvent::TransportEnded(end.0)
    }
}

/// The established SSH connection. Shared (`Arc`) by terminal channels, queries and herdr
/// watches; it is the [`RemoteHost`] they talk to.
pub(super) struct SshHost {
    handle: Handle<Client<HostEvent>>,
    exec_timeout: Duration,
    capabilities: OnceCell<HostCapabilities>,
    /// The last herdr session list read successfully (the probe's own list until then).
    sessions: probe::SessionsCache,
    /// The mosh servers this connection still has to stop (see [`mosh_session::ServerDebt`]).
    pub(super) servers: mosh_session::ServerDebt,
}

/// Every established SSH connection of the process, for [`network_changed`]. Weak: a closed
/// host is dropped by its owners, never kept alive here.
static LIVE_HOSTS: Mutex<Vec<Weak<SshHost>>> = Mutex::new(Vec::new());

fn register(host: &Arc<SshHost>) {
    let mut live = LIVE_HOSTS.lock().unwrap_or_else(PoisonError::into_inner);
    live.retain(|weak| weak.upgrade().is_some_and(|host| !host.handle.is_closed()));
    live.push(Arc::downgrade(host));
}

/// The device's network changed: send an SSH keepalive on every live connection at once, so a
/// connection that the change silently broke is noticed by the keepalive and TCP timeouts
/// from now, instead of after the next scheduled keepalive. Returns at once; the keepalives
/// are sent on the network runtime. A healthy connection is unaffected (the server's reply is
/// ignored).
pub(super) fn network_changed() {
    let hosts: Vec<Arc<SshHost>> = {
        let mut live = LIVE_HOSTS.lock().unwrap_or_else(PoisonError::into_inner);
        live.retain(|weak| weak.upgrade().is_some_and(|host| !host.handle.is_closed()));
        live.iter().filter_map(Weak::upgrade).collect()
    };
    for host in hosts {
        runtime().spawn(async move {
            let _ = timeout(KEEPALIVE_QUEUE_TIMEOUT, host.handle.send_keepalive(true)).await;
        });
    }
}

impl SshHost {
    /// The probe's answer, run once per connection. A failed probe is not cached.
    pub(super) async fn capabilities(&self) -> Result<&HostCapabilities, RemoteError> {
        self.capabilities
            .get_or_try_init(|| probe::probe(self))
            .await
    }

    /// How long one request to the server may take: an exec, a socket open, a terminal's
    /// channel setup.
    pub(super) fn exec_timeout(&self) -> Duration {
        self.exec_timeout
    }

    /// Whether the SSH connection is gone.
    pub(super) fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    /// A new session channel, for a terminal.
    pub(super) async fn open_channel(
        &self,
    ) -> Result<russh::Channel<russh_client::Msg>, russh::Error> {
        self.handle.channel_open_session().await
    }

    /// Runs `line` on `channel` without a PTY and collects it until the channel closes. No
    /// timing of its own: [`SshHost::run_exec`] bounds the whole exchange, because sending the
    /// request and the EOF await russh's bounded outbound queue just as the reads await the
    /// server.
    async fn collect(
        &self,
        channel: &mut russh::Channel<russh_client::Msg>,
        line: &str,
    ) -> Result<ExecOutput, RemoteError> {
        channel.exec(true, line).await.map_err(remote_error)?;
        // The command reads no input: give it EOF now, like a closed stdin.
        let _ = channel.eof().await;
        let mut output = ExecOutput {
            status: None,
            stdout: SecretBytes::new(),
            stderr: SecretBytes::new(),
        };
        let mut signalled = false;
        loop {
            match channel.wait().await {
                Some(russh::ChannelMsg::Data { data }) => {
                    output.stdout.extend_capped(&data, OUTPUT_CAP)?
                }
                Some(russh::ChannelMsg::ExtendedData { data, ext: 1 }) => {
                    output.stderr.extend_capped(&data, OUTPUT_CAP)?
                }
                Some(russh::ChannelMsg::ExitStatus { exit_status }) => {
                    output.status = Some(exit_status)
                }
                Some(russh::ChannelMsg::Failure) => {
                    return Err(RemoteError::Rejected("the host refused the command".into()));
                }
                Some(russh::ChannelMsg::ExitSignal { .. }) => signalled = true,
                Some(russh::ChannelMsg::Close) | None => {
                    // Ended by a signal (`status: None`), or an exit status arrived. Neither
                    // means the channel went away under us: the connection was lost.
                    return if output.status.is_some() || signalled {
                        Ok(output)
                    } else if self.handle.is_closed() {
                        Err(RemoteError::Closed)
                    } else {
                        Err(RemoteError::Io(
                            "the command ended without an exit status".into(),
                        ))
                    };
                }
                // Success, EOF, window adjusts.
                Some(_) => {}
            }
        }
    }
}

impl SshHost {
    /// Runs `line` on the freshly opened `channel` and fails with `TimedOut` unless the WHOLE
    /// exchange (the exec request, the EOF and collecting the output) is over by `deadline`.
    /// Sending either request awaits russh's bounded outbound queue, which a stalled
    /// connection fills (one terminal not draining its output blocks the shared loop), so the
    /// request and the EOF are no more immediate than the reads. The [`ExecChannel`] guard
    /// closes the channel whichever way this ends, cancellation included, and the guard's own
    /// close is bounded too.
    async fn run_exec(
        &self,
        channel: russh::Channel<russh_client::Msg>,
        line: &str,
        deadline: Instant,
    ) -> Result<ExecOutput, RemoteError> {
        let mut exec = ExecChannel(Some(channel));
        let result = match timeout_at(deadline, self.collect(exec.channel(), line)).await {
            Ok(result) => result,
            Err(_) => Err(RemoteError::TimedOut),
        };
        match result {
            Ok(_) => exec.finished(),
            Err(_) => exec.close().await,
        }
        result
    }
}

/// A session channel running an exec. Dropped while armed (the exec future was cancelled),
/// it closes the channel in the background; `finished` disarms it once the server has closed
/// the channel itself.
struct ExecChannel(Option<russh::Channel<russh_client::Msg>>);

impl ExecChannel {
    fn channel(&mut self) -> &mut russh::Channel<russh_client::Msg> {
        self.0.as_mut().expect("an armed exec channel")
    }

    fn finished(mut self) {
        self.0 = None;
    }

    /// Best effort and bounded: the connection may already be gone.
    async fn close(mut self) {
        if let Some(channel) = self.0.take() {
            let _ = timeout(CHANNEL_CLOSE_GRACE, channel.close()).await;
        }
    }
}

impl Drop for ExecChannel {
    fn drop(&mut self) {
        if let Some(channel) = self.0.take() {
            runtime().spawn(async move {
                let _ = timeout(CHANNEL_CLOSE_GRACE, channel.close()).await;
            });
        }
    }
}

fn remote_error(error: russh::Error) -> RemoteError {
    match error {
        russh::Error::ChannelOpenFailure(reason) => RemoteError::Rejected(format!("{reason:?}")),
        russh::Error::SendError
        | russh::Error::Disconnect
        | russh::Error::HUP
        | russh::Error::RecvError => RemoteError::Closed,
        russh::Error::IO(error) => RemoteError::Io(error.to_string()),
        error => RemoteError::Io(error.to_string()),
    }
}

/// [`remote_error`] for a `direct-streamlocal@openssh.com` open, where the refusal reason
/// carries the distinction `RemoteHost::open_unix` documents: OpenSSH answers `CONNECT_FAILED`
/// when connecting to the socket failed (missing, refused, no permission on it) and
/// `ADMINISTRATIVELY_PROHIBITED` when streamlocal forwarding is off (`AllowStreamLocalForwarding`,
/// `DisableForwarding`, `PermitOpen`). The first is [`RemoteError::Io`]; every other refusal is
/// a policy or resource decision of the server: [`RemoteError::Rejected`].
fn streamlocal_error(error: russh::Error) -> RemoteError {
    match error {
        russh::Error::ChannelOpenFailure(russh::ChannelOpenFailure::ConnectFailed) => {
            RemoteError::Io("the socket could not be connected to on the host".into())
        }
        error => remote_error(error),
    }
}

impl RemoteHost for SshHost {
    type Stream = russh::ChannelStream<russh_client::Msg>;

    /// One session channel per exec, no PTY. Output over the cap, a refused command and the
    /// timeout close the channel before failing, and so does dropping the future (a caller's
    /// own timeout or a cancelled query): an orphaned channel would keep counting toward the
    /// server's `MaxSessions`.
    async fn exec_rendered(&self, line: &str) -> Result<ExecOutput, RemoteError> {
        let deadline = Instant::now() + self.exec_timeout;
        let channel = timeout_at(deadline, self.handle.channel_open_session())
            .await
            .map_err(|_| RemoteError::TimedOut)?
            .map_err(remote_error)?;
        self.run_exec(channel, line, deadline).await
    }

    /// OpenSSH `direct-streamlocal@openssh.com`. A socket that is missing or refuses the
    /// connection fails the open with `CONNECT_FAILED`: `Io`. A server with streamlocal
    /// forwarding disabled (or any other refusal) is `Rejected` ([`streamlocal_error`]).
    async fn open_unix(&self, path: &str) -> Result<Self::Stream, RemoteError> {
        let channel = timeout(
            self.exec_timeout,
            self.handle.channel_open_direct_streamlocal(path),
        )
        .await
        .map_err(|_| RemoteError::TimedOut)?
        .map_err(streamlocal_error)?;
        Ok(channel.into_stream())
    }
}

pub(super) fn start<T: Transport>(
    transport: Arc<T>,
    request: HostConnectRequest,
    observer: Arc<dyn HostObserver>,
    options: HostOptions,
) -> HostHandle {
    start_datagrams(
        transport,
        Arc::new(crate::transport::DirectUdp),
        request,
        observer,
        options,
    )
}

/// [`start`] with the datagram transport that mosh terminals use.
pub(super) fn start_datagrams<T: Transport, D: DatagramTransport>(
    transport: Arc<T>,
    datagrams: Arc<D>,
    request: HostConnectRequest,
    observer: Arc<dyn HostObserver>,
    options: HostOptions,
) -> HostHandle {
    start_tapped_with(transport, datagrams, request, observer, options, None)
}

/// [`start`], also handing the established connection to `tap` (tests drive exec and
/// streamlocal directly through it).
#[cfg(any(test, feature = "test-support"))]
fn start_tapped<T: Transport>(
    transport: Arc<T>,
    request: HostConnectRequest,
    observer: Arc<dyn HostObserver>,
    options: HostOptions,
    tap: Option<oneshot::Sender<Arc<SshHost>>>,
) -> HostHandle {
    start_tapped_with(
        transport,
        Arc::new(crate::transport::DirectUdp),
        request,
        observer,
        options,
        tap,
    )
}

fn start_tapped_with<T: Transport, D: DatagramTransport>(
    transport: Arc<T>,
    datagrams: Arc<D>,
    request: HostConnectRequest,
    observer: Arc<dyn HostObserver>,
    options: HostOptions,
    tap: Option<oneshot::Sender<Arc<SshHost>>>,
) -> HostHandle {
    let (handle, mut driver) = crate::host::channel(observer);
    // Initialize before spawning so runtime initialization is never done in a callback.
    let runtime = runtime();
    std::thread::Builder::new()
        .name("or2-host".into())
        .spawn(move || {
            runtime.block_on(drive(
                transport,
                datagrams,
                request,
                &mut driver,
                options,
                tap,
            ))
        })
        .expect("create host thread");
    handle
}

/// The established connection as a [`RemoteHost`], for integration tests of exec limits and
/// streamlocal channels against a real sshd. Production code reaches it only through the
/// host driver.
#[cfg(any(test, feature = "test-support"))]
pub struct SshRemote(Arc<SshHost>);

#[cfg(any(test, feature = "test-support"))]
impl SshRemote {
    /// The pids of `mosh-server`s whose stop was given up on (still running on the host).
    pub fn stranded_servers(&self) -> Vec<u32> {
        self.0.servers.stranded()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl RemoteHost for SshRemote {
    type Stream = russh::ChannelStream<russh_client::Msg>;

    async fn exec_rendered(&self, line: &str) -> Result<ExecOutput, RemoteError> {
        self.0.exec_rendered(line).await
    }

    async fn open_unix(&self, path: &str) -> Result<Self::Stream, RemoteError> {
        self.0.open_unix(path).await
    }
}

/// [`start`] for tests: the host handle plus a receiver that yields the connection's
/// [`SshRemote`] once it is `Connected` (it never yields if the host fails first).
#[cfg(any(test, feature = "test-support"))]
pub fn connect_tapped<T: Transport>(
    transport: Arc<T>,
    request: HostConnectRequest,
    observer: Arc<dyn HostObserver>,
    options: HostOptions,
) -> (HostHandle, oneshot::Receiver<SshRemote>) {
    let (tap, tapped) = oneshot::channel();
    let (forward, forwarded) = oneshot::channel::<Arc<SshHost>>();
    let handle = start_tapped(transport, request, observer, options, Some(forward));
    runtime().spawn(async move {
        if let Ok(host) = forwarded.await {
            let _ = tap.send(SshRemote(host));
        }
    });
    (handle, tapped)
}

/// Publishes why the host closed to every terminal and watch.
pub(super) type Closing = watch::Receiver<Option<CloseReason>>;

/// Resolves with the host's close reason. A host driver that vanished without one reads as a
/// user disconnect.
pub(super) async fn closed_reason(closing: &mut Closing) -> CloseReason {
    match closing.wait_for(|reason| reason.is_some()).await {
        Ok(reason) => reason.clone().unwrap_or(CloseReason::Disconnected),
        Err(_) => CloseReason::Disconnected,
    }
}

async fn drive<T: Transport, D: DatagramTransport>(
    transport: Arc<T>,
    datagrams: Arc<D>,
    request: HostConnectRequest,
    driver: &mut HostDriver,
    options: HostOptions,
    mut tap: Option<oneshot::Sender<Arc<SshHost>>>,
) {
    let (events, mut incoming) = mpsc::channel(32);
    let (shutdown, stop) = watch::channel(false);
    let mut network_ended = false;
    let mut network = tokio::spawn(async move {
        let reason = network(transport, request, events.clone(), options, stop).await;
        let _ = events.send(HostEvent::Closed(reason)).await;
    });
    let (closing_sender, closing) = watch::channel(None);
    // Every terminal thread and watch task holds a clone; `drained.recv()` returns `None`
    // once they are all gone.
    let (tracker, mut drained) = mpsc::channel::<()>(1);
    // The same for mosh sessions, which are waited for longer (see `mosh_session::CLOSE_BUDGET`).
    let (mosh_tracker, mut mosh_drained) = mpsc::channel::<()>(1);
    let user_cancel = driver.user_cancel();
    let mut connected: Option<Arc<SshHost>> = None;
    let mut peer_addr: Option<SocketAddr> = None;
    let mut decision = None;
    let mut deadline = Instant::now() + options.connect_timeout;
    let mut remaining = options.connect_timeout;
    let mut timing = true;
    let outcome: Result<CloseReason, SessionFailure> = async {
        loop {
            tokio::select! {
                Some(event) = incoming.recv() => match event {
                    HostEvent::HostKey(prompt, reply) => {
                        remaining = deadline.saturating_duration_since(Instant::now());
                        timing = false; // Human confirmation is bounded by the server, not us.
                        decision = Some(reply);
                        driver.transition(HostState::AwaitingHostKey(prompt)).map_err(internal)?;
                    }
                    HostEvent::Authenticating => {
                        driver.transition(HostState::Authenticating).map_err(internal)?;
                    }
                    HostEvent::Connected { address_index, peer, host } => {
                        timing = false;
                        driver.set_peer_addr(peer);
                        peer_addr = peer;
                        if let Some(tap) = tap.take() {
                            let _ = tap.send(Arc::clone(&host));
                        }
                        connected = Some(host);
                        driver
                            .transition(HostState::Connected { address_index })
                            .map_err(internal)?;
                    }
                    HostEvent::TransportEnded(failure) => {
                        // Only while the prompt blocks russh; otherwise the SSH session reports
                        // its own end.
                        if decision.is_some() {
                            break Ok(CloseReason::Failed(failure));
                        }
                    }
                    HostEvent::Closed(reason) => break Ok(reason),
                },
                command = driver.next_command() => match command {
                    HostCommand::Disconnect => break Ok(CloseReason::Disconnected),
                    HostCommand::RejectHostKey => {
                        break Ok(CloseReason::Failed(SessionFailure::HostKeyRejected));
                    }
                    HostCommand::ApproveHostKey { .. } => {
                        if let Some(reply) = decision.take() {
                            let _ = reply.send(true);
                            deadline = Instant::now() + remaining;
                            timing = true;
                        }
                    }
                    // Handles only send these once connected; a command that races a failed
                    // handshake is failed by `transition(Closed)`.
                    command => {
                        if let Some(host) = &connected {
                            let mosh = MoshContext {
                                datagrams: &datagrams,
                                peer: peer_addr,
                                connect_timeout: options.mosh_connect_timeout,
                                tracker: &mosh_tracker,
                                user_cancel: &user_cancel,
                            };
                            dispatch(command, host, &closing, &tracker, &mosh);
                        }
                    }
                },
                () = sleep_until(deadline), if timing => {
                    break Ok(CloseReason::Failed(SessionFailure::TimedOut));
                }
                result = &mut network, if !network_ended => {
                    network_ended = true;
                    // A task that returns has queued its `Closed`, which the first arm takes
                    // next. One that panicked has not: without this the host would sit in its
                    // last state forever, its terminals and watches never told.
                    if result.is_err() {
                        break Err(internal("the host connection task failed"));
                    }
                }
            }
        }
    }
    .await;
    let reason = outcome.unwrap_or_else(CloseReason::Failed);
    // Terminals and watches first: tell them why, then wait until each has said `Closed`.
    closing_sender.send_replace(Some(reason.clone()));
    drop((tracker, mosh_tracker));
    let closing_since = Instant::now();
    let _ = timeout(SESSIONS_CLOSE_GRACE, drained.recv()).await;
    // Counted from the same moment: the mosh sessions have been closing meanwhile. A loss
    // releases their trackers at once (they outlive the connection), so only a user disconnect
    // waits here.
    let _ = timeout_at(
        closing_since + mosh_session::CLOSE_BUDGET,
        mosh_drained.recv(),
    )
    .await;
    if reason == CloseReason::Disconnected && !network_ended {
        // Every terminal has closed, so channels are free: stops that were waiting for one get
        // their last try while the connection is still up.
        if let Some(host) = &connected {
            mosh_session::settle_debts(&**host).await;
        }
        shutdown.send_replace(true);
        // Let the SSH disconnect flush, but never wait indefinitely for a peer.
        let _ = timeout(Duration::from_millis(400), &mut network).await;
    }
    network.abort();
    if !network.is_finished() {
        let _ = network.await;
    }
    driver.close(reason);
}

/// Starts the work a connected-host command asks for. Everything long-running is its own
/// thread or task holding `tracker`, and ends when `closing` fires.
fn dispatch<D: DatagramTransport>(
    command: HostCommand,
    host: &Arc<SshHost>,
    closing: &Closing,
    tracker: &mpsc::Sender<()>,
    mosh: &MoshContext<'_, D>,
) {
    match command {
        HostCommand::OpenTerminal {
            target,
            transport: TerminalTransport::Ssh,
            size,
            driver,
        } => spawn_terminal(host, target, size, driver, closing, tracker),
        HostCommand::OpenTerminal {
            target,
            transport: TerminalTransport::Mosh,
            size,
            driver,
        } => spawn_mosh(host, target, size, driver, closing, mosh),
        HostCommand::Capabilities { reply } => {
            let host = Arc::clone(host);
            let (mut closing, tracker) = (closing.clone(), tracker.clone());
            runtime().spawn(async move {
                let _tracker = tracker;
                // Programs and locale are the cached probe; herdr's session list is read
                // again so `running` and new sessions show, and a failed read reports the
                // last list that was read, not the one from connect time.
                let query = async {
                    let cached = host.capabilities().await.map_err(host_error)?;
                    host.sessions
                        .capabilities(&*host, cached)
                        .await
                        .map_err(host_error)
                };
                // A host that closes mid-query drops `reply`: the caller sees `Closed`.
                tokio::select! {
                    result = query => { let _ = reply.send(result); }
                    _ = closed_reason(&mut closing) => {}
                }
            });
        }
        HostCommand::ListTmux { reply } => {
            let host = Arc::clone(host);
            let (mut closing, tracker) = (closing.clone(), tracker.clone());
            runtime().spawn(async move {
                let _tracker = tracker;
                tokio::select! {
                    result = list_tmux(&host) => { let _ = reply.send(result); }
                    _ = closed_reason(&mut closing) => {}
                }
            });
        }
        HostCommand::FocusHerdrPane {
            session,
            pane_id,
            reply,
        } => {
            let host = Arc::clone(host);
            let (mut closing, tracker) = (closing.clone(), tracker.clone());
            runtime().spawn(async move {
                let _tracker = tracker;
                tokio::select! {
                    result = focus_herdr_pane(&host, session, pane_id) => { let _ = reply.send(result); }
                    _ = closed_reason(&mut closing) => {}
                }
            });
        }
        HostCommand::WatchHerdr { session, driver } => {
            let host = Arc::clone(host);
            let (mut closing, tracker) = (closing.clone(), tracker.clone());
            runtime().spawn(async move {
                let _tracker = tracker;
                // Losing the race drops the watch, which closes it: `Closed` is delivered once.
                tokio::select! {
                    () = watch_herdr(host, session, driver) => {}
                    _ = closed_reason(&mut closing) => {}
                }
            });
        }
        HostCommand::ApproveHostKey { .. }
        | HostCommand::RejectHostKey
        | HostCommand::Disconnect => {}
    }
}

/// What a mosh terminal needs beyond what every command gets.
struct MoshContext<'a, D> {
    datagrams: &'a Arc<D>,
    peer: Option<SocketAddr>,
    connect_timeout: Duration,
    /// Held by every mosh session until it has closed.
    tracker: &'a mpsc::Sender<()>,
    /// The user's disconnect, which outlives this driver (a mosh session survives a lost
    /// connection and must still be closed by the user's disconnect of the host).
    user_cancel: &'a UserCancel,
}

fn spawn_mosh<D: DatagramTransport>(
    host: &Arc<SshHost>,
    target: TerminalTarget,
    size: TerminalSize,
    session: SessionDriver,
    closing: &Closing,
    mosh: &MoshContext<'_, D>,
) {
    let open = mosh_session::Open {
        host: Arc::clone(host),
        datagrams: Arc::clone(mosh.datagrams),
        peer: mosh.peer,
        target,
        size,
        closing: closing.clone(),
        tracker: mosh.tracker.clone(),
        user_cancel: mosh.user_cancel.clone(),
        connect_timeout: mosh.connect_timeout,
    };
    // A failed spawn drops the closure and with it the driver, which closes the session with
    // `Failed(Internal)`.
    let _ = std::thread::Builder::new()
        .name("or2-mosh-open".into())
        .spawn(move || runtime().block_on(mosh_session::drive(open, session)));
}

fn spawn_terminal(
    host: &Arc<SshHost>,
    target: TerminalTarget,
    size: TerminalSize,
    session: SessionDriver,
    closing: &Closing,
    tracker: &mpsc::Sender<()>,
) {
    let (host, closing, tracker) = (Arc::clone(host), closing.clone(), tracker.clone());
    // A failed spawn drops the closure and with it the driver, which closes the session with
    // `Failed(Internal)`.
    let _ = std::thread::Builder::new()
        .name("or2-terminal".into())
        .spawn(move || {
            let _tracker = tracker;
            runtime().block_on(terminal_session::drive(
                host, target, size, session, closing,
            ));
        });
}

fn host_error(error: RemoteError) -> HostError {
    match error {
        RemoteError::Closed => HostError::Closed,
        error => HostError::CommandFailed {
            message: error.to_string(),
        },
    }
}

async fn list_tmux(host: &SshHost) -> Result<Vec<TmuxSession>, HostError> {
    let capabilities = host.capabilities().await.map_err(host_error)?;
    let Some(path) = &capabilities.tmux else {
        return Err(HostError::NotInstalled {
            program: "tmux".into(),
        });
    };
    tmux::list_sessions(host, path)
        .await
        .map_err(|error| match error {
            TmuxError::Remote(error) => host_error(error),
            TmuxError::Failed(message) => HostError::CommandFailed { message },
        })
}

/// `HostHandle::focus_herdr_pane`: one `pane.focus` through the probed herdr path.
async fn focus_herdr_pane(
    host: &SshHost,
    session: Option<String>,
    pane_id: String,
) -> Result<(), HostError> {
    let capabilities = host.capabilities().await.map_err(host_error)?;
    let Some(path) = &capabilities.herdr else {
        return Err(HostError::NotInstalled {
            program: "herdr".into(),
        });
    };
    herdr::focus_pane(host, path, session.as_deref(), &pane_id)
        .await
        .map_err(|error| match error {
            herdr::HerdrError::PaneNotFound => HostError::PaneNotFound,
            herdr::HerdrError::Remote(error) => host_error(error),
            error @ herdr::HerdrError::Failed(_) => HostError::CommandFailed {
                message: error.to_string(),
            },
        })
}

/// Runs a watch: herdr's client when the probe found herdr, else `Unavailable { NotInstalled }`
/// until stopped. A probe that fails is retried every [`WATCH_RETRY`], as the client retries
/// its own failures. Stopping interrupts the probe too (the watch closes at once, with no
/// `Unavailable` on the way).
async fn watch_herdr(host: Arc<SshHost>, session: Option<String>, mut driver: HerdrWatchDriver) {
    loop {
        // A stop (or the host closing, which drops this future) must not wait for the probe:
        // it can take as long as the exec timeout plus the herdr listing.
        let probed = tokio::select! {
            biased;
            () = driver.stopped() => {
                driver.close();
                return;
            }
            probed = host.capabilities() => probed,
        };
        match probed {
            Ok(capabilities) => {
                let Some(herdr) = capabilities.herdr.clone() else {
                    let _ = driver.transition(HerdrState::Unavailable {
                        reason: HerdrUnavailable::NotInstalled,
                        message: "herdr is not installed on the host".into(),
                    });
                    driver.stopped().await;
                    driver.close();
                    return;
                };
                return herdr::run(host, herdr, session, driver).await;
            }
            Err(error) => {
                let _ = driver.transition(HerdrState::Unavailable {
                    reason: HerdrUnavailable::Failed,
                    message: format!("capability probe failed: {error}"),
                });
                tokio::select! {
                    () = driver.stopped() => {
                        driver.close();
                        return;
                    }
                    () = tokio::time::sleep(WATCH_RETRY) => {}
                }
            }
        }
    }
}

/// Races the addresses, relays the winner, and holds the SSH connection until it ends.
async fn network<T: Transport>(
    transport: Arc<T>,
    request: HostConnectRequest,
    events: mpsc::Sender<HostEvent>,
    options: HostOptions,
    stop: watch::Receiver<bool>,
) -> CloseReason {
    let raced = match race(&transport, &request.addresses, options.stagger).await {
        Ok(raced) => raced,
        Err(failure) => {
            return CloseReason::Failed(SessionFailure::Unreachable(format!(
                "TCP connection failed: {failure}"
            )));
        }
    };
    // russh awaits check_server_key inside its reader. Relay the transport through a bounded
    // pipe so EOF is still observable while that callback waits for human confirmation.
    let (ssh_stream, relay) = relay(raced.stream, events.clone());
    tokio::pin!(relay);
    let result = tokio::select! {
        biased;
        result = hold(request, raced.index, raced.peer, &events, ssh_stream, options, stop) => {
            if matches!(result, Ok(CloseReason::Disconnected)) {
                // russh flushed into the duplex pipe; also drain that pipe to the transport.
                let _ = timeout(Duration::from_millis(75), &mut relay).await;
            }
            result
        },
        result = &mut relay => Err(match result {
            Ok(_) => lost(),
            Err(error) => connection_error(russh::Error::IO(error)),
        }),
    };
    match result {
        Ok(reason) => reason,
        Err(failure) => CloseReason::Failed(failure),
    }
}

/// Handshake, authentication, then waits for the SSH session to end or for `stop`.
async fn hold(
    request: HostConnectRequest,
    address_index: usize,
    peer: Option<SocketAddr>,
    events: &mpsc::Sender<HostEvent>,
    stream: DuplexStream,
    options: HostOptions,
    mut stop: watch::Receiver<bool>,
) -> Result<CloseReason, SessionFailure> {
    let HostConnectRequest {
        username,
        key,
        trusted_host_keys,
        ..
    } = request;
    let (ended_sender, mut ended) = oneshot::channel();
    let mut client = Client::new(trusted_host_keys, events.clone());
    client.ended = Some(ended_sender);
    let mut handle = russh_client::connect_stream(config(), stream, client)
        .await
        .map_err(handshake_failure)?;
    let _ = events.send(HostEvent::Authenticating).await;
    authenticate(&mut handle, username, &key).await?;
    // The key is needed for authentication only; do not keep it for the connection's lifetime.
    drop(key);
    let host = Arc::new(SshHost {
        handle,
        exec_timeout: options.exec_timeout,
        capabilities: OnceCell::new(),
        sessions: probe::SessionsCache::new(),
        servers: mosh_session::ServerDebt::default(),
    });
    register(&host);
    if events
        .send(HostEvent::Connected {
            address_index,
            peer,
            host: Arc::clone(&host),
        })
        .await
        .is_err()
    {
        return Ok(CloseReason::Disconnected);
    }
    tokio::select! {
        failure = &mut ended => Err(failure.unwrap_or_else(|_| lost())),
        () = async { let _ = stop.wait_for(|stopping| *stopping).await; } => {
            let _ = host
                .handle
                .disconnect(russh::Disconnect::ByApplication, "host closed", "")
                .await;
            let _ = timeout(Duration::from_millis(150), &mut ended).await;
            Ok(CloseReason::Disconnected)
        }
    }
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
