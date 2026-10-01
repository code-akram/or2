//! Mosh terminals on a host connection (M3), against a disposable loopback OpenSSH and a real
//! `mosh-server`: the bootstrap over the host's SSH connection, echo, resize, roaming, link
//! health, the target's command (shell, tmux, herdr), what losing or disconnecting the host
//! does to the sessions, and the cleanup of a server nobody reached (UDP blocked, disconnect
//! before connecting).
//!
//! Everything is disposable: the sshd, its keys, the private tmux socket directory and a fake
//! `herdr` live in a temporary directory, and the only `mosh-server` processes touched are the
//! ones this sshd's sessions started (found by the fixture's private `TMUX_TMPDIR` in their
//! environment and killed by exact pid, whatever happens to the test). Skips with a message
//! without `sshd`, `tmux` or `mosh-server`; `OR2_REQUIRE_SSHD`, `OR2_REQUIRE_TMUX` and
//! `OR2_REQUIRE_MOSH` turn the skips into failures.

mod common;

use std::fmt::Debug;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use common::{
    Grid, MoshReaper, Proxy, Sshd, TestUdp, fixture_processes, mosh_ready, sshd_ready, tmux_ready,
    wait_until,
};
use or2_core::host::{
    HostConnectRequest, HostError, HostHandle, HostObserver, HostState, TerminalTarget,
    TerminalTransport,
};
use or2_core::keys::ClientKey;
use or2_core::mosh::LinkHealth;
use or2_core::session::{
    CloseReason, SessionFailure, SessionHandle, SessionObserver, SessionState,
};
use or2_core::ssh::{HostOptions, connect_host_with_datagrams};
use or2_core::term::TerminalSize;
use or2_core::transport::DirectTcp;

/// Skips the test without sshd or mosh-server (failing when the matching `OR2_REQUIRE_*` is
/// set).
macro_rules! require {
    () => {
        if !(sshd_ready() && mosh_ready()) {
            return;
        }
    };
}

const WAIT: Duration = Duration::from_secs(15);

type Log = Arc<Mutex<Vec<String>>>;

fn word(value: &impl Debug) -> String {
    format!("{value:?}")
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default()
        .to_owned()
}

struct HostObs {
    tx: mpsc::Sender<HostState>,
    log: Log,
}

impl HostObserver for HostObs {
    fn state_changed(&self, state: &HostState) {
        self.log
            .lock()
            .unwrap()
            .push(format!("host:{}", word(state)));
        let _ = self.tx.send(state.clone());
    }
}

struct SessionObs {
    tag: String,
    tx: mpsc::Sender<SessionState>,
    log: Log,
    health: Arc<Mutex<Vec<LinkHealth>>>,
}

impl SessionObserver for SessionObs {
    fn state_changed(&self, state: &SessionState) {
        self.log
            .lock()
            .unwrap()
            .push(format!("{}:{}", self.tag, word(state)));
        let _ = self.tx.send(state.clone());
    }

    fn frame_ready(&self) {}

    fn link_health(&self, health: LinkHealth) {
        self.health.lock().unwrap().push(health);
    }
}

struct Term {
    handle: SessionHandle,
    states: mpsc::Receiver<SessionState>,
    grid: Grid,
    health: Arc<Mutex<Vec<LinkHealth>>>,
}

impl Term {
    fn next(&self) -> SessionState {
        self.states
            .recv_timeout(WAIT)
            .expect("a session state change")
    }

    fn closed(&self) -> CloseReason {
        let SessionState::Closed(reason) = self.next() else {
            panic!("expected Closed")
        };
        assert!(
            self.states
                .recv_timeout(Duration::from_millis(100))
                .is_err(),
            "Closed is delivered once, last"
        );
        reason
    }

    fn send(&self, text: &str) {
        self.handle.send_text(text.into()).unwrap();
    }

    fn wait(&mut self, expected: &str) {
        self.grid.wait(&self.handle, expected);
    }

    /// Silences the shell's echo and clears the screen, so later waits match output only.
    fn quiet(&mut self) {
        self.send(concat!(
            r"stty -echo; printf '\033[2J\033[H'; printf 'OR2-%s\n' READY",
            "\n"
        ));
        self.wait("OR2-READY");
    }
}

fn request(key: &ClientKey, port: u16, trusted: &str) -> HostConnectRequest {
    HostConnectRequest::new(
        &[("127.0.0.1", port)],
        &Sshd::username(),
        &key.to_stored(),
        &[trusted.to_owned()],
    )
    .unwrap()
}

/// A connected host on a private sshd, over a relay the test can cut, with the host key
/// pre-trusted.
struct Live {
    // Dropped first: kills the fixture's mosh-servers however the test ends.
    _reaper: MoshReaper,
    sshd: Sshd,
    proxy: Proxy,
    host: HostHandle,
    states: mpsc::Receiver<HostState>,
    log: Log,
    udp: TestUdp,
    slow: Option<SlowServer>,
}

/// A `mosh-server` in front of the real one on the sshd sessions' `PATH`: it records that it
/// was started, waits, then runs the real one. A bootstrap that takes a while.
struct SlowServer {
    directory: tempfile::TempDir,
}

impl SlowServer {
    fn new(wait: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let bin = directory.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let real = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .chain([PathBuf::from("/usr/bin")])
            .map(|dir| dir.join("mosh-server"))
            .find(|path| path.is_file())
            .expect("a real mosh-server");
        let script = bin.join("mosh-server");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\ntouch '{}'\nsleep {wait}\nexec '{}' \"$@\"\n",
                directory.path().join("started").display(),
                real.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        Self { directory }
    }

    /// The session environment that puts the wrapper first on `PATH`.
    fn environment(&self) -> String {
        format!(
            "PATH={}:{}",
            self.directory.path().join("bin").display(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
        )
    }

    fn started(&self) -> bool {
        self.directory.path().join("started").exists()
    }
}

impl Live {
    fn new() -> Self {
        Self::with(TestUdp::default(), HostOptions::default())
    }

    fn with(udp: TestUdp, options: HostOptions) -> Self {
        Self::build(udp, options, None)
    }

    /// A host whose `mosh-server` waits `wait` (a `sleep` argument) before it starts.
    fn with_slow_bootstrap(wait: &str) -> Self {
        Self::build(
            TestUdp::default(),
            HostOptions::default(),
            Some(SlowServer::new(wait)),
        )
    }

    fn build(udp: TestUdp, options: HostOptions, slow: Option<SlowServer>) -> Self {
        let sshd = match &slow {
            Some(slow) => Sshd::with_environment(false, "", &slow.environment()),
            None => Sshd::new(false),
        };
        let reaper = MoshReaper::new(&sshd);
        let key = ClientKey::generate_ed25519("");
        sshd.authorize(&key);
        let proxy = Proxy::new(sshd.port);
        let (tx, states) = mpsc::channel();
        let log = Log::default();
        let host = connect_host_with_datagrams(
            Arc::new(DirectTcp),
            Arc::new(udp.clone()),
            request(&key, proxy.port, &sshd.host),
            Arc::new(HostObs {
                tx,
                log: log.clone(),
            }),
            options,
        );
        assert_eq!(
            states.recv_timeout(WAIT).unwrap(),
            HostState::Authenticating
        );
        assert_eq!(
            states.recv_timeout(WAIT).unwrap(),
            HostState::Connected { address_index: 0 }
        );
        Self {
            _reaper: reaper,
            sshd,
            proxy,
            host,
            states,
            log,
            udp,
            slow,
        }
    }

    /// Opens a terminal and returns it without waiting for `Connected`.
    fn open_raw(
        &self,
        tag: &str,
        target: TerminalTarget,
        transport: TerminalTransport,
        size: (u16, u16),
    ) -> Term {
        let (tx, states) = mpsc::channel();
        let health = Arc::new(Mutex::new(Vec::new()));
        let handle = self
            .host
            .open_terminal_with(
                target,
                transport,
                TerminalSize::new(size.0, size.1).unwrap(),
                Arc::new(SessionObs {
                    tag: tag.into(),
                    tx,
                    log: self.log.clone(),
                    health: health.clone(),
                }),
            )
            .unwrap();
        Term {
            handle,
            states,
            grid: Grid::default(),
            health,
        }
    }

    fn open(&self, tag: &str, target: TerminalTarget, transport: TerminalTransport) -> Term {
        let term = self.open_raw(tag, target, transport, (80, 24));
        assert_eq!(term.next(), SessionState::Connected);
        term
    }

    fn mosh(&self, tag: &str, target: TerminalTarget) -> Term {
        self.open(tag, target, TerminalTransport::Mosh)
    }

    fn servers(&self) -> Vec<u32> {
        fixture_processes(&self.sshd, "mosh-server")
    }

    fn wait_no_servers(&self, why: &str) {
        wait_until(Duration::from_secs(10), why, || self.servers().is_empty());
    }

    fn host_closed(&self) -> CloseReason {
        let HostState::Closed(reason) = self.states.recv_timeout(WAIT).expect("a host state")
        else {
            panic!("expected the host to close")
        };
        assert!(
            self.states
                .recv_timeout(Duration::from_millis(100))
                .is_err(),
            "Closed is delivered once, last"
        );
        reason
    }
}

#[test]
fn a_mosh_terminal_echoes_resizes_roams_and_reports_health_through_a_real_host() {
    require!();
    let live = Live::new();
    let mut term = live.mosh("m", TerminalTarget::Shell);
    assert_eq!(
        live.servers().len(),
        1,
        "one mosh-server was started over SSH"
    );
    term.quiet();
    term.send("echo mosh-$((6*7))\n");
    term.wait("mosh-42");

    // A submit types the line and presses Enter separately.
    term.handle
        .submit_text("echo sub-$((6*9+1))".into())
        .unwrap();
    term.wait("sub-55");

    // A resize reaches the PTY: `stty size` reports rows then columns.
    term.handle
        .resize(TerminalSize::new(100, 30).unwrap())
        .unwrap();
    term.send("stty size\n");
    term.wait("30 100");

    // Link health arrives after Connected, about once a second, and a healthy link is fresh.
    wait_until(Duration::from_secs(5), "link health", || {
        !term.health.lock().unwrap().is_empty()
    });
    let first = term.health.lock().unwrap()[0];
    assert!(first.since_heard_ms < 5000, "{first:?}");

    // Roaming: a new socket opens, output keeps flowing and moves to it.
    assert_eq!(live.udp.sockets.lock().unwrap().len(), 1);
    term.handle.roam();
    wait_until(Duration::from_secs(10), "the second socket", || {
        live.udp.sockets.lock().unwrap().len() >= 2
    });
    term.send("echo roam-$((6*8))\n");
    term.wait("roam-48");
    let new = live.udp.sockets.lock().unwrap()[1].clone();
    wait_until(
        Duration::from_secs(5),
        "datagrams on the new socket",
        || new.load(Ordering::SeqCst) > 0,
    );
    term.send("echo again-$((6*9))\n");
    term.wait("again-54");
    // `network_changed` of the FFI is this same command on every live mosh session.
    term.handle.roam();
    term.send("echo third-$((6*10))\n");
    term.wait("third-60");

    // The user closes the terminal: the shutdown handshake ends the server; the host stays.
    term.handle.disconnect();
    assert_eq!(term.closed(), CloseReason::Disconnected);
    live.wait_no_servers("mosh-server to exit after the session's disconnect");
    assert!(matches!(
        live.host.state(),
        HostState::Connected { address_index: 0 }
    ));
    // The connection serves the next terminal, SSH or mosh.
    let mut ssh = live.open("ssh", TerminalTarget::Shell, TerminalTransport::Ssh);
    ssh.quiet();
    ssh.send("echo ssh-$((6*7))\n");
    ssh.wait("ssh-42");
    let mut again = live.mosh("m2", TerminalTarget::Shell);
    again.quiet();
    again.send("echo second-$((6*7))\n");
    again.wait("second-42");
    again.handle.disconnect();
    assert_eq!(again.closed(), CloseReason::Disconnected);
    live.host.disconnect();
}

#[test]
fn losing_the_host_connection_keeps_the_mosh_session_alive_and_usable() {
    require!();
    let live = Live::new();
    let mut mosh = live.mosh("m", TerminalTarget::Shell);
    let mut ssh = live.open("s", TerminalTarget::Shell, TerminalTransport::Ssh);
    mosh.quiet();
    ssh.quiet();
    mosh.send("echo before-$((6*7))\n");
    mosh.wait("before-42");

    live.proxy.cut();
    let reason = live.host_closed();
    let CloseReason::Failed(SessionFailure::ConnectionLost(_)) = &reason else {
        panic!("expected ConnectionLost, got {reason:?}")
    };
    // SSH terminals follow the host (M2's rule); the mosh session does not.
    assert_eq!(ssh.closed(), reason);
    assert!(
        mosh.states
            .recv_timeout(Duration::from_millis(500))
            .is_err(),
        "the mosh session was not told anything"
    );
    assert_eq!(mosh.handle.state(), SessionState::Connected);
    mosh.send("echo after-$((6*7+1))\n");
    mosh.wait("after-43");
    mosh.handle
        .resize(TerminalSize::new(90, 20).unwrap())
        .unwrap();
    mosh.send("stty size\n");
    mosh.wait("20 90");
    mosh.handle.roam();
    mosh.send("echo roamed-$((6*7+2))\n");
    mosh.wait("roamed-44");
    assert_eq!(
        live.host
            .open_terminal_with(
                TerminalTarget::Shell,
                TerminalTransport::Mosh,
                TerminalSize::new(80, 24).unwrap(),
                Arc::new(SessionObs {
                    tag: "late".into(),
                    tx: mpsc::channel().0,
                    log: Log::default(),
                    health: Arc::default(),
                }),
            )
            .err(),
        Some(HostError::Closed),
        "the lost host takes no more terminals"
    );
    assert_eq!(
        live.servers().len(),
        1,
        "the server outlives the connection"
    );

    let log = live.log.lock().unwrap().clone();
    assert!(!log.contains(&"m:Closed".to_owned()), "{log:?}");
    mosh.handle.disconnect();
    assert_eq!(mosh.closed(), CloseReason::Disconnected);
    live.wait_no_servers("mosh-server to exit after the lost host's session disconnected");
}

#[test]
fn a_user_disconnect_of_the_host_closes_its_mosh_sessions_before_the_host() {
    require!();
    let live = Live::new();
    let mut a = live.mosh("a", TerminalTarget::Shell);
    let b = live.mosh("b", TerminalTarget::Shell);
    let ssh = live.open("s", TerminalTarget::Shell, TerminalTransport::Ssh);
    a.quiet();
    a.send("echo up-$((6*7))\n");
    a.wait("up-42");
    assert_eq!(live.servers().len(), 2);

    live.host.disconnect();
    assert_eq!(a.closed(), CloseReason::Disconnected);
    assert_eq!(b.closed(), CloseReason::Disconnected);
    assert_eq!(ssh.closed(), CloseReason::Disconnected);
    assert_eq!(live.host_closed(), CloseReason::Disconnected);
    let log = live.log.lock().unwrap().clone();
    let position = |entry: &str| {
        log.iter()
            .position(|item| item == entry)
            .unwrap_or_else(|| panic!("{entry} missing from {log:?}"))
    };
    let host_closed = position("host:Closed");
    for entry in ["a:Closed", "b:Closed", "s:Closed"] {
        assert!(
            position(entry) < host_closed,
            "{entry} before the host: {log:?}"
        );
    }
    // The shutdown handshake ended both servers.
    live.wait_no_servers("both mosh-servers to exit after the host disconnect");
}

#[test]
fn blocked_udp_times_out_and_the_server_is_terminated() {
    require!();
    let live = Live::with(
        TestUdp {
            blackhole: true,
            ..TestUdp::default()
        },
        HostOptions {
            mosh_connect_timeout: Duration::from_millis(1500),
            ..HostOptions::default()
        },
    );
    let term = live.open_raw(
        "m",
        TerminalTarget::Shell,
        TerminalTransport::Mosh,
        (80, 24),
    );
    // The server is started and waits for a client that never reaches it.
    let mut seen = false;
    let deadline = Instant::now() + WAIT;
    let reason = loop {
        match term.states.try_recv() {
            Ok(SessionState::Closed(reason)) => break reason,
            Ok(other) => panic!("unexpected {other:?}"),
            Err(_) => {}
        }
        seen |= !live.servers().is_empty();
        assert!(Instant::now() < deadline, "the session never closed");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(reason, CloseReason::Failed(SessionFailure::TimedOut));
    assert!(seen, "the server was running while the client waited");
    live.wait_no_servers("mosh::terminate to stop the server");
    // Not a host failure: the connection is intact and serves the SSH fallback.
    assert!(matches!(live.host.state(), HostState::Connected { .. }));
    let mut ssh = live.open("s", TerminalTarget::Shell, TerminalTransport::Ssh);
    ssh.quiet();
    ssh.send("echo fallback-$((6*7))\n");
    ssh.wait("fallback-42");
    live.host.disconnect();
}

#[test]
fn a_disconnect_before_the_first_datagram_terminates_the_server() {
    require!();
    let live = Live::with(
        TestUdp {
            blackhole: true,
            ..TestUdp::default()
        },
        HostOptions::default(),
    );
    let term = live.open_raw(
        "m",
        TerminalTarget::Shell,
        TerminalTransport::Mosh,
        (80, 24),
    );
    wait_until(WAIT, "the server to start", || !live.servers().is_empty());
    assert_eq!(term.handle.state(), SessionState::Connecting);
    term.handle.disconnect();
    assert_eq!(term.closed(), CloseReason::Disconnected);
    live.wait_no_servers("the abandoned mosh-server to be terminated");
    live.host.disconnect();
}

#[test]
fn a_user_disconnect_of_the_host_while_a_mosh_session_connects_terminates_its_server() {
    require!();
    let live = Live::with(
        TestUdp {
            blackhole: true,
            ..TestUdp::default()
        },
        HostOptions::default(),
    );
    let term = live.open_raw(
        "m",
        TerminalTarget::Shell,
        TerminalTransport::Mosh,
        (80, 24),
    );
    wait_until(WAIT, "the server to start", || !live.servers().is_empty());
    live.host.disconnect();
    assert_eq!(term.closed(), CloseReason::Disconnected);
    assert_eq!(live.host_closed(), CloseReason::Disconnected);
    live.wait_no_servers("the abandoned mosh-server to be terminated");
}

/// Over a slow link a bootstrap that is still running when the user disconnects the host
/// finishes within the abandon grace, and stopping the server it started then takes a few round
/// trips: more than the 3 s that terminals get, so the host has to wait for the mosh session
/// (its servers must not leak and its `Closed` still comes first).
#[test]
fn a_host_disconnect_during_a_slow_bootstrap_waits_for_the_server_to_be_stopped() {
    require!();
    let live = Live::with_slow_bootstrap("1");
    let slow = live.slow.as_ref().unwrap();
    // The probe is cached by an SSH terminal, so only the bootstrap is in flight.
    let ssh = live.open("s", TerminalTarget::Shell, TerminalTransport::Ssh);
    live.proxy.slow(Duration::from_millis(500));
    let term = live.open_raw(
        "m",
        TerminalTarget::Shell,
        TerminalTransport::Mosh,
        (80, 24),
    );
    wait_until(WAIT, "the bootstrap to start", || slow.started());
    let disconnected = Instant::now();
    live.host.disconnect();
    assert_eq!(term.closed(), CloseReason::Disconnected);
    assert_eq!(ssh.closed(), CloseReason::Disconnected);
    assert_eq!(live.host_closed(), CloseReason::Disconnected);
    assert!(
        disconnected.elapsed() > Duration::from_millis(3200),
        "the scenario needs a cleanup longer than the terminals' grace, took {:?}",
        disconnected.elapsed()
    );
    let log = live.log.lock().unwrap().clone();
    let position = |entry: &str| log.iter().position(|item| item == entry).unwrap();
    assert!(
        position("m:Closed") < position("host:Closed"),
        "the session closes before the host: {log:?}"
    );
    // The server the abandoned bootstrap started was stopped before the host closed.
    live.proxy.slow(Duration::ZERO);
    assert!(
        live.servers().is_empty(),
        "the abandoned mosh-server was terminated before the host closed"
    );
}

/// A disconnect that arrives while the probe or pane focus is still running starts nothing:
/// no `mosh-server` is executed for a session nobody will use.
#[test]
fn a_disconnect_before_the_bootstrap_starts_executes_no_mosh_server() {
    require!();
    let live = Live::with_slow_bootstrap("1");
    let slow = live.slow.as_ref().unwrap();
    live.proxy.slow(Duration::from_millis(150));
    let term = live.open_raw(
        "m",
        TerminalTarget::Shell,
        TerminalTransport::Mosh,
        (80, 24),
    );
    term.handle.disconnect();
    assert_eq!(term.closed(), CloseReason::Disconnected);
    // Long enough for an exec that was started anyway to touch its marker.
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        !slow.started(),
        "mosh-server was executed after the disconnect"
    );
    assert!(live.servers().is_empty());
    live.proxy.slow(Duration::ZERO);
    live.host.disconnect();
}

/// Losing the host while the session is still waiting for its first datagram does not end it
/// (it never needed the connection again); it ends by its own timeout. The server cannot be
/// stopped then (contracts.md, "M3-A implementation"), which is the documented limit.
#[test]
fn losing_the_host_while_a_mosh_session_connects_leaves_it_to_time_out() {
    require!();
    let live = Live::with(
        TestUdp {
            blackhole: true,
            ..TestUdp::default()
        },
        HostOptions {
            mosh_connect_timeout: Duration::from_millis(1500),
            ..HostOptions::default()
        },
    );
    let term = live.open_raw(
        "m",
        TerminalTarget::Shell,
        TerminalTransport::Mosh,
        (80, 24),
    );
    // The bootstrap is over once the session has opened its first socket.
    wait_until(WAIT, "the session's first socket", || {
        !live.udp.sockets.lock().unwrap().is_empty()
    });
    live.proxy.cut();
    let CloseReason::Failed(SessionFailure::ConnectionLost(_)) = live.host_closed() else {
        panic!("expected the host to be lost")
    };
    assert_eq!(
        term.closed(),
        CloseReason::Failed(SessionFailure::TimedOut),
        "the session's own end, not the host's"
    );
}

#[test]
fn mosh_runs_the_tmux_attach_command_and_the_session_outlives_the_client() {
    require!();
    if !tmux_ready() {
        return;
    }
    let live = Live::new();
    // A plain `sh` in the session, not the runner's login shell with its first-run wizards.
    assert!(
        live.sshd
            .tmux()
            .args([
                "new-session",
                "-d",
                "-s",
                "or2mosh",
                "-x",
                "80",
                "-y",
                "24",
                "sh"
            ])
            .status()
            .unwrap()
            .success()
    );
    let mut term = live.mosh(
        "m",
        TerminalTarget::Tmux {
            session_name: "or2mosh".into(),
        },
    );
    term.send("echo in-tmux-$((6*7))\n");
    term.wait("in-tmux-42");
    // tmux itself holds the session, on the fixture's private socket.
    let listed = live
        .sshd
        .tmux()
        .args(["list-sessions", "-F", "#{session_name}"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&listed.stdout)
            .lines()
            .any(|name| name == "or2mosh"),
        "{listed:?}"
    );
    term.handle.disconnect();
    assert_eq!(term.closed(), CloseReason::Disconnected);
    live.wait_no_servers("mosh-server to exit");
    let listed = live
        .sshd
        .tmux()
        .args(["list-sessions", "-F", "#{session_name}"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&listed.stdout)
            .lines()
            .any(|name| name == "or2mosh"),
        "tmux keeps the session after the client detaches"
    );
    live.host.disconnect();
}

/// A fake `herdr` in the sshd sessions' `$HOME/.local/bin`: `session list --json` prints two
/// sessions, anything else prints its arguments and exits 3.
fn install_fake_herdr(sshd: &Sshd) {
    let bin = sshd.home().join(".local/bin");
    fs::create_dir_all(&bin).unwrap();
    let path = bin.join("herdr");
    fs::write(
        &path,
        r#"#!/bin/sh
if [ "$1" = session ] && [ "$2" = list ]; then
  echo '{"sessions":[{"default":true,"name":"default","running":true,"socket_path":"/nonexistent/herdr.sock"},{"default":false,"name":"or2-test-x","running":false,"socket_path":"/nonexistent/x.sock"}]}'
  exit 0
fi
echo "FAKE-HERDR $*"
exit 3
"#,
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn mosh_runs_the_herdr_command_and_a_failed_pane_focus_starts_no_server() {
    require!();
    let live = Live::new();
    install_fake_herdr(&live.sshd);
    // The target's command (`herdr --session <name>`) is mosh-server's command; the fake
    // prints its arguments and exits, which ends the mosh session as the server's end.
    let mut term = live.mosh(
        "h",
        TerminalTarget::Herdr {
            session: Some("or2-test-x".into()),
            pane_id: None,
        },
    );
    assert_eq!(
        term.closed(),
        CloseReason::RemoteExited { exit_status: None }
    );
    let screen = term.grid.refresh(&term.handle);
    assert!(
        screen.contains("FAKE-HERDR --session or2-test-x"),
        "the final frame survives the close: {screen:?}"
    );
    live.wait_no_servers("the finished mosh-server to be gone");

    // A pane is focused first (over SSH, like an SSH target); the fake fails the focus, so
    // the session closes CommandFailed before any server is started.
    let failed = live.open_raw(
        "f",
        TerminalTarget::Herdr {
            session: None,
            pane_id: Some("w1:p1".into()),
        },
        TerminalTransport::Mosh,
        (80, 24),
    );
    let SessionState::Closed(CloseReason::Failed(SessionFailure::CommandFailed(_))) = failed.next()
    else {
        panic!("expected a failed focus")
    };
    assert!(live.servers().is_empty(), "no server for a failed focus");
    live.host.disconnect();
}

/// How a mosh terminal whose goodbye cannot reach the server is ended.
enum Ending {
    TerminalDisconnect,
    LastHandleRelease,
    HostDisconnect,
}

/// The client's outbound UDP path breaks while SSH stays up: the shutdown handshake is never
/// acknowledged. The session still closes `Disconnected`, and the server must be stopped over
/// the live SSH connection (the goodbye proves nothing when it was not confirmed).
fn a_failed_goodbye_stops_the_server_over_ssh(ending: Ending) {
    let live = Live::new();
    let mut term = live.mosh("m", TerminalTarget::Shell);
    term.quiet();
    term.send("echo up-$((6*7))\n");
    term.wait("up-42");
    assert_eq!(live.servers().len(), 1);
    live.udp.mute.store(true, Ordering::SeqCst);
    match ending {
        Ending::TerminalDisconnect => {
            term.handle.disconnect();
            assert_eq!(term.closed(), CloseReason::Disconnected);
            assert!(matches!(live.host.state(), HostState::Connected { .. }));
        }
        Ending::LastHandleRelease => {
            let Term { handle, states, .. } = term;
            drop(handle);
            assert_eq!(
                states.recv_timeout(WAIT).expect("a session state change"),
                SessionState::Closed(CloseReason::Disconnected)
            );
            assert!(matches!(live.host.state(), HostState::Connected { .. }));
        }
        Ending::HostDisconnect => {
            live.host.disconnect();
            assert_eq!(term.closed(), CloseReason::Disconnected);
            assert_eq!(live.host_closed(), CloseReason::Disconnected);
        }
    }
    // Asserted here, not left to the fixture's reaper.
    live.wait_no_servers("the server to be stopped over SSH after an unconfirmed goodbye");
}

#[test]
fn a_failed_goodbye_still_stops_the_server_on_a_terminal_disconnect() {
    require!();
    a_failed_goodbye_stops_the_server_over_ssh(Ending::TerminalDisconnect);
}

#[test]
fn a_failed_goodbye_still_stops_the_server_on_the_release_of_the_last_handle() {
    require!();
    a_failed_goodbye_stops_the_server_over_ssh(Ending::LastHandleRelease);
}

#[test]
fn a_failed_goodbye_still_stops_the_server_on_a_host_disconnect() {
    require!();
    a_failed_goodbye_stops_the_server_over_ssh(Ending::HostDisconnect);
}
/// The host is lost (the mosh session survives), then the user disconnects the retained host:
/// it must still close the surviving session, though the SSH driver is gone.
#[test]
fn disconnecting_a_lost_host_closes_its_surviving_mosh_session() {
    require!();
    let live = Live::new();
    let mut mosh = live.mosh("m", TerminalTarget::Shell);
    mosh.quiet();
    mosh.send("echo before-$((6*7))\n");
    mosh.wait("before-42");
    live.proxy.cut();
    let CloseReason::Failed(SessionFailure::ConnectionLost(_)) = live.host_closed() else {
        panic!("expected the host to be lost")
    };
    assert_eq!(mosh.handle.state(), SessionState::Connected);
    live.host.disconnect();
    assert_eq!(mosh.closed(), CloseReason::Disconnected);
    // The handshake over UDP ended the server; the host's own `Closed` is not repeated.
    live.wait_no_servers("the server to exit after the lost host's disconnect");
    assert!(
        live.states
            .recv_timeout(Duration::from_millis(300))
            .is_err(),
        "the host closed once"
    );
    live.host.disconnect();
}

/// Releasing the last handle of a lost host closes its surviving mosh sessions too.
#[test]
fn releasing_a_lost_host_closes_its_surviving_mosh_session() {
    require!();
    let live = Live::new();
    let mut mosh = live.mosh("m", TerminalTarget::Shell);
    mosh.quiet();
    mosh.send("echo before-$((6*7))\n");
    mosh.wait("before-42");
    live.proxy.cut();
    let CloseReason::Failed(SessionFailure::ConnectionLost(_)) = live.host_closed() else {
        panic!("expected the host to be lost")
    };
    assert_eq!(mosh.handle.state(), SessionState::Connected);
    drop(live.host);
    assert_eq!(mosh.closed(), CloseReason::Disconnected);
    wait_until(
        Duration::from_secs(10),
        "the server to exit after the lost host was released",
        || fixture_processes(&live.sshd, "mosh-server").is_empty(),
    );
}

/// A user disconnect that races the loss of the connection reaches the mosh session whichever
/// the host driver sees first.
#[test]
fn a_host_disconnect_racing_the_loss_of_the_connection_still_closes_mosh_sessions() {
    require!();
    for round in 0..3 {
        let live = Live::new();
        let mut mosh = live.mosh("m", TerminalTarget::Shell);
        mosh.quiet();
        mosh.send("echo race-$((6*7))\n");
        mosh.wait("race-42");
        if round == 1 {
            live.host.disconnect();
            live.proxy.cut();
        } else {
            live.proxy.cut();
            live.host.disconnect();
        }
        assert_eq!(mosh.closed(), CloseReason::Disconnected, "round {round}");
        match live.host_closed() {
            CloseReason::Disconnected | CloseReason::Failed(SessionFailure::ConnectionLost(_)) => {}
            other => panic!("round {round}: unexpected host close {other:?}"),
        }
        live.wait_no_servers("the server to exit after the raced disconnect");
    }
}
/// With every SSH session channel taken (OpenSSH's `MaxSessions` of 10), stopping the server
/// of a mosh session that timed out has no channel to run on. The debt is kept and settled when
/// capacity returns; the server disappears without the fixture's reaper.
fn a_full_channel_limit_scenario(host_disconnect: bool) {
    let live = Live::with(
        TestUdp {
            blackhole: true,
            ..TestUdp::default()
        },
        HostOptions {
            mosh_connect_timeout: Duration::from_secs(3),
            ..HostOptions::default()
        },
    );
    let mosh = live.open_raw(
        "m",
        TerminalTarget::Shell,
        TerminalTransport::Mosh,
        (80, 24),
    );
    // The bootstrap's exec has finished (and released its channel) once the first socket
    // is open.
    wait_until(WAIT, "the session's first socket", || {
        !live.udp.sockets.lock().unwrap().is_empty()
    });
    let terminals: Vec<Term> = (0..10)
        .map(|index| {
            live.open(
                &format!("s{index}"),
                TerminalTarget::Shell,
                TerminalTransport::Ssh,
            )
        })
        .collect();
    assert_eq!(mosh.closed(), CloseReason::Failed(SessionFailure::TimedOut));
    assert!(matches!(live.host.state(), HostState::Connected { .. }));
    assert_eq!(
        live.servers().len(),
        1,
        "no channel was free to stop the server yet"
    );
    if host_disconnect {
        // The user disconnects the host: its terminals close, and the host makes one last try
        // at what it owes before the connection goes.
        live.host.disconnect();
        for terminal in &terminals {
            assert_eq!(terminal.closed(), CloseReason::Disconnected);
        }
        assert_eq!(live.host_closed(), CloseReason::Disconnected);
    } else {
        // Capacity returns as the terminals close: the owed stop goes through.
        for terminal in &terminals {
            terminal.handle.disconnect();
        }
    }
    live.wait_no_servers("the owed cleanup to stop the server once channels are free");
    if !host_disconnect {
        assert!(matches!(live.host.state(), HostState::Connected { .. }));
        live.host.disconnect();
    }
}

#[test]
fn a_full_ssh_channel_limit_does_not_leak_the_server_of_a_timed_out_mosh_session() {
    require!();
    a_full_channel_limit_scenario(false);
}

#[test]
fn a_host_disconnect_settles_a_stop_that_was_waiting_for_a_channel() {
    require!();
    a_full_channel_limit_scenario(true);
}
