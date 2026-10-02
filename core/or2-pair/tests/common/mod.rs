//! A pretend host for the CLI's integration tests: a temporary home, a temporary `/etc/ssh`,
//! made-up interfaces and a scripted terminal. Nothing here touches the user's own `~/.ssh`,
//! sshd, tmux or herdr.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use or2_pair::account::Account;
use or2_pair::addresses::Iface;
use or2_pair::args::Options;
use or2_pair::checks::Platform;
use or2_pair::date::DateTime;
use or2_pair::hostkey::Keyscan;
use or2_pair::net::Net;
use or2_pair::payload::{self, Payload, reference};
use or2_pair::prompt::CodePrompt;
use or2_pair::run::{Env, Exit, Ready, RunError, Signals, run};
use zeroize::Zeroizing;

pub const HOST_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7";
pub const HOST_FINGERPRINT: &str = "SHA256:PK/nvGiFusFK9/6Qf8dSOX99mI5XQYiMAML2JHCjgQI";
/// A phone's public key (any valid Ed25519 key line will do).
pub const PHONE_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBCJz8goXA2qGjRTNHhOwsljhOuCXG/+2B/zTJH/brc5";
pub const OTHER_KEY: &str = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBJKzE3AhnQL0fsk77dWQaJJj+GHelMX7ge+TZ2xgJ/Y3O5aMMlnH6Hn/tnucj94OezBBvEj2VIcJGu8ABqUkhFA=";
pub const EXE: &str = "/usr/local/bin/or2-pair";

pub struct World {
    pub home: tempfile::TempDir,
    pub etc: tempfile::TempDir,
}

impl World {
    pub fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let world = Self {
            home: tempfile::tempdir().unwrap(),
            etc: tempfile::tempdir().unwrap(),
        };
        // sshd's StrictModes wants a home that others cannot write.
        std::fs::set_permissions(world.home.path(), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        std::fs::write(
            world.etc.path().join("ssh_host_ed25519_key.pub"),
            format!("{HOST_KEY} root@testhost\n"),
        )
        .unwrap();
        world
    }

    pub fn account(&self) -> Account {
        Account::new("alice", self.home.path())
    }

    pub fn ssh_dir(&self) -> PathBuf {
        self.home.path().join(".ssh")
    }

    pub fn authorized_keys(&self) -> Option<String> {
        std::fs::read_to_string(self.ssh_dir().join("authorized_keys")).ok()
    }

    /// Writes `~/.ssh/authorized_keys` (mode 600, `~/.ssh` mode 700).
    pub fn write_keys(&self, text: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(self.ssh_dir()).unwrap();
        std::fs::set_permissions(self.ssh_dir(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let file = self.ssh_dir().join("authorized_keys");
        std::fs::write(&file, text).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    /// The files of runs in `~/.ssh/or2-pair` (not its lock file), sorted.
    pub fn state_files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.ssh_dir().join("or2-pair"))
            .map(|dir| {
                dir.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .filter(|name| name != or2_pair::state::LOCK)
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// The backups of `authorized_keys`, sorted.
    pub fn backups(&self) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = std::fs::read_dir(self.ssh_dir())
            .map(|dir| {
                dir.map(|e| e.unwrap().path())
                    .filter(|p| {
                        p.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .contains("or2-backup")
                    })
                    .collect()
            })
            .unwrap_or_default();
        found.sort();
        found
    }
}

/// Types the lines the test gives, then reaches the end of input; counts what it was asked.
pub struct Script {
    lines: Mutex<VecDeque<String>>,
    pub asked: AtomicU32,
}

impl Script {
    pub fn new(lines: &[&str]) -> Self {
        Self {
            lines: Mutex::new(lines.iter().map(|l| (*l).to_owned()).collect()),
            asked: AtomicU32::new(0),
        }
    }
}

impl CodePrompt for Script {
    fn read_line(&self) -> Option<Zeroizing<String>> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.lines.lock().unwrap().pop_front().map(Zeroizing::new)
    }
}

/// A signal counter the test increments.
#[derive(Default)]
pub struct FakeSignals {
    count: AtomicU32,
    pub armed: AtomicU32,
}

impl FakeSignals {
    pub fn raise(&self) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }
}

impl Signals for FakeSignals {
    fn arm(&self) -> io::Result<()> {
        self.armed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn count(&self) -> u32 {
        self.count.load(Ordering::SeqCst)
    }
}

/// What the sshd probe answers.
pub struct FakeNet(pub Result<String, io::ErrorKind>);

impl FakeNet {
    pub fn openssh() -> Self {
        Self(Ok("SSH-2.0-OpenSSH_9.9".to_owned()))
    }
}

impl Net for FakeNet {
    fn probe_ssh(&self, _: u16) -> io::Result<String> {
        match &self.0 {
            Ok(banner) => Ok(banner.clone()),
            Err(kind) => Err((*kind).into()),
        }
    }
}

/// A login shell that starts the program (the run's version is "test").
pub struct FakeShell;

impl or2_pair::checks::ShellProbe for FakeShell {
    fn run(&self, _: &str, _: &str) -> Result<String, String> {
        Ok("or2-pair test\n".to_owned())
    }
}

pub struct NoKeyscan;

impl Keyscan for NoKeyscan {
    fn scan(&self, _: u16, _: &str) -> io::Result<String> {
        Ok(String::new())
    }
}

pub fn interfaces() -> Vec<Iface> {
    [
        ("lo", "127.0.0.1"),
        ("eth0", "192.168.1.20"),
        ("eth0", "203.0.113.9"),
        ("eth0", "2001:db8::9"),
        ("eth0", "fe80::1"),
        ("docker0", "172.17.0.1"),
        ("zt0", "10.147.17.5"),
    ]
    .iter()
    .map(|(name, ip)| Iface {
        name: (*name).into(),
        ip: ip.parse().unwrap(),
    })
    .collect()
}

pub fn options() -> Options {
    Options {
        user: Some("alice".into()),
        name: Some("Test Host".into()),
        ssh_port: Some(22),
        ..Options::default()
    }
}

/// Everything about the pretend host a test may change.
pub struct Setup {
    pub banner: Result<String, io::ErrorKind>,
    pub exe: Result<PathBuf, String>,
    pub window: Duration,
    pub poll: Duration,
    pub can_ask: bool,
    pub install_keys: bool,
    pub platform: Platform,
    pub shell: Option<String>,
    /// Whether `TZ` is set for the run.
    pub tz_set: bool,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            banner: Ok("SSH-2.0-OpenSSH_9.9".into()),
            exe: Ok(PathBuf::from(EXE)),
            window: Duration::from_secs(20),
            poll: Duration::from_millis(10),
            can_ask: true,
            install_keys: true,
            platform: Platform::Linux,
            shell: Some("/bin/bash".into()),
            tz_set: false,
        }
    }
}

pub struct Pairing<R> {
    pub exit: Result<Exit, RunError>,
    pub output: String,
    /// What the phone closure returned (`None` when the host never got ready).
    pub phone: Option<R>,
}

/// Runs the whole CLI flow in a thread against `world` and calls `phone` (on the test's thread)
/// once the host is waiting for the phone.
pub fn pair<R>(
    world: &World,
    options: &Options,
    setup: &Setup,
    script: &Script,
    signals: &FakeSignals,
    phone: impl FnOnce(Ready) -> R,
) -> Pairing<R> {
    let (sender, receiver) = mpsc::channel::<Ready>();
    std::thread::scope(|scope| {
        let host = scope.spawn(move || {
            let on_ready = |ready: &Ready| {
                let _ = sender.send(ready.clone());
            };
            let random = |buf: &mut [u8]| {
                use rand::Rng;
                rand::rng().fill_bytes(buf);
            };
            let net = FakeNet(setup.banner.clone());
            let mut account = world.account();
            account.shell = setup.shell.clone();
            let env = Env {
                version: "test",
                account,
                hostname: Some("testhost.example.net".into()),
                etc_ssh: world.etc.path().to_path_buf(),
                program_dirs: Vec::<PathBuf>::new(),
                interfaces: interfaces(),
                platform: setup.platform,
                net: &net,
                keyscan: &NoKeyscan,
                shell: &FakeShell,
                tz_set: setup.tz_set,
                exe: setup.exe.clone(),
                prompt: script,
                can_ask: setup.can_ask,
                color: false,
                random: &random,
                now: &DateTime::now,
                signals,
                window: setup.window,
                poll: setup.poll,
                on_ready: Some(&on_ready),
                install_keys: setup.install_keys,
            };
            let mut out = Vec::new();
            let exit = run(options, &env, &mut out);
            (exit, String::from_utf8(out).unwrap())
        });
        // Either the host gets ready, or it ends without (the sender is dropped with it).
        let phone = receiver
            .recv_timeout(Duration::from_secs(30))
            .ok()
            .map(phone);
        let (exit, output) = host.join().unwrap();
        Pairing {
            exit,
            output,
            phone,
        }
    })
}

/// A run with the default pretend host.
pub fn pair_default<R>(
    world: &World,
    options: &Options,
    lines: &[&str],
    phone: impl FnOnce(Ready) -> R,
) -> Pairing<R> {
    pair(
        world,
        options,
        &Setup::default(),
        &Script::new(lines),
        &FakeSignals::default(),
        phone,
    )
}

/// The pairing code the CLI printed, from its output.
pub fn printed_code(output: &str) -> String {
    output
        .lines()
        .find(|line| line.starts_with("or2-pair:2?"))
        .unwrap_or_else(|| panic!("no pairing code in:\n{output}"))
        .to_owned()
}

/// The code parsed by the strict reader (which mirrors the phone's parser).
pub fn parse_code(code: &str) -> Payload {
    reference::parse(code).unwrap_or_else(|error| panic!("{code}: {error:?}"))
}

pub fn max_bytes() -> usize {
    payload::MAX_BYTES
}
