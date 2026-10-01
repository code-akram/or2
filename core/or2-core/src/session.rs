//! Session lifecycle shared by every session implementation.
//!
//! [`channel`] splits a session into a [`SessionHandle`], which the FFI layer wraps for Kotlin,
//! and a [`SessionDriver`], which the task that owns the network connection and terminal uses.
//! The handle never blocks: it validates a request against the current state and enqueues a
//! [`Command`]. The driver owns every state change and frame publication and calls the
//! [`SessionObserver`] from its own thread, in order, without holding any lock.
//!
//! ```text
//! Connecting ──▶ AwaitingHostKey ──▶ Authenticating ──▶ Connected
//!     │  │                                  ▲                ▲
//!     │  └──────────────────────────────────┘                │
//!     └──────────── channel sessions (M2) ───────────────────┘
//! any state ──▶ Closed (terminal, once)
//! ```
//!
//! A session that owns a channel on an established host connection goes straight from
//! `Connecting` to `Connected`: host-key and authentication states belong to the host.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::mpsc;

use crate::frame::{Frame, FrameError, FrameMailbox, TakenFrame};
use crate::input::{KeyInput, ViewportScroll};
use crate::keys::{ClientKey, KeyError};
use crate::term::TerminalSize;
use crate::transport::{Endpoint, EndpointError};
use crate::trust::{HostKey, HostKeyVerdict};

/// A validated single-session request. The shipped connection is
/// [`crate::host::HostConnectRequest`] (terminals are channels of a host connection); this is
/// what the FFI's `contract_probe_session` fixture validates, with the same field rules, for the
/// session contract tests.
#[derive(Debug)]
pub struct ConnectRequest {
    pub endpoint: Endpoint,
    pub username: String,
    pub key: ClientKey,
    pub trusted_host_keys: Vec<HostKey>,
    pub size: TerminalSize,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectError {
    #[error(transparent)]
    Endpoint(#[from] EndpointError),
    #[error("username must be nonempty without control characters")]
    InvalidUsername,
    #[error("stored private key is unusable: {0}")]
    InvalidPrivateKey(KeyError),
    #[error("trusted host key {index} is not an OpenSSH public key")]
    InvalidTrustedHostKey { index: usize },
    #[error("terminal columns and rows must both be nonzero")]
    EmptyDimension,
}

impl ConnectRequest {
    pub fn new(
        host: &str,
        port: u16,
        username: &str,
        private_key: &[u8],
        trusted_host_keys: &[String],
        columns: u16,
        rows: u16,
    ) -> Result<Self, ConnectError> {
        let endpoint = Endpoint::new(host, port)?;
        if username.is_empty() || username.chars().any(char::is_control) {
            return Err(ConnectError::InvalidUsername);
        }
        let size = TerminalSize::new(columns, rows).map_err(|_| ConnectError::EmptyDimension)?;
        let trusted_host_keys = trusted_host_keys
            .iter()
            .enumerate()
            .map(|(index, line)| {
                HostKey::from_openssh(line)
                    .map_err(|_| ConnectError::InvalidTrustedHostKey { index })
            })
            .collect::<Result<_, _>>()?;
        let key = ClientKey::from_stored(private_key).map_err(ConnectError::InvalidPrivateKey)?;
        Ok(Self {
            endpoint,
            username: username.to_owned(),
            key,
            trusted_host_keys,
            size,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyPrompt {
    pub presented: HostKey,
    /// Keys trusted for this host before. Empty on first use; otherwise the key has changed.
    pub previously_trusted: Vec<HostKey>,
}

impl HostKeyPrompt {
    pub fn verdict(&self) -> HostKeyVerdict {
        crate::trust::verify(&self.presented, &self.previously_trusted)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// Transport and SSH handshake in progress. The initial state; not delivered as a change.
    Connecting,
    /// Waiting for the user to approve or reject an untrusted host key.
    AwaitingHostKey(HostKeyPrompt),
    Authenticating,
    /// Shell open: frames flow and input is accepted.
    Connected,
    Closed(CloseReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    /// The user disconnected, or Kotlin released the session.
    Disconnected,
    /// The remote shell ended.
    RemoteExited {
        exit_status: Option<u32>,
    },
    Failed(SessionFailure),
}

/// Why a session failed. Messages are diagnostics without secrets, not for matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionFailure {
    /// Name resolution or TCP connection failed.
    Unreachable(String),
    TimedOut,
    HostKeyRejected,
    /// The server offered only a host certificate or an unsupported key type.
    UnsupportedHostKey(String),
    AuthenticationRejected,
    /// The server refused the PTY or shell request, or, on a host, the session channel itself
    /// (OpenSSH `MaxSessions`: 10 per connection by default). The connection is healthy.
    ShellRejected,
    /// The terminal's program (`tmux`, `herdr`) is not installed on the host.
    NotInstalled {
        program: String,
    },
    /// A command the terminal needed (for example a herdr pane focus) failed.
    CommandFailed(String),
    ConnectionLost(String),
    Protocol(String),
    Internal(String),
}

impl SessionState {
    fn name(&self) -> &'static str {
        match self {
            Self::Connecting => "Connecting",
            Self::AwaitingHostKey(_) => "AwaitingHostKey",
            Self::Authenticating => "Authenticating",
            Self::Connected => "Connected",
            Self::Closed(_) => "Closed",
        }
    }

    fn can_become(&self, next: &SessionState) -> bool {
        use SessionState::*;
        matches!(
            (self, next),
            (Connecting, AwaitingHostKey(_) | Authenticating | Connected)
                | (AwaitingHostKey(_), Authenticating)
                | (Authenticating, Connected)
                | (
                    Connecting | AwaitingHostKey(_) | Authenticating | Connected,
                    Closed(_)
                )
        )
    }
}

/// Receives session changes on the driver's thread. Implementations must return quickly and
/// may call back into the [`SessionHandle`].
pub trait SessionObserver: Send + Sync {
    fn state_changed(&self, state: &SessionState);
    /// The frame mailbox became non-empty. Not repeated until the renderer takes the frame.
    fn frame_ready(&self);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    ApproveHostKey {
        fingerprint: String,
    },
    RejectHostKey,
    /// Latest wins; allowed before `Connected` so the PTY opens at the right size.
    Resize(TerminalSize),
    Text(String),
    /// Text, then Enter as a separate, delayed write (see [`crate::submit`]).
    Submit(String),
    Key(KeyInput),
    Scroll(ViewportScroll),
    /// The renderer lost its grid: publish a full frame.
    FullFrame,
    /// Explicit disconnect, or every handle was dropped.
    Disconnect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    #[error("the session is not connected yet")]
    NotConnected,
    #[error("the session is closed")]
    Closed,
    #[error("no host key is awaiting a decision")]
    NoHostKeyPrompt,
    #[error("the fingerprint does not match the presented host key")]
    HostKeyMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid session transition {from} -> {to}")]
pub struct TransitionError {
    pub from: &'static str,
    pub to: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PublishError {
    #[error("frames are published only while connected")]
    NotConnected,
    #[error(transparent)]
    Frame(#[from] FrameError),
}

struct Shared {
    state: Mutex<SessionState>,
    frames: Mutex<FrameMailbox>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn channel(observer: Arc<dyn SessionObserver>) -> (SessionHandle, SessionDriver) {
    let shared = Arc::new(Shared {
        state: Mutex::new(SessionState::Connecting),
        frames: Mutex::new(FrameMailbox::default()),
    });
    let (sender, receiver) = mpsc::unbounded_channel();
    (
        SessionHandle {
            shared: Arc::clone(&shared),
            commands: sender,
        },
        SessionDriver {
            shared,
            commands: receiver,
            observer: Some(observer),
        },
    )
}

/// Kotlin's side of a session. All methods are non-blocking and callable from any thread.
/// Dropping the last handle disconnects.
pub struct SessionHandle {
    shared: Arc<Shared>,
    commands: mpsc::UnboundedSender<Command>,
}

impl SessionHandle {
    pub fn state(&self) -> SessionState {
        lock(&self.shared.state).clone()
    }

    pub fn approve_host_key(&self, fingerprint: &str) -> Result<(), SessionError> {
        match &*lock(&self.shared.state) {
            SessionState::AwaitingHostKey(prompt)
                if prompt.presented.fingerprint() == fingerprint => {}
            SessionState::AwaitingHostKey(_) => return Err(SessionError::HostKeyMismatch),
            SessionState::Closed(_) => return Err(SessionError::Closed),
            _ => return Err(SessionError::NoHostKeyPrompt),
        }
        self.send(Command::ApproveHostKey {
            fingerprint: fingerprint.to_owned(),
        })
    }

    pub fn reject_host_key(&self) -> Result<(), SessionError> {
        match &*lock(&self.shared.state) {
            SessionState::AwaitingHostKey(_) => {}
            SessionState::Closed(_) => return Err(SessionError::Closed),
            _ => return Err(SessionError::NoHostKeyPrompt),
        }
        self.send(Command::RejectHostKey)
    }

    pub fn resize(&self, size: TerminalSize) -> Result<(), SessionError> {
        if matches!(*lock(&self.shared.state), SessionState::Closed(_)) {
            return Err(SessionError::Closed);
        }
        self.send(Command::Resize(size))
    }

    pub fn send_text(&self, text: String) -> Result<(), SessionError> {
        self.require_connected()?;
        if text.is_empty() {
            return Ok(());
        }
        self.send(Command::Text(text))
    }

    /// Types `text` and presses Enter so a program with paste-burst detection sees a submit,
    /// not a pasted newline: the driver writes the text (as one bracketed paste when the
    /// terminal has that mode on), pauses [`crate::submit::SUBMIT_ENTER_DELAY`] and writes Enter
    /// as its own write. Empty text only presses Enter. Input sent afterwards stays behind
    /// the Enter.
    pub fn submit_text(&self, text: String) -> Result<(), SessionError> {
        self.require_connected()?;
        self.send(Command::Submit(text))
    }

    pub fn send_key(&self, key: KeyInput) -> Result<(), SessionError> {
        self.require_connected()?;
        self.send(Command::Key(key))
    }

    pub fn scroll(&self, scroll: ViewportScroll) -> Result<(), SessionError> {
        self.require_connected()?;
        self.send(Command::Scroll(scroll))
    }

    pub fn request_full_frame(&self) -> Result<(), SessionError> {
        self.require_connected()?;
        self.send(Command::FullFrame)
    }

    /// The merged changes since the last take, or `None` if nothing changed. Frames published
    /// before the session closed remain takeable afterwards.
    pub fn take_frame(&self) -> Option<TakenFrame> {
        lock(&self.shared.frames).take()
    }

    /// Idempotent. The `Closed` state arrives through the observer.
    pub fn disconnect(&self) {
        let _ = self.commands.send(Command::Disconnect);
    }

    fn require_connected(&self) -> Result<(), SessionError> {
        match *lock(&self.shared.state) {
            SessionState::Connected => Ok(()),
            SessionState::Closed(_) => Err(SessionError::Closed),
            _ => Err(SessionError::NotConnected),
        }
    }

    fn send(&self, command: Command) -> Result<(), SessionError> {
        self.commands
            .send(command)
            .map_err(|_| SessionError::Closed)
    }
}

/// The session task's side. Dropping it without closing reports an internal failure, so
/// Kotlin always receives exactly one `Closed`.
pub struct SessionDriver {
    shared: Arc<Shared>,
    commands: mpsc::UnboundedReceiver<Command>,
    observer: Option<Arc<dyn SessionObserver>>,
}

impl SessionDriver {
    pub async fn next_command(&mut self) -> Command {
        self.commands.recv().await.unwrap_or(Command::Disconnect)
    }

    /// For drivers running on a plain thread. Must not be called inside an async runtime.
    pub fn blocking_next_command(&mut self) -> Command {
        self.commands.blocking_recv().unwrap_or(Command::Disconnect)
    }

    pub fn state(&self) -> SessionState {
        lock(&self.shared.state).clone()
    }

    /// Moves to `next` and notifies the observer. On `Closed` the observer is released
    /// afterwards, breaking any reference cycle through Kotlin, and further commands fail.
    pub fn transition(&mut self, next: SessionState) -> Result<(), TransitionError> {
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
        let closing = matches!(next, SessionState::Closed(_));
        let observer = if closing {
            self.commands.close();
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
        let _ = self.transition(SessionState::Closed(reason));
    }

    /// Releases a driver whose handle was never returned to anyone: closes silently, without
    /// calling the observer.
    pub fn discard(mut self) {
        *lock(&self.shared.state) = SessionState::Closed(CloseReason::Disconnected);
        self.commands.close();
        self.observer = None;
    }

    pub fn publish(&mut self, frame: Frame) -> Result<(), PublishError> {
        if *lock(&self.shared.state) != SessionState::Connected {
            return Err(PublishError::NotConnected);
        }
        let notify = lock(&self.shared.frames).publish(frame)?;
        if notify && let Some(observer) = &self.observer {
            observer.frame_ready();
        }
        Ok(())
    }
}

impl Drop for SessionDriver {
    fn drop(&mut self) {
        self.close(CloseReason::Failed(SessionFailure::Internal(
            "session task ended without closing".into(),
        )));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use super::*;
    use crate::frame::{Cell, CellStyle, CellWidth, Rgb, Row, Scrollback};
    use crate::input::{Key, Modifiers};

    #[derive(Default)]
    struct Recorder {
        events: Mutex<Vec<String>>,
        handle: Mutex<Option<Arc<SessionHandle>>>,
    }

    impl SessionObserver for Recorder {
        fn state_changed(&self, state: &SessionState) {
            // Reentrancy: reading state from inside the callback must not deadlock.
            if let Some(handle) = &*lock(&self.handle) {
                assert_eq!(&handle.state(), state);
            }
            lock(&self.events).push(state.name().into());
        }

        fn frame_ready(&self) {
            let taken = lock(&self.handle)
                .as_ref()
                .map(|handle| handle.take_frame());
            lock(&self.events).push(format!("frame_ready(taken={})", taken.flatten().is_some()));
        }
    }

    fn setup(reentrant: bool) -> (Arc<Recorder>, Arc<SessionHandle>, SessionDriver) {
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

    fn frame(full: bool) -> Frame {
        let size = TerminalSize::new(3, 2).unwrap();
        let style = CellStyle::plain(Rgb::new(1, 2, 3), Rgb::new(4, 5, 6));
        let row = |index| {
            let cell = Cell {
                text: "x".into(),
                width: CellWidth::Narrow,
                style,
            };
            Row::new(index, false, vec![cell; 3])
        };
        if full {
            Frame::full(
                size,
                vec![row(0), row(1)],
                None,
                style.background,
                Scrollback::default(),
            )
        } else {
            Frame::delta(
                size,
                vec![row(1)],
                None,
                style.background,
                Scrollback::default(),
            )
        }
        .unwrap()
    }

    #[test]
    fn host_key_decision_is_bound_to_the_presented_key() {
        let (recorder, handle, mut driver) = setup(false);
        assert_eq!(handle.state(), SessionState::Connecting);
        assert!(
            events(&recorder).is_empty(),
            "the initial state is not a change"
        );
        assert_eq!(
            handle.approve_host_key("SHA256:x"),
            Err(SessionError::NoHostKeyPrompt)
        );

        let presented = host_key();
        let prompt = HostKeyPrompt {
            presented: presented.clone(),
            previously_trusted: vec![host_key()],
        };
        assert_eq!(prompt.verdict(), HostKeyVerdict::Changed);
        driver
            .transition(SessionState::AwaitingHostKey(prompt))
            .unwrap();
        assert_eq!(
            handle.approve_host_key(&host_key().fingerprint()),
            Err(SessionError::HostKeyMismatch)
        );
        handle.approve_host_key(&presented.fingerprint()).unwrap();
        assert_eq!(
            driver.blocking_next_command(),
            Command::ApproveHostKey {
                fingerprint: presented.fingerprint()
            }
        );
        driver.transition(SessionState::Authenticating).unwrap();
        assert_eq!(handle.reject_host_key(), Err(SessionError::NoHostKeyPrompt));
    }

    #[test]
    fn input_requires_connected_and_resize_does_not() {
        let (_, handle, mut driver) = setup(false);
        let key = KeyInput::new(Key::Enter, Modifiers::default()).unwrap();
        assert_eq!(
            handle.send_text("ls".into()),
            Err(SessionError::NotConnected)
        );
        assert_eq!(
            handle.send_key(key.clone()),
            Err(SessionError::NotConnected)
        );
        assert_eq!(
            handle.scroll(ViewportScroll::Top),
            Err(SessionError::NotConnected)
        );
        assert_eq!(handle.request_full_frame(), Err(SessionError::NotConnected));
        assert_eq!(
            handle.submit_text("ls".into()),
            Err(SessionError::NotConnected)
        );
        let size = TerminalSize::new(120, 40).unwrap();
        handle.resize(size).unwrap();
        assert_eq!(driver.blocking_next_command(), Command::Resize(size));

        driver.transition(SessionState::Authenticating).unwrap();
        driver.transition(SessionState::Connected).unwrap();
        handle.send_text(String::new()).unwrap();
        handle.send_text("ls\n".into()).unwrap();
        handle.send_key(key.clone()).unwrap();
        handle.submit_text("go".into()).unwrap();
        handle.submit_text(String::new()).unwrap();
        assert_eq!(driver.blocking_next_command(), Command::Text("ls\n".into()));
        assert_eq!(driver.blocking_next_command(), Command::Key(key));
        assert_eq!(driver.blocking_next_command(), Command::Submit("go".into()));
        assert_eq!(
            driver.blocking_next_command(),
            Command::Submit(String::new())
        );
    }

    #[test]
    fn frames_notify_once_until_taken_even_from_inside_the_callback() {
        let (recorder, handle, mut driver) = setup(false);
        assert_eq!(driver.publish(frame(true)), Err(PublishError::NotConnected));
        driver.transition(SessionState::Authenticating).unwrap();
        driver.transition(SessionState::Connected).unwrap();
        assert_eq!(
            driver.publish(frame(false)),
            Err(PublishError::Frame(FrameError::NoBase))
        );
        driver.publish(frame(true)).unwrap();
        driver.publish(frame(false)).unwrap();
        driver.publish(frame(false)).unwrap();
        assert_eq!(
            events(&recorder),
            ["Authenticating", "Connected", "frame_ready(taken=false)"]
        );
        let taken = handle.take_frame().unwrap();
        assert!(taken.frame.is_full());
        driver.publish(frame(false)).unwrap();
        assert_eq!(events(&recorder).len(), 4);

        // A renderer that takes inside the callback re-arms the notification immediately.
        let (recorder, _handle, mut driver) = setup(true);
        driver.transition(SessionState::Authenticating).unwrap();
        driver.transition(SessionState::Connected).unwrap();
        driver.publish(frame(true)).unwrap();
        driver.publish(frame(false)).unwrap();
        assert_eq!(
            events(&recorder)[2..],
            ["frame_ready(taken=true)", "frame_ready(taken=true)"]
        );
    }

    #[test]
    fn invalid_transitions_are_rejected_without_callbacks() {
        let (recorder, _handle, mut driver) = setup(false);
        driver.transition(SessionState::Authenticating).unwrap();
        assert_eq!(
            driver.transition(SessionState::AwaitingHostKey(HostKeyPrompt {
                presented: host_key(),
                previously_trusted: Vec::new(),
            })),
            Err(TransitionError {
                from: "Authenticating",
                to: "AwaitingHostKey"
            })
        );
        driver.transition(SessionState::Connected).unwrap();
        assert_eq!(
            driver.transition(SessionState::Authenticating),
            Err(TransitionError {
                from: "Connected",
                to: "Authenticating"
            })
        );
        driver.close(CloseReason::Disconnected);
        assert!(driver.transition(SessionState::Authenticating).is_err());
        driver.close(CloseReason::Failed(SessionFailure::TimedOut));
        assert_eq!(events(&recorder), ["Authenticating", "Connected", "Closed"]);
    }

    #[test]
    fn channel_sessions_skip_straight_to_connected() {
        let (recorder, handle, mut driver) = setup(false);
        driver.transition(SessionState::Connected).unwrap();
        handle.send_text("ls\n".into()).unwrap();
        assert_eq!(driver.blocking_next_command(), Command::Text("ls\n".into()));
        driver.publish(frame(true)).unwrap();
        assert_eq!(events(&recorder), ["Connected", "frame_ready(taken=false)"]);

        let (_recorder, _handle, mut driver) = setup(false);
        driver.close(CloseReason::Disconnected);
        assert!(driver.transition(SessionState::Connected).is_err());
    }

    #[test]
    fn closing_releases_the_observer_and_rejects_commands_but_keeps_the_last_frame() {
        let (recorder, handle, mut driver) = setup(true);
        let weak: Weak<Recorder> = Arc::downgrade(&recorder);
        *lock(&recorder.handle) = None;
        drop(recorder);
        driver.transition(SessionState::Authenticating).unwrap();
        driver.transition(SessionState::Connected).unwrap();
        driver.publish(frame(true)).unwrap();
        handle.disconnect();
        assert_eq!(driver.blocking_next_command(), Command::Disconnect);
        driver.close(CloseReason::Disconnected);
        assert!(weak.upgrade().is_none(), "observer released after Closed");
        assert_eq!(
            handle.state(),
            SessionState::Closed(CloseReason::Disconnected)
        );
        assert_eq!(handle.send_text("x".into()), Err(SessionError::Closed));
        assert_eq!(handle.submit_text("x".into()), Err(SessionError::Closed));
        assert_eq!(
            handle.resize(TerminalSize::new(1, 1).unwrap()),
            Err(SessionError::Closed)
        );
        assert_eq!(handle.reject_host_key(), Err(SessionError::Closed));
        handle.disconnect();
        assert!(handle.take_frame().unwrap().frame.is_full());
    }

    #[test]
    fn dropping_all_handles_disconnects_and_dropping_the_driver_closes() {
        let (recorder, handle, mut driver) = setup(false);
        drop(handle);
        assert_eq!(driver.blocking_next_command(), Command::Disconnect);
        drop(driver);
        assert_eq!(events(&recorder), ["Closed"]);

        let (_recorder, handle, driver) = setup(false);
        drop(driver);
        assert_eq!(
            handle.state(),
            SessionState::Closed(CloseReason::Failed(SessionFailure::Internal(
                "session task ended without closing".into()
            )))
        );
    }

    #[test]
    fn connect_request_validates_every_field() {
        let key = ClientKey::generate_ed25519("k").to_stored();
        let trusted = vec![host_key().info().openssh];
        let request = ConnectRequest::new("host", 22, "dev", &key, &trusted, 80, 24).unwrap();
        assert_eq!(request.endpoint.host(), "host");
        assert_eq!(request.endpoint.port(), 22);
        assert_eq!(request.trusted_host_keys.len(), 1);
        assert_eq!(request.size, TerminalSize::new(80, 24).unwrap());
        assert!(!format!("{request:?}").contains("OPENSSH"));

        let err = |host, port, user, key: &[u8], trusted: &[String], c, r| {
            ConnectRequest::new(host, port, user, key, trusted, c, r).unwrap_err()
        };
        assert_eq!(
            err("", 22, "a", &key, &[], 80, 24),
            ConnectError::Endpoint(EndpointError::InvalidHost)
        );
        assert_eq!(
            err("h", 0, "a", &key, &[], 80, 24),
            ConnectError::Endpoint(EndpointError::InvalidPort)
        );
        assert_eq!(
            err("h", 22, "", &key, &[], 80, 24),
            ConnectError::InvalidUsername
        );
        assert_eq!(
            err("h", 22, "a", &key, &[], 0, 24),
            ConnectError::EmptyDimension
        );
        let bad_trust = vec![trusted[0].clone(), "nope".into()];
        assert_eq!(
            err("h", 22, "a", &key, &bad_trust, 80, 24),
            ConnectError::InvalidTrustedHostKey { index: 1 }
        );
        assert_eq!(
            err("h", 22, "a", b"junk", &[], 80, 24),
            ConnectError::InvalidPrivateKey(KeyError::Malformed)
        );
    }
}
