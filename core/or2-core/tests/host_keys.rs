//! Host-key trust on a host with several host keys, as real hosts have (ED25519, ECDSA and RSA):
//! the host must present the key the phone trusts, so a trusted host never prompts, whatever
//! kind of key is trusted, through everything the app does with a connection after it is up
//! (the orphan reap, the capability probe, tmux, SSH and mosh terminals, a network change, a
//! reconnect). A host that no longer has any trusted key still gets the changed-key prompt.
//!
//! Disposable loopback OpenSSH only (temporary host keys, private tmux socket directory);
//! skips without sshd, tmux or mosh-server unless the matching `OR2_REQUIRE_*` is set.

mod common;

use std::fmt::Debug;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use common::{MoshReaper, Sshd, mosh_ready, sshd_ready, tmux_ready};
use or2_core::host::{
    HostConnectRequest, HostHandle, HostObserver, HostState, TerminalTarget, TerminalTransport,
};
use or2_core::keys::ClientKey;
use or2_core::session::{CloseReason, SessionObserver, SessionState};
use or2_core::ssh::{connect_host, network_changed};
use or2_core::term::TerminalSize;

const WAIT: Duration = Duration::from_secs(15);

fn word(value: &impl Debug) -> String {
    format!("{value:?}")
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

/// Every host state, in order.
struct HostObs {
    tx: mpsc::Sender<HostState>,
    seen: Arc<Mutex<Vec<String>>>,
}

impl HostObserver for HostObs {
    fn state_changed(&self, state: &HostState) {
        self.seen.lock().unwrap().push(word(state));
        let _ = self.tx.send(state.clone());
    }
}

struct SessionObs(mpsc::Sender<SessionState>);

impl SessionObserver for SessionObs {
    fn state_changed(&self, state: &SessionState) {
        let _ = self.0.send(state.clone());
    }

    fn frame_ready(&self) {}
}

struct Connection {
    host: HostHandle,
    states: mpsc::Receiver<HostState>,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Connection {
    /// `connect_host` as the app calls it: the paired account's key and the host's trusted keys.
    fn start(sshd: &Sshd, key: &ClientKey, trusted: &[String]) -> Self {
        let request = HostConnectRequest::new(
            &[("127.0.0.1", sshd.port)],
            &Sshd::username(),
            &key.to_stored(),
            trusted,
        )
        .unwrap();
        let (tx, states) = mpsc::channel();
        let seen = Arc::default();
        let host = connect_host(
            request,
            Arc::new(HostObs {
                tx,
                seen: Arc::clone(&seen),
            }),
        );
        Self { host, states, seen }
    }

    fn next(&self) -> HostState {
        self.states.recv_timeout(WAIT).expect("a host state")
    }

    /// Straight to `Connected`, never through a host-key prompt.
    fn connected_without_prompt(&self, what: &str) {
        let first = self.next();
        assert_eq!(
            first,
            HostState::Authenticating,
            "{what}: the trusted host prompted ({first:?})"
        );
        assert_eq!(self.next(), HostState::Connected { address_index: 0 });
    }

    fn disconnect(&self) {
        self.host.disconnect();
        assert_eq!(self.next(), HostState::Closed(CloseReason::Disconnected));
    }

    fn open(&self, target: TerminalTarget, transport: TerminalTransport) {
        let (tx, states) = mpsc::channel();
        let terminal = self
            .host
            .open_terminal(
                target,
                transport,
                TerminalSize::new(80, 24).unwrap(),
                None,
                Arc::new(SessionObs(tx)),
            )
            .unwrap();
        assert_eq!(
            states.recv_timeout(WAIT).expect("a terminal state"),
            SessionState::Connected,
            "{transport:?} terminal"
        );
        terminal.disconnect();
    }
}

/// A host key of `kind` that no host here has.
fn foreign_host_key(kind: &str) -> String {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("key");
    assert!(
        Command::new("ssh-keygen")
            .args(["-q", "-t", kind, "-N", "", "-C", "", "-f"])
            .arg(&file)
            .stdin(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    std::fs::read_to_string(file.with_extension("pub"))
        .unwrap()
        .trim()
        .to_owned()
}

/// The incident this guards against: Easy pair trusted the host's ED25519 key, and the
/// connection that followed must never ask about a key, through the whole sequence the app runs
/// after `Connected`.
#[test]
fn a_paired_ed25519_key_is_enough_on_a_host_with_every_kind_of_host_key() {
    if !(sshd_ready() && tmux_ready() && mosh_ready()) {
        return;
    }
    let sshd = Sshd::with_every_host_key();
    let _reaper = MoshReaper::new(&sshd);
    let key = ClientKey::generate_ed25519("");
    sshd.authorize(&key);
    let trusted = [sshd.host.clone()];

    let connection = Connection::start(&sshd, &key, &trusted);
    connection.connected_without_prompt("first connection after pairing");
    // What HostConnections does on Connected: reap orphaned mosh servers, then probe.
    block_on(async {
        connection.host.stop_mosh_server(4_194_000).await.unwrap();
        connection.host.capabilities().await.unwrap();
        connection.host.list_tmux_sessions().await.unwrap();
    });
    connection.open(TerminalTarget::Shell, TerminalTransport::Ssh);
    connection.open(
        TerminalTarget::Tmux {
            session_name: "or2-keys".into(),
        },
        TerminalTransport::Ssh,
    );
    connection.open(TerminalTarget::Shell, TerminalTransport::Mosh);
    network_changed();
    block_on(connection.host.capabilities()).unwrap();
    connection.disconnect();
    assert_eq!(
        *connection.seen.lock().unwrap(),
        ["Authenticating", "Connected", "Closed"]
    );

    // The next connection (the app reconnecting) is the same.
    let again = Connection::start(&sshd, &key, &trusted);
    again.connected_without_prompt("reconnect");
    again.disconnect();
}

/// The client asks for the algorithms of the trusted keys first, so a host with several keys
/// presents a trusted one whichever kind it is: never a prompt for the host's ED25519 key when
/// its RSA or ECDSA key is the trusted one.
#[test]
fn a_trusted_key_of_any_kind_is_the_one_presented() {
    if !sshd_ready() {
        return;
    }
    let sshd = Sshd::with_every_host_key();
    let key = ClientKey::generate_ed25519("");
    sshd.authorize(&key);
    let [ecdsa, rsa] = [&sshd.other_host_keys[0], &sshd.other_host_keys[1]];
    assert!(ecdsa.starts_with("ecdsa-sha2-nistp256 "), "{ecdsa}");
    assert!(rsa.starts_with("ssh-rsa "), "{rsa}");
    let cases: [(&str, Vec<String>); 4] = [
        ("ED25519", vec![sshd.host.clone()]),
        ("ECDSA", vec![ecdsa.clone()]),
        ("RSA", vec![rsa.clone()]),
        ("RSA and ECDSA", vec![rsa.clone(), ecdsa.clone()]),
    ];
    for (what, trusted) in cases {
        let connection = Connection::start(&sshd, &key, &trusted);
        connection.connected_without_prompt(what);
        connection.disconnect();
    }
}

/// Trust that no longer matches any key of the host is still the changed-key prompt (never a
/// failed handshake): every algorithm stays on offer, the trusted ones first, so the host
/// presents its key of the trusted kind.
#[test]
fn a_host_without_any_trusted_key_still_prompts_with_the_key_of_the_trusted_kind() {
    if !sshd_ready() {
        return;
    }
    let sshd = Sshd::with_every_host_key();
    let key = ClientKey::generate_ed25519("");
    sshd.authorize(&key);
    let cases = [
        (foreign_host_key("ed25519"), sshd.host.clone()),
        (foreign_host_key("ecdsa"), sshd.other_host_keys[0].clone()),
    ];
    for (stale, presented) in cases {
        let connection = Connection::start(&sshd, &key, std::slice::from_ref(&stale));
        let HostState::AwaitingHostKey(prompt) = connection.next() else {
            panic!("expected the changed-key prompt")
        };
        assert_eq!(prompt.previously_trusted.len(), 1);
        assert_eq!(prompt.previously_trusted[0].info().openssh, stale);
        assert_eq!(prompt.presented.info().openssh, presented);
        connection.host.reject_host_key().unwrap();
        assert!(matches!(connection.next(), HostState::Closed(_)));
    }

    // First use (nothing trusted) asks about the key the default order prefers: ED25519.
    let connection = Connection::start(&sshd, &key, &[]);
    let HostState::AwaitingHostKey(prompt) = connection.next() else {
        panic!("expected the first-use prompt")
    };
    assert!(prompt.previously_trusted.is_empty());
    assert_eq!(prompt.presented.info().openssh, sshd.host);
    connection.host.reject_host_key().unwrap();
    assert!(matches!(connection.next(), HostState::Closed(_)));
}
