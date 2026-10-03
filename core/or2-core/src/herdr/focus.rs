//! Focusing a pane: one `pane.focus` on a short-lived stream, with the socket taken from the
//! connection's [`Directory`], and a [`FocusGate`] that keeps one connection from asking herdr
//! for the same focus twice at once or twice in a row.
//!
//! The gate exists because the app focuses a pane for the user (a reuse, or an agent tap) and
//! the terminal it then opens on that pane focuses it again as its first step. The two meet in
//! one of two ways, both covered here: the terminal opens while the app's focus is still
//! pending (it joins that request and shares its answer) or just after it finished (a focus
//! that was acknowledged within [`RECENT`] satisfies a terminal's own, which only wants the
//! pane to be the focused one before it starts). An explicit request from the app is never
//! answered from memory, only joined.
//!
//! A herdr session has one focused pane, and the pane it ends on is the one whose request
//! herdr received **last**. So every decision about a session's focuses is made by **one
//! actor task per session**, which owns all of that session's state and is the only thing that
//! sends to herdr; callers hand it `(pane, reply)` over a channel and wait for the reply. It is
//! spawned by the first request and ends when it is idle, with nothing remembered that could
//! still be used. It owns:
//!
//! - a FIFO of **pending requests** (a pane and the list of callers waiting for it), and the
//!   one request **in flight**;
//! - `recent`: the pane herdr was last successfully focused to by this gate, valid only while
//!   the FIFO is empty and nothing is in flight.
//!
//! Requests are taken from the channel in the order they were sent, which is the order they
//! were asked in:
//!
//! - one for the pane of the FIFO's tail (or of the request in flight, when the FIFO is empty)
//!   **joins** it and shares its answer; for another pane it is queued behind it (A, B, A is
//!   three requests);
//! - with the FIFO empty and nothing in flight, `recent` equal to the pane and younger than
//!   [`RECENT`], a terminal's request (`accept_recent`) is answered from memory;
//! - anything else is queued and clears `recent`, since herdr's focused pane is about to
//!   change.
//!
//! The actor sends the FIFO's requests strictly one at a time, in order. On success `recent`
//! becomes the pane only if the FIFO is empty by then; on failure it is cleared (herdr may or
//! may not have acted).
//!
//! A caller that gives up only drops its reply channel; it never touches the actor's state. A
//! queued request whose callers have all gone is skipped (and no longer counts as pending): it
//! is never sent. A request already in flight completes, and its result counts like any other.
//! Sessions have their own actors and do not wait for each other.

use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep_until};

use super::HerdrError;
use super::discovery::{Directory, DiscoveryError};
use super::generated::request::{PaneTarget, RequestBody};
use super::watch::PANE_NOT_FOUND;
use super::wire::WireError;
use crate::remote::RemoteHost;

/// How long an acknowledged focus counts for a terminal that opens on the same pane.
pub const RECENT: Duration = Duration::from_secs(2);

type Session = Option<String>;
type Reply = oneshot::Sender<Result<(), HerdrError>>;
/// The focus request, not yet polled: it holds the host and directory it will use.
type Job = Pin<Box<dyn Future<Output = Result<(), HerdrError>> + Send>>;
type Mailboxes = Arc<Mutex<HashMap<Session, mpsc::UnboundedSender<Request>>>>;
/// A test's view of a recent-acknowledgement answer: the pane and how many requests were
/// pending (queued or in flight) at that moment.
#[cfg(test)]
pub(super) type HitHook = Arc<dyn Fn(&str, usize) + Send + Sync>;

/// Per host connection. See the module documentation.
#[derive(Default)]
pub struct FocusGate {
    /// The mailbox of each session's running actor. Only mailboxes: a sender is used under
    /// this lock and an actor leaves under it, so a request is never sent to an actor that
    /// has decided to end.
    actors: Mailboxes,
    #[cfg(test)]
    pub(super) hit_hook: Mutex<Option<HitHook>>,
}

struct Request {
    pane_id: String,
    accept_recent: bool,
    reply: Reply,
    job: Job,
}

/// A pending request: the pane, those waiting for it and the request to send.
struct Queued {
    pane_id: String,
    waiters: Vec<Reply>,
    job: Job,
}

impl Queued {
    fn is_live(&self) -> bool {
        self.waiters.iter().any(|waiter| !waiter.is_closed())
    }
}

impl FocusGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of sessions with a running actor.
    #[cfg(test)]
    pub(super) fn actors_running(&self) -> usize {
        self.actors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Focuses `pane_id` of `session`. A request for the same pane as the session's latest
    /// pending one is joined; with `accept_recent` an acknowledgement younger than [`RECENT`]
    /// that is still the session's last is the answer (a terminal's focus: the app's own
    /// request passes `false` and is never answered from memory). Everything else is one
    /// request, with the socket from `directory`, sent after the session's earlier ones were
    /// answered. Dropping the returned future abandons the wait, not a request already sent.
    pub async fn focus<H: RemoteHost>(
        &self,
        host: &Arc<H>,
        herdr: &str,
        directory: &Arc<Directory>,
        session: Option<&str>,
        pane_id: &str,
        accept_recent: bool,
    ) -> Result<(), HerdrError> {
        let (reply, answer) = oneshot::channel();
        let job: Job = {
            let (host, directory) = (Arc::clone(host), Arc::clone(directory));
            let (herdr, session, pane_id) = (
                herdr.to_owned(),
                session.map(str::to_owned),
                pane_id.to_owned(),
            );
            Box::pin(async move {
                focus_pane_in(&*host, &herdr, &directory, session.as_deref(), &pane_id).await
            })
        };
        self.submit(
            session.map(str::to_owned),
            Request {
                pane_id: pane_id.to_owned(),
                accept_recent,
                reply,
                job,
            },
        );
        answer
            .await
            .unwrap_or_else(|_| Err(HerdrError::Failed("the focus was not answered".into())))
    }

    /// Puts `request` in its session's mailbox (synchronously, so mailbox order is the order
    /// of asking), starting the session's actor if none is running.
    fn submit(&self, session: Session, mut request: Request) {
        let mut actors = self.actors.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(mailbox) = actors.get(&session) {
            match mailbox.send(request) {
                Ok(()) => return,
                // An actor that died (it panicked): replace it.
                Err(mpsc::error::SendError(returned)) => request = returned,
            }
        }
        let (mailbox, requests) = mpsc::unbounded_channel();
        mailbox
            .send(request)
            .unwrap_or_else(|_| unreachable!("the receiver is held here"));
        actors.insert(session.clone(), mailbox);
        let actor = Actor {
            fifo: VecDeque::new(),
            in_flight: None,
            recent: None,
            #[cfg(test)]
            hit_hook: self
                .hit_hook
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        };
        tokio::spawn(actor.run(session, requests, Arc::clone(&self.actors)));
    }
}

/// One session's state. Touched only by the task that runs [`Actor::run`].
struct Actor {
    fifo: VecDeque<Queued>,
    in_flight: Option<(String, Vec<Reply>)>,
    /// The pane herdr was last successfully focused to by this gate, and when. `None` whenever
    /// anything is pending or in flight, or the last focus failed.
    recent: Option<(String, Instant)>,
    #[cfg(test)]
    hit_hook: Option<HitHook>,
}

impl Actor {
    async fn run(
        mut self,
        session: Session,
        mut requests: mpsc::UnboundedReceiver<Request>,
        actors: Mailboxes,
    ) {
        loop {
            while let Ok(request) = requests.try_recv() {
                self.admit(request);
            }
            self.fifo.retain(Queued::is_live);
            if let Some(queued) = self.fifo.pop_front() {
                self.send(queued, &mut requests).await;
                continue;
            }
            // Idle. A remembered acknowledgement keeps the actor for as long as it can be used.
            if let Some((_, at)) = &self.recent {
                let until = *at + RECENT;
                if Instant::now() >= until {
                    self.recent = None;
                    continue;
                }
                tokio::select! {
                    request = requests.recv() => match request {
                        Some(request) => self.admit(request),
                        None => return,
                    },
                    () = sleep_until(until) => self.recent = None,
                }
                continue;
            }
            // Nothing to do and nothing to remember: leave, unless a request was sent just now
            // (requests are sent under this lock, so none can follow the check).
            let mut mailboxes = actors.lock().unwrap_or_else(PoisonError::into_inner);
            match requests.try_recv() {
                Ok(request) => {
                    drop(mailboxes);
                    self.admit(request);
                }
                Err(_) => {
                    mailboxes.remove(&session);
                    return;
                }
            }
        }
    }

    /// Takes in one request: joins the tail, is answered from memory, or is queued.
    fn admit(&mut self, request: Request) {
        // A request nobody waits for any more is not a request.
        self.fifo.retain(Queued::is_live);
        let Request {
            pane_id,
            accept_recent,
            reply,
            job,
        } = request;
        if let Some(tail) = self.fifo.back_mut() {
            if tail.pane_id == pane_id {
                tail.waiters.push(reply);
                return;
            }
        } else if let Some((in_flight, waiters)) = &mut self.in_flight {
            if *in_flight == pane_id {
                waiters.push(reply);
                return;
            }
        } else if accept_recent
            && let Some((recent, at)) = &self.recent
            && *recent == pane_id
            && at.elapsed() < RECENT
        {
            #[cfg(test)]
            if let Some(hook) = &self.hit_hook {
                hook(
                    &pane_id,
                    self.fifo.len() + usize::from(self.in_flight.is_some()),
                );
            }
            let _ = reply.send(Ok(()));
            return;
        }
        // The session's focused pane is about to change: what was acknowledged before no
        // longer says which pane it is.
        self.recent = None;
        self.fifo.push_back(Queued {
            pane_id,
            waiters: vec![reply],
            job,
        });
    }

    /// Sends `queued` and waits for herdr's answer, taking in the requests that arrive meanwhile.
    async fn send(&mut self, queued: Queued, requests: &mut mpsc::UnboundedReceiver<Request>) {
        let Queued {
            pane_id,
            waiters,
            mut job,
        } = queued;
        self.recent = None;
        self.in_flight = Some((pane_id.clone(), waiters));
        let result = loop {
            tokio::select! {
                result = &mut job => break result,
                Some(request) = requests.recv() => self.admit(request),
            }
        };
        // Whatever was asked before this answer is in the FIFO (or a waiter) before `recent`
        // is decided.
        while let Ok(request) = requests.try_recv() {
            self.admit(request);
        }
        self.fifo.retain(Queued::is_live);
        let (_, waiters) = self.in_flight.take().expect("set above");
        self.recent = match &result {
            Ok(()) if self.fifo.is_empty() => Some((pane_id, Instant::now())),
            _ => None,
        };
        for waiter in waiters {
            let _ = waiter.send(result.clone());
        }
    }
}

pub(super) fn discovery_error(error: DiscoveryError) -> HerdrError {
    match error {
        DiscoveryError::Remote(error) => HerdrError::Remote(error),
        other => HerdrError::Failed(other.to_string()),
    }
}

pub(super) fn wire_error(error: WireError) -> HerdrError {
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
    super::scroll::call(host, herdr, directory, session, "or2_focus", &body)
        .await
        .map(|_| ())
}
