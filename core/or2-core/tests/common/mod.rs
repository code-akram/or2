//! Shared by the disposable OpenSSH integration tests (`host.rs`, `host_mosh.rs`). Only
//! temporary keys/configuration and an ephemeral loopback listener are used; no home keys,
//! system sshd or existing authorization are touched.
#![allow(dead_code)]

use std::fs;
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use or2_core::frame::{CellWidth, Row};
use or2_core::keys::ClientKey;
use or2_core::session::{SessionHandle, SessionState};
use or2_core::term::TerminalSize;
use or2_core::transport::{DatagramSocket, DatagramTransport, DirectUdp, Endpoint};

pub fn sshd_available() -> bool {
    Path::new("/usr/bin/sshd").exists()
}

/// Whether the sshd tests can run. A missing `/usr/bin/sshd` skips them (printing `SKIP`)
/// unless `OR2_REQUIRE_SSHD` is set, which fails instead: set it in CI so the suite is never
/// vacuously green.
pub fn sshd_ready() -> bool {
    ready("sshd", "OR2_REQUIRE_SSHD", sshd_available())
}

/// Like [`sshd_ready`] for `tmux` (`OR2_REQUIRE_TMUX`).
pub fn tmux_ready() -> bool {
    ready(
        "tmux",
        "OR2_REQUIRE_TMUX",
        Command::new("tmux").arg("-V").output().is_ok(),
    )
}

fn ready(program: &str, require: &str, available: bool) -> bool {
    if !available {
        assert!(
            std::env::var_os(require).is_none(),
            "{require} is set but {program} is absent"
        );
        eprintln!("SKIP: {program} is absent");
    }
    available
}

/// How many ports `Sshd` tries before giving up.
const BIND_ATTEMPTS: usize = 5;

/// A loopback port nothing listens on right now. Another process can take it before the caller
/// binds it, so callers that must bind it retry (see [`Sshd`]).
fn free_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Waits for sshd's "Server listening" line. `Err` carries the log when sshd exited or stayed
/// silent for 5 s.
fn wait_until_listening(child: &mut Child, log_path: &Path) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let log = fs::read_to_string(log_path).unwrap();
        if log.contains("Server listening on") {
            return Ok(());
        }
        if child.try_wait().unwrap().is_some() {
            // Everything sshd wrote before it exited is in the file now.
            return Err(fs::read_to_string(log_path).unwrap());
        }
        if Instant::now() >= deadline {
            return Err(log);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Whether sshd's log says it could not bind its port (it then exits).
fn bind_failed(log: &str) -> bool {
    log.contains("Address already in use") || log.contains("Cannot bind any address")
}

pub struct Sshd {
    pub directory: tempfile::TempDir,
    child: Child,
    pub port: u16,
    /// The host's public key, as an `authorized_keys`-style line.
    pub host: String,
    /// `TMUX_TMPDIR` of every session this sshd starts: tmux in these tests never touches the
    /// user's default socket.
    pub tmux_dir: PathBuf,
}

impl Sshd {
    pub fn new(certificate_only: bool) -> Self {
        Self::with_config(certificate_only, "")
    }

    /// [`Sshd::new`] with extra `sshd_config` lines appended (for example
    /// `AllowStreamLocalForwarding no`).
    pub fn with_config(certificate_only: bool, extra: &str) -> Self {
        Self::with_environment(certificate_only, extra, "")
    }

    /// [`Sshd::with_config`] with `environment` (`NAME=value` words) added to what every
    /// session of this sshd runs with. Only the first `SetEnv` line of a config counts, hence
    /// a parameter rather than another line in `extra`.
    pub fn with_environment(certificate_only: bool, extra: &str, environment: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path();
        let host = ClientKey::generate_ed25519("");
        fs::write(path.join("host"), &*host.to_stored()).unwrap();
        fs::set_permissions(path.join("host"), fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(path.join("authorized"), "").unwrap();
        let tmux_dir = path.join("tmux");
        fs::create_dir(&tmux_dir).unwrap();
        let mut config = format!(
            "ListenAddress {}\nHostKey {}\nAuthorizedKeysFile {}\nPidFile {}\nStrictModes no\nUsePAM no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPrintMotd no\nPrintLastLog no\nSetEnv HOME={} HISTFILE=/dev/null ENV=/dev/null BASH_ENV=/dev/null ZDOTDIR={} TMUX_TMPDIR={} {environment}\nLogLevel VERBOSE\n",
            Ipv4Addr::LOCALHOST,
            path.join("host").display(),
            path.join("authorized").display(),
            path.join("pid").display(),
            path.display(),
            path.display(),
            tmux_dir.display(),
        );
        if certificate_only {
            let ca = ClientKey::generate_ed25519("");
            fs::write(path.join("ca"), &*ca.to_stored()).unwrap();
            fs::set_permissions(path.join("ca"), fs::Permissions::from_mode(0o600)).unwrap();
            fs::write(path.join("host.pub"), host.public_key().openssh).unwrap();
            assert!(
                Command::new("ssh-keygen")
                    .args(["-q", "-s"])
                    .arg(path.join("ca"))
                    .args(["-I", "fixture", "-h", "-V", "+1h"])
                    .arg(path.join("host.pub"))
                    .status()
                    .unwrap()
                    .success()
            );
            config.push_str(&format!(
                "HostCertificate {}\nHostKeyAlgorithms ssh-ed25519-cert-v01@openssh.com\n",
                path.join("host-cert.pub").display()
            ));
        }
        config.push_str(extra);
        config.push('\n');
        // A free port is found by binding and releasing it, so another process (often a loopback
        // client's ephemeral port) can take it before sshd binds. sshd then exits with "Cannot
        // bind any address": start it again on a new port.
        let mut last_log = String::new();
        for _ in 0..BIND_ATTEMPTS {
            let port = free_port();
            fs::write(path.join("config"), format!("Port {port}\n{config}")).unwrap();
            let log_path = path.join("log");
            let mut child = Command::new("/usr/bin/sshd")
                .args(["-D", "-e", "-f"])
                .arg(path.join("config"))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(fs::File::create(&log_path).unwrap())
                .spawn()
                .unwrap();
            match wait_until_listening(&mut child, &log_path) {
                Ok(()) => {
                    return Self {
                        directory,
                        child,
                        port,
                        host: host.public_key().openssh,
                        tmux_dir,
                    };
                }
                Err(log) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    assert!(bind_failed(&log), "disposable sshd failed to start: {log}");
                    last_log = log;
                }
            }
        }
        panic!("disposable sshd could not bind a port in {BIND_ATTEMPTS} attempts: {last_log}");
    }

    pub fn username() -> String {
        let user = Command::new("id").arg("-un").output().unwrap();
        assert!(user.status.success());
        std::str::from_utf8(&user.stdout).unwrap().trim().to_owned()
    }

    /// Makes `key` the only key sshd accepts.
    pub fn authorize(&self, key: &ClientKey) {
        fs::write(
            self.directory.path().join("authorized"),
            key.public_key().openssh + "\n",
        )
        .unwrap();
    }

    /// The directory sshd sessions use as `$HOME`.
    pub fn home(&self) -> &Path {
        self.directory.path()
    }

    /// A `tmux` command bound to this sshd's private socket directory and a hermetic
    /// environment, so it can never reach the user's own tmux server.
    pub fn tmux(&self) -> Command {
        let mut command = Command::new("tmux");
        command
            .env("TMUX_TMPDIR", &self.tmux_dir)
            .env("HOME", self.home())
            .env("ZDOTDIR", self.home())
            .env_remove("TMUX")
            .stdin(Stdio::null());
        command
    }
}

impl Drop for Sshd {
    fn drop(&mut self) {
        // Only the private socket's server, if a test started one.
        if self.tmux_dir.starts_with(std::env::temp_dir()) {
            let _ = self
                .tmux()
                .arg("kill-server")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The text of a terminal as the renderer would show it, built from taken frames.
#[derive(Default)]
pub struct Grid {
    pub rows: Vec<Option<Row>>,
    pub full_seen: bool,
    pub size: Option<TerminalSize>,
}

impl Grid {
    /// Applies whatever frame is waiting; returns the screen text.
    pub fn refresh(&mut self, handle: &SessionHandle) -> String {
        if let Some(taken) = handle.take_frame() {
            let frame = taken.frame;
            if frame.is_full() {
                self.full_seen = true;
                self.rows = vec![None; usize::from(frame.size().rows())];
                self.size = Some(frame.size());
            } else {
                assert_eq!(self.size, Some(frame.size()));
            }
            for row in frame.rows() {
                self.rows[usize::from(row.index())] = Some(row.clone());
            }
        }
        self.rows
            .iter()
            .flatten()
            .map(|row| {
                let mut text: String = row
                    .cells()
                    .iter()
                    .map(|cell| {
                        if cell.width == CellWidth::SpacerTail {
                            ""
                        } else if cell.text.is_empty() {
                            " "
                        } else {
                            &cell.text
                        }
                    })
                    .collect();
                text.push('\n');
                text
            })
            .collect()
    }

    pub fn wait(&mut self, handle: &SessionHandle, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let text = self.refresh(handle);
            if text.contains(expected) {
                assert!(self.full_seen);
                return;
            }
            // Do not print the screen: it may contain the runner's account or hostname.
            assert!(
                Instant::now() < deadline,
                "expected output {expected:?} did not arrive in terminal frames"
            );
            assert!(
                !matches!(handle.state(), SessionState::Closed(_)),
                "session closed unexpectedly"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// True if `text` appears in the current screen (after applying any waiting frame).
    pub fn contains(&mut self, handle: &SessionHandle, text: &str) -> bool {
        self.refresh(handle).contains(text)
    }
}

/// A TCP relay the test can cut: loss without touching sshd.
pub struct Proxy {
    pub port: u16,
    connections: Arc<Mutex<Vec<TcpStream>>>,
    stop: Arc<AtomicBool>,
    /// Bytes relayed client to server and server to client so far.
    pub to_server: Arc<AtomicUsize>,
    pub to_client: Arc<AtomicUsize>,
    /// Milliseconds every relayed chunk is held before it is forwarded, in each direction: a
    /// slow link. Zero (the default) forwards at once; it can be changed while connected.
    pub latency_ms: Arc<AtomicU64>,
}

impl Proxy {
    pub fn new(target: u16) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let connections = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (kept, stopping) = (connections.clone(), stop.clone());
        let to_server = Arc::new(AtomicUsize::new(0));
        let to_client = Arc::new(AtomicUsize::new(0));
        let counters = (to_server.clone(), to_client.clone());
        let latency_ms = Arc::new(AtomicU64::new(0));
        let latency = latency_ms.clone();
        std::thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                let Ok((client, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                };
                client.set_nonblocking(false).unwrap();
                let upstream = TcpStream::connect((Ipv4Addr::LOCALHOST, target)).unwrap();
                for (mut from, mut sink, count) in [
                    (
                        client.try_clone().unwrap(),
                        upstream.try_clone().unwrap(),
                        counters.0.clone(),
                    ),
                    (
                        upstream.try_clone().unwrap(),
                        client.try_clone().unwrap(),
                        counters.1.clone(),
                    ),
                ] {
                    let latency = latency.clone();
                    // The reader stamps each chunk; the writer releases it `latency` after
                    // that stamp, so the delay does not add up over a burst of chunks.
                    let (stamped, due) = std::sync::mpsc::channel::<(Instant, Vec<u8>)>();
                    std::thread::spawn(move || {
                        for (stamp, chunk) in due {
                            let held = Duration::from_millis(latency.load(Ordering::SeqCst));
                            if let Some(wait) =
                                (stamp + held).checked_duration_since(Instant::now())
                            {
                                std::thread::sleep(wait);
                            }
                            if std::io::Write::write_all(&mut sink, &chunk).is_err() {
                                break;
                            }
                        }
                        let _ = sink.shutdown(Shutdown::Both);
                    });
                    std::thread::spawn(move || {
                        let mut buffer = [0u8; 16 * 1024];
                        while let Ok(length) = std::io::Read::read(&mut from, &mut buffer) {
                            if length == 0
                                || stamped
                                    .send((Instant::now(), buffer[..length].to_vec()))
                                    .is_err()
                            {
                                break;
                            }
                            count.fetch_add(length, Ordering::SeqCst);
                        }
                        // Dropping `stamped` lets the writer drain what is queued, then close.
                    });
                }
                kept.lock().unwrap().extend([client, upstream]);
            }
        });
        Self {
            port,
            connections,
            stop,
            to_server,
            to_client,
            latency_ms,
        }
    }

    /// Holds every chunk for `delay` from now on (each way, so a round trip costs twice).
    pub fn slow(&self, delay: Duration) {
        self.latency_ms
            .store(delay.as_millis() as u64, Ordering::SeqCst);
    }

    pub fn cut(&self) {
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

/// Like [`sshd_ready`] for `mosh-server` (`OR2_REQUIRE_MOSH`), which the sshd sessions find on
/// the standard path.
pub fn mosh_ready() -> bool {
    let found = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .chain([PathBuf::from("/usr/bin")])
        .any(|directory| directory.join("mosh-server").is_file());
    ready("mosh-server", "OR2_REQUIRE_MOSH", found)
}

/// Whether process `pid`'s environment carries this sshd's private `TMUX_TMPDIR`, i.e. one of
/// the fixture's sessions started it (or something it started).
fn belongs_to(pid: u32, tmux_dir: &Path) -> bool {
    let marker = format!("TMUX_TMPDIR={}", tmux_dir.display());
    fs::read(format!("/proc/{pid}/environ")).is_ok_and(|environ| {
        environ
            .split(|byte| *byte == 0)
            .any(|entry| entry == marker.as_bytes())
    })
}

fn comm(pid: u32) -> Option<String> {
    fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|name| name.trim().to_owned())
}

fn all_pids() -> Vec<u32> {
    fs::read_dir("/proc")
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Every process named `name` that one of this sshd's sessions started. That is how a test
/// finds the `mosh-server` a host connection to the fixture started, without touching any
/// other process.
pub fn fixture_processes(sshd: &Sshd, name: &str) -> Vec<u32> {
    all_pids()
        .into_iter()
        .filter(|pid| comm(*pid).as_deref() == Some(name) && belongs_to(*pid, &sshd.tmux_dir))
        .collect()
}

/// Waits until `condition` holds, up to `limit`.
pub fn wait_until(limit: Duration, what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Kills, when dropped (also while panicking), the `mosh-server`s and the shells under them
/// that this fixture's sessions started: only processes carrying the fixture's private
/// `TMUX_TMPDIR`, named like what `mosh-server` runs, each re-checked right before the
/// signal. Create it first, so a test that fails halfway leaves nothing behind.
pub struct MoshReaper {
    tmux_dir: PathBuf,
}

impl MoshReaper {
    pub fn new(sshd: &Sshd) -> Self {
        Self {
            tmux_dir: sshd.tmux_dir.clone(),
        }
    }
}

impl Drop for MoshReaper {
    fn drop(&mut self) {
        for pid in all_pids() {
            let Some(name) = comm(pid) else { continue };
            if matches!(
                name.as_str(),
                "mosh-server" | "bash" | "zsh" | "sh" | "fish"
            ) && belongs_to(pid, &self.tmux_dir)
                && comm(pid).as_deref() == Some(name.as_str())
            {
                let _ = Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .status();
            }
        }
    }
}

/// A datagram transport over real UDP that records its sockets (bytes received on each), and
/// optionally drops everything the server sends back: a firewalled UDP port, where our
/// datagrams leave and nothing returns.
#[derive(Clone, Default)]
pub struct TestUdp {
    pub sockets: Arc<Mutex<Vec<Arc<AtomicUsize>>>>,
    pub blackhole: bool,
    /// While set, every datagram the client sends is silently dropped (a broken outbound
    /// path; what the server sends still arrives). Shared by every socket, flipped at any time.
    pub mute: Arc<AtomicBool>,
}

pub struct TestSocket {
    inner: tokio::net::UdpSocket,
    received: Arc<AtomicUsize>,
    blackhole: bool,
    mute: Arc<AtomicBool>,
}

impl DatagramSocket for TestSocket {
    fn local_addr(&self) -> std::io::Result<SocketAddr> {
        DatagramSocket::local_addr(&self.inner)
    }

    fn peer_addr(&self) -> std::io::Result<SocketAddr> {
        DatagramSocket::peer_addr(&self.inner)
    }

    fn try_send(&self, datagram: &[u8]) -> std::io::Result<usize> {
        if self.mute.load(Ordering::SeqCst) {
            return Ok(datagram.len());
        }
        DatagramSocket::try_send(&self.inner, datagram)
    }

    fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<std::io::Result<usize>> {
        if self.blackhole {
            // Read and discard so the kernel buffer does not fill; never report a datagram.
            while let Poll::Ready(Ok(_)) = DatagramSocket::poll_recv(&self.inner, cx, buf) {}
            return Poll::Pending;
        }
        let polled = DatagramSocket::poll_recv(&self.inner, cx, buf);
        if let Poll::Ready(Ok(length)) = &polled {
            self.received.fetch_add(*length, Ordering::SeqCst);
        }
        polled
    }
}

impl DatagramTransport for TestUdp {
    type Socket = TestSocket;

    async fn bind(&self, endpoint: &Endpoint) -> std::io::Result<TestSocket> {
        let inner = DirectUdp.bind(endpoint).await?;
        let received = Arc::new(AtomicUsize::new(0));
        self.sockets.lock().unwrap().push(received.clone());
        Ok(TestSocket {
            inner,
            received,
            blackhole: self.blackhole,
            mute: self.mute.clone(),
        })
    }
}
