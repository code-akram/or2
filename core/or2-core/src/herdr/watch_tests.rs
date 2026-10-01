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
    focus_pane,
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
        let task = tokio::spawn(watch::run(
            Arc::new(host.clone()),
            HERDR.into(),
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
    assert_eq!(view.protocol, 22);
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
    assert_eq!(view.protocol, 23);
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
    let task = tokio::spawn(watch::run(
        Arc::new(host.clone()),
        HERDR.into(),
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
    focus_pane(&host, HERDR, Some("work"), "w1:p2")
        .await
        .unwrap();
    assert_eq!(host.served(), [Served::Focus("w1:p2".into())]);
    assert_eq!(host.opened(), [WORK_SOCKET]);
    focus_pane(&host, HERDR, None, "w2:p1").await.unwrap();
    assert_eq!(host.opened()[1], DEFAULT_SOCKET);
}

#[tokio::test(start_paused = true)]
async fn focus_failures_are_reported() {
    let host = host_with(&two_panes());
    host.fail_focus("pane_not_found", "pane w9:p9 not found");
    let error = focus_pane(&host, HERDR, None, "w9:p9").await.unwrap_err();
    assert!(
        matches!(&error, HerdrError::Failed(m) if m.contains("pane_not_found")),
        "{error:?}"
    );

    let error = focus_pane(&host, HERDR, Some("idle"), "w1:p1")
        .await
        .unwrap_err();
    assert!(matches!(error, HerdrError::Failed(_)), "{error:?}");

    host.set_exec(127, "", "");
    let error = focus_pane(&host, HERDR, None, "w1:p1").await.unwrap_err();
    assert!(matches!(error, HerdrError::Failed(_)), "{error:?}");

    host.set_exec_error(RemoteError::Closed);
    assert_eq!(
        focus_pane(&host, HERDR, None, "w1:p1").await,
        Err(HerdrError::Remote(RemoteError::Closed))
    );

    host.set_listing(&fixture("session_list.json"));
    host.set_open_error(Some(RemoteError::Closed));
    assert_eq!(
        focus_pane(&host, HERDR, None, "w1:p1").await,
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
