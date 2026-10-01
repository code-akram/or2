//! Host connections against a disposable loopback OpenSSH (`common::Sshd`): trust, racing,
//! probe, exec limits, streamlocal, terminals (shell, tmux, herdr), concurrency, close
//! ordering and loss. Only temporary keys and configuration are used. tmux runs on a private
//! socket directory (`TMUX_TMPDIR` in the sshd session environment), herdr is a fake script
//! in the sshd session's `$HOME`: nothing here touches a real tmux or herdr server.

mod common;

use std::fmt::Debug;
use std::fs;
use std::future::Future;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use common::{Grid, Sshd, sshd_available};
use or2_core::herdr::{HerdrObserver, HerdrState};
use or2_core::host::{
    HerdrSessionInfo, HostConnectRequest, HostError, HostHandle, HostObserver, HostState,
    TerminalTarget,
};
use or2_core::keys::ClientKey;
use or2_core::remote::{OUTPUT_CAP, RemoteCommand, RemoteError, RemoteHost};
use or2_core::session::{
    CloseReason, SessionFailure, SessionHandle, SessionObserver, SessionState,
};
use or2_core::ssh::{HostOptions, SshRemote, connect_host, connect_tapped};
use or2_core::term::TerminalSize;
use or2_core::transport::DirectTcp;

macro_rules! require_sshd {
    () => {
        if !sshd_available() {
            eprintln!("SKIP: /usr/bin/sshd is absent");
            return;
        }
    };
}

macro_rules! require_tmux {
    () => {
        if Command::new("tmux").arg("-V").output().is_err() {
            eprintln!("SKIP: tmux is absent");
            return;
        }
    };
}

const WAIT: Duration = Duration::from_secs(10);

type Log = Arc<Mutex<Vec<String>>>;

/// `Closed(..)` -> `Closed`: the variant name, for the callback timeline.
fn word(value: &impl Debug) -> String {
    format!("{value:?}")
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
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
}

struct WatchObs {
    tag: String,
    tx: mpsc::Sender<HerdrState>,
    log: Log,
}

impl HerdrObserver for WatchObs {
    fn state_changed(&self, state: &HerdrState) {
        self.log
            .lock()
            .unwrap()
            .push(format!("{}:{}", self.tag, word(state)));
        let _ = self.tx.send(state.clone());
    }
}

fn lo() -> &'static str {
    "127.0.0.1"
}

fn request(key: &ClientKey, addresses: &[(&str, u16)], trusted: &[String]) -> HostConnectRequest {
    HostConnectRequest::new(addresses, &Sshd::username(), &key.to_stored(), trusted).unwrap()
}

fn host_observer() -> (Arc<HostObs>, mpsc::Receiver<HostState>, Log) {
    let (tx, states) = mpsc::channel();
    let log = Log::default();
    (
        Arc::new(HostObs {
            tx,
            log: log.clone(),
        }),
        states,
        log,
    )
}

fn next(states: &mpsc::Receiver<HostState>) -> HostState {
    states.recv_timeout(WAIT).expect("a host state change")
}

fn closed(states: &mpsc::Receiver<HostState>) -> CloseReason {
    let HostState::Closed(reason) = next(states) else {
        panic!("expected Closed")
    };
    assert!(
        states.recv_timeout(Duration::from_millis(100)).is_err(),
        "Closed is delivered once, last"
    );
    reason
}

/// A connected host with a private sshd, an authorized key and the host key pre-trusted.
struct Live {
    sshd: Sshd,
    host: HostHandle,
    states: mpsc::Receiver<HostState>,
    log: Log,
}

impl Live {
    fn new() -> Self {
        let sshd = Sshd::new(false);
        let key = ClientKey::generate_ed25519("");
        sshd.authorize(&key);
        let (observer, states, log) = host_observer();
        let host = connect_host(
            request(&key, &[(lo(), sshd.port)], std::slice::from_ref(&sshd.host)),
            observer,
        );
        assert_eq!(next(&states), HostState::Authenticating);
        assert_eq!(next(&states), HostState::Connected { address_index: 0 });
        Self {
            sshd,
            host,
            states,
            log,
        }
    }

    fn open(&self, tag: &str, target: TerminalTarget, columns: u16, rows: u16) -> Term {
        let (handle, states) = self.open_raw(tag, target, columns, rows);
        let mut term = Term {
            handle,
            states,
            grid: Grid::default(),
        };
        assert_eq!(term.next(), SessionState::Connected);
        term.grid = Grid::default();
        term
    }

    fn open_raw(
        &self,
        tag: &str,
        target: TerminalTarget,
        columns: u16,
        rows: u16,
    ) -> (SessionHandle, mpsc::Receiver<SessionState>) {
        let (tx, states) = mpsc::channel();
        let handle = self
            .host
            .open_terminal(
                target,
                TerminalSize::new(columns, rows).unwrap(),
                Arc::new(SessionObs {
                    tag: tag.into(),
                    tx,
                    log: self.log.clone(),
                }),
            )
            .unwrap();
        (handle, states)
    }
}

struct Term {
    handle: SessionHandle,
    states: mpsc::Receiver<SessionState>,
    grid: Grid,
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

    /// Whether the screen shows `text` now.
    fn has(&mut self, text: &str) -> bool {
        self.grid.contains(&self.handle, text)
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

/// A fake `herdr` in the sshd sessions' `$HOME/.local/bin`: `session list --json` prints two
/// sessions, anything else prints its arguments and exits 3.
fn install_fake_herdr(sshd: &Sshd) -> String {
    let bin = sshd.home().join(".local/bin");
    fs::create_dir_all(&bin).unwrap();
    let path = bin.join("herdr");
    fs::write(
        &path,
        r#"#!/bin/sh
if [ "$1" = session ] && [ "$2" = list ]; then
  echo '{"sessions":[{"default":true,"name":"default","running":true,"socket_path":"/nonexistent/herdr.sock","future":1},{"default":false,"name":"or2-test-x","running":false,"socket_path":"/nonexistent/x.sock"}]}'
  exit 0
fi
echo "FAKE-HERDR $*"
exit 3
"#,
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path.to_str().unwrap().to_owned()
}

fn dead_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn first_use_prompt_approve_trusted_reconnect_changed_key_and_reject() {
    require_sshd!();
    let sshd = Sshd::new(false);
    let key = ClientKey::generate_ed25519("");
    sshd.authorize(&key);
    let address = [(lo(), sshd.port)];

    // First use: nothing proceeds until the user decides, and the decision is bound to the
    // fingerprint that was shown.
    let (observer, states, _) = host_observer();
    let host = connect_host(request(&key, &address, &[]), observer);
    let HostState::AwaitingHostKey(prompt) = next(&states) else {
        panic!("expected the first-use prompt")
    };
    assert!(prompt.previously_trusted.is_empty());
    assert_eq!(prompt.presented.info().openssh, sshd.host);
    assert_eq!(
        host.approve_host_key("SHA256:not-the-key"),
        Err(HostError::HostKeyMismatch)
    );
    assert!(states.recv_timeout(Duration::from_millis(200)).is_err());
    assert_eq!(host.state(), HostState::AwaitingHostKey(prompt.clone()));
    host.approve_host_key(&prompt.presented.fingerprint())
        .unwrap();
    assert_eq!(next(&states), HostState::Authenticating);
    assert_eq!(next(&states), HostState::Connected { address_index: 0 });
    assert_eq!(host.approve_host_key("x"), Err(HostError::NoHostKeyPrompt));
    host.disconnect();
    assert_eq!(closed(&states), CloseReason::Disconnected);
    assert_eq!(host.approve_host_key("x"), Err(HostError::Closed));

    // Trusted reconnect: no prompt.
    let (observer, states, _) = host_observer();
    let host = connect_host(
        request(&key, &address, std::slice::from_ref(&sshd.host)),
        observer,
    );
    assert_eq!(next(&states), HostState::Authenticating);
    assert_eq!(next(&states), HostState::Connected { address_index: 0 });
    host.disconnect();
    assert_eq!(closed(&states), CloseReason::Disconnected);

    // A different trusted key: the presented one is a change; rejecting closes.
    let other = ClientKey::generate_ed25519("").public_key().openssh;
    let (observer, states, _) = host_observer();
    let host = connect_host(
        request(&key, &address, std::slice::from_ref(&other)),
        observer,
    );
    let HostState::AwaitingHostKey(prompt) = next(&states) else {
        panic!("expected the changed-key prompt")
    };
    assert_eq!(prompt.previously_trusted.len(), 1);
    assert_eq!(prompt.previously_trusted[0].info().openssh, other);
    assert_eq!(prompt.presented.info().openssh, sshd.host);
    host.reject_host_key().unwrap();
    assert_eq!(
        closed(&states),
        CloseReason::Failed(SessionFailure::HostKeyRejected)
    );
    assert_eq!(host.reject_host_key(), Err(HostError::Closed));
}

#[test]
fn an_unauthorized_key_closes_with_authentication_rejected() {
    require_sshd!();
    let sshd = Sshd::new(false);
    sshd.authorize(&ClientKey::generate_ed25519(""));
    let stranger = ClientKey::generate_ed25519("");
    let (observer, states, _) = host_observer();
    let _host = connect_host(
        request(
            &stranger,
            &[(lo(), sshd.port)],
            std::slice::from_ref(&sshd.host),
        ),
        observer,
    );
    assert_eq!(next(&states), HostState::Authenticating);
    assert_eq!(
        closed(&states),
        CloseReason::Failed(SessionFailure::AuthenticationRejected)
    );
}

#[test]
fn addresses_race_and_a_dead_first_address_loses_to_a_live_second() {
    require_sshd!();
    let sshd = Sshd::new(false);
    let key = ClientKey::generate_ed25519("");
    sshd.authorize(&key);
    let trusted = std::slice::from_ref(&sshd.host);
    let live = (lo(), sshd.port);

    for (addresses, expected) in [
        (vec![(lo(), dead_port()), live], 1),
        (vec![live, (lo(), dead_port())], 0),
        (vec![(lo(), dead_port()), (lo(), dead_port()), live], 2),
    ] {
        let (observer, states, _) = host_observer();
        let host = connect_host(request(&key, &addresses, trusted), observer);
        assert_eq!(next(&states), HostState::Authenticating);
        assert_eq!(
            next(&states),
            HostState::Connected {
                address_index: expected
            }
        );
        host.disconnect();
        assert_eq!(closed(&states), CloseReason::Disconnected);
    }

    let (observer, states, _) = host_observer();
    let _host = connect_host(
        request(&key, &[(lo(), dead_port()), (lo(), dead_port())], trusted),
        observer,
    );
    let CloseReason::Failed(SessionFailure::Unreachable(message)) = closed(&states) else {
        panic!("expected Unreachable")
    };
    assert!(message.contains("address 0") && message.contains("address 1"));
}

#[test]
fn capability_probe_finds_programs_locale_and_herdr_sessions_once_per_connection() {
    require_sshd!();
    let live = Live::new();
    let herdr = install_fake_herdr(&live.sshd);
    let first = block_on(live.host.capabilities()).unwrap();
    assert_eq!(first.herdr.as_deref(), Some(herdr.as_str()));
    assert_eq!(
        first.herdr_sessions,
        [
            HerdrSessionInfo {
                name: "default".into(),
                running: true,
                is_default: true
            },
            HerdrSessionInfo {
                name: "or2-test-x".into(),
                running: false,
                is_default: false
            },
        ]
    );
    for program in [&first.tmux, &first.mosh_server].into_iter().flatten() {
        assert!(program.starts_with('/'), "{program}");
    }
    assert!(
        first.utf8_locale.to_lowercase().contains("utf"),
        "{}",
        first.utf8_locale
    );
    // Cached for the connection: removing herdr changes nothing.
    fs::remove_file(&herdr).unwrap();
    assert_eq!(block_on(live.host.capabilities()).unwrap(), first);
    live.host.disconnect();
}

/// The established connection of a fresh host, as a `RemoteHost`.
fn remote_with(sshd: &Sshd, options: HostOptions) -> (HostHandle, SshRemote) {
    let key = ClientKey::generate_ed25519("");
    sshd.authorize(&key);
    let (observer, _states, _) = host_observer();
    let (host, tapped) = connect_tapped(
        Arc::new(DirectTcp),
        request(&key, &[(lo(), sshd.port)], std::slice::from_ref(&sshd.host)),
        observer,
        options,
    );
    let remote = tapped.blocking_recv().expect("the host connects");
    (host, remote)
}

#[test]
fn exec_collects_streams_status_quotes_for_the_login_shell_and_enforces_its_caps() {
    require_sshd!();
    let sshd = Sshd::new(false);
    let (host, remote) = remote_with(
        &sshd,
        HostOptions {
            exec_timeout: Duration::from_millis(700),
            ..HostOptions::default()
        },
    );
    let out = block_on(remote.exec_script("echo out; echo err >&2; exit 3")).unwrap();
    assert_eq!(out.status, Some(3));
    assert_eq!(out.stdout, b"out\n");
    assert_eq!(out.stderr, b"err\n");
    // Every token reaches the program intact through whatever the login shell is.
    let printf = RemoteCommand::new("printf").args(["[%s]", "a b", "it's", "$HOME", "$(id)", "é"]);
    let out = block_on(remote.exec(&printf)).unwrap();
    assert_eq!(out.stdout, "[a b][it's][$HOME][$(id)][é]".as_bytes());
    assert!(
        block_on(remote.exec(&RemoteCommand::new("/nonexistent/or2-prog")))
            .unwrap()
            .status
            == Some(127)
    );
    // Stdin is closed: a command that reads it ends instead of hanging.
    assert!(block_on(remote.exec_script("cat")).unwrap().success());

    // The cap is 1 MiB per stream: exactly the cap passes, one more byte fails.
    let at_cap = block_on(remote.exec_script(&format!("head -c {OUTPUT_CAP} /dev/zero"))).unwrap();
    assert_eq!(at_cap.stdout.len(), OUTPUT_CAP);
    for redirect in ["", " >&2"] {
        let script = format!("head -c {} /dev/zero{redirect}", OUTPUT_CAP + 1);
        assert_eq!(
            block_on(remote.exec_script(&script)),
            Err(RemoteError::OutputTooLarge),
            "{redirect:?}"
        );
    }
    // The timeout fails the exec, and the connection carries on.
    let started = Instant::now();
    assert_eq!(
        block_on(remote.exec_script("sleep 3")),
        Err(RemoteError::TimedOut)
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(
        block_on(remote.exec_script("echo alive")).unwrap().stdout,
        b"alive\n"
    );
    host.disconnect();
}

#[test]
fn open_unix_reaches_a_unix_socket_on_the_host_and_reports_refusals() {
    require_sshd!();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let sshd = Sshd::new(false);
    let (host, remote) = remote_with(&sshd, HostOptions::default());
    let path = sshd.home().join("echo.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4];
        socket.read_exact(&mut buf).unwrap();
        socket
            .write_all(&buf.map(|b| b.to_ascii_uppercase()))
            .unwrap();
        // Hold the socket until the client has read, then hang up.
        let _ = socket.read(&mut [0u8; 1]);
    });
    block_on(async {
        let mut stream = remote.open_unix(path.to_str().unwrap()).await.unwrap();
        stream.write_all(b"ping").await.unwrap();
        let mut reply = [0u8; 4];
        stream.read_exact(&mut reply).await.unwrap();
        assert_eq!(&reply, b"PING");
        drop(stream);
        let missing = sshd.home().join("missing.sock");
        assert!(matches!(
            remote.open_unix(missing.to_str().unwrap()).await,
            Err(RemoteError::Rejected(_))
        ));
    });
    server.join().unwrap();
    host.disconnect();
}

#[test]
fn shell_terminal_echoes_resizes_before_and_after_connected_and_reports_the_exit_status() {
    require_sshd!();
    let live = Live::new();
    // A resize before Connected wins: the PTY opens at (or is moved to) the latest size.
    let (handle, states) = live.open_raw("t", TerminalTarget::Shell, 80, 24);
    handle.resize(TerminalSize::new(100, 30).unwrap()).unwrap();
    let mut term = Term {
        handle,
        states,
        grid: Grid::default(),
    };
    assert_eq!(term.next(), SessionState::Connected);
    term.quiet();
    term.send("printf 'SIZE:'; stty size\n");
    term.wait("SIZE:30 100");
    term.handle
        .resize(TerminalSize::new(93, 37).unwrap())
        .unwrap();
    term.send("printf 'AFTER:'; stty size\n");
    term.wait("AFTER:37 93");
    assert_eq!(term.grid.size, Some(TerminalSize::new(93, 37).unwrap()));
    term.send("printf 'TERM-%s\\n' \"$TERM\"\n");
    term.wait("TERM-xterm-256color");
    term.send("printf 'UTF-%s\\n' 'é界😀'\n");
    term.wait("UTF-é界😀");
    term.send("exit 17\n");
    assert_eq!(
        term.closed(),
        CloseReason::RemoteExited {
            exit_status: Some(17)
        }
    );
    // The host and its other users are unaffected.
    assert_eq!(live.host.state(), HostState::Connected { address_index: 0 });
    let mut second = live.open("t2", TerminalTarget::Shell, 80, 24);
    second.quiet();
    live.host.disconnect();
    assert_eq!(second.closed(), CloseReason::Disconnected);
    assert_eq!(closed(&live.states), CloseReason::Disconnected);
}

#[test]
fn several_terminals_share_one_connection_independently() {
    require_sshd!();
    let live = Live::new();
    let mut terms: Vec<Term> = [(60u16, 20u16), (90, 30), (120, 40)]
        .into_iter()
        .enumerate()
        .map(|(index, (columns, rows))| {
            live.open(&format!("t{index}"), TerminalTarget::Shell, columns, rows)
        })
        .collect();
    for (index, term) in terms.iter_mut().enumerate() {
        term.quiet();
        term.send(&format!("printf 'ID-%s:' {index}; stty size\n"));
    }
    for (index, (columns, rows)) in [(60, 20), (90, 30), (120, 40)].into_iter().enumerate() {
        terms[index].wait(&format!("ID-{index}:{rows} {columns}"));
        for other in 0..3 {
            if other != index {
                assert!(
                    !terms[index].has(&format!("ID-{other}:")),
                    "terminal {index} must not show terminal {other}'s output"
                );
            }
        }
    }
    // One ends; the others and the host carry on, queries included.
    terms[1].send("exit 5\n");
    assert_eq!(
        terms[1].closed(),
        CloseReason::RemoteExited {
            exit_status: Some(5)
        }
    );
    terms[0].send("printf 'STILL-%s\\n' UP\n");
    terms[0].wait("STILL-UP");
    terms[2].send("printf 'STILL-%s\\n' UP\n");
    terms[2].wait("STILL-UP");
    assert!(block_on(live.host.capabilities()).is_ok());
    assert_eq!(live.host.state(), HostState::Connected { address_index: 0 });
    live.host.disconnect();
    assert_eq!(terms[0].closed(), CloseReason::Disconnected);
    assert_eq!(terms[2].closed(), CloseReason::Disconnected);
    assert_eq!(closed(&live.states), CloseReason::Disconnected);
}

#[test]
fn closing_one_terminal_closes_only_its_channel() {
    require_sshd!();
    let live = Live::new();
    let mut keep = live.open("keep", TerminalTarget::Shell, 80, 24);
    let drop_me = live.open("drop", TerminalTarget::Shell, 80, 24);
    keep.quiet();
    drop_me.handle.disconnect();
    assert_eq!(drop_me.closed(), CloseReason::Disconnected);
    keep.send("printf 'KEPT-%s\\n' OK\n");
    keep.wait("KEPT-OK");
    assert_eq!(live.host.state(), HostState::Connected { address_index: 0 });
    live.host.disconnect();
}

#[test]
fn user_disconnect_closes_terminals_and_watches_before_the_host_with_disconnected() {
    require_sshd!();
    let live = Live::new();
    install_fake_herdr(&live.sshd);
    let a = live.open("a", TerminalTarget::Shell, 80, 24);
    let b = live.open("b", TerminalTarget::Shell, 80, 24);
    let (tx, watch_states) = mpsc::channel();
    let watch = live
        .host
        .watch_herdr(
            None,
            Arc::new(WatchObs {
                tag: "w".into(),
                tx,
                log: live.log.clone(),
            }),
        )
        .unwrap();
    // The watch reaches a state before the close (Unavailable until herdr is integrated,
    // Live afterwards: either way not Closed).
    let first = watch_states.recv_timeout(WAIT).unwrap();
    assert_ne!(first, HerdrState::Closed);
    live.host.disconnect();
    assert_eq!(a.closed(), CloseReason::Disconnected);
    assert_eq!(b.closed(), CloseReason::Disconnected);
    assert_eq!(closed(&live.states), CloseReason::Disconnected);
    let mut last = first;
    while let Ok(state) = watch_states.recv_timeout(Duration::from_millis(300)) {
        last = state;
    }
    assert_eq!(last, HerdrState::Closed);
    assert_eq!(watch.state(), HerdrState::Closed);

    let log = live.log.lock().unwrap().clone();
    let position = |entry: &str| {
        log.iter()
            .position(|item| item == entry)
            .unwrap_or_else(|| panic!("{entry} missing from {log:?}"))
    };
    let host_closed = position("host:Closed");
    for entry in ["a:Closed", "b:Closed", "w:Closed"] {
        assert!(
            position(entry) < host_closed,
            "{entry} before the host: {log:?}"
        );
    }
    assert_eq!(host_closed, log.len() - 1, "the host closes last: {log:?}");
    // After the close everything is refused, quietly.
    assert!(matches!(
        live.host
            .open_terminal(
                TerminalTarget::Shell,
                TerminalSize::new(80, 24).unwrap(),
                Arc::new(SessionObs {
                    tag: "late".into(),
                    tx: mpsc::channel().0,
                    log: live.log.clone(),
                }),
            )
            .err(),
        Some(HostError::Closed)
    ));
    assert_eq!(block_on(live.host.capabilities()), Err(HostError::Closed));
    assert!(
        !live
            .log
            .lock()
            .unwrap()
            .iter()
            .any(|entry| entry.starts_with("late"))
    );
}

/// A TCP relay the test can cut: loss without touching sshd.
struct Proxy {
    port: u16,
    connections: Arc<Mutex<Vec<TcpStream>>>,
    stop: Arc<AtomicBool>,
}

impl Proxy {
    fn new(target: u16) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let connections = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (kept, stopping) = (connections.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                let Ok((client, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                };
                client.set_nonblocking(false).unwrap();
                let upstream = TcpStream::connect((Ipv4Addr::LOCALHOST, target)).unwrap();
                for (mut from, mut to) in [
                    (client.try_clone().unwrap(), upstream.try_clone().unwrap()),
                    (upstream.try_clone().unwrap(), client.try_clone().unwrap()),
                ] {
                    std::thread::spawn(move || {
                        let _ = std::io::copy(&mut from, &mut to);
                        let _ = to.shutdown(Shutdown::Both);
                    });
                }
                kept.lock().unwrap().extend([client, upstream]);
            }
        });
        Self {
            port,
            connections,
            stop,
        }
    }

    fn cut(&self) {
        for connection in self.connections.lock().unwrap().iter() {
            let _ = connection.shutdown(Shutdown::Both);
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.cut();
    }
}

#[test]
fn losing_the_connection_closes_terminals_and_the_host_with_the_same_failure() {
    require_sshd!();
    let sshd = Sshd::new(false);
    let key = ClientKey::generate_ed25519("");
    sshd.authorize(&key);
    let proxy = Proxy::new(sshd.port);
    let (observer, states, log) = host_observer();
    let host = connect_host(
        request(
            &key,
            &[(lo(), proxy.port)],
            std::slice::from_ref(&sshd.host),
        ),
        observer,
    );
    assert_eq!(next(&states), HostState::Authenticating);
    assert_eq!(next(&states), HostState::Connected { address_index: 0 });
    let (tx, term_states) = mpsc::channel();
    let handle = host
        .open_terminal(
            TerminalTarget::Shell,
            TerminalSize::new(80, 24).unwrap(),
            Arc::new(SessionObs {
                tag: "t".into(),
                tx,
                log: log.clone(),
            }),
        )
        .unwrap();
    assert_eq!(
        term_states.recv_timeout(WAIT).unwrap(),
        SessionState::Connected
    );
    let mut grid = Grid::default();
    handle
        .send_text("stty -echo; printf 'OR2-%s\\n' LIVE\n".into())
        .unwrap();
    grid.wait(&handle, "OR2-LIVE");

    proxy.cut();
    let host_reason = closed(&states);
    let CloseReason::Failed(SessionFailure::ConnectionLost(_)) = &host_reason else {
        panic!("expected ConnectionLost, got {host_reason:?}")
    };
    let SessionState::Closed(term_reason) = term_states.recv_timeout(WAIT).unwrap() else {
        panic!("expected the terminal to close")
    };
    assert_eq!(term_reason, host_reason, "terminals get the host's failure");
    let log = log.lock().unwrap().clone();
    assert_eq!(
        log.last().map(String::as_str),
        Some("host:Closed"),
        "{log:?}"
    );
    assert!(log.contains(&"t:Closed".to_string()), "{log:?}");
    assert_eq!(
        host.open_terminal(
            TerminalTarget::Shell,
            TerminalSize::new(80, 24).unwrap(),
            Arc::new(SessionObs {
                tag: "late".into(),
                tx: mpsc::channel().0,
                log: Log::default(),
            }),
        )
        .err(),
        Some(HostError::Closed)
    );
}

#[test]
fn names_are_validated_before_anything_runs() {
    require_sshd!();
    let live = Live::new();
    for target in [
        TerminalTarget::Tmux {
            session_name: "a:b".into(),
        },
        TerminalTarget::Tmux {
            session_name: String::new(),
        },
        TerminalTarget::Herdr {
            session: Some("bad name".into()),
            pane_id: None,
        },
        TerminalTarget::Herdr {
            session: None,
            pane_id: Some("p;1".into()),
        },
    ] {
        let (tx, states) = mpsc::channel();
        let result = live.host.open_terminal(
            target,
            TerminalSize::new(80, 24).unwrap(),
            Arc::new(SessionObs {
                tag: "x".into(),
                tx,
                log: live.log.clone(),
            }),
        );
        assert_eq!(result.err(), Some(HostError::InvalidName));
        assert!(states.recv_timeout(Duration::from_millis(50)).is_err());
    }
    live.host.disconnect();
}

#[test]
fn tmux_lists_sessions_attaches_creates_and_detaches_on_a_private_socket() {
    require_sshd!();
    require_tmux!();
    let live = Live::new();
    // No server on the private socket yet: an empty list, not an error.
    assert!(block_on(live.host.list_tmux_sessions()).unwrap().is_empty());
    for name in ["or2-one", "or2-two"] {
        let status = live
            .sshd
            .tmux()
            .args([
                "new-session",
                "-d",
                "-s",
                name,
                "-x",
                "80",
                "-y",
                "24",
                "sh",
            ])
            .status()
            .unwrap();
        assert!(status.success());
    }
    // Sessions created later (by attaching to a new name) get a plain shell too, not the
    // runner's login shell with its first-run wizards.
    assert!(
        live.sshd
            .tmux()
            .args(["set-option", "-g", "default-shell", "/bin/sh"])
            .status()
            .unwrap()
            .success()
    );
    let sessions = block_on(live.host.list_tmux_sessions()).unwrap();
    let mut names: Vec<&str> = sessions.iter().map(|s| s.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["or2-one", "or2-two"]);
    assert!(
        sessions
            .windows(2)
            .all(|pair| pair[0].activity_unix >= pair[1].activity_unix),
        "most recently active first"
    );
    for session in &sessions {
        assert_eq!((session.windows, session.attached_clients), (1, 0));
        assert!(session.created_unix > 1_600_000_000);
    }

    // Attach: the typed command runs in the existing tmux session.
    let mut term = live.open(
        "tmux",
        TerminalTarget::Tmux {
            session_name: "or2-one".into(),
        },
        80,
        24,
    );
    term.send("echo OR2-$((6*7))\n");
    term.wait("OR2-42");
    let attached = |name: &str, clients: u32| {
        let deadline = Instant::now() + WAIT;
        loop {
            let sessions = block_on(live.host.list_tmux_sessions()).unwrap();
            let session = sessions.iter().find(|s| s.name == name).unwrap();
            if session.attached_clients == clients {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "tmux never reported {clients} clients on {name}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    attached("or2-one", 1);

    // A name that does not exist is created ("attach or create").
    let mut created = live.open(
        "tmux-new",
        TerminalTarget::Tmux {
            session_name: "or2-created".into(),
        },
        100,
        30,
    );
    created.send("echo OR2-$((7*7))\n");
    created.wait("OR2-49");
    let names: Vec<String> = block_on(live.host.list_tmux_sessions())
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(names.contains(&"or2-created".to_string()), "{names:?}");

    // Ending the terminal detaches the tmux client; the session itself lives on.
    term.handle.disconnect();
    assert_eq!(term.closed(), CloseReason::Disconnected);
    attached("or2-one", 0);
    // tmux exiting its client (here: the session's shell exits) is a remote exit.
    created.send("exit\n");
    let CloseReason::RemoteExited { .. } = created.closed() else {
        panic!("expected RemoteExited")
    };
    live.host.disconnect();
}

#[test]
fn herdr_terminals_run_the_probed_herdr_with_the_session_and_report_a_failed_focus() {
    require_sshd!();
    let live = Live::new();
    // Without herdr on the host the terminal reports it instead of opening a channel.
    let (handle, states) = live.open_raw(
        "missing",
        TerminalTarget::Herdr {
            session: None,
            pane_id: None,
        },
        80,
        24,
    );
    let Ok(SessionState::Closed(reason)) = states.recv_timeout(WAIT) else {
        panic!("expected the terminal to close")
    };
    assert_eq!(
        reason,
        CloseReason::Failed(SessionFailure::NotInstalled {
            program: "herdr".into()
        })
    );
    drop(handle);

    let herdr = install_fake_herdr(&live.sshd);
    // The probe is cached per connection, so use a fresh host that sees the fake.
    live.host.disconnect();
    assert_eq!(closed(&live.states), CloseReason::Disconnected);
    let key = ClientKey::generate_ed25519("");
    live.sshd.authorize(&key);
    let (observer, states, log) = host_observer();
    let host = connect_host(
        request(
            &key,
            &[(lo(), live.sshd.port)],
            std::slice::from_ref(&live.sshd.host),
        ),
        observer,
    );
    assert_eq!(next(&states), HostState::Authenticating);
    assert_eq!(next(&states), HostState::Connected { address_index: 0 });
    let open = |tag: &str, session: Option<&str>, pane: Option<&str>| {
        let (tx, term_states) = mpsc::channel();
        let handle = host
            .open_terminal(
                TerminalTarget::Herdr {
                    session: session.map(Into::into),
                    pane_id: pane.map(Into::into),
                },
                TerminalSize::new(80, 24).unwrap(),
                Arc::new(SessionObs {
                    tag: tag.into(),
                    tx,
                    log: log.clone(),
                }),
            )
            .unwrap();
        Term {
            handle,
            states: term_states,
            grid: Grid::default(),
        }
    };
    let mut default = open("default", None, None);
    assert_eq!(default.next(), SessionState::Connected);
    assert_eq!(
        default.closed(),
        CloseReason::RemoteExited {
            exit_status: Some(3)
        }
    );
    assert!(default.has("FAKE-HERDR"));
    assert!(!default.has("--session"));

    let mut named = open("named", Some("or2-test-x"), None);
    assert_eq!(named.next(), SessionState::Connected);
    named.closed();
    assert!(named.has("FAKE-HERDR --session or2-test-x"));

    // A pane is focused first through herdr's API; the fake has no socket, so the focus fails
    // and the terminal never runs herdr.
    let focus = open("focus", None, Some("w1:p1"));
    let SessionState::Closed(reason) = focus.next() else {
        panic!("a failed focus closes the terminal before it connects")
    };
    assert!(
        matches!(
            reason,
            CloseReason::Failed(SessionFailure::CommandFailed(_))
        ),
        "{reason:?}"
    );
    let _ = herdr;
    host.disconnect();
}
