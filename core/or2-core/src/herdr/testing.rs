//! Test doubles for the herdr client: sanitized fixtures and a scripted [`FakeHost`] that plays
//! a herdr server over in-memory streams. Deterministic under tokio's paused clock.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::Value;
use tokio::io::{DuplexStream, duplex};
use tokio::sync::{Notify, mpsc};

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
    Other(String),
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
    open_error: Option<RemoteError>,
    opened: Vec<String>,
    snapshots: VecDeque<Step>,
    /// Per `events.subscribe`: `Some((code, message))` rejects it and closes the stream.
    subscribe_script: VecDeque<Option<(String, String)>>,
    focus_error: Option<(String, String)>,
    streams: Vec<mpsc::UnboundedSender<Command>>,
    served: Vec<Served>,
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
                open_error: None,
                opened: Vec::new(),
                snapshots: VecDeque::new(),
                subscribe_script: VecDeque::new(),
                focus_error: None,
                streams: Vec::new(),
                served: Vec::new(),
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

    pub fn set_exec_error(&self, error: RemoteError) {
        lock(&self.state).exec = Err(error);
    }

    pub fn exec_log(&self) -> Vec<String> {
        lock(&self.state).exec_log.clone()
    }

    pub fn set_open_error(&self, error: Option<RemoteError>) {
        lock(&self.state).open_error = error;
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

    pub fn fail_focus(&self, code: &str, message: &str) {
        lock(&self.state).focus_error = Some((code.to_owned(), message.to_owned()));
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
        let mut state = lock(&self.state);
        state.exec_log.push(line.to_owned());
        state.exec.clone()
    }

    async fn open_unix(&self, path: &str) -> Result<DuplexStream, RemoteError> {
        {
            let mut state = lock(&self.state);
            state.opened.push(path.to_owned());
            if let Some(error) = state.open_error.clone() {
                return Err(error);
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
        "pane.focus" => {
            let pane = request["params"]["pane_id"]
                .as_str()
                .unwrap_or("")
                .to_owned();
            let failure = {
                let mut state = lock(&state);
                state.served.push(Served::Focus(pane.clone()));
                state.focus_error.clone()
            };
            let reply = match failure {
                Some((code, message)) => error(&code, &message),
                None => format!("{{\"id\":{id:?},\"result\":{{\"type\":\"ok\"}}}}\n"),
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
