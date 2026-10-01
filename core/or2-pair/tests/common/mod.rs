//! A pretend host for the CLI's integration tests: a temporary home, a temporary `/etc/ssh`,
//! made-up interfaces and the real loopback sockets. Nothing here touches the user's own
//! `~/.ssh`, sshd, tmux or herdr.

#![allow(dead_code)]

use std::io;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use or2_core::keys::ClientKey;
use or2_core::pair::{PairError, PairOffer, PairTiming, submit_key};
use or2_core::transport::DirectTcp;
use or2_pair::account::Account;
use or2_pair::addresses::Iface;
use or2_pair::args::Options;
use or2_pair::checks::Platform;
use or2_pair::confirm::{Answer, Confirm, ConfirmRequest};
use or2_pair::date::DateTime;
use or2_pair::hostkey::Keyscan;
use or2_pair::net::{Net, StdNet};
use or2_pair::run::{Env, Exit, Ready, RunError, run};

pub const HOST_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7";
pub const HOST_FINGERPRINT: &str = "SHA256:PK/nvGiFusFK9/6Qf8dSOX99mI5XQYiMAML2JHCjgQI";

pub struct World {
    pub home: tempfile::TempDir,
    pub etc: tempfile::TempDir,
}

impl World {
    pub fn new() -> Self {
        let world = Self {
            home: tempfile::tempdir().unwrap(),
            etc: tempfile::tempdir().unwrap(),
        };
        std::fs::write(
            world.etc.path().join("ssh_host_ed25519_key.pub"),
            format!("{HOST_KEY} root@testhost\n"),
        )
        .unwrap();
        world
    }

    pub fn authorized_keys(&self) -> Option<String> {
        std::fs::read_to_string(self.home.path().join(".ssh/authorized_keys")).ok()
    }
}

/// Answers the question the way the test says; records what was asked.
pub struct Auto {
    pub answer: Answer,
    pub asked: std::sync::Mutex<Vec<ConfirmRequest>>,
}

impl Auto {
    pub fn new(answer: Answer) -> Self {
        Self {
            answer,
            asked: Default::default(),
        }
    }
}

impl Confirm for Auto {
    fn confirm(&self, request: &ConfirmRequest, _: Instant) -> Answer {
        self.asked.lock().unwrap().push(request.clone());
        self.answer
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
        ssh_port: Some(1),
        bind: vec!["127.0.0.1".parse::<IpAddr>().unwrap()],
        ..Options::default()
    }
}

/// A fixed sshd that does not exist: the check reports it and carries on.
pub struct LoopbackNet<'a>(pub &'a dyn Net);

pub struct Pairing<R> {
    pub exit: Result<Exit, RunError>,
    pub output: String,
    /// What the phone closure returned (`None` when the host never got ready).
    pub phone: Option<R>,
}

/// Runs the whole CLI flow in a thread against `world` and calls `phone` once the host is
/// listening.
pub fn pair<R>(
    world: &World,
    options: &Options,
    confirm: &Auto,
    window: Duration,
    phone: impl FnOnce(Ready) -> R,
) -> Pairing<R> {
    pair_with(world, options, confirm, window, &StdNet, true, phone)
}

pub fn pair_with<R>(
    world: &World,
    options: &Options,
    confirm: &Auto,
    window: Duration,
    net: &(dyn Net + Sync),
    can_ask: bool,
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
            let now = || DateTime::from_unix(1_782_867_661);
            let env = Env {
                version: "test",
                account: Account::new("alice", world.home.path()),
                hostname: Some("testhost.example.net".into()),
                etc_ssh: world.etc.path().to_path_buf(),
                program_dirs: Vec::<PathBuf>::new(),
                interfaces: interfaces(),
                platform: Platform::Linux,
                net,
                keyscan: &NoKeyscan,
                confirm,
                can_ask,
                color: false,
                random: &random,
                now: &now,
                window,
                on_ready: Some(&on_ready),
            };
            let mut out = Vec::new();
            let exit = run(options, &env, &mut out);
            (exit, String::from_utf8(out).unwrap())
        });
        let phone = receiver
            .recv_timeout(Duration::from_secs(20))
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

/// The phone: parses the code and runs the or2-core client over the real `DirectTcp`.
pub fn phone_pairs(code: &str, public_key: &str, device: &str) -> Result<(), PairError> {
    let offer = PairOffer::parse(code).expect("the CLI's code parses");
    phone_submits(&offer, public_key, device)
}

pub fn phone_submits(offer: &PairOffer, public_key: &str, device: &str) -> Result<(), PairError> {
    phone_submits_within(offer, public_key, device, Duration::from_secs(5))
}

/// The phone with its own patience for the host's greeting and for each step.
pub fn phone_submits_within(
    offer: &PairOffer,
    public_key: &str,
    device: &str,
    step: Duration,
) -> Result<(), PairError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(submit_key(
        &Arc::new(DirectTcp),
        offer,
        public_key,
        device,
        PairTiming {
            step,
            verdict: Duration::from_secs(30),
        },
    ))
}

/// A fresh phone key as `<algorithm> <base64>`.
pub fn phone_key() -> String {
    let line = ClientKey::generate_ed25519("phone").public_key().openssh;
    line.split(' ').take(2).collect::<Vec<_>>().join(" ")
}
