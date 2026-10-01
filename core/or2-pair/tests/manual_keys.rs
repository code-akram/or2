//! Where `or2-pair` does not install keys (every target that is not Unix, until a fully
//! implemented and tested path exists there): it behaves like `--no-listen`, binds no socket,
//! touches no file and prints exact manual instructions. Run here on Linux with
//! `Env::install_keys` off, which is what a Windows build sets.

mod common;

use std::io;
use std::net::IpAddr;
use std::time::Duration;

use common::*;
use or2_pair::account::Account;
use or2_pair::checks::Platform;
use or2_pair::date::DateTime;
use or2_pair::net::{Net, PairListener};
use or2_pair::run::{Env, Exit, RunError, run};

/// A network that must not be asked to listen; probing sshd finds nothing.
struct NoListening;

impl Net for NoListening {
    fn probe_ssh(&self, _: u16) -> io::Result<String> {
        Err(io::ErrorKind::ConnectionRefused.into())
    }
    fn listen(&self, _: &[IpAddr], _: u16) -> io::Result<Box<dyn PairListener>> {
        panic!("a host that installs no keys must not listen");
    }
}

fn manual(
    world: &World,
    platform: Platform,
    account: Account,
    options: &or2_pair::args::Options,
) -> (Result<Exit, RunError>, String) {
    let confirm = Auto::new(or2_pair::confirm::Answer::Yes);
    let random = |buf: &mut [u8]| buf.fill(9);
    let now = || DateTime::from_unix(1_782_867_661);
    let env = Env {
        version: "test",
        account,
        hostname: Some("testhost.example.net".into()),
        etc_ssh: world.etc.path().to_path_buf(),
        program_dirs: Vec::new(),
        interfaces: interfaces(),
        platform,
        net: &NoListening,
        keyscan: &NoKeyscan,
        confirm: &confirm,
        can_ask: true,
        color: false,
        random: &random,
        now: &now,
        window: Duration::from_secs(120),
        on_ready: None,
        install_keys: false,
    };
    let mut out = Vec::new();
    let exit = run(options, &env, &mut out);
    assert!(confirm.asked.lock().unwrap().is_empty());
    (exit, String::from_utf8(out).unwrap())
}

#[test]
fn without_key_installation_a_listening_request_prints_the_code_and_instructions_only() {
    let world = World::new();
    // The home the account carries is not even looked at; nothing may appear in it.
    let account = Account::login_only("alice");
    let (exit, output) = manual(&world, Platform::Windows, account, &options());
    assert_eq!(exit.unwrap(), Exit::CodeOnly, "{output}");
    assert!(output.contains("or2-pair:1?"), "{output}");
    assert!(
        !output.contains("&otp=") && !output.contains("&pair="),
        "{output}"
    );
    assert!(
        output.contains("does not listen") && output.contains("authorized_keys"),
        "{output}"
    );
    // Both Windows OpenSSH cases, spelled out.
    assert!(
        output.contains(r"C:\Users\alice\.ssh\authorized_keys"),
        "{output}"
    );
    assert!(
        output.contains(r"C:\ProgramData\ssh\administrators_authorized_keys")
            && output.contains("icacls"),
        "{output}"
    );
    assert!(world.authorized_keys().is_none());
    assert!(!world.home.path().join(".ssh").exists());
}

#[test]
fn no_listen_gives_the_same_instructions() {
    let world = World::new();
    let mut options = options();
    options.no_listen = true;
    let account = Account::login_only("alice");
    let (exit, output) = manual(&world, Platform::Windows, account, &options);
    assert_eq!(exit.unwrap(), Exit::CodeOnly, "{output}");
    assert!(
        output.contains(r"C:\ProgramData\ssh\administrators_authorized_keys"),
        "{output}"
    );
}

#[test]
fn another_platform_gets_generic_instructions() {
    let world = World::new();
    let account = Account::login_only("alice");
    let (exit, output) = manual(&world, Platform::Other, account, &options());
    assert_eq!(exit.unwrap(), Exit::CodeOnly, "{output}");
    assert!(output.contains("does not listen"), "{output}");
    assert!(
        !output.contains("administrators_authorized_keys"),
        "{output}"
    );
}

#[test]
fn the_check_does_not_inspect_or_open_any_key_file() {
    let world = World::new();
    let mut options = options();
    options.check_only = true;
    let account = Account::login_only("alice");
    let (exit, output) = manual(&world, Platform::Windows, account, &options);
    assert_eq!(exit.unwrap(), Exit::Checked, "{output}");
    assert!(
        output.contains("not checked") && output.contains("by hand"),
        "{output}"
    );
    assert!(!world.home.path().join(".ssh").exists());
}

#[test]
fn a_login_only_account_has_no_home_to_write_to() {
    let account = Account::login_only("alice");
    assert_eq!(account.name, "alice");
    assert!(account.home.as_os_str().is_empty());
    // Whatever the platform, nothing can be installed for it.
    let key = or2_pair::keyline::KeyLine::parse(
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBCJz8goXA2qGjRTNHhOwsljhOuCXG/+2B/zTJH/brc5",
    )
    .unwrap();
    assert!(
        or2_pair::authorized_keys::add(&account, &key, "phone", DateTime::from_unix(0)).is_err()
    );
}

#[test]
fn nothing_in_the_sources_resolves_an_account_from_the_windows_environment() {
    // The login and profile of a Windows process are plain environment variables, set by whoever
    // launched it: never a basis for deciding whose `authorized_keys` to write.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|extension| extension == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            for variable in ["USERNAME", "USERPROFILE", "HOMEDRIVE", "HOMEPATH"] {
                assert!(
                    !text.contains(variable),
                    "{} mentions {variable}",
                    path.display()
                );
            }
        }
    }
}
