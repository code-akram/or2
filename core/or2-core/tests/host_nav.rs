//! `HostHandle::navigate` for tmux targets end to end: a disposable loopback OpenSSH
//! (`common::Sshd`) with tmux on a private socket directory, a real tmux client attached by an
//! SSH terminal, and the window, pane and session moves the swipe gestures make. herdr moves
//! are tested against a real isolated herdr in `herdr_live.rs` and against a scripted one in
//! `herdr::navigate`. Skips without sshd or tmux unless `OR2_REQUIRE_SSHD`/`OR2_REQUIRE_TMUX`
//! is set.

mod common;

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use common::{Sshd, sshd_ready, tmux_ready};
use or2_core::host::{
    HostConnectRequest, HostObserver, HostState, NavDirection, TargetNav, TargetScroll,
    TerminalTarget,
};
use or2_core::keys::ClientKey;
use or2_core::session::{SessionObserver, SessionState};
use or2_core::ssh::connect_host;
use or2_core::term::TerminalSize;

const WAIT: Duration = Duration::from_secs(10);

struct HostObs(mpsc::Sender<HostState>);

impl HostObserver for HostObs {
    fn state_changed(&self, state: &HostState) {
        let _ = self.0.send(state.clone());
    }
}

struct SessionObs(mpsc::Sender<SessionState>);

impl SessionObserver for SessionObs {
    fn state_changed(&self, state: &SessionState) {
        let _ = self.0.send(state.clone());
    }

    fn frame_ready(&self) {}
}

fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

/// `tmux` on the sshd's private socket; panics unless it succeeds. Returns stdout, trimmed.
fn tmux(sshd: &Sshd, args: &[&str]) -> String {
    let output = sshd.tmux().args(args).output().unwrap();
    assert!(
        output.status.success(),
        "tmux {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Polls `read` until it returns `expected`.
fn eventually(what: &str, expected: &str, read: impl Fn() -> String) {
    let deadline = Instant::now() + WAIT;
    loop {
        let value = read();
        if value == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: {value:?}, expected {expected:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn tmux_moves_windows_panes_and_the_terminal_client_between_sessions() {
    if !sshd_ready() || !tmux_ready() {
        return;
    }
    let sshd = Sshd::new(false);
    let key = ClientKey::generate_ed25519("");
    sshd.authorize(&key);
    let (tx, states) = mpsc::channel();
    let host = connect_host(
        HostConnectRequest::new(
            &[("127.0.0.1", sshd.port)],
            &Sshd::username(),
            &key.to_stored(),
            std::slice::from_ref(&sshd.host),
        )
        .unwrap(),
        Arc::new(HostObs(tx)),
    );
    let mut state = states.recv_timeout(WAIT).unwrap();
    while state != (HostState::Connected { address_index: 0 }) {
        assert!(!matches!(state, HostState::Closed(_)), "{state:?}");
        state = states.recv_timeout(WAIT).unwrap();
    }

    // or2-a: two windows, the first split in two side by side. or2-b: two windows. "or2-a2"
    // shares the prefix: an exact target must never reach it.
    for name in ["or2-a", "or2-a2", "or2-b"] {
        tmux(
            &sshd,
            &[
                "new-session",
                "-d",
                "-s",
                name,
                "-x",
                "80",
                "-y",
                "24",
                "sh",
            ],
        );
    }
    tmux(&sshd, &["split-window", "-h", "-t", "=or2-a:0", "sh"]);
    tmux(&sshd, &["new-window", "-d", "-t", "=or2-a:", "sh"]);
    tmux(&sshd, &["new-window", "-d", "-t", "=or2-b:", "sh"]);
    tmux(&sshd, &["select-pane", "-t", "=or2-a:0.1"]);
    let window = |session: &str| {
        tmux(
            &sshd,
            &[
                "display",
                "-p",
                "-t",
                &format!("={session}:"),
                "#{window_index}",
            ],
        )
    };
    let pane = || {
        tmux(
            &sshd,
            &[
                "display",
                "-p",
                "-t",
                "=or2-a:",
                "#{window_index}.#{pane_index}",
            ],
        )
    };
    assert_eq!(pane(), "0.1");

    let target = TerminalTarget::Tmux {
        session_name: "or2-a".into(),
    };
    // Before a terminal is attached a session move has no client to switch: nothing to do.
    assert_eq!(
        block_on(host.navigate(target.clone(), None, TargetNav::NextSession, None)),
        Ok(())
    );

    // The terminal: an SSH tmux client attached to or2-a.
    let (tx, session_states) = mpsc::channel();
    let terminal = host
        .open_terminal(
            target.clone(),
            TerminalSize::new(80, 24).unwrap(),
            Arc::new(SessionObs(tx)),
        )
        .unwrap();
    assert_eq!(
        session_states.recv_timeout(WAIT).unwrap(),
        SessionState::Connected
    );
    let client_session = || tmux(&sshd, &["list-clients", "-F", "#{client_session}"]);
    eventually("the terminal's client", "or2-a", client_session);
    let id = terminal.client_id().expect("a tmux terminal").to_owned();
    let navigate =
        |nav: TargetNav| block_on(host.navigate(target.clone(), None, nav, Some(id.clone())));

    // Without the terminal's id its client is unknown: a session move switches nothing (the
    // only client on the target is not taken on a guess).
    assert_eq!(
        block_on(host.navigate(target.clone(), None, TargetNav::NextSession, None)),
        Ok(())
    );
    assert_eq!(client_session(), "or2-a");

    // Panes: left from pane 1, then right again.
    navigate(TargetNav::Pane {
        direction: NavDirection::Left,
    })
    .unwrap();
    assert_eq!(pane(), "0.0");
    navigate(TargetNav::Pane {
        direction: NavDirection::Right,
    })
    .unwrap();
    assert_eq!(pane(), "0.1");

    // Windows, wrapping around; or2-a2 is never touched.
    navigate(TargetNav::NextWindow).unwrap();
    assert_eq!(window("or2-a"), "1");
    navigate(TargetNav::NextWindow).unwrap();
    assert_eq!(window("or2-a"), "0");
    navigate(TargetNav::PreviousWindow).unwrap();
    assert_eq!(window("or2-a"), "1");
    assert_eq!(window("or2-a2"), "0");

    // Sessions: the terminal's client moves (tmux orders sessions by name).
    navigate(TargetNav::NextSession).unwrap();
    eventually("after the next session", "or2-a2", client_session);
    navigate(TargetNav::NextSession).unwrap();
    eventually("after the next session", "or2-b", client_session);

    // Window moves now act on the session the terminal shows, not on its target.
    navigate(TargetNav::NextWindow).unwrap();
    assert_eq!(window("or2-b"), "1");
    assert_eq!(window("or2-a"), "1");

    // So does a swipe scroll (it used to scroll the target, out of sight).
    tmux(&sshd, &["send-keys", "-t", "=or2-b:", "seq 1 100", "Enter"]);
    let history = || {
        tmux(
            &sshd,
            &["display", "-p", "-t", "=or2-b:", "#{history_size}"],
        )
    };
    let deadline = Instant::now() + WAIT;
    while history() == "0" {
        assert!(Instant::now() < deadline, "no history in or2-b");
        std::thread::sleep(Duration::from_millis(25));
    }
    let in_mode = |session: &str| {
        let target = format!("={session}:");
        tmux(&sshd, &["display", "-p", "-t", &target, "#{pane_in_mode}"])
    };
    let scroll =
        |scroll| block_on(host.scroll_target(target.clone(), None, scroll, Some(id.clone())));
    scroll(TargetScroll::Up { lines: 3 }).unwrap();
    assert_eq!(in_mode("or2-b"), "1");
    assert_eq!(in_mode("or2-a"), "0");
    scroll(TargetScroll::Bottom).unwrap();
    assert_eq!(in_mode("or2-b"), "0");

    // And back, wrapping around: or2-b, or2-a, then (previous) or2-b again.
    navigate(TargetNav::NextSession).unwrap();
    eventually("wrapped to the first session", "or2-a", client_session);
    navigate(TargetNav::PreviousSession).unwrap();
    eventually("after the previous session", "or2-b", client_session);
    navigate(TargetNav::PreviousSession).unwrap();
    eventually("after the previous session", "or2-a2", client_session);

    // A one-window session has nowhere to go: still `Ok`.
    navigate(TargetNav::NextWindow).unwrap();
    assert_eq!(window("or2-a2"), "0");

    // A shell target has nothing to move.
    assert_eq!(
        block_on(host.navigate(TerminalTarget::Shell, None, TargetNav::NextSession, None)),
        Ok(())
    );

    terminal.disconnect();
    host.disconnect();
}

/// Two terminals on the same tmux session, the app's two-clients case: each gesture moves the
/// client of the terminal it was made on (by that terminal's `client_id`), never the other
/// one, although the other is the more recently active client on the target. A closed terminal
/// releases what its attach recorded.
#[test]
fn two_terminals_on_one_tmux_session_each_move_only_their_own_client() {
    if !sshd_ready() || !tmux_ready() {
        return;
    }
    let sshd = Sshd::new(false);
    let key = ClientKey::generate_ed25519("");
    sshd.authorize(&key);
    let (tx, states) = mpsc::channel();
    let host = connect_host(
        HostConnectRequest::new(
            &[("127.0.0.1", sshd.port)],
            &Sshd::username(),
            &key.to_stored(),
            std::slice::from_ref(&sshd.host),
        )
        .unwrap(),
        Arc::new(HostObs(tx)),
    );
    let mut state = states.recv_timeout(WAIT).unwrap();
    while state != (HostState::Connected { address_index: 0 }) {
        assert!(!matches!(state, HostState::Closed(_)), "{state:?}");
        state = states.recv_timeout(WAIT).unwrap();
    }
    // Sessions in tmux's (name) order: or2-a, or2-b, or2-c; or2-a and or2-b with two windows.
    for name in ["or2-a", "or2-b", "or2-c"] {
        tmux(
            &sshd,
            &[
                "new-session",
                "-d",
                "-s",
                name,
                "-x",
                "80",
                "-y",
                "24",
                "sh",
            ],
        );
    }
    tmux(&sshd, &["new-window", "-d", "-t", "=or2-a:", "sh"]);
    tmux(&sshd, &["new-window", "-d", "-t", "=or2-b:", "sh"]);
    let window = |session: &str| {
        tmux(
            &sshd,
            &[
                "display",
                "-p",
                "-t",
                &format!("={session}:"),
                "#{window_index}",
            ],
        )
    };

    let target = TerminalTarget::Tmux {
        session_name: "or2-a".into(),
    };
    let open = || {
        let (tx, states) = mpsc::channel();
        let terminal = host
            .open_terminal(
                target.clone(),
                TerminalSize::new(80, 24).unwrap(),
                Arc::new(SessionObs(tx)),
            )
            .unwrap();
        assert_eq!(states.recv_timeout(WAIT).unwrap(), SessionState::Connected);
        terminal
    };
    let first = open();
    let second = open();
    let (a, b) = (
        first.client_id().unwrap().to_owned(),
        second.client_id().unwrap().to_owned(),
    );
    assert_ne!(a, b);
    // Each attach recorded its own client.
    let recorded = |id: &str| {
        let output = sshd
            .tmux()
            .args([
                "show-options",
                "-s",
                "-q",
                "-v",
                &format!("@or2-client-{id}"),
            ])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    let deadline = Instant::now() + WAIT;
    while recorded(&a).is_empty() || recorded(&b).is_empty() {
        assert!(Instant::now() < deadline, "the attaches recorded no client");
        std::thread::sleep(Duration::from_millis(25));
    }
    let (tty_a, tty_b) = (recorded(&a), recorded(&b));
    assert_ne!(tty_a, tty_b);
    // What `field` of the client named `tty` is ("" while it is not listed).
    let client = |tty: &str, field: &str| {
        tmux(
            &sshd,
            &["list-clients", "-F", &format!("#{{client_name}} {field}")],
        )
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{tty} ")).map(str::to_owned))
        .unwrap_or_default()
    };
    let shows = |tty: &str| client(tty, "#{client_session}");
    eventually("A's client", "or2-a", || shows(&tty_a));
    eventually("B's client", "or2-a", || shows(&tty_b));

    // B is the more recently active client on the target: a guess by the target would take it
    // for A's gestures too.
    std::thread::sleep(Duration::from_millis(1100));
    second.send_text(" ".into()).unwrap();
    let activity = |tty: &str| {
        client(tty, "#{client_activity}")
            .parse::<u64>()
            .unwrap_or_default()
    };
    let deadline = Instant::now() + WAIT;
    while activity(&tty_b) <= activity(&tty_a) {
        assert!(Instant::now() < deadline, "B never became the more active");
        std::thread::sleep(Duration::from_millis(25));
    }

    let navigate = |id: &str, nav: TargetNav| {
        block_on(host.navigate(target.clone(), None, nav, Some(id.to_owned())))
    };
    // Alternating session moves: each moves its own client only.
    navigate(&a, TargetNav::NextSession).unwrap();
    eventually("A after its next session", "or2-b", || shows(&tty_a));
    assert_eq!(shows(&tty_b), "or2-a", "B stayed");
    navigate(&b, TargetNav::PreviousSession).unwrap();
    eventually("B after its previous session", "or2-c", || shows(&tty_b));
    assert_eq!(shows(&tty_a), "or2-b", "A stayed");
    navigate(&a, TargetNav::NextSession).unwrap();
    eventually("A after its next session", "or2-c", || shows(&tty_a));
    assert_eq!(shows(&tty_b), "or2-c", "B stayed");
    navigate(&b, TargetNav::NextSession).unwrap();
    eventually("B after its next session", "or2-a", || shows(&tty_b));
    assert_eq!(shows(&tty_a), "or2-c", "A stayed");

    // Window moves act on the session each terminal's own client shows.
    navigate(&b, TargetNav::NextWindow).unwrap();
    assert_eq!(window("or2-a"), "1");
    navigate(&a, TargetNav::PreviousSession).unwrap();
    eventually("A after its previous session", "or2-b", || shows(&tty_a));
    navigate(&a, TargetNav::NextWindow).unwrap();
    assert_eq!(window("or2-b"), "1");
    assert_eq!(window("or2-a"), "1", "B's session untouched by A's move");

    // Closing A releases what its attach recorded; B still moves its own client.
    first.disconnect();
    let deadline = Instant::now() + WAIT;
    while !recorded(&a).is_empty() {
        assert!(Instant::now() < deadline, "A's record was not released");
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(recorded(&b), tty_b);
    navigate(&b, TargetNav::NextSession).unwrap();
    eventually("B after its next session", "or2-b", || shows(&tty_b));

    second.disconnect();
    host.disconnect();
}

/// A `tmux` in front of the real one on the sshd sessions' `PATH` that behaves like a tmux
/// older than 2.6 where the identity step is concerned: `-V` says `tmux 2.5`, and any command
/// list with `set-option -F` is refused whole, as an old tmux client refuses a command it
/// cannot parse (the attach with it included). Every invocation is logged.
struct OldTmux {
    directory: tempfile::TempDir,
}

impl OldTmux {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let bin = directory.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let real = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .chain([std::path::PathBuf::from("/usr/bin")])
            .map(|dir| dir.join("tmux"))
            .find(|path| path.is_file())
            .expect("a real tmux");
        let script = bin.join("tmux");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 printf '%s\\n' \"$*\" >> '{log}'\n\
                 if [ \"$1\" = -V ]; then echo 'tmux 2.5'; exit 0; fi\n\
                 or2_set=0\n\
                 for or2_arg in \"$@\"; do\n\
                   case \"$or2_arg\" in\n\
                     set-option) or2_set=1 ;;\n\
                     ';') or2_set=0 ;;\n\
                     -F) if [ $or2_set = 1 ]; then echo 'usage: set-option [-agoqsuw] option [value]' >&2; exit 1; fi ;;\n\
                   esac\n\
                 done\n\
                 exec '{real}' \"$@\"\n",
                log = directory.path().join("log").display(),
                real = real.display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { directory }
    }

    /// The session environment that puts the old tmux first on `PATH`.
    fn environment(&self) -> String {
        format!(
            "PATH={}:{}",
            self.directory.path().join("bin").display(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
        )
    }

    /// Every command line it was run with.
    fn log(&self) -> String {
        std::fs::read_to_string(self.directory.path().join("log")).unwrap_or_default()
    }
}

/// A tmux too old to record a terminal's client (`set-option -F`): the attach is plain and
/// works, a session move switches nothing (the terminal's client cannot be known, and it is
/// never guessed), and window and pane moves still act on the target.
#[test]
fn an_old_tmux_attaches_plainly_and_its_session_moves_do_nothing() {
    if !sshd_ready() || !tmux_ready() {
        return;
    }
    let old = OldTmux::new();
    let sshd = Sshd::with_environment(false, "", &old.environment());
    let key = ClientKey::generate_ed25519("");
    sshd.authorize(&key);
    let (tx, states) = mpsc::channel();
    let host = connect_host(
        HostConnectRequest::new(
            &[("127.0.0.1", sshd.port)],
            &Sshd::username(),
            &key.to_stored(),
            std::slice::from_ref(&sshd.host),
        )
        .unwrap(),
        Arc::new(HostObs(tx)),
    );
    let mut state = states.recv_timeout(WAIT).unwrap();
    while state != (HostState::Connected { address_index: 0 }) {
        assert!(!matches!(state, HostState::Closed(_)), "{state:?}");
        state = states.recv_timeout(WAIT).unwrap();
    }
    for name in ["or2-a", "or2-b"] {
        tmux(
            &sshd,
            &[
                "new-session",
                "-d",
                "-s",
                name,
                "-x",
                "80",
                "-y",
                "24",
                "sh",
            ],
        );
    }
    tmux(&sshd, &["split-window", "-h", "-t", "=or2-a:0", "sh"]);
    tmux(&sshd, &["new-window", "-d", "-t", "=or2-a:", "sh"]);
    tmux(&sshd, &["select-pane", "-t", "=or2-a:0.1"]);
    let display = |format: &str| tmux(&sshd, &["display", "-p", "-t", "=or2-a:", format]);

    let target = TerminalTarget::Tmux {
        session_name: "or2-a".into(),
    };
    let (tx, session_states) = mpsc::channel();
    let terminal = host
        .open_terminal(
            target.clone(),
            TerminalSize::new(80, 24).unwrap(),
            Arc::new(SessionObs(tx)),
        )
        .unwrap();
    assert_eq!(
        session_states.recv_timeout(WAIT).unwrap(),
        SessionState::Connected
    );
    // The attach worked: its client shows the target.
    let client_session = || tmux(&sshd, &["list-clients", "-F", "#{client_session}"]);
    eventually("the terminal's client", "or2-a", client_session);
    let id = terminal.client_id().expect("a tmux terminal").to_owned();
    let navigate =
        |nav: TargetNav| block_on(host.navigate(target.clone(), None, nav, Some(id.clone())));

    // Session moves: nothing to do, nothing switched.
    assert_eq!(navigate(TargetNav::NextSession), Ok(()));
    assert_eq!(navigate(TargetNav::PreviousSession), Ok(()));
    assert_eq!(client_session(), "or2-a");

    // Window and pane moves act on the target.
    navigate(TargetNav::Pane {
        direction: NavDirection::Left,
    })
    .unwrap();
    assert_eq!(display("#{window_index}.#{pane_index}"), "0.0");
    navigate(TargetNav::NextWindow).unwrap();
    assert_eq!(display("#{window_index}"), "1");

    terminal.disconnect();
    let deadline = Instant::now() + WAIT;
    loop {
        let state = session_states.recv_timeout(WAIT).unwrap();
        if matches!(state, SessionState::Closed(_)) {
            break;
        }
        assert!(Instant::now() < deadline, "the terminal never closed");
    }
    // A release would run in the background: give it the time it would take.
    std::thread::sleep(Duration::from_millis(500));
    let log = old.log();
    assert!(log.lines().any(|line| line == "-V"), "{log}");
    assert!(
        log.lines().any(|line| line == "-u new-session -A -s or2-a"),
        "the attach is plain: {log}"
    );
    assert!(!log.contains("set-option"), "no identity step: {log}");
    assert!(!log.contains("switch-client"), "no session switched: {log}");
    host.disconnect();
}
