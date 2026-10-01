//! Live interop with a real `mosh-server` on this machine (feature `test-support`).
//!
//! The server is started through `mosh::bootstrap` over `LocalHost` (so the exact command line
//! M2 uses is exercised), the client connects over 127.0.0.1 with `mosh::start_with`, and the
//! test runs a command, resizes, roams and disconnects. The server this test started is killed
//! by exact process id however the test ends, and nothing else is touched. Skipped, with a
//! message, when `mosh-server` is not installed; `OR2_REQUIRE_MOSH` turns the skip into a
//! failure so CI cannot pass vacuously.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use or2_core::frame::{CellWidth, Frame};
use or2_core::host::HostCapabilities;
use or2_core::mosh::{self, MoshParams};
use or2_core::remote::{ExecOutput, LocalHost, RemoteError, RemoteHost};
use or2_core::session::{CloseReason, SessionHandle, SessionObserver, SessionState};
use or2_core::term::TerminalSize;
use or2_core::transport::{DatagramSocket, DatagramTransport, DirectUdp, Endpoint};
use tokio::net::UdpSocket;

fn mosh_server() -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join("mosh-server"))
        .chain([PathBuf::from("/usr/bin/mosh-server")])
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

/// Kills the one `mosh-server` this test started, and whatever it spawned, when dropped, even
/// during a panic. It only ever signals the exact process ids it was given, and re-checks that
/// each still is what it was before signalling, so a recycled id is never hit.
struct ServerGuard {
    /// `(pid, executable name)` pairs: the server, then its direct children.
    processes: Vec<(u32, String)>,
}

fn comm(pid: u32) -> Option<String> {
    fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|name| name.trim().to_owned())
}

fn children(pid: u32) -> Vec<u32> {
    fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
        .map(|text| {
            text.split_whitespace()
                .filter_map(|id| id.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

impl ServerGuard {
    fn new(pid: u32) -> Self {
        let name = comm(pid).expect("the reported mosh-server pid is not running");
        assert_eq!(name, "mosh-server", "pid {pid} is not a mosh-server");
        Self {
            processes: vec![(pid, name)],
        }
    }

    /// Remembers the server's children (its shell) so they can be cleaned up exactly too.
    fn note_children(&mut self) {
        let server = self.processes[0].0;
        for child in children(server) {
            if let Some(name) = comm(child)
                && !self.processes.iter().any(|(pid, _)| *pid == child)
            {
                self.processes.push((child, name));
            }
        }
    }

    fn server_running(&self) -> bool {
        let (pid, name) = &self.processes[0];
        comm(*pid).as_deref() == Some(name.as_str())
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        self.note_children();
        for (pid, name) in &self.processes {
            if comm(*pid).as_deref() == Some(name.as_str()) {
                let _ = Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .status();
            }
        }
    }
}

/// A transport that records which sockets exist and how much each has received.
#[derive(Clone, Default)]
struct Recording {
    sockets: Arc<Mutex<Vec<Arc<AtomicUsize>>>>,
}

struct RecordedSocket {
    inner: UdpSocket,
    received: Arc<AtomicUsize>,
}

impl DatagramSocket for RecordedSocket {
    fn local_addr(&self) -> io::Result<std::net::SocketAddr> {
        DatagramSocket::local_addr(&self.inner)
    }

    fn peer_addr(&self) -> io::Result<std::net::SocketAddr> {
        DatagramSocket::peer_addr(&self.inner)
    }

    fn try_send(&self, datagram: &[u8]) -> io::Result<usize> {
        DatagramSocket::try_send(&self.inner, datagram)
    }

    fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let polled = DatagramSocket::poll_recv(&self.inner, cx, buf);
        if let Poll::Ready(Ok(length)) = &polled {
            self.received.fetch_add(*length, Ordering::SeqCst);
        }
        polled
    }
}

impl DatagramTransport for Recording {
    type Socket = RecordedSocket;

    async fn bind(&self, endpoint: &Endpoint) -> io::Result<RecordedSocket> {
        let inner = DirectUdp.bind(endpoint).await?;
        let received = Arc::new(AtomicUsize::new(0));
        self.sockets.lock().unwrap().push(received.clone());
        Ok(RecordedSocket { inner, received })
    }
}

/// A host that behaves like sshd for commands: `SSH_CONNECTION` is set to a loopback connection.
/// `mosh-server -s` binds to the server address in it, so this makes the test independent of
/// whatever ssh session the developer's own shell happens to be in, and exercises `-s`.
struct LoopbackSsh(LocalHost);

impl RemoteHost for LoopbackSsh {
    type Stream = <LocalHost as RemoteHost>::Stream;

    async fn exec_rendered(&self, line: &str) -> Result<ExecOutput, RemoteError> {
        self.0
            .exec_rendered(&format!(
                "SSH_CONNECTION='127.0.0.1 40000 127.0.0.1 22'; export SSH_CONNECTION; {line}"
            ))
            .await
    }

    async fn open_unix(&self, path: &str) -> Result<Self::Stream, RemoteError> {
        self.0.open_unix(path).await
    }
}

struct Observer(Mutex<mpsc::Sender<SessionState>>);

impl SessionObserver for Observer {
    fn state_changed(&self, state: &SessionState) {
        let _ = self.0.lock().unwrap().send(state.clone());
    }

    fn frame_ready(&self) {}
}

/// The renderer's view of the frames, merged the way Kotlin merges them.
#[derive(Default)]
struct Grid {
    rows: Vec<String>,
    size: Option<TerminalSize>,
}

impl Grid {
    fn merge(&mut self, frame: &Frame) {
        if frame.is_full() {
            self.rows = vec![String::new(); usize::from(frame.size().rows())];
            self.size = Some(frame.size());
        } else {
            assert_eq!(
                self.size,
                Some(frame.size()),
                "a delta matches the held grid"
            );
        }
        for row in frame.rows() {
            self.rows[usize::from(row.index())] = row
                .cells()
                .iter()
                .filter(|cell| cell.width != CellWidth::SpacerTail)
                .map(|cell| cell.text.as_str())
                .collect::<String>()
                .trim_end()
                .to_owned();
        }
    }

    fn wait_for(&mut self, handle: &SessionHandle, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(taken) = handle.take_frame() {
                self.merge(&taken.frame);
            }
            if self.rows.join("\n").contains(expected) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "never saw {expected:?}; screen: {:#?}",
                self.rows
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn expect_state(states: &mpsc::Receiver<SessionState>, expected: SessionState) {
    assert_eq!(
        states.recv_timeout(Duration::from_secs(15)).unwrap(),
        expected
    );
}

#[tokio::test]
async fn a_real_mosh_server_session() {
    let Some(server) = mosh_server() else {
        assert!(
            std::env::var_os("OR2_REQUIRE_MOSH").is_none(),
            "mosh-server is required (OR2_REQUIRE_MOSH) but not installed"
        );
        eprintln!("SKIP: mosh-server is not installed");
        return;
    };
    let caps = HostCapabilities {
        tmux: None,
        herdr: None,
        mosh_server: Some(server),
        utf8_locale: "C.UTF-8".into(),
        herdr_sessions: Vec::new(),
    };
    let size = TerminalSize::new(80, 24).unwrap();
    let shell = ["/bin/bash", "--norc", "--noprofile"].map(String::from);
    let params: MoshParams = mosh::bootstrap(&LoopbackSsh(LocalHost::new()), &caps, size, &shell)
        .await
        .expect("bootstrap a local mosh-server");
    // From here on the guard owns the server's lifetime.
    let pid = params.server_pid.expect("mosh-server reports its pid");
    let mut guard = ServerGuard::new(pid);
    assert!(format!("{params:?}").contains("redacted"), "{params:?}");

    let transport = Recording::default();
    let (sender, states) = mpsc::channel();
    let (handle, control) = mosh::start_with(
        transport.clone(),
        params,
        "127.0.0.1",
        Arc::new(Observer(Mutex::new(sender))),
        None,
    )
    .unwrap();
    let mut grid = Grid::default();

    // Connected once the real server authenticates; the first frame shows the shell prompt.
    expect_state(&states, SessionState::Connected);
    grid.wait_for(&handle, "$");
    guard.note_children();

    // A command's output reaches the screen. The arithmetic proves it ran rather than echoed.
    handle.send_text("echo out-$((6*7))\n".into()).unwrap();
    grid.wait_for(&handle, "out-42");

    // A resize reaches the PTY: `stty size` reports rows then columns.
    handle.resize(TerminalSize::new(100, 30).unwrap()).unwrap();
    handle.send_text("stty size\n".into()).unwrap();
    grid.wait_for(&handle, "30 100");
    assert_eq!(grid.size, Some(TerminalSize::new(100, 30).unwrap()));

    // Roaming: a new socket is opened, and the server's replies move to it.
    let sockets_before = transport.sockets.lock().unwrap().len();
    assert_eq!(sockets_before, 1);
    control.roam();
    let deadline = Instant::now() + Duration::from_secs(10);
    while transport.sockets.lock().unwrap().len() < 2 {
        assert!(Instant::now() < deadline, "the client never rebound");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let (old, new) = {
        let sockets = transport.sockets.lock().unwrap();
        (sockets[0].clone(), sockets[1].clone())
    };
    handle.send_text("echo roam-$((6*8))\n".into()).unwrap();
    grid.wait_for(&handle, "roam-48");
    assert!(
        new.load(Ordering::SeqCst) > 0,
        "the server should answer on the new socket after the roam"
    );
    let old_after_roam = old.load(Ordering::SeqCst);
    handle.send_text("echo again-$((6*9))\n".into()).unwrap();
    grid.wait_for(&handle, "again-54");
    assert_eq!(
        old.load(Ordering::SeqCst),
        old_after_roam,
        "once it follows the new source the server stops answering the old port"
    );

    // Disconnect tells the server, which ends its session.
    assert!(guard.server_running());
    handle.disconnect();
    expect_state(&states, SessionState::Closed(CloseReason::Disconnected));
    let deadline = Instant::now() + Duration::from_secs(10);
    while guard.server_running() {
        assert!(
            Instant::now() < deadline,
            "mosh-server outlived a client shutdown"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
