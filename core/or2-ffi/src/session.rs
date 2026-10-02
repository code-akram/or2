//! Session contract for Kotlin: request, state, errors, input, the `Session` object and the
//! `SessionListener` callback. See docs/contracts.md for threading and ownership rules.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use or2_core::host::TerminalTransport as CoreTransport;
use or2_core::input as core_input;
use or2_core::mosh::LinkHealth as CoreLinkHealth;
use or2_core::session as core;
use or2_core::term::TerminalSize;
use or2_core::transport::EndpointError;
use uniffi::UnexpectedUniFFICallbackError;
use zeroize::Zeroizing;

use crate::frame::TerminalFrame;
use crate::keys::PublicKeyInfo;

/// The request `contract_probe_session` validates (a test fixture; real connections use
/// `HostConnectRequest`).
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
    /// While the program tracks the mouse (`TerminalModes.mouse_tracking`): `rows` wheel events
    /// (negative = up) at the touched cell (`column`, `row`), in the terminal's mouse format.
    /// Without mouse tracking it is a `Delta` of `rows`.
    Wheel {
        rows: i32,
        column: u16,
        row: u16,
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
            ViewportScroll::Wheel { rows, column, row } => Self::Wheel { rows, column, row },
        }
    }
}

/// How a terminal reaches its host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TerminalTransport {
    /// A PTY channel on the host's SSH connection.
    Ssh,
    /// A mosh session, bootstrapped over the host's SSH connection.
    Mosh,
}

impl From<TerminalTransport> for CoreTransport {
    fn from(transport: TerminalTransport) -> Self {
        match transport {
            TerminalTransport::Ssh => Self::Ssh,
            TerminalTransport::Mosh => Self::Mosh,
        }
    }
}

/// How long a mosh server has been silent, as `on_link_health` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct LinkHealth {
    /// Milliseconds since anything at all arrived from the server.
    pub since_heard_ms: u64,
    /// Milliseconds since the server acknowledged something the client sent.
    pub since_ack_ms: u64,
}

impl From<CoreLinkHealth> for LinkHealth {
    fn from(health: CoreLinkHealth) -> Self {
        Self {
            since_heard_ms: health.since_heard_ms,
            since_ack_ms: health.since_ack_ms,
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
/// in order. Return quickly (post to the main or render thread; only `on_server_pid` may block
/// briefly, for a durable write). Exceptions are ignored and
/// do not affect the session. Released by Rust right after `Closed` is delivered.
#[uniffi::export(callback_interface)]
pub trait SessionListener: Send + Sync {
    /// Every change after the initial `Connecting`. `Closed` is delivered exactly once, last.
    fn on_state_changed(&self, state: SessionState) -> Result<(), ListenerError>;
    /// A frame is ready for `Session.take_frame`. Not repeated until it is taken.
    fn on_frame_ready(&self) -> Result<(), ListenerError>;
    /// mosh only, at most once a second and only when what the UI shows changes (the first
    /// sample, the link turning stale past 5 s of silence, each further second while stale,
    /// recovery); never called for SSH terminals. Delivered between `Connected` and `Closed`.
    fn on_link_health(&self, health: LinkHealth) -> Result<(), ListenerError>;
    /// The host's program set the clipboard (OSC 52 or OSC 1337 Copy), as text of at most
    /// 1 MiB (UTF-8). Both transports; only between `Connected` and `Closed`. A read request is
    /// never answered, so the host never learns the phone's clipboard.
    fn on_clipboard_write(&self, text: String) -> Result<(), ListenerError>;
    /// mosh only (API 14): the session's `mosh-server` runs on the host with this pid, at most
    /// once, before `Connected` and before the session sends the server anything. Rust waits
    /// for the return before it lets the server see its client: this is the one callback that
    /// may block briefly, so the app writes the pid durably here (the orphan record) and a
    /// process death at any moment afterwards still leaves the pid to stop. The same pid is
    /// `Session.server_pid()`. Never called for SSH terminals.
    fn on_server_pid(&self, pid: u32) -> Result<(), ListenerError>;
}

pub(crate) struct ListenerObserver(pub(crate) Box<dyn SessionListener>);

impl core::SessionObserver for ListenerObserver {
    fn state_changed(&self, state: &core::SessionState) {
        let _ = self.0.on_state_changed(state.clone().into());
    }

    fn frame_ready(&self) {
        let _ = self.0.on_frame_ready();
    }

    fn link_health(&self, health: CoreLinkHealth) {
        let _ = self.0.on_link_health(health.into());
    }

    fn clipboard_write(&self, text: String) {
        let _ = self.0.on_clipboard_write(text);
    }

    fn server_pid_known(&self, pid: u32) {
        let _ = self.0.on_server_pid(pid);
    }
}

/// One terminal: an SSH shell channel or a mosh session. Methods never block and may be called from any thread, including inside
/// listener callbacks. Kotlin owns it; `close()` without `disconnect()` also disconnects.
#[derive(uniffi::Object)]
pub struct Session {
    handle: core::SessionHandle,
    transport: TerminalTransport,
}

/// Every mosh session not yet known to be closed, for [`network_changed`].
static MOSH_SESSIONS: Mutex<Vec<Weak<Session>>> = Mutex::new(Vec::new());

impl Session {
    pub(crate) fn new(handle: core::SessionHandle, transport: TerminalTransport) -> Arc<Self> {
        let session = Arc::new(Self { handle, transport });
        if transport == TerminalTransport::Mosh {
            let mut live = MOSH_SESSIONS.lock().unwrap_or_else(PoisonError::into_inner);
            live.retain(|weak| weak.upgrade().is_some_and(|s| s.is_open()));
            live.push(Arc::downgrade(&session));
        }
        session
    }

    fn is_open(&self) -> bool {
        !matches!(self.handle.state(), core::SessionState::Closed(_))
    }
}

/// The device's network changed (Kotlin calls this from its default-network callback,
/// debounced). Every live mosh session opens a new UDP socket now (`Session.roam`), and every
/// established SSH host connection sends a keepalive at once so a connection the change
/// broke is noticed promptly. Returns at once; callable from any thread.
#[uniffi::export]
pub fn network_changed() {
    for session in live_mosh_sessions() {
        session.roam();
    }
    or2_core::ssh::network_changed();
}

/// The registered mosh sessions that are not closed, forgetting the ones that are (or that
/// Kotlin released).
fn live_mosh_sessions() -> Vec<Arc<Session>> {
    let mut live = MOSH_SESSIONS.lock().unwrap_or_else(PoisonError::into_inner);
    live.retain(|weak| weak.upgrade().is_some_and(|s| s.is_open()));
    live.iter().filter_map(Weak::upgrade).collect()
}

#[uniffi::export]
impl Session {
    pub fn state(&self) -> SessionState {
        self.handle.state().into()
    }

    /// How this terminal reaches its host; fixed for the session's life.
    pub fn transport(&self) -> TerminalTransport {
        self.transport
    }

    /// A mosh terminal's `mosh-server` process id on the host (API 10), set before the
    /// session is `Connected` and kept after it closes; `None` for SSH, and when the bootstrap
    /// did not report it. Not a secret. The app records it with the host while the session
    /// lives, so a server orphaned by the process's death can be stopped over the next
    /// connection (`HostConnection.stop_mosh_server`).
    pub fn server_pid(&self) -> Option<u32> {
        self.handle.server_pid()
    }

    /// A tmux terminal's client id (API 14): opaque, fixed for the session's life, and its
    /// own (two terminals on one tmux session have two; the session a transport swap moves a
    /// terminal to has a new one). Pass the shown session's id to `HostConnection.navigate`
    /// so a gesture moves this terminal's tmux client. `None` for shell and herdr terminals.
    pub fn client_id(&self) -> Option<String> {
        self.handle.client_id().map(str::to_owned)
    }

    /// mosh: open a new UDP socket now (the network changed), instead of noticing after
    /// seconds without answers. SSH: a no-op. Never fails; a closed session ignores it.
    pub fn roam(&self) {
        self.handle.roam();
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

    /// Types `text` and presses Enter so agent TUIs with paste-burst detection submit instead
    /// of inserting a newline: the text (one bracketed paste when the terminal has that mode
    /// on), then Enter as a separate write after a short pause. Empty text only presses
    /// Enter. Input sent afterwards stays behind the Enter. Requires `Connected`.
    pub fn submit_text(&self, text: String) -> Result<(), SessionError> {
        Ok(self.handle.submit_text(text)?)
    }

    /// Pastes `text` (API 16; the image path of contracts.md, "Image paste"): one bracketed
    /// paste when the terminal has that mode on (a paste end marker inside `text` is removed),
    /// else typed with newlines as carriage returns. No Enter. Empty text sends nothing. Input
    /// after a `submit_text` waits for its Enter. Requires `Connected`. A contract probe
    /// terminal echoes it as `paste` and the bytes a bracketed paste writes.
    pub fn paste_text(&self, text: String) -> Result<(), SessionError> {
        Ok(self.handle.paste_text(text)?)
    }

    /// Keys row, hardware keys and modifier combinations; encoded with the terminal's modes.
    pub fn send_key(&self, input: KeyInput) -> Result<(), SessionError> {
        let input = input.try_into()?;
        Ok(self.handle.send_key(input)?)
    }

    pub fn scroll(&self, scroll: ViewportScroll) -> Result<(), SessionError> {
        Ok(self.handle.scroll(scroll.into())?)
    }

    /// A tap while the program tracks the mouse (`TerminalModes.mouse_tracking`, API 15): a
    /// left-button press and release at the viewport cell (`column`, `row`; clamped to the
    /// grid), encoded with the terminal's own mouse format. Nothing is sent when the program
    /// does not track the mouse. Both transports.
    pub fn mouse_click(&self, column: u16, row: u16) -> Result<(), SessionError> {
        Ok(self.handle.mouse_click(column, row)?)
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

#[cfg(test)]
mod tests {
    use super::*;
    use or2_core::session::{Command, SessionObserver, SessionState, channel};

    struct Quiet;

    impl SessionObserver for Quiet {
        fn state_changed(&self, _: &SessionState) {}
        fn frame_ready(&self) {}
    }

    fn open(transport: TerminalTransport) -> (Arc<Session>, or2_core::session::SessionDriver) {
        let (handle, mut driver) = channel(Arc::new(Quiet));
        driver.transition(SessionState::Connected).unwrap();
        (Session::new(handle, transport), driver)
    }

    /// Held by every test that opens mosh sessions or calls `network_changed`: the registry is
    /// process-wide, and `live_mosh_sessions` briefly holds every live session, so a parallel
    /// test could keep a session alive past the drop another test checks.
    static REGISTRY: Mutex<()> = Mutex::new(());

    fn registry() -> std::sync::MutexGuard<'static, ()> {
        REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn registered(session: &Arc<Session>) -> bool {
        let weak = Arc::downgrade(session);
        MOSH_SESSIONS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|entry| Weak::ptr_eq(entry, &weak))
    }

    #[test]
    fn network_changed_roams_live_mosh_sessions_only() {
        let _registry = registry();
        let (mosh, mut mosh_driver) = open(TerminalTransport::Mosh);
        let (ssh, mut ssh_driver) = open(TerminalTransport::Ssh);
        assert!(registered(&mosh));
        assert!(!registered(&ssh), "SSH sessions have nothing to roam");

        network_changed();
        assert_eq!(mosh_driver.blocking_next_command(), Command::Roam);
        // Nothing was queued for the SSH session: the next command is the text sent now.
        ssh.send_text("a".into()).unwrap();
        assert_eq!(
            ssh_driver.blocking_next_command(),
            Command::Text("a".into())
        );

        // `Session.roam` is the same command, one session at a time.
        mosh.roam();
        assert_eq!(mosh_driver.blocking_next_command(), Command::Roam);
        ssh.roam();
        ssh.send_text("b".into()).unwrap();
        assert_eq!(ssh_driver.blocking_next_command(), Command::Roam);
        assert_eq!(
            ssh_driver.blocking_next_command(),
            Command::Text("b".into())
        );
    }

    #[test]
    fn a_mouse_click_reaches_the_driver_as_its_cell() {
        let (session, mut driver) = open(TerminalTransport::Ssh);
        session.mouse_click(7, 3).unwrap();
        assert_eq!(
            driver.blocking_next_command(),
            Command::MouseClick { column: 7, row: 3 }
        );
    }

    #[test]
    fn clipboard_writes_reach_the_listener() {
        struct Copies(Arc<std::sync::Mutex<Vec<String>>>);
        impl SessionListener for Copies {
            fn on_state_changed(&self, _: super::SessionState) -> Result<(), ListenerError> {
                Ok(())
            }
            fn on_frame_ready(&self) -> Result<(), ListenerError> {
                Ok(())
            }
            fn on_link_health(&self, _: LinkHealth) -> Result<(), ListenerError> {
                Ok(())
            }
            fn on_clipboard_write(&self, text: String) -> Result<(), ListenerError> {
                self.0.lock().unwrap().push(text);
                // A listener failure is ignored.
                Err(ListenerError::Failed {
                    reason: "ignored".into(),
                })
            }
            fn on_server_pid(&self, _: u32) -> Result<(), ListenerError> {
                Ok(())
            }
        }
        let copies = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observer = ListenerObserver(Box::new(Copies(copies.clone())));
        let (_handle, mut driver) = channel(Arc::new(observer));
        driver.transition(SessionState::Connected).unwrap();
        driver.publish_clipboard("copied".into());
        driver.publish_clipboard("again".into());
        assert_eq!(*copies.lock().unwrap(), ["copied", "again"]);
    }

    #[test]
    fn a_mosh_server_pid_reaches_the_listener_before_connected() {
        struct Pids(Arc<std::sync::Mutex<Vec<String>>>);
        impl SessionListener for Pids {
            fn on_state_changed(&self, state: super::SessionState) -> Result<(), ListenerError> {
                self.0.lock().unwrap().push(format!("{state:?}"));
                Ok(())
            }
            fn on_frame_ready(&self) -> Result<(), ListenerError> {
                Ok(())
            }
            fn on_link_health(&self, _: LinkHealth) -> Result<(), ListenerError> {
                Ok(())
            }
            fn on_clipboard_write(&self, _: String) -> Result<(), ListenerError> {
                Ok(())
            }
            fn on_server_pid(&self, pid: u32) -> Result<(), ListenerError> {
                self.0.lock().unwrap().push(format!("pid {pid}"));
                // A listener failure is ignored: the session goes on.
                Err(ListenerError::Failed {
                    reason: "ignored".into(),
                })
            }
        }
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observer = ListenerObserver(Box::new(Pids(events.clone())));
        let (handle, mut driver) = channel(Arc::new(observer));
        let session = Session::new(handle, TerminalTransport::Ssh);
        driver.set_server_pid(Some(4242));
        driver.transition(SessionState::Connected).unwrap();
        assert_eq!(session.server_pid(), Some(4242));
        assert_eq!(*events.lock().unwrap(), ["pid 4242", "Connected"]);
    }

    #[test]
    fn closed_and_released_mosh_sessions_leave_the_registry() {
        let _registry = registry();
        let (closed, mut closed_driver) = open(TerminalTransport::Mosh);
        let (released, _released_driver) = open(TerminalTransport::Mosh);
        let (kept, _kept_driver) = open(TerminalTransport::Mosh);
        let released_weak = Arc::downgrade(&released);
        assert!(registered(&closed) && registered(&released) && registered(&kept));

        closed_driver.close(or2_core::session::CloseReason::Disconnected);
        drop(released);
        let live = live_mosh_sessions();
        assert!(live.iter().any(|s| Arc::ptr_eq(s, &kept)));
        assert!(!live.iter().any(|s| Arc::ptr_eq(s, &closed)));
        assert!(!registered(&closed), "a closed session is forgotten");
        assert!(released_weak.upgrade().is_none());
        assert!(
            !MOSH_SESSIONS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .any(|entry| Weak::ptr_eq(entry, &released_weak)),
            "the registry holds no entry for a released session"
        );
        // The registry never keeps a session alive.
        drop(live);
        let weak = Arc::downgrade(&kept);
        drop(kept);
        assert!(weak.upgrade().is_none());
    }
}
