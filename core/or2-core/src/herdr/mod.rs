//! herdr integration: a watch that projects a session into a [`HerdrView`], and pane focus.
//!
//! The watch has the same handle/driver split as `session`: [`HerdrWatchHandle`] is Kotlin's
//! side and never blocks; [`HerdrWatchDriver`] belongs to the task that talks to herdr and
//! calls the [`HerdrObserver`] in order without holding a lock.
//!
//! ```text
//! Starting ──▶ Live ⇄ Unavailable ──▶ Closed (terminal, once)
//! ```
//!
//! The client behind it: [`generated`] holds herdr's wire types (generated from
//! `herdr api schema --json`, never edited), [`wire`] the newline-delimited JSON layer,
//! `discovery` lists sessions and finds a session's socket with `session list --json`
//! ([`list_sessions`] is public, for the capability probe), `project` turns a
//! `session.snapshot` into the [`view`], and `watch` keeps it current from subscribe, snapshot
//! and invalidating events. Everything reaches herdr through [`RemoteHost`].

pub mod generated;
pub mod view;
pub mod wire;

mod discovery;
mod focus;
mod navigate;
mod project;
mod scroll;
#[cfg(test)]
mod testing;
mod watch;
#[cfg(test)]
mod watch_tests;

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::mpsc;

use crate::remote::{RemoteError, RemoteHost};

pub(crate) use discovery::parse_listing;
pub use discovery::{Directory, DiscoveryError, SessionEntry, list_sessions};
pub use focus::{FocusGate, focus_pane_in};
pub use navigate::navigate_in;
pub use scroll::{ScrollOffsets, next_offset, scroll_pane_in};
pub use view::{Agent, AgentStatus, HerdrView, Pane, Tab, Workspace};
/// The watch's intervals, for integration tests that cannot wait for the production ones.
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use watch::Timing;

/// Why there is no live view. `NotInstalled` and `IncompatibleProtocol` are final;
/// `NotRunning` and `Failed` are retried while the host is connected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HerdrUnavailable {
    NotInstalled,
    IncompatibleProtocol { protocol: u32 },
    NotRunning,
    Failed,
}

impl HerdrUnavailable {
    pub fn is_final(&self) -> bool {
        matches!(self, Self::NotInstalled | Self::IncompatibleProtocol { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HerdrState {
    /// The initial state; not delivered as a change.
    Starting,
    Live {
        view: HerdrView,
    },
    /// `message` is a diagnostic without secrets, not for matching.
    Unavailable {
        reason: HerdrUnavailable,
        message: String,
    },
    Closed,
}

impl HerdrState {
    fn name(&self) -> &'static str {
        match self {
            Self::Starting => "Starting",
            Self::Live { .. } => "Live",
            Self::Unavailable { .. } => "Unavailable",
            Self::Closed => "Closed",
        }
    }

    fn can_become(&self, next: &HerdrState) -> bool {
        match (self, next) {
            (_, Self::Starting) | (Self::Closed, _) => false,
            (Self::Unavailable { reason, .. }, next) if reason.is_final() => {
                matches!(next, Self::Closed)
            }
            _ => true,
        }
    }
}

/// Receives watch changes on the driver's thread. Implementations must return quickly.
pub trait HerdrObserver: Send + Sync {
    fn state_changed(&self, state: &HerdrState);
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HerdrError {
    #[error(transparent)]
    Remote(#[from] RemoteError),
    /// herdr answered `pane_not_found`: the pane named in a request no longer exists (an agent
    /// pane that was closed since the caller last saw it).
    #[error("the herdr pane no longer exists")]
    PaneNotFound,
    /// herdr is missing, its session is not running, or it answered with an error.
    #[error("herdr failed: {0}")]
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid herdr watch transition {from} -> {to}")]
pub struct TransitionError {
    pub from: &'static str,
    pub to: &'static str,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn channel(observer: Arc<dyn HerdrObserver>) -> (HerdrWatchHandle, HerdrWatchDriver) {
    let state = Arc::new(Mutex::new(HerdrState::Starting));
    let (sender, receiver) = mpsc::unbounded_channel();
    (
        HerdrWatchHandle {
            state: Arc::clone(&state),
            stop: sender,
        },
        HerdrWatchDriver {
            state,
            stop: receiver,
            observer: Some(observer),
        },
    )
}

/// Kotlin's side of a watch. Non-blocking, callable from any thread. Dropping the last handle
/// stops the watch.
pub struct HerdrWatchHandle {
    state: Arc<Mutex<HerdrState>>,
    stop: mpsc::UnboundedSender<()>,
}

impl HerdrWatchHandle {
    pub fn state(&self) -> HerdrState {
        lock(&self.state).clone()
    }

    /// Idempotent. `Closed` arrives through the observer.
    pub fn stop(&self) {
        let _ = self.stop.send(());
    }
}

/// The watch task's side. Dropping it without closing delivers `Closed`, so the observer
/// always receives exactly one.
pub struct HerdrWatchDriver {
    state: Arc<Mutex<HerdrState>>,
    stop: mpsc::UnboundedReceiver<()>,
    observer: Option<Arc<dyn HerdrObserver>>,
}

impl HerdrWatchDriver {
    /// Resolves once `stop()` was called or every handle was dropped. Cancel-safe.
    pub async fn stopped(&mut self) {
        let _ = self.stop.recv().await;
    }

    pub fn state(&self) -> HerdrState {
        lock(&self.state).clone()
    }

    /// Moves to `next` and notifies the observer. A state equal to the current one is not
    /// redelivered. On `Closed` the observer is released afterwards.
    pub fn transition(&mut self, next: HerdrState) -> Result<(), TransitionError> {
        {
            let mut state = lock(&self.state);
            if *state == next {
                return Ok(());
            }
            if !state.can_become(&next) {
                return Err(TransitionError {
                    from: state.name(),
                    to: next.name(),
                });
            }
            *state = next.clone();
        }
        let observer = if next == HerdrState::Closed {
            self.stop.close();
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
    pub fn close(&mut self) {
        let _ = self.transition(HerdrState::Closed);
    }

    /// Releases a driver whose handle was never returned to anyone: closes silently, without
    /// calling the observer.
    pub fn discard(mut self) {
        *lock(&self.state) = HerdrState::Closed;
        self.stop.close();
        self.observer = None;
    }
}

impl Drop for HerdrWatchDriver {
    fn drop(&mut self) {
        self.close();
    }
}

/// Starts a watch of `session` (`None` is herdr's default session) on the process-wide
/// runtime. `herdr` is the absolute path from the capability probe; a caller whose probe
/// found no herdr reports `Unavailable { NotInstalled }` itself instead of calling this. The
/// host connection driver creates the [`channel`] itself and calls [`run`], so the handle can
/// be returned synchronously.
pub fn watch<H: RemoteHost>(
    host: Arc<H>,
    herdr: String,
    session: Option<String>,
    observer: Arc<dyn HerdrObserver>,
) -> HerdrWatchHandle {
    let (handle, driver) = channel(observer);
    crate::ssh::runtime().spawn(run(host, herdr, session, driver));
    handle
}

/// [`watch`] with the intervals of `timing` instead of the contract's, for integration tests
/// (feature `test-support`).
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn watch_with_timing<H: RemoteHost>(
    host: Arc<H>,
    herdr: String,
    session: Option<String>,
    observer: Arc<dyn HerdrObserver>,
    timing: Timing,
) -> HerdrWatchHandle {
    let (handle, driver) = channel(observer);
    crate::ssh::runtime().spawn(watch::run(host, herdr, session, driver, timing));
    handle
}

/// Drives `driver` until it is stopped or the host closes, then closes it. The host driver
/// that owns this task ends it on host close by aborting or dropping it (the driver's `Drop`
/// delivers `Closed`): a watch learns of a lost host only from a failing call, and one parked
/// at a final `Unavailable` makes none. `herdr` is the
/// absolute path from the capability probe. The session's socket is not an input: the client
/// finds it with `<herdr> session list --json` (`socket_path`), so it never leaves the herdr
/// module. See [`watch`](self::watch) for the protocol: subscribe, snapshot, invalidating
/// events, `events_lost` recovery, and the retry rules for each [`HerdrUnavailable`].
pub async fn run<H: RemoteHost>(
    host: Arc<H>,
    herdr: String,
    session: Option<String>,
    driver: HerdrWatchDriver,
) {
    watch::run(host, herdr, session, driver, watch::Timing::default()).await;
}

/// [`run`] with the connection's own [`Directory`]: the watch takes its first socket from it
/// (the probe's listing, so a watch costs no `session list`) and re-discovers only after a
/// failure.
pub async fn run_in<H: RemoteHost>(
    host: Arc<H>,
    herdr: String,
    directory: Arc<Directory>,
    session: Option<String>,
    driver: HerdrWatchDriver,
) {
    watch::run_in(
        host,
        herdr,
        directory,
        session,
        driver,
        watch::Timing::default(),
    )
    .await;
}

/// Focuses `pane_id` in `session` with one `pane.focus` request on a short-lived stream.
/// `herdr` is the absolute path from the capability probe. It changes what the user's herdr
/// clients show. A pane that no longer exists is [`HerdrError::PaneNotFound`]. Reads the
/// session listing for the socket every time; a host connection uses [`focus_pane_in`] with
/// its [`Directory`] instead.
pub async fn focus_pane<H: RemoteHost>(
    host: &H,
    herdr: &str,
    session: Option<&str>,
    pane_id: &str,
) -> Result<(), HerdrError> {
    focus_pane_in(host, herdr, &Directory::new(), session, pane_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Recorder(Mutex<Vec<HerdrState>>);

    impl HerdrObserver for Recorder {
        fn state_changed(&self, state: &HerdrState) {
            lock(&self.0).push(state.clone());
        }
    }

    fn view(version: u64) -> HerdrView {
        HerdrView {
            version,
            protocol: 22,
            focused_pane_id: None,
            workspaces: Vec::new(),
            tabs: Vec::new(),
            panes: Vec::new(),
            agents: Vec::new(),
        }
    }

    fn unavailable(reason: HerdrUnavailable) -> HerdrState {
        HerdrState::Unavailable {
            reason,
            message: "m".into(),
        }
    }

    #[test]
    fn states_flow_live_updates_and_unavailable_with_dedup() {
        let recorder = Arc::new(Recorder::default());
        let (handle, mut driver) = channel(recorder.clone());
        assert_eq!(handle.state(), HerdrState::Starting);
        assert!(lock(&recorder.0).is_empty(), "Starting is not delivered");

        driver
            .transition(unavailable(HerdrUnavailable::NotRunning))
            .unwrap();
        driver
            .transition(unavailable(HerdrUnavailable::NotRunning))
            .unwrap();
        driver
            .transition(HerdrState::Live { view: view(1) })
            .unwrap();
        driver
            .transition(HerdrState::Live { view: view(1) })
            .unwrap();
        driver
            .transition(HerdrState::Live { view: view(2) })
            .unwrap();
        assert_eq!(handle.state(), HerdrState::Live { view: view(2) });
        assert!(driver.transition(HerdrState::Starting).is_err());
        driver
            .transition(unavailable(HerdrUnavailable::Failed))
            .unwrap();
        assert_eq!(
            lock(&recorder.0).len(),
            4,
            "equal states are not redelivered"
        );
    }

    #[test]
    fn final_unavailable_reasons_only_allow_closed() {
        for reason in [
            HerdrUnavailable::NotInstalled,
            HerdrUnavailable::IncompatibleProtocol { protocol: 99 },
        ] {
            let (_handle, mut driver) = channel(Arc::new(Recorder::default()));
            driver.transition(unavailable(reason.clone())).unwrap();
            assert_eq!(
                driver.transition(HerdrState::Live { view: view(1) }),
                Err(TransitionError {
                    from: "Unavailable",
                    to: "Live"
                })
            );
            assert!(
                driver
                    .transition(unavailable(HerdrUnavailable::NotRunning))
                    .is_err()
            );
            driver.close();
            assert_eq!(driver.state(), HerdrState::Closed);
        }
    }

    #[test]
    fn closed_is_delivered_once_last_and_releases_the_observer() {
        let recorder = Arc::new(Recorder::default());
        let weak = Arc::downgrade(&recorder);
        let (handle, mut driver) = channel(recorder.clone());
        driver
            .transition(HerdrState::Live { view: view(1) })
            .unwrap();
        handle.stop();
        handle.stop();
        driver.close();
        driver.close();
        assert!(
            driver
                .transition(HerdrState::Live { view: view(2) })
                .is_err()
        );
        assert_eq!(
            *lock(&recorder.0),
            [HerdrState::Live { view: view(1) }, HerdrState::Closed]
        );
        assert_eq!(handle.state(), HerdrState::Closed);
        // The driver's reference is gone; only the test's remains.
        assert_eq!(weak.strong_count(), 1);
        drop(recorder);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn dropping_the_driver_closes_and_dropping_handles_stops() {
        let recorder = Arc::new(Recorder::default());
        let (handle, driver) = channel(recorder.clone());
        drop(driver);
        assert_eq!(*lock(&recorder.0), [HerdrState::Closed]);
        assert_eq!(handle.state(), HerdrState::Closed);

        let (handle, mut driver) = channel(Arc::new(Recorder::default()));
        drop(handle);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(driver.stopped());
    }
}
