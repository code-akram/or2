//! `or2-pair`: pair a phone with this host in one command and one QR scan.
//!
//! A host-side tool, deliberately separate from `or2-core` (which carries the app's SSH, mosh
//! and terminal stack and needs none of it here). Pairing runs over the host's own sshd: the
//! person types the code `K` shown on the phone, `or2-pair` adds a temporary key derived from it
//! to `~/.ssh/authorized_keys`, prints a QR with nothing secret in it, and the phone logs in with
//! the temporary key and hands over its own public key. See `docs/contracts.md`, "Easy pair",
//! and `docs/pairing.md`.
//!
//! - [`checks`]: report sshd (and its version), `authorized_keys`, `sshd_config`, the login shell
//!   and the program's path, tmux/herdr/mosh-server and firewall hints,
//! - [`hints`]: the exact fix of what is missing or failing, for this host,
//! - [`addresses`], [`hostkey`]: what the phone needs to reach and recognise this host,
//! - [`payload`], [`qr`]: the pairing code (QR text) and its terminal drawing,
//! - [`code`], [`bootstrap`], [`prompt`]: the code `K`, the key and `authorized_keys` line derived
//!   from it, and where it is typed,
//! - [`authorized_keys`], [`state`], [`safefs`]: the checked-handle file access (Unix),
//! - [`pairing`], [`exchange`], [`signals`]: the live run and the forced command `or2-pair enroll`
//!   (Unix),
//! - [`net`]: the one socket, behind a small trait,
//! - [`rail`]: how all of it is drawn: one clack-style rail, colour and glyphs as the terminal allows,
//! - [`run`]: the whole flow, with its environment injected.

pub mod account;
pub mod addresses;
pub mod args;
pub mod authorized_keys;
pub mod bootstrap;
pub mod checks;
pub mod code;
pub mod date;
#[cfg(unix)]
pub mod exchange;
pub mod hints;
pub mod hostkey;
pub mod keyline;
pub mod net;
#[cfg(unix)]
pub mod pairing;
pub mod payload;
pub mod prompt;
pub mod qr;
pub mod rail;
pub mod run;
#[cfg(unix)]
pub mod safefs;
#[cfg(unix)]
pub mod signals;
#[cfg(unix)]
pub mod state;

use std::ffi::OsString;
use std::process::ExitCode;

use crate::account::{Account, AccountError};
use crate::addresses::Iface;
use crate::checks::Platform;
use crate::date::DateTime;
use crate::hostkey::SystemKeyscan;
use crate::net::StdNet;
use crate::prompt::Stdin;
use crate::rail::{Rail, Stream, Style};
use crate::run::{Env, Exit, OsSignals, RunError};

/// The operating system's interface addresses (IPv4 and IPv6; the classification is in
/// [`addresses`]).
pub fn system_interfaces() -> Vec<Iface> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|iface| Iface {
            name: iface.name.clone(),
            ip: iface.ip(),
        })
        .collect()
}

/// What a `test-support` build (never the installed binary) reads from the environment so that
/// the tests of the built binary can point it at throwaway files: `OR2_PAIR_TEST_HOME` and
/// `OR2_PAIR_TEST_USER` (the account), `OR2_PAIR_TEST_AUTHORIZED_KEYS` (a key file elsewhere than
/// `<home>/.ssh/authorized_keys`: a disposable sshd's `AuthorizedKeysFile`),
/// `OR2_PAIR_TEST_ETC_SSH` (where the host key and `sshd_config` are read) and
/// `OR2_PAIR_TEST_WINDOW_SECS` (the pairing window).
#[cfg(feature = "test-support")]
fn test_var(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

/// Who this process pairs for: the effective user, from the account database (Unix). A build
/// with the `test-support` feature lets the tests point it at a throwaway account.
///
/// Elsewhere there is no account lookup (no key is installed there): the login is the one given
/// with `--user`, and nothing else about the account is guessed.
fn resolve_account(options: &args::Options) -> Result<Account, AccountError> {
    #[cfg(feature = "test-support")]
    if let (Some(user), Some(home)) = (
        test_var("OR2_PAIR_TEST_USER"),
        test_var("OR2_PAIR_TEST_HOME"),
    ) {
        let account = Account::new(user.to_string_lossy(), home);
        return Ok(match test_var("OR2_PAIR_TEST_AUTHORIZED_KEYS") {
            Some(file) => account.with_keys_file(file),
            None => account,
        });
    }
    #[cfg(unix)]
    {
        let _ = options;
        Account::current()
    }
    #[cfg(not(unix))]
    {
        options
            .user
            .as_deref()
            .map(Account::login_only)
            .ok_or_else(|| {
                AccountError::Unknown(
                    "on this platform or2-pair cannot look up the account; say which login the phone should use with --user <login>".into(),
                )
            })
    }
}

/// Runs the tool for real: this process's environment, the system's sockets, the terminal.
/// `code_from_stdin` (the `test-support` host only) takes the code from a pipe instead of
/// insisting on a terminal.
pub fn run_main(options: &args::Options, code_from_stdin: bool) -> Result<Exit, RunError> {
    let style = Style::detect(Stream::Stdout);
    let account = match resolve_account(options) {
        Ok(account) => account,
        Err(error) => {
            // The rail opens before the error that ends it.
            let rail = Rail::new(style.with_flags(options.no_color, options.ascii));
            run::open(
                &rail,
                &mut std::io::stdout().lock(),
                env!("CARGO_PKG_VERSION"),
                options,
            )?;
            return Err(error.into());
        }
    };
    let path: Option<OsString> = std::env::var_os("PATH");
    let hostname = gethostname::gethostname().to_string_lossy().into_owned();
    let random = |buf: &mut [u8]| {
        use rand::Rng;
        rand::rng().fill_bytes(buf);
    };
    let now = DateTime::now;
    #[allow(unused_mut)]
    let mut etc_ssh = hostkey::default_etc_ssh();
    #[allow(unused_mut)]
    let mut window = bootstrap::WINDOW;
    #[cfg(feature = "test-support")]
    {
        if let Some(dir) = test_var("OR2_PAIR_TEST_ETC_SSH") {
            etc_ssh = dir.into();
        }
        if let Some(seconds) = test_var("OR2_PAIR_TEST_WINDOW_SECS")
            .and_then(|value| value.to_str().and_then(|text| text.parse::<u64>().ok()))
        {
            window = std::time::Duration::from_secs(seconds);
        }
    }
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|error| error.to_string());
    let program_dirs = checks::program_dirs(path.as_deref(), &account.home);
    let facts = hints::HostFacts::detect(
        Platform::current(),
        std::path::Path::new("/"),
        &program_dirs,
        cfg!(unix) && account.uid == 0,
        &hints::SystemCommands::default(),
    );
    let env = Env {
        version: env!("CARGO_PKG_VERSION"),
        program_dirs,
        facts,
        account,
        hostname: Some(hostname),
        etc_ssh,
        interfaces: system_interfaces(),
        platform: Platform::current(),
        net: &StdNet,
        keyscan: &SystemKeyscan,
        shell: &checks::SystemShell,
        exe,
        prompt: &Stdin,
        can_ask: code_from_stdin || Stdin::available(),
        style,
        random: &random,
        now: &now,
        signals: &OsSignals,
        window,
        poll: run::POLL,
        on_ready: None,
        install_keys: cfg!(unix),
    };
    run::run(options, &env, &mut std::io::stdout().lock())
}

/// The forced command: `or2-pair enroll <id>`, with the phone's exec channel as standard input
/// and output. Exit 0 when the phone's key was installed, 1 otherwise.
#[cfg(unix)]
pub fn enroll_main(id: &bootstrap::PairingId) -> ExitCode {
    let account = match resolve_account(&args::Options::default()) {
        Ok(account) => account,
        Err(error) => {
            eprintln!("or2-pair: {error}");
            return ExitCode::FAILURE;
        }
    };
    let now = DateTime::now;
    let env = exchange::Enroll {
        account: &account,
        now: &now,
        request_timeout: exchange::REQUEST_TIMEOUT,
        hook: None,
    };
    let outcome = exchange::enroll(
        id,
        &env,
        Box::new(std::io::stdin()),
        &mut std::io::stdout().lock(),
    );
    match outcome {
        exchange::Outcome::Installed { .. } => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}

/// The whole command line, for both binaries (`or2-pair` and the `test-support` host).
pub fn cli(args: impl IntoIterator<Item = String>, code_from_stdin: bool) -> ExitCode {
    let options = match args::parse(args) {
        Ok(args::Parsed::Run(options)) => options,
        Ok(args::Parsed::Help) => {
            print!("{}", args::HELP);
            return ExitCode::SUCCESS;
        }
        Ok(args::Parsed::Version) => {
            println!("or2-pair {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        #[cfg(unix)]
        Ok(args::Parsed::Enroll(id)) => return enroll_main(&id),
        #[cfg(not(unix))]
        Ok(args::Parsed::Enroll(_)) => {
            eprintln!("or2-pair: enroll is not available on this platform");
            return ExitCode::from(2);
        }
        Err(error) => {
            eprintln!("or2-pair: {error}");
            return ExitCode::from(2);
        }
    };
    match run_main(&options, code_from_stdin) {
        Ok(exit) => ExitCode::from(u8::try_from(exit.code()).unwrap_or(1)),
        Err(error) => {
            // The rail went to standard output: all of it first, then the error that ends it.
            let _ = std::io::Write::flush(&mut std::io::stdout());
            let rail = Rail::new(
                Style::detect(Stream::Stderr).with_flags(options.no_color, options.ascii),
            );
            let _ = run::report_error(&rail, &mut std::io::stderr().lock(), &error);
            ExitCode::FAILURE
        }
    }
}
