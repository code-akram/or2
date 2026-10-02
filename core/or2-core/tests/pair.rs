//! The phone's Easy pair client (`or2_core::pair::pair_enroll`) against a disposable loopback
//! OpenSSH (`common::Sshd`). The bootstrap key is authorized for a forced command that is a small
//! POSIX `sh` script emulating `or2-pair enroll` (hello, one request line, verdict), so the whole
//! path is real: sshd's authentication, its forced command through the account's login shell, the
//! exec channel and its close. Only temporary keys, a temporary home and an ephemeral port are
//! used. The CLI's own end-to-end tests (the real `or2-pair`) are in the `or2-pair` crate.
//!
//! Skipped without `/usr/bin/sshd`; `OR2_REQUIRE_SSHD` turns the skip into a failure.

mod common;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Sshd, sshd_ready};
use or2_core::keys::ClientKey;
use or2_core::pair::{PairCode, PairError, PairOffer, PairResult, PairTiming, pair_enroll};
use or2_core::transport::{DirectTcp, Endpoint};
use or2_core::trust::HostKey;

macro_rules! require_sshd {
    () => {
        if !sshd_ready() {
            return;
        }
    };
}

const ID: &str = "abcdefghijklm";
const CODE: &str = "7KQ4-M2XD-9PTM";
const STEP: Duration = Duration::from_secs(10);

/// What every emulated `or2-pair` does first: record the command sshd was asked for.
const PRELUDE: &str =
    "dir=$(dirname \"$0\")\nprintf '%s' \"$SSH_ORIGINAL_COMMAND\" > \"$dir/command\"\n";
const HELLO: &str = "printf '{\"v\":2,\"hello\":\"or2-pair\",\"id\":\"abcdefghijklm\"}\\n'\n";
/// Reads the request line (or notes that stdin ended first) and keeps it.
const READ: &str = "IFS= read -r request || { : > \"$dir/eof\"; exit 1; }\nprintf '%s\\n' \"$request\" > \"$dir/request\"\n";

struct Fixture {
    sshd: Sshd,
    offer: PairOffer,
    code: PairCode,
    phone: ClientKey,
}

impl Fixture {
    /// An sshd whose `authorized_keys` holds the bootstrap key of `CODE` and `ID`, restricted to
    /// the script `body` (rewritten with [`Fixture::script`]).
    fn new(body: &str) -> Self {
        let sshd = Sshd::new(false);
        let code = PairCode::parse_typed(CODE).unwrap();
        let fixture = Self {
            offer: PairOffer {
                name: "workstation".into(),
                username: Sshd::username(),
                port: sshd.port,
                addresses: vec![Endpoint::new("127.0.0.1", sshd.port).unwrap()],
                host_key: HostKey::from_openssh(&sshd.host).unwrap(),
                pairing_id: Some(ID.into()),
            },
            sshd,
            code,
            phone: ClientKey::generate_ed25519("phone"),
        };
        fixture.authorize(&fixture.code);
        fixture.script(body);
        fixture
    }

    fn path(&self, name: &str) -> PathBuf {
        self.sshd.home().join(name)
    }

    /// Authorizes the bootstrap key `code` derives, for the forced command.
    fn authorize(&self, code: &PairCode) {
        let line = format!(
            "restrict,command=\"/bin/sh {}\" {} or2-pair-bootstrap-{ID}\n",
            self.path("or2-pair.sh").display(),
            code.bootstrap_public_key(ID)
        );
        fs::write(self.path("authorized"), line).unwrap();
    }

    fn script(&self, body: &str) {
        fs::write(self.path("or2-pair.sh"), format!("{PRELUDE}{body}")).unwrap();
        for file in ["command", "request", "eof"] {
            let _ = fs::remove_file(self.path(file));
        }
    }

    fn fingerprint(&self) -> String {
        self.phone.public_key().fingerprint
    }

    /// `printf` of one verdict line.
    fn say(&self, verdict: &str) -> String {
        format!("printf '%s\\n' '{verdict}'\n")
    }

    fn ok(&self) -> String {
        let user = &self.offer.username;
        self.say(&format!(
            "{{\"v\":2,\"ok\":true,\"user\":\"{user}\",\"fingerprint\":\"{}\"}}",
            self.fingerprint()
        ))
    }

    async fn enroll(&self, step: Duration) -> Result<PairResult, PairError> {
        self.enroll_with(&self.offer, &self.code, step).await
    }

    async fn enroll_with(
        &self,
        offer: &PairOffer,
        code: &PairCode,
        step: Duration,
    ) -> Result<PairResult, PairError> {
        pair_enroll(
            &Arc::new(DirectTcp),
            offer,
            code,
            &self.phone.public_key().openssh,
            "OnePlus",
            PairTiming { step },
        )
        .await
    }

    fn read(&self, name: &str) -> Option<String> {
        fs::read_to_string(self.path(name)).ok()
    }

    /// How many times sshd logged `what` (its log is at VERBOSE).
    fn logged(&self, what: &str) -> usize {
        fs::read_to_string(self.path("log"))
            .unwrap()
            .matches(what)
            .count()
    }

    /// Waits until the script has created the file `name` in the sshd's home.
    async fn wait_for(&self, name: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.path(name).exists() {
            assert!(Instant::now() < deadline, "{name} never appeared");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

#[tokio::test]
async fn pairs_through_sshd_with_a_forced_command() {
    require_sshd!();
    let fixture = Fixture::new("");
    let ok = fixture.ok();
    fixture.script(&format!("{HELLO}{READ}{ok}"));
    let result = fixture.enroll(STEP).await.unwrap();
    assert_eq!(result.username, Sshd::username());
    assert_eq!(result.fingerprint, fixture.fingerprint());
    // sshd ran the forced command for the exec the phone asked for.
    assert_eq!(fixture.read("command").as_deref(), Some("or2-pair"));
    let request = fixture.read("request").unwrap();
    let key = fixture.phone.public_key().openssh;
    let key = key.split(' ').take(2).collect::<Vec<_>>().join(" ");
    assert_eq!(
        request,
        format!("{{\"v\":2,\"key\":\"{key}\",\"device\":\"OnePlus\"}}\n")
    );
    // One handshake, one authentication, nothing refused.
    assert_eq!(fixture.logged("Accepted publickey"), 1);
    assert_eq!(fixture.logged("Failed"), 0);
    assert_eq!(fixture.logged("Connection from"), 1);
}

#[tokio::test]
async fn shell_noise_before_the_hello_is_skipped() {
    require_sshd!();
    let fixture = Fixture::new("");
    let ok = fixture.ok();
    // What an rc file that echoes looks like: a banner, and a line on stderr.
    fixture.script(&format!(
        "echo 'Welcome back'\necho 'warning: no tty' >&2\n{HELLO}{READ}{ok}"
    ));
    assert!(fixture.enroll(STEP).await.is_ok());
}

#[tokio::test]
async fn too_much_noise_or_another_program_is_not_or2_pair() {
    require_sshd!();
    let fixture = Fixture::new("");
    // A line of 4100 characters before the hello: past the 4 KB budget.
    fixture.script(&format!(
        "i=0\nwhile [ $i -lt 41 ]; do printf '%0100d' 0; i=$((i+1)); done; echo\n{HELLO}{READ}"
    ));
    assert_eq!(fixture.enroll(STEP).await, Err(PairError::NotOr2Pair));
    // Something else answers (a ForceCommand in sshd_config would look like this).
    fixture.script("echo 'This account is currently not available.'\nexit 1\n");
    assert_eq!(fixture.enroll(STEP).await, Err(PairError::NotOr2Pair));
    // Nothing at all, and it exits.
    fixture.script("exit 0\n");
    assert_eq!(fixture.enroll(STEP).await, Err(PairError::NotOr2Pair));
}

#[tokio::test]
async fn every_refusal_reason_maps_to_its_error() {
    require_sshd!();
    let fixture = Fixture::new("");
    for (reason, expected) in [
        ("expired", PairError::Expired),
        ("gone", PairError::Gone),
        ("key", PairError::KeyNotAccepted),
        ("failed", PairError::HostFailed),
        ("request", PairError::Refused),
        ("not-a-reason-this-phone-knows", PairError::Refused),
    ] {
        let verdict = fixture.say(&format!("{{\"v\":2,\"ok\":false,\"reason\":\"{reason}\"}}"));
        fixture.script(&format!("{HELLO}{READ}{verdict}"));
        assert_eq!(fixture.enroll(STEP).await, Err(expected), "{reason}");
    }
}

#[tokio::test]
async fn a_hello_for_another_run_is_a_protocol_error() {
    require_sshd!();
    let fixture =
        Fixture::new("printf '{\"v\":2,\"hello\":\"or2-pair\",\"id\":\"bcdefghijklmn\"}\\n'\n");
    assert_eq!(fixture.enroll(STEP).await, Err(PairError::Protocol));
    assert_eq!(fixture.read("request"), None, "nothing was sent");
}

#[tokio::test]
async fn another_code_is_refused_and_the_command_never_runs() {
    require_sshd!();
    let fixture = Fixture::new("");
    fixture.script(&format!("{HELLO}{READ}"));
    let other = PairCode::parse_typed("1111-0000-000A").unwrap();
    assert_eq!(
        fixture.enroll_with(&fixture.offer, &other, STEP).await,
        Err(PairError::BootstrapRefused)
    );
    assert_eq!(fixture.read("command"), None);
    // The same when the run is over and the host removed the key.
    fs::write(fixture.path("authorized"), "").unwrap();
    assert_eq!(fixture.enroll(STEP).await, Err(PairError::BootstrapRefused));
    assert_eq!(fixture.read("command"), None);
}

#[tokio::test]
async fn another_host_key_is_refused_before_authentication() {
    require_sshd!();
    let fixture = Fixture::new("");
    fixture.script(&format!("{HELLO}{READ}"));
    // Another ed25519 key, and a key of a type the host does not have.
    for other in [
        ClientKey::generate_ed25519("").public_key().openssh,
        "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBJKzE3AhnQL0fsk77dWQaJJj+GHelMX7ge+TZ2xgJ/Y3O5aMMlnH6Hn/tnucj94OezBBvEj2VIcJGu8ABqUkhFA=".to_owned(),
    ] {
        let offer = PairOffer {
            host_key: HostKey::from_openssh(&other).unwrap(),
            ..fixture.offer.clone()
        };
        assert_eq!(
            fixture.enroll_with(&offer, &fixture.code, STEP).await,
            Err(PairError::HostKeyMismatch)
        );
    }
    assert_eq!(fixture.read("command"), None);
    assert_eq!(fixture.logged("Accepted publickey"), 0);
}

#[tokio::test]
async fn a_closed_port_is_unreachable() {
    require_sshd!();
    let fixture = Fixture::new("");
    let closed = {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.local_addr().unwrap().port()
    };
    let offer = PairOffer {
        addresses: vec![Endpoint::new("127.0.0.1", closed).unwrap()],
        port: closed,
        ..fixture.offer.clone()
    };
    assert_eq!(
        fixture.enroll_with(&offer, &fixture.code, STEP).await,
        Err(PairError::Unreachable)
    );
}

#[tokio::test]
async fn a_host_that_stalls_times_out_at_the_hello_and_at_the_verdict() {
    require_sshd!();
    let fixture = Fixture::new("");
    let step = Duration::from_millis(700);
    fixture.script("sleep 4\n");
    let started = Instant::now();
    assert_eq!(fixture.enroll(step).await, Err(PairError::TimedOut));
    assert!(started.elapsed() < Duration::from_secs(3));
    fixture.script(&format!("{HELLO}{READ}sleep 4\n"));
    assert_eq!(fixture.enroll(step).await, Err(PairError::TimedOut));
    assert!(fixture.read("request").is_some(), "the request was sent");
}

#[tokio::test]
async fn cancelling_closes_the_channel_so_the_command_sees_its_input_end() {
    require_sshd!();
    let fixture = Fixture::new("");
    // The command reads the request and never answers; `cat` returns only when its input ends.
    fixture.script(&format!("{HELLO}{READ}cat > /dev/null\n: > \"$dir/eof\"\n"));
    let mut enrolling = Box::pin(fixture.enroll(Duration::from_secs(30)));
    tokio::select! {
        result = &mut enrolling => panic!("nobody answered, yet {result:?}"),
        () = fixture.wait_for("request") => {}
    }
    // The caller gives up: the connection's close path ends the channel and the command's stdin.
    drop(enrolling);
    fixture.wait_for("eof").await;
}
