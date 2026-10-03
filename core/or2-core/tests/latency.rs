//! What the app's critical paths cost over a slow link: a real disposable OpenSSH behind a relay
//! that holds every chunk 60 ms each way (a 120 ms round trip, the phone's measured RTT to its
//! host), a scripted herdr (a fake `herdr` that lists one running session, and a real Unix
//! socket server that speaks herdr's wire format), and a real `mosh-server`.
//!
//! The paths are measured the way the app runs them: connect, then the capability query
//! followed by the default session's watch (the inbox), then an agent tap (pane focus plus a
//! mosh terminal on that pane) and a reuse (focus only). Besides timing, the server side counts
//! what reached it, so the protocol work is asserted exactly (the number of `herdr session
//! list` runs, of herdr connections and requests) and the timings only with generous bounds.
//! Run with `-- --nocapture` to read the table. The UDP leg of a mosh start is local here
//! (one more round trip on a real link).
//!
//! Nothing here touches a real herdr, tmux or ssh configuration.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use common::{MoshReaper, Proxy, Sshd, TestUdp, mosh_ready, sshd_ready};
use or2_core::herdr::{HerdrObserver, HerdrState};
use or2_core::host::{
    HostConnectRequest, HostHandle, HostObserver, HostState, TerminalTarget, TerminalTransport,
};
use or2_core::keys::ClientKey;
use or2_core::session::{CloseReason, SessionFailure, SessionObserver, SessionState};
use or2_core::ssh::{HostOptions, connect_host_with};
use or2_core::term::TerminalSize;
use or2_core::transport::DirectTcp;

/// One way, so a round trip costs 120 ms.
const ONE_WAY: Duration = Duration::from_millis(60);
const RTT: Duration = Duration::from_millis(120);
const WAIT: Duration = Duration::from_secs(20);

macro_rules! require {
    () => {
        if !(sshd_ready() && mosh_ready()) {
            return;
        }
    };
}

/// What the fake herdr server saw: one entry per request line, `method` or `connect`.
type Served = Arc<Mutex<Vec<String>>>;

/// A herdr socket server speaking the wire format: `events.subscribe` is acknowledged and the
/// stream held open, `session.snapshot` answers the checked-in two-pane snapshot, `pane.focus`
/// succeeds (or `pane_not_found` for the pane `w9:p9`).
fn serve_herdr(socket: &Path, focus_delay: Duration) -> Served {
    let listener = UnixListener::bind(socket).unwrap();
    let served = Served::default();
    let log = served.clone();
    let snapshot: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(format!(
            "{}/src/herdr/fixtures/snapshot_two_panes.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            log.lock().unwrap().push("connect".into());
            let (log, snapshot) = (log.clone(), snapshot.clone());
            std::thread::spawn(move || {
                let mut writer = stream.try_clone().unwrap();
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
                    let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                    let id = request["id"].clone();
                    let method = request["method"].as_str().unwrap_or("").to_owned();
                    log.lock().unwrap().push(method.clone());
                    if method == "pane.focus" && !focus_delay.is_zero() {
                        std::thread::sleep(focus_delay);
                    }
                    let reply = match method.as_str() {
                        "events.subscribe" => {
                            serde_json::json!({"id": id, "result": {"type": "subscription_started"}})
                        }
                        "session.snapshot" => {
                            let mut reply = snapshot.clone();
                            reply["id"] = id;
                            reply
                        }
                        "pane.focus" if request["params"]["pane_id"] == "w9:p9" => {
                            serde_json::json!({"id": id, "error": {"code": "pane_not_found", "message": "no such pane"}})
                        }
                        _ => serde_json::json!({"id": id, "result": {"type": "ok"}}),
                    };
                    if writeln!(writer, "{reply}").is_err() {
                        break;
                    }
                }
            });
        }
    });
    served
}

/// A fake `herdr` for the sshd sessions: `session list --json` logs the run and lists one
/// running default session at `socket`; anything else is the "client" and just stays alive.
fn install_herdr(sshd: &Sshd, socket: &Path) {
    let bin = sshd.home().join(".local/bin");
    fs::create_dir_all(&bin).unwrap();
    let path = bin.join("herdr");
    fs::write(
        &path,
        format!(
            r#"#!/bin/sh
if [ "$1" = session ] && [ "$2" = list ]; then
  echo list >> '{log}'
  echo '{{"sessions":[{{"default":true,"name":"default","running":true,"socket_path":"{socket}"}}]}}'
  exit 0
fi
exec sleep 600
"#,
            log = sshd.home().join("listings.log").display(),
            socket = socket.display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn listings(sshd: &Sshd) -> usize {
    fs::read_to_string(sshd.home().join("listings.log"))
        .map(|log| log.lines().count())
        .unwrap_or(0)
}

struct HostObs(mpsc::Sender<(HostState, Instant)>);

impl HostObserver for HostObs {
    fn state_changed(&self, state: &HostState) {
        let _ = self.0.send((state.clone(), Instant::now()));
    }
}

struct WatchObs(mpsc::Sender<(HerdrState, Instant)>);

impl HerdrObserver for WatchObs {
    fn state_changed(&self, state: &HerdrState) {
        let _ = self.0.send((state.clone(), Instant::now()));
    }
}

struct TermObs(mpsc::Sender<(SessionState, Instant)>);

impl SessionObserver for TermObs {
    fn state_changed(&self, state: &SessionState) {
        let _ = self.0.send((state.clone(), Instant::now()));
    }

    fn frame_ready(&self) {}
}

fn run<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

/// The fixture: sshd behind the 120 ms link, fake herdr and its socket server.
struct Rig {
    _reaper: MoshReaper,
    sshd: Sshd,
    proxy: Proxy,
    served: Served,
    key: ClientKey,
}

impl Rig {
    fn new() -> Self {
        Self::with_focus_delay(Duration::ZERO)
    }

    fn with_focus_delay(focus_delay: Duration) -> Self {
        let sshd = Sshd::new(false);
        let reaper = MoshReaper::new(&sshd);
        let key = ClientKey::generate_ed25519("");
        sshd.authorize(&key);
        let socket = sshd.home().join("herdr.sock");
        let served = serve_herdr(&socket, focus_delay);
        install_herdr(&sshd, &socket);
        let proxy = Proxy::new(sshd.port);
        proxy.slow(ONE_WAY);
        Self {
            _reaper: reaper,
            sshd,
            proxy,
            served,
            key,
        }
    }

    /// Connects through the slow link; returns the host and the time it took to be `Connected`.
    fn connect(&self) -> (HostHandle, Duration, Instant) {
        let (tx, states) = mpsc::channel();
        let started = Instant::now();
        let (host, _) = connect_host_with(
            Arc::new(DirectTcp),
            Arc::new(TestUdp::default()),
            HostConnectRequest::new(
                &[("127.0.0.1", self.proxy.port)],
                &Sshd::username(),
                &self.key.to_stored(),
                std::slice::from_ref(&self.sshd.host),
            )
            .unwrap(),
            Arc::new(HostObs(tx)),
            HostOptions::default(),
        );
        loop {
            let (state, at) = states.recv_timeout(WAIT).expect("a host state");
            if matches!(state, HostState::Connected { .. }) {
                return (host, at - started, at);
            }
        }
    }

    fn requests(&self, method: &str) -> usize {
        self.served
            .lock()
            .unwrap()
            .iter()
            .filter(|entry| *entry == method)
            .count()
    }
}

/// The first `Live` of a watch of the default session.
fn first_live(host: &HostHandle) -> Instant {
    let (tx, states) = mpsc::channel();
    let watch = host.watch_herdr(None, Arc::new(WatchObs(tx))).unwrap();
    let live = loop {
        let (state, at) = states.recv_timeout(WAIT).expect("a watch state");
        if matches!(state, HerdrState::Live { .. }) {
            break at;
        }
        assert!(
            !matches!(state, HerdrState::Closed | HerdrState::Unavailable { .. }),
            "{state:?}"
        );
    };
    // Keep the watch running for the rest of the test.
    std::mem::forget(watch);
    live
}

fn ms(duration: Duration) -> u128 {
    duration.as_millis()
}

fn rtts(duration: Duration) -> String {
    format!("{:.1} RTT", duration.as_secs_f64() / RTT.as_secs_f64())
}

#[test]
fn the_inbox_and_agent_paths_over_a_120_ms_link() {
    require!();
    let rig = Rig::new();
    let (host, connect, connected_at) = rig.connect();

    // The inbox, as the app runs it: the capability query, then the default session's watch.
    let started = Instant::now();
    let caps = run(host.capabilities()).expect("capabilities");
    let capabilities = started.elapsed();
    assert!(caps.herdr.is_some() && caps.mosh_server.is_some());
    let live = first_live(&host);
    let inbox = live - connected_at;
    let list_runs_after_inbox = listings(&rig.sshd);
    // Let the watch finish its own bootstrap (the per-pane subscription and its read).
    std::thread::sleep(Duration::from_secs(2));
    let settled_requests = rig.served.lock().unwrap().len();

    // Reuse: the terminal is open and only the pane is focused again.
    let before = rig.served.lock().unwrap().len();
    let started = Instant::now();
    run(host.focus_herdr_pane(None, "w2:p1".into())).expect("focus");
    let reuse = started.elapsed();
    let focus_requests = rig.served.lock().unwrap().len() - before;

    // An agent tap, the way the app ran it before: the pane is focused, and only then does a
    // mosh terminal open on it.
    let lists_before_tap = listings(&rig.sshd);
    let focuses_before_tap = rig.requests("pane.focus");
    let started = Instant::now();
    run(host.focus_herdr_pane(None, "w2:p2".into())).expect("focus");
    let focused = started.elapsed();
    let (terminal, tap) = open_on_pane(&host, "w2:p2", started);
    let lists_in_tap = listings(&rig.sshd) - lists_before_tap;
    let focuses_in_tap = rig.requests("pane.focus") - focuses_before_tap;
    terminal.disconnect();

    // The same tap with the focus and the open started together (the app now does this): the
    // terminal's own focus joins the one in flight.
    let focuses_before = rig.requests("pane.focus");
    let host = Arc::new(host);
    let started = Instant::now();
    let focusing = {
        let host = host.clone();
        std::thread::spawn(move || run(host.focus_herdr_pane(None, "w2:p1".into())))
    };
    let (terminal, together) = open_on_pane(&host, "w2:p1", started);
    focusing.join().unwrap().expect("focus");
    let focuses_together = rig.requests("pane.focus") - focuses_before;

    eprintln!("120 ms RTT link, real sshd, scripted herdr");
    eprintln!(
        "  connect (TCP, SSH handshake, auth)       {:>6} ms  {}",
        ms(connect),
        rtts(connect)
    );
    eprintln!(
        "  capabilities() after Connected           {:>6} ms  {}",
        ms(capabilities),
        rtts(capabilities)
    );
    eprintln!(
        "  inbox: Connected -> first Live           {:>6} ms  {}",
        ms(inbox),
        rtts(inbox)
    );
    eprintln!(
        "  reuse: focus only                        {:>6} ms  {}",
        ms(reuse),
        rtts(reuse)
    );
    eprintln!(
        "  tap: focus                               {:>6} ms  {}",
        ms(focused),
        rtts(focused)
    );
    eprintln!(
        "  tap: focus, then mosh terminal Connected {:>6} ms  {}",
        ms(tap),
        rtts(tap)
    );
    eprintln!(
        "  tap: focus and open together             {:>6} ms  {}",
        ms(together),
        rtts(together)
    );
    eprintln!(
        "  `herdr session list` runs: inbox {list_runs_after_inbox}, reuse 0, tap {lists_in_tap}; \
         herdr requests: watch bootstrap {settled_requests}, one focus {focus_requests}; \
         pane.focus requests: tap {focuses_in_tap}, together {focuses_together}"
    );

    // What reached herdr and the host, exactly: one listing for the whole inbox, none for a
    // focus or a tap; one `pane.focus` per tap whichever way the app starts it.
    assert_eq!(
        list_runs_after_inbox, 1,
        "the probe's listing is all a watch needs"
    );
    assert_eq!(
        focus_requests, 2,
        "a focus is one connection and one request"
    );
    assert_eq!((lists_in_tap, focuses_in_tap, focuses_together), (0, 1, 1));
    // And the time, with room for a loaded machine: a round trip is 120 ms.
    assert!(inbox < RTT * 10, "inbox {inbox:?}");
    assert!(reuse < RTT * 4, "reuse {reuse:?}");
    assert!(tap < RTT * 8, "tap {tap:?}");
    assert!(together < tap, "together {together:?} vs {tap:?}");

    terminal.disconnect();
    host.disconnect();
}

/// A mosh terminal on the default session's pane, opened `started` ago; returns it once
/// `Connected` and how long that took.
fn open_on_pane(
    host: &HostHandle,
    pane: &str,
    started: Instant,
) -> (or2_core::session::SessionHandle, Duration) {
    let (tx, states) = mpsc::channel();
    let terminal = host
        .open_terminal_within(
            TerminalTarget::Herdr {
                session: None,
                pane_id: Some(pane.into()),
            },
            TerminalTransport::Mosh,
            TerminalSize::new(80, 24).unwrap(),
            None,
            Arc::new(TermObs(tx)),
        )
        .unwrap();
    let connected = loop {
        let (state, at) = states.recv_timeout(WAIT).expect("a terminal state");
        if state == SessionState::Connected {
            break at;
        }
        assert!(
            !matches!(state, SessionState::Closed(_)),
            "the terminal closed: {state:?}"
        );
    };
    (terminal, connected - started)
}

#[test]
fn an_unreachable_host_never_delays_another_hosts_connection_or_inbox() {
    require!();
    // A host that accepts the TCP connection and never says a word: the SSH handshake hangs
    // until its 20 s connect timeout.
    let silent = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let silent_port = silent.local_addr().unwrap().port();
    let done = Arc::new(AtomicBool::new(false));
    let rig = Rig::new();

    let (tx, states) = mpsc::channel();
    let key = ClientKey::generate_ed25519("");
    let (stuck, _) = connect_host_with(
        Arc::new(DirectTcp),
        Arc::new(TestUdp::default()),
        HostConnectRequest::new(
            &[("127.0.0.1", silent_port)],
            &Sshd::username(),
            &key.to_stored(),
            &[],
        )
        .unwrap(),
        Arc::new(HostObs(tx)),
        HostOptions::default(),
    );
    // Make sure the stuck host really is stuck while the other one connects.
    let (healthy, connect, connected_at) = rig.connect();
    let _ = run(healthy.capabilities()).expect("capabilities");
    let live = first_live(&healthy);
    let inbox = live - connected_at;
    assert!(
        !states
            .try_iter()
            .any(|(state, _)| matches!(state, HostState::Connected { .. } | HostState::Closed(_))),
        "the silent host must still be connecting"
    );
    eprintln!(
        "silent host connecting: other host connect {} ms, inbox {} ms",
        ms(connect),
        ms(inbox)
    );
    assert!(connect < Duration::from_secs(5) && inbox < Duration::from_secs(5));
    done.store(true, Ordering::SeqCst);
    stuck.disconnect();
    healthy.disconnect();
    drop(silent);
}

#[test]
fn a_terminal_on_a_vanished_pane_fails_and_leaves_no_mosh_server_behind() {
    require!();
    let rig = Rig::new();
    let (host, _, _) = rig.connect();
    for transport in [TerminalTransport::Mosh, TerminalTransport::Ssh] {
        let (tx, states) = mpsc::channel();
        let terminal = host
            .open_terminal_within(
                TerminalTarget::Herdr {
                    session: None,
                    pane_id: Some("w9:p9".into()),
                },
                transport,
                TerminalSize::new(80, 24).unwrap(),
                None,
                Arc::new(TermObs(tx)),
            )
            .unwrap();
        // The focus runs beside the server's start (or the channel's open); its failure still
        // fails the terminal, which never reaches `Connected`.
        let closed = loop {
            let (state, _) = states.recv_timeout(WAIT).expect("a terminal state");
            match state {
                SessionState::Connected => panic!("{transport:?}: connected on a vanished pane"),
                SessionState::Closed(reason) => break reason,
                _ => {}
            }
        };
        assert!(
            matches!(
                closed,
                CloseReason::Failed(SessionFailure::CommandFailed(_))
            ),
            "{transport:?}: {closed:?}"
        );
        drop(terminal);
    }
    // The server the mosh start had already begun is stopped before the failure is reported.
    common::wait_until(WAIT, "the unused mosh-server to be stopped", || {
        common::fixture_processes(&rig.sshd, "mosh-server").is_empty()
    });
    host.disconnect();
}

#[test]
fn an_ssh_terminal_on_a_pane_connects_once_the_focus_and_the_channel_are_both_done() {
    require!();
    // Wall-clock bound under a loaded machine (the full workspace runs in parallel): load only
    // ever adds time, so the best of three tries is the one that says what the code does.
    let mut best = Duration::MAX;
    for _ in 0..3 {
        let connected = ssh_terminal_on_a_pane();
        best = best.min(connected);
        if best < RTT * 5 {
            break;
        }
    }
    // Focus and channel open share their round trips: the channel (open, pty, exec) is three
    // and the focus overlaps the first.
    assert!(best < RTT * 5, "{best:?}");
}

/// One connection, one SSH terminal on a herdr pane: how long until it is `Connected`.
fn ssh_terminal_on_a_pane() -> Duration {
    let rig = Rig::new();
    let (host, _, _) = rig.connect();
    let _ = run(host.capabilities()).expect("capabilities");
    let started = Instant::now();
    let (tx, states) = mpsc::channel();
    let terminal = host
        .open_terminal_within(
            TerminalTarget::Herdr {
                session: None,
                pane_id: Some("w2:p1".into()),
            },
            TerminalTransport::Ssh,
            TerminalSize::new(80, 24).unwrap(),
            None,
            Arc::new(TermObs(tx)),
        )
        .unwrap();
    let connected = loop {
        let (state, at) = states.recv_timeout(WAIT).expect("a terminal state");
        if state == SessionState::Connected {
            break at - started;
        }
        assert!(!matches!(state, SessionState::Closed(_)), "{state:?}");
    };
    eprintln!(
        "ssh terminal on a pane: {} ms ({})",
        ms(connected),
        rtts(connected)
    );
    assert_eq!(rig.requests("pane.focus"), 1);
    terminal.disconnect();
    host.disconnect();
    connected
}

/// The bootstrap has returned its server's pid while the pane focus is still pending (the
/// herdr server answers it only after eight seconds): giving the start up must still stop
/// that server, not lose the pid with the dropped focus.
fn focus_pending_rig() -> (Rig, HostHandle) {
    let rig = Rig::with_focus_delay(Duration::from_secs(8));
    let (host, _, _) = rig.connect();
    run(host.capabilities()).unwrap();
    (rig, host)
}

fn open_pending(
    host: &HostHandle,
    budget: Option<Duration>,
) -> (
    or2_core::session::SessionHandle,
    mpsc::Receiver<(SessionState, Instant)>,
) {
    let (tx, states) = mpsc::channel();
    let terminal = host
        .open_terminal_within(
            TerminalTarget::Herdr {
                session: None,
                pane_id: Some("w2:p1".into()),
            },
            TerminalTransport::Mosh,
            TerminalSize::new(80, 24).unwrap(),
            budget,
            Arc::new(TermObs(tx)),
        )
        .unwrap();
    (terminal, states)
}

#[test]
fn a_spent_budget_stops_a_finished_bootstrap_while_the_focus_is_pending() {
    require!();
    let (rig, host) = focus_pending_rig();
    let (_terminal, states) = open_pending(&host, Some(Duration::from_secs(1)));
    let (state, _) = states.recv_timeout(WAIT).unwrap();
    assert_eq!(
        state,
        SessionState::Closed(CloseReason::Failed(SessionFailure::TimedOut))
    );
    assert_eq!(rig.requests("pane.focus"), 1);
    let servers = common::fixture_processes(&rig.sshd, "mosh-server");
    assert!(
        servers.is_empty(),
        "the finished bootstrap's server was lost while the focus waited: {servers:?}"
    );
    host.disconnect();
}

#[test]
fn a_dismissed_start_stops_a_finished_bootstrap_while_the_focus_is_pending() {
    require!();
    let (rig, host) = focus_pending_rig();
    let (terminal, states) = open_pending(&host, None);
    // The bootstrap takes a few round trips; the focus holds for eight seconds.
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        !common::fixture_processes(&rig.sshd, "mosh-server").is_empty(),
        "the bootstrap has not started its server yet"
    );
    terminal.disconnect();
    let (state, _) = states.recv_timeout(WAIT).unwrap();
    assert_eq!(state, SessionState::Closed(CloseReason::Disconnected));
    let servers = common::fixture_processes(&rig.sshd, "mosh-server");
    assert!(
        servers.is_empty(),
        "the finished bootstrap's server was lost while the focus waited: {servers:?}"
    );
    host.disconnect();
}
