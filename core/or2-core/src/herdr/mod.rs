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
//! **Not integrated yet.** [`run`] and [`focus_pane`] fail honestly until the herdr client
//! (generated wire types, discovery, subscribe/snapshot reconciliation) replaces them.

pub mod view;

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::mpsc;

use crate::remote::{RemoteError, RemoteHost};

pub use view::{Agent, AgentStatus, HerdrView, Pane, Tab, Workspace};

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
    #[error("the herdr client is not integrated yet")]
    NotIntegrated,
    #[error(transparent)]
    Remote(#[from] RemoteError),
    /// herdr answered with an error.
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
}

impl Drop for HerdrWatchDriver {
    fn drop(&mut self) {
        self.close();
    }
}

/// Starts a watch of `session` (`None` is herdr's default session) on the process-wide
/// runtime. The host connection driver instead creates the [`channel`] itself and calls
/// [`run`], so the handle can be returned synchronously.
pub fn watch<H: RemoteHost>(
    host: Arc<H>,
    session: Option<String>,
    observer: Arc<dyn HerdrObserver>,
) -> HerdrWatchHandle {
    let (handle, driver) = channel(observer);
    crate::ssh::runtime().spawn(run(host, session, driver));
    handle
}

/// Drives `driver` until it is stopped, then closes it.
///
/// Not integrated: reports `Unavailable { Failed }` and waits for the stop. Never a pretend
/// `Live`.
pub async fn run<H: RemoteHost>(
    host: Arc<H>,
    session: Option<String>,
    mut driver: HerdrWatchDriver,
) {
    let _ = (host, session);
    let _ = driver.transition(HerdrState::Unavailable {
        reason: HerdrUnavailable::Failed,
        message: "herdr client not integrated".into(),
    });
    driver.stopped().await;
    driver.close();
}

/// Focuses `pane_id` in `session` with one `pane.focus` request. It changes what the user's
/// herdr clients show.
///
/// Not integrated: always fails with [`HerdrError::NotIntegrated`].
pub async fn focus_pane<H: RemoteHost>(
    host: &H,
    session: Option<&str>,
    pane_id: &str,
) -> Result<(), HerdrError> {
    let _ = (host, session, pane_id);
    Err(HerdrError::NotIntegrated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::LocalHost;

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

    #[tokio::test]
    async fn the_unintegrated_watch_fails_honestly_and_closes_on_stop() {
        let recorder = Arc::new(Recorder::default());
        let handle = watch(Arc::new(LocalHost::new()), None, recorder.clone());
        for _ in 0..200 {
            if handle.state() != HerdrState::Starting {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(
            handle.state(),
            HerdrState::Unavailable {
                reason: HerdrUnavailable::Failed,
                message: "herdr client not integrated".into()
            }
        );
        handle.stop();
        for _ in 0..200 {
            if handle.state() == HerdrState::Closed {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let events = lock(&recorder.0).clone();
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(events[1], HerdrState::Closed);
    }

    #[tokio::test]
    async fn focus_is_not_integrated() {
        assert_eq!(
            focus_pane(&LocalHost::new(), Some("work"), "p1").await,
            Err(HerdrError::NotIntegrated)
        );
    }
}
