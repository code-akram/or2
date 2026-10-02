//! The whole CLI flow in process, in a temporary home with a scripted terminal and a pretend sshd
//! banner: what `or2-pair` asks, prints and writes, and how it ends. The phone is played by a
//! direct call of the forced command's code (`exchange::enroll`); the real sshd path is in
//! `tests/sshd.rs`.
#![cfg(unix)]

mod common;

use std::io::Cursor;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use or2_pair::bootstrap::{self, PairingId};
use or2_pair::code::PairCode;
use or2_pair::date::DateTime;
use or2_pair::exchange::{self, Enroll, Outcome, Reason};
use or2_pair::run::{Exit, RunError};

use common::*;

fn new_code() -> PairCode {
    PairCode::generate(&|buf: &mut [u8]| {
        use rand::Rng;
        rand::rng().fill_bytes(buf);
    })
}

/// The phone's side of the exchange, minus SSH: runs the forced command's code against the
/// world's home with this request.
fn enroll(world: &World, id: &str, key: &str, device: &str) -> (Outcome, String) {
    let account = world.account();
    let env = Enroll {
        account: &account,
        now: &DateTime::now,
        request_timeout: Duration::from_secs(5),
    };
    let request = format!("{{\"v\":2,\"key\":\"{key}\",\"device\":\"{device}\"}}\n");
    let mut out = Vec::new();
    let outcome = exchange::enroll(
        &PairingId::parse(id).unwrap(),
        &env,
        Box::new(Cursor::new(request.into_bytes())),
        &mut out,
    );
    (outcome, String::from_utf8(out).unwrap())
}

fn id_of(code: &str) -> String {
    parse_code(code).id.expect("a pairing id").to_string()
}

#[test]
fn a_phone_pairs_and_its_key_replaces_the_temporary_one() {
    let world = World::new();
    let original = format!("# mine\n{OTHER_KEY} me@laptop\n");
    world.write_keys(&original);
    let k = new_code();
    let typed = k.display().to_lowercase();
    let result = pair_default(&world, &options(), &[&typed], |ready| {
        let code = parse_code(&ready.payload);
        let id = code.id.clone().unwrap();
        // While the run waits: the temporary line is in the file, with the options for this
        // sshd (9.9), the executable's path and the key derived from K and the id.
        let keys = world.authorized_keys().unwrap();
        let line = keys
            .lines()
            .find(|l| l.contains("or2-pair-bootstrap-"))
            .unwrap()
            .to_owned();
        let bootstrap_key = bootstrap::public_key(&k, &id);
        let expiry = line
            .split("expiry-time=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap();
        assert_eq!(expiry.len(), 12);
        assert!(expiry.bytes().all(|b| b.is_ascii_digit()), "{expiry}");
        assert!(
            line.starts_with(&format!(
                "restrict,command=\"{EXE} enroll {id}\",expiry-time=\""
            )),
            "{line}"
        );
        assert!(
            line.ends_with(&format!(
                "\" {} or2-pair-bootstrap-{id}",
                bootstrap_key.openssh()
            )),
            "{line}"
        );
        assert!(keys.starts_with(&original), "what was there is kept");
        assert_eq!(world.state_files(), [format!("{id}.json")], "one live run");
        let state: serde_json::Value = serde_json::from_slice(
            &std::fs::read(world.ssh_dir().join("or2-pair").join(format!("{id}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(state["id"], id.as_str());
        assert_eq!(state["fingerprint"], bootstrap_key.fingerprint());
        assert_eq!(state["uid"], world.account().uid);

        let (outcome, said) = enroll(&world, id.as_str(), PHONE_KEY, "Pixel 8");
        assert!(
            matches!(outcome, Outcome::Installed { added: true, .. }),
            "{outcome:?}\n{said}"
        );
        outcome
    });
    assert!(result.phone.is_some());
    assert_eq!(result.exit.unwrap(), Exit::Paired, "{}", result.output);

    // The file: what was there, the phone's line, no temporary key; one backup of the original.
    let keys = world.authorized_keys().unwrap();
    let date = DateTime::now().date();
    assert_eq!(
        keys,
        format!("{original}no-agent-forwarding,no-X11-forwarding {PHONE_KEY} or2-Pixel-8-{date}\n")
    );
    assert!(world.state_files().is_empty(), "{:?}", world.state_files());
    let backups = world.backups();
    assert_eq!(backups.len(), 1);
    assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), original);

    // What was printed, in order: the checks, the prompt, the host, the key line, the QR.
    let out = &result.output;
    let order = [
        "Checks",
        "sshd is answering on port 22 (OpenSSH_9.9)",
        "Code shown on your phone: ",
        "This host",
        HOST_FINGERPRINT,
        "A temporary pairing key was added for alice until ",
        "or2-pair:2?name=Test%20Host",
        "Waiting for the phone",
        "Paired \"Pixel-8\" (SHA256:",
    ];
    let mut at = 0;
    for needle in order {
        let found = out[at..]
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} missing or out of order in:\n{out}"));
        at += found + needle.len();
    }
    assert!(
        out.contains(&format!("delete the line ending or2-Pixel-8-{date}")),
        "{out}"
    );
    assert!(
        !out.contains(&k.display()) && !out.contains(&typed),
        "K is never printed"
    );
    assert!(
        !out.contains("172.17.0.1"),
        "container bridges are not listed"
    );
}

#[test]
fn the_code_is_a_valid_version_2_code_with_the_new_address_order() {
    let world = World::new();
    let result = pair_default(&world, &options(), &[&new_code().display()], |ready| {
        let code = parse_code(&ready.payload);
        enroll(&world, code.id.as_ref().unwrap().as_str(), PHONE_KEY, "p");
        ready.payload
    });
    let text = result.phone.unwrap();
    let code = parse_code(&text);
    assert_eq!(code.name, "Test Host");
    assert_eq!(code.user, "alice");
    assert_eq!(code.port, 22);
    assert_eq!(code.host_key, HOST_KEY);
    assert_eq!(
        code.addresses,
        [
            "10.147.17.5",
            "192.168.1.20",
            "203.0.113.9",
            "2001:db8::9",
            "testhost.local"
        ],
        "overlay, LAN, public IPv4, public IPv6, then the mDNS name"
    );
    assert!(text.len() <= max_bytes());
    assert!(
        result.output.contains(
            "10.147.17.5 (overlay), 192.168.1.20 (LAN), 203.0.113.9 (public), 2001:db8::9 (public)"
        ),
        "{}",
        result.output
    );
}

#[test]
fn a_typo_asks_again_and_costs_nothing() {
    let world = World::new();
    let good = new_code().display();
    // Change the check character.
    let mut typo: Vec<char> = good.chars().collect();
    let last = typo.len() - 1;
    typo[last] = if typo[last] == '3' { '4' } else { '3' };
    let typo: String = typo.into_iter().collect();
    let script = Script::new(&[&typo, "not a code", &good]);
    let result = pair(
        &world,
        &options(),
        &Setup::default(),
        &script,
        &FakeSignals::default(),
        |ready| {
            let code = parse_code(&ready.payload);
            enroll(&world, code.id.as_ref().unwrap().as_str(), PHONE_KEY, "p")
        },
    );
    assert_eq!(result.exit.unwrap(), Exit::Paired, "{}", result.output);
    assert_eq!(script.asked.load(Ordering::SeqCst), 3);
    assert!(
        result
            .output
            .contains("That code has a typo. Type it again"),
        "{}",
        result.output
    );
    assert!(result.output.contains("wrong length"), "{}", result.output);
}

#[test]
fn an_empty_line_or_the_end_of_input_ends_the_run_with_nothing_changed() {
    for lines in [&[""][..], &["   "], &[]] {
        let world = World::new();
        let result = pair_default(&world, &options(), lines, |_| ());
        assert!(result.phone.is_none(), "never got ready");
        assert_eq!(result.exit.unwrap(), Exit::Cancelled, "{}", result.output);
        assert!(
            result.output.contains("Nothing was changed"),
            "{}",
            result.output
        );
        assert!(
            !world.ssh_dir().exists(),
            "{lines:?}: ~/.ssh was not even created"
        );
    }
}

#[test]
fn too_many_typos_end_the_run_with_nothing_changed() {
    let world = World::new();
    let lines = ["0000-0000-0001"; 12];
    let script = Script::new(&lines);
    let result = pair(
        &world,
        &options(),
        &Setup::default(),
        &script,
        &FakeSignals::default(),
        |_| (),
    );
    assert_eq!(result.exit.unwrap(), Exit::Cancelled);
    assert_eq!(script.asked.load(Ordering::SeqCst), 10);
    assert!(!world.ssh_dir().exists());
}

#[test]
fn without_a_terminal_it_refuses_before_asking_or_changing_anything() {
    let world = World::new();
    let script = Script::new(&[&new_code().display()]);
    let setup = Setup {
        can_ask: false,
        ..Setup::default()
    };
    let result = pair(
        &world,
        &options(),
        &setup,
        &script,
        &FakeSignals::default(),
        |_| (),
    );
    assert!(
        matches!(result.exit, Err(RunError::NotInteractive)),
        "{:?}",
        result.exit
    );
    assert_eq!(script.asked.load(Ordering::SeqCst), 0);
    assert!(!result.output.contains("Checks") && !world.ssh_dir().exists());
    let message = RunError::NotInteractive.to_string();
    assert!(
        message.contains("terminal") && message.contains("--manual"),
        "{message}"
    );
}

#[test]
fn the_timeout_removes_the_temporary_key_and_a_late_phone_finds_nothing() {
    let world = World::new();
    let original = format!("{OTHER_KEY} mine\n");
    world.write_keys(&original);
    let setup = Setup {
        window: Duration::from_secs(1),
        ..Setup::default()
    };
    let id = std::sync::Mutex::new(String::new());
    let started = Instant::now();
    let result = pair(
        &world,
        &options(),
        &setup,
        &Script::new(&[&new_code().display()]),
        &FakeSignals::default(),
        |ready| *id.lock().unwrap() = id_of(&ready.payload),
    );
    assert_eq!(result.exit.unwrap(), Exit::TimedOut, "{}", result.output);
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert_eq!(
        world.authorized_keys().unwrap(),
        original,
        "back to what it was"
    );
    assert!(world.state_files().is_empty());
    assert!(result.output.contains("Timed out") && result.output.contains("removed"));
    // The forced command of a run that is over refuses.
    let (outcome, said) = enroll(&world, &id.lock().unwrap(), PHONE_KEY, "late");
    assert_eq!(outcome, Outcome::Refused(Reason::Expired), "{said}");
    assert_eq!(world.authorized_keys().unwrap(), original);
}

#[test]
fn a_signal_ends_the_run_and_removes_the_temporary_key() {
    let world = World::new();
    let original = format!("{OTHER_KEY} mine\n");
    world.write_keys(&original);
    let signals = FakeSignals::default();
    let result = pair(
        &world,
        &options(),
        &Setup::default(),
        &Script::new(&[&new_code().display()]),
        &signals,
        |ready| {
            assert!(
                world
                    .authorized_keys()
                    .unwrap()
                    .contains("or2-pair-bootstrap-")
            );
            signals.raise();
            id_of(&ready.payload)
        },
    );
    assert_eq!(result.exit.unwrap(), Exit::Interrupted, "{}", result.output);
    assert_eq!(
        signals.armed.load(Ordering::SeqCst),
        1,
        "armed once, before the key was written"
    );
    assert_eq!(world.authorized_keys().unwrap(), original);
    assert!(world.state_files().is_empty());
    assert!(
        result
            .output
            .contains("Cancelled. The temporary key was removed.")
    );
    let (outcome, _) = enroll(&world, &result.phone.unwrap(), PHONE_KEY, "late");
    assert_eq!(outcome, Outcome::Refused(Reason::Expired));
}

#[test]
fn a_second_phone_is_told_gone() {
    let world = World::new();
    // The run looks for the result only every half second, so the state is still there when
    // the second phone arrives.
    let setup = Setup {
        poll: Duration::from_millis(500),
        ..Setup::default()
    };
    let result = pair(
        &world,
        &options(),
        &setup,
        &Script::new(&[&new_code().display()]),
        &FakeSignals::default(),
        |ready| {
            let id = id_of(&ready.payload);
            let first = enroll(&world, &id, PHONE_KEY, "first");
            let second = enroll(&world, &id, OTHER_KEY, "second");
            (first.0, second.0)
        },
    );
    let (first, second) = result.phone.unwrap();
    assert!(matches!(first, Outcome::Installed { .. }));
    // The state file is still there until the run notices, but the entry is gone.
    assert_eq!(second, Outcome::Refused(Reason::Gone));
    assert_eq!(result.exit.unwrap(), Exit::Paired);
    let keys = world.authorized_keys().unwrap();
    assert!(keys.contains(PHONE_KEY) && !keys.contains(OTHER_KEY));
}

#[test]
fn two_phones_at_once_one_wins() {
    let world = World::new();
    let setup = Setup {
        poll: Duration::from_millis(500),
        ..Setup::default()
    };
    let result = pair(
        &world,
        &options(),
        &setup,
        &Script::new(&[&new_code().display()]),
        &FakeSignals::default(),
        |ready| {
            let id = id_of(&ready.payload);
            std::thread::scope(|scope| {
                let a = scope.spawn(|| enroll(&world, &id, PHONE_KEY, "a").0);
                let b = scope.spawn(|| enroll(&world, &id, OTHER_KEY, "b").0);
                [a.join().unwrap(), b.join().unwrap()]
            })
        },
    );
    let outcomes = result.phone.unwrap();
    let installed = outcomes
        .iter()
        .filter(|o| matches!(o, Outcome::Installed { .. }))
        .count();
    assert_eq!(installed, 1, "{outcomes:?}");
    assert!(
        outcomes.contains(&Outcome::Refused(Reason::Gone)),
        "{outcomes:?}"
    );
    assert_eq!(result.exit.unwrap(), Exit::Paired);
    let keys = world.authorized_keys().unwrap();
    assert_eq!(keys.matches("no-agent-forwarding").count(), 1, "{keys}");
}

#[test]
fn old_runs_are_swept_and_live_ones_are_left_alone() {
    use or2_pair::state::{State, StateDir};
    let world = World::new();
    let account = world.account();
    let now = DateTime::now().to_unix();
    let dead = "aaaaaaaaaaaaa"; // a state file past its deadline
    let orphan = "bbbbbbbbbbbbb"; // an entry with no state file
    let live = "ccccccccccccc"; // another run still waiting
    let line = |id: &str, key: &str| {
        format!("restrict,command=\"/x enroll {id}\" {key} or2-pair-bootstrap-{id}\n")
    };
    world.write_keys(&format!(
        "{OTHER_KEY} mine\n{}{}{}",
        line(dead, PHONE_KEY),
        line(orphan, HOST_KEY),
        line(
            live,
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIALYvXruViE9G83T84ZJqbdJkEImlV0NRg9AC6Yw4NYo"
        )
    ));
    let dir = StateDir::open(&account, true).unwrap().unwrap();
    for (id, deadline) in [(dead, now - 100), (live, now + 1000)] {
        dir.write_state(&State {
            id: id.into(),
            deadline,
            uid: account.uid,
            fingerprint: "SHA256:x".into(),
        })
        .unwrap();
    }
    dir.write_done(
        &PairingId::parse("ddddddddddddd").unwrap(),
        &or2_pair::state::Done {
            device: "x".into(),
            fingerprint: "SHA256:y".into(),
        },
    )
    .unwrap();
    drop(dir);
    let before = world.authorized_keys().unwrap();

    let setup = Setup {
        window: Duration::from_secs(1),
        ..Setup::default()
    };
    let result = pair(
        &world,
        &options(),
        &setup,
        &Script::new(&[&new_code().display()]),
        &FakeSignals::default(),
        |ready| {
            let keys = world.authorized_keys().unwrap();
            assert!(
                !keys.contains(&format!("or2-pair-bootstrap-{dead}")),
                "{keys}"
            );
            assert!(
                !keys.contains(&format!("or2-pair-bootstrap-{orphan}")),
                "{keys}"
            );
            assert!(
                keys.contains(&format!("or2-pair-bootstrap-{live}")),
                "{keys}"
            );
            let files = world.state_files();
            assert!(files.contains(&format!("{live}.json")), "{files:?}");
            assert!(!files.contains(&format!("{dead}.json")), "{files:?}");
            assert!(
                !files.contains(&"ddddddddddddd.done".to_owned()),
                "{files:?}"
            );
            assert_eq!(files.len(), 2, "the other run's and ours: {files:?}");
            id_of(&ready.payload)
        },
    );
    assert_eq!(result.exit.unwrap(), Exit::TimedOut, "{}", result.output);
    // Afterwards the other run's entry and state are as they were, and ours is gone.
    let keys = world.authorized_keys().unwrap();
    assert!(keys.starts_with(&format!("{OTHER_KEY} mine\n")));
    assert_eq!(keys.matches("or2-pair-bootstrap-").count(), 1, "{keys}");
    assert_eq!(world.state_files(), [format!("{live}.json")]);
    // One backup, of the file as it was before the sweep changed it.
    let backups = world.backups();
    assert_eq!(backups.len(), 1);
    assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), before);
}

#[test]
fn check_reports_leftovers_and_changes_nothing() {
    let world = World::new();
    let stale = "restrict,command=\"/x enroll aaaaaaaaaaaaa\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIALYvXruViE9G83T84ZJqbdJkEImlV0NRg9AC6Yw4NYo or2-pair-bootstrap-aaaaaaaaaaaaa\n";
    world.write_keys(stale);
    let mut options = options();
    options.check_only = true;
    let script = Script::new(&[]);
    let result = pair(
        &world,
        &options,
        &Setup::default(),
        &script,
        &FakeSignals::default(),
        |_| (),
    );
    assert_eq!(result.exit.unwrap(), Exit::Checked, "{}", result.output);
    assert!(
        result
            .output
            .contains("1 temporary pairing key(s) of earlier runs"),
        "{}",
        result.output
    );
    assert!(result.output.contains("Nothing was changed"));
    assert_eq!(world.authorized_keys().unwrap(), stale);
    assert!(world.state_files().is_empty() && world.backups().is_empty());
    assert_eq!(script.asked.load(Ordering::SeqCst), 0);
    assert!(
        !result.output.contains("or2-pair:2?"),
        "no pairing code in a check"
    );
}

#[test]
fn the_options_follow_the_sshd_version() {
    for (banner, expiry, restrict, note) in [
        ("SSH-2.0-OpenSSH_9.9", true, true, false),
        ("SSH-2.0-OpenSSH_7.4p1 Debian-10", false, true, true),
        ("SSH-2.0-OpenSSH_6.6.1p1 Ubuntu-2", false, false, true),
    ] {
        let world = World::new();
        let setup = Setup {
            banner: Ok(banner.into()),
            window: Duration::from_secs(1),
            ..Setup::default()
        };
        let result = pair(
            &world,
            &options(),
            &setup,
            &Script::new(&[&new_code().display()]),
            &FakeSignals::default(),
            |_| {
                let keys = world.authorized_keys().unwrap();
                let line = keys.lines().next().unwrap().to_owned();
                assert_eq!(line.contains("expiry-time="), expiry, "{banner}: {line}");
                assert_eq!(line.starts_with("restrict,"), restrict, "{banner}: {line}");
                if !restrict {
                    assert!(
                        line.contains(",no-pty,no-port-forwarding,no-agent-forwarding,no-X11-forwarding,no-user-rc "),
                        "{line}"
                    );
                }
            },
        );
        assert!(result.phone.is_some(), "{banner}: {}", result.output);
        assert_eq!(result.exit.unwrap(), Exit::TimedOut);
        assert_eq!(
            result.output.contains("cannot expire the key in the file"),
            note,
            "{banner}"
        );
        assert_eq!(
            world.authorized_keys().unwrap(),
            "",
            "{banner}: removed again"
        );
    }
}

#[test]
fn what_makes_pairing_impossible_stops_the_run_before_the_prompt_and_before_any_change() {
    use std::os::unix::fs::PermissionsExt;
    type Change = Box<dyn Fn(&World, &mut Setup)>;
    let cases: Vec<(&str, Change, &str)> = vec![
        (
            "not OpenSSH",
            Box::new(|_, s| s.banner = Ok("SSH-2.0-dropbear_2022.83".into())),
            "not OpenSSH",
        ),
        (
            "no banner",
            Box::new(|_, s| s.banner = Err(std::io::ErrorKind::ConnectionRefused)),
            "sshd is not answering",
        ),
        (
            "a path that needs quoting",
            Box::new(|_, s| s.exe = Ok("/home/my user/bin/or2-pair".into())),
            "a space",
        ),
        (
            "an unknown path",
            Box::new(|_, s| s.exe = Err("no /proc".into())),
            "cannot tell where",
        ),
        (
            "a login shell that cannot run commands",
            Box::new(|_, s| s.shell = Some("/usr/sbin/nologin".into())),
            "login shell",
        ),
        (
            "a home others can write (StrictModes)",
            Box::new(|w, _| {
                std::fs::set_permissions(w.home.path(), std::fs::Permissions::from_mode(0o775))
                    .unwrap();
            }),
            "StrictModes",
        ),
        (
            "PubkeyAuthentication no",
            Box::new(|w, _| {
                std::fs::write(
                    w.etc.path().join("sshd_config"),
                    "PubkeyAuthentication no\n",
                )
                .unwrap();
            }),
            "PubkeyAuthentication no",
        ),
    ];
    for (what, change, reason) in cases {
        let world = World::new();
        let mut setup = Setup::default();
        change(&world, &mut setup);
        let script = Script::new(&[&new_code().display()]);
        let result = pair(
            &world,
            &options(),
            &setup,
            &script,
            &FakeSignals::default(),
            |_| (),
        );
        assert!(
            matches!(result.exit, Err(RunError::Blocked)),
            "{what}: {:?}\n{}",
            result.exit,
            result.output
        );
        assert!(
            result.output.contains("fail") && result.output.contains(reason),
            "{what}:\n{}",
            result.output
        );
        assert_eq!(
            script.asked.load(Ordering::SeqCst),
            0,
            "{what}: asked for a code"
        );
        assert!(!world.ssh_dir().exists(), "{what}: touched ~/.ssh");
        let message = RunError::Blocked.to_string();
        assert!(message.contains("--manual"), "{message}");
    }
}

#[test]
fn sshd_config_warnings_are_shown_but_do_not_stop_the_pairing() {
    let world = World::new();
    std::fs::write(
        world.etc.path().join("sshd_config"),
        "ForceCommand /bin/true\n",
    )
    .unwrap();
    let setup = Setup {
        window: Duration::from_secs(1),
        ..Setup::default()
    };
    let result = pair(
        &world,
        &options(),
        &setup,
        &Script::new(&[&new_code().display()]),
        &FakeSignals::default(),
        |_| (),
    );
    assert_eq!(result.exit.unwrap(), Exit::TimedOut);
    assert!(
        result
            .output
            .contains("warn  sshd_config sets a ForceCommand")
            && result.output.contains("--manual"),
        "{}",
        result.output
    );
}

#[test]
fn manual_prints_the_code_without_an_id_and_changes_nothing() {
    let world = World::new();
    let mut options = options();
    options.manual = true;
    let script = Script::new(&[]);
    let setup = Setup {
        can_ask: false,
        ..Setup::default()
    };
    let result = pair(
        &world,
        &options,
        &setup,
        &script,
        &FakeSignals::default(),
        |_| (),
    );
    assert_eq!(result.exit.unwrap(), Exit::CodeOnly, "{}", result.output);
    assert_eq!(
        script.asked.load(Ordering::SeqCst),
        0,
        "no code is asked for"
    );
    let code = parse_code(&printed_code(&result.output));
    assert!(code.id.is_none());
    assert!(!result.output.contains("&id="));
    assert!(
        result.output.contains("--manual: nothing was changed"),
        "{}",
        result.output
    );
    assert!(
        result.output.contains("authorized_keys"),
        "{}",
        result.output
    );
    assert!(!world.ssh_dir().exists());
}

#[test]
fn the_printed_qr_decodes_to_the_pairing_code() {
    let world = World::new();
    let result = pair_default(&world, &options(), &[&new_code().display()], |ready| {
        enroll(&world, &id_of(&ready.payload), PHONE_KEY, "p");
    });
    assert_eq!(result.exit.unwrap(), Exit::Paired);
    let drawing: Vec<&str> = result
        .output
        .lines()
        .skip_while(|line| !line.contains("Scan this with the same phone"))
        .skip(2)
        .take_while(|line| !line.is_empty())
        .collect();
    assert_eq!(decode(&drawing), printed_code(&result.output));
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
fn a_missing_host_key_stops_before_anything_is_asked_or_written() {
    let world = World::new();
    std::fs::remove_file(world.etc.path().join("ssh_host_ed25519_key.pub")).unwrap();
    let script = Script::new(&[&new_code().display()]);
    let result = pair(
        &world,
        &options(),
        &Setup::default(),
        &script,
        &FakeSignals::default(),
        |_| (),
    );
    assert!(
        matches!(result.exit, Err(RunError::HostKey(_))),
        "{:?}",
        result.exit
    );
    assert_eq!(script.asked.load(Ordering::SeqCst), 0);
    assert!(!result.output.contains("or2-pair:2?") && !world.ssh_dir().exists());
}

#[test]
fn another_user_is_refused_before_anything_is_touched() {
    let world = World::new();
    let mut options = options();
    options.user = Some("bob".into());
    let result = pair_default(&world, &options, &[], |_| ());
    match result.exit {
        Err(RunError::UserMismatch {
            requested,
            effective,
        }) => {
            assert_eq!((requested.as_str(), effective.as_str()), ("bob", "alice"));
        }
        other => panic!("{other:?}"),
    }
    assert!(!result.output.contains("Checks"));
}

#[test]
fn an_unwritable_key_file_is_refused_and_leaves_no_state_behind() {
    // The file is a directory: the checks refuse before any prompt.
    let world = World::new();
    std::fs::create_dir_all(world.ssh_dir().join("authorized_keys")).unwrap();
    let script = Script::new(&[&new_code().display()]);
    let result = pair(
        &world,
        &options(),
        &Setup::default(),
        &script,
        &FakeSignals::default(),
        |_| (),
    );
    assert!(
        matches!(result.exit, Err(RunError::Blocked)),
        "{:?}",
        result.exit
    );
    assert!(world.state_files().is_empty());
}
