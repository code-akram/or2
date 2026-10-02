//! Where `or2-pair` does not install keys (every target that is not Unix, until a fully
//! implemented and tested path exists there): it behaves like `--manual`, asks for no code,
//! touches no file and prints exact manual instructions. Run here on Linux with
//! `Env::install_keys` off, which is what a Windows build sets.
#![cfg(unix)]

mod common;

use std::sync::atomic::Ordering;

use common::*;
use or2_pair::account::Account;
use or2_pair::checks::Platform;
use or2_pair::run::{Env, Exit, RunError, run};

fn manual(
    world: &World,
    platform: Platform,
    account: Account,
    options: &or2_pair::args::Options,
) -> (Result<Exit, RunError>, String, u32) {
    let script = Script::new(&["7KQ4-M2XD-9PTM"]);
    let random = |buf: &mut [u8]| buf.fill(9);
    let net = FakeNet(Err(std::io::ErrorKind::ConnectionRefused));
    let signals = FakeSignals::default();
    let env = Env {
        version: "test",
        account,
        hostname: Some("testhost.example.net".into()),
        etc_ssh: world.etc.path().to_path_buf(),
        program_dirs: Vec::new(),
        interfaces: interfaces(),
        platform,
        net: &net,
        keyscan: &NoKeyscan,
        exe: Ok(EXE.into()),
        prompt: &script,
        can_ask: true,
        color: false,
        random: &random,
        now: &|| or2_pair::date::DateTime::from_unix(1_782_867_661),
        signals: &signals,
        window: or2_pair::bootstrap::WINDOW,
        poll: or2_pair::run::POLL,
        on_ready: None,
        install_keys: false,
    };
    let mut out = Vec::new();
    let exit = run(options, &env, &mut out);
    assert_eq!(
        signals.armed.load(Ordering::SeqCst),
        0,
        "no signal handler either"
    );
    (
        exit,
        String::from_utf8(out).unwrap(),
        script.asked.load(Ordering::SeqCst),
    )
}

#[test]
fn without_key_installation_a_pairing_request_prints_the_code_and_instructions_only() {
    let world = World::new();
    // The home the account carries is not even looked at; nothing may appear in it.
    let account = Account::login_only("alice");
    let (exit, output, asked) = manual(&world, Platform::Windows, account, &options());
    assert_eq!(exit.unwrap(), Exit::CodeOnly, "{output}");
    assert_eq!(asked, 0, "no code is asked for");
    let code = parse_code(&printed_code(&output));
    assert!(code.id.is_none(), "no pairing id: nothing can pair");
    assert!(
        output.contains("does not pair automatically") && output.contains("authorized_keys"),
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
    assert!(!world.ssh_dir().exists());
}

#[test]
fn manual_gives_the_same_instructions() {
    let world = World::new();
    let mut options = options();
    options.manual = true;
    let account = Account::login_only("alice");
    let (exit, output, asked) = manual(&world, Platform::Windows, account, &options);
    assert_eq!(exit.unwrap(), Exit::CodeOnly, "{output}");
    assert_eq!(asked, 0);
    assert!(
        output.contains(r"C:\ProgramData\ssh\administrators_authorized_keys")
            && output.contains("--manual"),
        "{output}"
    );
}

#[test]
fn another_platform_gets_generic_instructions() {
    let world = World::new();
    let account = Account::login_only("alice");
    let (exit, output, _) = manual(&world, Platform::Other, account, &options());
    assert_eq!(exit.unwrap(), Exit::CodeOnly, "{output}");
    assert!(output.contains("does not pair automatically"), "{output}");
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
    let (exit, output, _) = manual(&world, Platform::Windows, account, &options);
    assert_eq!(exit.unwrap(), Exit::Checked, "{output}");
    assert!(
        output.contains("not checked") && output.contains("by hand"),
        "{output}"
    );
    assert!(!world.ssh_dir().exists());
}

#[test]
fn a_login_only_account_has_no_home_to_write_to() {
    let account = Account::login_only("alice");
    assert_eq!(account.name, "alice");
    assert!(account.home.as_os_str().is_empty());
    // Whatever the platform, nothing can be installed for it.
    let backup = or2_pair::authorized_keys::Backup::new(or2_pair::date::DateTime::from_unix(0));
    assert!(or2_pair::authorized_keys::append(&account, &backup, "x").is_err());
    assert!(
        or2_pair::authorized_keys::remove(&account, "SHA256:x", None).is_err(),
        "even a removal needs a home"
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
