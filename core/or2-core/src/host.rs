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

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

use crate::herdr::{self, HerdrObserver, HerdrWatchDriver, HerdrWatchHandle};
use crate::keys::{ClientKey, KeyError};
use crate::remote::RemoteError;
use crate::session::{
    self, CloseReason, HostKeyPrompt, SessionDriver, SessionHandle, SessionObserver,
};
use crate::term::TerminalSize;
use crate::tmux::TmuxError;
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
    /// A login shell in a literal absolute working directory.
    ShellIn { path: String },
    /// Attach to, or create, a tmux session.
    Tmux { session_name: String },
    /// herdr in `session` (`None` is the default session), after focusing `pane_id` if given.
    Herdr {
        session: Option<String>,
        pane_id: Option<String>,
    },
}

/// What [`HostHandle::scroll_target`] does to the history a tmux or herdr target shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetScroll {
    Up {
        lines: u32,
    },
    Down {
        lines: u32,
    },
    /// Back to the live screen: tmux leaves copy mode, herdr's offset returns to 0.
    Bottom,
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

/// `[A-Za-z0-9:_-]{1,128}`; herdr's tab ids (`w1:t1`) follow the same rule.
pub fn is_valid_herdr_pane_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-'))
}

/// An agent's kind as herdr names it (`claude`): 1 to 128 bytes, no control characters. It is
/// only compared with herdr's answer, never sent.
fn is_valid_agent_kind(kind: &str) -> bool {
    (1..=128).contains(&kind.len()) && !kind.chars().any(char::is_control)
}

/// An agent's session value (herdr's `agent_session.value`: an id, or a path): 1 to 4096 bytes
/// with no control characters.
fn is_valid_agent_session(value: &str) -> bool {
    (1..=4096).contains(&value.len()) && !value.chars().any(char::is_control)
}

/// The herdr session, pane and agent instance a permission answer names, validated as a reply's
/// ([`HostHandle::reply_to_pane`]): `InvalidName` for a malformed one, `CommandFailed` with
/// [`herdr::OPEN_THE_PANE`] for an agent that names no instance.
fn validate_agent_target(
    session: Option<&str>,
    pane_id: &str,
    agent: &herdr::AgentIdentity,
) -> Result<(), HostError> {
    if !session.is_none_or(is_valid_herdr_session_name)
        || !is_valid_herdr_pane_id(pane_id)
        || !is_valid_herdr_pane_id(&agent.terminal_id)
        || !agent.agent.as_deref().is_none_or(is_valid_agent_kind)
        || !agent.name.as_deref().is_none_or(is_valid_agent_kind)
        || !agent.session.as_ref().is_none_or(|session| {
            is_valid_agent_kind(&session.kind) && is_valid_agent_session(&session.value)
        })
    {
        return Err(HostError::InvalidName);
    }
    if !agent.is_instance() {
        return Err(HostError::CommandFailed {
            message: herdr::OPEN_THE_PANE.into(),
        });
    }
    Ok(())
}

impl TerminalTarget {
    pub fn validate(&self) -> Result<(), HostError> {
        let valid = match self {
            Self::Shell => true,
            Self::ShellIn { path } => crate::directories::valid_path(path),
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
/// them to the herdr client, the tmux and the mosh commands. A missing program
/// is `None`, and whoever would use it reports `NotInstalled`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCapabilities {
    pub tmux: Option<String>,
    pub herdr: Option<String>,
    pub mosh_server: Option<String>,
    /// The host's tmux can record which client a terminal's attach made (`set-option -F`,
    /// tmux 2.6 and later, from `tmux -V` in the program probe: [`crate::tmux::records_clients`]).
    /// Without it a tmux attach is plain and a terminal's session moves do nothing
    /// ([`HostHandle::navigate`]). Not exported over the FFI.
    pub tmux_records_clients: bool,
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

/// A direction on screen, for moving between panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavDirection {
    Left,
    Right,
    Up,
    Down,
}

/// What a navigation gesture asks of the multiplexer a terminal shows
/// ([`HostHandle::navigate`]): tmux windows, panes and sessions, or herdr tabs, panes and
/// workspaces. The previous and next ones wrap around, as tmux's own keys do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetNav {
    /// The next tmux window, or herdr tab of the focused workspace.
    NextWindow,
    PreviousWindow,
    /// The pane in `direction` from the active (tmux) or focused (herdr) pane.
    Pane {
        direction: NavDirection,
    },
    /// The next tmux session (the terminal's tmux client switches to it), or herdr workspace.
    NextSession,
    PreviousSession,
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
    /// `focus_herdr_pane`, `focus_herdr_tab`: herdr no longer has that pane or tab (closed since
    /// the caller last saw it). Refresh the inbox; do not open or reuse a terminal for it.
    #[error("the herdr pane no longer exists")]
    PaneNotFound,
    /// A command ran but failed; the message is a diagnostic without secrets.
    #[error("command failed: {message}")]
    CommandFailed { message: String },
    /// `upload_image`: the server has no SFTP subsystem (or it would not start).
    #[error("SFTP is not available on this host")]
    SftpUnavailable,
    /// What was to be sent is over its limit (`reply_to_pane`: more than
    /// [`herdr::MAX_REPLY_BYTES`]; `upload_image`: more than [`MAX_IMAGE_BYTES`]); nothing was sent.
    #[error("too large to send")]
    TooLarge,
    /// `answer_permission`: the agent no longer waits at the permission prompt it was notified
    /// of ([`herdr::HerdrError::PromptChanged`]); nothing was sent.
    #[error("the permission prompt changed")]
    PromptChanged,
}

/// A failure to reach the host: `Closed` once the connection is gone, else a failed command.
impl From<RemoteError> for HostError {
    fn from(error: RemoteError) -> Self {
        match error {
            RemoteError::Closed => Self::Closed,
            error => Self::CommandFailed {
                message: error.to_string(),
            },
        }
    }
}

impl From<TmuxError> for HostError {
    fn from(error: TmuxError) -> Self {
        match error {
            TmuxError::Remote(error) => error.into(),
            TmuxError::Failed(message) => Self::CommandFailed { message },
        }
    }
}

impl From<herdr::HerdrError> for HostError {
    fn from(error: herdr::HerdrError) -> Self {
        match error {
            herdr::HerdrError::PaneNotFound => Self::PaneNotFound,
            herdr::HerdrError::PromptChanged => Self::PromptChanged,
            herdr::HerdrError::Remote(error) => error.into(),
            error @ herdr::HerdrError::Failed(_) => Self::CommandFailed {
                message: error.to_string(),
            },
        }
    }
}

/// A program the capability probe looks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program {
    Tmux,
    Herdr,
    MoshServer,
}

impl Program {
    /// Its name, as [`HostError::NotInstalled`] reports it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Tmux => "tmux",
            Self::Herdr => "herdr",
            Self::MoshServer => "mosh-server",
        }
    }
}

impl HostCapabilities {
    /// The absolute path of `program`, or [`HostError::NotInstalled`] when the probe found none.
    pub fn program(&self, program: Program) -> Result<&str, HostError> {
        match program {
            Program::Tmux => &self.tmux,
            Program::Herdr => &self.herdr,
            Program::MoshServer => &self.mosh_server,
        }
        .as_deref()
        .ok_or_else(|| HostError::NotInstalled {
            program: program.name().into(),
        })
    }
}

/// The largest image [`HostHandle::upload_image`] sends (20 MiB).
pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

/// The image types [`HostHandle::upload_image`] takes, by file extension (lower case).
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

/// The longest [`upload_timeout`].
pub const MAX_UPLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(240);

/// How long [`HostHandle::upload_image`] of `bytes` may take: [`QUERY_TIMEOUT`] and one more
/// second for each 100 KiB begun (a link of 100 KiB/s, about 0.8 Mbit/s, still gets there), at
/// most [`MAX_UPLOAD_TIMEOUT`] (a 20 MiB image gets 235 s).
pub fn upload_timeout(bytes: usize) -> std::time::Duration {
    let seconds = bytes.div_ceil(100 * 1024) as u64;
    (QUERY_TIMEOUT + std::time::Duration::from_secs(seconds)).min(MAX_UPLOAD_TIMEOUT)
}

/// An uploaded image's path, as [`HostCommand::UploadImage`] answers it. A path sent is not yet
/// a path taken: [`HostHandle::upload_image`] acknowledges it on `taken` in the same step that
/// receives it, before returning it. A caller that stopped waiting just as it was sent drops it
/// unacknowledged, and the host removes the image (nothing would ever name it).
#[derive(Debug)]
pub struct UploadedImage {
    pub path: String,
    pub taken: oneshot::Sender<()>,
}

impl UploadedImage {
    /// `path`, and where its acknowledgement arrives: `Ok` once taken, an error when dropped.
    pub fn new(path: String) -> (Self, oneshot::Receiver<()>) {
        let (taken, acknowledged) = oneshot::channel();
        (Self { path, taken }, acknowledged)
    }
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
    /// The path of `mosh-server` (`None`: not installed), from the program probe alone: the
    /// reply never waits for herdr's session listing.
    MoshServer {
        reply: oneshot::Sender<Result<Option<String>, HostError>>,
    },
    RecentDirectories {
        reply: oneshot::Sender<Result<Vec<String>, HostError>>,
    },
    ListTmux {
        reply: oneshot::Sender<Result<Vec<TmuxSession>, HostError>>,
    },
    /// Focus `pane_id` in herdr `session` with the herdr path from the probe
    /// ([`herdr::focus_pane_in`]). Reply `NotInstalled { program: "herdr" }` with no herdr found,
    /// `PaneNotFound` when herdr says the pane is gone, `CommandFailed` for other failures.
    FocusHerdrPane {
        session: Option<String>,
        pane_id: String,
        reply: oneshot::Sender<Result<(), HostError>>,
    },
    /// Focus tab `tab_id` in herdr `session` (`tab.focus`), in order with the session's pane
    /// focuses. Replies as [`HostCommand::FocusHerdrPane`]; a tab that has gone is
    /// `PaneNotFound`.
    FocusHerdrTab {
        session: Option<String>,
        tab_id: String,
        reply: oneshot::Sender<Result<(), HostError>>,
    },
    /// Stop the `mosh-server` with process id `pid` on the host ([`crate::mosh::terminate`]):
    /// only a process `ps` names `mosh-server` is signalled, and one that is already gone is
    /// success. Reply `CommandFailed` when the stop could not run (no free channel, a slow
    /// host), `Closed` when the connection ended.
    StopMoshServer {
        pid: u32,
        reply: oneshot::Sender<Result<(), HostError>>,
    },
    /// Scroll the history `target` shows ([`HostHandle::scroll_target`]): tmux through exec
    /// ([`crate::tmux::scroll`]), herdr through `pane.scroll` with the offset this connection
    /// keeps per pane ([`herdr::ScrollOffsets`]). `target` is a validated tmux or herdr target;
    /// `pane_id` (validated) is the herdr pane, `None` for the focused one. Reply
    /// `NotInstalled` without the program, `PaneNotFound` for a vanished herdr pane,
    /// `CommandFailed` for other failures.
    ScrollTarget {
        target: TerminalTarget,
        pane_id: Option<String>,
        /// The scrolling terminal's tmux client id (validated; [`SessionHandle::client_id`]).
        client_id: Option<String>,
        scroll: TargetScroll,
        reply: oneshot::Sender<Result<(), HostError>>,
    },
    /// Move the multiplexer `target` shows ([`HostHandle::navigate`]): tmux through exec with the
    /// probed tmux path, herdr through its API with the probed herdr path. `target` is never
    /// `Shell` (the handle answers that itself) and its names are validated. Reply
    /// `NotInstalled` without the program, `PaneNotFound` when herdr says `pane_id` is gone,
    /// `CommandFailed` for other failures.
    Navigate {
        target: TerminalTarget,
        pane_id: Option<String>,
        /// The moving terminal's tmux client id (validated; [`SessionHandle::client_id`]).
        client_id: Option<String>,
        nav: TargetNav,
        reply: oneshot::Sender<Result<(), HostError>>,
    },
    /// Send `text` to `agent` in herdr pane `pane_id` of `session` and submit it
    /// ([`HostHandle::reply_to_pane`], [`herdr::reply_in`]) with the probed herdr path. Names and
    /// text are validated. Reply `NotInstalled` without herdr, `PaneNotFound` when the pane or that
    /// agent is gone, `CommandFailed` for other failures. `text` is never logged. The caller stops
    /// waiting at `deadline`, or earlier by dropping `reply`: the worker then sends nothing more
    /// (it never abandons a request that sends, and starts one only while it can end before
    /// `deadline`).
    ReplyToPane {
        session: Option<String>,
        pane_id: String,
        agent: herdr::AgentIdentity,
        text: String,
        deadline: tokio::time::Instant,
        reply: oneshot::Sender<Result<herdr::ReplyRoute, HostError>>,
    },
    /// The permission prompt `agent` waits at in herdr pane `pane_id` of `session`
    /// ([`HostHandle::permission_prompt`], [`herdr::permission_prompt_in`]) with the probed herdr
    /// path; `None` when it waits at none that can be answered. Names are validated. Reply
    /// `NotInstalled` without herdr, `PaneNotFound` when the pane or that agent is gone,
    /// `CommandFailed` for other failures. Sends nothing to the pane.
    PermissionPrompt {
        session: Option<String>,
        pane_id: String,
        agent: herdr::AgentIdentity,
        reply: oneshot::Sender<Result<Option<herdr::PermissionPrompt>, HostError>>,
    },
    /// Approve or deny the permission prompt `agent` waits at in herdr pane `pane_id` of
    /// `session`, the one notified at `seq` ([`HostHandle::answer_permission`],
    /// [`herdr::answer_permission_in`]) with the probed herdr path. Names are validated. Reply
    /// `NotInstalled` without herdr, `PaneNotFound` when the pane or that agent is gone or its
    /// shell has the foreground, `PromptChanged` when it is not at that prompt any more,
    /// `CommandFailed` for other failures. The caller stops waiting at `deadline`, or earlier by
    /// dropping `reply`, as for [`HostCommand::ReplyToPane`].
    AnswerPermission {
        session: Option<String>,
        pane_id: String,
        agent: herdr::AgentIdentity,
        seq: u64,
        answer: herdr::PermissionAnswer,
        deadline: tokio::time::Instant,
        reply: oneshot::Sender<Result<(), HostError>>,
    },
    /// Write `bytes` over SFTP to the host's image directory (contracts.md, "Image paste"):
    /// `~/.cache/or2/images` (created `0700`), a temporary name renamed to
    /// `or2-<UTC yyyyMMdd-HHmmss>-<6 hex>.<extension>` (`0600`), on the connection's one SFTP
    /// session (contracts.md, "Upload speed"); once the path is delivered, that directory's
    /// `or2-*` files older than seven days are swept. Reply the absolute path (one safe to
    /// type into a terminal), `SftpUnavailable` without an SFTP subsystem, `CommandFailed` for
    /// other failures. `bytes` and `extension` are validated (size, lower-case known
    /// extension). A dropped `reply` (the caller cancelled or timed out) stops the upload and
    /// removes what it made (the temporary file, or the image once renamed), best effort, as
    /// does a path the caller never took ([`UploadedImage::taken`] not acknowledged).
    UploadImage {
        bytes: Vec<u8>,
        extension: String,
        reply: oneshot::Sender<Result<UploadedImage, HostError>>,
    },
    /// Install herdr's integration `id` (one of [`herdr::INTEGRATIONS`], validated) with the
    /// probed herdr path ([`herdr::install_integration`]): one exec. Reply `NotInstalled`
    /// without herdr, `CommandFailed` with the first line of its stderr when it fails.
    InstallHerdrIntegration {
        id: String,
        reply: oneshot::Sender<Result<(), HostError>>,
    },
    /// Read `herdr integration status` with the probed herdr path
    /// ([`herdr::integration_states`]). Reply `NotInstalled` without herdr.
    HerdrIntegrations {
        reply: oneshot::Sender<Result<Vec<herdr::Integration>, HostError>>,
    },
    /// Run [`herdr::run_in`] (or an equivalent) on `driver`, with the herdr path from the probe.
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

/// Becomes `true` once the user has ended the host: an explicit `disconnect()` or the release
/// of the last handle. Unlike the command queue it belongs to the shared state, not to the
/// host driver, so it still works after the driver has exited (a lost connection): the mosh
/// sessions that survive the loss listen to it. It never goes back to `false`.
pub(crate) type UserCancel = watch::Receiver<bool>;

struct Shared {
    state: Mutex<HostState>,
    /// See [`UserCancel`].
    user_cancel: watch::Sender<bool>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn channel(observer: Arc<dyn HostObserver>) -> (HostHandle, HostDriver) {
    let shared = Arc::new(Shared {
        state: Mutex::new(HostState::Connecting),
        user_cancel: watch::channel(false).0,
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

impl Drop for HostHandle {
    /// Releasing the last handle is the user's disconnect, for the sessions that outlive a
    /// lost connection too (the command queue closing only tells a live driver).
    fn drop(&mut self) {
        self.shared.user_cancel.send_replace(true);
    }
}

impl HostHandle {
    pub fn state(&self) -> HostState {
        lock(&self.shared.state).clone()
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
    ///
    /// Also works after the connection was lost: the host driver is gone by then, so the
    /// request goes through the shared [`UserCancel`] signal, which the mosh sessions that
    /// survived the loss still listen to.
    pub fn disconnect(&self) {
        self.shared.user_cancel.send_replace(true);
        let _ = self.commands.send(HostCommand::Disconnect);
    }

    /// Opens a terminal session on the host over `transport`. The returned session starts in
    /// `Connecting` and reaches `Connected` once its channel is open (SSH) or the server's first
    /// datagram authenticates (mosh); failures close it.
    ///
    /// `budget` is for a mosh terminal: it must be `Connected` within `budget` **of this call**
    /// (the deadline is absolute, so the probe, the pane focus, the bootstrap, the socket and
    /// the first authenticated datagram all spend from the same allowance), else it closes
    /// `Failed { TimedOut }` after stopping the server it may have started. `None` keeps the
    /// host's own `mosh_connect_timeout`, counted from the end of the bootstrap. Ignored for SSH
    /// terminals.
    pub fn open_terminal(
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
        // Every tmux terminal gets its own client id, which its attach records its tmux client
        // under (`tmux::attach_command`): what `navigate` moves is then exactly its client.
        let client_id =
            matches!(target, TerminalTarget::Tmux { .. }).then(crate::tmux::new_client_id);
        let (handle, driver) = session::channel_with_client(observer, client_id);
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
        self.query(QUERY_TIMEOUT, |reply| HostCommand::Capabilities { reply })
            .await
    }

    /// The path of `mosh-server` on the host, `None` when it is not installed. Resolved by the
    /// program probe alone (one exec round trip, cached per connection), never by herdr's
    /// session listing, so a transport choice that awaits it is not held up by a slow herdr.
    pub async fn mosh_server(&self) -> Result<Option<String>, HostError> {
        self.query(QUERY_TIMEOUT, |reply| HostCommand::MoshServer { reply })
            .await
    }

    /// Bounded Claude Code/Codex history read, newest first; missing histories are empty.
    pub async fn recent_directories(&self) -> Result<Vec<String>, HostError> {
        self.query(QUERY_TIMEOUT, |reply| HostCommand::RecentDirectories {
            reply,
        })
        .await
    }

    /// tmux sessions, most recently active first; empty when no tmux server runs.
    pub async fn list_tmux_sessions(&self) -> Result<Vec<TmuxSession>, HostError> {
        self.query(QUERY_TIMEOUT, |reply| HostCommand::ListTmux { reply })
            .await
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
        if !session.as_deref().is_none_or(is_valid_herdr_session_name)
            || !is_valid_herdr_pane_id(&pane_id)
        {
            return Err(HostError::InvalidName);
        }
        self.query(QUERY_TIMEOUT, |reply| HostCommand::FocusHerdrPane {
            session,
            pane_id,
            reply,
        })
        .await
    }

    /// Focuses tab `tab_id` in herdr `session` (`None` is the default session): herdr shows the
    /// pane that tab last had focused, and a terminal running the herdr client follows. Names
    /// are validated like [`HostHandle::focus_herdr_pane`]'s (`InvalidName`); a tab that has
    /// gone is [`HostError::PaneNotFound`], a host without herdr `NotInstalled`.
    pub async fn focus_herdr_tab(
        &self,
        session: Option<String>,
        tab_id: String,
    ) -> Result<(), HostError> {
        if !session.as_deref().is_none_or(is_valid_herdr_session_name)
            || !is_valid_herdr_pane_id(&tab_id)
        {
            return Err(HostError::InvalidName);
        }
        self.query(QUERY_TIMEOUT, |reply| HostCommand::FocusHerdrTab {
            session,
            tab_id,
            reply,
        })
        .await
    }

    /// Stops the `mosh-server` with process id `pid` on the host, which an earlier client left
    /// running (the app's process died with its session open: the key died with it, and
    /// `mosh-server` has no idle timeout). Resolves `Ok` when the server is gone: stopped, or
    /// not running (a recorded pid is old; the process may have ended, or the id been reused).
    /// Only a process that `ps` names `mosh-server` is signalled, so a reused pid is never
    /// touched. `Err` when the stop could not run (the host had no free channel, answered too
    /// slowly or closed): the caller keeps the pid and tries again on the next connection.
    pub async fn stop_mosh_server(&self, pid: u32) -> Result<(), HostError> {
        if pid == 0 {
            return Err(HostError::InvalidName);
        }
        self.query(QUERY_TIMEOUT, |reply| HostCommand::StopMoshServer {
            pid,
            reply,
        })
        .await
    }

    /// Scrolls the history of what `target` shows, without the mouse (contracts.md,
    /// "Wheel-aware scrolling"): a tmux target enters copy mode and scrolls by lines (`Bottom`
    /// leaves copy mode); a herdr target sets `pane_id`'s `offset_from_bottom` (`None`: the
    /// session's focused pane), this connection keeping each pane's offset. A `Shell` target, or
    /// zero lines, does nothing and is `Ok`. Names are validated like [`TerminalTarget`]'s
    /// (`InvalidName`); a vanished herdr pane is [`HostError::PaneNotFound`].
    ///
    /// `client_id` is the scrolling terminal's [`SessionHandle::client_id`]: after a session
    /// move ([`HostHandle::navigate`]) a tmux scroll acts on the session that terminal's client
    /// shows, as window and pane moves do, not on the one it was opened on. herdr ignores it; a
    /// malformed id is `InvalidName`.
    pub async fn scroll_target(
        &self,
        target: TerminalTarget,
        pane_id: Option<String>,
        scroll: TargetScroll,
        client_id: Option<String>,
    ) -> Result<(), HostError> {
        target.validate()?;
        if !pane_id.as_deref().is_none_or(is_valid_herdr_pane_id)
            || !client_id
                .as_deref()
                .is_none_or(crate::tmux::is_valid_client_id)
        {
            return Err(HostError::InvalidName);
        }
        if matches!(
            target,
            TerminalTarget::Shell | TerminalTarget::ShellIn { .. }
        ) || matches!(
            scroll,
            TargetScroll::Up { lines: 0 } | TargetScroll::Down { lines: 0 }
        ) {
            return Ok(());
        }
        self.query(QUERY_TIMEOUT, |reply| HostCommand::ScrollTarget {
            target,
            pane_id,
            client_id,
            scroll,
            reply,
        })
        .await
    }

    /// Moves what a terminal on `target` shows (a gesture or a shortcut): for tmux the window,
    /// the pane or (switching the terminal's tmux client) the session; for herdr the tab of the
    /// focused workspace, the pane, or the workspace. `pane_id` is herdr's pane to move from
    /// (`None`: the focused one, which is what a herdr client shows); tmux ignores it. A
    /// `Shell` target has nothing to move: `Ok(())` at once, nothing runs. Names are validated
    /// like [`TerminalTarget`]'s (`InvalidName`); a host without the program is
    /// `NotInstalled`, a vanished herdr pane `PaneNotFound`. Moving past the last window, tab or
    /// workspace wraps around; with only one there is nothing to do, which is `Ok(())`.
    ///
    /// `client_id` is the moving terminal's [`SessionHandle::client_id`] (the session it shows
    /// now): a tmux session move then acts on exactly that terminal's tmux client, whatever
    /// other terminals show the same tmux session. When that client cannot be identified
    /// (`None`, a tmux too old to record it, nothing recorded) a session move is nothing to do,
    /// `Ok(())` with nothing switched: it never guesses. Window and pane moves act on the
    /// session, not a client, and run either way. herdr ignores the id. A malformed id is
    /// `InvalidName`.
    pub async fn navigate(
        &self,
        target: TerminalTarget,
        pane_id: Option<String>,
        nav: TargetNav,
        client_id: Option<String>,
    ) -> Result<(), HostError> {
        target.validate()?;
        if !pane_id.as_deref().is_none_or(is_valid_herdr_pane_id)
            || !client_id
                .as_deref()
                .is_none_or(crate::tmux::is_valid_client_id)
        {
            return Err(HostError::InvalidName);
        }
        if matches!(
            target,
            TerminalTarget::Shell | TerminalTarget::ShellIn { .. }
        ) {
            return Ok(());
        }
        self.query(QUERY_TIMEOUT, |reply| HostCommand::Navigate {
            target,
            pane_id,
            client_id,
            nav,
            reply,
        })
        .await
    }

    /// Sends `text` to `agent` in herdr pane `pane_id` of `session` (`None` is the default
    /// session) and submits it, with no terminal open (contracts.md, "Reply from a
    /// notification"): through herdr's `agent.prompt` ([`herdr::ReplyRoute::Prompted`]), or, when
    /// herdr refuses that because the agent is blocked or not driven by herdr, typed into the pane
    /// with its Enter in one request, once the pane was checked to hold that agent in its
    /// foreground ([`herdr::ReplyRoute::Typed`]). Several lines are sent as they are. Names are
    /// validated like [`TerminalTarget`]'s (the terminal id like a pane id, the agent's kind and
    /// name, and the session's kind, at most 128 bytes without control characters, the session's
    /// value at most 4096), and an empty text is `InvalidName` (Enter alone could answer a
    /// dialog); a text over [`herdr::MAX_REPLY_BYTES`] is [`HostError::TooLarge`]. An agent that
    /// names no instance (no kind, or neither a session nor a name;
    /// [`herdr::AgentIdentity::is_instance`]) is `CommandFailed` with [`herdr::OPEN_THE_PANE`],
    /// before anything is sent. A host without herdr is `NotInstalled`; a pane that is gone,
    /// holds no agent or another one (another session, name, terminal or kind), or whose shell
    /// has its foreground, `PaneNotFound`. Bounded by
    /// [`QUERY_TIMEOUT`]; dropping the future, or that timeout, stops the reply before anything
    /// more is sent. The text is never logged.
    pub async fn reply_to_pane(
        &self,
        session: Option<String>,
        pane_id: String,
        agent: herdr::AgentIdentity,
        text: String,
    ) -> Result<herdr::ReplyRoute, HostError> {
        if !session.as_deref().is_none_or(is_valid_herdr_session_name)
            || !is_valid_herdr_pane_id(&pane_id)
            || !is_valid_herdr_pane_id(&agent.terminal_id)
            || !agent.agent.as_deref().is_none_or(is_valid_agent_kind)
            || !agent.name.as_deref().is_none_or(is_valid_agent_kind)
            || !agent.session.as_ref().is_none_or(|session| {
                is_valid_agent_kind(&session.kind) && is_valid_agent_session(&session.value)
            })
            || text.is_empty()
        {
            return Err(HostError::InvalidName);
        }
        if !agent.is_instance() {
            return Err(HostError::CommandFailed {
                message: herdr::OPEN_THE_PANE.into(),
            });
        }
        if text.len() > herdr::MAX_REPLY_BYTES {
            return Err(HostError::TooLarge);
        }
        self.query(QUERY_TIMEOUT, |reply| HostCommand::ReplyToPane {
            session,
            pane_id,
            agent,
            text,
            deadline: tokio::time::Instant::now() + QUERY_TIMEOUT,
            reply,
        })
        .await
    }

    /// The yes/no permission prompt `agent` waits at in herdr pane `pane_id` of `session`
    /// (`None` is the default session), or `None` when it waits at none that can be answered
    /// from a notification (contracts.md, "Answer: approve or deny a permission prompt"): only a
    /// blocked Claude Code whose matched herdr rule is a permission prompt it shows on screen.
    /// Names are validated as for [`Self::reply_to_pane`] (`InvalidName`), and an agent that
    /// names no instance is `CommandFailed` with [`herdr::OPEN_THE_PANE`]. A host without herdr
    /// is `NotInstalled`; a pane that is gone or holds another agent instance `PaneNotFound`.
    /// Sends nothing to the pane. Bounded by [`QUERY_TIMEOUT`].
    pub async fn permission_prompt(
        &self,
        session: Option<String>,
        pane_id: String,
        agent: herdr::AgentIdentity,
    ) -> Result<Option<herdr::PermissionPrompt>, HostError> {
        validate_agent_target(session.as_deref(), &pane_id, &agent)?;
        self.query(QUERY_TIMEOUT, |reply| HostCommand::PermissionPrompt {
            session,
            pane_id,
            agent,
            reply,
        })
        .await
    }

    /// Approves (Enter on its highlighted first "Yes") or denies (Escape) the permission prompt
    /// `agent` waits at in herdr pane `pane_id` of `session`, the one [`Self::permission_prompt`]
    /// found at `seq`, with no terminal open. Every check runs again first, and any that fails
    /// sends nothing: `PromptChanged` when the agent is not at that prompt any more (another
    /// `seq`, no permission prompt, or, to approve, another option highlighted), `PaneNotFound`
    /// when the pane or that agent is gone or its shell has the foreground. Validation is
    /// [`Self::permission_prompt`]'s. Bounded by [`QUERY_TIMEOUT`]; dropping the future, or that
    /// timeout, stops the answer before its key is sent, as for [`Self::reply_to_pane`].
    pub async fn answer_permission(
        &self,
        session: Option<String>,
        pane_id: String,
        agent: herdr::AgentIdentity,
        seq: u64,
        answer: herdr::PermissionAnswer,
    ) -> Result<(), HostError> {
        validate_agent_target(session.as_deref(), &pane_id, &agent)?;
        self.query(QUERY_TIMEOUT, |reply| HostCommand::AnswerPermission {
            session,
            pane_id,
            agent,
            seq,
            answer,
            deadline: tokio::time::Instant::now() + QUERY_TIMEOUT,
            reply,
        })
        .await
    }

    /// Uploads an image for an agent to read (contracts.md, "Image paste"): `bytes` go over
    /// SFTP, on this connection, to `~/.cache/or2/images/or2-<UTC yyyyMMdd-HHmmss>-<6 hex>.<ext>`
    /// (`~` being where the server's SFTP starts, the login's home; the directory `0700`, the
    /// file `0600`, written to a temporary name and renamed); once its path is delivered, an
    /// upload removes that directory's `or2-*` files older than seven days, best effort and at
    /// most hourly. The connection's uploads share one SFTP session and run one at a time
    /// (contracts.md, "Upload speed"). No shell command runs.
    /// Resolves with the file's absolute path. `extension` is one of [`IMAGE_EXTENSIONS`]
    /// (any case; `jpeg` is kept as given, lower-cased), else `InvalidName`, as is an empty
    /// image; more than [`MAX_IMAGE_BYTES`] is `TooLarge`; both are refused before anything is
    /// sent. A server without SFTP is `SftpUnavailable`; other failures `CommandFailed`.
    /// Bounded by [`upload_timeout`] of its size. Dropping the future (a cancelled upload)
    /// stops the upload and removes what it made, best effort, also once the path was sent but
    /// not yet returned ([`UploadedImage`]).
    pub async fn upload_image(&self, bytes: Vec<u8>, extension: &str) -> Result<String, HostError> {
        let extension = extension.to_ascii_lowercase();
        if bytes.is_empty() || !IMAGE_EXTENSIONS.contains(&extension.as_str()) {
            return Err(HostError::InvalidName);
        }
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(HostError::TooLarge);
        }
        let timeout = upload_timeout(bytes.len());
        let uploaded = self
            .query(timeout, |reply| HostCommand::UploadImage {
                bytes,
                extension,
                reply,
            })
            .await?;
        // Taken, in the step that received it (nothing awaits in between): the host keeps the
        // image. A caller dropped before this never acknowledges it, and the host removes it.
        // A caller that comes for the path only after the host stopped waiting for its
        // acknowledgement finds the image removed: the path names nothing, so it is not returned.
        if uploaded.taken.send(()).is_err() {
            return Err(HostError::CommandFailed {
                message: "the upload took too long and its image was removed".into(),
            });
        }
        Ok(uploaded.path)
    }

    /// Installs herdr's integration `id` on the host (contracts.md, "v0.1.3: zero-config
    /// Reply", Lane App): `<herdr> integration install <id>` as one exec, with the herdr path from
    /// the capability probe, within the exec timeout. `id` must be one of
    /// [`herdr::INTEGRATIONS`], else `InvalidName` with nothing sent (it is the only input, and
    /// it goes as its own argument). `Ok` on exit 0; `NotInstalled` without herdr;
    /// `CommandFailed` with the first line of its stderr otherwise. A running agent loads the
    /// integration when it next starts.
    pub async fn install_herdr_integration(&self, id: String) -> Result<(), HostError> {
        if !herdr::is_integration(&id) {
            return Err(HostError::InvalidName);
        }
        self.query(QUERY_TIMEOUT, |reply| {
            HostCommand::InstallHerdrIntegration { id, reply }
        })
        .await
    }

    /// The state of herdr's integrations on the host (`<herdr> integration status`, one exec):
    /// each of [`herdr::INTEGRATIONS`] that herdr lists, current, outdated or not installed.
    /// `NotInstalled` without herdr; `CommandFailed` when herdr cannot say (a herdr without
    /// integrations, a slow host).
    pub async fn herdr_integrations(&self) -> Result<Vec<herdr::Integration>, HostError> {
        self.query(QUERY_TIMEOUT, |reply| HostCommand::HerdrIntegrations {
            reply,
        })
        .await
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

    /// Sends the query `command` builds around its reply sender, on a connected host, and waits
    /// for the answer for at most `timeout` ([`await_reply`]).
    async fn query<T>(
        &self,
        timeout: Duration,
        command: impl FnOnce(oneshot::Sender<Result<T, HostError>>) -> HostCommand,
    ) -> Result<T, HostError> {
        let (reply, response) = oneshot::channel();
        self.require_connected()?;
        self.send(command(reply))?;
        await_reply(response, timeout).await
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

    /// The signal that the user has ended the host, which stays usable after this driver exits.
    pub(crate) fn user_cancel(&self) -> UserCancel {
        self.shared.user_cancel.subscribe()
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
                    TerminalTransport::Ssh,
                    size(),
                    None,
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

    #[test]
    fn the_users_disconnect_is_a_signal_that_outlives_the_driver() {
        // An explicit disconnect sets it, and still does once the driver (and with it the
        // command queue) is gone, as after a lost connection.
        let (_recorder, handle, driver) = setup(false);
        let cancel = driver.user_cancel();
        assert!(!*cancel.borrow());
        drop(driver);
        assert!(
            !*cancel.borrow(),
            "a lost driver is not the user's disconnect"
        );
        handle.disconnect();
        assert!(*cancel.borrow());

        // So does releasing the last handle, and a late subscriber sees it too.
        let (_recorder, handle, driver) = setup(false);
        let cancel = driver.user_cancel();
        drop(driver);
        drop(handle);
        assert!(*cancel.borrow());
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
                .open_terminal(
                    TerminalTarget::Shell,
                    TerminalTransport::Ssh,
                    size(),
                    None,
                    sessions.clone()
                )
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
            handle
                .open_terminal(bad, TerminalTransport::Ssh, size(), None, sessions.clone())
                .err(),
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
            assert_eq!(
                handle
                    .focus_herdr_tab(session.map(str::to_owned), pane.replace('p', "t"))
                    .await,
                Err(HostError::InvalidName),
                "tab {session:?} {pane:?}"
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
            tmux_records_clients: true,
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
    async fn a_herdr_tab_focus_carries_its_names_and_is_answered_through_its_reply() {
        let (_recorder, handle, mut driver) = setup(false);
        connect(&mut driver);
        let answers = std::thread::spawn(move || {
            for answer in [Ok(()), Err(HostError::PaneNotFound)] {
                let HostCommand::FocusHerdrTab {
                    session,
                    tab_id,
                    reply,
                } = driver.blocking_next_command()
                else {
                    panic!("unexpected command")
                };
                assert_eq!(session, None);
                assert_eq!(tab_id, "w2:t3");
                reply.send(answer).unwrap();
            }
            driver
        });
        let focus = || handle.focus_herdr_tab(None, "w2:t3".into());
        assert_eq!(focus().await, Ok(()));
        assert_eq!(focus().await, Err(HostError::PaneNotFound));
        drop(answers.join().unwrap());
    }

    #[test]
    fn the_upload_timeout_grows_with_the_image_up_to_a_cap() {
        let seconds = |bytes| upload_timeout(bytes).as_secs();
        assert_eq!(seconds(1), 31);
        assert_eq!(seconds(100 * 1024), 31);
        assert_eq!(seconds(100 * 1024 + 1), 32);
        assert_eq!(seconds(3 * 1024 * 1024), 61, "a 3 MiB photo");
        assert_eq!(seconds(MAX_IMAGE_BYTES), 235);
        assert_eq!(seconds(usize::MAX / 2), MAX_UPLOAD_TIMEOUT.as_secs());
    }

    #[tokio::test(start_paused = true)]
    async fn an_upload_waits_its_size_s_timeout_not_the_query_timeout() {
        let (_recorder, handle, mut driver) = setup(false);
        connect(&mut driver);
        // The host takes the upload and never answers (a slow link).
        let held = tokio::spawn(async move {
            let command = driver.next_command().await;
            let HostCommand::UploadImage { reply, .. } = command else {
                panic!("unexpected command")
            };
            std::future::pending::<()>().await;
            drop(reply);
        });
        let started = tokio::time::Instant::now();
        let bytes = vec![1; 3 * 1024 * 1024];
        assert_eq!(
            handle.upload_image(bytes, "jpg").await,
            Err(HostError::CommandFailed {
                message: "the host did not answer in time".into()
            })
        );
        assert_eq!(started.elapsed(), Duration::from_secs(61));
        assert!(started.elapsed() > QUERY_TIMEOUT);
        held.abort();
    }

    /// The host side of an upload's last step: answers the next command's reply with `path` at
    /// `at`, says so on the returned receiver, and resolves with whether its caller acknowledged
    /// the path.
    fn answer_upload_at(
        mut driver: HostDriver,
        path: &str,
        at: tokio::time::Instant,
    ) -> (tokio::task::JoinHandle<bool>, oneshot::Receiver<()>) {
        let path = path.to_owned();
        let (answered, sent) = oneshot::channel();
        let host = tokio::spawn(async move {
            let HostCommand::UploadImage { reply, .. } = driver.next_command().await else {
                panic!("unexpected command")
            };
            tokio::time::sleep_until(at).await;
            let (uploaded, acknowledged) = UploadedImage::new(path);
            if reply.send(Ok(uploaded)).is_err() {
                return false;
            }
            let _ = answered.send(());
            acknowledged.await.is_ok()
        });
        (host, sent)
    }

    #[tokio::test(start_paused = true)]
    async fn an_uploaded_path_is_acknowledged_as_its_caller_takes_it() {
        let (_recorder, handle, mut driver) = setup(false);
        connect(&mut driver);
        let (host, _) = answer_upload_at(driver, "/home/u/i.png", tokio::time::Instant::now());
        assert_eq!(
            handle.upload_image(vec![1; 10], "png").await,
            Ok("/home/u/i.png".into())
        );
        assert!(host.await.unwrap(), "taken");
    }

    /// The path is sent while the caller is still waiting, and the caller stops waiting (a
    /// cancel) before it runs again: the path was queued, never taken. The host must hear so, to
    /// remove the image.
    #[tokio::test(start_paused = true)]
    async fn a_path_sent_as_its_caller_gives_up_is_never_acknowledged() {
        let (_recorder, handle, mut driver) = setup(false);
        connect(&mut driver);
        let (host, sent) = answer_upload_at(driver, "/home/u/i.png", tokio::time::Instant::now());
        let mut upload = Box::pin(handle.upload_image(vec![1; 10], "png"));
        // The caller waits for the host, which answers; the caller is cancelled before it runs
        // again.
        tokio::select! {
            biased;
            answered = sent => answered.unwrap(),
            result = &mut upload => panic!("answered before the host did: {result:?}"),
        }
        drop(upload);
        assert!(!host.await.unwrap(), "a dropped path is not taken");
    }

    /// The host's answer and the caller's deadline fall due at the same instant: whichever wins,
    /// the image is kept exactly when the caller got its path.
    #[tokio::test(start_paused = true)]
    async fn a_path_sent_at_its_callers_deadline_is_kept_only_if_returned() {
        let (_recorder, handle, mut driver) = setup(false);
        connect(&mut driver);
        let deadline = tokio::time::Instant::now() + upload_timeout(10);
        let (host, _) = answer_upload_at(driver, "/home/u/i.png", deadline);
        let result = handle.upload_image(vec![1; 10], "png").await;
        assert_eq!(tokio::time::Instant::now(), deadline);
        assert_eq!(host.await.unwrap(), result.is_ok(), "{result:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_path_whose_acknowledgement_the_host_stopped_waiting_for_is_not_returned() {
        let (_recorder, handle, mut driver) = setup(false);
        connect(&mut driver);
        // The host answers, then gives up waiting for the acknowledgement (its own timeout) and
        // removes the image before the caller comes for the path.
        let host = tokio::spawn(async move {
            let HostCommand::UploadImage { reply, .. } = driver.next_command().await else {
                panic!("unexpected command")
            };
            let (uploaded, acknowledged) = UploadedImage::new("/home/u/late.png".into());
            assert!(reply.send(Ok(uploaded)).is_ok());
            drop(acknowledged);
            driver
        });
        // The caller's future is first polled (sending the command) and then woken only after the
        // host has answered and dropped its acknowledgement.
        assert_eq!(
            handle.upload_image(vec![1; 10], "png").await,
            Err(HostError::CommandFailed {
                message: "the upload took too long and its image was removed".into()
            })
        );
        let _driver = host.await.unwrap();
    }

    fn claude(terminal: &str) -> herdr::AgentIdentity {
        herdr::AgentIdentity {
            terminal_id: terminal.into(),
            agent: Some("claude".into()),
            name: None,
            session: Some(herdr::AgentSession {
                kind: "id".into(),
                value: "0b1f6c1e-1111-4a8e-9a55-2a0c6a3b9d01".into(),
            }),
        }
    }

    #[tokio::test]
    async fn a_reply_is_validated_and_answered_through_its_reply() {
        let (_recorder, handle, mut driver) = setup(false);
        let reply = |session: Option<&str>, pane: &str, agent, text: String| {
            handle.reply_to_pane(session.map(str::to_owned), pane.to_owned(), agent, text)
        };
        // Refused before anything is sent, connected or not: names, the agent, an empty and an
        // over-long text.
        let at_limit = "é".repeat(herdr::MAX_REPLY_BYTES / 2);
        let agent = || claude("term_1");
        assert_eq!(
            reply(Some("a b"), "w1:p1", agent(), "hi".into()).await,
            Err(HostError::InvalidName)
        );
        assert_eq!(
            reply(None, "w1 p1", agent(), "hi".into()).await,
            Err(HostError::InvalidName)
        );
        for bad in [
            claude(""),
            claude("term 1"),
            herdr::AgentIdentity {
                agent: Some(String::new()),
                ..agent()
            },
            herdr::AgentIdentity {
                agent: Some("claude\n".into()),
                ..agent()
            },
            herdr::AgentIdentity {
                name: Some("rev\u{1b}iewer".into()),
                ..agent()
            },
            herdr::AgentIdentity {
                session: Some(herdr::AgentSession {
                    kind: "id".into(),
                    value: String::new(),
                }),
                ..agent()
            },
            herdr::AgentIdentity {
                session: Some(herdr::AgentSession {
                    kind: "id\r".into(),
                    value: "s".into(),
                }),
                ..agent()
            },
            herdr::AgentIdentity {
                session: Some(herdr::AgentSession {
                    kind: "id".into(),
                    value: "s".repeat(4097),
                }),
                ..agent()
            },
        ] {
            assert_eq!(
                reply(None, "w1:p1", bad.clone(), "hi".into()).await,
                Err(HostError::InvalidName),
                "{bad:?}"
            );
        }
        // An agent herdr reports no instance of (no kind, or neither a session nor a name):
        // only the pane can be answered, connected or not.
        for unknown in [
            herdr::AgentIdentity {
                agent: None,
                ..agent()
            },
            herdr::AgentIdentity {
                session: None,
                name: None,
                ..agent()
            },
        ] {
            assert_eq!(
                reply(None, "w1:p1", unknown.clone(), "hi".into()).await,
                Err(HostError::CommandFailed {
                    message: herdr::OPEN_THE_PANE.into()
                }),
                "{unknown:?}"
            );
        }
        assert_eq!(
            reply(None, "w1:p1", agent(), String::new()).await,
            Err(HostError::InvalidName)
        );
        assert_eq!(
            reply(None, "w1:p1", agent(), format!("{at_limit}x")).await,
            Err(HostError::TooLarge)
        );
        assert_eq!(
            reply(None, "w1:p1", agent(), "hi".into()).await,
            Err(HostError::NotConnected)
        );
        connect(&mut driver);
        let answers = std::thread::spawn(move || {
            for answer in [
                Ok(herdr::ReplyRoute::Prompted),
                Ok(herdr::ReplyRoute::Typed),
                Err(HostError::PaneNotFound),
            ] {
                let HostCommand::ReplyToPane {
                    session,
                    pane_id,
                    agent,
                    text,
                    deadline,
                    reply,
                } = driver.blocking_next_command()
                else {
                    panic!("unexpected command")
                };
                assert_eq!(session.as_deref(), Some("work"));
                assert_eq!(pane_id, "w1:p2");
                assert_eq!(agent, claude("term_1"));
                assert_eq!(
                    text.len(),
                    herdr::MAX_REPLY_BYTES,
                    "4 KiB exactly is allowed"
                );
                // The worker learns when its caller stops waiting.
                let left = deadline - tokio::time::Instant::now();
                assert!(
                    left <= QUERY_TIMEOUT
                        && left > QUERY_TIMEOUT - std::time::Duration::from_secs(5),
                    "{left:?}"
                );
                reply.send(answer).unwrap();
            }
            driver
        });
        let send = || reply(Some("work"), "w1:p2", agent(), at_limit.clone());
        assert_eq!(send().await, Ok(herdr::ReplyRoute::Prompted));
        assert_eq!(send().await, Ok(herdr::ReplyRoute::Typed));
        assert_eq!(send().await, Err(HostError::PaneNotFound));
        drop(answers.join().unwrap());
    }

    #[tokio::test]
    async fn a_permission_prompt_and_its_answer_are_validated_and_answered_through_their_reply() {
        let (_recorder, handle, mut driver) = setup(false);
        let agent = || claude("term_1");
        // Refused before anything is sent, connected or not, as a reply is.
        for (session, pane, bad) in [
            (Some("a b"), "w1:p1", agent()),
            (None, "w1 p1", agent()),
            (None, "w1:p1", claude("term 1")),
            (
                None,
                "w1:p1",
                herdr::AgentIdentity {
                    agent: Some("claude\n".into()),
                    ..agent()
                },
            ),
        ] {
            let session = session.map(str::to_owned);
            assert_eq!(
                handle
                    .permission_prompt(session.clone(), pane.into(), bad.clone())
                    .await,
                Err(HostError::InvalidName)
            );
            assert_eq!(
                handle
                    .answer_permission(
                        session,
                        pane.into(),
                        bad,
                        1,
                        herdr::PermissionAnswer::Approve
                    )
                    .await,
                Err(HostError::InvalidName)
            );
        }
        let unknown = herdr::AgentIdentity {
            session: None,
            name: None,
            ..agent()
        };
        let open_the_pane = Err(HostError::CommandFailed {
            message: herdr::OPEN_THE_PANE.into(),
        });
        assert_eq!(
            handle
                .permission_prompt(None, "w1:p1".into(), unknown.clone())
                .await,
            open_the_pane
        );
        assert_eq!(
            handle
                .answer_permission(
                    None,
                    "w1:p1".into(),
                    unknown,
                    1,
                    herdr::PermissionAnswer::Deny
                )
                .await,
            open_the_pane.map(drop)
        );
        assert_eq!(
            handle
                .permission_prompt(None, "w1:p1".into(), agent())
                .await,
            Err(HostError::NotConnected)
        );
        connect(&mut driver);
        let answers = std::thread::spawn(move || {
            let HostCommand::PermissionPrompt {
                session,
                pane_id,
                agent,
                reply,
            } = driver.blocking_next_command()
            else {
                panic!("unexpected command")
            };
            assert_eq!(
                (session.as_deref(), pane_id.as_str(), agent),
                (Some("work"), "w1:p2", claude("term_1"))
            );
            reply
                .send(Ok(Some(herdr::PermissionPrompt {
                    state_change_seq: 9,
                })))
                .unwrap();
            for result in [Ok(()), Err(HostError::PromptChanged)] {
                let HostCommand::AnswerPermission {
                    session,
                    pane_id,
                    agent,
                    seq,
                    answer,
                    deadline,
                    reply,
                } = driver.blocking_next_command()
                else {
                    panic!("unexpected command")
                };
                assert_eq!(
                    (session.as_deref(), pane_id.as_str(), agent, seq, answer),
                    (
                        Some("work"),
                        "w1:p2",
                        claude("term_1"),
                        9,
                        herdr::PermissionAnswer::Approve
                    )
                );
                // The worker learns when its caller stops waiting.
                let left = deadline - tokio::time::Instant::now();
                assert!(
                    left <= QUERY_TIMEOUT
                        && left > QUERY_TIMEOUT - std::time::Duration::from_secs(5),
                    "{left:?}"
                );
                reply.send(result).unwrap();
            }
            driver
        });
        assert_eq!(
            handle
                .permission_prompt(Some("work".into()), "w1:p2".into(), agent())
                .await,
            Ok(Some(herdr::PermissionPrompt {
                state_change_seq: 9
            }))
        );
        let approve = || {
            handle.answer_permission(
                Some("work".into()),
                "w1:p2".into(),
                agent(),
                9,
                herdr::PermissionAnswer::Approve,
            )
        };
        assert_eq!(approve().await, Ok(()));
        assert_eq!(approve().await, Err(HostError::PromptChanged));
        drop(answers.join().unwrap());
    }

    #[tokio::test]
    async fn navigation_validates_skips_a_shell_and_is_answered_through_its_reply() {
        let (_recorder, handle, mut driver) = setup(false);
        let tmux = TerminalTarget::Tmux {
            session_name: "main".into(),
        };
        // A shell has nothing to move: `Ok` whatever the host's state, and nothing is sent.
        assert_eq!(
            handle
                .navigate(TerminalTarget::Shell, None, TargetNav::NextWindow, None)
                .await,
            Ok(())
        );
        assert_eq!(
            handle
                .navigate(tmux.clone(), None, TargetNav::NextWindow, None)
                .await,
            Err(HostError::NotConnected)
        );
        connect(&mut driver);
        assert_eq!(
            handle
                .navigate(TerminalTarget::Shell, None, TargetNav::NextSession, None)
                .await,
            Ok(())
        );
        let herdr = |session: Option<&str>| TerminalTarget::Herdr {
            session: session.map(str::to_owned),
            pane_id: None,
        };
        for (target, pane) in [
            (
                TerminalTarget::Tmux {
                    session_name: "a:b".into(),
                },
                None,
            ),
            (herdr(Some("a b")), None),
            (herdr(None), Some("w1 p1")),
            (herdr(None), Some("")),
        ] {
            assert_eq!(
                handle
                    .navigate(
                        target.clone(),
                        pane.map(str::to_owned),
                        TargetNav::NextWindow,
                        None
                    )
                    .await,
                Err(HostError::InvalidName),
                "{target:?} {pane:?}"
            );
        }
        // A client id becomes part of a tmux option's name: only one `new_client_id` makes.
        for id in ["", "x", "@or2-client-1", "0123456789ABCDEF0123456789ABCDEF"] {
            assert_eq!(
                handle
                    .navigate(tmux.clone(), None, TargetNav::NextWindow, Some(id.into()))
                    .await,
                Err(HostError::InvalidName),
                "{id:?}"
            );
        }
        assert!(driver.commands.try_recv().is_err(), "nothing was enqueued");
        let id = crate::tmux::new_client_id();
        let sent_id = id.clone();

        let answers = std::thread::spawn(move || {
            let expected = [
                (
                    TerminalTarget::Tmux {
                        session_name: "main".into(),
                    },
                    None,
                    Some(sent_id),
                    TargetNav::NextWindow,
                    Ok(()),
                ),
                (
                    TerminalTarget::Herdr {
                        session: Some("work".into()),
                        pane_id: None,
                    },
                    Some("w1:p2".to_owned()),
                    None,
                    TargetNav::Pane {
                        direction: NavDirection::Left,
                    },
                    Err(HostError::PaneNotFound),
                ),
            ];
            for (want_target, want_pane, want_client, want_nav, answer) in expected {
                let HostCommand::Navigate {
                    target,
                    pane_id,
                    client_id,
                    nav,
                    reply,
                } = driver.blocking_next_command()
                else {
                    panic!("unexpected command")
                };
                assert_eq!(
                    (target, pane_id, client_id, nav),
                    (want_target, want_pane, want_client, want_nav)
                );
                reply.send(answer).unwrap();
            }
            driver
        });
        assert_eq!(
            handle
                .navigate(tmux, None, TargetNav::NextWindow, Some(id))
                .await,
            Ok(())
        );
        assert_eq!(
            handle
                .navigate(
                    herdr(Some("work")),
                    Some("w1:p2".into()),
                    TargetNav::Pane {
                        direction: NavDirection::Left
                    },
                    None
                )
                .await,
            Err(HostError::PaneNotFound)
        );
        let mut driver = answers.join().unwrap();
        driver.close(CloseReason::Disconnected);
        assert_eq!(
            handle
                .navigate(herdr(None), None, TargetNav::NextSession, None)
                .await,
            Err(HostError::Closed)
        );
    }

    #[tokio::test]
    async fn stopping_a_mosh_server_carries_its_pid_and_is_answered_through_its_reply() {
        let (_recorder, handle, mut driver) = setup(false);
        assert_eq!(
            handle.stop_mosh_server(7).await,
            Err(HostError::NotConnected)
        );
        connect(&mut driver);
        assert_eq!(
            handle.stop_mosh_server(0).await,
            Err(HostError::InvalidName),
            "pid 0 names no process"
        );
        let answers = std::thread::spawn(move || {
            for answer in [
                Ok(()),
                Err(HostError::CommandFailed {
                    message: "no channel".into(),
                }),
            ] {
                let HostCommand::StopMoshServer { pid, reply } = driver.blocking_next_command()
                else {
                    panic!("unexpected command")
                };
                assert_eq!(pid, 4242);
                reply.send(answer).unwrap();
            }
            driver
        });
        assert_eq!(handle.stop_mosh_server(4242).await, Ok(()));
        assert_eq!(
            handle.stop_mosh_server(4242).await,
            Err(HostError::CommandFailed {
                message: "no channel".into()
            })
        );
        drop(answers.join().unwrap());
    }

    #[tokio::test]
    async fn an_integration_install_takes_only_an_allowlisted_id_and_is_answered_through_its_reply()
    {
        let (_recorder, handle, mut driver) = setup(false);
        assert_eq!(
            handle.install_herdr_integration("pi".into()).await,
            Err(HostError::NotConnected)
        );
        connect(&mut driver);
        // Refused before anything is sent: the driver below would see it otherwise.
        for id in ["", "amp", "Pi", "pi ", "pi;id", "cursor-agent", "agy", "-h"] {
            assert_eq!(
                handle.install_herdr_integration(id.into()).await,
                Err(HostError::InvalidName),
                "{id:?}"
            );
        }
        let answers = std::thread::spawn(move || {
            let HostCommand::InstallHerdrIntegration { id, reply } = driver.blocking_next_command()
            else {
                panic!("unexpected command")
            };
            assert_eq!(id, "antigravity-cli");
            reply.send(Ok(())).unwrap();
            let HostCommand::InstallHerdrIntegration { id, reply } = driver.blocking_next_command()
            else {
                panic!("unexpected command")
            };
            assert_eq!(id, "pi");
            reply
                .send(Err(HostError::NotInstalled {
                    program: "herdr".into(),
                }))
                .unwrap();
            let HostCommand::HerdrIntegrations { reply } = driver.blocking_next_command() else {
                panic!("unexpected command")
            };
            reply
                .send(Ok(vec![herdr::Integration {
                    id: "pi".into(),
                    state: herdr::IntegrationState::Current,
                }]))
                .unwrap();
            driver
        });
        assert_eq!(
            handle
                .install_herdr_integration("antigravity-cli".into())
                .await,
            Ok(())
        );
        assert_eq!(
            handle.install_herdr_integration("pi".into()).await,
            Err(HostError::NotInstalled {
                program: "herdr".into()
            })
        );
        assert_eq!(
            handle.herdr_integrations().await,
            Ok(vec![herdr::Integration {
                id: "pi".into(),
                state: herdr::IntegrationState::Current,
            }])
        );
        let mut driver = answers.join().unwrap();
        driver.close(CloseReason::Disconnected);
        assert_eq!(
            handle.install_herdr_integration("pi".into()).await,
            Err(HostError::Closed)
        );
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
                .open_terminal(
                    TerminalTarget::Shell,
                    TerminalTransport::Ssh,
                    size(),
                    None,
                    sessions.clone()
                )
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
            .open_terminal(
                TerminalTarget::Shell,
                TerminalTransport::Mosh,
                size(),
                None,
                sessions.clone(),
            )
            .unwrap();
        let _budgeted = handle
            .open_terminal(
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

    /// Every tmux terminal has a client id of its own, shared by its handle and its driver (which
    /// gives it to the attach); other targets have none.
    #[test]
    fn each_tmux_terminal_gets_its_own_client_id() {
        let (_recorder, handle, mut driver) = setup(false);
        connect(&mut driver);
        let sessions = Arc::new(SessionRecorder::default());
        let tmux = TerminalTarget::Tmux {
            session_name: "work".into(),
        };
        let open = |target: &TerminalTarget, transport| {
            handle
                .open_terminal(target.clone(), transport, size(), None, sessions.clone())
                .unwrap()
        };
        let first = open(&tmux, TerminalTransport::Ssh);
        let second = open(&tmux, TerminalTransport::Mosh);
        let shell = open(&TerminalTarget::Shell, TerminalTransport::Ssh);
        let first_id = first.client_id().expect("a tmux terminal has an id");
        let second_id = second.client_id().expect("a tmux terminal has an id");
        assert!(crate::tmux::is_valid_client_id(first_id));
        assert_ne!(first_id, second_id, "one per terminal, not per target");
        assert_eq!(shell.client_id(), None);
        for want in [Some(first_id), Some(second_id), None] {
            let HostCommand::OpenTerminal {
                driver: session_driver,
                ..
            } = driver.blocking_next_command()
            else {
                panic!("expected OpenTerminal");
            };
            assert_eq!(session_driver.client_id(), want);
            session_driver.discard();
        }
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
            .open_terminal(
                target.clone(),
                TerminalTransport::Ssh,
                size(),
                None,
                sessions.clone(),
            )
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
            .open_terminal(
                TerminalTarget::Shell,
                TerminalTransport::Ssh,
                size(),
                None,
                user_closed.clone(),
            )
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
            .open_terminal(
                TerminalTarget::Shell,
                TerminalTransport::Ssh,
                size(),
                None,
                lost.clone(),
            )
            .unwrap();
        let reason = CloseReason::Failed(SessionFailure::ConnectionLost("gone".into()));
        driver.close(reason.clone());
        assert_eq!(queued.state(), SessionState::Closed(reason));
    }
}
