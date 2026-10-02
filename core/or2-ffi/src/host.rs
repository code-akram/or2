//! Host connection contract for Kotlin (FFI API 10): request, state, errors, terminal targets,
//! queries, the `HostConnection` object and the `HostListener` callback. See
//! docs/contracts.md for threading and ownership rules.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use or2_core::host as core;
use or2_core::term::TerminalSize;
use or2_core::transport::EndpointError;
use zeroize::Zeroizing;

use crate::herdr::{HerdrListener, HerdrListenerObserver, HerdrWatch};
use crate::keys::PublicKeyInfo;
use crate::session::{CloseReason, Session, SessionListener, TerminalTransport};

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HostAddress {
    pub host: String,
    pub port: u16,
}

#[derive(uniffi::Record)]
pub struct HostConnectRequest {
    /// In preference order, 1 to 8.
    pub addresses: Vec<HostAddress>,
    pub username: String,
    /// `ClientKeyMaterial.private_key`, decrypted from the Keystore for this call only.
    pub private_key: Vec<u8>,
    /// `PublicKeyInfo.openssh` lines Kotlin trusts for this host. Empty on first use.
    pub trusted_host_keys: Vec<String>,
}

impl fmt::Debug for HostConnectRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostConnectRequest")
            .field("addresses", &self.addresses)
            .field("username", &self.username)
            .field("trusted_host_keys", &self.trusted_host_keys.len())
            .finish_non_exhaustive()
    }
}

impl HostConnectRequest {
    /// Validates every field and decodes the key. The FFI copy of the key is zeroized here.
    pub(crate) fn validate(self) -> Result<core::HostConnectRequest, HostConnectError> {
        let private_key = Zeroizing::new(self.private_key);
        let addresses: Vec<(&str, u16)> = self
            .addresses
            .iter()
            .map(|address| (address.host.as_str(), address.port))
            .collect();
        Ok(core::HostConnectRequest::new(
            &addresses,
            &self.username,
            &private_key,
            &self.trusted_host_keys,
        )?)
    }
}

/// `InvalidAddress.index` is the position in `addresses`.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum HostConnectError {
    #[error("a host needs at least one address")]
    NoAddresses,
    #[error("a host has at most 8 addresses")]
    TooManyAddresses,
    #[error("address {index} is invalid")]
    InvalidAddress { index: u32 },
    #[error("username must be nonempty without control characters")]
    InvalidUsername,
    #[error("the stored private key is unusable")]
    InvalidPrivateKey,
    #[error("trusted host key {index} is not an OpenSSH public key")]
    InvalidTrustedHostKey { index: u32 },
}

fn index(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

impl From<core::HostConnectError> for HostConnectError {
    fn from(error: core::HostConnectError) -> Self {
        match error {
            core::HostConnectError::NoAddresses => Self::NoAddresses,
            core::HostConnectError::TooManyAddresses => Self::TooManyAddresses,
            core::HostConnectError::InvalidAddress {
                index: i,
                error: EndpointError::InvalidHost | EndpointError::InvalidPort,
            } => Self::InvalidAddress { index: index(i) },
            core::HostConnectError::InvalidUsername => Self::InvalidUsername,
            core::HostConnectError::InvalidPrivateKey(_) => Self::InvalidPrivateKey,
            core::HostConnectError::InvalidTrustedHostKey { index: i } => {
                Self::InvalidTrustedHostKey { index: index(i) }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum HostState {
    Connecting,
    /// Show `presented.fingerprint`. `previously_trusted` empty means first use; otherwise the
    /// host key changed. Persist trust in Kotlin before calling `approve_host_key`.
    AwaitingHostKeyDecision {
        presented: PublicKeyInfo,
        previously_trusted: Vec<PublicKeyInfo>,
    },
    Authenticating,
    /// `address_index` is the position in the request's `addresses` that won the race.
    Connected {
        address_index: u32,
    },
    Closed {
        reason: CloseReason,
    },
}

impl From<core::HostState> for HostState {
    fn from(state: core::HostState) -> Self {
        match state {
            core::HostState::Connecting => Self::Connecting,
            core::HostState::AwaitingHostKey(prompt) => Self::AwaitingHostKeyDecision {
                presented: prompt.presented.info().into(),
                previously_trusted: prompt
                    .previously_trusted
                    .iter()
                    .map(|key| key.info().into())
                    .collect(),
            },
            core::HostState::Authenticating => Self::Authenticating,
            core::HostState::Connected { address_index } => Self::Connected {
                address_index: index(address_index),
            },
            core::HostState::Closed(reason) => Self::Closed {
                reason: reason.into(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum HostError {
    #[error("the host is not connected yet")]
    NotConnected,
    #[error("the host connection is closed")]
    Closed,
    #[error("no host key is awaiting a decision")]
    NoHostKeyPrompt,
    #[error("the fingerprint does not match the presented host key")]
    HostKeyMismatch,
    #[error("terminal columns and rows must both be nonzero")]
    EmptyDimension,
    #[error("session, pane or tmux name is not allowed")]
    InvalidName,
    #[error("{program} is not installed on the host")]
    NotInstalled { program: String },
    /// `focus_herdr_pane`: the pane no longer exists in herdr.
    #[error("the herdr pane no longer exists")]
    PaneNotFound,
    /// `reason` is a diagnostic without secrets; do not match on it. (Not `message`: that
    /// would clash with `Throwable.message` in the generated Kotlin exception.)
    #[error("command failed: {reason}")]
    CommandFailed { reason: String },
}

impl From<core::HostError> for HostError {
    fn from(error: core::HostError) -> Self {
        match error {
            core::HostError::NotConnected => Self::NotConnected,
            core::HostError::Closed => Self::Closed,
            core::HostError::NoHostKeyPrompt => Self::NoHostKeyPrompt,
            core::HostError::HostKeyMismatch => Self::HostKeyMismatch,
            core::HostError::InvalidName => Self::InvalidName,
            core::HostError::NotInstalled { program } => Self::NotInstalled { program },
            core::HostError::PaneNotFound => Self::PaneNotFound,
            core::HostError::CommandFailed { message } => Self::CommandFailed { reason: message },
        }
    }
}

/// What a terminal opens. Names are validated by Rust (`InvalidName`): tmux names are nonempty,
/// at most 128 bytes, without control characters, `\`, `:` or `.`; herdr session names are
/// `[A-Za-z0-9_-]{1,64}`; pane ids `[A-Za-z0-9:_-]{1,128}`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
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

impl From<TerminalTarget> for core::TerminalTarget {
    fn from(target: TerminalTarget) -> Self {
        match target {
            TerminalTarget::Shell => Self::Shell,
            TerminalTarget::Tmux { session_name } => Self::Tmux { session_name },
            TerminalTarget::Herdr { session, pane_id } => Self::Herdr { session, pane_id },
        }
    }
}

/// What `scroll_target` does to the history a tmux or herdr target shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
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

impl From<TargetScroll> for core::TargetScroll {
    fn from(scroll: TargetScroll) -> Self {
        match scroll {
            TargetScroll::Up { lines } => Self::Up { lines },
            TargetScroll::Down { lines } => Self::Down { lines },
            TargetScroll::Bottom => Self::Bottom,
        }
    }
}

/// A direction on screen, for moving between panes (API 14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum NavDirection {
    Left,
    Right,
    Up,
    Down,
}

/// What `HostConnection.navigate` moves (API 14). Previous and next wrap around.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TargetNav {
    /// The next tmux window, or herdr tab of the focused workspace.
    NextWindow,
    PreviousWindow,
    /// The pane in `direction` (tmux: from the active pane; herdr: from `pane_id` or the focused one).
    Pane {
        direction: NavDirection,
    },
    /// The next tmux session (the terminal's tmux client switches to it), or herdr workspace.
    NextSession,
    PreviousSession,
}

impl From<NavDirection> for core::NavDirection {
    fn from(direction: NavDirection) -> Self {
        match direction {
            NavDirection::Left => Self::Left,
            NavDirection::Right => Self::Right,
            NavDirection::Up => Self::Up,
            NavDirection::Down => Self::Down,
        }
    }
}

impl From<TargetNav> for core::TargetNav {
    fn from(nav: TargetNav) -> Self {
        match nav {
            TargetNav::NextWindow => Self::NextWindow,
            TargetNav::PreviousWindow => Self::PreviousWindow,
            TargetNav::Pane { direction } => Self::Pane {
                direction: direction.into(),
            },
            TargetNav::NextSession => Self::NextSession,
            TargetNav::PreviousSession => Self::PreviousSession,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HerdrSessionInfo {
    pub name: String,
    pub running: bool,
    pub is_default: bool,
}

/// What the host offers. Programs are absolute paths; `None` means not installed.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HostCapabilities {
    pub tmux: Option<String>,
    pub herdr: Option<String>,
    pub mosh_server: Option<String>,
    pub utf8_locale: String,
    pub herdr_sessions: Vec<HerdrSessionInfo>,
}

impl From<core::HostCapabilities> for HostCapabilities {
    fn from(caps: core::HostCapabilities) -> Self {
        Self {
            tmux: caps.tmux,
            herdr: caps.herdr,
            mosh_server: caps.mosh_server,
            utf8_locale: caps.utf8_locale,
            herdr_sessions: caps
                .herdr_sessions
                .into_iter()
                .map(|session| HerdrSessionInfo {
                    name: session.name,
                    running: session.running,
                    is_default: session.is_default,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TmuxSession {
    pub name: String,
    pub windows: u32,
    pub attached_clients: u32,
    pub created_unix: i64,
    pub activity_unix: i64,
}

impl From<core::TmuxSession> for TmuxSession {
    fn from(session: core::TmuxSession) -> Self {
        Self {
            name: session.name,
            windows: session.windows,
            attached_clients: session.attached_clients,
            created_unix: session.created_unix,
            activity_unix: session.activity_unix,
        }
    }
}

/// Implemented in Kotlin. Same threading rules as `SessionListener`: a Rust-owned thread, never
/// concurrent for one connection, in order; exceptions are ignored; released right after
/// `Closed`.
#[uniffi::export(callback_interface)]
pub trait HostListener: Send + Sync {
    /// Every change after the initial `Connecting`. `Closed` is delivered exactly once, last.
    fn on_host_state_changed(&self, state: HostState) -> Result<(), crate::session::ListenerError>;
}

pub(crate) struct HostListenerObserver(pub(crate) Box<dyn HostListener>);

impl core::HostObserver for HostListenerObserver {
    fn state_changed(&self, state: &core::HostState) {
        let _ = self.0.on_host_state_changed(state.clone().into());
    }
}

/// One SSH connection to a host, shared by terminals, queries and herdr watches. Methods never
/// block (the `async` ones suspend); callable from any thread, including inside listener
/// callbacks. Kotlin owns it; `close()` without `disconnect()` also disconnects.
#[derive(uniffi::Object)]
pub struct HostConnection {
    handle: core::HostHandle,
}

impl HostConnection {
    pub(crate) fn new(handle: core::HostHandle) -> Arc<Self> {
        Arc::new(Self { handle })
    }
}

/// Validates synchronously; networking and all callbacks run on Rust-owned threads. The
/// connection races the request's addresses, asks for a host-key decision when needed, and
/// ends with exactly one `Closed`. Terminals and herdr watches on it close first (`Disconnected`
/// for a user disconnect, else the host's failure), then the host reports `Closed`. The
/// exception is a mosh terminal: it needs the SSH connection only to start, so a *lost*
/// connection leaves it running, while a user disconnect closes it like the others.
#[uniffi::export]
pub fn connect_host(
    request: HostConnectRequest,
    listener: Box<dyn HostListener>,
) -> Result<Arc<HostConnection>, HostConnectError> {
    let request = request.validate()?;
    Ok(HostConnection::new(or2_core::ssh::connect_host(
        request,
        Arc::new(HostListenerObserver(listener)),
    )))
}

#[uniffi::export(async_runtime = "tokio")]
impl HostConnection {
    pub fn state(&self) -> HostState {
        self.handle.state().into()
    }

    /// `fingerprint` must be the prompt's `presented.fingerprint`, binding the decision to the
    /// key the user saw.
    pub fn approve_host_key(&self, fingerprint: String) -> Result<(), HostError> {
        Ok(self.handle.approve_host_key(&fingerprint)?)
    }

    pub fn reject_host_key(&self) -> Result<(), HostError> {
        Ok(self.handle.reject_host_key()?)
    }

    /// Idempotent. Closes every terminal (mosh ones too) and watch on the host;
    /// `Closed { Disconnected }` follows through the listener.
    pub fn disconnect(&self) {
        self.handle.disconnect();
    }

    /// Opens a terminal session over `transport`. It starts in `Connecting` and reaches
    /// `Connected` once the channel is open (SSH) or the first datagram from the server
    /// authenticates (mosh); failures close it through its listener. Only allowed while the
    /// host is `Connected`.
    ///
    /// `mosh_budget_ms` (API 9) is for `Mosh` only and ignored for `Ssh`: a deadline counted
    /// from this call by which the session must be `Connected`, the capability probe, the pane
    /// focus, the `mosh-server` bootstrap, the UDP socket and the first authenticated datagram
    /// all spending from it. When it is spent first the session stops the server it started
    /// and closes `Failed { TimedOut }` (after the bootstrap exec has had up to 2 s to report
    /// its server, and the stop up to 5 s, so the close can come that much later). `None`
    /// keeps the default: 15 s for the first datagram, counted from the end of the bootstrap.
    /// Kotlin passes 5000 for an Auto choice and `None` for an explicit Mosh.
    pub fn open_terminal(
        &self,
        target: TerminalTarget,
        transport: TerminalTransport,
        columns: u16,
        rows: u16,
        mosh_budget_ms: Option<u32>,
        listener: Box<dyn SessionListener>,
    ) -> Result<Arc<Session>, HostError> {
        let size = TerminalSize::new(columns, rows).map_err(|_| HostError::EmptyDimension)?;
        let handle = self.handle.open_terminal_within(
            target.into(),
            transport.into(),
            size,
            mosh_budget_ms.map(|ms| Duration::from_millis(u64::from(ms))),
            Arc::new(crate::session::ListenerObserver(listener)),
        )?;
        Ok(Session::new(handle, transport))
    }

    /// Programs and locale are probed once per connection; `herdr_sessions` is read afresh
    /// on every call. Cancelling the coroutine cancels the query.
    pub async fn capabilities(&self) -> Result<HostCapabilities, HostError> {
        Ok(self.handle.capabilities().await?.into())
    }

    /// The path of `mosh-server` on the host, `None` when it is not installed (API 14).
    /// Resolved by the program probe alone (one exec round trip, cached per connection), never
    /// by herdr's session listing: the transport choice awaits this instead of
    /// `capabilities()`. Cancelling the coroutine drops the reply only.
    pub async fn mosh_server(&self) -> Result<Option<String>, HostError> {
        Ok(self.handle.mosh_server().await?)
    }

    /// Most recently active first; empty when no tmux server runs.
    pub async fn list_tmux_sessions(&self) -> Result<Vec<TmuxSession>, HostError> {
        Ok(self
            .handle
            .list_tmux_sessions()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Focuses `pane_id` in herdr `session` (`None` is the default session) and resolves once
    /// herdr has acknowledged it. herdr's focus is shared by every client of the session, and a
    /// terminal running `herdr` shows whatever is focused, so the app must call this (and await
    /// success) whenever an agent-target terminal is activated or reused, not only when it is
    /// first opened. `InvalidName` for a malformed session or pane id, `NotInstalled` without
    /// herdr, `PaneNotFound` when the pane has gone (refresh the inbox instead of showing the
    /// terminal), `CommandFailed` otherwise. Cancelling the coroutine drops the reply only.
    pub async fn focus_herdr_pane(
        &self,
        session: Option<String>,
        pane_id: String,
    ) -> Result<(), HostError> {
        Ok(self.handle.focus_herdr_pane(session, pane_id).await?)
    }

    /// Stops the `mosh-server` with process id `pid` on the host (API 10): one an earlier
    /// process left running when it died with its mosh session open (its key died with it, and
    /// `mosh-server` has no idle timeout). Only a process the host's `ps` names `mosh-server`
    /// is signalled, so a pid that was reused is left alone, and a server that is already gone
    /// is success. `Ok` therefore means "no such server runs any more"; an error (no free SSH
    /// channel, a slow host, a stop command that failed or could not inspect the process
    /// because the host has no usable `ps`, `Closed`) means it may still run and the caller should keep the
    /// pid for the next connection. `InvalidName` for pid 0. Cancelling the coroutine drops
    /// the reply only.
    pub async fn stop_mosh_server(&self, pid: u32) -> Result<(), HostError> {
        Ok(self.handle.stop_mosh_server(pid).await?)
    }

    /// Scrolls the history `target` shows, for a swipe when the program does not track the
    /// mouse (API 14; contracts.md, "Wheel-aware scrolling"). tmux: `copy-mode -e` then
    /// `send-keys -X -N <lines> scroll-up`/`scroll-down` over exec (`Bottom` is `-X cancel`).
    /// herdr: `pane.scroll` of `pane_id` (`None`: the session's focused pane), Rust keeping each
    /// pane's `offset_from_bottom` (`Bottom` is 0). A `Shell` target, or zero lines, does
    /// nothing and is `Ok`. `InvalidName` for a malformed name or pane id, `NotInstalled`
    /// without the program, `PaneNotFound` for a vanished herdr pane, `CommandFailed`
    /// otherwise. Call it at most once at a time per terminal (sum the deltas meanwhile).
    /// Cancelling the coroutine drops the reply only.
    pub async fn scroll_target(
        &self,
        target: TerminalTarget,
        pane_id: Option<String>,
        scroll: TargetScroll,
    ) -> Result<(), HostError> {
        Ok(self
            .handle
            .scroll_target(target.into(), pane_id, scroll.into())
            .await?)
    }

    /// Moves what a terminal on `target` shows (API 14), for the swipe gestures: tmux over exec
    /// (`next-window`, `previous-window`, `select-pane -L/-R/-U/-D`, and `switch-client -n/-p`
    /// of the terminal's own tmux client, found with `list-clients`), herdr through its API (the
    /// neighbouring tab of the focused workspace, `pane.focus_direction`, the neighbouring
    /// workspace). Next and previous wrap around; nowhere to go is `Ok`. `pane_id` is the herdr
    /// pane to move from (`None`: the focused one, which a herdr client shows); tmux ignores it.
    /// A `Shell` target returns `Ok(())` and does nothing. `InvalidName` for a malformed name,
    /// `NotInstalled` without the program, `PaneNotFound` for a vanished herdr pane,
    /// `CommandFailed` otherwise (a tmux session move with no client attached to the target).
    /// Cancelling the coroutine drops the reply only.
    pub async fn navigate(
        &self,
        target: TerminalTarget,
        pane_id: Option<String>,
        nav: TargetNav,
    ) -> Result<(), HostError> {
        Ok(self
            .handle
            .navigate(target.into(), pane_id, nav.into())
            .await?)
    }

    /// Watches a herdr session (`None` is the default session); it ends with the connection.
    pub fn watch_herdr(
        &self,
        session: Option<String>,
        listener: Box<dyn HerdrListener>,
    ) -> Result<Arc<HerdrWatch>, HostError> {
        let handle = self
            .handle
            .watch_herdr(session, Arc::new(HerdrListenerObserver(listener)))?;
        Ok(HerdrWatch::new(handle))
    }
}

#[cfg(test)]
mod tests {
    use or2_core::keys::ClientKey;
    use or2_core::session::{CloseReason as CoreCloseReason, SessionFailure};

    use super::*;

    fn address(host: &str, port: u16) -> HostAddress {
        HostAddress {
            host: host.into(),
            port,
        }
    }

    fn request(addresses: Vec<HostAddress>) -> HostConnectRequest {
        HostConnectRequest {
            addresses,
            username: "dev".into(),
            private_key: ClientKey::generate_ed25519("k").to_stored().to_vec(),
            trusted_host_keys: Vec::new(),
        }
    }

    #[test]
    fn debug_never_prints_key_material() {
        let request = request(vec![address("h", 22)]);
        let text = format!("{request:?}");
        assert!(
            !text.contains("OPENSSH") && !text.contains("private_key"),
            "{text}"
        );
        assert!(text.contains("dev") && text.contains("\"h\""));
    }

    #[test]
    fn validation_maps_every_core_error_with_indices() {
        let ok = request(vec![address("a", 22), address("b", 2222)])
            .validate()
            .unwrap();
        assert_eq!(ok.addresses.len(), 2);
        assert!(matches!(
            request(vec![]).validate(),
            Err(HostConnectError::NoAddresses)
        ));
        assert!(matches!(
            request(vec![address("h", 22); 9]).validate(),
            Err(HostConnectError::TooManyAddresses)
        ));
        assert!(matches!(
            request(vec![address("h", 22), address("a b", 22)]).validate(),
            Err(HostConnectError::InvalidAddress { index: 1 })
        ));
        assert!(matches!(
            request(vec![address("h", 0)]).validate(),
            Err(HostConnectError::InvalidAddress { index: 0 })
        ));
        let mut bad_user = request(vec![address("h", 22)]);
        bad_user.username = String::new();
        assert!(matches!(
            bad_user.validate(),
            Err(HostConnectError::InvalidUsername)
        ));
        let mut bad_key = request(vec![address("h", 22)]);
        bad_key.private_key = b"junk".to_vec();
        assert!(matches!(
            bad_key.validate(),
            Err(HostConnectError::InvalidPrivateKey)
        ));
        let mut bad_trust = request(vec![address("h", 22)]);
        bad_trust.trusted_host_keys = vec!["nope".into()];
        assert!(matches!(
            bad_trust.validate(),
            Err(HostConnectError::InvalidTrustedHostKey { index: 0 })
        ));
    }

    #[test]
    fn maps_states_errors_targets_and_records() {
        assert_eq!(
            HostState::from(core::HostState::Connected { address_index: 3 }),
            HostState::Connected { address_index: 3 }
        );
        assert_eq!(
            HostState::from(core::HostState::Closed(CoreCloseReason::Failed(
                SessionFailure::TimedOut
            ))),
            HostState::Closed {
                reason: CloseReason::Failed {
                    failure: crate::session::SessionFailure::TimedOut
                }
            }
        );
        assert_eq!(
            crate::session::SessionFailure::from(SessionFailure::NotInstalled {
                program: "tmux".into()
            }),
            crate::session::SessionFailure::NotInstalled {
                program: "tmux".into()
            }
        );
        assert_eq!(
            crate::session::SessionFailure::from(SessionFailure::CommandFailed("x".into())),
            crate::session::SessionFailure::CommandFailed {
                message: "x".into()
            }
        );
        assert_eq!(
            HostError::from(core::HostError::NotInstalled {
                program: "tmux".into()
            }),
            HostError::NotInstalled {
                program: "tmux".into()
            }
        );
        assert_eq!(
            HostError::from(core::HostError::InvalidName),
            HostError::InvalidName
        );
        assert_eq!(
            HostError::from(core::HostError::PaneNotFound),
            HostError::PaneNotFound
        );
        assert_eq!(
            core::TerminalTarget::from(TerminalTarget::Herdr {
                session: Some("s".into()),
                pane_id: None
            }),
            core::TerminalTarget::Herdr {
                session: Some("s".into()),
                pane_id: None
            }
        );
        let caps = HostCapabilities::from(core::HostCapabilities {
            tmux: Some("/usr/bin/tmux".into()),
            herdr: None,
            mosh_server: Some("/usr/bin/mosh-server".into()),
            utf8_locale: "C.UTF-8".into(),
            herdr_sessions: vec![core::HerdrSessionInfo {
                name: "default".into(),
                running: true,
                is_default: true,
            }],
        });
        assert_eq!(caps.herdr, None);
        assert_eq!(
            caps.herdr_sessions,
            [HerdrSessionInfo {
                name: "default".into(),
                running: true,
                is_default: true
            }]
        );
        let tmux = TmuxSession::from(core::TmuxSession {
            name: "main".into(),
            windows: 3,
            attached_clients: 1,
            created_unix: -1,
            activity_unix: i64::MAX,
        });
        assert_eq!((tmux.windows, tmux.created_unix), (3, -1));
        assert_eq!(tmux.activity_unix, i64::MAX);
        assert_eq!(
            core::TargetScroll::from(TargetScroll::Up { lines: 3 }),
            core::TargetScroll::Up { lines: 3 }
        );
        assert_eq!(
            core::TargetScroll::from(TargetScroll::Down { lines: 4 }),
            core::TargetScroll::Down { lines: 4 }
        );
        assert_eq!(
            core::TargetScroll::from(TargetScroll::Bottom),
            core::TargetScroll::Bottom
        );
    }

    #[test]
    fn navigation_maps_every_move_and_direction() {
        for (direction, core_direction) in [
            (NavDirection::Left, core::NavDirection::Left),
            (NavDirection::Right, core::NavDirection::Right),
            (NavDirection::Up, core::NavDirection::Up),
            (NavDirection::Down, core::NavDirection::Down),
        ] {
            assert_eq!(core::NavDirection::from(direction), core_direction);
            assert_eq!(
                core::TargetNav::from(TargetNav::Pane { direction }),
                core::TargetNav::Pane {
                    direction: core_direction
                }
            );
        }
        for (nav, core_nav) in [
            (TargetNav::NextWindow, core::TargetNav::NextWindow),
            (TargetNav::PreviousWindow, core::TargetNav::PreviousWindow),
            (TargetNav::NextSession, core::TargetNav::NextSession),
            (TargetNav::PreviousSession, core::TargetNav::PreviousSession),
        ] {
            assert_eq!(core::TargetNav::from(nav), core_nav);
        }
    }

    #[test]
    fn open_terminal_rejects_empty_dimensions_before_touching_the_host() {
        struct Silent;
        impl SessionListener for Silent {
            fn on_state_changed(
                &self,
                _: crate::session::SessionState,
            ) -> Result<(), crate::session::ListenerError> {
                Ok(())
            }
            fn on_frame_ready(&self) -> Result<(), crate::session::ListenerError> {
                Ok(())
            }
            fn on_link_health(
                &self,
                _: crate::session::LinkHealth,
            ) -> Result<(), crate::session::ListenerError> {
                Ok(())
            }
            fn on_clipboard_write(&self, _: String) -> Result<(), crate::session::ListenerError> {
                Ok(())
            }
        }
        struct Quiet;
        impl HostListener for Quiet {
            fn on_host_state_changed(
                &self,
                _: HostState,
            ) -> Result<(), crate::session::ListenerError> {
                Ok(())
            }
        }
        // A loopback address nothing listens on: the connection fails fast, offline.
        let host = connect_host(request(vec![address("127.0.0.1", 1)]), Box::new(Quiet)).unwrap();
        assert_eq!(
            host.open_terminal(
                TerminalTarget::Shell,
                TerminalTransport::Ssh,
                0,
                24,
                None,
                Box::new(Silent)
            )
            .err(),
            Some(HostError::EmptyDimension)
        );
    }
}
