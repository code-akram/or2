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
//! A herdr session has one focused pane, so what the gate remembers is **per session**: the
//! moment a focus of another pane starts, every older acknowledgement of that session stops
//! counting (each focus takes a generation number; an acknowledgement is remembered only if no
//! later focus of the session started meanwhile). Otherwise a terminal on A would accept A's
//! old acknowledgement after B was focused and show B.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
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

type Key = (Option<String>, String);
type Outcome = Option<Result<(), HerdrError>>;

enum Slot {
    /// A focus is on its way; followers wait for its outcome.
    InFlight(watch::Receiver<Outcome>),
    /// Acknowledged at this moment.
    Done(Instant),
}

/// Per host connection. See the module documentation.
#[derive(Default)]
pub struct FocusGate {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    slots: HashMap<Key, Slot>,
    /// The generation of the latest focus started in each herdr session.
    latest: HashMap<Option<String>, u64>,
    counter: u64,
}

/// Removes a leader's in-flight slot if it is dropped before it finished (a cancelled query):
/// followers then see the sender gone and focus for themselves.
struct Leading<'a> {
    gate: &'a FocusGate,
    key: Key,
    armed: bool,
}

impl Drop for Leading<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.gate.lock().slots.remove(&self.key);
        }
    }
}

impl FocusGate {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Focuses `pane_id` of `session`. A request for the same pane that is already in flight is
    /// joined; with `accept_recent` an acknowledgement younger than [`RECENT`] is the answer
    /// (a terminal's focus: the app's own request passes `false` and is never answered from
    /// memory). Everything else is one request, with the socket from `directory`.
    pub async fn focus<H: RemoteHost>(
        &self,
        host: &H,
        herdr: &str,
        directory: &Directory,
        session: Option<&str>,
        pane_id: &str,
        accept_recent: bool,
    ) -> Result<(), HerdrError> {
        let key: Key = (session.map(str::to_owned), pane_id.to_owned());
        loop {
            enum Role {
                Recent,
                Follow(watch::Receiver<Outcome>),
                Lead(watch::Sender<Outcome>, u64),
            }
            let role = {
                let mut state = self.lock();
                match state.slots.get(&key) {
                    Some(Slot::Done(at)) if accept_recent && at.elapsed() < RECENT => Role::Recent,
                    Some(Slot::InFlight(receiver)) => Role::Follow(receiver.clone()),
                    _ => {
                        let (sender, receiver) = watch::channel(None);
                        // A new focus makes every older one of this herdr session history:
                        // the session's focused pane is about to change.
                        state.counter += 1;
                        let generation = state.counter;
                        state.latest.insert(key.0.clone(), generation);
                        state.slots.retain(|other, slot| {
                            other.0 != key.0 || matches!(slot, Slot::InFlight(_))
                        });
                        state.slots.insert(key.clone(), Slot::InFlight(receiver));
                        Role::Lead(sender, generation)
                    }
                }
            };
            match role {
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
                Role::Lead(sender, generation) => {
                    let mut leading = Leading {
                        gate: self,
                        key: key.clone(),
                        armed: true,
                    };
                    let result = focus_pane_in(host, herdr, directory, session, pane_id).await;
                    leading.armed = false;
                    {
                        let mut state = self.lock();
                        // Remembered only while no later focus of the session has started: that
                        // one decides which pane is focused, and this answer is history.
                        let current = state.latest.get(&key.0) == Some(&generation);
                        match &result {
                            Ok(()) if current => {
                                state.slots.insert(key.clone(), Slot::Done(Instant::now()))
                            }
                            _ => state.slots.remove(&key),
                        };
                    }
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
