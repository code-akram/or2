//! Live interop with a real `mosh-server` on this machine (feature `test-support`).
//!
//! The server is started through `mosh::bootstrap` over `LocalHost` (so the exact command line
//! M2 uses is exercised), the client connects over 127.0.0.1 with `mosh::run_session` (the driver
//! a host runs its mosh terminals on), and the test runs a command, resizes, roams and
//! disconnects. The server this test started is killed by exact process id however the test ends,
//! and nothing else is touched. Skipped, with a message, when `mosh-server` is not installed;
//! `OR2_REQUIRE_MOSH` turns the skip into a failure so CI cannot pass vacuously.

use std::fs;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
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
use or2_core::session::{self, CloseReason, SessionHandle, SessionObserver, SessionState};
use or2_core::term::TerminalSize;
use or2_core::transport::{DatagramSocket, DatagramTransport, DirectUdp, Endpoint};
use tokio::net::UdpSocket;
use tokio::sync::Notify;

fn mosh_server() -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join("mosh-server"))
        .chain([PathBuf::from("/usr/bin/mosh-server")])
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

/// Kills the one `mosh-server` this test started, and whatever it spawned, when dropped, even
/// during a panic. It only ever signals the exact process ids it identified, and re-checks that
/// each still is what it was before signalling, so a recycled id is never hit.
///
/// It is armed the moment the bootstrap returns, before anything can fail: from the pid the
/// server printed or, if it printed none, from whichever process holds the UDP port it said it
/// listens on.
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

/// Socket links for this fixture's IPv4 loopback endpoint, never the same port on another
/// interface: a developer's real mosh-server may use that port too. `/proc/net/udp` writes the
/// IPv4 address in native byte order (this Linux fixture runs on little-endian hosts).
fn loopback_udp_inodes(text: &str, port: u16) -> Vec<String> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (address, local_port) = fields.get(1)?.rsplit_once(':')?;
            (address == "0100007F" && u16::from_str_radix(local_port, 16).ok() == Some(port))
                .then(|| fields.get(9).map(|inode| format!("socket:[{inode}]")))
                .flatten()
        })
        .collect()
}

/// The processes of this user holding the fixture's UDP socket on `127.0.0.1:port`, found by
/// its inode in `/proc/net/udp` and the `socket:[inode]` links under `/proc/<pid>/fd`.
fn udp_port_owners(port: u16) -> Vec<u32> {
    let inodes = fs::read_to_string("/proc/net/udp")
        .map(|text| loopback_udp_inodes(&text, port))
        .unwrap_or_default();
    let mut owners = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return owners;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(fds) = fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        let holds = fds.flatten().any(|fd| {
            fs::read_link(fd.path())
                .map(|target| inodes.contains(&target.to_string_lossy().into_owned()))
                .unwrap_or(false)
        });
        if holds {
            owners.push(pid);
        }
    }
    owners
}

impl ServerGuard {
    /// Arms the guard with the server's process: the pid it reported, else the owner of its UDP
    /// port, and only a process that really is a `mosh-server`. Never panics, so that it can be
    /// built before any assertion; check [`ServerGuard::armed`] after.
    fn adopt(reported: Option<u32>, port: u16) -> Self {
        let mut processes: Vec<(u32, String)> = Vec::new();
        for pid in reported.into_iter().chain(udp_port_owners(port)) {
            if comm(pid).as_deref() == Some("mosh-server")
                && processes.iter().all(|(p, _)| *p != pid)
            {
                processes.push((pid, "mosh-server".to_owned()));
            }
        }
        Self { processes }
    }

    fn armed(&self) -> bool {
        !self.processes.is_empty()
    }

    /// Remembers the server's children (its shell) so they can be cleaned up exactly too.
    fn note_children(&mut self) {
        let Some(&(server, _)) = self.processes.first() else {
            return;
        };
        for child in children(server) {
            if let Some(name) = comm(child)
                && !self.processes.iter().any(|(pid, _)| *pid == child)
            {
                self.processes.push((child, name));
            }
        }
    }

    fn server_running(&self) -> bool {
        self.processes
            .first()
            .is_some_and(|(pid, name)| comm(*pid).as_deref() == Some(name.as_str()))
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

/// Starts a real `mosh-server` running bash, through the code M2 uses, and arms the guard over
/// it at once. `None` (after saying so) when `mosh-server` is not installed.
async fn bootstrap_local() -> Option<(MoshParams, ServerGuard)> {
    let Some(server) = mosh_server() else {
        assert!(
            std::env::var_os("OR2_REQUIRE_MOSH").is_none(),
            "mosh-server is required (OR2_REQUIRE_MOSH) but not installed"
        );
        eprintln!("SKIP: mosh-server is not installed");
        return None;
    };
    let caps = HostCapabilities {
        tmux: None,
        herdr: None,
        mosh_server: Some(server),
        tmux_records_clients: false,
        utf8_locale: "C.UTF-8".into(),
        herdr_sessions: Vec::new(),
    };
    let size = TerminalSize::new(80, 24).unwrap();
    let shell = ["/bin/bash", "--norc", "--noprofile"].map(String::from);
    let params: MoshParams = mosh::bootstrap(&LoopbackSsh(LocalHost::new()), &caps, size, &shell)
        .await
        .expect("bootstrap a local mosh-server");
    // From here on the guard owns the server's lifetime, before any assertion can fail.
    let guard = ServerGuard::adopt(params.server_pid, params.port);
    assert!(
        guard.armed(),
        "a mosh-server is listening on UDP port {} but could not be identified to stop it; \
         stop it by hand",
        params.port
    );
    assert!(
        params.server_pid.is_some(),
        "mosh-server did not report its pid (it was found by its port and will be stopped)"
    );
    Some((params, guard))
}

#[tokio::test]
async fn a_real_mosh_server_session() {
    let Some((params, mut guard)) = bootstrap_local().await else {
        return;
    };
    assert!(format!("{params:?}").contains("redacted"), "{params:?}");

    let transport = Recording::default();
    let (sender, states) = mpsc::channel();
    // The session driver the host runs a mosh terminal on, on a thread of its own as there.
    let (handle, mut driver) = session::channel(Arc::new(Observer(Mutex::new(sender))));
    let plan = mosh::Plan {
        transport: Arc::new(transport.clone()),
        peer: SocketAddr::new(Ipv4Addr::LOCALHOST.into(), params.port),
        params,
        shutdown: Arc::new(Notify::new()),
        connect_timeout: mosh::CONNECT_TIMEOUT,
        deadline: None,
    };
    let session = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                let ended = mosh::run_session(plan, &mut driver).await;
                driver.close(ended.reason);
            });
    });
    let mut grid = Grid::default();

    // Connected once the real server authenticates; the first frame shows the shell prompt.
    expect_state(&states, SessionState::Connected);
    grid.wait_for(&handle, "$");
    guard.note_children();

    // A command's output reaches the screen. The arithmetic proves it ran rather than echoed.
    handle.send_text("echo out-$((6*7))\n".into()).unwrap();
    grid.wait_for(&handle, "out-42");

    // A submit types the line and presses Enter on its own, so the shell runs it.
    handle.submit_text("echo sub-$((6*9+1))".into()).unwrap();
    grid.wait_for(&handle, "sub-55");

    // A resize reaches the PTY: `stty size` reports rows then columns.
    handle.resize(TerminalSize::new(100, 30).unwrap()).unwrap();
    handle.send_text("stty size\n".into()).unwrap();
    grid.wait_for(&handle, "30 100");
    assert_eq!(grid.size, Some(TerminalSize::new(100, 30).unwrap()));

    // Roaming: a new socket is opened, and the server's replies move to it.
    let sockets_before = transport.sockets.lock().unwrap().len();
    assert_eq!(sockets_before, 1);
    handle.roam();
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
    guard.note_children();
    // A datagram the server sent to the old port just before it followed the new source can
    // still be in flight or unread, so let those drain before counting.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (old_after_roam, new_after_roam) = (old.load(Ordering::SeqCst), new.load(Ordering::SeqCst));
    handle.send_text("echo again-$((6*9))\n".into()).unwrap();
    grid.wait_for(&handle, "again-54");
    assert!(
        new.load(Ordering::SeqCst) > new_after_roam,
        "the new socket carries the later output"
    );
    assert_eq!(
        old.load(Ordering::SeqCst),
        old_after_roam,
        "once it follows the new source the server stops answering the old port"
    );
    guard.note_children();

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
    session.join().unwrap();
}

/// A server nobody connected to (the UDP port was firewalled, say) is stopped by
/// `mosh::terminate`, and only a mosh-server is: a pid that is something else is left alone.
#[tokio::test]
async fn terminate_stops_a_server_nobody_connected_to() {
    let Some((params, guard)) = bootstrap_local().await else {
        return;
    };
    let pid = params.server_pid.expect("mosh-server reports its pid");
    let host = LoopbackSsh(LocalHost::new());

    // Without a reported pid the guard finds the server by the port it listens on. mosh-server
    // forks after binding, and the exiting parent (also a mosh-server) holds the socket until it
    // is gone: under load it can still be there, so wait for the server to be the only owner. (A
    // guard kills what it holds when dropped, so the wait reads the owners without making one.)
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let owners: Vec<_> = udp_port_owners(params.port)
            .into_iter()
            .filter(|owner| comm(*owner).as_deref() == Some("mosh-server"))
            .collect();
        if owners == [pid] {
            break;
        }
        // An empty scan, or just the exiting parent, is not a completed fork handover.
        assert!(
            Instant::now() < deadline,
            "UDP port {} never settled on the reported server {pid}; owners: {owners:?}",
            params.port
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let by_port = ServerGuard::adopt(None, params.port);
    assert!(by_port.armed() && by_port.processes[0].0 == pid);

    // This test process is not a mosh-server: nothing happens to it (or to anything else).
    mosh::terminate(&host, std::process::id()).await.unwrap();
    assert!(guard.server_running());

    mosh::terminate(&host, pid).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while guard.server_running() {
        assert!(
            Instant::now() < deadline,
            "mosh-server survived mosh::terminate"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // Already gone is not an error.
    mosh::terminate(&host, pid).await.unwrap();
}

#[test]
fn a_port_lookup_ignores_other_interfaces_and_malformed_entries() {
    let table =
        "sl local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode
0: 0100007F:EA60 00000000:0000 07 00000000:00000000 00:00000000 00000000 1000 0 101
1: 0100000A:EA60 00000000:0000 07 00000000:00000000 00:00000000 00000000 1000 0 202
2: 00000000:EA60 00000000:0000 07 00000000:00000000 00:00000000 00000000 1000 0 303
3: 0100007F:EA61 00000000:0000 07 00000000:00000000 00:00000000 00000000 1000 0 404
4: 0100007F:EA60
5: malformed
6: 0100007F:xxxx 00000000:0000 07 00000000:00000000 00:00000000 00000000 1000 0 505
";
    assert_eq!(loopback_udp_inodes(table, 60000), ["socket:[101]"]);
    assert_eq!(loopback_udp_inodes(table, 60001), ["socket:[404]"]);
    assert!(loopback_udp_inodes(table, 60002).is_empty());
}

#[tokio::test]
async fn a_udp_port_is_traced_to_the_process_holding_it() {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();
    assert!(udp_port_owners(port).contains(&std::process::id()));
}
