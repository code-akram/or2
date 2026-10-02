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
    EmptyParams, PaneAgentState, PaneReleaseAgentParams, PaneReportAgentParams,
    PaneRightClickTarget, PaneSendTextParams, PaneSplitParams, PaneTarget, RequestBody,
    SplitDirection, TabCreateParams, WorkspaceCloseParams, WorkspaceCreateParams,
    WorkspaceRenameParams,
};
use or2_core::herdr::view::AgentStatus;
use or2_core::herdr::{
    Directory, FocusGate, HerdrError, HerdrObserver, HerdrState, HerdrUnavailable, HerdrView,
    HerdrWatchHandle, Timing, focus_pane, list_sessions, navigate_in, run_in, watch,
    watch_with_timing, wire,
};
use or2_core::host::{NavDirection, TargetNav};
use or2_core::remote::{ExecOutput, LocalHost, RemoteError, RemoteHost};
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

/// A host whose `session list --json` names a socket nothing listens on: what a directory
/// holds after the session moved. Everything else is the real thing.
struct StaleListing(LocalHost);

impl RemoteHost for StaleListing {
    type Stream = <LocalHost as RemoteHost>::Stream;

    async fn exec_rendered(&self, line: &str) -> Result<ExecOutput, RemoteError> {
        let mut output = self.0.exec_rendered(line).await?;
        if line.contains("'session' 'list'") {
            let mut listing: Value = serde_json::from_slice(&output.stdout).unwrap();
            for session in listing["sessions"].as_array_mut().unwrap() {
                session["socket_path"] = "/nonexistent/or2-test/herdr.sock".into();
            }
            output.stdout = serde_json::to_vec(&listing).unwrap().into();
        }
        Ok(output)
    }

    async fn open_unix(&self, path: &str) -> Result<Self::Stream, RemoteError> {
        self.0.open_unix(path).await
    }
}

/// A watch and a focus that take their socket from the connection's directory (seeded with the
/// listing, as the capability probe does) work against a real herdr; a directory that names a
/// socket that is gone is rediscovered once, by the focus and by the watch.
#[tokio::test]
async fn a_directory_seeds_the_watch_and_the_focus_and_a_stale_path_is_rediscovered() {
    let Some(mut herdr) = Isolated::new() else {
        return;
    };
    herdr.start();
    let host = Arc::new(LocalHost::new());
    let created = herdr
        .call(RequestBody::WorkspaceCreate(WorkspaceCreateParams {
            cwd: Some("/tmp".into()),
            label: Some("or2-directory".into()),
            focus: true,
            ..WorkspaceCreateParams::default()
        }))
        .await;
    let root = str_at(&created, "/root_pane/pane_id").to_owned();

    // The directory as the probe seeds it: the real listing, with this session's real socket.
    let directory = Arc::new(Directory::new());
    directory.seed(list_sessions(&*host, herdr.herdr()).await.unwrap());
    let recorder = Arc::new(Recorder::default());
    let (handle, driver) = or2_core::herdr::channel(recorder.clone());
    let task = tokio::spawn(run_in(
        Arc::clone(&host),
        herdr.herdr().to_owned(),
        Arc::clone(&directory),
        Some(herdr.name.clone()),
        driver,
    ));
    let view = view_where(&handle, "the view from the seeded directory", |v| {
        v.panes.len() == 1
    })
    .await;
    assert_eq!(view.panes[0].pane_id, root);

    // A focus through the gate: the app's, then a terminal's own, which the first satisfies.
    let gate = FocusGate::new();
    let focus = |pane: String, from_terminal: bool| {
        let (gate, host, herdr, directory) = (&gate, &host, &herdr, &directory);
        async move {
            gate.focus(
                host,
                herdr.herdr(),
                directory,
                Some(&herdr.name),
                &pane,
                from_terminal,
            )
            .await
        }
    };
    focus(root.clone(), false).await.unwrap();
    focus(root.clone(), true).await.unwrap();
    // A pane that is not there is the explicit error.
    assert_eq!(
        focus("w99:p99".into(), false).await,
        Err(HerdrError::PaneNotFound)
    );
    handle.stop();
    task.await.unwrap();

    // The same with a directory that holds a socket that is gone: the focus lists once and
    // finds the real one; so does a watch (its first attempt, then the same attempt again).
    let stale_host = StaleListing(LocalHost::new());
    let stale = Arc::new(Directory::new());
    stale.seed(list_sessions(&stale_host, herdr.herdr()).await.unwrap());
    let gate = FocusGate::new();
    gate.focus(
        &host,
        herdr.herdr(),
        &stale,
        Some(&herdr.name),
        &root,
        false,
    )
    .await
    .expect("a stale socket is rediscovered");

    let stale = Arc::new(Directory::new());
    stale.seed(list_sessions(&stale_host, herdr.herdr()).await.unwrap());
    let (handle, driver) = or2_core::herdr::channel(Arc::new(Recorder::default()));
    let task = tokio::spawn(run_in(
        Arc::clone(&host),
        herdr.herdr().to_owned(),
        stale,
        Some(herdr.name.clone()),
        driver,
    ));
    view_where(&handle, "the view after rediscovering", |v| {
        v.panes.len() == 1
    })
    .await;
    handle.stop();
    task.await.unwrap();
}

/// The pane's `scroll.offset_from_bottom`, as herdr reports it.
async fn offset_from_bottom(herdr: &Isolated, pane: &str) -> u64 {
    let info = herdr
        .call(RequestBody::PaneGet(PaneTarget {
            pane_id: pane.to_owned(),
        }))
        .await;
    info.pointer("/pane/scroll/offset_from_bottom")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("no scroll in {info}"))
}

#[tokio::test]
async fn scroll_pane_moves_a_panes_history_by_lines_and_back_to_the_bottom() {
    use or2_core::herdr::{ScrollOffsets, scroll_pane_in};
    use or2_core::host::TargetScroll;
    let Some(mut herdr) = Isolated::new() else {
        return;
    };
    herdr.start();
    let created = herdr
        .call(RequestBody::WorkspaceCreate(WorkspaceCreateParams {
            cwd: Some("/tmp".into()),
            label: Some("or2-scroll".into()),
            focus: true,
            ..WorkspaceCreateParams::default()
        }))
        .await;
    let pane = str_at(&created, "/root_pane/pane_id").to_owned();
    herdr
        .call(RequestBody::PaneSendText(PaneSendTextParams {
            pane_id: pane.clone(),
            text: "seq 1 500\n".into(),
        }))
        .await;
    // Wait for the output (and the prompt after it) to reach the pane's history and stop
    // growing: new lines would move a scrolled pane's offset under the assertions.
    let deadline = Instant::now() + Duration::from_secs(30);
    let (mut last, mut steady_since) = (0, Instant::now());
    loop {
        let info = herdr
            .call(RequestBody::PaneGet(PaneTarget {
                pane_id: pane.clone(),
            }))
            .await;
        let max = info
            .pointer("/pane/scroll/max_offset_from_bottom")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if max != last {
            (last, steady_since) = (max, Instant::now());
        } else if max > 100 && steady_since.elapsed() >= Duration::from_millis(500) {
            break;
        }
        assert!(Instant::now() < deadline, "no steady history: {info}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (host, directory, offsets) = (LocalHost::new(), Directory::new(), ScrollOffsets::new());
    let session = Some(herdr.name.as_str());
    let scroll = async |pane_id: Option<&str>, scroll| {
        scroll_pane_in(
            &host,
            herdr.herdr(),
            &directory,
            &offsets,
            session,
            pane_id,
            scroll,
        )
        .await
    };
    // Without a pane: the focused one, which is this workspace's root pane.
    scroll(None, TargetScroll::Up { lines: 7 }).await.unwrap();
    assert_eq!(offset_from_bottom(&herdr, &pane).await, 7);
    scroll(Some(&pane), TargetScroll::Up { lines: 5 })
        .await
        .unwrap();
    assert_eq!(offset_from_bottom(&herdr, &pane).await, 12);
    scroll(Some(&pane), TargetScroll::Down { lines: 2 })
        .await
        .unwrap();
    assert_eq!(offset_from_bottom(&herdr, &pane).await, 10);
    assert_eq!(offsets.get(session, &pane), 10);
    scroll(Some(&pane), TargetScroll::Bottom).await.unwrap();
    assert_eq!(offset_from_bottom(&herdr, &pane).await, 0);
    assert_eq!(offsets.get(session, &pane), 0);
    // A pane that does not exist.
    assert_eq!(
        scroll(Some("w9:p9"), TargetScroll::Up { lines: 1 }).await,
        Err(HerdrError::PaneNotFound)
    );
}
/// The focused workspace, its active tab and the focused pane, from a snapshot.
async fn focus_of(herdr: &Isolated) -> (String, String, String) {
    let snapshot = herdr
        .call(RequestBody::SessionSnapshot(EmptyParams(
            serde_json::Map::new(),
        )))
        .await;
    let snapshot = &snapshot["snapshot"];
    let workspace = snapshot["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["focused"] == true)
        .unwrap_or_else(|| panic!("a focused workspace in {snapshot}"));
    (
        str_at(workspace, "/workspace_id").to_owned(),
        str_at(workspace, "/active_tab_id").to_owned(),
        str_at(snapshot, "/focused_pane_id").to_owned(),
    )
}

#[tokio::test]
async fn navigation_moves_between_tabs_panes_and_workspaces_of_an_isolated_session() {
    let Some(mut herdr) = Isolated::new() else {
        return;
    };
    herdr.start();
    let host = LocalHost::new();
    let directory = Directory::new();
    let session = Some(herdr.name.as_str());
    let nav = async |pane_id: Option<&str>, nav: TargetNav| {
        navigate_in(&host, herdr.herdr(), &directory, session, pane_id, nav).await
    };

    // An empty session has nothing to move.
    nav(None, TargetNav::NextWindow).await.unwrap();
    nav(None, TargetNav::NextSession).await.unwrap();

    // Workspace A (focused): two panes side by side in its first tab, and a second tab.
    let created = herdr
        .call(RequestBody::WorkspaceCreate(WorkspaceCreateParams {
            cwd: Some("/tmp".into()),
            label: Some("or2-a".into()),
            focus: true,
            ..WorkspaceCreateParams::default()
        }))
        .await;
    let a = str_at(&created, "/workspace/workspace_id").to_owned();
    let a_tab = str_at(&created, "/tab/tab_id").to_owned();
    let left = str_at(&created, "/root_pane/pane_id").to_owned();
    let split = herdr
        .call(RequestBody::PaneSplit(PaneSplitParams {
            cwd: None,
            direction: SplitDirection::Right,
            env: HashMap::new(),
            focus: false,
            ratio: None,
            right_click: PaneRightClickTarget::Herdr,
            target_pane_id: Some(left.clone()),
            workspace_id: None,
        }))
        .await;
    let right = str_at(&split, "/pane/pane_id").to_owned();
    let tab = herdr
        .call(RequestBody::TabCreate(TabCreateParams {
            workspace_id: Some(a.clone()),
            label: Some("second".into()),
            focus: false,
            ..TabCreateParams::default()
        }))
        .await;
    let a_second = str_at(&tab, "/tab/tab_id").to_owned();
    // Workspace B, not focused.
    let created = herdr
        .call(RequestBody::WorkspaceCreate(WorkspaceCreateParams {
            cwd: Some("/tmp".into()),
            label: Some("or2-b".into()),
            focus: false,
            ..WorkspaceCreateParams::default()
        }))
        .await;
    let b = str_at(&created, "/workspace/workspace_id").to_owned();
    assert_eq!(
        focus_of(&herdr).await,
        (a.clone(), a_tab.clone(), left.clone())
    );

    // Panes: right, then left again; nothing further left is not an error.
    let pane = |direction| TargetNav::Pane { direction };
    nav(None, pane(NavDirection::Right)).await.unwrap();
    assert_eq!(focus_of(&herdr).await.2, right);
    nav(None, pane(NavDirection::Left)).await.unwrap();
    assert_eq!(focus_of(&herdr).await.2, left);
    nav(None, pane(NavDirection::Left)).await.unwrap();
    assert_eq!(focus_of(&herdr).await.2, left);
    // From a given pane, and from one that does not exist.
    nav(Some(&left), pane(NavDirection::Right)).await.unwrap();
    assert_eq!(focus_of(&herdr).await.2, right);
    assert_eq!(
        nav(Some("w99:p99"), pane(NavDirection::Left)).await,
        Err(HerdrError::PaneNotFound)
    );

    // Tabs of the focused workspace, wrapping around.
    nav(None, TargetNav::NextWindow).await.unwrap();
    assert_eq!(focus_of(&herdr).await.1, a_second);
    nav(None, TargetNav::NextWindow).await.unwrap();
    assert_eq!(focus_of(&herdr).await.1, a_tab);
    nav(None, TargetNav::PreviousWindow).await.unwrap();
    assert_eq!(focus_of(&herdr).await.1, a_second);

    // Workspaces, wrapping around.
    nav(None, TargetNav::NextSession).await.unwrap();
    assert_eq!(focus_of(&herdr).await.0, b);
    nav(None, TargetNav::NextSession).await.unwrap();
    assert_eq!(focus_of(&herdr).await.0, a);
    nav(None, TargetNav::PreviousSession).await.unwrap();
    assert_eq!(focus_of(&herdr).await.0, b);
    // B has one tab: a window move there has nowhere to go.
    nav(None, TargetNav::NextWindow).await.unwrap();
    assert_eq!(focus_of(&herdr).await.0, b);
}

fn tmux_binary() -> Option<PathBuf> {
    ["/usr/bin/tmux", "/usr/local/bin/tmux", "/bin/tmux"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

/// A herdr client attached to the isolated session inside a private tmux server (its own
/// socket, no config), so the test can type into the client as the terminal it runs in
/// would. Dropping kills that tmux server only.
struct Client {
    tmux: PathBuf,
    socket: PathBuf,
    _dir: tempfile::TempDir,
}

impl Client {
    fn attach(herdr: &Isolated, tmux: PathBuf) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("tmux.sock");
        let client = Self {
            tmux,
            socket,
            _dir: dir,
        };
        let status = client
            .command()
            .args([
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-x",
                "100",
                "-y",
                "30",
            ])
            .arg(herdr.herdr())
            .args(["--session", &herdr.name])
            .status()
            .unwrap();
        assert!(status.success(), "the private tmux server started");
        client
    }

    /// tmux on the private socket, without the `HERDR_*` variables of a pane the tests may
    /// run in (the client must reach only the isolated session) or an outer `TMUX`.
    fn command(&self) -> Command {
        let mut command = Command::new(&self.tmux);
        for (name, _) in std::env::vars_os() {
            let text = name.to_string_lossy();
            if text.starts_with("HERDR_") || text == "TMUX" {
                command.env_remove(&name);
            }
        }
        command.arg("-S").arg(&self.socket).stdin(Stdio::null());
        command
    }

    /// Bytes typed into the herdr client, as its terminal would send them.
    fn type_bytes(&self, bytes: &[u8]) {
        let status = self
            .command()
            .args(["send-keys", "-H"])
            .args(bytes.iter().map(|b| format!("{b:02x}")))
            .status()
            .unwrap();
        assert!(status.success());
    }

    /// What the herdr client shows.
    fn screen(&self) -> String {
        let output = self
            .command()
            .args(["capture-pane", "-p"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.command().arg("kill-server").output();
    }
}

/// The phone's route 1: herdr tracks the mouse, so a swipe reaches its client as wheel events
/// and herdr scrolls the pane itself. `TargetScroll::Bottom` (`pane.scroll` to offset 0) must
/// bring that pane, and what the client shows, back to the live screen.
#[tokio::test]
async fn bottom_returns_a_pane_herdr_scrolled_by_wheel_events_to_live() {
    use or2_core::herdr::generated::request::PaneListParams;
    use or2_core::herdr::{ScrollOffsets, scroll_pane_in};
    use or2_core::host::TargetScroll;
    let Some(mut herdr) = Isolated::new() else {
        return;
    };
    let Some(tmux) = tmux_binary() else {
        eprintln!("skipping: tmux is absent (it hosts the herdr client)");
        return;
    };
    herdr.start();
    let client = Client::attach(&herdr, tmux);
    // The attaching client creates the session's first workspace.
    let deadline = Instant::now() + Duration::from_secs(30);
    let pane = loop {
        let panes = herdr
            .call(RequestBody::PaneList(PaneListParams::default()))
            .await;
        if let Some(pane) = panes.pointer("/panes/0/pane_id").and_then(Value::as_str) {
            break pane.to_owned();
        }
        assert!(Instant::now() < deadline, "no pane: {panes}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    herdr
        .call(RequestBody::PaneSendText(PaneSendTextParams {
            pane_id: pane.clone(),
            text: "seq 1 500\n".into(),
        }))
        .await;
    let deadline = Instant::now() + Duration::from_secs(30);
    while !client.screen().contains("500") {
        assert!(Instant::now() < deadline, "no output: {}", client.screen());
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Wheel up over the pane (SGR; 1-based column 60, row 10, right of herdr's sidebar) until
    // the whole viewport is in the history.
    let deadline = Instant::now() + Duration::from_secs(30);
    while offset_from_bottom(&herdr, &pane).await < 40 {
        assert!(Instant::now() < deadline, "the wheel did not scroll herdr");
        client.type_bytes(&b"\x1b[<64;60;10M".repeat(5));
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(!client.screen().contains("500"), "{}", client.screen());

    // As the app sends it: the focused pane (no pane id), no offset kept for it.
    let (host, directory, offsets) = (LocalHost::new(), Directory::new(), ScrollOffsets::new());
    scroll_pane_in(
        &host,
        herdr.herdr(),
        &directory,
        &offsets,
        Some(herdr.name.as_str()),
        None,
        TargetScroll::Bottom,
    )
    .await
    .unwrap();
    assert_eq!(offset_from_bottom(&herdr, &pane).await, 0);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !client.screen().contains("500") {
        assert!(
            Instant::now() < deadline,
            "the client still shows history: {}",
            client.screen()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// What `pane` shows now, as plain text.
async fn screen_of(herdr: &Isolated, pane: &str) -> String {
    use or2_core::herdr::generated::request::{PaneReadParams, ReadFormat, ReadSource};
    let read = herdr
        .call(RequestBody::PaneRead(PaneReadParams {
            format: ReadFormat::Text,
            lines: None,
            pane_id: pane.to_owned(),
            source: ReadSource::Visible,
            strip_ansi: true,
        }))
        .await;
    str_at(&read, "/read/text").to_owned()
}

/// Waits until `pane` shows `marker` at least `times` times.
async fn shows(herdr: &Isolated, pane: &str, marker: &str, times: usize) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let screen = screen_of(herdr, pane).await;
        if screen.matches(marker).count() >= times {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{marker} not shown {times} times:\n{screen}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A workspace whose pane runs `cat` (a foreground process keeps an agent report, and shows what
/// reaches it), and its terminal id.
async fn cat_pane(herdr: &Isolated, label: &str) -> (String, String) {
    let created = herdr
        .call(RequestBody::WorkspaceCreate(WorkspaceCreateParams {
            cwd: Some("/tmp".into()),
            label: Some(label.into()),
            focus: true,
            ..WorkspaceCreateParams::default()
        }))
        .await;
    let pane = str_at(&created, "/root_pane/pane_id").to_owned();
    let terminal = str_at(&created, "/root_pane/terminal_id").to_owned();
    herdr
        .call(RequestBody::PaneSendText(PaneSendTextParams {
            pane_id: pane.clone(),
            text: "cat\n".into(),
        }))
        .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    (pane, terminal)
}

/// The reported agent `or2-test-agent` of `pane` is now in `state`.
fn report(pane: &str, seq: u64, state: PaneAgentState) -> RequestBody {
    RequestBody::PaneReportAgent(PaneReportAgentParams {
        agent: "or2-test-agent".into(),
        agent_session_id: None,
        agent_session_path: None,
        message: None,
        pane_id: pane.to_owned(),
        resume_argv: None,
        seq: Some(seq),
        source: "or2-test".into(),
        state,
    })
}

/// One reply with no cancellation and the query timeout's deadline.
async fn reply_to(
    herdr: &Isolated,
    directory: &Directory,
    pane: &str,
    agent: &or2_core::herdr::AgentIdentity,
    text: &str,
) -> Result<or2_core::herdr::ReplyRoute, HerdrError> {
    let reply = or2_core::herdr::Reply {
        session: Some(herdr.name.as_str()),
        pane_id: pane,
        agent,
        text,
    };
    or2_core::herdr::reply_in(
        &LocalHost::new(),
        herdr.herdr(),
        directory,
        reply,
        tokio::time::Instant::now() + Duration::from_secs(30),
        std::future::pending(),
    )
    .await
}

/// A reply from a notification reaches the agent pane's input and is submitted: herdr refuses
/// `agent.prompt` for a blocked agent (and for one it does not drive, as every reported agent),
/// so the text and Enter are typed in one request. `cat` in the pane echoes the typed line and
/// prints it again once Enter arrives. A pane without an agent, one that is gone, and a reply
/// meant for another agent or terminal get nothing.
#[tokio::test]
async fn a_reply_reaches_an_agent_panes_input_and_is_submitted() {
    use or2_core::herdr::{AgentIdentity, ReplyRoute};
    let Some(mut herdr) = Isolated::new() else {
        return;
    };
    herdr.start();
    let (pane, terminal) = cat_pane(&herdr, "or2-reply").await;
    let directory = Directory::new();
    let agent = AgentIdentity {
        terminal_id: terminal.clone(),
        agent: Some("or2-test-agent".into()),
    };

    herdr.call(report(&pane, 1, PaneAgentState::Blocked)).await;
    assert_eq!(
        reply_to(&herdr, &directory, &pane, &agent, "or2-reply-blocked").await,
        Ok(ReplyRoute::Typed)
    );
    // Typed (echoed by the terminal) and submitted (printed again by `cat`).
    shows(&herdr, &pane, "or2-reply-blocked", 2).await;

    herdr.call(report(&pane, 2, PaneAgentState::Working)).await;
    assert_eq!(
        reply_to(&herdr, &directory, &pane, &agent, "or2-reply-working").await,
        Ok(ReplyRoute::Typed),
        "herdr does not drive a reported agent: agent_not_ready, so typed"
    );
    shows(&herdr, &pane, "or2-reply-working", 2).await;

    // A stale notification: another terminal under the same pane id, or another kind of agent.
    for stale in [
        AgentIdentity {
            terminal_id: "term_0".into(),
            agent: agent.agent.clone(),
        },
        AgentIdentity {
            terminal_id: terminal.clone(),
            agent: Some("codex".into()),
        },
    ] {
        assert_eq!(
            reply_to(&herdr, &directory, &pane, &stale, "or2-reply-stale").await,
            Err(HerdrError::PaneNotFound)
        );
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!screen_of(&herdr, &pane).await.contains("or2-reply-stale"));

    // A shell pane without an agent: nothing is typed, so a reply never runs as a command.
    let split = herdr
        .call(RequestBody::PaneSplit(PaneSplitParams {
            cwd: None,
            direction: SplitDirection::Right,
            env: HashMap::new(),
            focus: false,
            ratio: None,
            right_click: PaneRightClickTarget::Herdr,
            target_pane_id: Some(pane.clone()),
            workspace_id: None,
        }))
        .await;
    let shell = str_at(&split, "/pane/pane_id").to_owned();
    let shell_agent = AgentIdentity {
        terminal_id: str_at(&split, "/pane/terminal_id").to_owned(),
        agent: None,
    };
    assert_eq!(
        reply_to(&herdr, &directory, &shell, &shell_agent, "or2-reply-shell").await,
        Err(HerdrError::PaneNotFound)
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!screen_of(&herdr, &shell).await.contains("or2-reply-shell"));
    // A pane that does not exist.
    assert_eq!(
        reply_to(&herdr, &directory, "w9:p9", &agent, "or2-reply-gone").await,
        Err(HerdrError::PaneNotFound)
    );
}

/// The agent exits and its pane's shell has the foreground again, while herdr still reports the
/// agent (herdr 0.9.3 does for about half a second): a reply must not run as a shell command. Its
/// prompt is refused (`agent_not_ready`), and the typed path's foreground check refuses the rest.
#[tokio::test]
async fn a_reply_never_runs_in_the_shell_after_the_agent_exits() {
    use or2_core::herdr::AgentIdentity;
    use or2_core::herdr::generated::request::{
        AgentTarget, PaneProcessInfoParams, PaneSendKeysParams,
    };
    let Some(mut herdr) = Isolated::new() else {
        return;
    };
    herdr.start();
    let socket = herdr.socket().expect("running");
    let call = async |body: RequestBody| {
        wire::call(
            &LocalHost::new(),
            &socket,
            "test",
            &body,
            Duration::from_secs(10),
        )
        .await
    };
    // How long herdr keeps an agent varies (its detection polls): an attempt whose reply came after
    // herdr had dropped the agent did not test the window, so another pane tries again.
    for attempt in 0..8 {
        let (pane, terminal) = cat_pane(&herdr, &format!("or2-reply-exit-{attempt}")).await;
        herdr.call(report(&pane, 1, PaneAgentState::Blocked)).await;
        let agent = AgentIdentity {
            terminal_id: terminal,
            agent: Some("or2-test-agent".into()),
        };
        // The reply's directory knows the socket already (a reply for another agent: nothing
        // sent), and every request below goes straight to it: the window is short.
        let directory = Directory::new();
        let stale = AgentIdentity {
            terminal_id: "term_0".into(),
            agent: None,
        };
        assert_eq!(
            reply_to(&herdr, &directory, &pane, &stale, "or2-reply-stale").await,
            Err(HerdrError::PaneNotFound)
        );
        // The agent exits; wait until the shell leads the foreground again.
        call(RequestBody::PaneSendKeys(PaneSendKeysParams {
            keys: vec!["ctrl+c".into()],
            pane_id: pane.clone(),
        }))
        .await
        .expect("ctrl+c");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let info = call(RequestBody::PaneProcessInfo(PaneProcessInfoParams {
                pane_id: Some(pane.clone()),
            }))
            .await
            .expect("process info");
            let info = &info["process_info"];
            if info["foreground_process_group_id"] == info["shell_pid"] {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the shell never got the foreground"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let result = reply_to(
            &herdr,
            &directory,
            &pane,
            &agent,
            "echo or2-ran-in-the-shell-$((6*7))",
        )
        .await;
        // herdr still reported the agent after the reply, so the reply saw it too.
        let reported = call(RequestBody::AgentGet(AgentTarget {
            target: pane.clone(),
        }))
        .await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let screen = screen_of(&herdr, &pane).await;
        assert!(
            !screen.contains("or2-ran-in-the-shell"),
            "the reply reached the shell ({result:?}):\n{screen}"
        );
        assert_eq!(result, Err(HerdrError::PaneNotFound));
        if reported.is_ok() {
            return;
        }
        eprintln!("attempt {attempt}: herdr dropped the agent before the reply ended; again");
    }
    panic!("herdr never still reported the exited agent when the reply ran");
}
