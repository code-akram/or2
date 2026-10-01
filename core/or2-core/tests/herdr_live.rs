//! The herdr client against a real herdr server, through `LocalHost`.
//!
//! Every test starts its own uniquely named session (`herdr --session or2-test-<pid>-<n>
//! server`), talks only to that session's socket, and stops and deletes exactly that session
//! afterwards. The user's default session and every other session are never touched. Skipped
//! with a message when herdr is not installed, unless `OR2_REQUIRE_HERDR` is set.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use or2_core::herdr::generated::request::{
    PaneAgentState, PaneReleaseAgentParams, PaneReportAgentParams, PaneRightClickTarget,
    PaneSendTextParams, PaneSplitParams, PaneTarget, RequestBody, SplitDirection, TabCreateParams,
    WorkspaceCloseParams, WorkspaceCreateParams, WorkspaceRenameParams,
};
use or2_core::herdr::view::AgentStatus;
use or2_core::herdr::{
    HerdrObserver, HerdrState, HerdrUnavailable, HerdrView, HerdrWatchHandle, Timing, focus_pane,
    watch, watch_with_timing, wire,
};
use or2_core::remote::LocalHost;
use serde_json::Value;

static SESSIONS: AtomicUsize = AtomicUsize::new(0);

fn herdr_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("OR2_HERDR") {
        return Some(PathBuf::from(path));
    }
    let path = std::env::var_os("PATH")?;
    let home = std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/bin"));
    std::env::split_paths(&path)
        .chain(home)
        .map(|dir| dir.join("herdr"))
        .find(|candidate| candidate.is_file())
}

/// A herdr command that cannot reach the user's live session, and does not inherit its
/// startup directory, through the `HERDR_*` variables of the pane the tests may run in. The
/// isolated server therefore starts with no workspace.
fn herdr_command(herdr: &PathBuf) -> Command {
    let mut command = Command::new(herdr);
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("HERDR_") {
            command.env_remove(name);
        }
    }
    command.stdin(Stdio::null());
    command
}

/// A herdr server of one isolated named session. Dropping stops and deletes that session.
struct Isolated {
    herdr: PathBuf,
    name: String,
    server: Option<Child>,
}

impl Isolated {
    /// `None` (after saying why) when herdr is not installed.
    fn new() -> Option<Self> {
        let Some(herdr) = herdr_binary() else {
            assert!(
                std::env::var_os("OR2_REQUIRE_HERDR").is_none(),
                "OR2_REQUIRE_HERDR is set but herdr is not installed"
            );
            eprintln!("skipping: herdr is not installed (set OR2_HERDR to its path)");
            return None;
        };
        let name = format!(
            "or2-test-{}-{}",
            std::process::id(),
            SESSIONS.fetch_add(1, Ordering::Relaxed)
        );
        assert!(name.starts_with("or2-test-"));
        Some(Self {
            herdr,
            name,
            server: None,
        })
    }

    fn command(&self) -> Command {
        herdr_command(&self.herdr)
    }

    fn herdr(&self) -> &str {
        self.herdr.to_str().expect("utf-8 herdr path")
    }

    /// Starts the headless server child and waits for its socket.
    fn start(&mut self) {
        assert!(self.server.is_none(), "already started");
        let child = self
            .command()
            .args(["--session", &self.name, "server"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start the isolated herdr server");
        self.server = Some(child);
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if self.socket().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the isolated herdr server did not come up");
    }

    /// The running session's socket, from `session list --json`.
    fn socket(&self) -> Option<String> {
        let output = self
            .command()
            .args(["session", "list", "--json"])
            .output()
            .ok()?;
        let listing: Value = serde_json::from_slice(&output.stdout).ok()?;
        let entry = listing["sessions"]
            .as_array()?
            .iter()
            .find(|entry| entry["name"] == self.name.as_str())?;
        assert_eq!(entry["default"], false, "never the default session");
        let socket = entry["socket_path"].as_str()?;
        (entry["running"] == true && std::path::Path::new(socket).exists())
            .then(|| socket.to_owned())
    }

    /// Stops the server (the session stays on disk, as after a crash or `session stop`).
    fn stop(&mut self) {
        assert!(self.name.starts_with("or2-test-"));
        let _ = self
            .command()
            .args(["session", "stop", &self.name])
            .output();
        if let Some(mut child) = self.server.take() {
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut exited = false;
            while Instant::now() < deadline {
                if child.try_wait().ok().flatten().is_some() {
                    exited = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            if !exited {
                // Our own child, started by this test.
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        // The session counts as stopped once the listing says so and its socket is gone, even
        // if the server process hands over to something else on its way out.
        let deadline = Instant::now() + Duration::from_secs(15);
        while self.socket().is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// One request to the session's socket; returns the `result` object.
    async fn call(&self, body: RequestBody) -> Value {
        let socket = self.socket().expect("the session is running");
        wire::call(
            &LocalHost::new(),
            &socket,
            "test",
            &body,
            Duration::from_secs(10),
        )
        .await
        .unwrap_or_else(|error| panic!("{body:?}: {error}"))
    }
}

impl Drop for Isolated {
    fn drop(&mut self) {
        self.stop();
        assert!(self.name.starts_with("or2-test-"));
        let _ = self
            .command()
            .args(["session", "delete", &self.name])
            .output();
    }
}

#[derive(Default)]
struct Recorder(Mutex<Vec<HerdrState>>);

impl Recorder {
    fn states(&self) -> MutexGuard<'_, Vec<HerdrState>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl HerdrObserver for Recorder {
    fn state_changed(&self, state: &HerdrState) {
        self.states().push(state.clone());
    }
}

fn live_view(handle: &HerdrWatchHandle) -> Option<HerdrView> {
    match handle.state() {
        HerdrState::Live { view } => Some(view),
        _ => None,
    }
}

/// Polls until the watch's view satisfies `condition`; returns that view.
async fn view_where(
    handle: &HerdrWatchHandle,
    what: &str,
    condition: impl Fn(&HerdrView) -> bool,
) -> HerdrView {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Some(view) = live_view(handle).filter(&condition) {
            return view;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}; state: {:#?}", handle.state());
}

fn str_at<'a>(value: &'a Value, path: &str) -> &'a str {
    value
        .pointer(path)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{path} in {value}"))
}

#[tokio::test]
async fn the_view_follows_an_isolated_session_and_focus_pane_works() {
    let Some(mut herdr) = Isolated::new() else {
        return;
    };
    herdr.start();
    let host = Arc::new(LocalHost::new());
    let recorder = Arc::new(Recorder::default());
    let handle = watch(
        Arc::clone(&host),
        herdr.herdr().to_owned(),
        Some(herdr.name.clone()),
        recorder.clone(),
    );

    // A fresh session is empty.
    let initial = view_where(&handle, "the first view", |_| true).await;
    assert_eq!(initial.version, 1);
    assert_eq!(initial.protocol, 22);
    assert!(initial.workspaces.is_empty() && initial.tabs.is_empty());
    assert!(initial.panes.is_empty() && initial.agents.is_empty());
    assert_eq!(initial.focused_pane_id, None);

    // A focused workspace with its root pane.
    let created = herdr
        .call(RequestBody::WorkspaceCreate(WorkspaceCreateParams {
            cwd: Some("/tmp".into()),
            label: Some("or2-first".into()),
            focus: true,
            ..WorkspaceCreateParams::default()
        }))
        .await;
    let first_pane = str_at(&created, "/root_pane/pane_id").to_owned();
    let view = view_where(&handle, "the first workspace", |v| v.panes.len() == 1).await;
    assert_eq!(view.workspaces.len(), 1);
    assert_eq!(view.tabs.len(), 1);
    assert!(view.workspaces[0].focused && view.tabs[0].focused && view.panes[0].focused);
    assert_eq!(view.focused_pane_id.as_deref(), Some(first_pane.as_str()));

    // A second workspace (not focused) appears with its root pane.
    let created = herdr
        .call(RequestBody::WorkspaceCreate(WorkspaceCreateParams {
            cwd: Some("/tmp".into()),
            label: Some("or2-live".into()),
            focus: false,
            ..WorkspaceCreateParams::default()
        }))
        .await;
    let workspace = str_at(&created, "/workspace/workspace_id").to_owned();
    let root = str_at(&created, "/root_pane/pane_id").to_owned();
    let view = view_where(&handle, "the new workspace", |v| v.workspaces.len() == 2).await;
    let shown = view
        .workspaces
        .iter()
        .find(|w| w.workspace_id == workspace)
        .expect("the new workspace is in the view");
    assert_eq!(shown.label, "or2-live");
    assert!(!shown.focused);
    assert_eq!(view.panes.len(), 2);
    let pane = view.panes.iter().find(|p| p.pane_id == root).unwrap();
    assert_eq!(pane.workspace_id, workspace);
    assert_eq!(pane.cwd.as_deref(), Some("/tmp"));
    assert_eq!(view.focused_pane_id.as_deref(), Some(first_pane.as_str()));

    // Split it: a third pane.
    let split = herdr
        .call(RequestBody::PaneSplit(PaneSplitParams {
            cwd: None,
            direction: SplitDirection::Right,
            env: HashMap::new(),
            focus: false,
            ratio: None,
            right_click: PaneRightClickTarget::Herdr,
            target_pane_id: Some(root.clone()),
            workspace_id: None,
        }))
        .await;
    let split_pane = str_at(&split, "/pane/pane_id").to_owned();
    let view = view_where(&handle, "the split pane", |v| v.panes.len() == 3).await;
    assert!(view.panes.iter().any(|p| p.pane_id == split_pane));

    // A tab and a rename show up too.
    herdr
        .call(RequestBody::TabCreate(TabCreateParams {
            workspace_id: Some(workspace.clone()),
            label: Some("logs".into()),
            ..TabCreateParams::default()
        }))
        .await;
    let view = view_where(&handle, "the new tab", |v| {
        v.tabs.iter().any(|t| t.label == "logs")
    })
    .await;
    assert_eq!(
        view.tabs.len(),
        3,
        "one in the first workspace, two in the second"
    );
    herdr
        .call(RequestBody::WorkspaceRename(WorkspaceRenameParams {
            label: "or2-renamed".into(),
            workspace_id: workspace.clone(),
        }))
        .await;
    view_where(&handle, "the rename", |v| {
        v.workspaces.iter().any(|w| w.label == "or2-renamed")
    })
    .await;

    // An agent reports its state: the per-pane status subscription and the agent list. A
    // foreground process keeps herdr from clearing the report once the pane is back at an
    // idle shell prompt.
    herdr
        .call(RequestBody::PaneSendText(PaneSendTextParams {
            pane_id: split_pane.clone(),
            text: "sleep 120\n".into(),
        }))
        .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let report = |seq: u64, state: PaneAgentState| {
        RequestBody::PaneReportAgent(PaneReportAgentParams {
            agent: "or2-test-agent".into(),
            agent_session_id: None,
            agent_session_path: None,
            message: None,
            pane_id: split_pane.clone(),
            resume_argv: None,
            seq: Some(seq),
            source: "or2-test".into(),
            state,
        })
    };
    herdr.call(report(1, PaneAgentState::Working)).await;
    let view = view_where(&handle, "a working agent", |v| {
        v.agents.iter().any(|a| a.status == AgentStatus::Working)
    })
    .await;
    let agent = view
        .agents
        .iter()
        .find(|a| a.pane_id == split_pane)
        .unwrap();
    assert_eq!(agent.agent.as_deref(), Some("or2-test-agent"));
    assert_eq!(agent.workspace_id, workspace);
    let first_seq = agent.state_change_seq;
    let pane = view.panes.iter().find(|p| p.pane_id == split_pane).unwrap();
    assert_eq!(pane.agent_status, AgentStatus::Working);

    herdr.call(report(2, PaneAgentState::Blocked)).await;
    let view = view_where(&handle, "a blocked agent", |v| {
        v.agents.iter().any(|a| a.status == AgentStatus::Blocked)
    })
    .await;
    let agent = view
        .agents
        .iter()
        .find(|a| a.pane_id == split_pane)
        .unwrap();
    assert!(
        agent.state_change_seq > first_seq,
        "the transition counter moved"
    );
    assert_eq!(
        view.workspaces
            .iter()
            .find(|w| w.workspace_id == workspace)
            .unwrap()
            .agent_status,
        AgentStatus::Blocked,
        "the workspace aggregates its agents"
    );

    // Focus a pane in the other workspace, through `focus_pane`.
    focus_pane(&*host, herdr.herdr(), Some(&herdr.name), &split_pane)
        .await
        .unwrap();
    let view = view_where(&handle, "the focus change", |v| {
        v.focused_pane_id.as_deref() == Some(split_pane.as_str())
    })
    .await;
    assert!(
        view.panes
            .iter()
            .find(|p| p.pane_id == split_pane)
            .unwrap()
            .focused
    );
    assert!(
        !view
            .panes
            .iter()
            .find(|p| p.pane_id == first_pane)
            .unwrap()
            .focused
    );
    // A pane that does not exist is an error, not a silent success.
    assert!(
        focus_pane(&*host, herdr.herdr(), Some(&herdr.name), "w999:p999")
            .await
            .is_err()
    );

    // Release the agent, close the split pane and the workspace.
    herdr
        .call(RequestBody::PaneReleaseAgent(PaneReleaseAgentParams {
            agent: "or2-test-agent".into(),
            pane_id: split_pane.clone(),
            seq: Some(3),
            source: "or2-test".into(),
        }))
        .await;
    view_where(&handle, "the agent released", |v| v.agents.is_empty()).await;
    herdr
        .call(RequestBody::PaneClose(PaneTarget {
            pane_id: split_pane.clone(),
        }))
        .await;
    view_where(&handle, "the pane closed", |v| {
        v.panes.iter().all(|p| p.pane_id != split_pane)
    })
    .await;
    herdr
        .call(RequestBody::WorkspaceClose(WorkspaceCloseParams {
            close_group: None,
            workspace_id: workspace.clone(),
        }))
        .await;
    let view = view_where(&handle, "the workspace closed", |v| v.workspaces.len() == 1).await;
    assert_eq!(view.panes.len(), 1);
    assert_eq!(view.tabs.len(), 1);
    assert_eq!(view.focused_pane_id.as_deref(), Some(first_pane.as_str()));

    // Versions only increase, deliveries only ever Live until the stop, and Closed is last.
    handle.stop();
    let deadline = Instant::now() + Duration::from_secs(10);
    while handle.state() != HerdrState::Closed && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let states = recorder.states().clone();
    assert_eq!(states.last(), Some(&HerdrState::Closed));
    let versions: Vec<u64> = states
        .iter()
        .filter_map(|state| match state {
            HerdrState::Live { view } => Some(view.version),
            _ => None,
        })
        .collect();
    assert!(
        versions.windows(2).all(|pair| pair[0] < pair[1]),
        "{versions:?}"
    );
    assert!(
        states[..states.len() - 1]
            .iter()
            .all(|state| matches!(state, HerdrState::Live { .. })),
        "no Unavailable while the server stayed up"
    );
    assert_eq!(
        states.iter().filter(|s| **s == HerdrState::Closed).count(),
        1
    );
}

#[tokio::test]
async fn the_watch_recovers_when_the_server_starts_stops_and_restarts() {
    let Some(mut herdr) = Isolated::new() else {
        return;
    };
    let host = Arc::new(LocalHost::new());
    let recorder = Arc::new(Recorder::default());
    // The session exists in nobody's list yet. The retry pause is shortened from the
    // contract's 10 s; the logic under test is the same.
    let timing = Timing {
        retry: Duration::from_millis(500),
        ..Timing::default()
    };
    let handle = watch_with_timing(
        host,
        herdr.herdr().to_owned(),
        Some(herdr.name.clone()),
        recorder.clone(),
        timing,
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while matches!(handle.state(), HerdrState::Starting) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        matches!(
            handle.state(),
            HerdrState::Unavailable {
                reason: HerdrUnavailable::NotRunning,
                ..
            }
        ),
        "{:?}",
        handle.state()
    );

    // The server starts: the next retry goes live.
    herdr.start();
    view_where(&handle, "the view after the server started", |_| true).await;

    // The server stops: the stream drops and the watch reports it (a request that was in
    // flight may first see `Failed`, then the listing says the session is not running)...
    herdr.stop();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !matches!(
        handle.state(),
        HerdrState::Unavailable {
            reason: HerdrUnavailable::NotRunning,
            ..
        }
    ) && Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        matches!(
            handle.state(),
            HerdrState::Unavailable {
                reason: HerdrUnavailable::NotRunning,
                ..
            }
        ),
        "{:?}",
        handle.state()
    );

    // ...and recovers when it comes back.
    herdr.start();
    view_where(&handle, "the view after the restart", |_| true).await;
    handle.stop();
    let deadline = Instant::now() + Duration::from_secs(10);
    while handle.state() != HerdrState::Closed && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let states = recorder.states().clone();
    assert_eq!(states.last(), Some(&HerdrState::Closed));
    // Starting is never delivered; NotRunning is reported once per outage, never repeated
    // while it lasts.
    let not_running = states
        .iter()
        .filter(|s| {
            matches!(
                s,
                HerdrState::Unavailable {
                    reason: HerdrUnavailable::NotRunning,
                    ..
                }
            )
        })
        .count();
    assert_eq!(not_running, 2, "{states:#?}");
    assert!(
        states
            .iter()
            .all(|s| !matches!(s, HerdrState::Unavailable { reason, .. } if reason.is_final())),
        "{states:#?}"
    );
}
