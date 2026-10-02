//! Easy pair end to end against a disposable OpenSSH `sshd`: the built `or2-pair-testhost` adds
//! its temporary key to the file that sshd reads, a test-only phone (russh) derives the bootstrap
//! key from the code and the id in the QR, pins the host key, authenticates, runs the exchange
//! through sshd's forced command, and then logs in with the key it handed over.
//!
//! The sshd runs as the current user on a loopback port with its own host key, configuration
//! and `authorized_keys`; no real sshd, home, key, tmux or herdr is touched. Gated like the other
//! sshd tests: a missing `/usr/bin/sshd` (or `ssh-keygen`) skips them, printing `SKIP`, unless
//! `OR2_REQUIRE_SSHD` is set, which fails instead (set it in the full gate).
//!
//! The phone here is not the product's: it follows the contract's "The phone's connection" and
//! "The exchange" with russh, so the host side is tested without the phone lane's code.
#![cfg(all(unix, feature = "test-support"))]

use std::borrow::Cow;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use or2_pair::bootstrap::{self, PairingId};
use or2_pair::code::PairCode;
use or2_pair::payload::reference;
use russh::ChannelMsg;
use russh::client::{self, Handler};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::keys::ssh_key::{Algorithm, PrivateKey, PublicKey};
use russh::keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate};

const TESTHOST: &str = env!("CARGO_BIN_EXE_or2-pair-testhost");
const BIND_ATTEMPTS: usize = 5;

// --- gating -----------------------------------------------------------------------------------

fn available(program: &str) -> bool {
    Command::new(program)
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// Whether the sshd tests can run: `/usr/bin/sshd` and `ssh-keygen`, and a path for the test
/// host that a forced command can carry.
fn sshd_ready() -> bool {
    let present = Path::new("/usr/bin/sshd").exists() && available("ssh-keygen");
    if !present {
        assert!(
            std::env::var_os("OR2_REQUIRE_SSHD").is_none(),
            "OR2_REQUIRE_SSHD is set but sshd or ssh-keygen is absent"
        );
        eprintln!("SKIP: sshd or ssh-keygen is absent");
        return false;
    }
    let path_ok = fs::canonicalize(TESTHOST)
        .ok()
        .is_some_and(|path| bootstrap::check_exe_path(&path).is_ok());
    if !path_ok {
        assert!(
            std::env::var_os("OR2_REQUIRE_SSHD").is_none(),
            "the test host's path cannot be a forced command: build in a plainer directory"
        );
        eprintln!("SKIP: the test host's path has characters a forced command cannot carry");
        return false;
    }
    true
}

// --- the disposable sshd ----------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// The key file is `<home>/.ssh/authorized_keys` (the real layout).
    Home,
    /// The key file is some other file, which the test host is told about with
    /// `OR2_PAIR_TEST_AUTHORIZED_KEYS` (what the Kotlin fixture does).
    Explicit,
}

struct Sshd {
    root: tempfile::TempDir,
    child: Child,
    port: u16,
    layout: Layout,
    /// The host's public key line (`ssh-ed25519 AAAA…`).
    host_key: String,
}

fn free_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn wait_until_listening(child: &mut Child, log: &Path) -> Result<(), String> {
    let limit = Instant::now() + Duration::from_secs(20);
    loop {
        let text = fs::read_to_string(log).unwrap_or_default();
        if text.contains("Server listening") {
            return Ok(());
        }
        if child.try_wait().unwrap().is_some() || Instant::now() > limit {
            return Err(text);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

impl Sshd {
    /// `extra` is appended to `sshd_config`; `noise` makes every shell that sshd starts for a
    /// command print a line first (the rc-file case).
    fn start(layout: Layout, extra: &str, noise: bool) -> Self {
        Self::start_in(layout, extra, noise, None)
    }

    /// The same, with sshd's `TZ` set to `tz` (it reads `expiry-time` in that zone).
    fn start_in(layout: Layout, extra: &str, noise: bool, tz: Option<&str>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let home = dir.join("home");
        let etc = dir.join("etc");
        fs::create_dir(&home).unwrap();
        fs::set_permissions(&home, fs::Permissions::from_mode(0o755)).unwrap();
        fs::create_dir(&etc).unwrap();
        let host = dir.join("host");
        assert!(
            Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-C", "", "-f"])
                .arg(&host)
                .status()
                .unwrap()
                .success()
        );
        let host_key = fs::read_to_string(dir.join("host.pub")).unwrap();
        let host_key = host_key
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        fs::write(
            etc.join("ssh_host_ed25519_key.pub"),
            format!("{host_key}\n"),
        )
        .unwrap();

        let keys = match layout {
            Layout::Home => home.join(".ssh/authorized_keys"),
            Layout::Explicit => dir.join("authorized"),
        };
        if noise {
            // Whatever the account's login shell is, it prints before it runs the command.
            fs::write(dir.join("bash_env"), "echo 'noise from BASH_ENV'\n").unwrap();
            fs::write(dir.join(".zshenv"), "echo 'noise from .zshenv'\n").unwrap();
            fs::write(home.join(".bashrc"), "echo 'noise from .bashrc'\n").unwrap();
        } else {
            fs::write(dir.join("bash_env"), "").unwrap();
        }
        let user = current_user();
        let environment = format!(
            "HOME={home} ZDOTDIR={dir} BASH_ENV={bash_env} ENV=/dev/null HISTFILE=/dev/null \
             OR2_PAIR_TEST_HOME={home} OR2_PAIR_TEST_USER={user} OR2_PAIR_TEST_ETC_SSH={etc}{explicit}",
            home = home.display(),
            dir = dir.display(),
            bash_env = dir.join("bash_env").display(),
            etc = etc.display(),
            explicit = match layout {
                Layout::Home => String::new(),
                Layout::Explicit => format!(" OR2_PAIR_TEST_AUTHORIZED_KEYS={}", keys.display()),
            },
        );
        let config = format!(
            "ListenAddress {}\nHostKey {}\nAuthorizedKeysFile {}\nPidFile {}\nStrictModes no\nUsePAM no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\nPrintMotd no\nPrintLastLog no\nSetEnv {environment}\nLogLevel VERBOSE\n{extra}\n",
            Ipv4Addr::LOCALHOST,
            host.display(),
            keys.display(),
            dir.join("pid").display(),
        );
        // A free port is found by binding and releasing it, so another process can take it
        // before sshd binds; sshd then exits and is started again on a new port.
        let mut last = String::new();
        for _ in 0..BIND_ATTEMPTS {
            let port = free_port();
            fs::write(dir.join("config"), format!("Port {port}\n{config}")).unwrap();
            let log = dir.join("log");
            let mut command = Command::new("/usr/bin/sshd");
            command
                .args(["-D", "-e", "-f"])
                .arg(dir.join("config"))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(fs::File::create(&log).unwrap());
            if let Some(tz) = tz {
                command.env("TZ", tz);
            }
            let mut child = command.spawn().unwrap();
            match wait_until_listening(&mut child, &log) {
                Ok(()) => {
                    return Self {
                        root,
                        child,
                        port,
                        layout,
                        host_key,
                    };
                }
                Err(text) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    assert!(
                        text.contains("Address already in use") || text.contains("Cannot bind"),
                        "the disposable sshd failed to start: {text}"
                    );
                    last = text;
                }
            }
        }
        panic!("the disposable sshd could not bind a port in {BIND_ATTEMPTS} attempts: {last}");
    }

    fn dir(&self) -> &Path {
        self.root.path()
    }

    fn home(&self) -> PathBuf {
        self.dir().join("home")
    }

    fn etc(&self) -> PathBuf {
        self.dir().join("etc")
    }

    /// The file sshd reads keys from.
    fn keys(&self) -> PathBuf {
        match self.layout {
            Layout::Home => self.home().join(".ssh/authorized_keys"),
            Layout::Explicit => self.dir().join("authorized"),
        }
    }

    fn keys_text(&self) -> String {
        fs::read_to_string(self.keys()).unwrap_or_default()
    }

    fn state_files(&self) -> usize {
        let state = match self.layout {
            Layout::Home => self.home().join(".ssh/or2-pair"),
            Layout::Explicit => self.dir().join("or2-pair"),
        };
        // Not the lock file, which stays.
        fs::read_dir(state)
            .map(|d| {
                d.filter(|e| e.as_ref().unwrap().file_name() != "lock")
                    .count()
            })
            .unwrap_or(0)
    }

    fn log(&self) -> String {
        fs::read_to_string(self.dir().join("log")).unwrap_or_default()
    }

    fn address(&self) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, self.port))
    }

    /// The environment the test host (and the `enroll` that sshd starts) runs with.
    fn host_command(&self, extra: &[&str]) -> Command {
        let mut command = Command::new(TESTHOST);
        command
            .args(["--ssh-port", &self.port.to_string()])
            .args(["--address", "127.0.0.1", "--name", "Test Host"])
            .args(extra)
            .env("HOME", self.home())
            .env("OR2_PAIR_TEST_HOME", self.home())
            .env("OR2_PAIR_TEST_USER", current_user())
            .env("OR2_PAIR_TEST_ETC_SSH", self.etc())
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if self.layout == Layout::Explicit {
            command.env("OR2_PAIR_TEST_AUTHORIZED_KEYS", self.keys());
        }
        command
    }
}

impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn current_user() -> String {
    or2_pair::account::Account::current().unwrap().name
}

/// The account's login shell, which sshd runs commands through.
fn login_shell() -> String {
    or2_pair::account::Account::current()
        .unwrap()
        .shell
        .unwrap_or_else(|| "/bin/sh".into())
}

// --- the test host ----------------------------------------------------------------------------

/// A running `or2-pair-testhost`: its output line by line, and the standard input the code went
/// in on (kept open, as a terminal would).
struct Host {
    child: Child,
    lines: mpsc::Receiver<String>,
    _stdin: ChildStdin,
    seen: String,
}

impl Host {
    fn start(sshd: &Sshd, code: &PairCode, extra: &[&str]) -> Self {
        Self::start_command(sshd.host_command(extra), code)
    }

    /// Starts this command (a [`Sshd::host_command`]), types `code` and waits until it waits.
    fn start_command(mut command: Command, code: &PairCode) -> Self {
        let mut child = command.spawn().expect("run or2-pair-testhost");
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, "{}", code.display()).unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut host = Self {
            child,
            lines,
            _stdin: stdin,
            seen: String::new(),
        };
        host.until("Waiting for the phone");
        host
    }

    fn until(&mut self, needle: &str) {
        let limit = Instant::now() + Duration::from_secs(30);
        loop {
            let left = limit.saturating_duration_since(Instant::now());
            let line = self
                .lines
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no {needle:?} in:\n{}", self.seen));
            self.seen.push_str(&line);
            self.seen.push('\n');
            if line.contains(needle) {
                return;
            }
        }
    }

    /// The pairing code the host printed, as the phone reads it.
    fn offer(&self) -> reference_offer::Offer {
        let line = self
            .seen
            .lines()
            .find(|l| l.starts_with("or2-pair:2?"))
            .unwrap_or_else(|| panic!("no code in:\n{}", self.seen));
        reference_offer::Offer::parse(line)
    }

    fn finish(mut self) -> (Option<i32>, String) {
        let limit = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < limit,
                "the test host did not end:\n{}",
                self.seen
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        while let Ok(line) = self.lines.recv_timeout(Duration::from_secs(1)) {
            self.seen.push_str(&line);
            self.seen.push('\n');
        }
        (status.code(), std::mem::take(&mut self.seen))
    }

    fn interrupt(&self) {
        // SAFETY: signalling a child process this test started.
        assert_eq!(
            unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGINT) },
            0
        );
    }

    /// Stops the waiting run (SIGSTOP) and returns once it is stopped: it neither looks for the
    /// phone's result nor cleans up until [`Host::go_on`]. Its lock on its state file stays, so
    /// `enroll` still serves the run.
    fn pause(&self) {
        let pid = self.child.id() as libc::pid_t;
        let mut status = 0;
        // SAFETY: signalling and waiting for a child process this test started.
        unsafe {
            assert_eq!(libc::kill(pid, libc::SIGSTOP), 0);
            assert_eq!(libc::waitpid(pid, &mut status, libc::WUNTRACED), pid);
        }
        assert!(libc::WIFSTOPPED(status), "{status:#x}");
    }

    fn go_on(&self) {
        // SAFETY: signalling a child process this test started.
        assert_eq!(
            unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGCONT) },
            0
        );
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

mod reference_offer {
    use super::*;

    /// What the phone takes from the scan.
    pub struct Offer {
        pub user: String,
        pub port: u16,
        pub address: String,
        pub host_key: String,
        pub id: PairingId,
    }

    impl Offer {
        pub fn parse(code: &str) -> Self {
            let payload = reference::parse(code).unwrap();
            Self {
                user: payload.user,
                port: payload.port,
                address: payload.addresses[0].clone(),
                host_key: payload.host_key,
                id: payload.id.expect("an Easy pair code has an id"),
            }
        }
    }
}

use reference_offer::Offer;

// --- the test-only phone ----------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum PhoneError {
    HostKeyMismatch,
    BootstrapRefused,
    NotOr2Pair,
    Protocol(String),
    /// The host's verdict, `ok:false` with this reason.
    Refused(String),
}

struct Pinned {
    expected: PublicKey,
    mismatch: Arc<AtomicBool>,
}

impl Handler for Pinned {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let matches = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => {
                key.key_data() == self.expected.key_data()
            }
            PublicKeyOrCertificate::Certificate(_) => false,
        };
        if !matches {
            self.mismatch.store(true, Ordering::SeqCst);
        }
        Ok(matches)
    }
}

/// Accepts whatever the host presents (for the login with the paired key, whose pinning is the
/// product's concern, not this suite's).
struct Anything;

impl Handler for Anything {
    type Error = russh::Error;

    async fn check_server_key(&mut self, _: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

fn config(host_algorithm: Algorithm) -> Arc<client::Config> {
    Arc::new(client::Config {
        preferred: russh::Preferred {
            key: Cow::Owned(vec![host_algorithm]),
            ..russh::Preferred::default()
        },
        ..client::Config::default()
    })
}

/// The bootstrap key as a russh private key, from the same derivation the host uses (which the
/// unit tests pin to an independent OpenSSL vector).
fn bootstrap_key(code: &PairCode, id: &PairingId) -> PrivateKeyWithHashAlg {
    let seed = bootstrap::seed(code, id);
    let pair = Ed25519Keypair::from_seed(&seed);
    let key = PrivateKey::new(KeypairData::Ed25519(pair), "").unwrap();
    PrivateKeyWithHashAlg::new(Arc::new(key), None)
}

/// One pairing connection, step by step so a test can interleave two phones.
struct Phone {
    handle: client::Handle<Pinned>,
    channel: russh::Channel<client::Msg>,
    buffer: Vec<u8>,
    /// Bytes of shell noise skipped before the hello.
    skipped: usize,
}

impl Phone {
    /// Connects, checks the pinned host key, authenticates with the bootstrap key and starts
    /// `exec "or2-pair"`.
    async fn open(addr: SocketAddr, offer: &Offer, code: &PairCode) -> Result<Self, PhoneError> {
        let expected = PublicKey::from_openssh(&offer.host_key).unwrap();
        let mismatch = Arc::new(AtomicBool::new(false));
        let handler = Pinned {
            expected: expected.clone(),
            mismatch: Arc::clone(&mismatch),
        };
        let connect = client::connect(config(expected.algorithm()), addr, handler);
        let mut handle = match tokio::time::timeout(Duration::from_secs(10), connect).await {
            Ok(Ok(handle)) => handle,
            _ if mismatch.load(Ordering::SeqCst) => return Err(PhoneError::HostKeyMismatch),
            Ok(Err(error)) => return Err(PhoneError::Protocol(format!("connect: {error}"))),
            Err(_) => return Err(PhoneError::Protocol("connect timed out".into())),
        };
        let auth = handle
            .authenticate_publickey(offer.user.clone(), bootstrap_key(code, &offer.id))
            .await
            .map_err(|error| PhoneError::Protocol(format!("authenticate: {error}")))?;
        if !auth.success() {
            return Err(PhoneError::BootstrapRefused);
        }
        let channel = handle
            .channel_open_session()
            .await
            .map_err(|error| PhoneError::Protocol(format!("channel: {error}")))?;
        channel
            .exec(true, "or2-pair")
            .await
            .map_err(|error| PhoneError::Protocol(format!("exec: {error}")))?;
        Ok(Self {
            handle,
            channel,
            buffer: Vec::new(),
            skipped: 0,
        })
    }

    /// The next whole line from the channel, or `None` when it ends.
    async fn line(&mut self) -> Option<String> {
        loop {
            if let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.buffer.drain(..=end).collect();
                return Some(String::from_utf8_lossy(&line).trim_end().to_owned());
            }
            match tokio::time::timeout(Duration::from_secs(10), self.channel.wait()).await {
                Ok(Some(ChannelMsg::Data { data })) => self.buffer.extend_from_slice(&data),
                Ok(Some(ChannelMsg::ExtendedData { .. } | ChannelMsg::ExitStatus { .. })) => {}
                Ok(Some(ChannelMsg::Success | ChannelMsg::WindowAdjusted { .. })) => {}
                Ok(Some(_)) | Ok(None) | Err(_) => return None,
            }
        }
    }

    /// Skips shell noise (at most 4 KB) up to the hello and checks it is for this pairing.
    async fn hello(&mut self, offer: &Offer) -> Result<(), PhoneError> {
        loop {
            let Some(line) = self.line().await else {
                return Err(PhoneError::NotOr2Pair);
            };
            if line.starts_with("{\"v\":2,\"hello\"") {
                let hello: serde_json::Value = serde_json::from_str(&line)
                    .map_err(|_| PhoneError::Protocol(format!("hello: {line}")))?;
                return if hello["id"] == offer.id.as_str() && hello["hello"] == "or2-pair" {
                    Ok(())
                } else {
                    Err(PhoneError::Protocol(format!(
                        "hello for another id: {line}"
                    )))
                };
            }
            self.skipped += line.len() + 1;
            if self.skipped > 4096 {
                return Err(PhoneError::NotOr2Pair);
            }
        }
    }

    /// Sends the request and returns the verdict's `(user, fingerprint)`.
    async fn exchange(
        &mut self,
        public_key: &str,
        device: &str,
    ) -> Result<(String, String), PhoneError> {
        let request = format!("{{\"v\":2,\"key\":\"{public_key}\",\"device\":\"{device}\"}}\n");
        self.channel
            .data(request.as_bytes())
            .await
            .map_err(|error| PhoneError::Protocol(format!("request: {error}")))?;
        let Some(line) = self.line().await else {
            return Err(PhoneError::Protocol("no verdict".into()));
        };
        let verdict: serde_json::Value = serde_json::from_str(&line)
            .map_err(|_| PhoneError::Protocol(format!("verdict: {line}")))?;
        if verdict["v"] != 2 {
            return Err(PhoneError::Protocol(format!("verdict version: {line}")));
        }
        if verdict["ok"] == true {
            Ok((
                verdict["user"].as_str().unwrap_or_default().to_owned(),
                verdict["fingerprint"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            ))
        } else {
            Err(PhoneError::Refused(
                verdict["reason"].as_str().unwrap_or("?").to_owned(),
            ))
        }
    }

    async fn close(self) {
        let _ = self.channel.close().await;
        let _ = self
            .handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await;
    }
}

/// One whole pairing, as the phone does it.
async fn pair(
    addr: SocketAddr,
    offer: &Offer,
    code: &PairCode,
    public_key: &str,
    device: &str,
) -> Result<(String, String), PhoneError> {
    let mut phone = Phone::open(addr, offer, code).await?;
    phone.hello(offer).await?;
    let result = phone.exchange(public_key, device).await;
    phone.close().await;
    result
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn new_code() -> PairCode {
    PairCode::generate(&|buf: &mut [u8]| {
        use rand::Rng;
        rand::rng().fill_bytes(buf);
    })
}

/// A fresh phone key: the private half and `<algorithm> <base64>`.
fn phone_key() -> (PrivateKey, String) {
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    let line = key.public_key().to_openssh().unwrap();
    let line = line
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    (key, line)
}

/// Logs in with `key` as the paired phone would and runs `echo`; returns the output.
async fn login(addr: SocketAddr, user: &str, key: PrivateKey) -> Result<String, String> {
    let mut handle = client::connect(config(Algorithm::Ed25519), addr, Anything)
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let auth = handle
        .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), None))
        .await
        .map_err(|e| format!("authenticate: {e}"))?;
    if !auth.success() {
        return Err("the paired key was refused".into());
    }
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|e| e.to_string())?;
    channel
        .exec(true, "echo paired-ok")
        .await
        .map_err(|e| e.to_string())?;
    let mut output = Vec::new();
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => output.extend_from_slice(&data),
            ChannelMsg::Close => break,
            _ => {}
        }
    }
    Ok(String::from_utf8_lossy(&output).into_owned())
}

// --- the tests --------------------------------------------------------------------------------

fn paired_end_to_end(layout: Layout) {
    if !sshd_ready() {
        return;
    }
    let sshd = Sshd::start(layout, "", false);
    let code = new_code();
    let mut host = Host::start(&sshd, &code, &[]);
    let offer = host.offer();
    assert_eq!(offer.port, sshd.port);
    assert_eq!(offer.address, "127.0.0.1");
    assert_eq!(offer.host_key, sshd.host_key);
    assert_eq!(offer.user, current_user());

    // While the host waits, the temporary line is in the file sshd reads, and one state file.
    let before = sshd.keys_text();
    assert!(
        before.contains("or2-pair-bootstrap-") && before.contains("expiry-time=\""),
        "{before}"
    );
    assert_eq!(sshd.state_files(), 1);

    let (phone_private, phone_public) = phone_key();
    let rt = runtime();
    let (user, fingerprint) = rt
        .block_on(pair(
            sshd.address(),
            &offer,
            &code,
            &phone_public,
            "Pixel 8",
        ))
        .expect("the pairing succeeds");
    assert_eq!(user, current_user());
    assert_eq!(
        fingerprint,
        phone_private
            .public_key()
            .fingerprint(russh::keys::ssh_key::HashAlg::Sha256)
            .to_string()
    );

    // The host sees it, prints it and exits.
    host.until("\"Pixel-8\" can now log in as");
    let (code_exit, seen) = host.finish();
    assert_eq!(code_exit, Some(0), "{seen}\n{}", sshd.log());

    // The temporary key is gone, the phone's key is in, nothing is left behind.
    let after = sshd.keys_text();
    assert!(!after.contains("or2-pair-bootstrap-"), "{after}");
    assert!(
        after.contains(&phone_public) && after.contains("or2-Pixel-8-"),
        "{after}"
    );
    assert_eq!(sshd.state_files(), 0);

    // The paired key logs in; the bootstrap key no longer does.
    let output = rt
        .block_on(login(sshd.address(), &offer.user, phone_private))
        .expect("login with the paired key");
    assert!(output.contains("paired-ok"), "{output}");
    assert_eq!(
        rt.block_on(pair(sshd.address(), &offer, &code, &phone_public, "again")),
        Err(PhoneError::BootstrapRefused)
    );
}

#[test]
fn a_phone_pairs_through_sshd_and_logs_in_with_its_key() {
    paired_end_to_end(Layout::Explicit);
}

#[test]
fn the_same_in_the_real_home_layout() {
    paired_end_to_end(Layout::Home);
}

#[test]
fn a_different_code_is_refused_and_costs_nothing() {
    if !sshd_ready() {
        return;
    }
    let sshd = Sshd::start(Layout::Explicit, "", false);
    let code = new_code();
    let host = Host::start(&sshd, &code, &[]);
    let offer = host.offer();
    let (_, phone_public) = phone_key();
    let rt = runtime();
    let wrong = new_code();
    assert_eq!(
        rt.block_on(pair(sshd.address(), &offer, &wrong, &phone_public, "p")),
        Err(PhoneError::BootstrapRefused)
    );
    // The temporary key is still there and the right code still pairs.
    assert!(sshd.keys_text().contains("or2-pair-bootstrap-"));
    assert!(
        rt.block_on(pair(sshd.address(), &offer, &code, &phone_public, "p"))
            .is_ok()
    );
    let (exit, seen) = host.finish();
    assert_eq!(exit, Some(0), "{seen}");
}

#[test]
fn after_the_run_ended_the_code_is_refused() {
    if !sshd_ready() {
        return;
    }
    let sshd = Sshd::start(Layout::Explicit, "", false);
    let code = new_code();
    let host = Host::start(&sshd, &code, &[]);
    let offer = host.offer();
    host.interrupt();
    let (exit, seen) = host.finish();
    assert_eq!(exit, Some(1), "{seen}");
    assert!(seen.contains("The temporary key was removed"), "{seen}");
    assert!(!sshd.keys_text().contains("or2-pair-bootstrap-"));
    let (_, phone_public) = phone_key();
    assert_eq!(
        runtime().block_on(pair(sshd.address(), &offer, &code, &phone_public, "late")),
        Err(PhoneError::BootstrapRefused)
    );
}

#[test]
fn the_timeout_ends_the_pairing_the_same_way() {
    if !sshd_ready() {
        return;
    }
    let sshd = Sshd::start(Layout::Explicit, "", false);
    let code = new_code();
    let mut command = sshd.host_command(&[]);
    command.env("OR2_PAIR_TEST_WINDOW_SECS", "2");
    let mut child = command.spawn().unwrap();
    writeln!(child.stdin.take().unwrap(), "{}", code.display()).unwrap();
    let out = child.wait_with_output().unwrap();
    let shown = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(out.status.code(), Some(1), "{shown}");
    assert!(shown.contains("Timed out"), "{shown}");
    let offer = Offer::parse(
        shown
            .lines()
            .find(|l| l.starts_with("or2-pair:2?"))
            .unwrap(),
    );
    let (_, phone_public) = phone_key();
    assert_eq!(
        runtime().block_on(pair(sshd.address(), &offer, &code, &phone_public, "late")),
        Err(PhoneError::BootstrapRefused)
    );
    assert!(!sshd.keys_text().contains("or2-pair-bootstrap-"));
}

#[test]
fn a_different_host_key_ends_the_connection_before_anything_is_sent() {
    if !sshd_ready() {
        return;
    }
    let sshd = Sshd::start(Layout::Explicit, "", false);
    let code = new_code();
    let host = Host::start(&sshd, &code, &[]);
    let mut offer = host.offer();
    let (_, other_host_key) = phone_key();
    offer.host_key = other_host_key;
    let (_, phone_public) = phone_key();
    let rt = runtime();
    assert_eq!(
        rt.block_on(pair(sshd.address(), &offer, &code, &phone_public, "p")),
        Err(PhoneError::HostKeyMismatch)
    );
    // Nothing was authenticated, so nothing was spent: the real offer still pairs.
    assert!(sshd.keys_text().contains("or2-pair-bootstrap-"));
    assert!(!sshd.log().contains("Accepted publickey"), "{}", sshd.log());
    let real = host.offer();
    assert!(
        rt.block_on(pair(sshd.address(), &real, &code, &phone_public, "p"))
            .is_ok()
    );
    let (exit, seen) = host.finish();
    assert_eq!(exit, Some(0), "{seen}");
}

#[test]
fn a_second_phone_that_got_in_before_the_first_finished_is_told_gone() {
    if !sshd_ready() {
        return;
    }
    let sshd = Sshd::start(Layout::Explicit, "", false);
    let code = new_code();
    let host = Host::start(&sshd, &code, &[]);
    let offer = host.offer();
    let (second_private, second_public) = phone_key();
    let (first_private, first_public) = phone_key();
    let rt = runtime();
    // The waiting run must not end (and remove its state) between the first phone's pairing and
    // the second phone's request: the second would then be told `expired`, not `gone`, depending
    // on when the run next looked. It is stopped meanwhile; `enroll` does not need it to run.
    host.pause();
    rt.block_on(async {
        // The second phone is in (it authenticated and has its hello) but has not asked yet...
        let mut second = Phone::open(sshd.address(), &offer, &code).await.unwrap();
        second.hello(&offer).await.unwrap();
        // ...when the first one pairs completely...
        pair(sshd.address(), &offer, &code, &first_public, "first")
            .await
            .unwrap();
        // ...so the second one finds the temporary key gone.
        assert_eq!(
            second.exchange(&second_public, "second").await,
            Err(PhoneError::Refused("gone".into()))
        );
        second.close().await;
    });
    host.go_on();
    let keys = sshd.keys_text();
    assert!(
        keys.contains(&first_public) && !keys.contains(&second_public),
        "{keys}"
    );
    let (exit, seen) = host.finish();
    assert_eq!(exit, Some(0), "{seen}");
    // The first phone's key works; the second never got one.
    assert!(
        rt.block_on(login(sshd.address(), &offer.user, first_private))
            .is_ok()
    );
    assert!(
        rt.block_on(login(sshd.address(), &offer.user, second_private))
            .is_err()
    );
}

#[test]
fn rc_file_noise_before_the_hello_is_skipped() {
    if !sshd_ready() {
        return;
    }
    let shell = login_shell();
    let name = shell.rsplit('/').next().unwrap_or_default().to_owned();
    if name != "bash" && name != "zsh" {
        // The noise files cover bash (BASH_ENV) and zsh (.zshenv in ZDOTDIR) only.
        eprintln!("SKIP: the login shell {shell} is not bash or zsh");
        return;
    }
    let sshd = Sshd::start(Layout::Explicit, "", true);
    let code = new_code();
    let host = Host::start(&sshd, &code, &[]);
    let offer = host.offer();
    let (phone_private, phone_public) = phone_key();
    let rt = runtime();
    let (_, _) = rt.block_on(async {
        // Show that the shell really does print first: the hello is not the first line.
        let mut phone = Phone::open(sshd.address(), &offer, &code).await.unwrap();
        let hello = phone.hello(&offer).await;
        assert!(
            phone.skipped > 0,
            "the {name} noise was expected before the hello"
        );
        hello.unwrap();
        phone.exchange(&phone_public, "noisy").await.unwrap()
    });
    let (exit, seen) = host.finish();
    assert_eq!(exit, Some(0), "{seen}");
    let output = rt
        .block_on(login(sshd.address(), &offer.user, phone_private))
        .unwrap();
    assert!(output.contains("paired-ok"), "{output}");
}

#[test]
fn a_forcecommand_in_sshd_config_is_not_or2_pair() {
    if !sshd_ready() {
        return;
    }
    let sshd = Sshd::start(Layout::Explicit, "ForceCommand echo not-or2-pair\n", false);
    // When the checks can read that configuration they refuse before the prompt, with nothing
    // written (review of the v2 integration: this was a warning).
    fs::write(
        sshd.etc().join("sshd_config"),
        "ForceCommand echo not-or2-pair\n",
    )
    .unwrap();
    let code = new_code();
    let mut child = sshd.host_command(&[]).spawn().unwrap();
    writeln!(child.stdin.take().unwrap(), "{}", code.display()).unwrap();
    let out = child.wait_with_output().unwrap();
    let shown = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(out.status.code(), Some(1), "{shown}");
    assert!(
        shown.lines().any(|line| line.ends_with(
            "  sshd_config sets a ForceCommand, which would run instead of the pairing command"
        ) && (line.starts_with("■  ") || line.starts_with("x  ")))
            && shown.contains("--manual"),
        "{shown}"
    );
    assert!(!shown.contains("Code shown on your phone"), "{shown}");
    assert!(sshd.keys_text().is_empty() && sshd.state_files() == 0);

    // Where they cannot read it (a configuration readable only by root), the phone finds out.
    fs::remove_file(sshd.etc().join("sshd_config")).unwrap();
    let host = Host::start(&sshd, &code, &[]);
    let offer = host.offer();
    let (_, phone_public) = phone_key();
    assert_eq!(
        runtime().block_on(pair(sshd.address(), &offer, &code, &phone_public, "p")),
        Err(PhoneError::NotOr2Pair)
    );
    // Nothing was installed; the host ends cleanly when asked to.
    assert!(sshd.keys_text().contains("or2-pair-bootstrap-"));
    host.interrupt();
    let (exit, _) = host.finish();
    assert_eq!(exit, Some(1));
    assert!(!sshd.keys_text().contains("or2-pair-bootstrap-"));
}

#[test]
fn the_checks_read_enumerated_values_ignoring_case_as_sshd_does() {
    // Fix check of the v2 fixes: the checks compared `no` and `none` case-sensitively. sshd
    // does not: `sshd -T` (the effective configuration) of the same file says `no` and `none`,
    // and it accepts `AuthorizedKeysCommand NONE` without an `AuthorizedKeysCommandUser`
    // (which it requires for a real command), so it reads that as `none` too.
    if !sshd_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let etc = dir.path().join("etc");
    fs::create_dir(&etc).unwrap();
    let host = dir.path().join("host");
    assert!(
        Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", "", "-f"])
            .arg(&host)
            .status()
            .unwrap()
            .success()
    );
    let config = etc.join("sshd_config");
    fs::write(
        &config,
        format!(
            "Port {}\nListenAddress 127.0.0.1\nHostKey {}\nPidFile {}\nUsePAM no\nPubkeyAuthentication No\nForceCommand None\nAuthorizedKeysCommand NONE\n",
            free_port(),
            host.display(),
            dir.path().join("pid").display(),
        ),
    )
    .unwrap();
    let out = Command::new("/usr/bin/sshd")
        .arg("-T")
        .arg("-f")
        .arg(&config)
        .output()
        .unwrap();
    let effective = String::from_utf8_lossy(&out.stdout).to_lowercase();
    assert!(
        out.status.success(),
        "{effective}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lines: Vec<&str> = effective.lines().collect();
    assert!(lines.contains(&"pubkeyauthentication no"), "{effective}");
    assert!(lines.contains(&"forcecommand none"), "{effective}");

    let read = or2_pair::checks::read_sshd_config(&etc).unwrap();
    assert_eq!(read.global.pubkey_authentication, Some(false), "No is no");
    assert_eq!(read.global.force_command, Some(false), "None is none");
    assert_eq!(
        read.global.authorized_keys_command,
        Some(false),
        "NONE is none"
    );
}

#[test]
fn a_host_whose_time_zone_differs_from_sshds_still_pairs() {
    // Review of the v2 integration: with sshd under TZ=UTC and the host under TZ=Etc/GMT+5, the
    // expiry written in the host's local time was read by sshd as UTC, five hours in the past,
    // and the phone was refused. An sshd from 9.1 on is given UTC with `Z`. (`<-05>5` is
    // Etc/GMT+5 written as a POSIX rule, which needs no time zone files.)
    if !sshd_ready() {
        return;
    }
    let sshd = Sshd::start_in(Layout::Explicit, "", false, Some("UTC"));
    let code = new_code();
    let mut command = sshd.host_command(&[]);
    command.env("TZ", "<-05>5");
    let host = Host::start_command(command, &code);
    let offer = host.offer();
    let keys = sshd.keys_text();
    assert!(keys.contains("Z\" ssh-ed25519 "), "{keys}");
    let (_, phone_public) = phone_key();
    let paired = runtime().block_on(pair(sshd.address(), &offer, &code, &phone_public, "p"));
    assert!(paired.is_ok(), "{paired:?}\n{}", sshd.log());
    let (exit, seen) = host.finish();
    assert_eq!(exit, Some(0), "{seen}");
}

#[test]
fn sshd_reads_the_utc_expiry_this_tool_writes() {
    // The `Z` form against the real sshd: an expiry that has passed in UTC is refused, one that
    // has not is accepted, whatever sshd's own time zone (here five hours ahead of UTC).
    if !sshd_ready() {
        return;
    }
    let sshd = Sshd::start_in(Layout::Explicit, "", false, Some("<+05>-5"));
    let code = new_code();
    let id = PairingId::parse("abcdefghijklm").unwrap();
    let key = bootstrap::public_key(&code, &id);
    let offer = Offer {
        user: current_user(),
        port: sshd.port,
        address: "127.0.0.1".into(),
        host_key: sshd.host_key.clone(),
        id: id.clone(),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let rt = runtime();
    // One hour ago in UTC (still in the future in sshd's zone, were it read as local time),
    // and 20 minutes ahead.
    for (deadline, accepted) in [(now - 3_600 - 600, false), (now + 1_200 - 600, true)] {
        let expiry = bootstrap::expiry(deadline);
        fs::write(
            sshd.keys(),
            format!(
                "restrict,command=\"/bin/echo hi\",expiry-time=\"{expiry}\" {} x\n",
                key.openssh()
            ),
        )
        .unwrap();
        let result = rt.block_on(Phone::open(sshd.address(), &offer, &code));
        match (accepted, result) {
            (true, Ok(phone)) => rt.block_on(phone.close()),
            (false, Err(PhoneError::BootstrapRefused)) => {}
            (_, other) => panic!("expiry {expiry}: {:?}", other.err()),
        }
    }
}

// --- the product's phone client against the real host -------------------------------------------

/// The same pairing with the phone the app ships: `or2_core::pair` parses the code the host
/// printed (so its strict parser reads the host's real output), takes the code as its own
/// `PairCode`, and `pair_enroll` does the whole connection. The test-only phone above follows the
/// contract independently; these tests say the two real halves agree with each other too.
mod real_phone {
    use super::*;
    use or2_core::keys::ClientKey;
    use or2_core::pair::{
        PairCode as PhoneCode, PairError, PairOffer as PhoneOffer, PairResult, PairTiming,
        pair_enroll,
    };
    use or2_core::transport::DirectTcp;
    use or2_core::trust::HostKey;

    /// What the phone reads from the QR the host printed.
    fn scan(host: &Host) -> PhoneOffer {
        let line = host
            .seen
            .lines()
            .find(|l| l.starts_with("or2-pair:2?"))
            .unwrap_or_else(|| panic!("no code in:\n{}", host.seen));
        PhoneOffer::parse(line).expect("the phone's parser reads the host's code")
    }

    /// The code the person typed, as the phone's own type holds it.
    fn phone_code(code: &PairCode) -> PhoneCode {
        PhoneCode::parse_typed(&code.display()).expect("both crates agree on the code's shape")
    }

    async fn enroll(
        offer: &PhoneOffer,
        code: &PhoneCode,
        key: &ClientKey,
    ) -> Result<PairResult, PairError> {
        pair_enroll(
            &Arc::new(DirectTcp),
            offer,
            code,
            &key.public_key().openssh,
            "Pixel 8",
            PairTiming::default(),
        )
        .await
    }

    #[test]
    fn the_phone_parses_what_the_host_prints_and_pairs() {
        if !sshd_ready() {
            return;
        }
        let sshd = Sshd::start(Layout::Explicit, "", false);
        let code = new_code();
        let mut host = Host::start(&sshd, &code, &[]);
        let offer = scan(&host);
        // The two readers agree on the offer.
        let reference = host.offer();
        assert_eq!(offer.username, reference.user);
        assert_eq!(offer.port, reference.port);
        assert_eq!(offer.addresses[0].host(), reference.address);
        assert_eq!(offer.pairing_id.as_deref(), Some(reference.id.as_str()));

        let phone = ClientKey::generate_ed25519("phone");
        let rt = runtime();
        let result = rt
            .block_on(enroll(&offer, &phone_code(&code), &phone))
            .expect("the real client pairs with the real host");
        assert_eq!(result.username, current_user());
        assert_eq!(result.fingerprint, phone.public_key().fingerprint);

        host.until("\"Pixel-8\" can now log in as");
        let (exit, seen) = host.finish();
        assert_eq!(exit, Some(0), "{seen}\n{}", sshd.log());
        let after = sshd.keys_text();
        assert!(!after.contains("or2-pair-bootstrap-"), "{after}");
        let key_data = phone.public_key().openssh;
        assert!(
            after.contains(key_data.split_whitespace().nth(1).unwrap()),
            "{after}"
        );
        assert_eq!(sshd.state_files(), 0);
        // The bootstrap key is spent: the same code now finds nothing.
        assert_eq!(
            rt.block_on(enroll(&offer, &phone_code(&code), &phone)),
            Err(PairError::BootstrapRefused)
        );
    }

    #[test]
    fn a_wrong_code_a_wrong_host_key_and_a_finished_run_are_told_apart() {
        if !sshd_ready() {
            return;
        }
        let sshd = Sshd::start(Layout::Explicit, "", false);
        let code = new_code();
        let host = Host::start(&sshd, &code, &[]);
        let offer = scan(&host);
        let phone = ClientKey::generate_ed25519("phone");
        let rt = runtime();

        // Another code: refused by sshd, nothing spent.
        assert_eq!(
            rt.block_on(enroll(&offer, &phone_code(&new_code()), &phone)),
            Err(PairError::BootstrapRefused)
        );
        // Another host key: the connection ends before anything is authenticated.
        let mut other = offer.clone();
        other.host_key = HostKey::from_openssh(&phone_key().1).expect("a public key line");
        assert_eq!(
            rt.block_on(enroll(&other, &phone_code(&code), &phone)),
            Err(PairError::HostKeyMismatch)
        );
        assert!(sshd.keys_text().contains("or2-pair-bootstrap-"));
        assert!(!sshd.log().contains("Accepted publickey"), "{}", sshd.log());

        // The right code still pairs.
        assert!(
            rt.block_on(enroll(&offer, &phone_code(&code), &phone))
                .is_ok()
        );
        let (exit, seen) = host.finish();
        assert_eq!(exit, Some(0), "{seen}");

        // A run that ended: its code is refused.
        let sshd = Sshd::start(Layout::Explicit, "", false);
        let host = Host::start(&sshd, &code, &[]);
        let offer = scan(&host);
        host.interrupt();
        let (exit, seen) = host.finish();
        assert_eq!(exit, Some(1), "{seen}");
        assert_eq!(
            rt.block_on(enroll(&offer, &phone_code(&code), &phone)),
            Err(PairError::BootstrapRefused)
        );
    }
}
