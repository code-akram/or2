//! A host for tests of other programs (the Kotlin JVM end-to-end test): the real `or2-pair`
//! flow with a confirmation that answers by itself.
//!
//! Built only with the `test-support` feature, so `cargo install` and a plain build never see
//! it: the shipped `or2-pair` has no way to answer its own question.
//!
//! ```text
//! or2-pair-testhost --home DIR --etc DIR --ssh-port N [--answer yes|no] [--window-secs N] [--address HOST]
//! ```
//!
//! Standard output carries two machine-readable lines: the pairing code once the listener is up
//! (`PAYLOAD <code>`), and, when it ends, `RESULT <exit>`. The human output goes to standard
//! error. It listens on 127.0.0.1 only, in the given home, never in the real one.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use or2_pair::addresses::Iface;
use or2_pair::args::Options;
use or2_pair::checks::Platform;
use or2_pair::confirm::{Answer, Confirm, ConfirmRequest};
use or2_pair::date::DateTime;
use or2_pair::hostkey::Keyscan;
use or2_pair::net::StdNet;
use or2_pair::run::{Env, Ready, run};

struct Auto(Answer);

impl Confirm for Auto {
    fn confirm(&self, request: &ConfirmRequest, _: std::time::Instant) -> Answer {
        eprintln!(
            "auto-confirming {} for {}: {}",
            request.device, request.user, request.fingerprint
        );
        self.0
    }
}

struct NoKeyscan;

impl Keyscan for NoKeyscan {
    fn scan(&self, _: u16, _: &str) -> std::io::Result<String> {
        Ok(String::new())
    }
}

fn main() -> ExitCode {
    let mut home = None;
    let mut etc = None;
    let mut ssh_port = 22u16;
    let mut answer = Answer::Yes;
    let mut window = 30u64;
    let mut addresses = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().expect("every flag takes a value");
        match flag.as_str() {
            "--home" => home = Some(PathBuf::from(value())),
            "--etc" => etc = Some(PathBuf::from(value())),
            "--ssh-port" => ssh_port = value().parse().expect("a port"),
            "--answer" => {
                answer = if value() == "no" {
                    Answer::No
                } else {
                    Answer::Yes
                };
            }
            "--window-secs" => window = value().parse().expect("seconds"),
            "--address" => addresses.push(value()),
            other => panic!("unknown flag {other}"),
        }
    }
    let (Some(home), Some(etc)) = (home, etc) else {
        eprintln!("--home and --etc are required");
        return ExitCode::from(2);
    };
    let options = Options {
        user: Some("test".into()),
        name: Some("Test Host".into()),
        ssh_port: Some(ssh_port),
        bind: vec!["127.0.0.1".parse().expect("loopback")],
        addresses,
        ..Options::default()
    };
    let on_ready = |ready: &Ready| {
        println!("PAYLOAD {}", ready.payload);
        let _ = std::io::stdout().flush();
    };
    let random = |buf: &mut [u8]| {
        use rand::Rng;
        rand::rng().fill_bytes(buf);
    };
    let now = DateTime::now;
    let confirm = Auto(answer);
    let env = Env {
        version: "testhost",
        home,
        user: Some("test".into()),
        hostname: Some("testhost".into()),
        etc_ssh: etc,
        program_dirs: Vec::new(),
        interfaces: Vec::<Iface>::new(),
        platform: Platform::current(),
        net: &StdNet,
        keyscan: &NoKeyscan,
        confirm: &confirm,
        can_ask: true,
        color: false,
        random: &random,
        now: &now,
        window: Duration::from_secs(window),
        on_ready: Some(&on_ready),
    };
    let result = run(&options, &env, &mut std::io::stderr());
    match result {
        Ok(exit) => {
            println!("RESULT {exit:?}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            println!("RESULT error {error}");
            ExitCode::FAILURE
        }
    }
}
