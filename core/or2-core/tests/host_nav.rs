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
    HostConnectRequest, HostError, HostObserver, HostState, NavDirection, TargetNav, TerminalTarget,
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
    let navigate = |nav: TargetNav| block_on(host.navigate(target.clone(), None, nav));

    // Before a terminal is attached a session move has no client to switch.
    assert!(matches!(
        navigate(TargetNav::NextSession),
        Err(HostError::CommandFailed { .. })
    ));

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
        block_on(host.navigate(TerminalTarget::Shell, None, TargetNav::NextSession)),
        Ok(())
    );

    terminal.disconnect();
    host.disconnect();
}
