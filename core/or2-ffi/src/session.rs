//! Session contract for Kotlin: request, state, errors, input, the `Session` object and the
//! `SessionListener` callback. See docs/contracts.md for threading and ownership rules.

use std::fmt;
use std::sync::Arc;

use or2_core::input as core_input;
use or2_core::session as core;
use or2_core::term::TerminalSize;
use or2_core::transport::EndpointError;
use uniffi::UnexpectedUniFFICallbackError;
use zeroize::Zeroizing;

use crate::frame::TerminalFrame;
use crate::keys::PublicKeyInfo;

#[derive(uniffi::Record)]
pub struct ConnectRequest {
    pub host: String,
    pub port: u16,
    pub username: String,
    /// `ClientKeyMaterial.private_key`, decrypted from the Keystore for this call only.
    pub private_key: Vec<u8>,
    /// `PublicKeyInfo.openssh` lines Kotlin trusts for this host. Empty on first use.
    pub trusted_host_keys: Vec<String>,
    pub columns: u16,
    pub rows: u16,
}

impl fmt::Debug for ConnectRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectRequest")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("trusted_host_keys", &self.trusted_host_keys.len())
            .field("columns", &self.columns)
            .field("rows", &self.rows)
            .finish_non_exhaustive()
    }
}

impl ConnectRequest {
    /// Validates every field and decodes the key. The FFI copy of the key is zeroized here.
    pub(crate) fn validate(self) -> Result<core::ConnectRequest, ConnectError> {
        let private_key = Zeroizing::new(self.private_key);
        Ok(core::ConnectRequest::new(
            &self.host,
            self.port,
            &self.username,
            &private_key,
            &self.trusted_host_keys,
            self.columns,
            self.rows,
        )?)
    }
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ConnectError {
    #[error("host must be nonempty without whitespace or control characters")]
    InvalidHost,
    #[error("port must be nonzero")]
    InvalidPort,
    #[error("username must be nonempty without control characters")]
    InvalidUsername,
    #[error("the stored private key is unusable")]
    InvalidPrivateKey,
    #[error("trusted host key {index} is not an OpenSSH public key")]
    InvalidTrustedHostKey { index: u32 },
    #[error("terminal columns and rows must both be nonzero")]
    EmptyDimension,
}

impl From<core::ConnectError> for ConnectError {
    fn from(error: core::ConnectError) -> Self {
        match error {
            core::ConnectError::Endpoint(EndpointError::InvalidHost) => Self::InvalidHost,
            core::ConnectError::Endpoint(EndpointError::InvalidPort) => Self::InvalidPort,
            core::ConnectError::InvalidUsername => Self::InvalidUsername,
            core::ConnectError::InvalidPrivateKey(_) => Self::InvalidPrivateKey,
            core::ConnectError::InvalidTrustedHostKey { index } => Self::InvalidTrustedHostKey {
                index: u32::try_from(index).unwrap_or(u32::MAX),
            },
            core::ConnectError::EmptyDimension => Self::EmptyDimension,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum SessionState {
    Connecting,
    /// Show `presented.fingerprint`. `previously_trusted` empty means first use; otherwise the
    /// host key changed. Persist trust in Kotlin before calling `approve_host_key`.
    AwaitingHostKeyDecision {
        presented: PublicKeyInfo,
        previously_trusted: Vec<PublicKeyInfo>,
    },
    Authenticating,
    Connected,
    Closed {
        reason: CloseReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum CloseReason {
    Disconnected,
    RemoteExited { exit_status: Option<u32> },
    Failed { failure: SessionFailure },
}

/// `message` fields are diagnostics without secrets; do not match on them.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum SessionFailure {
    Unreachable {
        message: String,
    },
    TimedOut,
    HostKeyRejected,
    UnsupportedHostKey {
        message: String,
    },
    AuthenticationRejected,
    ShellRejected,
    /// A terminal target's program (`tmux`, `herdr`) is missing on the host.
    NotInstalled {
        program: String,
    },
    /// A command the terminal needed (for example a herdr pane focus) failed.
    CommandFailed {
        message: String,
    },
    ConnectionLost {
        message: String,
    },
    Protocol {
        message: String,
    },
    Internal {
        message: String,
    },
}

impl From<core::SessionState> for SessionState {
    fn from(state: core::SessionState) -> Self {
        match state {
            core::SessionState::Connecting => Self::Connecting,
            core::SessionState::AwaitingHostKey(prompt) => Self::AwaitingHostKeyDecision {
                presented: prompt.presented.info().into(),
                previously_trusted: prompt
                    .previously_trusted
                    .iter()
                    .map(|key| key.info().into())
                    .collect(),
            },
            core::SessionState::Authenticating => Self::Authenticating,
            core::SessionState::Connected => Self::Connected,
            core::SessionState::Closed(reason) => Self::Closed {
                reason: reason.into(),
            },
        }
    }
}

impl From<core::CloseReason> for CloseReason {
    fn from(reason: core::CloseReason) -> Self {
        match reason {
            core::CloseReason::Disconnected => Self::Disconnected,
            core::CloseReason::RemoteExited { exit_status } => Self::RemoteExited { exit_status },
            core::CloseReason::Failed(failure) => Self::Failed {
                failure: failure.into(),
            },
        }
    }
}

impl From<core::SessionFailure> for SessionFailure {
    fn from(failure: core::SessionFailure) -> Self {
        use core::SessionFailure as F;
        match failure {
            F::Unreachable(message) => Self::Unreachable { message },
            F::TimedOut => Self::TimedOut,
            F::HostKeyRejected => Self::HostKeyRejected,
            F::UnsupportedHostKey(message) => Self::UnsupportedHostKey { message },
            F::AuthenticationRejected => Self::AuthenticationRejected,
            F::ShellRejected => Self::ShellRejected,
            F::NotInstalled { program } => Self::NotInstalled { program },
            F::CommandFailed(message) => Self::CommandFailed { message },
            F::ConnectionLost(message) => Self::ConnectionLost { message },
            F::Protocol(message) => Self::Protocol { message },
            F::Internal(message) => Self::Internal { message },
        }
    }
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum SessionError {
    #[error("the session is not connected yet")]
    NotConnected,
    #[error("the session is closed")]
    Closed,
    #[error("no host key is awaiting a decision")]
    NoHostKeyPrompt,
    #[error("the fingerprint does not match the presented host key")]
    HostKeyMismatch,
    #[error("terminal columns and rows must both be nonzero")]
    EmptyDimension,
    #[error("function keys are F1 to F12; character keys need text without control characters")]
    InvalidKey,
}

impl From<core::SessionError> for SessionError {
    fn from(error: core::SessionError) -> Self {
        match error {
            core::SessionError::NotConnected => Self::NotConnected,
            core::SessionError::Closed => Self::Closed,
            core::SessionError::NoHostKeyPrompt => Self::NoHostKeyPrompt,
            core::SessionError::HostKeyMismatch => Self::HostKeyMismatch,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum TerminalKey {
    Enter,
    Tab,
    Backspace,
    Escape,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    /// F1 to F12.
    Function {
        number: u8,
    },
    /// The unmodified character, e.g. `c` for Ctrl+C.
    Character {
        text: String,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, uniffi::Record)]
pub struct KeyModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KeyInput {
    pub key: TerminalKey,
    pub modifiers: KeyModifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ViewportScroll {
    Top,
    Bottom,
    /// Negative moves up into history.
    Delta {
        rows: i32,
    },
}

impl TryFrom<KeyInput> for core_input::KeyInput {
    type Error = SessionError;

    fn try_from(input: KeyInput) -> Result<Self, SessionError> {
        use core_input::Key as K;
        let key = match input.key {
            TerminalKey::Enter => K::Enter,
            TerminalKey::Tab => K::Tab,
            TerminalKey::Backspace => K::Backspace,
            TerminalKey::Escape => K::Escape,
            TerminalKey::Insert => K::Insert,
            TerminalKey::Delete => K::Delete,
            TerminalKey::Home => K::Home,
            TerminalKey::End => K::End,
            TerminalKey::PageUp => K::PageUp,
            TerminalKey::PageDown => K::PageDown,
            TerminalKey::ArrowUp => K::ArrowUp,
            TerminalKey::ArrowDown => K::ArrowDown,
            TerminalKey::ArrowLeft => K::ArrowLeft,
            TerminalKey::ArrowRight => K::ArrowRight,
            TerminalKey::Function { number } => K::Function(number),
            TerminalKey::Character { text } => K::Character(text),
        };
        let m = input.modifiers;
        let modifiers = core_input::Modifiers {
            shift: m.shift,
            ctrl: m.ctrl,
            alt: m.alt,
            meta: m.meta,
        };
        core_input::KeyInput::new(key, modifiers).map_err(|_| SessionError::InvalidKey)
    }
}

impl From<ViewportScroll> for core_input::ViewportScroll {
    fn from(scroll: ViewportScroll) -> Self {
        match scroll {
            ViewportScroll::Top => Self::Top,
            ViewportScroll::Bottom => Self::Bottom,
            ViewportScroll::Delta { rows } => Self::Delta(rows),
        }
    }
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ListenerError {
    #[error("listener failed: {reason}")]
    Failed { reason: String },
}

impl From<UnexpectedUniFFICallbackError> for ListenerError {
    fn from(error: UnexpectedUniFFICallbackError) -> Self {
        Self::Failed {
            reason: error.reason,
        }
    }
}

/// Implemented in Kotlin. Called on a Rust-owned thread, never concurrently for one session,
/// in order. Return quickly (post to the main or render thread). Exceptions are ignored and
/// do not affect the session. Released by Rust right after `Closed` is delivered.
#[uniffi::export(callback_interface)]
pub trait SessionListener: Send + Sync {
    /// Every change after the initial `Connecting`. `Closed` is delivered exactly once, last.
    fn on_state_changed(&self, state: SessionState) -> Result<(), ListenerError>;
    /// A frame is ready for `Session.take_frame`. Not repeated until it is taken.
    fn on_frame_ready(&self) -> Result<(), ListenerError>;
}

pub(crate) struct ListenerObserver(pub(crate) Box<dyn SessionListener>);

impl core::SessionObserver for ListenerObserver {
    fn state_changed(&self, state: &core::SessionState) {
        let _ = self.0.on_state_changed(state.clone().into());
    }

    fn frame_ready(&self) {
        let _ = self.0.on_frame_ready();
    }
}

/// One SSH shell. Methods never block and may be called from any thread, including inside
/// listener callbacks. Kotlin owns it; `close()` without `disconnect()` also disconnects.
#[derive(uniffi::Object)]
pub struct Session {
    handle: core::SessionHandle,
}

impl Session {
    pub(crate) fn new(handle: core::SessionHandle) -> Arc<Self> {
        Arc::new(Self { handle })
    }
}

/// Validates synchronously; networking and all callbacks run on Rust-owned threads.
///
/// M1 path: removed when lane B lands (Kotlin moves to `connect_host` plus
/// `HostConnection.open_terminal`).
#[uniffi::export]
pub fn connect(
    request: ConnectRequest,
    listener: Box<dyn SessionListener>,
) -> Result<Arc<Session>, ConnectError> {
    let request = request.validate()?;
    Ok(Session::new(or2_core::ssh::connect(
        request,
        Arc::new(ListenerObserver(listener)),
    )))
}

#[uniffi::export]
impl Session {
    pub fn state(&self) -> SessionState {
        self.handle.state().into()
    }

    /// `fingerprint` must be the prompt's `presented.fingerprint`, binding the decision to the
    /// key the user saw.
    pub fn approve_host_key(&self, fingerprint: String) -> Result<(), SessionError> {
        Ok(self.handle.approve_host_key(&fingerprint)?)
    }

    pub fn reject_host_key(&self) -> Result<(), SessionError> {
        Ok(self.handle.reject_host_key()?)
    }

    /// Latest wins. Allowed before `Connected`; a full frame follows once connected.
    pub fn resize(&self, columns: u16, rows: u16) -> Result<(), SessionError> {
        let size = TerminalSize::new(columns, rows).map_err(|_| SessionError::EmptyDimension)?;
        Ok(self.handle.resize(size)?)
    }

    /// Committed IME text. Written as UTF-8; newlines become carriage returns.
    pub fn send_text(&self, text: String) -> Result<(), SessionError> {
        Ok(self.handle.send_text(text)?)
    }

    /// Keys row, hardware keys and modifier combinations; encoded with the terminal's modes.
    pub fn send_key(&self, input: KeyInput) -> Result<(), SessionError> {
        let input = input.try_into()?;
        Ok(self.handle.send_key(input)?)
    }

    pub fn scroll(&self, scroll: ViewportScroll) -> Result<(), SessionError> {
        Ok(self.handle.scroll(scroll.into())?)
    }

    /// The renderer lost its grid (new view): the next frame will be full.
    pub fn request_full_frame(&self) -> Result<(), SessionError> {
        Ok(self.handle.request_full_frame()?)
    }

    /// Changes since the last take, merged; `None` when nothing changed.
    pub fn take_frame(&self) -> Option<TerminalFrame> {
        self.handle.take_frame().map(Into::into)
    }

    /// Idempotent; `Closed { Disconnected }` follows through the listener.
    pub fn disconnect(&self) {
        self.handle.disconnect();
    }
}
