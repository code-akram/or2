//! Test doubles for the herdr client: sanitized fixtures and a scripted [`FakeHost`] that plays
//! a herdr server over in-memory streams. Deterministic under tokio's paused clock.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::Value;
use tokio::io::{DuplexStream, duplex};
use tokio::sync::{Notify, mpsc};
use tokio::time::Instant;

use super::wire::LineReader;
use crate::remote::{ExecOutput, RemoteError, RemoteHost};

/// A file from `src/herdr/fixtures/`: captured from isolated test sessions of herdr 0.9.3
/// (home paths and names replaced), or hand-written from the schema where noted in
/// `watch_tests.rs`.
pub(super) fn fixture(name: &str) -> String {
    let path = format!("{}/src/herdr/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

/// What the fake server received, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Served {
    /// `events.subscribe` with this many lifecycle subscriptions and these panes'
    /// `pane.agent_status_changed`.
    Subscribe {
        lifecycle: usize,
        panes: Vec<String>,
    },
    Snapshot,
    Focus(String),
    /// `tab.focus` of a tab id (it shares `pane.focus`'s failure and gate).
    TabFocus(String),
    /// `pane.scroll` of `pane_id` to `offset` rows from the bottom, as requested.
    Scroll {
        pane_id: String,
        offset: u64,
    },
    /// `agent.prompt` of `target` with `text`.
    Prompt {
        target: String,
        text: String,
    },
    /// `agent.get` of `target`.
    AgentGet {
        target: String,
    },
    /// `pane.process_info` of `pane_id`.
    ProcessInfo {
        pane_id: String,
    },
    /// `pane.send_input` of `text` and `keys` to `pane_id`, received `at` (tokio's clock).
    SendInput {
        pane_id: String,
        text: Option<String>,
        keys: Vec<String>,
        at: Instant,
    },
    /// `pane.read` of `pane_id`, with its parameters as sent.
    Read {
        pane_id: String,
        params: Value,
    },
    Other(String),
}

/// What the fake herdr reports of one pane through `agent.get` and `pane.process_info`.
#[derive(Debug, Clone)]
pub(super) struct FakeAgent {
    /// The agent's terminal and kind; `None`: no agent in the pane (`agent_not_found`).
    pub agent: Option<(String, Option<String>)>,
    /// The agent's `agent_session`, as `(kind, value)`.
    pub session: Option<(String, String)>,
    /// herdr's name for an agent it started.
    pub name: Option<String>,
    pub shell_pid: Option<u32>,
    pub foreground_group: Option<u32>,
    /// The foreground group's processes, as `(pid, name)`.
    pub foreground: Vec<(u32, String)>,
}

impl FakeAgent {
    /// The shell's pid, and its own process group.
    pub const SHELL: u32 = 100;

    /// A `claude` agent in the foreground, on terminal `term_<pane>`, with the session
    /// `sess_<pane>` (its hooks reported it, as Claude Code's do).
    pub fn default_for(pane: &str) -> Self {
        Self {
            session: Some(("id".to_owned(), format!("sess_{pane}"))),
            ..Self::running(&format!("term_{pane}"), Some("claude"), "claude")
        }
    }

    /// An agent of `kind` on `terminal`, its process `process` leading the foreground group, with
    /// no session and no name.
    pub fn running(terminal: &str, kind: Option<&str>, process: &str) -> Self {
        Self {
            agent: Some((terminal.to_owned(), kind.map(str::to_owned))),
            session: None,
            name: None,
            shell_pid: Some(Self::SHELL),
            foreground_group: Some(200),
            foreground: vec![(200, process.to_owned())],
        }
    }

    /// The agent left: the shell has the foreground again, though herdr may still report it.
    pub fn at_shell(self) -> Self {
        Self {
            foreground_group: Some(Self::SHELL),
            foreground: vec![(Self::SHELL, "zsh".to_owned())],
            ..self
        }
    }

    /// `agent.get`'s agent, trimmed to what this build reads plus herdr's required fields.
    fn info(&self, pane: &str) -> Value {
        let (terminal, kind) = self.agent.clone().unwrap_or_default();
        let mut info = serde_json::json!({
            "agent": kind, "agent_status": "blocked", "focused": false, "pane_id": pane,
            "revision": 0, "state_change_seq": 1, "tab_id": "w1:t1", "terminal_id": terminal,
            "workspace_id": "w1",
        });
        // Shaped like herdr 0.9.3's (an isolated session, a hook's report).
        if let Some((session_kind, value)) = &self.session {
            let session_agent = kind.clone().unwrap_or_default();
            let source = format!("herdr:{session_agent}");
            info["agent_session"] = serde_json::json!({
                "agent": session_agent, "kind": session_kind, "source": source, "value": value,
            });
        }
        if let Some(name) = &self.name {
            info["name"] = name.as_str().into();
            info["interactive_ready"] = true.into();
        }
        info
    }

    fn process_info(&self, pane: &str) -> Value {
        let processes: Vec<Value> = self
            .foreground
            .iter()
            .map(|(pid, name)| serde_json::json!({"pid": pid, "name": name}))
            .collect();
        serde_json::json!({
            "pane_id": pane, "shell_pid": self.shell_pid,
            "foreground_process_group_id": self.foreground_group,
            "foreground_processes": processes,
        })
    }
}

/// One scripted `session.snapshot` answer.
#[derive(Clone, Default)]
pub(super) struct Step {
    pub reply: String,
    /// Lines pushed to every event stream just before the reply, so they reach the watch while
    /// it is reading: invalidations during a read.
    pub events_before: Vec<String>,
    /// Holds the reply until notified.
    pub gate: Option<Arc<Notify>>,
}

impl Step {
    pub fn reply(reply: &str) -> Self {
        Self {
            reply: reply.trim_end().to_owned(),
            ..Self::default()
        }
    }
}

enum Command {
    Line(String),
    Close,
}

struct State {
    exec: Result<ExecOutput, RemoteError>,
    exec_log: Vec<String>,
    /// How long each exec takes (a slow host).
    exec_delay: std::time::Duration,
    open_error: Option<RemoteError>,
    /// Sockets nothing listens on: opening one is `Io`, as a stale path is over OpenSSH.
    dead_sockets: Vec<String>,
    opened: Vec<String>,
    snapshots: VecDeque<Step>,
    /// Per `events.subscribe`: `Some((code, message))` rejects it and closes the stream.
    subscribe_script: VecDeque<Option<(String, String)>>,
    focus_error: Option<(String, String)>,
    /// A focus to hold after its request was recorded: how many focuses to let pass first.
    focus_gate: Option<(usize, Arc<Notify>, Arc<Notify>)>,
    /// The history of every pane, in rows: `pane.scroll` answers with the offset clamped to it.
    scroll_max: u64,
    scroll_error: Option<(String, String)>,
    /// The `pane.current` answer: the focused pane and its offset.
    current: (String, u64),
    /// Where each pane is, as `pane.get` reports it: set by `pane.scroll` (clamped) or by a test.
    pane_offsets: HashMap<String, u64>,
    /// `agent.prompt` answers with this error from now on.
    prompt_error: Option<(String, String)>,
    /// `pane.send_input` answers with this error from now on.
    send_input_error: Option<(String, String)>,
    /// What `agent.get` and `pane.process_info` report of each pane; a pane not here has an agent
    /// ([`FakeAgent::default_for`]).
    agents: HashMap<String, FakeAgent>,
    /// Once a request of this method was served, every pane's agent leaves the foreground to
    /// the shell (herdr still reports it for a while, as herdr 0.9.3 does).
    exit_after: Option<String>,
    /// The agents left the foreground ([`Self::exit_after`]).
    exited: bool,
    /// Holds the answer to the next request of this method: `entered` is notified once herdr
    /// recorded it, and the answer goes when `release` is.
    hold: Option<(String, Arc<Notify>, Arc<Notify>)>,
    streams: Vec<mpsc::UnboundedSender<Command>>,
    served: Vec<Served>,
    /// What `pane.read` answers per pane: its text and herdr's `truncated`. A pane not here is
    /// `pane_not_found`.
    histories: HashMap<String, (String, bool)>,
}

/// A [`RemoteHost`] whose `exec` returns scripted output and whose sockets are served by a fake
/// herdr. Clones share the script.
#[derive(Clone)]
pub(super) struct FakeHost {
    state: Arc<Mutex<State>>,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

impl FakeHost {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                exec: Ok(output(0, "", "")),
                exec_log: Vec::new(),
                exec_delay: std::time::Duration::ZERO,
                open_error: None,
                dead_sockets: Vec::new(),
                opened: Vec::new(),
                snapshots: VecDeque::new(),
                subscribe_script: VecDeque::new(),
                focus_error: None,
                focus_gate: None,
                scroll_max: 1000,
                scroll_error: None,
                current: ("w1:p1".into(), 0),
                pane_offsets: HashMap::new(),
                prompt_error: None,
                send_input_error: None,
                agents: HashMap::new(),
                exit_after: None,
                exited: false,
                hold: None,
                streams: Vec::new(),
                served: Vec::new(),
                histories: HashMap::new(),
            })),
        }
    }

    /// `session list --json` answers with `json`.
    pub fn set_listing(&self, json: &str) {
        self.set_exec(0, json, "");
    }

    pub fn set_exec(&self, status: u32, stdout: &str, stderr: &str) {
        lock(&self.state).exec = Ok(output(status, stdout, stderr));
    }

    /// Every exec from now on takes `delay` (a slow host).
    pub fn set_exec_delay(&self, delay: std::time::Duration) {
        lock(&self.state).exec_delay = delay;
    }

    pub fn set_exec_error(&self, error: RemoteError) {
        lock(&self.state).exec = Err(error);
    }

    pub fn clear_exec_log(&self) {
        lock(&self.state).exec_log.clear();
    }

    pub fn clear_focus_error(&self) {
        lock(&self.state).focus_error = None;
    }

    pub fn exec_log(&self) -> Vec<String> {
        lock(&self.state).exec_log.clone()
    }

    pub fn set_open_error(&self, error: Option<RemoteError>) {
        lock(&self.state).open_error = error;
    }

    /// Opening `path` fails like a socket nothing listens on (other paths are unaffected).
    pub fn kill_socket(&self, path: &str) {
        lock(&self.state).dead_sockets.push(path.to_owned());
    }

    /// Sockets opened so far.
    pub fn opened(&self) -> Vec<String> {
        lock(&self.state).opened.clone()
    }

    /// Queues a snapshot answer. The last queued answer repeats forever.
    pub fn snapshot(&self, reply: &str) {
        self.step(Step::reply(reply));
    }

    pub fn step(&self, step: Step) {
        lock(&self.state).snapshots.push_back(step);
    }

    /// Replaces the queued answers: the first read after this gets `steps[0]`, and the last
    /// one repeats.
    pub fn script_snapshots(&self, steps: Vec<Step>) {
        lock(&self.state).snapshots = steps.into();
    }

    /// Scripts the next `events.subscribe` requests, one entry each (`None` accepts); further
    /// ones are accepted.
    pub fn script_subscribes(&self, script: Vec<Option<(&str, &str)>>) {
        lock(&self.state).subscribe_script = script
            .into_iter()
            .map(|entry| entry.map(|(code, message)| (code.to_owned(), message.to_owned())))
            .collect();
    }

    /// Holds the reply of the next `pane.focus` (after herdr recorded it): `entered` is
    /// notified, and the reply is sent when `release` is.
    pub fn hold_next_focus(&self, entered: Arc<Notify>, release: Arc<Notify>) {
        self.hold_focus_after(0, entered, release);
    }

    /// As [`Self::hold_next_focus`], for the focus after `skip` others.
    pub fn hold_focus_after(&self, skip: usize, entered: Arc<Notify>, release: Arc<Notify>) {
        lock(&self.state).focus_gate = Some((skip, entered, release));
    }

    pub fn fail_focus(&self, code: &str, message: &str) {
        lock(&self.state).focus_error = Some((code.to_owned(), message.to_owned()));
    }

    /// Every pane's history is `rows` long: `pane.scroll` stops there.
    pub fn set_scroll_max(&self, rows: u64) {
        lock(&self.state).scroll_max = rows;
    }

    /// `pane.scroll` answers with this error from now on.
    pub fn fail_scroll(&self, code: &str, message: &str) {
        lock(&self.state).scroll_error = Some((code.to_owned(), message.to_owned()));
    }

    /// `agent.prompt` answers with this error from now on (`agent_blocked`, ...).
    pub fn fail_prompt(&self, code: &str, message: &str) {
        lock(&self.state).prompt_error = Some((code.to_owned(), message.to_owned()));
    }

    /// `pane.send_input` answers with this error from now on.
    pub fn fail_send_input(&self, code: &str, message: &str) {
        lock(&self.state).send_input_error = Some((code.to_owned(), message.to_owned()));
    }

    /// What herdr reports of `pane_id` from now on.
    pub fn set_agent(&self, pane_id: &str, agent: FakeAgent) {
        lock(&self.state).agents.insert(pane_id.to_owned(), agent);
    }

    /// Once a request of `method` was served, every agent leaves the foreground to its shell.
    pub fn exit_agents_after(&self, method: &str) {
        lock(&self.state).exit_after = Some(method.to_owned());
    }

    /// Holds the answer to the next request of `method`: `entered` is notified once it was
    /// recorded, and it is answered when `release` is.
    pub fn hold_next(&self, method: &str, entered: Arc<Notify>, release: Arc<Notify>) {
        lock(&self.state).hold = Some((method.to_owned(), entered, release));
    }

    /// `pane_id` is now `offset` rows above its bottom, as new output moves a scrolled pane.
    pub fn set_pane_offset(&self, pane_id: &str, offset: u64) {
        lock(&self.state)
            .pane_offsets
            .insert(pane_id.to_owned(), offset);
    }

    /// `pane.read` of `pane_id` answers `text`, with herdr's `truncated`.
    pub fn set_history(&self, pane_id: &str, text: &str, truncated: bool) {
        lock(&self.state)
            .histories
            .insert(pane_id.to_owned(), (text.to_owned(), truncated));
    }

    /// `pane.current` names `pane_id`, scrolled `offset` rows above its bottom.
    pub fn set_current(&self, pane_id: &str, offset: u64) {
        lock(&self.state).current = (pane_id.to_owned(), offset);
    }

    /// Pushes a line to every open event stream.
    pub fn emit(&self, line: &str) {
        emit_to_streams(&self.state, line.trim_end());
    }

    /// Ends every event stream from the server side, as a dropped connection.
    pub fn close_events(&self) {
        for stream in lock(&self.state).streams.drain(..) {
            let _ = stream.send(Command::Close);
        }
    }

    pub fn served(&self) -> Vec<Served> {
        lock(&self.state).served.clone()
    }

    pub fn snapshots_served(&self) -> usize {
        self.served()
            .iter()
            .filter(|served| **served == Served::Snapshot)
            .count()
    }
}

fn output(status: u32, stdout: &str, stderr: &str) -> ExecOutput {
    ExecOutput {
        status: Some(status),
        stdout: stdout.as_bytes().into(),
        stderr: stderr.as_bytes().into(),
    }
}

impl RemoteHost for FakeHost {
    type Stream = DuplexStream;

    async fn exec_rendered(&self, line: &str) -> Result<ExecOutput, RemoteError> {
        let (result, delay) = {
            let mut state = lock(&self.state);
            state.exec_log.push(line.to_owned());
            (state.exec.clone(), state.exec_delay)
        };
        tokio::time::sleep(delay).await;
        result
    }

    async fn open_unix(&self, path: &str) -> Result<DuplexStream, RemoteError> {
        {
            let mut state = lock(&self.state);
            state.opened.push(path.to_owned());
            if let Some(error) = state.open_error.clone() {
                return Err(error);
            }
            if state.dead_sockets.iter().any(|dead| dead == path) {
                return Err(RemoteError::Io(
                    "the socket could not be connected to on the host".into(),
                ));
            }
        }
        let (client, server) = duplex(1 << 20);
        tokio::spawn(serve(Arc::clone(&self.state), server));
        Ok(client)
    }
}

fn emit_to_streams(state: &Mutex<State>, line: &str) {
    lock(state)
        .streams
        .retain(|stream| stream.send(Command::Line(line.to_owned())).is_ok());
}

/// One connection: reads the first request and answers by method.
async fn serve(state: Arc<Mutex<State>>, stream: DuplexStream) {
    let mut conn = LineReader::new(stream);
    let Ok(Some(line)) = conn.next_line().await else {
        return;
    };
    let request: Value = serde_json::from_slice(&line).expect("the client sends JSON");
    let id = request["id"].as_str().unwrap_or("").to_owned();
    let method = request["method"].as_str().unwrap_or("").to_owned();
    let error = |code: &str, message: &str| {
        serde_json::json!({"id": id, "error": {"code": code, "message": message}}).to_string()
            + "\n"
    };
    match method.as_str() {
        "events.subscribe" => {
            let subscriptions = request["params"]["subscriptions"]
                .as_array()
                .expect("subscriptions");
            let is_pane = |s: &&Value| s["type"] == "pane.agent_status_changed";
            let panes: Vec<String> = subscriptions
                .iter()
                .filter(is_pane)
                .map(|s| s["pane_id"].as_str().expect("pane_id").to_owned())
                .collect();
            let lifecycle = subscriptions.len() - panes.len();
            let (tx, mut rx) = mpsc::unbounded_channel();
            let rejection = {
                let mut state = lock(&state);
                state.served.push(Served::Subscribe { lifecycle, panes });
                let rejection = state.subscribe_script.pop_front().flatten();
                if rejection.is_none() {
                    state.streams.push(tx);
                }
                rejection
            };
            if let Some((code, message)) = rejection {
                let _ = conn.send(error(&code, &message).as_bytes()).await;
                return;
            }
            let ack =
                format!("{{\"id\":{id:?},\"result\":{{\"type\":\"subscription_started\"}}}}\n");
            if conn.send(ack.as_bytes()).await.is_err() {
                return;
            }
            while let Some(Command::Line(line)) = rx.recv().await {
                if conn.send(format!("{line}\n").as_bytes()).await.is_err() {
                    return;
                }
            }
        }
        "session.snapshot" => {
            let step = {
                let mut state = lock(&state);
                state.served.push(Served::Snapshot);
                if state.snapshots.len() > 1 {
                    state.snapshots.pop_front().expect("one queued")
                } else {
                    let last = state.snapshots.front().expect("a snapshot is scripted");
                    Step {
                        events_before: Vec::new(),
                        ..last.clone()
                    }
                }
            };
            if let Some(gate) = step.gate {
                gate.notified().await;
            }
            for line in &step.events_before {
                emit_to_streams(&state, line);
            }
            let _ = conn.send(format!("{}\n", step.reply).as_bytes()).await;
        }
        "pane.focus" | "tab.focus" => {
            let served = if method == "tab.focus" {
                Served::TabFocus(
                    request["params"]["tab_id"]
                        .as_str()
                        .unwrap_or("")
                        .to_owned(),
                )
            } else {
                Served::Focus(
                    request["params"]["pane_id"]
                        .as_str()
                        .unwrap_or("")
                        .to_owned(),
                )
            };
            let (failure, gate) = {
                let mut state = lock(&state);
                state.served.push(served);
                let gate = match state.focus_gate.take() {
                    Some((0, entered, release)) => Some((entered, release)),
                    Some((skip, entered, release)) => {
                        state.focus_gate = Some((skip - 1, entered, release));
                        None
                    }
                    None => None,
                };
                (state.focus_error.clone(), gate)
            };
            if let Some((entered, release)) = gate {
                entered.notify_one();
                release.notified().await;
            }
            let reply = match failure {
                Some((code, message)) => error(&code, &message),
                None => format!("{{\"id\":{id:?},\"result\":{{\"type\":\"ok\"}}}}\n"),
            };
            let _ = conn.send(reply.as_bytes()).await;
        }
        // A `pane_info`/`pane_current` answer shaped like herdr 0.9.3's (captured from an
        // isolated session), trimmed to the fields a scroll reads.
        "pane.scroll" | "pane.current" | "pane.get" => {
            let pane_info = |pane_id: &str, offset: u64, max: u64, kind: &str| {
                serde_json::json!({"id": id, "result": {"type": kind, "pane": {
                    "agent_status": "unknown", "focused": true, "pane_id": pane_id,
                    "revision": 0, "tab_id": "w1:t1", "terminal_id": "term_1",
                    "workspace_id": "w1",
                    "scroll": {"max_offset_from_bottom": max,
                        "offset_from_bottom": offset.min(max), "viewport_rows": 40},
                }}})
                .to_string()
                    + "\n"
            };
            let reply = {
                let mut state = lock(&state);
                if method == "pane.current" {
                    state.served.push(Served::Other(method.clone()));
                    let (pane, offset) = state.current.clone();
                    pane_info(&pane, offset, state.scroll_max, "pane_current")
                } else if method == "pane.get" {
                    state.served.push(Served::Other(method.clone()));
                    let pane = request["params"]["pane_id"]
                        .as_str()
                        .unwrap_or("")
                        .to_owned();
                    match state.scroll_error.clone() {
                        Some((code, message)) if code == "pane_not_found" => error(&code, &message),
                        _ => {
                            let offset = state.pane_offsets.get(&pane).copied().unwrap_or(0);
                            pane_info(&pane, offset, state.scroll_max, "pane_info")
                        }
                    }
                } else {
                    let pane = request["params"]["pane_id"].as_str().unwrap_or("");
                    let offset = request["params"]["offset_from_bottom"]
                        .as_u64()
                        .expect("offset_from_bottom");
                    state.served.push(Served::Scroll {
                        pane_id: pane.to_owned(),
                        offset,
                    });
                    match state.scroll_error.clone() {
                        Some((code, message)) => error(&code, &message),
                        None => {
                            let clamped = offset.min(state.scroll_max);
                            state.pane_offsets.insert(pane.to_owned(), clamped);
                            pane_info(pane, offset, state.scroll_max, "pane_info")
                        }
                    }
                }
            };
            let _ = conn.send(reply.as_bytes()).await;
        }
        // Shaped like herdr 0.9.3's answers (captured from an isolated session): `agent.prompt`
        // with `agent_prompted`, `agent.get` with `agent_info`, `pane.process_info` with
        // `pane_process_info`, `pane.send_input` with `ok`. A pane without an agent is
        // `agent_not_found` to the agent requests, as in herdr.
        "agent.prompt" | "agent.get" | "pane.process_info" | "pane.send_input" => {
            let params = &request["params"];
            let string = |name: &str| params[name].as_str().unwrap_or("").to_owned();
            let pane = match method.as_str() {
                "agent.prompt" | "agent.get" => string("target"),
                _ => string("pane_id"),
            };
            let (reply, gate) = {
                let mut state = lock(&state);
                let fake = state
                    .agents
                    .get(&pane)
                    .cloned()
                    .unwrap_or_else(|| FakeAgent::default_for(&pane));
                let fake = if state.exited { fake.at_shell() } else { fake };
                let no_agent = || {
                    Some((
                        "agent_not_found".to_owned(),
                        format!("agent target {pane} not found"),
                    ))
                };
                let (served, failure, result) = match method.as_str() {
                    "agent.prompt" => (
                        Served::Prompt {
                            target: pane.clone(),
                            text: string("text"),
                        },
                        match &fake.agent {
                            None => no_agent(),
                            Some(_) => state.prompt_error.clone(),
                        },
                        serde_json::json!({"type": "agent_prompted", "agent": fake.info(&pane)}),
                    ),
                    "agent.get" => (
                        Served::AgentGet {
                            target: pane.clone(),
                        },
                        fake.agent.is_none().then(no_agent).flatten(),
                        serde_json::json!({"type": "agent_info", "agent": fake.info(&pane)}),
                    ),
                    "pane.process_info" => (
                        Served::ProcessInfo {
                            pane_id: pane.clone(),
                        },
                        None,
                        serde_json::json!({"type": "pane_process_info",
                            "process_info": fake.process_info(&pane)}),
                    ),
                    _ => (
                        Served::SendInput {
                            pane_id: pane.clone(),
                            text: params["text"].as_str().map(str::to_owned),
                            keys: params["keys"]
                                .as_array()
                                .map(|keys| {
                                    keys.iter()
                                        .map(|key| key.as_str().expect("a key name").to_owned())
                                        .collect()
                                })
                                .unwrap_or_default(),
                            at: Instant::now(),
                        },
                        state.send_input_error.clone(),
                        serde_json::json!({"type": "ok"}),
                    ),
                };
                state.served.push(served);
                if state.exit_after.as_deref() == Some(method.as_str()) {
                    state.exited = true;
                }
                let gate = match state.hold.take() {
                    Some((held, entered, release)) if held == method => Some((entered, release)),
                    other => {
                        state.hold = other;
                        None
                    }
                };
                let reply = match failure {
                    Some((code, message)) => error(&code, &message),
                    None => serde_json::json!({"id": id, "result": result}).to_string() + "\n",
                };
                (reply, gate)
            };
            if let Some((entered, release)) = gate {
                entered.notify_one();
                release.notified().await;
            }
            let _ = conn.send(reply.as_bytes()).await;
        }
        // A `pane_read` answer shaped like the schema's `PaneReadResult`.
        "pane.read" => {
            let params = request["params"].clone();
            let pane = params["pane_id"].as_str().unwrap_or("").to_owned();
            let reply = {
                let mut state = lock(&state);
                state.served.push(Served::Read {
                    pane_id: pane.clone(),
                    params: params.clone(),
                });
                match state.histories.get(&pane) {
                    Some((text, truncated)) => {
                        serde_json::json!({"id": id, "result": {
                        "type": "pane_read", "read": {
                            "format": "text", "pane_id": pane, "revision": 1,
                            "source": "recent", "tab_id": "w1:t1", "text": text,
                            "truncated": truncated, "workspace_id": "w1",
                        }}})
                        .to_string()
                            + "\n"
                    }
                    None => error("pane_not_found", &format!("pane {pane} not found")),
                }
            };
            let _ = conn.send(reply.as_bytes()).await;
        }
        other => {
            lock(&state).served.push(Served::Other(other.to_owned()));
            let _ = conn
                .send(error("invalid_request", "unknown method").as_bytes())
                .await;
        }
    }
}
