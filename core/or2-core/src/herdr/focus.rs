//! Focusing a pane: one `pane.focus` on a short-lived stream, with the socket taken from the
//! connection's [`Directory`], and a [`FocusGate`] that keeps one connection from asking herdr
//! for the same focus twice at once or twice in a row.
//!
//! The gate exists because the app focuses a pane for the user (a reuse, or an agent tap) and
//! the terminal it then opens on that pane focuses it again as its first step. The two meet in
//! one of two ways, both covered here: the terminal opens while the app's focus is still in
//! flight (it joins that request and shares its answer) or just after it finished (a focus that
//! was acknowledged within [`RECENT`] satisfies a terminal's own, which only wants the pane to
//! be the focused one before it starts). An explicit request from the app is never answered
//! from memory, only joined.
//!
//! A herdr session has one focused pane, and the pane it ends on is the one whose request
//! herdr received **last**. So the gate **serializes the focuses of a session**: each one
//! queues behind the previous one (a per-session async mutex, which is first in, first out) and
//! is sent only once that one has been answered, so the order in which requests reach herdr is
//! the order in which they were asked. What the gate remembers follows from that, per session:
//!
//! - A request joins the **latest queued or running** focus of its session when that one is for
//!   the same pane (the answer is shared). If a focus of another pane was asked since, it is a
//!   request of its own, queued after it (A, B, A sends A, B, A).
//! - An acknowledgement is remembered only as the **last focus completed** in the session, and
//!   only while nothing newer is queued or running there. A failed or cancelled focus (herdr may
//!   or may not have acted on it) makes the session forget it.
//!
//! Sessions do not wait for each other. A focus that is cancelled while it waits or runs leaves
//! the queue (its place in the mutex is released), and followers that were sharing its answer
//! ask for themselves.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;

use super::HerdrError;
use super::discovery::{Directory, DiscoveryError};
use super::generated::request::{PaneTarget, RequestBody};
use super::watch::{PANE_NOT_FOUND, Timing};
use super::wire::{self, WireError};
use crate::remote::RemoteHost;

/// How long an acknowledged focus counts for a terminal that opens on the same pane.
pub const RECENT: Duration = Duration::from_secs(2);

type Session = Option<String>;
type Outcome = Option<Result<(), HerdrError>>;

/// Per host connection. See the module documentation.
#[derive(Default)]
pub struct FocusGate {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    sessions: HashMap<Session, SessionGate>,
    counter: u64,
}

#[derive(Default)]
struct SessionGate {
    /// Sends one focus at a time; tokio's mutex hands the lock out in the order it was asked.
    queue: Arc<tokio::sync::Mutex<()>>,
    /// The latest focus asked for and not yet answered (queued or running).
    latest: Option<Pending>,
    /// The last focus herdr acknowledged, if nothing has happened to the session since.
    done: Option<(String, Instant)>,
}

struct Pending {
    id: u64,
    pane_id: String,
    outcome: watch::Receiver<Outcome>,
}

/// Ends a leader's turn if it is dropped before it finished (a cancelled query): followers then
/// see the sender gone and focus for themselves, and the session forgets its last
/// acknowledgement because herdr may have acted on this request.
struct Leading<'a> {
    gate: &'a FocusGate,
    session: Session,
    id: u64,
    armed: bool,
}

impl Drop for Leading<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.gate.finish(&self.session, self.id, None);
        }
    }
}

enum Role {
    Recent,
    Follow(watch::Receiver<Outcome>),
    Lead(watch::Sender<Outcome>, u64, Arc<tokio::sync::Mutex<()>>),
}

impl FocusGate {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Ends focus `id` of `session`: it is no longer the latest, and the session's acknowledged
    /// pane is `acknowledged` (as of now), none if the focus failed or was cancelled.
    fn finish(&self, session: &Session, id: u64, acknowledged: Option<&str>) {
        let mut state = self.lock();
        let Some(gate) = state.sessions.get_mut(session) else {
            return;
        };
        gate.done = acknowledged.map(|pane| (pane.to_owned(), Instant::now()));
        if gate.latest.as_ref().is_some_and(|latest| latest.id == id) {
            gate.latest = None;
        }
    }

    /// Decides, in one synchronous step, what a request does. A leader takes its place in the
    /// session's queue in the same poll (see [`Self::focus`]), so the queue's order is the order
    /// of these decisions.
    fn decide(&self, session: &Session, pane_id: &str, accept_recent: bool) -> Role {
        let mut state = self.lock();
        state.counter += 1;
        let id = state.counter;
        let gate = state.sessions.entry(session.clone()).or_default();
        if let Some(latest) = &gate.latest {
            if latest.pane_id == pane_id {
                return Role::Follow(latest.outcome.clone());
            }
        } else if let Some((done, at)) = &gate.done
            && accept_recent
            && done == pane_id
            && at.elapsed() < RECENT
        {
            return Role::Recent;
        }
        let (sender, receiver) = watch::channel(None);
        gate.latest = Some(Pending {
            id,
            pane_id: pane_id.to_owned(),
            outcome: receiver,
        });
        // The session's focused pane is about to change: what was acknowledged before no
        // longer says which pane it is.
        gate.done = None;
        Role::Lead(sender, id, Arc::clone(&gate.queue))
    }

    /// Focuses `pane_id` of `session`. A request for the same pane as the session's latest is
    /// joined; with `accept_recent` an acknowledgement younger than [`RECENT`] that is still the
    /// session's last is the answer (a terminal's focus: the app's own request passes `false`
    /// and is never answered from memory). Everything else is one request, with the socket from
    /// `directory`, sent after the session's earlier ones were answered.
    pub async fn focus<H: RemoteHost>(
        &self,
        host: &H,
        herdr: &str,
        directory: &Directory,
        session: Option<&str>,
        pane_id: &str,
        accept_recent: bool,
    ) -> Result<(), HerdrError> {
        let session: Session = session.map(str::to_owned);
        loop {
            match self.decide(&session, pane_id, accept_recent) {
                Role::Recent => return Ok(()),
                Role::Follow(mut receiver) => {
                    let outcome = receiver.wait_for(Option::is_some).await;
                    match outcome {
                        Ok(outcome) => {
                            return (*outcome).clone().expect("waited for an outcome");
                        }
                        // The leader was cancelled before it finished: go again.
                        Err(_) => continue,
                    }
                }
                Role::Lead(sender, id, queue) => {
                    let mut leading = Leading {
                        gate: self,
                        session: session.clone(),
                        id,
                        armed: true,
                    };
                    // No await sits between `decide` and this first poll of the lock, so the
                    // place in line is the place in `decide`'s order.
                    let turn = queue.lock().await;
                    let result =
                        focus_pane_in(host, herdr, directory, session.as_deref(), pane_id).await;
                    leading.armed = false;
                    // The last focus sent to the session: its acknowledgement stands until the
                    // next one is asked for.
                    self.finish(&session, id, result.is_ok().then_some(pane_id));
                    drop(turn);
                    let _ = sender.send(Some(result.clone()));
                    return result;
                }
            }
        }
    }
}

fn discovery_error(error: DiscoveryError) -> HerdrError {
    match error {
        DiscoveryError::Remote(error) => HerdrError::Remote(error),
        other => HerdrError::Failed(other.to_string()),
    }
}

fn wire_error(error: WireError) -> HerdrError {
    match error {
        WireError::Remote(error) => HerdrError::Remote(error),
        WireError::Herdr { code, .. } if code == PANE_NOT_FOUND => HerdrError::PaneNotFound,
        other => HerdrError::Failed(other.to_string()),
    }
}

/// One `pane.focus`, the socket from `directory`. A socket the directory handed out that does
/// not open (the session restarted somewhere else, or stopped) makes it read the listing again
/// and try once more; that is the only time a focus runs `herdr session list`.
pub async fn focus_pane_in<H: RemoteHost>(
    host: &H,
    herdr: &str,
    directory: &Directory,
    session: Option<&str>,
    pane_id: &str,
) -> Result<(), HerdrError> {
    let body = RequestBody::PaneFocus(PaneTarget {
        pane_id: pane_id.to_owned(),
    });
    let mut fresh = false;
    loop {
        let cached = if fresh {
            None
        } else {
            directory.cached_socket(session)
        };
        let from_cache = cached.is_some();
        let socket = match cached {
            Some(socket) => socket,
            None => directory
                .locate_fresh(host, herdr, session)
                .await
                .map_err(discovery_error)?,
        };
        match wire::call(host, &socket, "or2_focus", &body, Timing::default().request).await {
            Ok(_) => return Ok(()),
            Err(WireError::Unreachable(_)) if from_cache => {
                directory.invalidate();
                fresh = true;
            }
            Err(error) => return Err(wire_error(error)),
        }
    }
}
