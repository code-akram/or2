//! The whole CLI flow against the or2-core client, in process, over loopback, in a temporary
//! home: what `or2-pair` prints, listens on and writes, and what the phone sees.

// The listener installs keys, which only Unix does; elsewhere `tests/manual_keys.rs` applies.
#![cfg(unix)]

mod common;

use std::io;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use or2_core::pair::{PairError, PairOffer, PairParseError, Refusal};
use or2_pair::account::Account;
use or2_pair::confirm::Answer;
use or2_pair::net::{Net, PairListener, StdNet};
use or2_pair::run::{Exit, RunError};

use common::*;

const WINDOW: Duration = Duration::from_secs(20);

#[test]
fn a_phone_pairs_end_to_end_and_its_key_lands_in_authorized_keys() {
    let world = World::new();
    let confirm = Auto::new(Answer::Yes);
    let key = phone_key();
    let result = pair(&world, &options(), &confirm, WINDOW, |ready| {
        phone_pairs(&ready.payload, &key, "Pixel 8")
    });
    assert_eq!(result.phone, Some(Ok(())));
    assert_eq!(result.exit.unwrap(), Exit::Paired);

    // The file: created private, one line, options and comment as specified, the phone's key.
    let keys = world.authorized_keys().unwrap();
    assert_eq!(
        keys,
        format!("no-agent-forwarding,no-X11-forwarding {key} or2-Pixel-8-2026-07-01\n")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &str| {
            std::fs::metadata(world.home.path().join(path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode(".ssh"), 0o700);
        assert_eq!(mode(".ssh/authorized_keys"), 0o600);
    }

    // The person was asked once, with the phone's fingerprint and the sanitized label.
    let asked = confirm.asked.lock().unwrap();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].user, "alice");
    assert_eq!(asked[0].device, "Pixel-8");
    assert!(asked[0].fingerprint.starts_with("SHA256:"));
    assert!(asked[0].peer.starts_with("127.0.0.1:"));

    // What was printed: the checks, the host, a QR and the code itself, and the verdict.
    let out = &result.output;
    for needle in [
        "Checks",
        "sshd is not answering on port 1",
        HOST_FINGERPRINT,
        "10.147.17.5",
        "192.168.1.20",
        "testhost.local",
        "or2-pair:1?name=Test%20Host",
        "Listening on 127.0.0.1:",
        "Authorized Pixel-8",
    ] {
        assert!(out.contains(needle), "missing {needle:?} in:\n{out}");
    }
    assert!(
        !out.contains("172.17.0.1"),
        "container bridges are not listed"
    );
}

#[test]
fn the_code_round_trips_through_the_strict_parser() {
    let world = World::new();
    let result = pair(
        &world,
        &options(),
        &Auto::new(Answer::No),
        // Nobody connects: the listener times out quickly.
        Duration::from_millis(300),
        |ready| {
            let offer = PairOffer::parse(&ready.payload).unwrap();
            (ready, offer)
        },
    );
    let (ready, offer) = result.phone.unwrap();
    assert_eq!(offer.name, "Test Host");
    assert_eq!(offer.username, "alice");
    assert_eq!(offer.port, 1);
    let addresses: Vec<_> = offer.addresses.iter().map(|a| a.host()).collect();
    assert_eq!(
        addresses,
        ["10.147.17.5", "192.168.1.20", "testhost.local"],
        "overlay, then LAN, then the mDNS name"
    );
    assert_eq!(offer.host_key.fingerprint(), HOST_FINGERPRINT);
    let exchange = offer.exchange.unwrap();
    assert_eq!(exchange.endpoints.len(), 1);
    assert_eq!(
        exchange.endpoints[0].host(),
        ready.listening[0].ip().to_string()
    );
    assert_eq!(exchange.endpoints[0].port(), ready.listening[0].port());
    assert!(ready.payload.len() <= 1024);
}

#[test]
fn declining_on_the_host_refuses_the_phone_and_changes_nothing() {
    let world = World::new();
    let key = phone_key();
    let result = pair(
        &world,
        &options(),
        &Auto::new(Answer::No),
        WINDOW,
        |ready| phone_pairs(&ready.payload, &key, "phone"),
    );
    assert_eq!(
        result.phone,
        Some(Err(PairError::Refused(Refusal::Declined)))
    );
    assert_eq!(result.exit.unwrap(), Exit::Declined);
    assert!(world.authorized_keys().is_none());
    assert!(!world.home.path().join(".ssh").exists());
}

#[test]
fn a_wrong_password_is_refused_but_does_not_end_the_listener() {
    let world = World::new();
    let confirm = Auto::new(Answer::Yes);
    let key = phone_key();
    let result = pair(&world, &options(), &confirm, WINDOW, |ready| {
        // A code that differs from the host's only in its password (an old code, say).
        let mut parts: Vec<String> = ready.payload.split('&').map(str::to_owned).collect();
        let last = parts.last_mut().unwrap();
        *last = "otp=AAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned();
        let wrong = phone_pairs(&parts.join("&"), &key, "phone");
        // Nothing was shown to the person for it, and the real code still works.
        let asked_after_wrong = confirm.asked.lock().unwrap().len();
        (
            wrong,
            asked_after_wrong,
            phone_pairs(&ready.payload, &key, "phone"),
        )
    });
    let (wrong, asked_after_wrong, right) = result.phone.unwrap();
    // The host cannot prove anything to a phone that holds another password, so the phone
    // believes neither the refusal nor anything else, and its code is not spent.
    assert_eq!(wrong, Err(PairError::HostNotAuthenticated));
    assert_eq!(asked_after_wrong, 0, "nothing was shown to the person");
    assert_eq!(right, Ok(()));
    assert_eq!(result.exit.unwrap(), Exit::Paired);
    assert!(
        result.output.contains("1 connection(s) were refused"),
        "{}",
        result.output
    );
    assert!(world.authorized_keys().unwrap().contains(&key));
}

/// A man in the middle for one connection: forwards everything, but rewrites the host's
/// refusals into successes (the proof it cannot make stays what it was).
fn flipping_proxy(target: std::net::SocketAddr) -> std::net::SocketAddr {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let (mut client, _) = listener.accept().unwrap();
        let upstream = std::net::TcpStream::connect(target).unwrap();
        let mut upstream_write = upstream.try_clone().unwrap();
        let mut client_read = client.try_clone().unwrap();
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut client_read, &mut upstream_write);
        });
        for line in BufReader::new(upstream).lines() {
            let Ok(line) = line else { break };
            let line = line.replace("\"ok\":false", "\"ok\":true");
            if client.write_all(format!("{line}\n").as_bytes()).is_err() {
                break;
            }
        }
    });
    address
}

#[test]
fn the_phone_does_not_believe_a_success_the_host_did_not_send() {
    // Finding 5 end to end: the real host declines; a party on the path turns its refusal into
    // a success. The phone checks the host's proof, so it reports an unverified answer and not
    // a paired host.
    let world = World::new();
    let key = phone_key();
    let result = pair(
        &world,
        &options(),
        &Auto::new(Answer::No),
        WINDOW,
        |ready| {
            let proxy = flipping_proxy(ready.listening[0]);
            let code = ready
                .payload
                .replace(&ready.listening[0].to_string(), &proxy.to_string());
            phone_pairs(&code, &key, "phone")
        },
    );
    assert_eq!(result.phone, Some(Err(PairError::HostNotAuthenticated)));
    assert_eq!(result.exit.unwrap(), Exit::Declined);
    assert!(world.authorized_keys().is_none());
}

#[test]
fn the_listener_serves_one_attempt_and_then_closes() {
    let world = World::new();
    let confirm = Auto::new(Answer::Yes);
    let key = phone_key();
    let result = pair(&world, &options(), &confirm, WINDOW, |ready| {
        let offer = PairOffer::parse(&ready.payload).unwrap();
        let first = phone_submits(&offer, &key, "phone");
        // The same code, used again after the host finished: nobody is listening any more.
        let again = phone_submits(&offer, &key, "phone");
        (first, again)
    });
    let (first, again) = result.phone.unwrap();
    assert_eq!(first, Ok(()));
    assert_eq!(again, Err(PairError::Unreachable));
    assert_eq!(world.authorized_keys().unwrap().lines().count(), 1);
    assert_eq!(confirm.asked.lock().unwrap().len(), 1);
}

#[test]
fn a_connection_that_never_speaks_does_not_use_up_the_attempt() {
    let world = World::new();
    let confirm = Auto::new(Answer::Yes);
    let key = phone_key();
    let result = pair(&world, &options(), &confirm, WINDOW, |ready| {
        // A port scanner: connects, hears the hello, hangs up.
        drop(std::net::TcpStream::connect(ready.listening[0]).unwrap());
        phone_pairs(&ready.payload, &key, "phone")
    });
    assert_eq!(result.phone, Some(Ok(())));
    assert_eq!(result.exit.unwrap(), Exit::Paired);
}

#[test]
fn idle_connections_do_not_keep_an_honest_phone_waiting() {
    // Finding 4: the listener served one connection at a time, so a peer that connected and said
    // nothing held every phone behind it until its own timeout; two of them outlasted the
    // phone's 10 s wait for the greeting.
    let world = World::new();
    let confirm = Auto::new(Answer::Yes);
    let key = phone_key();
    let result = pair(&world, &options(), &confirm, WINDOW, |ready| {
        // Idle sockets, held open for the whole exchange. Some hear the greeting, some do not.
        let idle: Vec<std::net::TcpStream> = (0..3)
            .map(|_| std::net::TcpStream::connect(ready.listening[0]).unwrap())
            .collect();
        let offer = PairOffer::parse(&ready.payload).unwrap();
        let started = Instant::now();
        // The phone waits two seconds for the greeting, not the shipped ten.
        let paired = phone_submits_within(&offer, &key, "phone", Duration::from_secs(2));
        let took = started.elapsed();
        drop(idle);
        (paired, took)
    });
    let (paired, took) = result.phone.unwrap();
    assert_eq!(paired, Ok(()), "after {took:?}");
    assert!(took < Duration::from_secs(2), "{took:?}");
    assert_eq!(result.exit.unwrap(), Exit::Paired);
}

#[test]
fn one_unauthenticated_byte_does_not_use_up_the_attempt() {
    // Finding 3: a scanner that sends a newline (or junk, or a wrong proof) used to end the
    // listener for the phone that follows.
    use std::io::{Read, Write};
    let world = World::new();
    let confirm = Auto::new(Answer::Yes);
    let key = phone_key();
    let result = pair(&world, &options(), &confirm, WINDOW, |ready| {
        let mut replies = Vec::new();
        for junk in [&b"\n"[..], b"GET / HTTP/1.1\r\n\r\n", b"{\"v\":1"] {
            let mut probe = std::net::TcpStream::connect(ready.listening[0]).unwrap();
            probe
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            probe.write_all(junk).unwrap();
            let _ = probe.shutdown(std::net::Shutdown::Write);
            let mut seen = String::new();
            let _ = probe.read_to_string(&mut seen);
            replies.push(seen);
        }
        (replies, phone_pairs(&ready.payload, &key, "phone"))
    });
    let (replies, paired) = result.phone.unwrap();
    assert!(
        replies.iter().all(|seen| seen.contains("\"ok\":false")),
        "{replies:?}"
    );
    assert_eq!(paired, Ok(()));
    assert_eq!(result.exit.unwrap(), Exit::Paired);
    assert!(world.authorized_keys().unwrap().contains(&key));
}

#[test]
fn nobody_pairing_ends_at_the_window_with_nothing_changed() {
    let world = World::new();
    let started = Instant::now();
    let result = pair(
        &world,
        &options(),
        &Auto::new(Answer::Yes),
        Duration::from_millis(300),
        |_| (),
    );
    assert_eq!(result.exit.unwrap(), Exit::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(result.output.contains("Timed out"));
    assert!(world.authorized_keys().is_none());
}

#[test]
fn an_already_authorized_key_is_acknowledged_without_a_second_line() {
    let world = World::new();
    let key = phone_key();
    let ssh = world.home.path().join(".ssh");
    std::fs::create_dir(&ssh).unwrap();
    std::fs::write(ssh.join("authorized_keys"), format!("{key} old\n")).unwrap();
    let result = pair(
        &world,
        &options(),
        &Auto::new(Answer::Yes),
        WINDOW,
        |ready| phone_pairs(&ready.payload, &key, "phone"),
    );
    assert_eq!(result.phone, Some(Ok(())));
    assert_eq!(result.exit.unwrap(), Exit::Paired);
    assert_eq!(world.authorized_keys().unwrap(), format!("{key} old\n"));
    assert!(result.output.contains("already authorized"));
}

#[cfg(unix)]
#[test]
fn a_writable_by_others_authorized_keys_is_refused_with_the_fix_not_silently_appended_to() {
    // Finding 6: sshd (StrictModes) would ignore this file; the pairing used to report success.
    use std::os::unix::fs::PermissionsExt;
    let world = World::new();
    let key = phone_key();
    let ssh = world.home.path().join(".ssh");
    std::fs::create_dir(&ssh).unwrap();
    std::fs::write(ssh.join("authorized_keys"), "# mine\n").unwrap();
    std::fs::set_permissions(
        ssh.join("authorized_keys"),
        std::fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    let result = pair(
        &world,
        &options(),
        &Auto::new(Answer::Yes),
        WINDOW,
        |ready| phone_pairs(&ready.payload, &key, "phone"),
    );
    assert_eq!(
        result.phone,
        Some(Err(PairError::Refused(Refusal::HostFailed)))
    );
    assert_eq!(result.exit.unwrap(), Exit::Failed);
    assert!(
        result.output.contains("writable by other users") && result.output.contains("chmod go-w"),
        "{}",
        result.output
    );
    assert_eq!(world.authorized_keys().unwrap(), "# mine\n");
    // The check run before listening already said so.
    assert!(
        result.output.contains("warn") && result.output.contains("StrictModes"),
        "{}",
        result.output
    );
}

#[test]
fn an_existing_file_is_backed_up_and_appended_to() {
    let world = World::new();
    let key = phone_key();
    let ssh = world.home.path().join(".ssh");
    std::fs::create_dir(&ssh).unwrap();
    std::fs::write(ssh.join("authorized_keys"), "# mine\n").unwrap();
    let result = pair(
        &world,
        &options(),
        &Auto::new(Answer::Yes),
        WINDOW,
        |ready| phone_pairs(&ready.payload, &key, "phone"),
    );
    assert_eq!(result.exit.unwrap(), Exit::Paired);
    let keys = world.authorized_keys().unwrap();
    assert!(keys.starts_with("# mine\n"));
    assert_eq!(keys.lines().count(), 2);
    let backups: Vec<_> = std::fs::read_dir(&ssh)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("authorized_keys.or2-backup-"))
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(
        std::fs::read_to_string(ssh.join(&backups[0])).unwrap(),
        "# mine\n"
    );
}

#[test]
fn no_listen_prints_a_code_with_no_exchange_and_opens_nothing() {
    let world = World::new();
    let mut options = options();
    options.bind.clear();
    options.no_listen = true;
    let result = pair(&world, &options, &Auto::new(Answer::Yes), WINDOW, |_| ());
    assert!(result.phone.is_none(), "no listener, so never ready");
    assert_eq!(result.exit.unwrap(), Exit::CodeOnly);
    let code = result
        .output
        .lines()
        .find(|line| line.starts_with("or2-pair:1?"))
        .expect("the code is printed as text");
    assert!(!code.contains("pair=") && !code.contains("otp="));
    let offer = PairOffer::parse(code).unwrap();
    assert!(offer.exchange.is_none());
    assert!(result.output.contains("No listener"));
    assert!(world.authorized_keys().is_none());
}

#[test]
fn listening_needs_a_person_to_ask() {
    let world = World::new();
    let result = pair_with(
        &world,
        &options(),
        &Auto::new(Answer::Yes),
        WINDOW,
        &StdNet,
        false,
        |_| (),
    );
    assert!(matches!(result.exit, Err(RunError::NotInteractive)));
    // It is fine without a listener.
    let mut options = options();
    options.bind.clear();
    options.no_listen = true;
    let result = pair_with(
        &world,
        &options,
        &Auto::new(Answer::Yes),
        WINDOW,
        &StdNet,
        false,
        |_| (),
    );
    assert_eq!(result.exit.unwrap(), Exit::CodeOnly);
}

#[test]
fn check_only_reports_and_changes_nothing() {
    let world = World::new();
    let mut options = options();
    options.check_only = true;
    let result = pair(&world, &options, &Auto::new(Answer::Yes), WINDOW, |_| ());
    assert_eq!(result.exit.unwrap(), Exit::Checked);
    assert!(result.output.contains("Checks") && result.output.contains("Nothing was changed"));
    assert!(!result.output.contains("or2-pair:1?"));
    assert!(!world.home.path().join(".ssh").exists());
}

/// Records what the CLI asks the network layer to bind and then refuses.
struct Recording(Mutex<Vec<Vec<IpAddr>>>);

impl Net for Recording {
    fn probe_ssh(&self, _: u16) -> io::Result<String> {
        Ok("SSH-2.0-Fake".into())
    }
    fn listen(&self, ips: &[IpAddr], _: u16) -> io::Result<Box<dyn PairListener>> {
        self.0.lock().unwrap().push(ips.to_vec());
        Err(io::Error::other("recorded"))
    }
}

#[test]
fn by_default_only_overlay_and_lan_addresses_are_bound() {
    let world = World::new();
    let net = Recording(Mutex::default());
    let mut options = options();
    options.bind.clear();
    let result = pair_with(
        &world,
        &options,
        &Auto::new(Answer::Yes),
        WINDOW,
        &net,
        true,
        |_| (),
    );
    assert!(matches!(result.exit, Err(RunError::Listen(_))));
    let bound = net.0.lock().unwrap();
    let expected: Vec<IpAddr> = vec![
        "10.147.17.5".parse().unwrap(),
        "192.168.1.20".parse().unwrap(),
    ];
    assert_eq!(
        *bound,
        [expected],
        "no loopback, no docker bridge, no names"
    );
}

#[test]
fn a_user_that_is_not_this_account_is_refused_before_anything_happens() {
    // Finding 1: `--user bob` while running as alice used to print and confirm "bob" and then
    // authorize the key in alice's home.
    let world = World::new();
    let net = Recording(Mutex::default());
    let mut options = options();
    options.bind.clear();
    options.user = Some("bob".into());
    let result = pair_with(
        &world,
        &options,
        &Auto::new(Answer::Yes),
        WINDOW,
        &net,
        true,
        |_| (),
    );
    match &result.exit {
        Err(RunError::UserMismatch {
            requested,
            effective,
        }) => {
            assert_eq!((requested.as_str(), effective.as_str()), ("bob", "alice"));
        }
        other => panic!("{other:?}"),
    }
    assert!(net.0.lock().unwrap().is_empty(), "nothing was bound");
    assert!(!result.output.contains("or2-pair:1?"), "{}", result.output);
    assert!(!world.home.path().join(".ssh").exists());
    let message = result.exit.unwrap_err().to_string();
    assert!(
        message.contains("bob") && message.contains("alice"),
        "{message}"
    );
}

#[test]
fn the_code_and_the_confirmation_name_the_account_whose_file_is_written() {
    let world = World::new();
    let confirm = Auto::new(Answer::Yes);
    let key = phone_key();
    let mut options = options();
    options.user = None;
    let result = pair(&world, &options, &confirm, WINDOW, |ready| {
        let offer = PairOffer::parse(&ready.payload).unwrap();
        assert_eq!(offer.username, "alice", "the account's own name");
        phone_submits(&offer, &key, "phone")
    });
    assert_eq!(result.phone, Some(Ok(())));
    let asked = confirm.asked.lock().unwrap();
    assert_eq!(asked[0].user, "alice");
    assert_eq!(
        asked[0].target,
        world.home.path().join(".ssh/authorized_keys"),
        "the prompt shows the file that will change"
    );
    assert!(world.authorized_keys().unwrap().contains(&key));
}

fn code_line(output: &str) -> &str {
    output
        .lines()
        .find(|line| line.starts_with("or2-pair:1?"))
        .expect("a code was printed")
}

fn many_interfaces(count: usize) -> Vec<or2_pair::addresses::Iface> {
    (1..=count)
        .map(|i| or2_pair::addresses::Iface {
            name: format!("eth{i}"),
            ip: format!("192.168.{i}.20").parse().unwrap(),
        })
        .collect()
}

#[test]
fn a_host_with_many_addresses_prints_a_code_the_phone_accepts() {
    // Finding 9: nine or more short addresses stayed under 1 KB, so nothing trimmed them, and the
    // phone's limit of eight refused the code.
    let world = World::new();
    let mut options = options();
    options.bind.clear();
    options.no_listen = true;
    options.addresses = vec![
        "dev.example.org".into(),
        "backup.example.org".into(),
        "192.168.3.20".into(),
    ];
    let mut out = Vec::new();
    let exit = run_with_output(
        &world,
        &options,
        &QueueingNet(Mutex::default()),
        many_interfaces(12),
        Duration::from_millis(100),
        &mut out,
    );
    assert_eq!(exit.unwrap(), Exit::CodeOnly);
    let output = String::from_utf8(out).unwrap();
    let code = code_line(&output);
    assert!(code.len() <= 1024);
    let offer = PairOffer::parse(code).expect("the phone accepts every code the CLI prints");
    assert_eq!(offer.addresses.len(), 8);
    // The named ones come first and the ones that did not fit are reported, not hidden.
    let hosts: Vec<_> = offer.addresses.iter().map(|a| a.host()).collect();
    assert_eq!(
        hosts[..3],
        ["dev.example.org", "backup.example.org", "192.168.3.20"]
    );
    assert!(
        output.contains("were left out") && output.contains("at most 8 addresses"),
        "{output}"
    );
    assert!(output.contains("[left out"), "{output}");
}

#[test]
fn a_code_the_phone_would_refuse_is_never_printed() {
    let world = World::new();
    let mut options = options();
    options.bind.clear();
    options.no_listen = true;
    options.user = None;
    let net = QueueingNet(Mutex::default());
    // A login the phone would refuse (over 64 characters).
    let mut out = Vec::new();
    let result = run_as(
        &world,
        &"u".repeat(65),
        &options,
        &net,
        interfaces(),
        Duration::from_millis(100),
        &mut out,
    );
    let error = result.unwrap_err();
    assert!(matches!(error, RunError::InvalidCode(_)), "{error}");
    assert!(error.to_string().contains("`user`"), "{error}");
    let output = String::from_utf8(out).unwrap();
    assert!(!output.contains("or2-pair:1?"), "{output}");
    // An address the phone would refuse.
    options.addresses = vec!["not a host".into()];
    let mut out = Vec::new();
    let error = run_with_output(
        &world,
        &options,
        &net,
        interfaces(),
        Duration::from_millis(100),
        &mut out,
    )
    .unwrap_err();
    assert!(matches!(error, RunError::InvalidCode(_)), "{error}");
    assert!(!String::from_utf8(out).unwrap().contains("or2-pair:1?"));
}

#[test]
fn naming_a_detected_address_with_address_keeps_every_default_listener() {
    // Finding 8: `--address <the LAN address>` put it first and dropped it from the bind list.
    let world = World::new();
    let net = Recording(Mutex::default());
    let mut options = options();
    options.bind.clear();
    options.addresses = vec!["192.168.1.20".into()];
    let result = pair_with(
        &world,
        &options,
        &Auto::new(Answer::Yes),
        WINDOW,
        &net,
        true,
        |_| (),
    );
    assert!(matches!(result.exit, Err(RunError::Listen(_))));
    let bound = net.0.lock().unwrap();
    let expected: Vec<IpAddr> = vec![
        "192.168.1.20".parse().unwrap(),
        "10.147.17.5".parse().unwrap(),
    ];
    assert_eq!(*bound, [expected], "the named address first, nothing lost");
}

#[test]
fn a_host_with_only_a_public_address_refuses_to_listen_by_default() {
    // The same fake world but the only interface is public: nothing is bindable.
    struct PublicOnly;
    impl Net for PublicOnly {
        fn probe_ssh(&self, _: u16) -> io::Result<String> {
            Ok("SSH-2.0-Fake".into())
        }
        fn listen(&self, _: &[IpAddr], _: u16) -> io::Result<Box<dyn PairListener>> {
            panic!("must not listen");
        }
    }
    let world = World::new();
    let mut options = options();
    options.bind.clear();
    // Name the public address explicitly as an advertised one, and give no private interface:
    // `common::interfaces` has private ones, so use --address only plus a world without them.
    options.addresses = vec!["203.0.113.9".into()];
    let result = run_with_interfaces(
        &world,
        &options,
        &PublicOnly,
        vec![or2_pair::addresses::Iface {
            name: "eth1".into(),
            ip: "203.0.113.9".parse().unwrap(),
        }],
    );
    assert!(matches!(result, Err(RunError::NoBindAddress)), "{result:?}");
}

fn run_with_interfaces(
    world: &World,
    options: &or2_pair::args::Options,
    net: &dyn Net,
    interfaces: Vec<or2_pair::addresses::Iface>,
) -> Result<Exit, RunError> {
    run_with_output(
        world,
        options,
        net,
        interfaces,
        Duration::from_millis(100),
        &mut Vec::new(),
    )
}

fn run_with_output(
    world: &World,
    options: &or2_pair::args::Options,
    net: &dyn Net,
    interfaces: Vec<or2_pair::addresses::Iface>,
    window: Duration,
    out: &mut dyn std::io::Write,
) -> Result<Exit, RunError> {
    run_as(world, "alice", options, net, interfaces, window, out)
}

/// [`run_with_output`] for an account of the given login name.
fn run_as(
    world: &World,
    login: &str,
    options: &or2_pair::args::Options,
    net: &dyn Net,
    interfaces: Vec<or2_pair::addresses::Iface>,
    window: Duration,
    out: &mut dyn std::io::Write,
) -> Result<Exit, RunError> {
    use or2_pair::checks::Platform;
    use or2_pair::date::DateTime;
    use or2_pair::run::{Env, run};
    let confirm = Auto::new(Answer::Yes);
    let random = |buf: &mut [u8]| buf.fill(1);
    let now = || DateTime::from_unix(0);
    let env = Env {
        version: "test",
        account: Account::new(login, world.home.path()),
        hostname: Some("box".into()),
        etc_ssh: world.etc.path().to_path_buf(),
        program_dirs: vec![],
        interfaces,
        platform: Platform::Linux,
        net,
        keyscan: &NoKeyscan,
        confirm: &confirm,
        can_ask: true,
        color: false,
        random: &random,
        now: &now,
        window,
        on_ready: None,
        install_keys: true,
    };
    run(options, &env, out)
}

/// Binds like the real network and queues a client on the listener at once.
struct QueueingNet(Mutex<Option<std::net::TcpStream>>);

impl Net for QueueingNet {
    fn probe_ssh(&self, _: u16) -> io::Result<String> {
        Ok("SSH-2.0-Fake".into())
    }
    fn listen(&self, ips: &[IpAddr], port: u16) -> io::Result<Box<dyn PairListener>> {
        let listener = StdNet.listen(ips, port)?;
        let endpoint = listener.endpoints()[0];
        *self.0.lock().unwrap() = Some(std::net::TcpStream::connect(endpoint)?);
        Ok(listener)
    }
}

/// An output that takes its time, like a slow terminal or a pipe nobody reads.
struct SlowWriter(Duration);

impl std::io::Write for SlowWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        std::thread::sleep(self.0);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn slow_output_does_not_extend_the_window_and_a_queued_connection_is_not_served_after_it() {
    // Finding 7: the window used to start after the QR was printed, and `accept` handed out a
    // connection that was queued before the window ended even when it was over.
    use std::io::Read;
    let world = World::new();
    let net = QueueingNet(Mutex::default());
    let window = Duration::from_millis(300);
    let started = Instant::now();
    let exit = run_with_output(
        &world,
        &options(),
        &net,
        interfaces(),
        window,
        // Dozens of writes at 30 ms each: printing alone outlasts the window.
        &mut SlowWriter(Duration::from_millis(30)),
    );
    assert_eq!(exit.unwrap(), Exit::TimedOut);
    let mut client = net.0.lock().unwrap().take().expect("a client was queued");
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut heard = [0u8; 64];
    let count = client.read(&mut heard).unwrap_or(0);
    assert_eq!(
        count, 0,
        "a connection queued during the window got a hello"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert!(world.authorized_keys().is_none());
}

#[test]
fn an_explicit_public_bind_is_allowed_with_a_warning() {
    // 203.0.113.9 is not ours, so the real bind fails, but only after the warning.
    let world = World::new();
    let mut options = options();
    options.bind = vec!["203.0.113.9".parse().unwrap()];
    let result = pair(&world, &options, &Auto::new(Answer::Yes), WINDOW, |_| ());
    assert!(matches!(result.exit, Err(RunError::Listen(_))));
    // The warning went to the output before the bind was attempted.
    assert!(
        result.output.contains("is a public address"),
        "{}",
        result.output
    );
}

#[test]
fn a_missing_host_key_stops_before_anything_is_listed_or_listened() {
    let world = World::new();
    std::fs::remove_file(world.etc.path().join("ssh_host_ed25519_key.pub")).unwrap();
    let result = pair(&world, &options(), &Auto::new(Answer::Yes), WINDOW, |_| ());
    assert!(matches!(result.exit, Err(RunError::HostKey(_))));
    assert!(!result.output.contains("or2-pair:1?"));
}

#[test]
fn the_printed_qr_decodes_to_the_pairing_code() {
    let world = World::new();
    let mut options = options();
    options.bind.clear();
    options.no_listen = true;
    let result = pair(&world, &options, &Auto::new(Answer::Yes), WINDOW, |_| ());
    // Colourless unicode on a dark terminal: ink is the *light* module.
    let drawing: Vec<&str> = result
        .output
        .lines()
        .skip_while(|line| !line.contains("Scan this"))
        .skip(2)
        .take_while(|line| !line.is_empty())
        .collect();
    let code = result
        .output
        .lines()
        .find(|line| line.starts_with("or2-pair:1?"))
        .unwrap();
    assert_eq!(decode(&drawing), code);
}

/// Turns the half-block drawing back into pixels (ink = light module on a dark terminal) and
/// reads it with an independent QR decoder.
fn decode(lines: &[&str]) -> String {
    let scale = 4;
    let width = lines[0].chars().count();
    let height = lines.len() * 2;
    let mut pixels = vec![0u8; width * height];
    for (row, line) in lines.iter().enumerate() {
        for (col, c) in line.chars().enumerate() {
            let (top_ink, bottom_ink) = match c {
                '█' => (true, true),
                '▀' => (true, false),
                '▄' => (false, true),
                _ => (false, false),
            };
            // Ink is light on a dark terminal: a dark module is the absence of ink.
            pixels[(row * 2) * width + col] = if top_ink { 255 } else { 0 };
            pixels[(row * 2 + 1) * width + col] = if bottom_ink { 255 } else { 0 };
        }
    }
    let mut image =
        rqrr::PreparedImage::prepare_from_greyscale(width * scale, height * scale, |x, y| {
            pixels[(y / scale) * width + x / scale]
        });
    let grids = image.detect_grids();
    assert_eq!(grids.len(), 1, "one QR code in the drawing");
    let (_, content) = grids[0].decode().expect("the QR decodes");
    content
}

#[test]
fn the_parser_names_what_is_wrong_with_a_code_from_a_newer_host_tool() {
    let world = World::new();
    let mut options = options();
    options.bind.clear();
    options.no_listen = true;
    let result = pair(&world, &options, &Auto::new(Answer::Yes), WINDOW, |_| ());
    let code = result
        .output
        .lines()
        .find(|line| line.starts_with("or2-pair:1?"))
        .unwrap()
        .replacen("or2-pair:1?", "or2-pair:2?", 1);
    assert_eq!(
        PairOffer::parse(&code).unwrap_err(),
        PairParseError::UnsupportedVersion
    );
}
