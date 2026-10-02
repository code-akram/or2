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
use tokio::task::JoinSet;
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
    HostObserver, HostState, TargetScroll, TerminalTarget, TerminalTransport, TmuxSession,
    UserCancel,
};
use crate::mosh;
use crate::probe;
use crate::remote::{ExecOutput, OUTPUT_CAP, RemoteError, RemoteHost, SecretBytes};
use crate::session::{CloseReason, HostKeyPrompt, SessionDriver, SessionFailure};
use crate::term::TerminalSize;
use crate::tmux::{self, TmuxError};
use crate::transport::{
    ADDRESS_TIMEOUT, DatagramTransport, RACE_STAGGER, RaceReport, RaceTiming, Transport, race_with,
    seconds,
};

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
    /// How long one address may take (name resolution and TCP connect) before the race counts
    /// it as unanswered. Production: `transport::ADDRESS_TIMEOUT`.
    pub address_timeout: Duration,
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
            address_timeout: ADDRESS_TIMEOUT,
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

/// From the network task to the host thread. Pairing ([`super::pair_client`]) runs on the same
/// connection machinery and answers only the host-key and transport-end events.
pub(super) enum HostEvent {
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
    /// The last herdr session list read successfully (the probe's own list until then), which
    /// the herdr watches and pane focuses find their sockets in.
    sessions: probe::SessionsCache,
    /// Keeps the app's pane focus and the terminal's own from both reaching herdr.
    pub(super) focus: herdr::FocusGate,
    /// Where each herdr pane was scrolled to through this connection (`scroll_target`).
    scroll_offsets: herdr::ScrollOffsets,
    /// The mosh servers this connection still has to stop (see [`mosh_session::ServerDebt`]).
    pub(super) servers: mosh_session::ServerDebt,
    /// The session-channel opens still waiting for the server's answer (see
    /// [`SshHost::start_open`]). `None` once the connection is over.
    opens: Mutex<Option<JoinSet<()>>>,
    /// This connection's own `Arc`, for the `&self` callers (the [`RemoteHost`] methods) that
    /// start an open.
    me: Weak<SshHost>,
    /// Test only: see `Client::reader_gate`.
    #[cfg(test)]
    reader_gate: Arc<Mutex<Option<super::client::TestReaderGate>>>,
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
    /// Wraps an authenticated connection. The caller ends the connection's opens
    /// ([`SshHost::end_opens`]) when it is done with it.
    pub(super) fn new(
        handle: Handle<Client<HostEvent>>,
        exec_timeout: Duration,
        #[cfg(test)] reader_gate: Arc<Mutex<Option<super::client::TestReaderGate>>>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| SshHost {
            handle,
            exec_timeout,
            capabilities: OnceCell::new(),
            sessions: probe::SessionsCache::new(),
            focus: herdr::FocusGate::new(),
            scroll_offsets: herdr::ScrollOffsets::new(),
            servers: mosh_session::ServerDebt::default(),
            opens: Mutex::new(Some(JoinSet::new())),
            me: me.clone(),
            #[cfg(test)]
            reader_gate,
        })
    }

    /// Tells the server this side is done. The caller bounds the wait.
    pub(super) async fn disconnect(&self, reason: &str) {
        let _ = self
            .handle
            .disconnect(russh::Disconnect::ByApplication, reason, "")
            .await;
    }

    /// The probe's answer, run once per connection. A failed probe is not cached.
    pub(super) async fn capabilities(&self) -> Result<&HostCapabilities, RemoteError> {
        self.capabilities
            .get_or_try_init(|| async {
                let (capabilities, entries) = probe::probe_entries(self).await?;
                // The listing the probe read is what the watches and focuses discover from.
                if let Some(entries) = entries {
                    self.sessions.directory().seed(entries);
                }
                Ok(capabilities)
            })
            .await
    }

    /// `herdr` focus of `pane_id`, the socket from this connection's directory. `from_terminal`
    /// is a terminal's own focus before it starts, which a focus the app acknowledged a moment
    /// ago satisfies; the app's request is never answered from memory (see [`herdr::FocusGate`]).
    pub(super) async fn focus_pane(
        self: &Arc<Self>,
        herdr: &str,
        session: Option<&str>,
        pane_id: &str,
        from_terminal: bool,
    ) -> Result<(), herdr::HerdrError> {
        self.focus
            .focus(
                self,
                herdr,
                self.sessions.directory(),
                session,
                pane_id,
                from_terminal,
            )
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

    /// Starts opening a session channel. The open belongs to the connection, not to the caller:
    /// it runs as a task of [`SshHost::opens`] until the server answers, however long that takes
    /// and whatever the caller does meanwhile. A caller that gave up (its timeout, a stop, a
    /// cancelled future) and dropped the [`PendingOpen`] leaves the task to close the channel the
    /// server confirms later; a raw russh channel does not close itself when dropped, and a
    /// leaked one counts against the server's `MaxSessions` for the connection's lifetime. The
    /// task ends with its answer, or with the connection ([`SshHost::end_opens`]).
    pub(super) fn start_open(self: &Arc<Self>) -> PendingOpen {
        self.start_open_of(OpenKind::Session)
    }

    /// [`SshHost::start_open`] for any kind of channel open.
    fn start_open_of(self: &Arc<Self>, kind: OpenKind) -> PendingOpen {
        let (answer, reply) = oneshot::channel();
        let host = Arc::clone(self);
        let mut opens = self.opens.lock().unwrap_or_else(PoisonError::into_inner);
        // After the connection ended nothing is spawned: the dropped sender reads as `Disconnect`.
        if let Some(set) = opens.as_mut() {
            // Finished tasks are reaped as new ones come.
            while set.try_join_next().is_some() {}
            set.spawn_on(
                async move {
                    let opened = match kind {
                        OpenKind::Session => host.handle.channel_open_session().await,
                        OpenKind::Streamlocal(path) => {
                            host.handle.channel_open_direct_streamlocal(path).await
                        }
                    };
                    // The channel travels in a guard that closes it unless the caller takes it
                    // out: a failed send (nobody waits any more) drops the guard here, and one
                    // queued but never received is dropped with the receiver.
                    let weak = Arc::downgrade(&host);
                    drop(host);
                    let _ = answer.send(opened.map(|channel| OpenedChannel::new(channel, weak)));
                },
                runtime().handle(),
            );
        }
        PendingOpen(reply)
    }

    /// The one way a channel is closed. `closing` owns the channel (`async move { channel
    /// .close().await }`) and is moved into a task of the connection, which awaits it with NO
    /// deadline: russh's `close` waits for room in its bounded command queue, and dropping it
    /// (a cancelled caller, a timeout) before the `Close` is queued would leave the channel open
    /// on the server for the connection's lifetime. The task ends when the `Close` is queued, or
    /// with the connection ([`SshHost::end_opens`]), whose channels go with it. The receiver
    /// resolves once the `Close` is queued (an error if it could not be: the connection is over);
    /// a caller that needs the ordering awaits it with a deadline of its own, and the task keeps
    /// the obligation if the caller stops waiting.
    fn close_owned(
        &self,
        closing: impl Future<Output = Result<(), russh::Error>> + Send + 'static,
    ) -> oneshot::Receiver<()> {
        let (queued, receiver) = oneshot::channel();
        let mut opens = self.opens.lock().unwrap_or_else(PoisonError::into_inner);
        // Once the connection is over nothing is spawned: `closing` is dropped with its channel
        // and the receiver reads as an error.
        if let Some(set) = opens.as_mut() {
            while set.try_join_next().is_some() {}
            set.spawn_on(
                async move {
                    if closing.await.is_ok() {
                        let _ = queued.send(());
                    }
                },
                runtime().handle(),
            );
        }
        receiver
    }

    /// The connection is over: stop every open still waiting (their callers see `Disconnect`),
    /// and refuse new ones. Also breaks the reference each task holds to this host.
    pub(super) fn end_opens(&self) {
        let set = self
            .opens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(set); // Dropping a `JoinSet` aborts its tasks.
    }

    /// How many opens are still waiting for the server.
    #[cfg(test)]
    pub(super) fn outstanding_opens(&self) -> usize {
        self.opens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
            .map_or(0, |set| {
                while set.try_join_next().is_some() {}
                set.len()
            })
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

/// Which channel [`SshHost::start_open_of`] opens.
enum OpenKind {
    Session,
    /// `direct-streamlocal@openssh.com` to this socket path.
    Streamlocal(String),
}

/// A confirmed channel that nobody has taken yet. Dropped, it closes the channel through the
/// connection (a raw russh channel does not close itself, and a leaked one holds a `MaxSessions`
/// slot); [`OpenedChannel::into_inner`] hands it over, and with it the duty to close it.
pub(super) struct OpenedChannel {
    channel: Option<russh::Channel<russh_client::Msg>>,
    host: Weak<SshHost>,
}

impl OpenedChannel {
    fn new(channel: russh::Channel<russh_client::Msg>, host: Weak<SshHost>) -> Self {
        Self {
            channel: Some(channel),
            host,
        }
    }

    /// Hands the channel over, and with it the duty to close it (or to know that the server
    /// has). Call it only where the next owner takes over without an `await` in between.
    pub(super) fn into_inner(mut self) -> russh::Channel<russh_client::Msg> {
        self.channel.take().expect("an armed opened channel")
    }

    /// Closes the channel deliberately: hands it to the connection's close task
    /// ([`SshHost::close_owned`]) and waits until the `Close` is queued. The guard is disarmed
    /// at once; the caller may bound or cancel the wait, and the task keeps the obligation.
    pub(super) async fn close(mut self) -> Result<(), russh::Error> {
        let channel = self.channel.take().expect("an armed opened channel");
        close_via(&self.host, async move { channel.close().await }).await
    }

    /// Splits the channel for a pump that reads and writes it concurrently; the write half stays
    /// guarded.
    pub(super) fn split(mut self) -> (russh::ChannelReadHalf, OpenedWriter) {
        let (reader, writer) = self
            .channel
            .take()
            .expect("an armed opened channel")
            .split();
        (
            reader,
            OpenedWriter {
                writer: Some(writer),
                host: self.host.clone(),
            },
        )
    }
}

/// Hands `closing` to the connection (if it is still there) and waits until the `Close` is
/// queued.
async fn close_via(
    host: &Weak<SshHost>,
    closing: impl Future<Output = Result<(), russh::Error>> + Send + 'static,
) -> Result<(), russh::Error> {
    // A host that is gone took its channels with it.
    let host = host.upgrade().ok_or(russh::Error::Disconnect)?;
    host.close_owned(closing)
        .await
        .map_err(|_| russh::Error::Disconnect)
}

/// The write half of an [`OpenedChannel`] a pump is running: the same duty, the same guard.
pub(super) struct OpenedWriter {
    writer: Option<russh::ChannelWriteHalf<russh_client::Msg>>,
    host: Weak<SshHost>,
}

impl OpenedWriter {
    /// The server closed the channel or the connection broke: nothing left to close.
    pub(super) fn disarm(&mut self) {
        self.writer = None;
    }

    /// Like [`OpenedChannel::close`].
    pub(super) async fn close(mut self) -> Result<(), russh::Error> {
        let writer = self.writer.take().expect("an armed channel writer");
        close_via(&self.host, async move { writer.close().await }).await
    }
}

impl std::ops::Deref for OpenedWriter {
    type Target = russh::ChannelWriteHalf<russh_client::Msg>;

    fn deref(&self) -> &Self::Target {
        self.writer.as_ref().expect("an armed channel writer")
    }
}

impl Drop for OpenedWriter {
    fn drop(&mut self) {
        if let (Some(writer), Some(host)) = (self.writer.take(), self.host.upgrade()) {
            drop(host.close_owned(async move { writer.close().await }));
        }
    }
}

impl std::ops::Deref for OpenedChannel {
    type Target = russh::Channel<russh_client::Msg>;

    fn deref(&self) -> &Self::Target {
        self.channel.as_ref().expect("an armed opened channel")
    }
}

impl std::ops::DerefMut for OpenedChannel {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.channel.as_mut().expect("an armed opened channel")
    }
}

impl Drop for OpenedChannel {
    fn drop(&mut self) {
        // A host that is gone took its channels with it.
        if let (Some(channel), Some(host)) = (self.channel.take(), self.host.upgrade()) {
            drop(host.close_owned(async move { channel.close().await }));
        }
    }
}

/// A channel being opened by the connection ([`SshHost::start_open`]). Dropping it is
/// safe at any moment: the connection closes the channel if the server confirms it later, and
/// one that was confirmed but not yet taken is closed here.
pub(super) struct PendingOpen(oneshot::Receiver<Result<OpenedChannel, russh::Error>>);

impl PendingOpen {
    /// Resolves with the server's answer. Cancel-safe: the answer is kept for the next call. The
    /// channel stays in its close-on-drop guard until the caller disarms it, so no path (a
    /// cancelled future, an aborted task) can leave it unclosed.
    pub(super) async fn wait(&mut self) -> Result<OpenedChannel, russh::Error> {
        match (&mut self.0).await {
            Ok(opened) => opened,
            // The connection ended and took the open with it.
            Err(_) => Err(russh::Error::Disconnect),
        }
    }
}

impl Drop for PendingOpen {
    fn drop(&mut self) {
        // Refuse further answers and dispose of one that is already queued: dropping its guard
        // closes the channel.
        self.0.close();
        drop(self.0.try_recv());
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
        channel: OpenedChannel,
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

/// A session channel running an exec. Dropped while armed (the exec future was cancelled) the
/// [`OpenedChannel`] inside closes the channel through the connection; `finished` disarms it
/// once the server has closed the channel itself.
struct ExecChannel(Option<OpenedChannel>);

impl ExecChannel {
    fn channel(&mut self) -> &mut russh::Channel<russh_client::Msg> {
        self.0.as_mut().expect("an armed exec channel")
    }

    fn finished(mut self) {
        drop(self.0.take().map(OpenedChannel::into_inner));
    }

    /// Waits (bounded) until the `Close` is queued; the connection finishes it if that takes
    /// longer, or if this is cancelled.
    async fn close(mut self) {
        if let Some(channel) = self.0.take() {
            let _ = timeout(CHANNEL_CLOSE_GRACE, channel.close()).await;
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
        // The open is the connection's (see `start_open`): past the deadline, a confirmation that
        // arrives late is closed by the connection instead of orphaned on it.
        let mut opening = self.me.upgrade().ok_or(RemoteError::Closed)?.start_open();
        let channel = timeout_at(deadline, opening.wait())
            .await
            .map_err(|_| RemoteError::TimedOut)?
            .map_err(remote_error)?;
        self.run_exec(channel, line, deadline).await
    }

    /// OpenSSH `direct-streamlocal@openssh.com`. A socket that is missing or refuses the
    /// connection fails the open with `CONNECT_FAILED`: `Io`. A server with streamlocal
    /// forwarding disabled (or any other refusal) is `Rejected` ([`streamlocal_error`]).
    async fn open_unix(&self, path: &str) -> Result<Self::Stream, RemoteError> {
        // The open is the connection's (see `start_open`): past the deadline, a confirmation that
        // arrives late is closed by the connection instead of orphaned on it.
        let mut opening = self
            .me
            .upgrade()
            .ok_or(RemoteError::Closed)?
            .start_open_of(OpenKind::Streamlocal(path.to_owned()));
        let channel = timeout(self.exec_timeout, opening.wait())
            .await
            .map_err(|_| RemoteError::TimedOut)?
            .map_err(streamlocal_error)?;
        Ok(channel.into_inner().into_stream())
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
    // What each address has done, for a connect timeout that fires while the race still runs.
    let report = RaceReport::new();
    let race_report = report.clone();
    let mut network = tokio::spawn(async move {
        let reason = network(
            transport,
            request,
            events.clone(),
            options,
            stop,
            &race_report,
        )
        .await;
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
                    // No TCP connection yet: the host is unreachable, and each address says what
                    // it did. Once one connected, the handshake or authentication is what hung.
                    break Ok(CloseReason::Failed(if report.is_pending() {
                        SessionFailure::Unreachable(format!(
                            "no address answered within {}: {}",
                            seconds(options.connect_timeout),
                            report.describe()
                        ))
                    } else {
                        SessionFailure::TimedOut
                    }));
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
            ..
        } => spawn_terminal(host, target, size, driver, closing, tracker),
        HostCommand::OpenTerminal {
            target,
            transport: TerminalTransport::Mosh,
            size,
            deadline,
            driver,
        } => spawn_mosh(host, target, size, deadline, driver, closing, mosh),
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
        HostCommand::StopMoshServer { pid, reply } => {
            let host = Arc::clone(host);
            let (mut closing, tracker) = (closing.clone(), tracker.clone());
            runtime().spawn(async move {
                let _tracker = tracker;
                tokio::select! {
                    result = mosh::terminate(&*host, pid) => {
                        let _ = reply.send(result.map_err(host_error));
                    }
                    _ = closed_reason(&mut closing) => {}
                }
            });
        }
        HostCommand::ScrollTarget {
            target,
            pane_id,
            scroll,
            reply,
        } => {
            let host = Arc::clone(host);
            let (mut closing, tracker) = (closing.clone(), tracker.clone());
            runtime().spawn(async move {
                let _tracker = tracker;
                tokio::select! {
                    result = scroll_target(&host, target, pane_id, scroll) => { let _ = reply.send(result); }
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
    deadline: Option<Instant>,
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
        deadline,
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
    host: &Arc<SshHost>,
    session: Option<String>,
    pane_id: String,
) -> Result<(), HostError> {
    let capabilities = host.capabilities().await.map_err(host_error)?;
    let Some(path) = &capabilities.herdr else {
        return Err(HostError::NotInstalled {
            program: "herdr".into(),
        });
    };
    host.focus_pane(path, session.as_deref(), &pane_id, false)
        .await
        .map_err(|error| match error {
            herdr::HerdrError::PaneNotFound => HostError::PaneNotFound,
            herdr::HerdrError::Remote(error) => host_error(error),
            error @ herdr::HerdrError::Failed(_) => HostError::CommandFailed {
                message: error.to_string(),
            },
        })
}

/// `HostHandle::scroll_target`: tmux through exec, herdr through `pane.scroll`, each with the
/// program path from the probe. A `Shell` target never gets here.
async fn scroll_target(
    host: &Arc<SshHost>,
    target: TerminalTarget,
    pane_id: Option<String>,
    scroll: TargetScroll,
) -> Result<(), HostError> {
    let capabilities = host.capabilities().await.map_err(host_error)?;
    let missing = |program: &str| HostError::NotInstalled {
        program: program.into(),
    };
    match target {
        TerminalTarget::Shell => Ok(()),
        TerminalTarget::Tmux { session_name } => {
            let path = capabilities
                .tmux
                .as_deref()
                .ok_or_else(|| missing("tmux"))?;
            tmux::scroll(&**host, path, &session_name, scroll)
                .await
                .map_err(|error| match error {
                    TmuxError::Remote(error) => host_error(error),
                    TmuxError::Failed(message) => HostError::CommandFailed { message },
                })
        }
        TerminalTarget::Herdr { session, .. } => {
            let path = capabilities
                .herdr
                .as_deref()
                .ok_or_else(|| missing("herdr"))?;
            herdr::scroll_pane_in(
                &**host,
                path,
                host.sessions.directory(),
                &host.scroll_offsets,
                session.as_deref(),
                pane_id.as_deref(),
                scroll,
            )
            .await
            .map_err(|error| match error {
                herdr::HerdrError::PaneNotFound => HostError::PaneNotFound,
                herdr::HerdrError::Remote(error) => host_error(error),
                error @ herdr::HerdrError::Failed(_) => HostError::CommandFailed {
                    message: error.to_string(),
                },
            })
        }
    }
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
                let directory = Arc::clone(host.sessions.directory());
                return herdr::run_in(host, herdr, directory, session, driver).await;
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
    report: &RaceReport,
) -> CloseReason {
    let timing = RaceTiming {
        stagger: options.stagger,
        address_timeout: options.address_timeout,
    };
    let raced = match race_with(&transport, &request.addresses, timing, Some(report)).await {
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

/// Ends the connection's outstanding channel opens when dropped.
struct EndOpens(Arc<SshHost>);

impl Drop for EndOpens {
    fn drop(&mut self) {
        self.0.end_opens();
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
    // The trusted keys' algorithms first, so a host with several keys presents a trusted one.
    let config = config(&trusted_host_keys);
    let mut client = Client::new(trusted_host_keys, events.clone());
    client.ended = Some(ended_sender);
    #[cfg(test)]
    let reader_gate = Arc::clone(&client.reader_gate);
    let mut handle = russh_client::connect_stream(config, stream, client)
        .await
        .map_err(handshake_failure)?;
    let _ = events.send(HostEvent::Authenticating).await;
    authenticate(&mut handle, username, &key).await?;
    // The key is needed for authentication only; do not keep it for the connection's lifetime.
    drop(key);
    let host = SshHost::new(
        handle,
        options.exec_timeout,
        #[cfg(test)]
        reader_gate,
    );
    register(&host);
    // However this function ends, also when the host driver aborts it, the opens that are still
    // waiting end with the connection.
    let _opens = EndOpens(Arc::clone(&host));
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
