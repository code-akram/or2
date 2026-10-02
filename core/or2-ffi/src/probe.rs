//! Contract probe: a test fixture, not a connection.
//!
//! `contract_probe_session` validates a real `ConnectRequest` and returns a real `Session`
//! whose driver is a deterministic script on a Rust thread instead of an SSH connection. It
//! presents a host key generated once per process, uses the production trust check against
//! the request's trusted keys, and renders fixed cells plus echoes of the input it receives.
//! JVM and device tests use it to exercise listener threading, lifecycle, host-key decisions,
//! frames and input across the real FFI. App code must never call it.
//!
//! `contract_probe_host` does the same for a host connection (API 4): see its documentation.
//! It also serves `TerminalTransport::Mosh` terminals (API 8 to 10) deterministically.

use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};

use or2_core::frame::{
    Cell, CellStyle, CellWidth, Cursor, CursorShape, Frame, Rgb, Row, Scrollback, Underline,
};
use or2_core::herdr::{
    Agent, AgentStatus, HerdrState, HerdrView, HerdrWatchDriver, Pane, Tab, Workspace,
};
use or2_core::host::TerminalTransport as CoreTransport;
use or2_core::host::{
    self as core_host, HerdrSessionInfo, HostCapabilities, HostCommand, HostDriver, HostState,
    TmuxSession,
};
use or2_core::input::{ViewportScroll, text_bytes};
use or2_core::keys::ClientKey;
use or2_core::mosh::LinkHealth;
use or2_core::session::{
    self as core, CloseReason, Command, HostKeyPrompt, SessionDriver, SessionFailure, SessionState,
};
use or2_core::submit::submit_text_bytes;
use or2_core::term::TerminalSize;
use or2_core::trust::{self, HostKey, HostKeyVerdict};
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::host::{
    HostConnectError, HostConnectRequest, HostConnection, HostListener, HostListenerObserver,
};
use crate::session::{
    ConnectError, ConnectRequest, ListenerObserver, Session, SessionListener, TerminalTransport,
};

const FOREGROUND: Rgb = Rgb::new(0xd0, 0xd0, 0xd0);
const BACKGROUND: Rgb = Rgb::new(0x10, 0x10, 0x18);
const HISTORY_ROWS: u64 = 100;
const BANNER: &str = "or2 contract probe";
/// What a probe mosh terminal reports through `on_link_health`, in order, right after its first
/// frame: healthy, stale (past the app's 5 s grey-out threshold), recovered.
const MOSH_HEALTH_SEQUENCE: [LinkHealth; 3] = [
    LinkHealth {
        since_heard_ms: 300,
        since_ack_ms: 300,
    },
    LinkHealth {
        since_heard_ms: 6_000,
        since_ack_ms: 9_000,
    },
    LinkHealth {
        since_heard_ms: 400,
        since_ack_ms: 400,
    },
];

/// Test fixture only; see the module documentation. Never connects to anything.
#[uniffi::export]
pub fn contract_probe_session(
    request: ConnectRequest,
    listener: Box<dyn SessionListener>,
) -> Result<Arc<Session>, ConnectError> {
    let request = request.validate()?;
    let (handle, driver) = core::channel(Arc::new(ListenerObserver(listener)));
    spawn_probe_thread("or2-contract-probe", async move {
        run_session(&request.trusted_host_keys, request.size, driver).await;
    });
    Ok(Session::new(handle, TerminalTransport::Ssh))
}

/// Runs a probe script on its own thread with a single-threaded runtime, so every callback it
/// makes comes from a Rust-owned thread, one at a time.
fn spawn_probe_thread(name: &str, script: impl Future<Output = ()> + Send + 'static) {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("building the probe runtime")
                .block_on(script);
        })
        .expect("spawning the probe thread");
}

fn probe_host_key() -> &'static HostKey {
    static KEY: OnceLock<HostKey> = OnceLock::new();
    KEY.get_or_init(|| {
        HostKey::from_openssh(&ClientKey::generate_ed25519("").public_key().openssh)
            .expect("a generated public key parses")
    })
}

fn untrusted_prompt(trusted: &[HostKey]) -> Option<HostKeyPrompt> {
    let presented = probe_host_key();
    (trust::verify(presented, trusted) != HostKeyVerdict::Trusted).then(|| HostKeyPrompt {
        presented: presented.clone(),
        previously_trusted: trusted.to_vec(),
    })
}

async fn run_session(trusted: &[HostKey], mut size: TerminalSize, mut driver: SessionDriver) {
    if let Some(prompt) = untrusted_prompt(trusted) {
        driver
            .transition(SessionState::AwaitingHostKey(prompt))
            .expect("Connecting -> AwaitingHostKey");
        loop {
            match driver.next_command().await {
                Command::ApproveHostKey { fingerprint }
                    if fingerprint == probe_host_key().fingerprint() =>
                {
                    break;
                }
                Command::RejectHostKey => {
                    return driver.close(CloseReason::Failed(SessionFailure::HostKeyRejected));
                }
                Command::Disconnect => return driver.close(CloseReason::Disconnected),
                Command::Resize(new_size) => size = new_size,
                _ => {}
            }
        }
    }
    driver
        .transition(SessionState::Authenticating)
        .expect("-> Authenticating");
    driver
        .transition(SessionState::Connected)
        .expect("-> Connected");
    // Nothing but its own commands ever stops a standalone session.
    let (_keep_open, stop) = watch::channel(None);
    serve_terminal(driver, size, BANNER.into(), false, stop).await;
}

/// Serves one connected terminal session: fixed cells plus echoes of the input it receives.
/// A `mosh` terminal also reports the fixed link-health sequence after its first frame and
/// counts `Roam` commands in the echo row. Ends on `Disconnect`, or when `stop` carries the
/// reason the host closed.
async fn serve_terminal(
    mut driver: SessionDriver,
    size: TerminalSize,
    title: String,
    mosh: bool,
    mut stop: watch::Receiver<Option<CloseReason>>,
) {
    let mut screen = Screen::new(size, title);
    publish(&mut driver, screen.full());
    if mosh {
        for health in MOSH_HEALTH_SEQUENCE {
            driver.publish_link_health(health);
        }
    }
    loop {
        let command = tokio::select! {
            command = driver.next_command() => command,
            _ = stop.changed() => {
                let reason = stop.borrow().clone().unwrap_or(CloseReason::Disconnected);
                return driver.close(reason);
            }
        };
        match command {
            Command::Resize(new_size) => {
                screen.size = new_size;
                publish(&mut driver, screen.full());
            }
            Command::Text(text) => {
                let hex: Vec<String> = text_bytes(&text)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                screen.text_echo = format!("text {}", hex.join(" "));
                publish(&mut driver, screen.delta(2));
            }
            Command::Submit(text) => {
                // The probe terminal never turns bracketed paste on, so the text is typed;
                // the Enter is the separate second write.
                let typed: String = submit_text_bytes(&text, false)
                    .iter()
                    .map(|b| format!(" {b:02x}"))
                    .collect();
                screen.text_echo = format!("submit{typed} | 0d");
                publish(&mut driver, screen.delta(2));
            }
            Command::Key(key) => {
                let m = key.modifiers();
                let modifiers: String = [
                    (m.shift, "+shift"),
                    (m.ctrl, "+ctrl"),
                    (m.alt, "+alt"),
                    (m.meta, "+meta"),
                ]
                .into_iter()
                .filter_map(|(on, name)| on.then_some(name))
                .collect();
                screen.key_echo = format!("key {:?}{modifiers}", key.key());
                publish(&mut driver, screen.delta(3));
            }
            Command::Scroll(scroll) => {
                screen.history_offset = match scroll {
                    ViewportScroll::Top => 0,
                    ViewportScroll::Bottom => HISTORY_ROWS,
                    ViewportScroll::Delta(rows) | ViewportScroll::Wheel { rows, .. } => screen
                        .history_offset
                        .saturating_add_signed(i64::from(rows))
                        .min(HISTORY_ROWS),
                };
                publish(&mut driver, screen.delta_without_rows());
            }
            Command::MouseClick { column, row } => {
                // The probe terminal tracks no mouse; it echoes the cell so the FFI is visible.
                screen.key_echo = format!("click {column} {row}");
                publish(&mut driver, screen.delta(3));
            }
            Command::FullFrame => publish(&mut driver, screen.full()),
            Command::Roam => {
                if mosh {
                    screen.roams += 1;
                    publish(&mut driver, screen.delta(2));
                }
            }
            Command::Disconnect => return driver.close(CloseReason::Disconnected),
            Command::ApproveHostKey { .. } | Command::RejectHostKey => {}
        }
        if matches!(driver.state(), SessionState::Closed(_)) {
            return;
        }
    }
}

fn publish(driver: &mut SessionDriver, frame: Frame) {
    if let Err(error) = driver.publish(frame) {
        driver.close(CloseReason::Failed(SessionFailure::Internal(
            error.to_string(),
        )));
    }
}

/// Test fixture only; see the module documentation. Never connects to anything.
///
/// The host uses the production trust check against the request's trusted keys with the same
/// per-process host key as `contract_probe_session`, then reports `Connected { 0 }`.
/// `capabilities` (which reports a `mosh-server`), `mosh_server` (its path) and `list_tmux_sessions` return fixed data. `focus_herdr_pane` succeeds for the
/// probe view's panes (`w1:p1`, `w1:p2`, `w2:p1`) and is `PaneNotFound` for any other id; the
/// focused pane then shows as `focused` in the views of watches started afterwards. `open_terminal` returns a
/// session served by the M1 probe script without host-key states (`Connecting` to
/// `Connected`; row 0 names the target). With `TerminalTransport::Mosh` the terminal behaves the
/// same, plus: after its first frame `on_link_health` receives three values in order,
/// (300, 300), (6000, 9000) and (400, 400) ms for (`since_heard_ms`, `since_ack_ms`); and each
/// `Session.roam()` (or `network_changed()`) is counted in row 2, the echo row: `roams N`
/// alone, or after the latest text echo as `text 61 | roams N`. SSH probe terminals never
/// report health and ignore `roam()`. `watch_herdr` goes `Live`, updates once and closes on
/// `stop()`. `navigate` (API 14) succeeds for every tmux and herdr move, except one from a
/// `pane_id` the probe view does not have (`PaneNotFound`). Closing the host closes its
/// terminals and watches first.
#[uniffi::export]
pub fn contract_probe_host(
    request: HostConnectRequest,
    listener: Box<dyn HostListener>,
) -> Result<Arc<HostConnection>, HostConnectError> {
    let request = request.validate()?;
    let (handle, driver) = core_host::channel(Arc::new(HostListenerObserver(listener)));
    spawn_probe_thread("or2-contract-probe-host", async move {
        run_host(&request.trusted_host_keys, driver).await;
    });
    Ok(HostConnection::new(handle))
}

fn probe_capabilities() -> HostCapabilities {
    HostCapabilities {
        tmux: Some("/usr/bin/tmux".into()),
        herdr: Some("/home/probe/.local/bin/herdr".into()),
        mosh_server: Some("/usr/bin/mosh-server".into()),
        tmux_records_clients: true,
        utf8_locale: "C.UTF-8".into(),
        herdr_sessions: vec![
            HerdrSessionInfo {
                name: "default".into(),
                running: true,
                is_default: true,
            },
            HerdrSessionInfo {
                name: "or2-probe".into(),
                running: false,
                is_default: false,
            },
        ],
    }
}

/// Most recently active first, as `list_tmux_sessions` promises.
fn probe_tmux_sessions() -> Vec<TmuxSession> {
    [
        ("main", 3, 1, 1_700_000_000, 1_700_003_600),
        ("build", 1, 0, 1_700_000_500, 1_700_001_000),
    ]
    .into_iter()
    .map(
        |(name, windows, attached_clients, created_unix, activity_unix)| TmuxSession {
            name: name.into(),
            windows,
            attached_clients,
            created_unix,
            activity_unix,
        },
    )
    .collect()
}

fn target_title(target: &core_host::TerminalTarget) -> String {
    use core_host::TerminalTarget as T;
    match target {
        T::Shell => format!("{BANNER} shell"),
        T::Tmux { session_name } => format!("{BANNER} tmux {session_name}"),
        T::Herdr { session, pane_id } => format!(
            "{BANNER} herdr {} {}",
            session.as_deref().unwrap_or("default"),
            pane_id.as_deref().unwrap_or("-")
        ),
    }
}

async fn run_host(trusted: &[HostKey], mut driver: HostDriver) {
    if let Some(prompt) = untrusted_prompt(trusted) {
        driver
            .transition(HostState::AwaitingHostKey(prompt))
            .expect("Connecting -> AwaitingHostKey");
        loop {
            match driver.next_command().await {
                HostCommand::ApproveHostKey { fingerprint }
                    if fingerprint == probe_host_key().fingerprint() =>
                {
                    break;
                }
                HostCommand::RejectHostKey => {
                    return driver.close(CloseReason::Failed(SessionFailure::HostKeyRejected));
                }
                HostCommand::Disconnect => return driver.close(CloseReason::Disconnected),
                _ => {}
            }
        }
    }
    driver
        .transition(HostState::Authenticating)
        .expect("-> Authenticating");
    driver
        .transition(HostState::Connected { address_index: 0 })
        .expect("-> Connected");

    let (stop_sender, stop) = watch::channel(None);
    let mut tasks = JoinSet::new();
    let focused = Arc::new(Mutex::new(PROBE_PANES[0].to_owned()));
    loop {
        match driver.next_command().await {
            HostCommand::Capabilities { reply } => {
                let _ = reply.send(Ok(probe_capabilities()));
            }
            HostCommand::MoshServer { reply } => {
                let _ = reply.send(Ok(probe_capabilities().mosh_server));
            }
            HostCommand::ListTmux { reply } => {
                let _ = reply.send(Ok(probe_tmux_sessions()));
            }
            HostCommand::OpenTerminal {
                target,
                transport,
                size,
                driver: session,
                ..
            } => {
                tasks.spawn(run_terminal(
                    session,
                    size,
                    target_title(&target),
                    transport == CoreTransport::Mosh,
                    stop.clone(),
                ));
            }
            HostCommand::FocusHerdrPane { pane_id, reply, .. } => {
                let _ = reply.send(if PROBE_PANES.contains(&pane_id.as_str()) {
                    *focused.lock().unwrap() = pane_id;
                    Ok(())
                } else {
                    Err(core_host::HostError::PaneNotFound)
                });
            }
            HostCommand::StopMoshServer { pid, reply } => {
                // A stop that cannot run, for tests of the caller keeping the pid.
                let _ = reply.send(if pid == PROBE_UNSTOPPABLE_PID {
                    Err(core_host::HostError::CommandFailed {
                        message: "the host did not answer in time".into(),
                    })
                } else {
                    Ok(())
                });
            }
            HostCommand::ScrollTarget { reply, .. } => {
                let _ = reply.send(Ok(()));
            }
            HostCommand::Navigate { pane_id, reply, .. } => {
                // Every move succeeds, except one from a herdr pane the probe does not have.
                let _ = reply.send(match pane_id {
                    Some(pane) if !PROBE_PANES.contains(&pane.as_str()) => {
                        Err(core_host::HostError::PaneNotFound)
                    }
                    _ => Ok(()),
                });
            }
            HostCommand::WatchHerdr {
                session,
                driver: watcher,
            } => {
                let focus = focused.lock().unwrap().clone();
                tasks.spawn(run_herdr_watch(watcher, session, focus, stop.clone()));
            }
            HostCommand::Disconnect => break,
            HostCommand::ApproveHostKey { .. } | HostCommand::RejectHostKey => {}
        }
    }
    // Terminals and watches close first, then the host.
    let _ = stop_sender.send(Some(CloseReason::Disconnected));
    while tasks.join_next().await.is_some() {}
    driver.close(CloseReason::Disconnected);
}

async fn run_terminal(
    mut driver: SessionDriver,
    size: TerminalSize,
    title: String,
    mosh: bool,
    stop: watch::Receiver<Option<CloseReason>>,
) {
    if mosh {
        driver.set_server_pid(Some(PROBE_SERVER_PID));
    }
    driver
        .transition(SessionState::Connected)
        .expect("Connecting -> Connected");
    serve_terminal(driver, size, title, mosh, stop).await;
}

async fn run_herdr_watch(
    mut driver: HerdrWatchDriver,
    session: Option<String>,
    focus: String,
    mut stop: watch::Receiver<Option<CloseReason>>,
) {
    let label = session.unwrap_or_else(|| "default".into());
    for view in [
        probe_view(&label, 1, false, &focus),
        probe_view(&label, 2, true, &focus),
    ] {
        driver
            .transition(HerdrState::Live { view })
            .expect("-> Live");
    }
    tokio::select! {
        () = driver.stopped() => {}
        _ = stop.changed() => {}
    }
    driver.close();
}

/// The panes of [`probe_view`]; the first is focused until `focus_herdr_pane` moves it.
/// The `mosh-server` pid every probe mosh terminal reports (`Session.server_pid`).
pub const PROBE_SERVER_PID: u32 = 4242;
/// A pid whose `stop_mosh_server` fails on a probe host, whatever else is true.
pub const PROBE_UNSTOPPABLE_PID: u32 = 13;
const PROBE_PANES: [&str; 3] = ["w1:p1", "w1:p2", "w2:p1"];

/// One blocked, one working and one idle agent; `resolved` turns the blocked one into working;
/// `focus` is the focused pane.
fn probe_view(label: &str, version: u64, resolved: bool, focus: &str) -> HerdrView {
    let first = if resolved {
        AgentStatus::Working
    } else {
        AgentStatus::Blocked
    };
    let agent = |pane: &str, name: &str, status, seq| {
        let (workspace, _) = pane.split_once(':').expect("pane ids are workspace:pane");
        Agent {
            pane_id: pane.into(),
            tab_id: format!("{workspace}:t1"),
            workspace_id: workspace.into(),
            name: Some(name.into()),
            agent: Some(name.into()),
            display_agent: Some(name.to_uppercase()),
            status,
            cwd: Some(format!("/home/probe/{name}")),
            title: None,
            focused: pane == focus,
            state_change_seq: seq,
        }
    };
    let pane = |a: &Agent| Pane {
        pane_id: a.pane_id.clone(),
        tab_id: a.tab_id.clone(),
        workspace_id: a.workspace_id.clone(),
        label: None,
        agent: a.agent.clone(),
        agent_status: a.status,
        cwd: a.cwd.clone(),
        title: None,
        focused: a.focused,
    };
    let agents = vec![
        agent("w1:p1", "claude", first, if resolved { 5 } else { 4 }),
        agent("w1:p2", "codex", AgentStatus::Working, 2),
        agent("w2:p1", "pi", AgentStatus::Idle, 1),
    ];
    HerdrView {
        version,
        protocol: 22,
        focused_pane_id: Some(focus.into()),
        workspaces: vec![
            Workspace {
                workspace_id: "w1".into(),
                number: 1,
                label: label.into(),
                focused: true,
                agent_status: first,
            },
            Workspace {
                workspace_id: "w2".into(),
                number: 2,
                label: "scratch".into(),
                focused: false,
                agent_status: AgentStatus::Idle,
            },
        ],
        tabs: vec![
            Tab {
                tab_id: "w1:t1".into(),
                workspace_id: "w1".into(),
                number: 1,
                label: "agents".into(),
                focused: true,
                agent_status: first,
            },
            Tab {
                tab_id: "w2:t1".into(),
                workspace_id: "w2".into(),
                number: 1,
                label: "shell".into(),
                focused: false,
                agent_status: AgentStatus::Idle,
            },
        ],
        panes: agents.iter().map(pane).collect(),
        agents,
    }
}

struct Screen {
    size: TerminalSize,
    title: String,
    text_echo: String,
    key_echo: String,
    /// `Roam` commands a mosh probe terminal has received; shown in the echo row.
    roams: u32,
    history_offset: u64,
}

impl Screen {
    fn new(size: TerminalSize, title: String) -> Self {
        Self {
            size,
            title,
            text_echo: String::new(),
            key_echo: String::new(),
            roams: 0,
            history_offset: HISTORY_ROWS,
        }
    }

    /// Row 2: the latest text echo, followed by `roams N` once a mosh terminal has roamed
    /// (`roams 1` alone when nothing was typed).
    fn echo_row(&self) -> String {
        match (self.text_echo.is_empty(), self.roams) {
            (_, 0) => self.text_echo.clone(),
            (true, roams) => format!("roams {roams}"),
            (false, roams) => format!("{} | roams {roams}", self.text_echo),
        }
    }

    fn plain() -> CellStyle {
        CellStyle::plain(FOREGROUND, BACKGROUND)
    }

    /// Row 1 holds the risky cells: styles, a combining mark, CJK and emoji wide cells.
    fn row(&self, index: u16) -> Row {
        let plain = Self::plain();
        let red_bold = CellStyle {
            bold: true,
            ..CellStyle::plain(Rgb::new(0xff, 0x33, 0x33), BACKGROUND)
        };
        let inverse = CellStyle::plain(BACKGROUND, FOREGROUND);
        let curly = CellStyle {
            underline: Underline::Curly,
            underline_color: Some(Rgb::new(0x33, 0x99, 0xff)),
            italic: true,
            ..plain
        };
        let graphemes: Vec<(String, bool, CellStyle)> = match index {
            0 => ascii(&self.title, plain),
            1 => vec![
                ("R".into(), false, red_bold),
                ("界".into(), true, plain),
                ("😀".into(), true, plain),
                ("e\u{301}".into(), false, curly),
                ("I".into(), false, inverse),
            ],
            2 => ascii(&self.echo_row(), plain),
            3 => ascii(&self.key_echo, plain),
            _ => Vec::new(),
        };
        let columns = usize::from(self.size.columns());
        let blank = Cell {
            text: String::new(),
            width: CellWidth::Narrow,
            style: plain,
        };
        let mut cells = Vec::with_capacity(columns);
        for (text, wide, style) in graphemes {
            let needed = if wide { 2 } else { 1 };
            if cells.len() + needed > columns {
                break;
            }
            if wide {
                cells.push(Cell {
                    text,
                    width: CellWidth::Wide,
                    style,
                });
                cells.push(Cell {
                    text: String::new(),
                    width: CellWidth::SpacerTail,
                    style,
                });
            } else {
                cells.push(Cell {
                    text,
                    width: CellWidth::Narrow,
                    style,
                });
            }
        }
        cells.resize(columns, blank);
        Row::new(index, false, cells)
    }

    fn cursor(&self) -> Option<Cursor> {
        // On the wide CJK head at row 1, column 1, when it fits.
        (self.size.rows() > 1 && self.size.columns() >= 3).then_some(Cursor {
            column: 1,
            row: 1,
            wide: true,
            shape: CursorShape::Bar,
            blinking: true,
            color: Rgb::new(0xff, 0xcc, 0x00),
        })
    }

    fn scrollback(&self) -> Scrollback {
        Scrollback {
            total_rows: HISTORY_ROWS + u64::from(self.size.rows()),
            offset: self.history_offset,
        }
    }

    fn full(&self) -> Frame {
        let rows = (0..self.size.rows()).map(|index| self.row(index)).collect();
        Frame::full(
            self.size,
            rows,
            self.cursor(),
            BACKGROUND,
            self.scrollback(),
        )
        .expect("probe rows fit the viewport")
    }

    fn delta(&self, index: u16) -> Frame {
        let rows = if index < self.size.rows() {
            vec![self.row(index)]
        } else {
            Vec::new()
        };
        Frame::delta(
            self.size,
            rows,
            self.cursor(),
            BACKGROUND,
            self.scrollback(),
        )
        .expect("probe rows fit the viewport")
    }

    fn delta_without_rows(&self) -> Frame {
        Frame::delta(
            self.size,
            Vec::new(),
            self.cursor(),
            BACKGROUND,
            self.scrollback(),
        )
        .expect("probe cursor fits the viewport")
    }
}

fn ascii(text: &str, style: CellStyle) -> Vec<(String, bool, CellStyle)> {
    text.chars()
        .map(|c| (c.to_string(), false, style))
        .collect()
}
