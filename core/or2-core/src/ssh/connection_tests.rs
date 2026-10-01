//! Host driver tests that need no sshd: connection failures, and an in-process russh server
//! that answers exec requests so the query paths (probe caching, output cap, timeout,
//! refusal, loss) can be driven through the public `HostHandle`.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc as sync;

use russh::server;
use tokio::net::TcpListener;

use super::*;
use crate::herdr::{HerdrObserver, HerdrState, HerdrUnavailable};
use crate::host::{TerminalTarget, TmuxSession};
use crate::keys::ClientKey;
use crate::session::{SessionFailure, SessionObserver, SessionState};
use crate::ssh::connect_host;
use crate::term::TerminalSize;
use crate::transport::DirectTcp;

struct Recorder(sync::Sender<(HostState, std::thread::ThreadId)>);

impl HostObserver for Recorder {
    fn state_changed(&self, state: &HostState) {
        let _ = self.0.send((state.clone(), std::thread::current().id()));
    }
}

fn recorder() -> (
    Arc<Recorder>,
    sync::Receiver<(HostState, std::thread::ThreadId)>,
) {
    let (sender, states) = sync::channel();
    (Arc::new(Recorder(sender)), states)
}

fn next(states: &sync::Receiver<(HostState, std::thread::ThreadId)>) -> HostState {
    let (state, thread) = states.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_ne!(
        thread,
        std::thread::current().id(),
        "callbacks are Rust-owned"
    );
    state
}

fn closed(states: &sync::Receiver<(HostState, std::thread::ThreadId)>) -> CloseReason {
    let HostState::Closed(reason) = next(states) else {
        panic!("expected closed")
    };
    assert!(
        states.recv_timeout(Duration::from_millis(100)).is_err(),
        "Closed is delivered once, last"
    );
    reason
}

/// A port with nothing listening.
fn dead_port() -> u16 {
    std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn request(addresses: &[(&str, u16)], trusted: &[String]) -> HostConnectRequest {
    let key = ClientKey::generate_ed25519("k").to_stored();
    HostConnectRequest::new(addresses, "fixture", &key, trusted).unwrap()
}

fn fast() -> HostOptions {
    HostOptions {
        connect_timeout: Duration::from_millis(150),
        ..HostOptions::default()
    }
}

#[test]
fn unreachable_addresses_close_with_each_error_listed_from_a_rust_thread() {
    let (observer, states) = recorder();
    let request = request(
        &[("127.0.0.1", dead_port()), ("127.0.0.1", dead_port())],
        &[],
    );
    let handle = connect_host(request, observer);
    let CloseReason::Failed(SessionFailure::Unreachable(message)) = closed(&states) else {
        panic!("expected Unreachable")
    };
    assert!(
        message.contains("address 0: ConnectionRefused"),
        "{message}"
    );
    assert!(
        message.contains("address 1: ConnectionRefused"),
        "{message}"
    );
    assert!(
        !message.contains("127.0.0.1"),
        "no host names in diagnostics"
    );
    assert!(matches!(handle.state(), HostState::Closed(_)));
}

#[test]
fn a_peer_that_never_speaks_ssh_times_out_and_disconnect_cancels_it() {
    for disconnect in [false, true] {
        let (listener, port) = runtime().block_on(async {
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let port = listener.local_addr().unwrap().port();
            (listener, port)
        });
        // Accept and read everything the client sends, never answer; ends when it hangs up.
        let peer = runtime().spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(&mut socket, &mut bytes)
                .await
                .unwrap();
            bytes
        });
        let (observer, states) = recorder();
        let handle = connect_host_with_options(port, observer, fast());
        if disconnect {
            std::thread::sleep(Duration::from_millis(30));
            handle.disconnect();
        }
        assert_eq!(
            closed(&states),
            if disconnect {
                CloseReason::Disconnected
            } else {
                CloseReason::Failed(SessionFailure::TimedOut)
            }
        );
        let bytes = runtime().block_on(async {
            tokio::time::timeout(Duration::from_secs(2), peer)
                .await
                .unwrap()
                .unwrap()
        });
        assert!(bytes.starts_with(b"SSH-2.0-"));
    }
}

fn connect_host_with_options(
    port: u16,
    observer: Arc<Recorder>,
    options: HostOptions,
) -> HostHandle {
    start(
        Arc::new(DirectTcp),
        request(&[("127.0.0.1", port)], &[]),
        observer,
        options,
    )
}

// ---------------------------------------------------------------------------------------
// An in-process SSH server that answers exec requests.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Listing {
    Sessions,
    NoServer,
    Huge,
    Hang,
    Refuse,
    Lose,
}

const PROBE_WITH_TMUX: &str = "or2:tmux:/fake/tmux\nor2:locale:C.UTF-8\nor2:end\n";
const PROBE_WITHOUT_PROGRAMS: &str = "or2:tmux:\nor2:herdr:\nor2:locale:C.UTF-8\nor2:end\n";

struct Shared {
    listing: Mutex<Listing>,
    probes: AtomicUsize,
    /// What the capability probe prints.
    probe: Mutex<&'static str>,
}

struct Server {
    client_key: russh::keys::PublicKey,
    shared: Arc<Shared>,
}

impl server::Handler for Server {
    type Error = russh::Error;

    async fn auth_publickey(
        &mut self,
        _: &str,
        key: &russh::keys::PublicKey,
    ) -> Result<server::Auth, Self::Error> {
        Ok(if key == &self.client_key {
            server::Auth::Accept
        } else {
            server::Auth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        drop(channel);
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: russh::ChannelId,
        command: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(command).into_owned();
        if command.starts_with("sh -c") {
            self.shared.probes.fetch_add(1, Ordering::SeqCst);
            session.channel_success(channel)?;
            let output = *self.shared.probe.lock().unwrap();
            session.data(channel, output.as_bytes().to_vec())?;
            return finish(session, channel, 0);
        }
        assert!(command.contains("list-sessions"), "{command}");
        let listing = *self.shared.listing.lock().unwrap();
        match listing {
            Listing::Sessions => {
                session.channel_success(channel)?;
                session.data(channel, b"1:0:10:20:older\n2:1:11:90:newer\n".to_vec())?;
                finish(session, channel, 0)
            }
            Listing::NoServer => {
                session.channel_success(channel)?;
                session.extended_data(
                    channel,
                    1,
                    b"no server running on /tmp/tmux-1000/default\n".to_vec(),
                )?;
                finish(session, channel, 1)
            }
            Listing::Huge => {
                session.channel_success(channel)?;
                for _ in 0..65 {
                    session.data(channel, vec![b'x'; 16 * 1024])?;
                }
                finish(session, channel, 0)
            }
            Listing::Hang => Ok(()),
            Listing::Refuse => {
                session.channel_failure(channel)?;
                Ok(())
            }
            Listing::Lose => Err(russh::Error::Disconnect),
        }
    }
}

fn finish(
    session: &mut server::Session,
    channel: russh::ChannelId,
    status: u32,
) -> Result<(), russh::Error> {
    session.exit_status_request(channel, status)?;
    session.eof(channel)?;
    session.close(channel)?;
    Ok(())
}

struct Fixture {
    handle: HostHandle,
    states: sync::Receiver<(HostState, std::thread::ThreadId)>,
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    /// A host key trusted up front, so the connection goes straight to `Connected`.
    fn connected(exec_timeout: Duration) -> Self {
        Self::connected_with(exec_timeout, PROBE_WITH_TMUX)
    }

    fn connected_with(exec_timeout: Duration, probe: &'static str) -> Self {
        let host = ClientKey::generate_ed25519("");
        let host_openssh = host.public_key().openssh;
        let key = ClientKey::generate_ed25519("");
        let client_key = key.private_key().public_key().clone();
        let shared = Arc::new(Shared {
            listing: Mutex::new(Listing::Sessions),
            probes: AtomicUsize::new(0),
            probe: Mutex::new(probe),
        });
        let (listener, port) = runtime().block_on(async {
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let port = listener.local_addr().unwrap().port();
            (listener, port)
        });
        let config = Arc::new(server::Config {
            keys: vec![host.private_key().clone()],
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });
        let handler = Server {
            client_key,
            shared: shared.clone(),
        };
        let task = runtime().spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let session = server::run_stream(config, socket, handler).await.unwrap();
            let _ = session.await;
        });
        let (observer, states) = recorder();
        let request = HostConnectRequest::new(
            &[("127.0.0.1", port)],
            "fixture",
            &key.to_stored(),
            &[host_openssh],
        )
        .unwrap();
        let handle = start(
            Arc::new(DirectTcp),
            request,
            observer,
            HostOptions {
                exec_timeout,
                ..HostOptions::default()
            },
        );
        let fixture = Self {
            handle,
            states,
            shared,
            task,
        };
        assert_eq!(next(&fixture.states), HostState::Authenticating);
        assert_eq!(
            next(&fixture.states),
            HostState::Connected { address_index: 0 }
        );
        fixture
    }

    fn list(&self, listing: Listing) -> Result<Vec<TmuxSession>, HostError> {
        *self.shared.listing.lock().unwrap() = listing;
        runtime().block_on(self.handle.list_tmux_sessions())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[test]
fn capabilities_are_probed_once_and_tmux_listings_sort_and_survive_no_server() {
    let fixture = Fixture::connected(Duration::from_secs(5));
    for _ in 0..2 {
        let caps = runtime().block_on(fixture.handle.capabilities()).unwrap();
        assert_eq!(caps.tmux.as_deref(), Some("/fake/tmux"));
        assert_eq!(caps.herdr, None);
        assert_eq!(caps.utf8_locale, "C.UTF-8");
    }
    let names = |sessions: Vec<TmuxSession>| -> Vec<String> {
        sessions.into_iter().map(|s| s.name).collect()
    };
    assert_eq!(
        names(fixture.list(Listing::Sessions).unwrap()),
        ["newer", "older"]
    );
    // The listing needed the cached probe, not a second one.
    assert_eq!(fixture.shared.probes.load(Ordering::SeqCst), 1);
    assert!(fixture.list(Listing::NoServer).unwrap().is_empty());
    fixture.handle.disconnect();
}

#[test]
fn exec_output_over_the_cap_a_hung_command_and_a_refused_one_fail_typed() {
    let fixture = Fixture::connected(Duration::from_millis(400));
    let Err(HostError::CommandFailed { message }) = fixture.list(Listing::Huge) else {
        panic!("expected CommandFailed")
    };
    assert!(message.contains("1 MiB"), "{message}");
    let Err(HostError::CommandFailed { message }) = fixture.list(Listing::Hang) else {
        panic!("expected CommandFailed")
    };
    assert!(message.contains("did not finish in time"), "{message}");
    let Err(HostError::CommandFailed { message }) = fixture.list(Listing::Refuse) else {
        panic!("expected CommandFailed")
    };
    assert!(message.contains("refused"), "{message}");
    // The connection survives every failed exec.
    assert_eq!(fixture.list(Listing::Sessions).unwrap().len(), 2);
    assert!(matches!(
        fixture.handle.state(),
        HostState::Connected { .. }
    ));
    fixture.handle.disconnect();
    assert_eq!(closed(&fixture.states), CloseReason::Disconnected);
}

#[test]
fn losing_the_connection_closes_the_host_with_connection_lost_and_fails_the_query() {
    let fixture = Fixture::connected(Duration::from_secs(5));
    assert_eq!(fixture.list(Listing::Lose), Err(HostError::Closed));
    let CloseReason::Failed(SessionFailure::ConnectionLost(message)) = closed(&fixture.states)
    else {
        panic!("expected ConnectionLost")
    };
    assert!(!message.is_empty());
    assert_eq!(
        runtime().block_on(fixture.handle.list_tmux_sessions()),
        Err(HostError::Closed)
    );
}

struct SessionRecorder(sync::Sender<SessionState>);

impl SessionObserver for SessionRecorder {
    fn state_changed(&self, state: &SessionState) {
        let _ = self.0.send(state.clone());
    }

    fn frame_ready(&self) {}
}

struct WatchRecorder(sync::Sender<HerdrState>);

impl HerdrObserver for WatchRecorder {
    fn state_changed(&self, state: &HerdrState) {
        let _ = self.0.send(state.clone());
    }
}

#[test]
fn a_host_without_tmux_or_herdr_reports_not_installed_everywhere_without_opening_channels() {
    let fixture = Fixture::connected_with(Duration::from_secs(5), PROBE_WITHOUT_PROGRAMS);
    let caps = runtime().block_on(fixture.handle.capabilities()).unwrap();
    assert_eq!((caps.tmux, caps.herdr), (None, None));
    assert_eq!(
        runtime().block_on(fixture.handle.list_tmux_sessions()),
        Err(HostError::NotInstalled {
            program: "tmux".into()
        })
    );
    for (target, program) in [
        (
            TerminalTarget::Tmux {
                session_name: "work".into(),
            },
            "tmux",
        ),
        (
            TerminalTarget::Herdr {
                session: None,
                pane_id: Some("w1:p1".into()),
            },
            "herdr",
        ),
    ] {
        let (tx, states) = sync::channel();
        let _session = fixture
            .handle
            .open_terminal(
                target,
                TerminalSize::new(80, 24).unwrap(),
                Arc::new(SessionRecorder(tx)),
            )
            .unwrap();
        // Closed from `Connecting`: no channel was ever needed (this server has no PTY
        // support at all, so reaching one would fail differently).
        assert_eq!(
            states.recv_timeout(Duration::from_secs(5)).unwrap(),
            SessionState::Closed(CloseReason::Failed(SessionFailure::NotInstalled {
                program: program.into()
            }))
        );
        assert!(states.recv_timeout(Duration::from_millis(100)).is_err());
    }

    let (tx, watch_states) = sync::channel();
    let watch = fixture
        .handle
        .watch_herdr(None, Arc::new(WatchRecorder(tx)))
        .unwrap();
    assert_eq!(
        watch_states.recv_timeout(Duration::from_secs(5)).unwrap(),
        HerdrState::Unavailable {
            reason: HerdrUnavailable::NotInstalled,
            message: "herdr is not installed on the host".into()
        }
    );
    watch.stop();
    assert_eq!(
        watch_states.recv_timeout(Duration::from_secs(5)).unwrap(),
        HerdrState::Closed
    );
    assert!(
        watch_states
            .recv_timeout(Duration::from_millis(100))
            .is_err()
    );
    fixture.handle.disconnect();
}
