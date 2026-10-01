//! The watch task behind [`super::run`]: keeps a [`HerdrView`] of one herdr session current.
//!
//! One attempt is: find the socket (`session list --json`), open the long-lived event stream
//! and subscribe, then reconcile. herdr gives snapshots and events no shared sequence number,
//! so events are *invalidations*, never patches: the snapshot is read on a separate short-lived
//! stream, installed, and read again while another event arrived during the read.
//!
//! ```text
//! attempt ──▶ locate ──▶ subscribe ──▶ [snapshot ──▶ install ──▶ (resubscribe)]* ──▶ idle ─┐
//!    ▲                                      ▲   events during a read repeat the loop         │
//!    │                                      └──────────────────────────────────────── event ◀┘
//!    └── Resubscribe (events_lost, dropped stream) after a short pause
//!    └── Unavailable: NotRunning/Failed retry after 10 s; NotInstalled/IncompatibleProtocol final
//! ```
//!
//! Subscriptions: the lifecycle events that change what [`HerdrView`] projects (workspaces,
//! tabs, panes, focus, agent detection) plus `pane.agent_status_changed`, which herdr only
//! accepts per pane. A subscription cannot grow, and herdr rejects a whole request that names
//! a pane that no longer exists, so when panes appear the stream is replaced by one that
//! covers every current pane, and the view is read again to close the gap.
//!
//! Deliveries are coalesced to at most one per [`Timing::coalesce`]; a view equal to the last
//! delivered one is not delivered at all.

use std::collections::BTreeSet;
use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::{Instant, sleep, sleep_until};

use super::discovery::{self, DiscoveryError};
use super::generated::request::{EmptyParams, EventsSubscribeParams, RequestBody, Subscription};
use super::project::{self, SnapshotError};
use super::view::HerdrView;
use super::wire::{self, LineReader, Message, WireError};
use super::{HerdrState, HerdrUnavailable, HerdrWatchDriver};
use crate::remote::{RemoteError, RemoteHost};

/// Intervals of the watch. [`Timing::default`] is the contract; tests shorten them.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// At most one view delivery per interval; also the least time between snapshot reads.
    pub coalesce: Duration,
    /// Pause before retrying after `NotRunning` or `Failed`.
    pub retry: Duration,
    /// Pause before resubscribing after `events_lost` or a dropped event stream.
    pub resubscribe: Duration,
    /// Bound on one request (open, send, response) and on the subscription acknowledgement.
    pub request: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            coalesce: Duration::from_millis(100),
            retry: Duration::from_secs(10),
            resubscribe: Duration::from_millis(500),
            request: Duration::from_secs(10),
        }
    }
}

/// How many consecutive times herdr may reject a subscription (a pane vanished before the
/// request arrived) before the attempt fails. Each rejection re-reads the panes first.
const MAX_REJECTED_SUBSCRIPTIONS: u32 = 3;

/// herdr's code for a request that names a pane that does not exist (any more).
const PANE_NOT_FOUND: &str = "pane_not_found";

/// The `protocol` reported for a herdr that predates `session list --json`: older than any
/// protocol number, which are positive.
const PROTOCOL_UNKNOWN: u32 = 0;

/// The lifecycle events behind [`HerdrView`]. Left out: `workspace.metadata_updated` (tokens),
/// `worktree.*`, `layout.updated`, `pane.output_matched` and `pane.scroll_changed` (none is
/// projected).
pub(super) fn lifecycle() -> Vec<Subscription> {
    vec![
        Subscription::WorkspaceCreated,
        Subscription::WorkspaceUpdated,
        Subscription::WorkspaceRenamed,
        Subscription::WorkspaceMoved,
        Subscription::WorkspaceReordered,
        Subscription::WorkspaceClosed,
        Subscription::WorkspaceFocused,
        Subscription::TabCreated,
        Subscription::TabClosed,
        Subscription::TabFocused,
        Subscription::TabRenamed,
        Subscription::TabMoved,
        Subscription::PaneCreated,
        Subscription::PaneClosed,
        Subscription::PaneUpdated,
        Subscription::PaneFocused,
        Subscription::PaneMoved,
        Subscription::PaneExited,
        Subscription::PaneAgentDetected,
    ]
}

type PaneKeys = BTreeSet<(String, String)>;

/// Why an attempt ended.
#[derive(Debug)]
enum Exit {
    Stopped,
    HostClosed,
    /// `events_lost` or a dropped event stream: reconnect and read again.
    Resubscribe,
    Unavailable {
        reason: HerdrUnavailable,
        message: String,
    },
}

impl Exit {
    fn failed(message: impl Into<String>) -> Self {
        Self::Unavailable {
            reason: HerdrUnavailable::Failed,
            message: message.into(),
        }
    }
}

fn exit_for_wire(error: WireError) -> Exit {
    match error {
        WireError::Remote(RemoteError::Closed) => Exit::HostClosed,
        // Every call of an attempt follows a discovery that found the session running, so a
        // socket that cannot be opened is not "not running". OpenSSH answers a streamlocal open
        // that policy forbids with the same failure as a dead socket, and this is the likelier
        // cause: say so, and keep retrying (`Failed`).
        WireError::Unreachable(detail) => Exit::failed(format!(
            "herdr lists this session as running, but its socket cannot be reached over SSH \
             ({detail}); if the host forbids streamlocal forwarding, check \
             AllowStreamLocalForwarding and DisableForwarding in its sshd_config"
        )),
        other => Exit::failed(other.to_string()),
    }
}

fn exit_for_discovery(error: DiscoveryError) -> Exit {
    match error {
        DiscoveryError::Remote(RemoteError::Closed) => Exit::HostClosed,
        DiscoveryError::Remote(other) => Exit::failed(other.to_string()),
        DiscoveryError::NotInstalled => Exit::Unavailable {
            reason: HerdrUnavailable::NotInstalled,
            message: "herdr is not installed on the host".into(),
        },
        DiscoveryError::Unsupported => Exit::Unavailable {
            reason: HerdrUnavailable::IncompatibleProtocol {
                protocol: PROTOCOL_UNKNOWN,
            },
            message: "this herdr has no `session list --json`; it is older than the supported \
                      protocol"
                .into(),
        },
        DiscoveryError::NotRunning(message) => Exit::Unavailable {
            reason: HerdrUnavailable::NotRunning,
            message,
        },
        DiscoveryError::Failed(message) => Exit::failed(message),
    }
}

fn exit_for_snapshot(error: SnapshotError) -> Exit {
    match error {
        SnapshotError::Incompatible { protocol } => Exit::Unavailable {
            reason: HerdrUnavailable::IncompatibleProtocol { protocol },
            message: format!(
                "herdr protocol {protocol} is older than the supported {}",
                project::MIN_PROTOCOL
            ),
        },
        SnapshotError::Unreadable(message) => Exit::failed(message),
    }
}

/// What a watch is for.
struct Target {
    herdr: String,
    session: Option<String>,
}

/// Coalescing of deliveries. Views are held with `version` 0; the counter is applied on
/// delivery.
struct Delivery {
    coalesce: Duration,
    last_sent: Option<Instant>,
    /// The last delivered view, version 0.
    sent: Option<HerdrView>,
    /// The latest view not delivered yet.
    pending: Option<HerdrView>,
    version: u64,
}

impl Delivery {
    fn new(coalesce: Duration) -> Self {
        Self {
            coalesce,
            last_sent: None,
            sent: None,
            pending: None,
            version: 0,
        }
    }

    fn offer(&mut self, view: HerdrView) {
        self.pending = (self.sent.as_ref() != Some(&view)).then_some(view);
    }

    /// When the pending view may be delivered; `None` when there is none.
    fn due(&self) -> Option<Instant> {
        self.pending.as_ref()?;
        Some(
            self.last_sent
                .map_or_else(Instant::now, |sent| sent + self.coalesce),
        )
    }

    fn take_due(&mut self) -> Option<HerdrView> {
        let due = self.due()?;
        if Instant::now() < due {
            return None;
        }
        let mut view = self.pending.take()?;
        self.version += 1;
        self.last_sent = Some(Instant::now());
        self.sent = Some(view.clone());
        view.version = self.version;
        Some(view)
    }

    /// Forgets what was delivered: the next view is a change from `Unavailable`.
    fn reset(&mut self) {
        self.pending = None;
        self.sent = None;
    }
}

async fn next_event<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    events: &mut Option<LineReader<S>>,
) -> Result<Option<Vec<u8>>, WireError> {
    match events {
        Some(reader) => reader.next_line().await,
        None => std::future::pending().await,
    }
}

async fn sleep_until_due(due: Option<Instant>) {
    match due {
        Some(at) => sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Opens an event stream, subscribes to the lifecycle events and to `pane.agent_status_changed`
/// of every pane in `panes`, and waits for the acknowledgement. Events that follow it stay
/// buffered in the returned reader.
async fn subscribe<H: RemoteHost>(
    host: &H,
    socket: &str,
    id: &str,
    panes: &PaneKeys,
    timeout: Duration,
) -> Result<LineReader<H::Stream>, WireError> {
    let mut subscriptions = lifecycle();
    subscriptions.extend(
        panes
            .iter()
            .map(|(pane_id, _)| Subscription::PaneAgentStatusChanged {
                agent_status: None,
                pane_id: pane_id.clone(),
            }),
    );
    let line = wire::encode_request(
        id,
        &RequestBody::EventsSubscribe(EventsSubscribeParams { subscriptions }),
    )?;
    tokio::time::timeout(timeout, async {
        let mut reader = wire::open(host, socket).await?;
        reader.send(&line).await?;
        loop {
            match reader.next_message().await? {
                Some(Message::Success { result, .. }) => {
                    return if result.get("type").and_then(|t| t.as_str())
                        == Some("subscription_started")
                    {
                        Ok(reader)
                    } else {
                        Err(WireError::Malformed(
                            "unexpected answer to events.subscribe".into(),
                        ))
                    };
                }
                Some(Message::Error { code, message, .. }) => {
                    return Err(WireError::Herdr { code, message });
                }
                Some(Message::Event { .. }) => {}
                None => return Err(WireError::Eof),
            }
        }
    })
    .await
    .map_err(|_| WireError::TimedOut)?
}

struct Watch<H: RemoteHost> {
    host: Arc<H>,
    target: Arc<Target>,
    driver: HerdrWatchDriver,
    timing: Timing,
    /// The long-lived event stream, once subscribed.
    events: Option<LineReader<H::Stream>>,
    /// An event arrived since the last read started.
    dirty: bool,
    out: Delivery,
    requests: u64,
}

impl<H: RemoteHost> Watch<H> {
    fn request_id(&mut self) -> String {
        self.requests += 1;
        format!("or2_{}", self.requests)
    }

    /// Delivers the pending view if its time has come.
    fn flush(&mut self) {
        if let Some(view) = self.out.take_due() {
            let _ = self.driver.transition(HerdrState::Live { view });
        }
    }

    fn install(&mut self, view: HerdrView) {
        self.out.offer(view);
        self.flush();
    }

    /// Delivers `Unavailable` unless it is already the reason; drops any pending view.
    fn set_unavailable(&mut self, reason: HerdrUnavailable, message: String) {
        self.out.reset();
        if let HerdrState::Unavailable {
            reason: current, ..
        } = self.driver.state()
            && current == reason
        {
            return;
        }
        let _ = self
            .driver
            .transition(HerdrState::Unavailable { reason, message });
    }

    fn on_event(&mut self, line: Result<Option<Vec<u8>>, WireError>) -> Result<(), Exit> {
        match line {
            Ok(Some(bytes)) => match wire::classify(&bytes) {
                // Whatever it says, something may have changed.
                Ok(Message::Event { .. }) | Err(_) => {
                    self.dirty = true;
                    Ok(())
                }
                Ok(Message::Success { .. }) => Ok(()),
                // `events_lost`, or any other error herdr ends a subscription with.
                Ok(Message::Error { .. }) => self.lost(),
            },
            Ok(None) | Err(_) => self.lost(),
        }
    }

    fn lost(&mut self) -> Result<(), Exit> {
        self.events = None;
        Err(Exit::Resubscribe)
    }

    /// Drives `fut` to completion while the watch keeps up: stop requests end it, events mark
    /// the view dirty, and coalesced deliveries go out on time.
    async fn pump<T>(&mut self, fut: impl Future<Output = T>) -> Result<T, Exit> {
        tokio::pin!(fut);
        loop {
            let due = self.out.due();
            tokio::select! {
                biased;
                () = self.driver.stopped() => return Err(Exit::Stopped),
                line = next_event(&mut self.events) => self.on_event(line)?,
                () = sleep_until_due(due) => self.flush(),
                out = &mut fut => return Ok(out),
            }
        }
    }

    /// Returns once an event is pending.
    async fn idle(&mut self) -> Result<(), Exit> {
        while !self.dirty {
            let due = self.out.due();
            tokio::select! {
                biased;
                () = self.driver.stopped() => return Err(Exit::Stopped),
                line = next_event(&mut self.events) => self.on_event(line)?,
                () = sleep_until_due(due) => self.flush(),
            }
        }
        Ok(())
    }

    async fn attempt(&mut self) -> Exit {
        let exit = match self.attempt_inner().await {
            Ok(never) => match never {},
            Err(exit) => exit,
        };
        // The stream of an ended attempt must not be read while waiting to retry: its end is
        // not a reason to stop.
        self.events = None;
        exit
    }

    /// The lifecycle-only subscription was rejected: read the snapshot once to see whether
    /// herdr is too old (final) before reporting the rejection (`Failed`, retried).
    async fn rejected_subscribe(&mut self, socket: &str, error: WireError) -> Exit {
        let id = self.request_id();
        let host = Arc::clone(&self.host);
        let timeout = self.timing.request;
        let snapshot = self
            .pump(wire::call(
                &*host,
                socket,
                &id,
                &RequestBody::SessionSnapshot(EmptyParams(serde_json::Map::new())),
                timeout,
            ))
            .await;
        match snapshot {
            Err(exit) => exit,
            Ok(Ok(result)) => match project::parse_snapshot(&result) {
                Err(error @ SnapshotError::Incompatible { .. }) => exit_for_snapshot(error),
                _ => exit_for_wire(error),
            },
            Ok(Err(_)) => exit_for_wire(error),
        }
    }

    async fn attempt_inner(&mut self) -> Result<Infallible, Exit> {
        self.events = None;
        self.dirty = false;
        let host = Arc::clone(&self.host);
        let target = Arc::clone(&self.target);
        let timeout = self.timing.request;

        let socket = self
            .pump(discovery::locate(
                &*host,
                &target.herdr,
                target.session.as_deref(),
            ))
            .await?
            .map_err(exit_for_discovery)?;

        // Subscribe first, then read: nothing that happens from here on can be missed.
        let id = self.request_id();
        let none = PaneKeys::new();
        let stream = match self
            .pump(subscribe(&*host, &socket, &id, &none, timeout))
            .await?
        {
            Ok(stream) => stream,
            // An older herdr may not know every lifecycle event and reject the request; its
            // protocol decides between "retry" and "final" as it would for a snapshot.
            Err(error @ WireError::Herdr { .. }) => {
                return Err(self.rejected_subscribe(&socket, error).await);
            }
            Err(error) => return Err(exit_for_wire(error)),
        };
        self.events = Some(stream);

        let mut subscribed = PaneKeys::new();
        let mut rejected = 0;
        let mut last_read: Option<Instant> = None;
        loop {
            // Reconcile: read, install, and read again while an event arrived meanwhile.
            loop {
                if let Some(at) = last_read {
                    let next = at + self.timing.coalesce;
                    if Instant::now() < next {
                        self.pump(sleep_until(next)).await?;
                    }
                }
                // Everything seen so far is covered by the read that starts now.
                self.dirty = false;
                last_read = Some(Instant::now());
                let id = self.request_id();
                let result = self
                    .pump(wire::call(
                        &*host,
                        &socket,
                        &id,
                        &RequestBody::SessionSnapshot(EmptyParams(serde_json::Map::new())),
                        timeout,
                    ))
                    .await?
                    .map_err(exit_for_wire)?;
                let snapshot = project::parse_snapshot(&result).map_err(exit_for_snapshot)?;
                self.install(project::project(&snapshot));

                let panes = project::pane_keys(&snapshot);
                if !panes.is_subset(&subscribed) {
                    let id = self.request_id();
                    match self
                        .pump(subscribe(&*host, &socket, &id, &panes, timeout))
                        .await?
                    {
                        Ok(stream) => {
                            self.events = Some(stream);
                            subscribed = panes;
                            rejected = 0;
                            // Events between the read and the new subscription are gone.
                            self.dirty = true;
                        }
                        // A pane in the list closed first: read the panes again, a few times.
                        Err(WireError::Herdr { code, .. })
                            if code == PANE_NOT_FOUND && rejected < MAX_REJECTED_SUBSCRIPTIONS =>
                        {
                            rejected += 1;
                            self.dirty = true;
                        }
                        // Any other rejection is not about a vanished pane, so re-reading the
                        // panes cannot help. The view stays live on the lifecycle stream that
                        // is still open; per-pane status events are missing until a later
                        // read (the next invalidation) is accepted.
                        Err(WireError::Herdr { code, .. }) if code != PANE_NOT_FOUND => {}
                        Err(error) => return Err(exit_for_wire(error)),
                    }
                }
                if !self.dirty {
                    break;
                }
            }
            self.idle().await?;
        }
    }

    async fn run(mut self) {
        loop {
            match self.attempt().await {
                Exit::Stopped | Exit::HostClosed => break,
                Exit::Resubscribe => {
                    if self.pump(sleep(self.timing.resubscribe)).await.is_err() {
                        break;
                    }
                }
                Exit::Unavailable { reason, message } => {
                    let is_final = reason.is_final();
                    self.set_unavailable(reason, message);
                    if is_final {
                        self.driver.stopped().await;
                        break;
                    }
                    if self.pump(sleep(self.timing.retry)).await.is_err() {
                        break;
                    }
                }
            }
        }
        self.driver.close();
    }
}

/// Runs the watch until `driver` is stopped or the host closes, then closes it.
pub(super) async fn run<H: RemoteHost>(
    host: Arc<H>,
    herdr: String,
    session: Option<String>,
    driver: HerdrWatchDriver,
    timing: Timing,
) {
    Watch {
        host,
        target: Arc::new(Target { herdr, session }),
        driver,
        timing,
        events: None,
        dirty: false,
        out: Delivery::new(timing.coalesce),
        requests: 0,
    }
    .run()
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::view::HerdrView;

    fn view(label: &str) -> HerdrView {
        HerdrView {
            version: 0,
            protocol: 22,
            focused_pane_id: Some(label.to_owned()),
            workspaces: Vec::new(),
            tabs: Vec::new(),
            panes: Vec::new(),
            agents: Vec::new(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_first_view_goes_out_at_once_and_a_burst_collapses_to_the_latest() {
        let mut out = Delivery::new(Duration::from_millis(100));
        out.offer(view("a"));
        let first = out.take_due().expect("immediate");
        assert_eq!(first.version, 1);
        assert_eq!(first.focused_pane_id.as_deref(), Some("a"));

        // A burst inside the window: nothing is due until 100 ms after the first delivery.
        for label in ["b", "c", "d", "e"] {
            tokio::time::advance(Duration::from_millis(10)).await;
            out.offer(view(label));
            assert!(out.take_due().is_none(), "too early for {label}");
        }
        assert_eq!(
            out.due(),
            Some(out.last_sent.unwrap() + Duration::from_millis(100))
        );
        tokio::time::advance(Duration::from_millis(60)).await;
        let second = out.take_due().expect("due now");
        assert_eq!(second.version, 2);
        assert_eq!(
            second.focused_pane_id.as_deref(),
            Some("e"),
            "only the latest"
        );
        assert!(out.take_due().is_none());
        assert!(out.due().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn an_unchanged_view_is_not_delivered_and_a_reset_forgets_what_was_sent() {
        let mut out = Delivery::new(Duration::from_millis(100));
        out.offer(view("a"));
        assert!(out.take_due().is_some());
        tokio::time::advance(Duration::from_secs(1)).await;
        out.offer(view("a"));
        assert!(out.due().is_none(), "equal to what was delivered");
        out.offer(view("b"));
        out.offer(view("a"));
        assert!(out.due().is_none(), "changed back before it was delivered");

        out.reset();
        out.offer(view("a"));
        assert_eq!(
            out.take_due().expect("a change from Unavailable").version,
            2
        );
    }
}
