//! One SSH connection per host, shared by terminals, exec queries and herdr watches.
//!
//! [`channel`] splits a host connection into a [`HostHandle`], which the FFI layer wraps for
//! Kotlin, and a [`HostDriver`], which the task that owns the connection uses. Exactly like
//! `session`: the handle never blocks, it validates against the current state and enqueues a
//! [`HostCommand`]; the driver owns every state change and calls the [`HostObserver`] from its
//! own thread, in order, without holding any lock.
//!
//! ```text
//! Connecting ──▶ AwaitingHostKey ──▶ Authenticating ──▶ Connected { address_index }
//!     │  └──────────────────────────────────▲                      │
//!     └─────────────┴───────────────────────┴──────────────────────┴──▶ Closed (terminal, once)
//! ```
//!
//! Operations that return a handle at once ([`HostHandle::open_terminal`],
//! [`HostHandle::watch_herdr`]) create the handle/driver pair themselves and send the *driver*
//! to the host driver in the command; a host that cannot honour the request closes that driver
//! with a failure ([`session::SessionFailure::NotInstalled`] for a missing program,
//! [`session::SessionFailure::CommandFailed`] for a failed helper command). Queries
//! ([`HostHandle::capabilities`], [`HostHandle::list_tmux_sessions`]) carry a oneshot reply and
//! are bounded by [`QUERY_TIMEOUT`].
//!
//! **What closing the host does to its terminals depends on the transport.** SSH terminals are
//! channels of the connection and close with it: `Disconnected` for a user disconnect, else the
//! host's failure. A mosh terminal ([`TerminalTransport::Mosh`]) needs the connection only to
//! bootstrap `mosh-server`, so **losing the connection leaves it running**; a user disconnect
//! still closes it (`Disconnected`, with mosh's shutdown handshake so the server exits).

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::herdr::{self, HerdrObserver, HerdrWatchDriver, HerdrWatchHandle};
use crate::keys::{ClientKey, KeyError};
use crate::session::{
    self, CloseReason, HostKeyPrompt, SessionDriver, SessionHandle, SessionObserver,
};
use crate::term::TerminalSize;
use crate::transport::{Endpoint, EndpointError};
use crate::trust::HostKey;

/// At most this many addresses per host.
pub const MAX_ADDRESSES: usize = 8;

/// A query that the host driver neither answers nor drops fails with
/// [`HostError::CommandFailed`] after this long. Longer than the 10 s exec timeout a driver
/// normally answers within.
pub const QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Everything Rust needs to connect to one host. Kotlin assembles it per connection from Room
/// and the Keystore; Rust keeps nothing after the connection ends. Trust belongs to the host,
/// not to an address.
#[derive(Debug)]
pub struct HostConnectRequest {
    /// In preference order, 1 to [`MAX_ADDRESSES`].
    pub addresses: Vec<Endpoint>,
    pub username: String,
    pub key: ClientKey,
    pub trusted_host_keys: Vec<HostKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostConnectError {
    #[error("a host needs at least one address")]
    NoAddresses,
    #[error("a host has at most 8 addresses")]
    TooManyAddresses,
    #[error("address {index} is invalid: {error}")]
    InvalidAddress { index: usize, error: EndpointError },
    #[error("username must be nonempty without control characters")]
    InvalidUsername,
    #[error("stored private key is unusable: {0}")]
    InvalidPrivateKey(KeyError),
    #[error("trusted host key {index} is not an OpenSSH public key")]
    InvalidTrustedHostKey { index: usize },
}

impl HostConnectRequest {
    /// Validates in order: address count, each address, username, trusted keys, private key.
    pub fn new(
        addresses: &[(&str, u16)],
        username: &str,
        private_key: &[u8],
        trusted_host_keys: &[String],
    ) -> Result<Self, HostConnectError> {
        if addresses.is_empty() {
            return Err(HostConnectError::NoAddresses);
        }
        if addresses.len() > MAX_ADDRESSES {
            return Err(HostConnectError::TooManyAddresses);
        }
        let addresses = addresses
            .iter()
            .enumerate()
            .map(|(index, (host, port))| {
                Endpoint::new(host, *port)
                    .map_err(|error| HostConnectError::InvalidAddress { index, error })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if username.is_empty() || username.chars().any(char::is_control) {
            return Err(HostConnectError::InvalidUsername);
        }
        let trusted_host_keys = trusted_host_keys
            .iter()
            .enumerate()
            .map(|(index, line)| {
                HostKey::from_openssh(line)
                    .map_err(|_| HostConnectError::InvalidTrustedHostKey { index })
            })
            .collect::<Result<_, _>>()?;
        let key =
            ClientKey::from_stored(private_key).map_err(HostConnectError::InvalidPrivateKey)?;
        Ok(Self {
            addresses,
            username: username.to_owned(),
            key,
            trusted_host_keys,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostState {
    /// Racing addresses and handshaking. The initial state; not delivered as a change.
    Connecting,
    /// Waiting for the user to approve or reject an untrusted host key.
    AwaitingHostKey(HostKeyPrompt),
    Authenticating,
    /// Established: terminals, queries and watches are accepted. `address_index` is the
    /// request address that won the race.
    Connected {
        address_index: usize,
    },
    Closed(CloseReason),
}

impl HostState {
    fn name(&self) -> &'static str {
        match self {
            Self::Connecting => "Connecting",
            Self::AwaitingHostKey(_) => "AwaitingHostKey",
            Self::Authenticating => "Authenticating",
            Self::Connected { .. } => "Connected",
            Self::Closed(_) => "Closed",
        }
    }

    fn can_become(&self, next: &HostState) -> bool {
        use HostState::*;
        matches!(
            (self, next),
            (Connecting, AwaitingHostKey(_) | Authenticating)
                | (AwaitingHostKey(_), Authenticating)
                | (Authenticating, Connected { .. })
                | (
                    Connecting | AwaitingHostKey(_) | Authenticating | Connected { .. },
                    Closed(_)
                )
        )
    }
}

/// Receives host changes on the driver's thread. Implementations must return quickly and may
/// call back into the [`HostHandle`].
pub trait HostObserver: Send + Sync {
    fn state_changed(&self, state: &HostState);
}

/// How a terminal reaches the host: a PTY channel on the host's SSH connection (it ends with
/// the connection), or a mosh session that the SSH connection only bootstraps (it survives the
/// connection's loss, not the user's disconnect).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalTransport {
    Ssh,
    Mosh,
}

/// What the terminal opens. Names are validated by [`TerminalTarget::validate`] before
/// anything runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalTarget {
    /// The login shell.
    Shell,
    /// Attach to, or create, a tmux session.
    Tmux { session_name: String },
    /// herdr in `session` (`None` is the default session), after focusing `pane_id` if given.
    Herdr {
        session: Option<String>,
        pane_id: Option<String>,
    },
}

/// Nonempty, at most 128 bytes, no control characters, `\`, `:` or `.`.
pub fn is_valid_tmux_session_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name
            .chars()
            .any(|c| c.is_control() || matches!(c, '\\' | ':' | '.'))
}

/// `[A-Za-z0-9_-]{1,64}`.
pub fn is_valid_herdr_session_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

/// `[A-Za-z0-9:_-]{1,128}`.
pub fn is_valid_herdr_pane_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-'))
}

impl TerminalTarget {
    pub fn validate(&self) -> Result<(), HostError> {
        let valid = match self {
            Self::Shell => true,
            Self::Tmux { session_name } => is_valid_tmux_session_name(session_name),
            Self::Herdr { session, pane_id } => {
                session.as_deref().is_none_or(is_valid_herdr_session_name)
                    && pane_id.as_deref().is_none_or(is_valid_herdr_pane_id)
            }
        };
        valid.then_some(()).ok_or(HostError::InvalidName)
    }
}

/// What the host offers, found by a probe per connection (`herdr_sessions` is re-read on
/// every query). Programs are absolute paths; pass
/// them to `herdr::run`/`watch`/`focus_pane` and the tmux and mosh commands. A missing program
/// is `None`, and whoever would use it reports `NotInstalled`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCapabilities {
    pub tmux: Option<String>,
    pub herdr: Option<String>,
    pub mosh_server: Option<String>,
    pub utf8_locale: String,
    /// Empty when herdr is missing, has no sessions or could not list them. Read afresh on
    /// every `capabilities()` query (a failed read reports the last list that succeeded); live
    /// state comes from `watch_herdr`.
    pub herdr_sessions: Vec<HerdrSessionInfo>,
}

/// A herdr session on the host. The default session is listed under its own name with
/// `is_default`; open or watch it with `session: None`, never `Some(name)`, which runs
/// `herdr --session <name>` and need not be the same session. The socket path stays inside the
/// herdr client, which rediscovers it with `session list --json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrSessionInfo {
    pub name: String,
    pub running: bool,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxSession {
    pub name: String,
    pub windows: u32,
    pub attached_clients: u32,
    pub created_unix: i64,
    pub activity_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    #[error("the host is not connected yet")]
    NotConnected,
    #[error("the host connection is closed")]
    Closed,
    #[error("no host key is awaiting a decision")]
    NoHostKeyPrompt,
    #[error("the fingerprint does not match the presented host key")]
    HostKeyMismatch,
    #[error("session, pane or tmux name is not allowed")]
    InvalidName,
    #[error("{program} is not installed on the host")]
    NotInstalled { program: String },
    /// `focus_herdr_pane`: herdr no longer has that pane (the agent's pane was closed since
    /// the caller last saw it). Refresh the inbox; do not open or reuse a terminal for it.
    #[error("the herdr pane no longer exists")]
    PaneNotFound,
    /// A command ran but failed; the message is a diagnostic without secrets.
    #[error("command failed: {message}")]
    CommandFailed { message: String },
}

/// What the host driver is asked to do.
pub enum HostCommand {
    ApproveHostKey {
        fingerprint: String,
    },
    RejectHostKey,
    /// Explicit disconnect, or every handle was dropped.
    Disconnect,
    /// Open a PTY channel for `target` and run `driver` (a session in `Connecting`) on it.
    /// On failure close `driver` with a [`CloseReason::Failed`]: `NotInstalled { program }` when
    /// the probe found no `tmux`/`herdr`, `CommandFailed` when a helper command (herdr pane
    /// focus) failed, else the matching failure.
    OpenTerminal {
        target: TerminalTarget,
        transport: TerminalTransport,
        size: TerminalSize,
        /// Mosh only: the moment by which the session must be `Connected` (bootstrap, socket
        /// and first authenticated datagram together), else it closes `Failed { TimedOut }`
        /// with its server terminated. `None`: the host's `mosh_connect_timeout`, counted from
        /// the end of the bootstrap.
        deadline: Option<Instant>,
        driver: SessionDriver,
    },
    Capabilities {
        reply: oneshot::Sender<Result<HostCapabilities, HostError>>,
    },
    ListTmux {
        reply: oneshot::Sender<Result<Vec<TmuxSession>, HostError>>,
    },
    /// Focus `pane_id` in herdr `session` with the herdr path from the probe
    /// ([`herdr::focus_pane`]). Reply `NotInstalled { program: "herdr" }` with no herdr found,
    /// `PaneNotFound` when herdr says the pane is gone, `CommandFailed` for other failures.
    FocusHerdrPane {
        session: Option<String>,
        pane_id: String,
        reply: oneshot::Sender<Result<(), HostError>>,
    },
    /// Run [`herdr::run`] (or an equivalent) on `driver`, with the herdr path from the probe.
    /// The session name is validated. With no herdr found, move `driver` to
    /// `Unavailable { NotInstalled }` and wait for its stop.
    WatchHerdr {
        session: Option<String>,
        driver: HerdrWatchDriver,
    },
}

impl HostCommand {
    /// Releases a command that was never delivered: its drivers close silently, because no
    /// handle for them was ever returned and Kotlin must not hear about them.
    fn discard(self) {
        match self {
            Self::OpenTerminal { driver, .. } => driver.discard(),
            Self::WatchHerdr { driver, .. } => driver.discard(),
            _ => {}
        }
    }
}

struct Shared {
    state: Mutex<HostState>,
    /// The remote address the winning TCP connection reached; set before `Connected`.
    peer: Mutex<Option<SocketAddr>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn channel(observer: Arc<dyn HostObserver>) -> (HostHandle, HostDriver) {
    let shared = Arc::new(Shared {
        state: Mutex::new(HostState::Connecting),
        peer: Mutex::new(None),
    });
    let (sender, receiver) = mpsc::unbounded_channel();
    (
        HostHandle {
            shared: Arc::clone(&shared),
            commands: sender,
        },
        HostDriver {
            shared,
            commands: receiver,
            observer: Some(observer),
        },
    )
}

/// Kotlin's side of a host connection. All methods are non-blocking (the queries only wait for
/// their reply) and callable from any thread. Dropping the last handle disconnects.
pub struct HostHandle {
    shared: Arc<Shared>,
    commands: mpsc::UnboundedSender<HostCommand>,
}

impl HostHandle {
    pub fn state(&self) -> HostState {
        lock(&self.shared.state).clone()
    }

    /// The remote address the host's TCP connection actually reached (the winner of address
    /// racing, after name resolution), once it is `Connected`; `None` before that, and for a
    /// transport that cannot say. This is the address mosh must send its UDP datagrams to:
    /// the host name may resolve to other addresses, now or later, which are not this
    /// session's `mosh-server`. Only the IP is meaningful for UDP (the port is the SSH one).
    pub fn peer_addr(&self) -> Option<SocketAddr> {
        *lock(&self.shared.peer)
    }

    pub fn approve_host_key(&self, fingerprint: &str) -> Result<(), HostError> {
        match &*lock(&self.shared.state) {
            HostState::AwaitingHostKey(prompt) if prompt.presented.fingerprint() == fingerprint => {
            }
            HostState::AwaitingHostKey(_) => return Err(HostError::HostKeyMismatch),
            HostState::Closed(_) => return Err(HostError::Closed),
            _ => return Err(HostError::NoHostKeyPrompt),
        }
        self.send(HostCommand::ApproveHostKey {
            fingerprint: fingerprint.to_owned(),
        })
    }

    pub fn reject_host_key(&self) -> Result<(), HostError> {
        match &*lock(&self.shared.state) {
            HostState::AwaitingHostKey(_) => {}
            HostState::Closed(_) => return Err(HostError::Closed),
            _ => return Err(HostError::NoHostKeyPrompt),
        }
        self.send(HostCommand::RejectHostKey)
    }

    /// Idempotent. Closes every terminal and watch on the host (mosh terminals too: this is the
    /// user's disconnect; a *lost* connection leaves mosh terminals running); `Closed` arrives
    /// through the observer after theirs.
    pub fn disconnect(&self) {
        let _ = self.commands.send(HostCommand::Disconnect);
    }

    /// Opens a terminal session on the host. The returned session starts in `Connecting` and
    /// reaches `Connected` once its channel is open; failures close it.
    pub fn open_terminal(
        &self,
        target: TerminalTarget,
        size: TerminalSize,
        observer: Arc<dyn SessionObserver>,
    ) -> Result<SessionHandle, HostError> {
        self.open_terminal_with(target, TerminalTransport::Ssh, size, observer)
    }

    /// [`HostHandle::open_terminal`] with the choice of how the terminal reaches the host.
    pub fn open_terminal_with(
        &self,
        target: TerminalTarget,
        transport: TerminalTransport,
        size: TerminalSize,
        observer: Arc<dyn SessionObserver>,
    ) -> Result<SessionHandle, HostError> {
        self.open_terminal_within(target, transport, size, None, observer)
    }

    /// [`HostHandle::open_terminal_with`] with a budget for a mosh terminal: it must be
    /// `Connected` within `budget` **of this call** (the deadline is absolute, so the probe,
    /// the pane focus, the bootstrap, the socket and the first authenticated datagram all
    /// spend from the same allowance), else it closes `Failed { TimedOut }` after stopping
    /// the server it may have started. `None` keeps the host's own `mosh_connect_timeout`,
    /// counted from the end of the bootstrap. Ignored for SSH terminals.
    pub fn open_terminal_within(
        &self,
        target: TerminalTarget,
        transport: TerminalTransport,
        size: TerminalSize,
        budget: Option<Duration>,
        observer: Arc<dyn SessionObserver>,
    ) -> Result<SessionHandle, HostError> {
        self.require_connected()?;
        target.validate()?;
        let deadline = budget.map(|budget| Instant::now() + budget);
        let (handle, driver) = session::channel(observer);
        self.send(HostCommand::OpenTerminal {
            target,
            transport,
            size,
            deadline,
            driver,
        })?;
        Ok(handle)
    }

    /// The host's programs and locale, probed once per connection (the driver caches them),
    /// with herdr's session list read afresh on every call.
    pub async fn capabilities(&self) -> Result<HostCapabilities, HostError> {
        let (reply, response) = oneshot::channel();
        self.require_connected()?;
        self.send(HostCommand::Capabilities { reply })?;
        await_reply(response, QUERY_TIMEOUT).await
    }

    /// tmux sessions, most recently active first; empty when no tmux server runs.
    pub async fn list_tmux_sessions(&self) -> Result<Vec<TmuxSession>, HostError> {
        let (reply, response) = oneshot::channel();
        self.require_connected()?;
        self.send(HostCommand::ListTmux { reply })?;
        await_reply(response, QUERY_TIMEOUT).await
    }

    /// Focuses `pane_id` in herdr `session` (`None` is the default session): herdr's focus is
    /// shared state of the session, so a terminal that shows the herdr client for an agent
    /// pane must have that pane focused whenever it is opened OR reused. Resolves once herdr
    /// has acknowledged the focus. Names are validated like [`TerminalTarget`]'s
    /// (`InvalidName`); a vanished pane is [`HostError::PaneNotFound`], a host without herdr
    /// `NotInstalled`.
    pub async fn focus_herdr_pane(
        &self,
        session: Option<String>,
        pane_id: String,
    ) -> Result<(), HostError> {
        let (reply, response) = oneshot::channel();
        self.require_connected()?;
        if !session.as_deref().is_none_or(is_valid_herdr_session_name)
            || !is_valid_herdr_pane_id(&pane_id)
        {
            return Err(HostError::InvalidName);
        }
        self.send(HostCommand::FocusHerdrPane {
            session,
            pane_id,
            reply,
        })?;
        await_reply(response, QUERY_TIMEOUT).await
    }

    /// Watches a herdr session (`None` is the default session). The watch ends with the
    /// connection.
    pub fn watch_herdr(
        &self,
        session: Option<String>,
        observer: Arc<dyn HerdrObserver>,
    ) -> Result<HerdrWatchHandle, HostError> {
        self.require_connected()?;
        if !session.as_deref().is_none_or(is_valid_herdr_session_name) {
            return Err(HostError::InvalidName);
        }
        let (handle, driver) = herdr::channel(observer);
        self.send(HostCommand::WatchHerdr { session, driver })?;
        Ok(handle)
    }

    fn require_connected(&self) -> Result<(), HostError> {
        match *lock(&self.shared.state) {
            HostState::Connected { .. } => Ok(()),
            HostState::Closed(_) => Err(HostError::Closed),
            _ => Err(HostError::NotConnected),
        }
    }

    fn send(&self, command: HostCommand) -> Result<(), HostError> {
        self.commands.send(command).map_err(|rejected| {
            // The host closed since the state check. Dropping the command would close its
            // driver loudly, on this thread, for a handle nobody receives.
            rejected.0.discard();
            HostError::Closed
        })
    }
}

/// A dropped reply means the connection ended before the answer; no answer within `timeout`
/// is a failed command. Dropping this future (a cancelled query) only drops the receiver: the
/// host driver's `reply.send` must tolerate `Err` and its exec runs to its own end.
async fn await_reply<T>(
    response: oneshot::Receiver<Result<T, HostError>>,
    timeout: std::time::Duration,
) -> Result<T, HostError> {
    match tokio::time::timeout(timeout, response).await {
        Ok(reply) => reply.unwrap_or(Err(HostError::Closed)),
        Err(_) => Err(HostError::CommandFailed {
            message: "the host did not answer in time".into(),
        }),
    }
}

/// The connection task's side. Dropping it without closing reports an internal failure, so
/// Kotlin always receives exactly one `Closed`.
pub struct HostDriver {
    shared: Arc<Shared>,
    commands: mpsc::UnboundedReceiver<HostCommand>,
    observer: Option<Arc<dyn HostObserver>>,
}

impl HostDriver {
    pub async fn next_command(&mut self) -> HostCommand {
        self.commands
            .recv()
            .await
            .unwrap_or(HostCommand::Disconnect)
    }

    /// For drivers running on a plain thread. Must not be called inside an async runtime.
    pub fn blocking_next_command(&mut self) -> HostCommand {
        self.commands
            .blocking_recv()
            .unwrap_or(HostCommand::Disconnect)
    }

    pub fn state(&self) -> HostState {
        lock(&self.shared.state).clone()
    }

    /// Records the address the winning connection reached, to be set before the move to
    /// `Connected` so a handle that sees `Connected` can read it.
    pub fn set_peer_addr(&self, peer: Option<SocketAddr>) {
        *lock(&self.shared.peer) = peer;
    }

    /// Moves to `next` and notifies the observer. On `Closed` further commands are refused,
    /// those already queued are failed (terminals close with the host's reason, watches close,
    /// queries see `Closed`), and the observer is released afterwards.
    pub fn transition(&mut self, next: HostState) -> Result<(), TransitionError> {
        {
            let mut state = lock(&self.shared.state);
            if !state.can_become(&next) {
                return Err(TransitionError {
                    from: state.name(),
                    to: next.name(),
                });
            }
            *state = next.clone();
        }
        let observer = if let HostState::Closed(reason) = &next {
            self.commands.close();
            self.fail_queued(reason);
            self.observer.take()
        } else {
            self.observer.clone()
        };
        if let Some(observer) = observer {
            observer.state_changed(&next);
        }
        Ok(())
    }

    /// Closes unless already closed.
    pub fn close(&mut self, reason: CloseReason) {
        let _ = self.transition(HostState::Closed(reason));
    }

    fn fail_queued(&mut self, reason: &CloseReason) {
        while let Ok(command) = self.commands.try_recv() {
            match command {
                HostCommand::OpenTerminal { mut driver, .. } => driver.close(reason.clone()),
                HostCommand::WatchHerdr { mut driver, .. } => driver.close(),
                // Dropping a reply sender makes the query fail with `Closed`.
                _ => {}
            }
        }
    }
}

impl Drop for HostDriver {
    fn drop(&mut self) {
        self.close(CloseReason::Failed(session::SessionFailure::Internal(
            "host task ended without closing".into(),
        )));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid host transition {from} -> {to}")]
pub struct TransitionError {
    pub from: &'static str,
    pub to: &'static str,
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use super::*;
    use crate::herdr::{HerdrState, HerdrUnavailable};
    use crate::session::{SessionFailure, SessionState};
    use crate::trust::HostKeyVerdict;

    #[derive(Default)]
    struct Recorder {
        events: Mutex<Vec<String>>,
        handle: Mutex<Option<Arc<HostHandle>>>,
    }

    impl HostObserver for Recorder {
        fn state_changed(&self, state: &HostState) {
            // Reentrancy: reading state from inside the callback must not deadlock.
            if let Some(handle) = &*lock(&self.handle) {
                assert_eq!(&handle.state(), state);
            }
            lock(&self.events).push(state.name().into());
        }
    }

    #[derive(Default)]
    struct SessionRecorder(Mutex<Vec<SessionState>>);

    impl SessionObserver for SessionRecorder {
        fn state_changed(&self, state: &SessionState) {
            lock(&self.0).push(state.clone());
        }

        fn frame_ready(&self) {}
    }

    #[derive(Default)]
    struct WatchRecorder(Mutex<Vec<HerdrState>>);

    impl HerdrObserver for WatchRecorder {
        fn state_changed(&self, state: &HerdrState) {
            lock(&self.0).push(state.clone());
        }
    }

    fn setup(reentrant: bool) -> (Arc<Recorder>, Arc<HostHandle>, HostDriver) {
        let recorder = Arc::new(Recorder::default());
        let (handle, driver) = channel(recorder.clone());
        let handle = Arc::new(handle);
        if reentrant {
            *lock(&recorder.handle) = Some(handle.clone());
        }
        (recorder, handle, driver)
    }

    fn events(recorder: &Recorder) -> Vec<String> {
        lock(&recorder.events).clone()
    }

    fn host_key() -> HostKey {
        HostKey::from_openssh(&ClientKey::generate_ed25519("h").public_key().openssh).unwrap()
    }

    fn connect(driver: &mut HostDriver) {
        driver.transition(HostState::Authenticating).unwrap();
        driver
            .transition(HostState::Connected { address_index: 1 })
            .unwrap();
    }

    fn size() -> TerminalSize {
        TerminalSize::new(80, 24).unwrap()
    }

    #[test]
    fn host_connect_request_validates_every_field_with_indices() {
        let key = ClientKey::generate_ed25519("k").to_stored();
        let trusted = vec![host_key().info().openssh];
        let addresses = [("a.example", 22), ("198.51.100.2", 2222)];
        let request = HostConnectRequest::new(&addresses, "dev", &key, &trusted).unwrap();
        assert_eq!(request.addresses.len(), 2);
        assert_eq!(request.addresses[1].host(), "198.51.100.2");
        assert_eq!(request.addresses[1].port(), 2222);
        assert_eq!(request.trusted_host_keys.len(), 1);
        assert!(!format!("{request:?}").contains("OPENSSH"));

        let err = |addresses: &[(&str, u16)], user, key: &[u8], trusted: &[String]| {
            HostConnectRequest::new(addresses, user, key, trusted).unwrap_err()
        };
        assert_eq!(err(&[], "a", &key, &[]), HostConnectError::NoAddresses);
        let nine = vec![("h", 22); 9];
        assert_eq!(
            err(&nine, "a", &key, &[]),
            HostConnectError::TooManyAddresses
        );
        assert!(HostConnectRequest::new(&nine[..8], "a", &key, &[]).is_ok());
        assert_eq!(
            err(&[("h", 22), ("a b", 22)], "a", &key, &[]),
            HostConnectError::InvalidAddress {
                index: 1,
                error: EndpointError::InvalidHost
            }
        );
        assert_eq!(
            err(&[("h", 0)], "a", &key, &[]),
            HostConnectError::InvalidAddress {
                index: 0,
                error: EndpointError::InvalidPort
            }
        );
        assert_eq!(
            err(&[("h", 22)], "", &key, &[]),
            HostConnectError::InvalidUsername
        );
        assert_eq!(
            err(&[("h", 22)], "a\n", &key, &[]),
            HostConnectError::InvalidUsername
        );
        let bad_trust = vec![trusted[0].clone(), "nope".into()];
        assert_eq!(
            err(&[("h", 22)], "a", &key, &bad_trust),
            HostConnectError::InvalidTrustedHostKey { index: 1 }
        );
        assert_eq!(
            err(&[("h", 22)], "a", b"junk", &[]),
            HostConnectError::InvalidPrivateKey(KeyError::Malformed)
        );
    }

    #[test]
    fn terminal_targets_validate_names_exactly() {
        let tmux = |name: &str| TerminalTarget::Tmux {
            session_name: name.into(),
        };
        let herdr = |session: Option<&str>, pane: Option<&str>| TerminalTarget::Herdr {
            session: session.map(Into::into),
            pane_id: pane.map(Into::into),
        };
        let long = "x".repeat(129);
        let max = "x".repeat(128);
        for ok in [
            TerminalTarget::Shell,
            tmux("main"),
            tmux("my work 2"),
            tmux("é界😀"),
            tmux("-dash"),
            tmux(&"é".repeat(64)),
            tmux(&max),
            herdr(None, None),
            herdr(Some("or2-test_1"), Some("w1:p_2-3")),
            herdr(Some(&"a".repeat(64)), Some(&"a".repeat(128))),
        ] {
            assert_eq!(ok.validate(), Ok(()), "{ok:?}");
        }
        for bad in [
            tmux(""),
            tmux(&long),
            tmux(&"é".repeat(65)),
            tmux("a:b"),
            tmux("a.b"),
            tmux("a\\b"),
            tmux("a\nb"),
            tmux("a\u{7f}"),
            herdr(Some(""), None),
            herdr(Some("a b"), None),
            herdr(Some("a:b"), None),
            herdr(Some("é"), None),
            herdr(Some(&"a".repeat(65)), None),
            herdr(None, Some("")),
            herdr(None, Some("a b")),
            herdr(None, Some("a.b")),
            herdr(None, Some(&"a".repeat(129))),
            herdr(Some("ok"), Some("bad/id")),
        ] {
            assert_eq!(bad.validate(), Err(HostError::InvalidName), "{bad:?}");
        }
    }

    #[test]
    fn host_key_decision_is_bound_to_the_presented_key() {
        let (recorder, handle, mut driver) = setup(false);
        assert_eq!(handle.state(), HostState::Connecting);
        assert!(
            events(&recorder).is_empty(),
            "the initial state is not a change"
        );
        assert_eq!(
            handle.approve_host_key("SHA256:x"),
            Err(HostError::NoHostKeyPrompt)
        );

        let presented = host_key();
        let prompt = HostKeyPrompt {
            presented: presented.clone(),
            previously_trusted: vec![host_key()],
        };
        assert_eq!(prompt.verdict(), HostKeyVerdict::Changed);
        driver
            .transition(HostState::AwaitingHostKey(prompt))
            .unwrap();
        assert_eq!(
            handle.approve_host_key(&host_key().fingerprint()),
            Err(HostError::HostKeyMismatch)
        );
        handle.approve_host_key(&presented.fingerprint()).unwrap();
        assert!(matches!(
            driver.blocking_next_command(),
            HostCommand::ApproveHostKey { fingerprint } if fingerprint == presented.fingerprint()
        ));
        handle.reject_host_key().unwrap();
        assert!(matches!(
            driver.blocking_next_command(),
            HostCommand::RejectHostKey
        ));
        driver.transition(HostState::Authenticating).unwrap();
        assert_eq!(handle.reject_host_key(), Err(HostError::NoHostKeyPrompt));
    }

    #[test]
    fn transitions_are_ordered_and_invalid_ones_are_rejected_without_callbacks() {
        let (recorder, _handle, mut driver) = setup(true);
        assert_eq!(
            driver.transition(HostState::Connected { address_index: 0 }),
            Err(TransitionError {
                from: "Connecting",
                to: "Connected"
            })
        );
        connect(&mut driver);
        assert_eq!(
            driver.transition(HostState::Authenticating),
            Err(TransitionError {
                from: "Connected",
                to: "Authenticating"
            })
        );
        assert_eq!(driver.state(), HostState::Connected { address_index: 1 });
        driver.close(CloseReason::Disconnected);
        assert!(driver.transition(HostState::Authenticating).is_err());
        driver.close(CloseReason::Failed(SessionFailure::TimedOut));
        assert_eq!(events(&recorder), ["Authenticating", "Connected", "Closed"]);
    }

    #[test]
    fn closing_releases_the_observer_and_rejects_everything() {
        let (recorder, handle, mut driver) = setup(true);
        let weak: Weak<Recorder> = Arc::downgrade(&recorder);
        *lock(&recorder.handle) = None;
        drop(recorder);
        connect(&mut driver);
        handle.disconnect();
        assert!(matches!(
            driver.blocking_next_command(),
            HostCommand::Disconnect
        ));
        driver.close(CloseReason::Disconnected);
        assert!(weak.upgrade().is_none(), "observer released after Closed");
        assert_eq!(handle.state(), HostState::Closed(CloseReason::Disconnected));
        assert_eq!(handle.reject_host_key(), Err(HostError::Closed));
        assert_eq!(handle.approve_host_key("x"), Err(HostError::Closed));
        assert_eq!(
            handle
                .open_terminal(
                    TerminalTarget::Shell,
                    size(),
                    Arc::new(SessionRecorder::default())
                )
                .err(),
            Some(HostError::Closed)
        );
        assert_eq!(
            handle
                .watch_herdr(None, Arc::new(WatchRecorder::default()))
                .err(),
            Some(HostError::Closed)
        );
        handle.disconnect();
    }

    #[test]
    fn dropping_all_handles_disconnects_and_dropping_the_driver_closes() {
        let (recorder, handle, mut driver) = setup(false);
        drop(handle);
        assert!(matches!(
            driver.blocking_next_command(),
            HostCommand::Disconnect
        ));
        drop(driver);
        assert_eq!(events(&recorder), ["Closed"]);

        let (_recorder, handle, driver) = setup(false);
        drop(driver);
        assert_eq!(
            handle.state(),
            HostState::Closed(CloseReason::Failed(SessionFailure::Internal(
                "host task ended without closing".into()
            )))
        );
    }

    #[tokio::test]
    async fn queries_and_openings_require_a_connected_host() {
        let (_recorder, handle, mut driver) = setup(false);
        let sessions = Arc::new(SessionRecorder::default());
        let watches = Arc::new(WatchRecorder::default());
        assert_eq!(handle.capabilities().await, Err(HostError::NotConnected));
        assert_eq!(
            handle.list_tmux_sessions().await,
            Err(HostError::NotConnected)
        );
        assert_eq!(
            handle
                .open_terminal(TerminalTarget::Shell, size(), sessions.clone())
                .err(),
            Some(HostError::NotConnected)
        );
        assert_eq!(
            handle.watch_herdr(None, watches.clone()).err(),
            Some(HostError::NotConnected)
        );
        assert_eq!(
            handle.focus_herdr_pane(None, "w1:p1".into()).await,
            Err(HostError::NotConnected)
        );
        driver.transition(HostState::Authenticating).unwrap();
        assert_eq!(handle.capabilities().await, Err(HostError::NotConnected));
        connect_from_authenticating(&mut driver);

        // Invalid names are rejected before anything is enqueued.
        let bad = TerminalTarget::Tmux {
            session_name: "a:b".into(),
        };
        assert_eq!(
            handle.open_terminal(bad, size(), sessions.clone()).err(),
            Some(HostError::InvalidName)
        );
        assert_eq!(
            handle.watch_herdr(Some("a b".into()), watches).err(),
            Some(HostError::InvalidName)
        );
        assert!(lock(&sessions.0).is_empty(), "no session was created");
        // The focus validates like terminal targets, before anything is enqueued.
        for (session, pane) in [
            (Some("a b"), "w1:p1"),
            (Some(""), "w1:p1"),
            (None, ""),
            (None, "w1 p1"),
            (None, "w1.p1"),
        ] {
            assert_eq!(
                handle
                    .focus_herdr_pane(session.map(str::to_owned), pane.into())
                    .await,
                Err(HostError::InvalidName),
                "{session:?} {pane:?}"
            );
        }
        assert!(driver.commands.try_recv().is_err(), "nothing was enqueued");
        driver.close(CloseReason::Disconnected);
        assert_eq!(handle.capabilities().await, Err(HostError::Closed));
        assert_eq!(
            handle.focus_herdr_pane(None, "w1:p1".into()).await,
            Err(HostError::Closed)
        );
        assert_eq!(handle.list_tmux_sessions().await, Err(HostError::Closed));
    }

    fn connect_from_authenticating(driver: &mut HostDriver) {
        driver
            .transition(HostState::Connected { address_index: 0 })
            .unwrap();
    }

    fn capabilities() -> HostCapabilities {
        HostCapabilities {
            tmux: Some("/usr/bin/tmux".into()),
            herdr: None,
            mosh_server: None,
            utf8_locale: "C.UTF-8".into(),
            herdr_sessions: vec![HerdrSessionInfo {
                name: "default".into(),
                running: true,
                is_default: true,
            }],
        }
    }

    #[tokio::test]
    async fn queries_are_answered_through_their_reply() {
        let (_recorder, handle, mut driver) = setup(false);
        connect(&mut driver);
        let answers = std::thread::spawn(move || {
            for _ in 0..2 {
                match driver.blocking_next_command() {
                    HostCommand::Capabilities { reply } => {
                        reply.send(Ok(capabilities())).unwrap();
                    }
                    HostCommand::ListTmux { reply } => {
                        reply
                            .send(Err(HostError::NotInstalled {
                                program: "tmux".into(),
                            }))
                            .unwrap();
                    }
                    _ => panic!("unexpected command"),
                }
            }
            driver
        });
        assert_eq!(handle.capabilities().await, Ok(capabilities()));
        assert_eq!(
            handle.list_tmux_sessions().await,
            Err(HostError::NotInstalled {
                program: "tmux".into()
            })
        );
        drop(answers.join().unwrap());
    }

    #[tokio::test]
    async fn a_herdr_focus_carries_its_names_and_is_answered_through_its_reply() {
        let (_recorder, handle, mut driver) = setup(false);
        connect(&mut driver);
        let answers = std::thread::spawn(move || {
            for answer in [Ok(()), Err(HostError::PaneNotFound)] {
                let HostCommand::FocusHerdrPane {
                    session,
                    pane_id,
                    reply,
                } = driver.blocking_next_command()
                else {
                    panic!("unexpected command")
                };
                assert_eq!(session.as_deref(), Some("work"));
                assert_eq!(pane_id, "w1:p2");
                reply.send(answer).unwrap();
            }
            driver
        });
        let focus = || handle.focus_herdr_pane(Some("work".into()), "w1:p2".into());
        assert_eq!(focus().await, Ok(()));
        assert_eq!(focus().await, Err(HostError::PaneNotFound));
        drop(answers.join().unwrap());
    }

    #[tokio::test]
    async fn an_unanswered_query_times_out_but_a_dropped_one_is_closed() {
        let timeout = std::time::Duration::from_millis(50);
        let (reply, response) = oneshot::channel::<Result<u8, HostError>>();
        assert!(matches!(
            await_reply(response, timeout).await,
            Err(HostError::CommandFailed { .. })
        ));
        // The sender outlived the wait; answering a cancelled query is an error to tolerate.
        assert!(reply.send(Ok(1)).is_err());

        let (reply, response) = oneshot::channel::<Result<u8, HostError>>();
        drop(reply);
        assert_eq!(await_reply(response, timeout).await, Err(HostError::Closed));
        let (reply, response) = oneshot::channel::<Result<u8, HostError>>();
        reply.send(Ok(7)).unwrap();
        assert_eq!(await_reply(response, timeout).await, Ok(7));
    }

    #[test]
    fn a_request_rejected_after_the_host_closed_fires_no_callback() {
        let (_recorder, handle, mut driver) = setup(false);
        connect(&mut driver);
        // The window: the state still reads Connected, but the command queue is closed.
        driver.commands.close();
        let sessions = Arc::new(SessionRecorder::default());
        let watches = Arc::new(WatchRecorder::default());
        assert_eq!(
            handle
                .open_terminal(TerminalTarget::Shell, size(), sessions.clone())
                .err(),
            Some(HostError::Closed)
        );
        assert_eq!(
            handle.watch_herdr(None, watches.clone()).err(),
            Some(HostError::Closed)
        );
        assert!(lock(&sessions.0).is_empty(), "no session callback");
        assert!(lock(&watches.0).is_empty(), "no watch callback");
        // The discarded drivers released their observers.
        assert_eq!(Arc::strong_count(&sessions), 1);
        assert_eq!(Arc::strong_count(&watches), 1);
    }

    #[tokio::test]
    async fn closing_fails_pending_queries_with_closed() {
        let (_recorder, handle, mut driver) = setup(false);
        connect(&mut driver);
        let pending = tokio::spawn({
            let handle = handle.clone();
            async move { handle.list_tmux_sessions().await }
        });
        // The driver took the query and then the connection ended: its reply is dropped.
        let taken = std::thread::spawn(move || {
            let command = driver.blocking_next_command();
            driver.close(CloseReason::Failed(SessionFailure::ConnectionLost(
                "gone".into(),
            )));
            drop(command);
        });
        assert_eq!(pending.await.unwrap(), Err(HostError::Closed));
        taken.join().unwrap();

        // A query still queued when the host closes fails the same way.
        let (_recorder, handle, mut driver) = setup(false);
        connect(&mut driver);
        let queued = tokio::spawn({
            let handle = handle.clone();
            async move { handle.capabilities().await }
        });
        while driver.commands.is_empty() {
            tokio::task::yield_now().await;
        }
        driver.close(CloseReason::Disconnected);
        assert_eq!(queued.await.unwrap(), Err(HostError::Closed));
    }

    #[test]
    fn a_terminal_budget_becomes_an_absolute_deadline_at_the_call() {
        let (_recorder, handle, mut driver) = setup(false);
        connect(&mut driver);
        let sessions = Arc::new(SessionRecorder::default());
        let before = Instant::now();
        let _plain = handle
            .open_terminal_with(
                TerminalTarget::Shell,
                TerminalTransport::Mosh,
                size(),
                sessions.clone(),
            )
            .unwrap();
        let _budgeted = handle
            .open_terminal_within(
                TerminalTarget::Shell,
                TerminalTransport::Mosh,
                size(),
                Some(Duration::from_secs(5)),
                sessions,
            )
            .unwrap();
        let after = Instant::now();
        let HostCommand::OpenTerminal { deadline, .. } = driver.blocking_next_command() else {
            panic!("expected OpenTerminal");
        };
        assert_eq!(deadline, None, "no budget keeps the host's own timeout");
        let HostCommand::OpenTerminal { deadline, .. } = driver.blocking_next_command() else {
            panic!("expected OpenTerminal");
        };
        let deadline = deadline.expect("a budget gives a deadline");
        assert!(deadline >= before + Duration::from_secs(5));
        assert!(deadline <= after + Duration::from_secs(5));
    }

    #[test]
    fn terminals_and_watches_are_created_by_the_handle_and_failed_when_the_host_closes() {
        let (_recorder, handle, mut driver) = setup(false);
        connect(&mut driver);
        let sessions = Arc::new(SessionRecorder::default());
        let watches = Arc::new(WatchRecorder::default());
        let target = TerminalTarget::Herdr {
            session: Some("work".into()),
            pane_id: Some("w1:p2".into()),
        };
        let terminal = handle
            .open_terminal(target.clone(), size(), sessions.clone())
            .unwrap();
        let watch = handle
            .watch_herdr(Some("work".into()), watches.clone())
            .unwrap();
        assert_eq!(terminal.state(), SessionState::Connecting);
        assert_eq!(watch.state(), HerdrState::Starting);

        // The driver receives each pair's driver half and drives it.
        let HostCommand::OpenTerminal {
            target: got,
            transport,
            size: got_size,
            driver: mut session_driver,
            ..
        } = driver.blocking_next_command()
        else {
            panic!("expected OpenTerminal");
        };
        assert_eq!(
            (got, transport, got_size),
            (target, TerminalTransport::Ssh, size())
        );
        session_driver.transition(SessionState::Connected).unwrap();
        assert_eq!(terminal.state(), SessionState::Connected);
        let HostCommand::WatchHerdr {
            session,
            driver: mut watch_driver,
        } = driver.blocking_next_command()
        else {
            panic!("expected WatchHerdr");
        };
        assert_eq!(session.as_deref(), Some("work"));
        watch_driver
            .transition(HerdrState::Unavailable {
                reason: HerdrUnavailable::NotRunning,
                message: "m".into(),
            })
            .unwrap();
        watch.stop();
        watch_driver.close();
        assert_eq!(watch.state(), HerdrState::Closed);
        drop(session_driver);

        // Opened but not yet picked up when the host closes: closed with the host's reason.
        let user_closed = Arc::new(SessionRecorder::default());
        let queued = handle
            .open_terminal(TerminalTarget::Shell, size(), user_closed.clone())
            .unwrap();
        let queued_watch = handle
            .watch_herdr(None, Arc::new(WatchRecorder::default()))
            .unwrap();
        driver.close(CloseReason::Disconnected);
        assert_eq!(
            queued.state(),
            SessionState::Closed(CloseReason::Disconnected)
        );
        assert_eq!(
            *lock(&user_closed.0),
            [SessionState::Closed(CloseReason::Disconnected)]
        );
        assert_eq!(queued_watch.state(), HerdrState::Closed);

        let (_recorder, handle, mut driver) = setup(false);
        connect(&mut driver);
        let lost = Arc::new(SessionRecorder::default());
        let queued = handle
            .open_terminal(TerminalTarget::Shell, size(), lost.clone())
            .unwrap();
        let reason = CloseReason::Failed(SessionFailure::ConnectionLost("gone".into()));
        driver.close(reason.clone());
        assert_eq!(queued.state(), SessionState::Closed(reason));
    }
}
