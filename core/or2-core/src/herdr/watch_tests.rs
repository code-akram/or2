//! The watch and `focus_pane` against a scripted fake herdr, under tokio's paused clock: every
//! interval in the contract (100 ms coalescing, 10 s retry) is exercised in virtual time.
//!
//! Fixtures are captured from isolated herdr 0.9.3 test sessions and sanitized, except
//! `snapshot_agents_future.json`, `event_unknown.jsonl` and `error_events_lost.json`, which are
//! written from the schema and the socket documentation (no agent runs in a test session, and
//! `events_lost` cannot be provoked on demand).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep};

use super::testing::{FakeHost, Served, Step, fixture};
use super::view::AgentStatus;
use super::watch::{self, Timing, lifecycle};
use super::{
    HerdrError, HerdrObserver, HerdrState, HerdrUnavailable, HerdrView, HerdrWatchHandle, channel,
    focus_pane_in,
};
use crate::remote::RemoteError;

const HERDR: &str = "/opt/herdr";
const DEFAULT_SOCKET: &str = "/home/user/.config/herdr/herdr.sock";
const WORK_SOCKET: &str = "/home/user/.config/herdr/sessions/work/herdr.sock";

#[derive(Default)]
struct Recorder(Mutex<Vec<(Instant, HerdrState)>>);

impl Recorder {
    fn lock(&self) -> MutexGuard<'_, Vec<(Instant, HerdrState)>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl HerdrObserver for Recorder {
    fn state_changed(&self, state: &HerdrState) {
        self.lock().push((Instant::now(), state.clone()));
    }
}

struct Harness {
    handle: HerdrWatchHandle,
    recorder: Arc<Recorder>,
    task: JoinHandle<()>,
}

impl Harness {
    fn start(host: &FakeHost, session: Option<&str>) -> Self {
        Self::start_with(host, session, Timing::default())
    }

    fn start_with(host: &FakeHost, session: Option<&str>, timing: Timing) -> Self {
        let recorder = Arc::new(Recorder::default());
        let (handle, driver) = channel(recorder.clone());
        let task = tokio::spawn(watch::run_in(
            Arc::new(host.clone()),
            HERDR.into(),
            Arc::new(Directory::new()),
            session.map(str::to_owned),
            driver,
            timing,
        ));
        Self {
            handle,
            recorder,
            task,
        }
    }

    fn states(&self) -> Vec<HerdrState> {
        self.recorder
            .lock()
            .iter()
            .map(|(_, s)| s.clone())
            .collect()
    }

    fn views(&self) -> Vec<HerdrView> {
        self.states()
            .into_iter()
            .filter_map(|state| match state {
                HerdrState::Live { view } => Some(view),
                _ => None,
            })
            .collect()
    }

    fn latest_view(&self) -> Option<HerdrView> {
        self.views().pop()
    }

    /// Lets virtual time run, 10 ms at a time, until `condition` holds.
    async fn until(&self, what: &str, condition: impl Fn(&Self) -> bool) {
        for _ in 0..30_000 {
            if condition(self) {
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {what}; states: {:#?}", self.states());
    }

    async fn live(&self) {
        self.until("a live view", |h| h.latest_view().is_some())
            .await;
    }

    /// Stops the watch and returns everything it delivered.
    async fn stop(self) -> Vec<HerdrState> {
        self.handle.stop();
        self.task.await.unwrap();
        self.recorder
            .lock()
            .iter()
            .map(|(_, s)| s.clone())
            .collect()
    }
}

fn host_with(snapshot: &str) -> FakeHost {
    let host = FakeHost::new();
    host.set_listing(&fixture("session_list.json"));
    host.snapshot(snapshot);
    host
}

fn two_panes() -> String {
    fixture("snapshot_two_panes.json")
}

/// `two_panes` with the second workspace renamed: same panes, a different view.
fn labelled(label: &str) -> String {
    two_panes().replace("ws-one", label)
}

fn workspace_label(view: &HerdrView) -> &str {
    &view.workspaces[1].label
}

fn subscribe(panes: &[&str]) -> Served {
    Served::Subscribe {
        lifecycle: lifecycle().len(),
        panes: panes.iter().map(|p| (*p).to_owned()).collect(),
    }
}

const ALL_PANES: [&str; 3] = ["w1:p1", "w2:p1", "w2:p2"];

#[tokio::test(start_paused = true)]
async fn bootstrap_subscribes_then_snapshots_then_subscribes_per_pane() {
    let host = host_with(&two_panes());
    let harness = Harness::start(&host, None);
    harness.live().await;
    sleep(Duration::from_secs(1)).await;

    // Subscribe first (lifecycle only, no pane exists yet as far as the client knows), read,
    // then replace the stream by one that also covers every pane, and read again.
    assert_eq!(
        host.served(),
        [
            subscribe(&[]),
            Served::Snapshot,
            subscribe(&ALL_PANES),
            Served::Snapshot
        ]
    );
    assert_eq!(host.opened(), [DEFAULT_SOCKET; 4]);
    assert_eq!(host.exec_log(), ["'/opt/herdr' 'session' 'list' '--json'"]);

    let views = harness.views();
    assert_eq!(
        views.len(),
        1,
        "the identical second read is not redelivered"
    );
    let view = &views[0];
    assert_eq!(view.version, 1);
    assert_eq!(view.focused_pane_id.as_deref(), Some("w2:p2"));
    assert_eq!(view.workspaces.len(), 2);
    assert_eq!(view.tabs.len(), 2);
    assert_eq!(view.panes.len(), 3);
    let states = harness.stop().await;
    assert_eq!(states.last(), Some(&HerdrState::Closed));
}

#[tokio::test(start_paused = true)]
async fn a_named_session_uses_its_own_socket() {
    let host = host_with(&two_panes());
    let harness = Harness::start(&host, Some("work"));
    harness.live().await;
    assert!(host.opened().iter().all(|path| path == WORK_SOCKET));
    harness.stop().await;
}

/// What the app does not read is not projected, so a snapshot that differs only there (a
/// workspace's aggregate status, a pane's scroll position) is read but not delivered again.
#[tokio::test(start_paused = true)]
async fn a_change_the_app_does_not_read_is_not_delivered() {
    let host = host_with(&two_panes());
    let harness = Harness::start(&host, None);
    harness.live().await;
    sleep(Duration::from_secs(1)).await;
    let before = host.snapshots_served();
    let unread = two_panes()
        .replacen(
            r#""agent_status":"unknown"}"#,
            r#""agent_status":"working"}"#,
            1,
        )
        .replace(
            r#""offset_from_bottom":0,"max"#,
            r#""offset_from_bottom":3,"max"#,
        );
    assert_ne!(unread, two_panes());
    host.script_snapshots(vec![Step::reply(&unread)]);
    host.emit(fixture("events_lifecycle.jsonl").lines().next().unwrap());
    sleep(Duration::from_secs(1)).await;
    assert!(host.snapshots_served() > before, "the change was read");
    assert_eq!(harness.views().len(), 1, "and not delivered");
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn events_during_a_read_force_another_authoritative_read() {
    let host = host_with(&labelled("first"));
    let harness = Harness::start(&host, None);
    harness
        .until("bootstrap", |_| host.snapshots_served() == 2)
        .await;
    sleep(Duration::from_secs(1)).await;
    assert_eq!(harness.views().len(), 1);
    let before = host.snapshots_served();

    // The next read is answered while an event is already in flight: the reply is installed,
    // then the view is read again, and that read (no event during it) settles on "third".
    host.script_snapshots(vec![
        Step {
            reply: labelled("second").trim_end().to_owned(),
            events_before: vec![
                fixture("events_lifecycle.jsonl")
                    .lines()
                    .next()
                    .unwrap()
                    .to_owned(),
            ],
            gate: None,
        },
        Step::reply(&labelled("third")),
    ]);
    host.emit(fixture("events_lifecycle.jsonl").lines().nth(1).unwrap());
    harness
        .until("the final view", |h| {
            h.latest_view()
                .is_some_and(|v| workspace_label(&v) == "third")
        })
        .await;
    sleep(Duration::from_secs(1)).await;

    assert_eq!(
        host.snapshots_served() - before,
        2,
        "exactly one repeat read"
    );
    let labels: Vec<_> = harness
        .views()
        .iter()
        .map(|v| workspace_label(v).to_owned())
        .collect();
    assert_eq!(labels, ["first", "second", "third"]);
    assert_eq!(
        harness
            .views()
            .iter()
            .map(|v| v.version)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    // No resubscription: the stream stayed up.
    assert_eq!(
        host.served()
            .iter()
            .filter(|s| matches!(s, Served::Subscribe { .. }))
            .count(),
        2
    );
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn deliveries_are_coalesced_to_one_per_100_ms_and_end_on_the_latest_view() {
    let host = host_with(&labelled("v0"));
    let harness = Harness::start(&host, None);
    harness
        .until("bootstrap", |_| host.snapshots_served() == 2)
        .await;

    // Twenty distinct views with an event each, 5 ms apart: a burst of invalidations.
    let before = host.snapshots_served();
    host.script_snapshots(
        (1..=20)
            .map(|n| Step::reply(&labelled(&format!("v{n}"))))
            .collect(),
    );
    for _ in 1..=20 {
        host.emit(fixture("events_lifecycle.jsonl").lines().next().unwrap());
        sleep(Duration::from_millis(5)).await;
    }
    sleep(Duration::from_secs(2)).await;
    // Reads are paced too: events inside one 100 ms window share a read.
    let reads = host.snapshots_served() - before;
    assert!((2..=3).contains(&reads), "{reads} reads for a 100 ms burst");
    assert_eq!(
        workspace_label(&harness.latest_view().unwrap()),
        format!("v{reads}"),
        "the last read is what is shown"
    );

    let times: Vec<Instant> = harness
        .recorder
        .lock()
        .iter()
        .filter(|(_, s)| matches!(s, HerdrState::Live { .. }))
        .map(|(t, _)| *t)
        .collect();
    assert!(times.len() >= 2);
    for pair in times.windows(2) {
        assert!(
            pair[1] - pair[0] >= Duration::from_millis(100),
            "deliveries closer than 100 ms: {:?}",
            pair[1] - pair[0]
        );
    }
    let versions: Vec<_> = harness.views().iter().map(|v| v.version).collect();
    assert_eq!(versions, (1..=times.len() as u64).collect::<Vec<_>>());
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn events_lost_resubscribes_and_resnapshots_without_leaving_live() {
    let host = host_with(&labelled("before"));
    let harness = Harness::start(&host, None);
    harness
        .until("bootstrap", |_| host.snapshots_served() == 2)
        .await;
    host.script_snapshots(vec![Step::reply(&labelled("after"))]);

    host.emit(&fixture("error_events_lost.json"));
    harness
        .until("the refreshed view", |h| {
            h.latest_view()
                .is_some_and(|v| workspace_label(&v) == "after")
        })
        .await;
    sleep(Duration::from_secs(1)).await;

    let served = host.served();
    assert_eq!(
        served[4..],
        [
            subscribe(&[]),
            Served::Snapshot,
            subscribe(&ALL_PANES),
            Served::Snapshot
        ],
        "a full bootstrap again"
    );
    assert_eq!(host.exec_log().len(), 2, "the socket is rediscovered");
    assert!(
        harness
            .states()
            .iter()
            .all(|s| matches!(s, HerdrState::Live { .. })),
        "the stale view stays up while reconnecting: {:?}",
        harness.states()
    );
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_dropped_event_stream_resubscribes_and_resnapshots() {
    let host = host_with(&labelled("before"));
    let harness = Harness::start(&host, None);
    harness
        .until("bootstrap", |_| host.snapshots_served() == 2)
        .await;
    host.script_snapshots(vec![Step::reply(&labelled("after"))]);
    host.close_events();
    harness
        .until("the refreshed view", |h| {
            h.latest_view()
                .is_some_and(|v| workspace_label(&v) == "after")
        })
        .await;
    assert_eq!(host.served()[4], subscribe(&[]));
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_server_that_went_away_becomes_unavailable_after_the_stream_drops() {
    let host = host_with(&two_panes());
    let harness = Harness::start(&host, None);
    harness.live().await;
    sleep(Duration::from_secs(1)).await;

    // The server stops: its stream drops and the listing now says so.
    host.set_listing(&fixture("session_list.json").replace(
        r#""name":"default","running":true"#,
        r#""name":"default","running":false"#,
    ));
    host.close_events();
    harness
        .until("unavailable", |h| {
            matches!(h.states().last(), Some(HerdrState::Unavailable { .. }))
        })
        .await;
    assert!(matches!(
        harness.states().last(),
        Some(HerdrState::Unavailable {
            reason: HerdrUnavailable::NotRunning,
            ..
        })
    ));
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn not_running_retries_every_ten_seconds_and_is_delivered_once() {
    let host = host_with(&two_panes());
    let harness = Harness::start(&host, Some("idle"));
    harness
        .until("unavailable", |h| !h.states().is_empty())
        .await;
    assert_eq!(host.exec_log().len(), 1);

    sleep(Duration::from_secs(35)).await;
    assert_eq!(host.exec_log().len(), 4, "attempts at 0, 10, 20 and 30 s");
    assert_eq!(
        harness.states().len(),
        1,
        "an unchanged state is not redelivered"
    );
    assert!(matches!(
        harness.states()[0],
        HerdrState::Unavailable {
            reason: HerdrUnavailable::NotRunning,
            ..
        }
    ));
    assert_eq!(host.served(), [], "nothing was opened");

    // The session starts: the next retry goes live.
    host.set_listing(
        &fixture("session_list.json").replace(r#""running":false"#, r#""running":true"#),
    );
    harness.live().await;
    let states = harness.stop().await;
    assert!(matches!(states[1], HerdrState::Live { .. }));
    assert_eq!(states.last(), Some(&HerdrState::Closed));
}

#[tokio::test(start_paused = true)]
async fn failures_retry_and_report_failed_once_per_cause() {
    let host = host_with(&two_panes());
    host.set_exec(1, "", "herdr: broken");
    let harness = Harness::start(&host, None);
    harness.until("failed", |h| !h.states().is_empty()).await;
    sleep(Duration::from_secs(25)).await;
    assert_eq!(host.exec_log().len(), 3);
    let states = harness.states();
    assert_eq!(states.len(), 1);
    let HerdrState::Unavailable { reason, message } = &states[0] else {
        panic!("{states:?}")
    };
    assert_eq!(*reason, HerdrUnavailable::Failed);
    assert!(message.contains("herdr: broken"), "{message}");
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn not_installed_is_final() {
    let host = host_with(&two_panes());
    host.set_exec(127, "", "sh: /opt/herdr: not found");
    let harness = Harness::start(&host, None);
    harness
        .until("unavailable", |h| !h.states().is_empty())
        .await;
    sleep(Duration::from_secs(120)).await;
    assert_eq!(host.exec_log().len(), 1, "no retries");
    let states = harness.stop().await;
    assert_eq!(states.len(), 2);
    assert!(matches!(
        states[0],
        HerdrState::Unavailable {
            reason: HerdrUnavailable::NotInstalled,
            ..
        }
    ));
    assert_eq!(states[1], HerdrState::Closed);
}

#[tokio::test(start_paused = true)]
async fn an_old_protocol_is_incompatible_and_final() {
    let host = host_with(&two_panes().replace(r#""protocol":22"#, r#""protocol":5"#));
    let harness = Harness::start(&host, None);
    harness
        .until("unavailable", |h| !h.states().is_empty())
        .await;
    sleep(Duration::from_secs(120)).await;
    assert_eq!(host.snapshots_served(), 1, "no retries");
    let states = harness.stop().await;
    assert!(matches!(
        &states[0],
        HerdrState::Unavailable {
            reason: HerdrUnavailable::IncompatibleProtocol { protocol: 5 },
            ..
        }
    ));
    assert_eq!(states.len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_newer_protocol_with_unknown_fields_and_values_still_projects() {
    let host = host_with(&fixture("snapshot_agents_future.json"));
    let harness = Harness::start(&host, None);
    harness.live().await;
    let view = harness.latest_view().unwrap();
    assert_eq!(view.agents.len(), 2);
    assert_eq!(view.agents[0].status, AgentStatus::Blocked);
    assert_eq!(view.agents[1].status, AgentStatus::Unknown);
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn an_unreadable_snapshot_fails_and_retries() {
    let host =
        host_with(r#"{"id":"c","result":{"type":"session_snapshot","snapshot":{"protocol":22}}}"#);
    let harness = Harness::start(&host, None);
    harness.until("failed", |h| !h.states().is_empty()).await;
    assert!(matches!(
        harness.states()[0],
        HerdrState::Unavailable {
            reason: HerdrUnavailable::Failed,
            ..
        }
    ));
    sleep(Duration::from_secs(15)).await;
    assert_eq!(host.exec_log().len(), 2);
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn the_end_of_a_failed_attempts_stream_does_not_end_the_watch() {
    // The stream is up (subscribed) when the snapshot turns out unreadable: the attempt
    // fails and the watch waits to retry. The server closing that stream meanwhile is not a
    // stop.
    let host =
        host_with(r#"{"id":"c","result":{"type":"session_snapshot","snapshot":{"protocol":22}}}"#);
    let harness = Harness::start(&host, None);
    harness.until("failed", |h| !h.states().is_empty()).await;
    host.close_events();
    sleep(Duration::from_secs(25)).await;
    assert!(!harness.task.is_finished(), "{:?}", harness.states());
    assert_eq!(host.exec_log().len(), 3, "still retrying every 10 s");
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_running_session_whose_socket_cannot_be_opened_is_failed_and_names_the_likely_cause() {
    // The listing says `running: true`, but the open fails with `Io`: what OpenSSH answers
    // both for a dead socket and for a host that forbids streamlocal forwarding.
    let host = host_with(&two_panes());
    host.set_open_error(Some(RemoteError::Io("connection refused".into())));
    let harness = Harness::start(&host, None);
    harness
        .until("unavailable", |h| !h.states().is_empty())
        .await;
    let HerdrState::Unavailable { reason, message } = harness.states()[0].clone() else {
        panic!("expected Unavailable")
    };
    assert_eq!(reason, HerdrUnavailable::Failed);
    for expected in [
        "running",
        "over SSH",
        "connection refused",
        "AllowStreamLocalForwarding",
        "DisableForwarding",
    ] {
        assert!(
            message.contains(expected),
            "{expected:?} missing: {message}"
        );
    }

    // It keeps retrying every 10 s without redelivering the unchanged reason, and recovers
    // once the socket opens.
    sleep(Duration::from_secs(25)).await;
    assert_eq!(host.exec_log().len(), 3, "attempts at 0, 10 and 20 s");
    assert_eq!(harness.states().len(), 1);
    host.set_open_error(None);
    harness.live().await;
    let states = harness.stop().await;
    assert!(matches!(states[1], HerdrState::Live { .. }));
}

#[tokio::test(start_paused = true)]
async fn a_session_that_is_not_listed_as_running_stays_not_running_and_a_refused_channel_is_failed()
{
    // Not running is decided by the listing, before any socket is opened.
    let host = host_with(&two_panes());
    let harness = Harness::start(&host, Some("idle"));
    harness
        .until("unavailable", |h| !h.states().is_empty())
        .await;
    assert!(matches!(
        harness.states()[0],
        HerdrState::Unavailable {
            reason: HerdrUnavailable::NotRunning,
            ..
        }
    ));
    assert_eq!(host.served(), [], "nothing was opened");
    harness.stop().await;

    // A channel the host refuses outright (`Rejected`) is a failure with its own message.
    let host = host_with(&two_panes());
    host.set_open_error(Some(RemoteError::Rejected("streamlocal disabled".into())));
    let harness = Harness::start(&host, None);
    harness
        .until("unavailable", |h| !h.states().is_empty())
        .await;
    let HerdrState::Unavailable { reason, message } = harness.states()[0].clone() else {
        panic!("expected Unavailable")
    };
    assert_eq!(reason, HerdrUnavailable::Failed);
    assert!(message.contains("streamlocal disabled"), "{message}");
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_pane_that_closes_before_the_subscription_is_retried_after_a_new_read() {
    let host = host_with(&two_panes());
    // The pane-level subscription is rejected once: a pane in it was gone.
    host.script_subscribes(vec![None, Some(("pane_not_found", "pane w2:p2 not found"))]);
    let harness = Harness::start(&host, None);
    harness.live().await;
    sleep(Duration::from_secs(2)).await;
    assert_eq!(
        host.served(),
        [
            subscribe(&[]),
            Served::Snapshot,
            subscribe(&ALL_PANES), // rejected
            Served::Snapshot,
            subscribe(&ALL_PANES),
            Served::Snapshot,
        ]
    );
    assert!(
        harness
            .states()
            .iter()
            .all(|s| matches!(s, HerdrState::Live { .. }))
    );
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn persistent_subscription_rejection_fails_visibly() {
    let host = host_with(&two_panes());
    let mut script = vec![None];
    script.extend((0..20).map(|_| Some(("pane_not_found", "pane gone"))));
    host.script_subscribes(script);
    let harness = Harness::start(&host, None);
    harness
        .until("failed", |h| {
            h.states()
                .iter()
                .any(|s| matches!(s, HerdrState::Unavailable { .. }))
        })
        .await;
    let states = harness.states();
    let HerdrState::Unavailable { reason, message } = states.last().unwrap() else {
        panic!("{states:?}")
    };
    assert_eq!(*reason, HerdrUnavailable::Failed);
    assert!(message.contains("pane_not_found"), "{message}");
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn unknown_events_and_garbage_lines_invalidate_without_dropping_the_stream() {
    let host = host_with(&two_panes());
    let harness = Harness::start(&host, None);
    harness
        .until("bootstrap", |_| host.snapshots_served() == 2)
        .await;
    let before = host.snapshots_served();
    let subscribes = || {
        host.served()
            .iter()
            .filter(|s| matches!(s, Served::Subscribe { .. }))
            .count()
    };
    for line in [
        fixture("event_unknown.jsonl"),
        "this is not json".to_owned(),
        r#"{"surprise":true}"#.to_owned(),
        fixture("ack.json"),
    ] {
        host.emit(&line);
        sleep(Duration::from_millis(300)).await;
    }
    assert_eq!(
        host.snapshots_served() - before,
        3,
        "an invalidation each, none for the ack"
    );
    assert_eq!(subscribes(), 2);
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn stop_delivers_closed_once_last_and_releases_the_observer() {
    let host = host_with(&two_panes());
    let recorder = Arc::new(Recorder::default());
    let weak = Arc::downgrade(&recorder);
    let (handle, driver) = channel(recorder.clone());
    let task = tokio::spawn(watch::run_in(
        Arc::new(host.clone()),
        HERDR.into(),
        Arc::new(Directory::new()),
        None,
        driver,
        Timing::default(),
    ));
    while !matches!(handle.state(), HerdrState::Live { .. }) {
        sleep(Duration::from_millis(10)).await;
    }
    handle.stop();
    handle.stop();
    task.await.unwrap();
    assert_eq!(handle.state(), HerdrState::Closed);
    {
        let states: Vec<_> = recorder.lock().iter().map(|(_, s)| s.clone()).collect();
        assert_eq!(
            states.iter().filter(|s| **s == HerdrState::Closed).count(),
            1
        );
        assert_eq!(states.last(), Some(&HerdrState::Closed));
    }
    assert_eq!(weak.strong_count(), 1, "the driver released the observer");
    drop(recorder);
    assert!(weak.upgrade().is_none());
}

#[tokio::test(start_paused = true)]
async fn stopping_while_unavailable_or_mid_read_closes() {
    // Unavailable (final): waiting for the stop.
    let host = FakeHost::new();
    host.set_exec(127, "", "");
    let harness = Harness::start(&host, None);
    harness
        .until("unavailable", |h| !h.states().is_empty())
        .await;
    assert_eq!(harness.stop().await.last(), Some(&HerdrState::Closed));

    // Mid-read: the snapshot reply is held back.
    let host = FakeHost::new();
    host.set_listing(&fixture("session_list.json"));
    host.step(Step {
        reply: two_panes().trim_end().to_owned(),
        gate: Some(Arc::new(tokio::sync::Notify::new())),
        ..Step::default()
    });
    let harness = Harness::start(&host, None);
    harness
        .until("the read", |_| host.snapshots_served() == 1)
        .await;
    let states = harness.stop().await;
    assert_eq!(states, [HerdrState::Closed]);
}

#[tokio::test(start_paused = true)]
async fn a_closed_host_ends_the_watch_with_closed() {
    let host = host_with(&two_panes());
    let harness = Harness::start(&host, None);
    harness.live().await;
    host.set_exec_error(RemoteError::Closed);
    host.close_events();
    harness.task.await.unwrap();
    let states = harness
        .recorder
        .lock()
        .iter()
        .map(|(_, s)| s.clone())
        .collect::<Vec<_>>();
    assert_eq!(states.last(), Some(&HerdrState::Closed));
    assert_eq!(states.len(), 2, "Live, then Closed: {states:?}");
}

/// What the host driver does when the host closes and it holds only the driver's task: abort
/// it. The driver's `Drop` still delivers `Closed`, once, last.
#[tokio::test(start_paused = true)]
async fn aborting_the_run_task_delivers_closed_once_from_every_state() {
    // Live.
    let host = host_with(&two_panes());
    let harness = Harness::start(&host, None);
    harness.live().await;
    harness.task.abort();
    assert!(harness.task.await.unwrap_err().is_cancelled());
    let states = harness.recorder.lock().clone();
    let states: Vec<_> = states.into_iter().map(|(_, s)| s).collect();
    assert!(matches!(states[0], HerdrState::Live { .. }));
    assert_eq!(states[1..], [HerdrState::Closed]);
    assert_eq!(harness.handle.state(), HerdrState::Closed);

    // Parked at a final Unavailable, where no call is made that could see the host close.
    let host = FakeHost::new();
    host.set_exec(127, "", "");
    let harness = Harness::start(&host, None);
    harness
        .until("unavailable", |h| !h.states().is_empty())
        .await;
    harness.task.abort();
    assert!(harness.task.await.unwrap_err().is_cancelled());
    let states = harness.recorder.lock().clone();
    let states: Vec<_> = states.into_iter().map(|(_, s)| s).collect();
    assert!(matches!(
        states[0],
        HerdrState::Unavailable {
            reason: HerdrUnavailable::NotInstalled,
            ..
        }
    ));
    assert_eq!(states[1..], [HerdrState::Closed]);
    assert_eq!(harness.handle.state(), HerdrState::Closed);
}

#[tokio::test(start_paused = true)]
async fn a_herdr_without_session_list_is_incompatible_and_final() {
    // herdr's usage-error status: this release does not know `session list --json`.
    let host = FakeHost::new();
    host.set_exec(2, "", "herdr: unknown command: session");
    let harness = Harness::start(&host, None);
    harness
        .until("unavailable", |h| !h.states().is_empty())
        .await;
    sleep(Duration::from_secs(120)).await;
    assert_eq!(host.exec_log().len(), 1, "no retries");
    let states = harness.stop().await;
    assert!(matches!(
        &states[0],
        HerdrState::Unavailable {
            reason: HerdrUnavailable::IncompatibleProtocol { protocol: 0 },
            ..
        }
    ));
    assert_eq!(states[1], HerdrState::Closed);
}

#[tokio::test(start_paused = true)]
async fn a_rejected_first_subscribe_on_an_old_herdr_is_incompatible_and_final() {
    let host = host_with(&two_panes().replace(r#""protocol":22"#, r#""protocol":5"#));
    host.script_subscribes(vec![Some(("invalid_request", "unknown event type"))]);
    let harness = Harness::start(&host, None);
    harness
        .until("unavailable", |h| !h.states().is_empty())
        .await;
    sleep(Duration::from_secs(120)).await;
    assert_eq!(host.exec_log().len(), 1, "no retries");
    let states = harness.stop().await;
    assert!(matches!(
        &states[0],
        HerdrState::Unavailable {
            reason: HerdrUnavailable::IncompatibleProtocol { protocol: 5 },
            ..
        }
    ));
    assert_eq!(states.len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_rejected_first_subscribe_on_a_current_herdr_is_failed_and_retried() {
    let host = host_with(&two_panes());
    host.script_subscribes(vec![Some(("invalid_request", "bad request")); 50]);
    let harness = Harness::start(&host, None);
    harness.until("failed", |h| !h.states().is_empty()).await;
    let HerdrState::Unavailable { reason, message } = harness.states()[0].clone() else {
        panic!("{:?}", harness.states())
    };
    assert_eq!(reason, HerdrUnavailable::Failed);
    assert!(message.contains("invalid_request"), "{message}");
    sleep(Duration::from_secs(25)).await;
    assert!(host.exec_log().len() >= 3, "retried every 10 s");
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_pane_subscription_the_server_keeps_rejecting_leaves_the_view_live() {
    // Not `pane_not_found`: re-reading the panes cannot help, and the lifecycle stream and
    // the snapshots still work, so the view must not flap to Unavailable.
    let host = host_with(&two_panes());
    let mut script = vec![None];
    script.extend((0..50).map(|_| Some(("invalid_request", "cannot subscribe"))));
    host.script_subscribes(script);
    let harness = Harness::start(&host, None);
    harness.live().await;
    sleep(Duration::from_secs(25)).await;
    assert_eq!(host.exec_log().len(), 1, "never left the attempt");
    assert_eq!(harness.views().len(), 1);

    // The lifecycle stream still invalidates: the next read shows the change.
    host.script_snapshots(vec![Step::reply(&labelled("ws-two"))]);
    host.emit(fixture("events_lifecycle.jsonl").lines().next().unwrap());
    harness.until("the change", |h| h.views().len() == 2).await;
    assert_eq!(workspace_label(&harness.latest_view().unwrap()), "ws-two");
    let states = harness.stop().await;
    assert!(
        states[..states.len() - 1]
            .iter()
            .all(|s| matches!(s, HerdrState::Live { .. })),
        "{states:?}"
    );
}

/// A pane subscription herdr rejects for another reason than a vanished pane is not asked again
/// on every invalidation (it used to be, each read costing a refused request): only once the
/// panes have changed.
#[tokio::test(start_paused = true)]
async fn a_rejected_pane_subscription_is_asked_again_only_when_the_panes_change() {
    let host = host_with(&two_panes());
    let mut script = vec![None];
    script.extend((0..50).map(|_| Some(("invalid_request", "cannot subscribe"))));
    host.script_subscribes(script);
    let harness = Harness::start(&host, None);
    harness.live().await;
    sleep(Duration::from_secs(1)).await;
    let subscribes = || {
        host.served()
            .iter()
            .filter(|s| matches!(s, Served::Subscribe { .. }))
            .count()
    };
    assert_eq!(subscribes(), 2, "the lifecycle one, and the rejected one");

    // Invalidations over the same panes: each is read, none is subscribed again.
    let before = host.snapshots_served();
    let event = fixture("events_lifecycle.jsonl");
    let event = event.lines().next().unwrap();
    for _ in 0..3 {
        host.emit(event);
        sleep(Duration::from_millis(300)).await;
    }
    assert_eq!(host.snapshots_served() - before, 3);
    assert_eq!(subscribes(), 2);

    // Other panes: their subscription is asked for.
    host.script_snapshots(vec![Step::reply(&fixture(
        "snapshot_one_pane_renamed.json",
    ))]);
    host.emit(event);
    harness
        .until("a new subscription", |_| subscribes() == 3)
        .await;
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_slow_server_times_out_and_is_failed() {
    let host = FakeHost::new();
    host.set_listing(&fixture("session_list.json"));
    host.step(Step {
        reply: two_panes().trim_end().to_owned(),
        gate: Some(Arc::new(tokio::sync::Notify::new())),
        ..Step::default()
    });
    let timing = Timing {
        request: Duration::from_secs(2),
        ..Timing::default()
    };
    let harness = Harness::start_with(&host, None, timing);
    harness.until("failed", |h| !h.states().is_empty()).await;
    let HerdrState::Unavailable { reason, message } = &harness.states()[0] else {
        panic!()
    };
    assert_eq!(*reason, HerdrUnavailable::Failed);
    assert!(message.contains("in time"), "{message}");
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn focus_sends_one_pane_focus_request_to_the_sessions_socket() {
    let host = host_with(&two_panes());
    focus_pane_in(&host, HERDR, &Directory::new(), Some("work"), "w1:p2")
        .await
        .unwrap();
    assert_eq!(host.served(), [Served::Focus("w1:p2".into())]);
    assert_eq!(host.opened(), [WORK_SOCKET]);
    focus_pane_in(&host, HERDR, &Directory::new(), None, "w2:p1")
        .await
        .unwrap();
    assert_eq!(host.opened()[1], DEFAULT_SOCKET);
}

#[tokio::test(start_paused = true)]
async fn focus_failures_are_reported() {
    let host = host_with(&two_panes());
    host.fail_focus("pane_not_found", "pane w9:p9 not found");
    let error = focus_pane_in(&host, HERDR, &Directory::new(), None, "w9:p9")
        .await
        .unwrap_err();
    assert_eq!(error, HerdrError::PaneNotFound);
    // Any other herdr error stays a generic failure carrying herdr's code.
    host.fail_focus("invalid_request", "bad pane");
    let error = focus_pane_in(&host, HERDR, &Directory::new(), None, "w9:p9")
        .await
        .unwrap_err();
    assert!(
        matches!(&error, HerdrError::Failed(m) if m.contains("invalid_request")),
        "{error:?}"
    );

    let error = focus_pane_in(&host, HERDR, &Directory::new(), Some("idle"), "w1:p1")
        .await
        .unwrap_err();
    assert!(matches!(error, HerdrError::Failed(_)), "{error:?}");

    host.set_exec(127, "", "");
    let error = focus_pane_in(&host, HERDR, &Directory::new(), None, "w1:p1")
        .await
        .unwrap_err();
    assert!(matches!(error, HerdrError::Failed(_)), "{error:?}");

    host.set_exec_error(RemoteError::Closed);
    assert_eq!(
        focus_pane_in(&host, HERDR, &Directory::new(), None, "w1:p1").await,
        Err(HerdrError::Remote(RemoteError::Closed))
    );

    host.set_listing(&fixture("session_list.json"));
    host.set_open_error(Some(RemoteError::Closed));
    assert_eq!(
        focus_pane_in(&host, HERDR, &Directory::new(), None, "w1:p1").await,
        Err(HerdrError::Remote(RemoteError::Closed))
    );
}

#[test]
fn generated_types_decode_captured_messages_and_tolerate_the_future() {
    use super::generated::success_response;

    // A snapshot with unknown fields, unknown enum values and a map key the schema forbids.
    let line = fixture("snapshot_agents_future.json");
    let response: success_response::SuccessResponse = serde_json::from_str(&line).unwrap();
    let success_response::ResponseResult::SessionSnapshot { snapshot } = response.result else {
        panic!("not a snapshot")
    };
    assert_eq!(snapshot.protocol, 23);
    assert!(matches!(
        snapshot.workspaces[0].agent_status,
        success_response::AgentStatus::Variant1(ref v) if v == "napping"
    ));
    let ack: success_response::SuccessResponse =
        serde_json::from_str(&fixture("ack.json")).unwrap();
    assert!(matches!(
        ack.result,
        success_response::ResponseResult::SubscriptionStarted
    ));
}

// ---------------------------------------------------------------------------------------
// Discovery through the connection's directory, and the first view's cost.

use super::discovery::{Directory, list_sessions};
use super::focus::FocusGate;

/// A directory holding what the capability probe would have read from the fixture listing.
async fn seeded(host: &FakeHost) -> Arc<Directory> {
    let directory = Arc::new(Directory::new());
    directory.seed(list_sessions(host, HERDR).await.unwrap());
    host.clear_exec_log();
    directory
}

fn start_in(host: &FakeHost, directory: &Arc<Directory>, session: Option<&str>) -> Harness {
    let recorder = Arc::new(Recorder::default());
    let (handle, driver) = channel(recorder.clone());
    let task = tokio::spawn(watch::run_in(
        Arc::new(host.clone()),
        HERDR.into(),
        Arc::clone(directory),
        session.map(str::to_owned),
        driver,
        Timing::default(),
    ));
    Harness {
        handle,
        recorder,
        task,
    }
}

#[tokio::test(start_paused = true)]
async fn a_watch_on_a_seeded_directory_runs_no_listing_and_is_live_after_one_subscribe_and_one_snapshot()
 {
    let host = host_with(&two_panes());
    let directory = seeded(&host).await;
    let started = Instant::now();
    let harness = start_in(&host, &directory, None);
    harness.live().await;

    assert!(host.exec_log().is_empty(), "no `session list` for a watch");
    let (at, _) = harness.recorder.lock()[0].clone();
    assert_eq!(
        at, started,
        "the first view is not held back by the 100 ms between reads"
    );
    // Whatever else the bootstrap does afterwards (the per-pane subscription and its confirming
    // read), the view was installed from the first two requests.
    let served = host.served();
    assert_eq!(
        served[..2],
        [
            Served::Subscribe {
                lifecycle: lifecycle().len(),
                panes: Vec::new()
            },
            Served::Snapshot
        ]
    );
    // The two streams of the first read opened together, before either was used.
    assert_eq!(host.opened()[..2], [DEFAULT_SOCKET; 2]);
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn the_per_pane_subscription_and_its_confirming_read_follow_the_first_view_never_precede_it()
{
    let host = host_with(&two_panes());
    let directory = seeded(&host).await;
    let harness = start_in(&host, &directory, None);
    harness.live().await;
    harness
        .until("the confirming read", |_| host.snapshots_served() >= 2)
        .await;
    let served = host.served();
    let first_pane_subscription = served
        .iter()
        .position(|entry| matches!(entry, Served::Subscribe { panes, .. } if !panes.is_empty()))
        .expect("the per-pane subscription");
    let first_snapshot = served
        .iter()
        .position(|entry| *entry == Served::Snapshot)
        .unwrap();
    assert!(first_snapshot < first_pane_subscription);
    assert_eq!(
        harness.views().len(),
        1,
        "an unchanged view is not redelivered"
    );
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_cached_socket_that_does_not_open_sends_the_attempt_back_to_the_listing_once() {
    let host = host_with(&two_panes());
    let directory = seeded(&host).await;
    // The session moved: the directory still names the old path.
    host.kill_socket(DEFAULT_SOCKET);
    host.set_listing(&fixture("session_list.json").replace(DEFAULT_SOCKET, "/moved/herdr.sock"));
    let harness = start_in(&host, &directory, None);
    harness.live().await;
    assert_eq!(host.exec_log().len(), 1, "one listing, in the same attempt");
    assert!(host.opened().contains(&"/moved/herdr.sock".to_owned()));
    assert_eq!(
        directory.cached_socket(None).as_deref(),
        Some("/moved/herdr.sock"),
        "the directory learned the new path"
    );
    let states = harness.stop().await;
    assert!(
        !states
            .iter()
            .any(|state| matches!(state, HerdrState::Unavailable { .. })),
        "never reported as unavailable: {states:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_socket_from_a_fresh_listing_that_does_not_open_fails_without_a_second_listing() {
    let host = host_with(&two_panes());
    host.kill_socket(DEFAULT_SOCKET);
    let harness = start_in(&host, &Arc::new(Directory::new()), None);
    harness
        .until("unavailable", |h| !h.states().is_empty())
        .await;
    assert_eq!(host.exec_log().len(), 1);
    assert!(matches!(
        harness.states()[0],
        HerdrState::Unavailable {
            reason: HerdrUnavailable::Failed,
            ..
        }
    ));
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn only_the_first_attempt_trusts_the_directory_a_retry_reads_the_listing_again() {
    let host = host_with("not json");
    let directory = seeded(&host).await;
    let harness = start_in(&host, &directory, None);
    harness
        .until("the first failure", |h| !h.states().is_empty())
        .await;
    assert!(
        host.exec_log().is_empty(),
        "the first attempt used the seed"
    );
    // Ten seconds later the retry discovers again, and recovers when the snapshot reads.
    host.script_snapshots(vec![Step::reply(&two_panes())]);
    harness.live().await;
    assert_eq!(host.exec_log().len(), 1);
    harness.stop().await;
}

// ---------------------------------------------------------------------------------------
// The focus gate: a focus from the app and the terminal's own meet.

async fn focus_once(
    gate: &FocusGate,
    host: &FakeHost,
    directory: &Arc<Directory>,
    pane: &str,
    from_terminal: bool,
) -> Result<(), HerdrError> {
    gate.focus(
        &Arc::new(host.clone()),
        HERDR,
        directory,
        None,
        pane,
        from_terminal,
    )
    .await
}

fn focuses(host: &FakeHost) -> usize {
    host.served()
        .iter()
        .filter(|entry| matches!(entry, Served::Focus(_)))
        .count()
}

#[tokio::test(start_paused = true)]
async fn a_focus_uses_the_cached_socket_and_runs_no_listing() {
    let host = host_with(&two_panes());
    let directory = seeded(&host).await;
    let gate = FocusGate::new();
    focus_once(&gate, &host, &directory, "w2:p1", false)
        .await
        .unwrap();
    assert!(host.exec_log().is_empty());
    assert_eq!(host.served(), [Served::Focus("w2:p1".into())]);
}

#[tokio::test(start_paused = true)]
async fn a_focus_with_nothing_cached_reads_the_listing_once() {
    let host = host_with(&two_panes());
    let (gate, directory) = (FocusGate::new(), Arc::new(Directory::new()));
    focus_once(&gate, &host, &directory, "w2:p1", false)
        .await
        .unwrap();
    focus_once(&gate, &host, &directory, "w2:p2", false)
        .await
        .unwrap();
    assert_eq!(
        host.exec_log().len(),
        1,
        "the second focus knows the socket"
    );
}

#[tokio::test(start_paused = true)]
async fn a_stale_cached_socket_is_rediscovered_once_and_a_missing_session_is_not_retried() {
    let host = host_with(&two_panes());
    let directory = seeded(&host).await;
    let gate = FocusGate::new();
    host.kill_socket(DEFAULT_SOCKET);
    host.set_listing(&fixture("session_list.json").replace(DEFAULT_SOCKET, "/moved/herdr.sock"));
    focus_once(&gate, &host, &directory, "w2:p1", false)
        .await
        .unwrap();
    assert_eq!(host.exec_log().len(), 1);
    assert_eq!(host.opened(), [DEFAULT_SOCKET, "/moved/herdr.sock"]);
    // A path from a listing read just now that does not open is the answer, not a reason to
    // read the listing again.
    host.kill_socket("/moved/herdr.sock");
    let error = focus_once(&gate, &host, &directory, "w2:p2", false)
        .await
        .unwrap_err();
    assert!(matches!(error, HerdrError::Failed(_)), "{error:?}");
    assert_eq!(host.exec_log().len(), 2, "one more listing, not two");
}

#[tokio::test(start_paused = true)]
async fn a_focus_joins_the_same_focus_in_flight_instead_of_sending_another() {
    let host = host_with(&two_panes());
    let directory = seeded(&host).await;
    let gate = FocusGate::new();
    let (app, terminal) = tokio::join!(
        focus_once(&gate, &host, &directory, "w2:p1", false),
        focus_once(&gate, &host, &directory, "w2:p1", true),
    );
    app.unwrap();
    terminal.unwrap();
    assert_eq!(focuses(&host), 1);
    // A different pane is its own focus.
    focus_once(&gate, &host, &directory, "w2:p2", false)
        .await
        .unwrap();
    assert_eq!(focuses(&host), 2);
}

#[tokio::test(start_paused = true)]
async fn a_terminals_focus_accepts_a_recent_acknowledgement_but_the_apps_never_does() {
    let host = host_with(&two_panes());
    let directory = seeded(&host).await;
    let gate = FocusGate::new();
    focus_once(&gate, &host, &directory, "w2:p1", false)
        .await
        .unwrap();
    focus_once(&gate, &host, &directory, "w2:p1", true)
        .await
        .unwrap();
    assert_eq!(focuses(&host), 1, "the terminal's focus is already done");
    // The user taps the same agent again: a real focus, since the desktop may have moved on.
    focus_once(&gate, &host, &directory, "w2:p1", false)
        .await
        .unwrap();
    assert_eq!(focuses(&host), 2);
    // Past the window the terminal's own focus is sent too.
    sleep(super::focus::RECENT + Duration::from_millis(1)).await;
    focus_once(&gate, &host, &directory, "w2:p1", true)
        .await
        .unwrap();
    assert_eq!(focuses(&host), 3);
}

#[tokio::test(start_paused = true)]
async fn a_failed_focus_is_shared_with_those_waiting_and_remembered_by_nobody() {
    let host = host_with(&two_panes());
    host.fail_focus("pane_not_found", "no such pane");
    let directory = seeded(&host).await;
    let gate = FocusGate::new();
    let (app, terminal) = tokio::join!(
        focus_once(&gate, &host, &directory, "w9:p9", false),
        focus_once(&gate, &host, &directory, "w9:p9", true),
    );
    assert_eq!(app, Err(HerdrError::PaneNotFound));
    assert_eq!(terminal, Err(HerdrError::PaneNotFound));
    assert_eq!(focuses(&host), 1);
    // The pane is back: nothing remembers the failure.
    host.clear_focus_error();
    focus_once(&gate, &host, &directory, "w9:p9", true)
        .await
        .unwrap();
    assert_eq!(focuses(&host), 2);
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_focus_leaves_its_followers_to_focus_for_themselves() {
    let host = host_with(&two_panes());
    let directory = seeded(&host).await;
    let gate = FocusGate::new();
    // The leader is dropped while it waits for herdr's answer (a cancelled query).
    let follower = async {
        let mut leader = Box::pin(focus_once(&gate, &host, &directory, "w2:p1", false));
        // Poll the leader once so it holds the slot, then drop it.
        tokio::select! {
            biased;
            _ = &mut leader => unreachable!("the focus needs a round trip"),
            () = std::future::ready(()) => {}
        }
        drop(leader);
        focus_once(&gate, &host, &directory, "w2:p1", true).await
    };
    follower.await.unwrap();
    assert!(focuses(&host) >= 1);
}

#[tokio::test(start_paused = true)]
async fn a_recent_focus_of_a_pane_does_not_satisfy_a_terminal_after_another_pane_was_focused() {
    let host = host_with(&two_panes());
    let directory = seeded(&host).await;
    let gate = FocusGate::new();
    focus_once(&gate, &host, &directory, "w2:p1", false)
        .await
        .unwrap();
    focus_once(&gate, &host, &directory, "w2:p2", false)
        .await
        .unwrap();
    // Pane 2 is the focused one: a terminal on pane 1 must focus it again, inside RECENT.
    focus_once(&gate, &host, &directory, "w2:p1", true)
        .await
        .unwrap();
    assert_eq!(
        host.served().last(),
        Some(&Served::Focus("w2:p1".into())),
        "opening A after B must restore A"
    );
    // That focus is now the latest: the next terminal on the same pane accepts it.
    let before = focuses(&host);
    focus_once(&gate, &host, &directory, "w2:p1", true)
        .await
        .unwrap();
    assert_eq!(focuses(&host), before);
}

#[tokio::test(start_paused = true)]
async fn overlapping_focuses_of_two_panes_leave_only_the_later_one_recent() {
    let host = host_with(&two_panes());
    let directory = seeded(&host).await;
    let gate = FocusGate::new();
    // A is asked first and B while A is in flight: B is the pane herdr ends on.
    let (a, b) = tokio::join!(
        focus_once(&gate, &host, &directory, "w2:p1", false),
        focus_once(&gate, &host, &directory, "w2:p2", false),
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(focuses(&host), 2);
    focus_once(&gate, &host, &directory, "w2:p1", true)
        .await
        .unwrap();
    assert_eq!(
        host.served().last(),
        Some(&Served::Focus("w2:p1".into())),
        "A's acknowledgement is not authoritative once B was asked"
    );
    // B is not recent any more either, after A was focused again.
    focus_once(&gate, &host, &directory, "w2:p2", true)
        .await
        .unwrap();
    assert_eq!(host.served().last(), Some(&Served::Focus("w2:p2".into())));
}

#[tokio::test(start_paused = true)]
async fn a_focus_in_another_herdr_session_does_not_invalidate_a_recent_one() {
    let host = host_with(&two_panes());
    let directory = seeded(&host).await;
    let gate = FocusGate::new();
    focus_once(&gate, &host, &directory, "w2:p1", false)
        .await
        .unwrap();
    // The other session's focus fails (nothing is listed for it): it must not touch session None.
    let _ = gate
        .focus(
            &Arc::new(host.clone()),
            HERDR,
            &directory,
            Some("other"),
            "w2:p2",
            false,
        )
        .await;
    let before = focuses(&host);
    focus_once(&gate, &host, &directory, "w2:p1", true)
        .await
        .unwrap();
    assert_eq!(focuses(&host), before);
}

// ---------------------------------------------------------------------------------------
// The focus gate serializes a session's focuses: the order they reach herdr is the order they
// were asked in, and an acknowledgement is only trusted as the session's last.

/// Holds the first socket open until released: the request of the focus that opened it reaches
/// herdr only then, however early it started.
struct HoldFirstOpen {
    host: FakeHost,
    first: std::sync::atomic::AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl HoldFirstOpen {
    fn new(host: &FakeHost) -> Self {
        Self {
            host: host.clone(),
            first: std::sync::atomic::AtomicBool::new(true),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        }
    }
}

impl crate::remote::RemoteHost for HoldFirstOpen {
    type Stream = tokio::io::DuplexStream;

    async fn exec_rendered(&self, line: &str) -> Result<crate::remote::ExecOutput, RemoteError> {
        crate::remote::RemoteHost::exec_rendered(&self.host, line).await
    }

    async fn open_unix(&self, path: &str) -> Result<Self::Stream, RemoteError> {
        if self.first.swap(false, std::sync::atomic::Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        crate::remote::RemoteHost::open_unix(&self.host, path).await
    }
}

/// Lets every other task and future that can run, run.
async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

/// Polls `future` once so that it takes its place, and hands it back.
async fn poll_once<F: std::future::Future + Unpin>(future: &mut F) {
    tokio::select! {
        biased;
        _ = future => unreachable!("the focus needs round trips"),
        () = std::future::ready(()) => {}
    }
}

fn focused_panes(host: &FakeHost) -> Vec<String> {
    host.served()
        .into_iter()
        .filter_map(|entry| match entry {
            Served::Focus(pane) => Some(pane),
            _ => None,
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn out_of_order_focus_does_not_leave_the_wrong_pane_recent() {
    let fake = host_with(&two_panes());
    let directory = seeded(&fake).await;
    let host = Arc::new(HoldFirstOpen::new(&fake));
    let gate = FocusGate::new();
    // A starts first and its socket is held; B starts meanwhile and must not reach herdr first.
    let (a, b) = tokio::join!(
        gate.focus(&host, HERDR, &directory, None, "w2:p1", false),
        async {
            host.entered.notified().await;
            let b = gate.focus(&host, HERDR, &directory, None, "w2:p2", false);
            tokio::pin!(b);
            tokio::select! {
                biased;
                result = &mut b => panic!("B was sent ahead of A: {result:?}"),
                () = settle() => {}
            }
            assert!(focused_panes(&fake).is_empty(), "B overtook A");
            host.release.notify_one();
            b.await
        },
    );
    a.unwrap();
    b.unwrap();
    // Remote order is local order: A, then B. B is the pane herdr is on, and says so.
    assert_eq!(focused_panes(&fake), ["w2:p1", "w2:p2"]);
    gate.focus(&host, HERDR, &directory, None, "w2:p2", true)
        .await
        .unwrap();
    assert_eq!(
        focused_panes(&fake),
        ["w2:p1", "w2:p2"],
        "B is the last focus"
    );
    // A is not: a terminal on A sends its focus.
    gate.focus(&host, HERDR, &directory, None, "w2:p1", true)
        .await
        .unwrap();
    assert_eq!(focused_panes(&fake), ["w2:p1", "w2:p2", "w2:p1"]);
}

#[tokio::test(start_paused = true)]
async fn overlapping_a_b_a_reach_herdr_in_that_order_and_leave_a_recent() {
    let host = host_with(&two_panes());
    let directory = seeded(&host).await;
    let gate = FocusGate::new();
    // The second A (a terminal's) does not join the first or take its answer: B was asked in
    // between, so it is sent after B.
    let (a1, b, a2) = tokio::join!(
        focus_once(&gate, &host, &directory, "w2:p1", false),
        focus_once(&gate, &host, &directory, "w2:p2", false),
        focus_once(&gate, &host, &directory, "w2:p1", true),
    );
    a1.unwrap();
    b.unwrap();
    a2.unwrap();
    assert_eq!(focused_panes(&host), ["w2:p1", "w2:p2", "w2:p1"]);
    focus_once(&gate, &host, &directory, "w2:p1", true)
        .await
        .unwrap();
    assert_eq!(
        focused_panes(&host).len(),
        3,
        "A is the session's last focus"
    );
    focus_once(&gate, &host, &directory, "w2:p2", true)
        .await
        .unwrap();
    assert_eq!(focused_panes(&host), ["w2:p1", "w2:p2", "w2:p1", "w2:p2"]);
}

#[tokio::test(start_paused = true)]
async fn a_focus_cancelled_while_queued_does_not_wedge_the_queue_or_its_followers() {
    let fake = host_with(&two_panes());
    let directory = seeded(&fake).await;
    let host = Arc::new(HoldFirstOpen::new(&fake));
    let gate = FocusGate::new();
    let (a, c) = tokio::join!(
        gate.focus(&host, HERDR, &directory, None, "w2:p1", false),
        async {
            host.entered.notified().await;
            // B queues behind A, and C joins B.
            let mut b = Box::pin(gate.focus(&host, HERDR, &directory, None, "w2:p2", false));
            poll_once(&mut b).await;
            let mut c = Box::pin(gate.focus(&host, HERDR, &directory, None, "w2:p2", true));
            poll_once(&mut c).await;
            // B is given up while it waits for its turn.
            drop(b);
            host.release.notify_one();
            c.await
        },
    );
    a.unwrap();
    c.unwrap();
    // C asked for itself and was sent after A.
    assert_eq!(focused_panes(&fake), ["w2:p1", "w2:p2"]);
    // The queue is free: a later focus is served.
    focus_once(&gate, &fake, &directory, "w2:p1", false)
        .await
        .unwrap();
    assert_eq!(focused_panes(&fake), ["w2:p1", "w2:p2", "w2:p1"]);
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_focus_makes_the_session_forget_its_acknowledgement() {
    let fake = host_with(&two_panes());
    let directory = seeded(&fake).await;
    let gate = FocusGate::new();
    focus_once(&gate, &fake, &directory, "w2:p1", false)
        .await
        .unwrap();
    {
        // B is given up after it started: it may or may not have reached herdr.
        let mut b = Box::pin(focus_once(&gate, &fake, &directory, "w2:p2", false));
        poll_once(&mut b).await;
    }
    settle().await;
    let before = focused_panes(&fake).len();
    focus_once(&gate, &fake, &directory, "w2:p1", true)
        .await
        .unwrap();
    assert_eq!(
        focused_panes(&fake).last().map(String::as_str),
        Some("w2:p1")
    );
    assert_eq!(focused_panes(&fake).len(), before + 1, "A was sent again");
}

#[tokio::test(start_paused = true)]
async fn another_herdr_session_does_not_wait_for_a_held_focus() {
    let fake = host_with(&two_panes());
    let directory = seeded(&fake).await;
    let host = Arc::new(HoldFirstOpen::new(&fake));
    let gate = FocusGate::new();
    let (a, other) = tokio::join!(
        gate.focus(&host, HERDR, &directory, None, "w2:p1", false),
        async {
            host.entered.notified().await;
            // Session "work" is independent: its focus completes while the default session's
            // is still held.
            let other = gate
                .focus(&host, HERDR, &directory, Some("work"), "w2:p2", false)
                .await;
            assert_eq!(
                focused_panes(&fake),
                ["w2:p2"],
                "the held focus has not reached herdr"
            );
            host.release.notify_one();
            other
        },
    );
    a.unwrap();
    other.unwrap();
    assert_eq!(focused_panes(&fake), ["w2:p2", "w2:p1"]);
    // Each session's last acknowledgement stands on its own.
    gate.focus(&host, HERDR, &directory, None, "w2:p1", true)
        .await
        .unwrap();
    gate.focus(&host, HERDR, &directory, Some("work"), "w2:p2", true)
        .await
        .unwrap();
    assert_eq!(focused_panes(&fake).len(), 2);
}

// ---------------------------------------------------------------------------------------
// The focus actor: cancellation leaves the session's queue and its memory consistent.

fn terminal_focus<'a, H: crate::remote::RemoteHost>(
    gate: &'a FocusGate,
    host: &'a Arc<H>,
    directory: &'a Arc<Directory>,
    session: Option<&'a str>,
    pane: &'a str,
    from_terminal: bool,
) -> impl std::future::Future<Output = Result<(), HerdrError>> + 'a {
    gate.focus(host, HERDR, directory, session, pane, from_terminal)
}

#[tokio::test(start_paused = true)]
async fn out_of_order_release_still_reaches_herdr_in_asked_order() {
    let fake = host_with(&two_panes());
    let directory = seeded(&fake).await;
    let host = Arc::new(HoldFirstOpen::new(&fake));
    let gate = FocusGate::new();
    let mut a = Box::pin(terminal_focus(
        &gate, &host, &directory, None, "w2:p1", false,
    ));
    poll_once(&mut a).await;
    host.entered.notified().await;
    let mut b = Box::pin(terminal_focus(
        &gate, &host, &directory, None, "w2:p2", false,
    ));
    poll_once(&mut b).await;
    assert!(focused_panes(&fake).is_empty(), "B overtook held A");
    host.release.notify_one();
    a.await.unwrap();
    b.await.unwrap();
    assert_eq!(focused_panes(&fake), ["w2:p1", "w2:p2"]);
    terminal_focus(&gate, &host, &directory, None, "w2:p2", true)
        .await
        .unwrap();
    assert_eq!(focused_panes(&fake), ["w2:p1", "w2:p2"]);
    terminal_focus(&gate, &host, &directory, None, "w2:p1", true)
        .await
        .unwrap();
    assert_eq!(focused_panes(&fake), ["w2:p1", "w2:p2", "w2:p1"]);
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_tail_does_not_restore_a_stale_recent_while_an_older_pending_runs() {
    let fake = host_with(&two_panes());
    let directory = seeded(&fake).await;
    let host = Arc::new(HoldFirstOpen::new(&fake));
    let gate = FocusGate::new();
    let mut a = Box::pin(terminal_focus(
        &gate, &host, &directory, None, "w2:p1", false,
    ));
    poll_once(&mut a).await;
    host.entered.notified().await;
    let mut b = Box::pin(terminal_focus(
        &gate, &host, &directory, None, "w2:p2", false,
    ));
    poll_once(&mut b).await;
    let mut c = Box::pin(terminal_focus(
        &gate, &host, &directory, None, "w1:p1", false,
    ));
    poll_once(&mut c).await;
    drop(c);
    // Hold B's reply after herdr acted on it (A is the first focus herdr gets): the remote pane
    // is B, and a cached success for A would be wrong.
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    fake.hold_focus_after(1, entered.clone(), release.clone());
    host.release.notify_one();
    a.await.unwrap();
    entered.notified().await;
    assert_eq!(focused_panes(&fake), ["w2:p1", "w2:p2"]);
    let mut terminal_a = Box::pin(terminal_focus(
        &gate, &host, &directory, None, "w2:p1", true,
    ));
    tokio::select! {
        biased;
        result = &mut terminal_a => panic!("terminal A accepted a stale A while herdr is on B: {result:?}"),
        () = settle() => {}
    }
    release.notify_one();
    let (b, terminal_a) = tokio::join!(b, terminal_a);
    b.unwrap();
    terminal_a.unwrap();
    assert_eq!(focused_panes(&fake), ["w2:p1", "w2:p2", "w2:p1"]);
}

#[tokio::test(start_paused = true)]
async fn a_new_focus_after_a_cancelled_tail_joins_the_pending_one() {
    let fake = host_with(&two_panes());
    let directory = seeded(&fake).await;
    let host = Arc::new(HoldFirstOpen::new(&fake));
    let gate = FocusGate::new();
    let mut a = Box::pin(terminal_focus(
        &gate, &host, &directory, None, "w2:p1", false,
    ));
    poll_once(&mut a).await;
    host.entered.notified().await;
    let mut b = Box::pin(terminal_focus(
        &gate, &host, &directory, None, "w2:p2", false,
    ));
    poll_once(&mut b).await;
    let mut c = Box::pin(terminal_focus(
        &gate, &host, &directory, None, "w1:p1", false,
    ));
    poll_once(&mut c).await;
    drop(c);
    let mut follower_b = Box::pin(terminal_focus(
        &gate, &host, &directory, None, "w2:p2", true,
    ));
    poll_once(&mut follower_b).await;
    host.release.notify_one();
    a.await.unwrap();
    let (b, follower_b) = tokio::join!(b, follower_b);
    b.unwrap();
    follower_b.unwrap();
    assert_eq!(
        focused_panes(&fake),
        ["w2:p1", "w2:p2"],
        "the same-pane request shares the pending B once the cancelled C has left"
    );
}

#[tokio::test(start_paused = true)]
async fn the_last_queued_focus_is_remembered_only_once_the_queue_has_drained() {
    let fake = host_with(&two_panes());
    let directory = seeded(&fake).await;
    let gate = FocusGate::new();
    let (a, b, c) = tokio::join!(
        focus_once(&gate, &fake, &directory, "w2:p1", false),
        focus_once(&gate, &fake, &directory, "w2:p2", false),
        focus_once(&gate, &fake, &directory, "w1:p1", false),
    );
    a.unwrap();
    b.unwrap();
    c.unwrap();
    assert_eq!(focused_panes(&fake), ["w2:p1", "w2:p2", "w1:p1"]);
    focus_once(&gate, &fake, &directory, "w1:p1", true)
        .await
        .unwrap();
    assert_eq!(focused_panes(&fake).len(), 3, "C is the one herdr is on");
    focus_once(&gate, &fake, &directory, "w2:p2", true)
        .await
        .unwrap();
    assert_eq!(focused_panes(&fake).len(), 4, "B is not");
}

#[tokio::test(start_paused = true)]
async fn a_queued_focus_that_all_its_waiters_gave_up_is_never_sent() {
    let fake = host_with(&two_panes());
    let directory = seeded(&fake).await;
    let host = Arc::new(HoldFirstOpen::new(&fake));
    let gate = FocusGate::new();
    let mut a = Box::pin(terminal_focus(
        &gate, &host, &directory, None, "w2:p1", false,
    ));
    poll_once(&mut a).await;
    host.entered.notified().await;
    // B has two waiters, both of which leave; C has one, which stays.
    let mut b1 = Box::pin(terminal_focus(
        &gate, &host, &directory, None, "w2:p2", false,
    ));
    poll_once(&mut b1).await;
    let mut b2 = Box::pin(terminal_focus(
        &gate, &host, &directory, None, "w2:p2", true,
    ));
    poll_once(&mut b2).await;
    let mut c = Box::pin(terminal_focus(
        &gate, &host, &directory, None, "w1:p1", false,
    ));
    poll_once(&mut c).await;
    drop(b1);
    drop(b2);
    host.release.notify_one();
    a.await.unwrap();
    c.await.unwrap();
    assert_eq!(
        focused_panes(&fake),
        ["w2:p1", "w1:p1"],
        "B had nobody waiting: it has no side effect"
    );
}

#[tokio::test(start_paused = true)]
async fn one_live_waiter_is_enough_to_send_a_queued_focus() {
    let fake = host_with(&two_panes());
    let directory = seeded(&fake).await;
    let host = Arc::new(HoldFirstOpen::new(&fake));
    let gate = FocusGate::new();
    let mut a = Box::pin(terminal_focus(
        &gate, &host, &directory, None, "w2:p1", false,
    ));
    poll_once(&mut a).await;
    host.entered.notified().await;
    let mut b1 = Box::pin(terminal_focus(
        &gate, &host, &directory, None, "w2:p2", false,
    ));
    poll_once(&mut b1).await;
    let mut b2 = Box::pin(terminal_focus(
        &gate, &host, &directory, None, "w2:p2", true,
    ));
    poll_once(&mut b2).await;
    drop(b1);
    host.release.notify_one();
    a.await.unwrap();
    b2.await.unwrap();
    assert_eq!(focused_panes(&fake), ["w2:p1", "w2:p2"]);
}

#[tokio::test(start_paused = true)]
async fn a_focus_in_flight_completes_after_its_caller_left_and_is_remembered() {
    let fake = host_with(&two_panes());
    let directory = seeded(&fake).await;
    let gate = FocusGate::new();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    fake.hold_next_focus(entered.clone(), release.clone());
    let mut a = Box::pin(focus_once(&gate, &fake, &directory, "w2:p1", false));
    poll_once(&mut a).await;
    entered.notified().await;
    assert_eq!(focused_panes(&fake), ["w2:p1"], "herdr has the request");
    drop(a);
    // Herdr answers after the caller left: the focus counts, as the session's last.
    release.notify_one();
    settle().await;
    focus_once(&gate, &fake, &directory, "w2:p1", true)
        .await
        .unwrap();
    assert_eq!(focused_panes(&fake), ["w2:p1"], "nothing was sent again");
}

#[tokio::test(start_paused = true)]
async fn a_request_that_joins_a_focus_in_flight_after_its_caller_left_gets_its_answer() {
    let fake = host_with(&two_panes());
    let directory = seeded(&fake).await;
    let gate = FocusGate::new();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    fake.hold_next_focus(entered.clone(), release.clone());
    let mut a = Box::pin(focus_once(&gate, &fake, &directory, "w2:p1", false));
    poll_once(&mut a).await;
    entered.notified().await;
    drop(a);
    let mut joiner = Box::pin(focus_once(&gate, &fake, &directory, "w2:p1", true));
    poll_once(&mut joiner).await;
    release.notify_one();
    joiner.await.unwrap();
    assert_eq!(focused_panes(&fake), ["w2:p1"]);
}

#[tokio::test(start_paused = true)]
async fn a_pending_focus_in_one_session_does_not_touch_another_sessions_memory() {
    let fake = host_with(&two_panes());
    let directory = seeded(&fake).await;
    let gate = FocusGate::new();
    let host = Arc::new(fake.clone());
    terminal_focus(&gate, &host, &directory, None, "w2:p1", false)
        .await
        .unwrap();
    // Session "work" has a focus in flight, with a request queued and then cancelled behind it.
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    fake.hold_next_focus(entered.clone(), release.clone());
    let mut w1 = Box::pin(terminal_focus(
        &gate,
        &host,
        &directory,
        Some("work"),
        "w2:p2",
        false,
    ));
    poll_once(&mut w1).await;
    entered.notified().await;
    let mut w2 = Box::pin(terminal_focus(
        &gate,
        &host,
        &directory,
        Some("work"),
        "w1:p1",
        false,
    ));
    poll_once(&mut w2).await;
    drop(w2);
    // The default session still answers from memory, at once.
    terminal_focus(&gate, &host, &directory, None, "w2:p1", true)
        .await
        .unwrap();
    assert_eq!(focused_panes(&fake), ["w2:p1", "w2:p2"]);
    release.notify_one();
    w1.await.unwrap();
    terminal_focus(&gate, &host, &directory, Some("work"), "w2:p2", true)
        .await
        .unwrap();
    assert_eq!(focused_panes(&fake), ["w2:p1", "w2:p2"]);
}

#[tokio::test(start_paused = true)]
async fn an_idle_actor_stays_for_the_window_of_its_acknowledgement_and_then_ends() {
    let fake = host_with(&two_panes());
    let directory = seeded(&fake).await;
    let gate = FocusGate::new();
    focus_once(&gate, &fake, &directory, "w2:p1", false)
        .await
        .unwrap();
    // Idle for most of the window: the acknowledgement still stands.
    sleep(super::focus::RECENT - Duration::from_millis(100)).await;
    focus_once(&gate, &fake, &directory, "w2:p1", true)
        .await
        .unwrap();
    assert_eq!(focuses(&fake), 1);
    // Past it, the actor has gone: a new one starts, and sends.
    sleep(super::focus::RECENT + Duration::from_millis(100)).await;
    assert_eq!(
        gate.actors_running(),
        0,
        "no actor lingers without an acknowledgement to keep"
    );
    focus_once(&gate, &fake, &directory, "w2:p1", true)
        .await
        .unwrap();
    assert_eq!(focuses(&fake), 2);
}

/// Opens each socket after a pause that varies from one open to the next.
struct Jitter {
    host: FakeHost,
    opens: std::sync::atomic::AtomicU64,
}

impl crate::remote::RemoteHost for Jitter {
    type Stream = tokio::io::DuplexStream;

    async fn exec_rendered(&self, line: &str) -> Result<crate::remote::ExecOutput, RemoteError> {
        crate::remote::RemoteHost::exec_rendered(&self.host, line).await
    }

    async fn open_unix(&self, path: &str) -> Result<Self::Stream, RemoteError> {
        let n = self.opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        sleep(Duration::from_millis(n * 7 % 13)).await;
        crate::remote::RemoteHost::open_unix(&self.host, path).await
    }
}

/// A small deterministic generator: the stress test must be the same run every time.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, below: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % below
    }
}

#[tokio::test(start_paused = true)]
async fn a_recent_answer_is_only_ever_given_for_the_last_request_herdr_received_with_nothing_pending()
 {
    const PANES: [&str; 3] = ["w2:p1", "w2:p2", "w1:p1"];
    let fake = host_with(&two_panes());
    let directory = seeded(&fake).await;
    let host = Arc::new(Jitter {
        host: fake.clone(),
        opens: std::sync::atomic::AtomicU64::new(0),
    });
    let gate = Arc::new(FocusGate::new());
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let (fake, hits) = (fake.clone(), Arc::clone(&hits));
        *gate.hit_hook.lock().unwrap() = Some(Arc::new(move |pane: &str, pending: usize| {
            hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(pending, 0, "a recent answer with a focus pending");
            assert_eq!(
                focused_panes(&fake).last().map(String::as_str),
                Some(pane),
                "a recent answer for a pane herdr was not last given"
            );
        }));
    }
    let mut random = Lcg(7);
    let mut tasks = Vec::new();
    for _ in 0..400 {
        // Half arrive on their own, half in bursts that overlap.
        let start = Duration::from_millis(if random.next(2) == 0 {
            random.next(6000)
        } else {
            100 * random.next(60)
        });
        let pane = PANES[random.next(3) as usize];
        let from_terminal = random.next(3) != 0;
        // About a third give up at some point.
        let give_up = (random.next(3) == 0).then(|| Duration::from_millis(1 + random.next(40)));
        let (gate, host, directory) =
            (Arc::clone(&gate), Arc::clone(&host), Arc::clone(&directory));
        tasks.push(tokio::spawn(async move {
            sleep(start).await;
            let focus = gate.focus(&host, HERDR, &directory, None, pane, from_terminal);
            match give_up {
                Some(limit) => tokio::time::timeout(limit, focus).await.ok(),
                None => Some(focus.await),
            }
        }));
    }
    let mut answered = 0;
    for task in tasks {
        if let Some(result) = task.await.unwrap() {
            result.unwrap();
            answered += 1;
        }
    }
    assert!(answered > 100, "answered {answered}");
    let hits = hits.load(std::sync::atomic::Ordering::SeqCst);
    assert!(hits > 5, "the stress must exercise recent answers: {hits}");
    // Whatever was left behind, the gate still works.
    gate.focus(&host, HERDR, &directory, None, "w2:p1", false)
        .await
        .unwrap();
}
