//! `or2-pair` end to end: checks, ask for the phone's code, add the temporary key, print the
//! QR, wait for the phone, clean up.
//!
//! Everything the run touches comes in through [`Env`] (home directory, interfaces, the sshd
//! probe, the prompt, the clock, the signals), so the whole flow runs against a temporary home in
//! tests. `lib::run_main` builds the real `Env`.

use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use crate::account::{Account, AccountError};
use crate::addresses::{self, Iface, Kind};
use crate::args::Options;
#[cfg(unix)]
use crate::authorized_keys::Backup;
#[cfg(unix)]
use crate::bootstrap;
use crate::bootstrap::PairingId;
use crate::checks::{self, CheckInput, Level, Platform};
#[cfg(unix)]
use crate::code::PairCode;
use crate::date::DateTime;
use crate::hostkey::{self, Keyscan};
use crate::net::Net;
use crate::payload::{self, Payload};
use crate::prompt::CodePrompt;
use crate::qr::{self, QrStyle};

/// The signals the run reacts to (SIGINT, SIGTERM, SIGHUP): a counter that something else
/// increments. The real one is [`OsSignals`]; tests count by hand.
pub trait Signals {
    /// Starts watching; called once, before the temporary key is written (until then the
    /// default action, which ends the process with nothing changed, is what you want).
    fn arm(&self) -> io::Result<()>;
    /// How many signals have arrived since `arm`.
    fn count(&self) -> u32;
}

/// The real signals (Unix); elsewhere nothing is watched.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsSignals;

impl Signals for OsSignals {
    fn arm(&self) -> io::Result<()> {
        #[cfg(unix)]
        {
            crate::signals::install()
        }
        #[cfg(not(unix))]
        {
            Ok(())
        }
    }

    fn count(&self) -> u32 {
        #[cfg(unix)]
        {
            crate::signals::received()
        }
        #[cfg(not(unix))]
        {
            0
        }
    }
}

pub struct Env<'a> {
    pub version: &'static str,
    /// Who this run pairs for: the login shown and shipped in the code, and the home whose
    /// `authorized_keys` is written. One value, so they cannot differ.
    pub account: Account,
    pub hostname: Option<String>,
    pub etc_ssh: PathBuf,
    pub program_dirs: Vec<PathBuf>,
    pub interfaces: Vec<Iface>,
    pub platform: Platform,
    pub net: &'a dyn Net,
    pub keyscan: &'a dyn Keyscan,
    /// This program's canonical path (what sshd is told to run), or why it is unknown.
    pub exe: Result<PathBuf, String>,
    /// Where the code is typed.
    pub prompt: &'a dyn CodePrompt,
    /// Whether there is a person to ask. The binary sets it from "standard input is a terminal";
    /// pairing without one is refused.
    pub can_ask: bool,
    /// Whether the output takes ANSI colours.
    pub color: bool,
    pub random: &'a dyn Fn(&mut [u8]),
    pub now: &'a dyn Fn() -> DateTime,
    pub signals: &'a dyn Signals,
    /// How long the phone has: [`bootstrap::WINDOW`] (5 minutes); tests shorten it.
    pub window: Duration,
    /// How often the wait looks for the phone's result: [`POLL`] (250 ms).
    pub poll: Duration,
    /// Called once the code is printed and the run is waiting (tests use it to find the code).
    pub on_ready: Option<&'a dyn Fn(&Ready)>,
    /// Whether this platform installs keys at all. Where it does not (every target that is not
    /// Unix: there is no implementation that checks owners, links and permissions with handles)
    /// the run behaves like `--manual` whatever was asked: it opens no key file, asks for no
    /// code, prints the code and then exact manual instructions.
    pub install_keys: bool,
}

/// How often the wait looks for `<id>.done`: a local file, no network.
pub const POLL: Duration = Duration::from_millis(250);

/// What a test needs to play the phone.
#[derive(Debug, Clone)]
pub struct Ready {
    pub payload: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("{0}")]
    Account(#[from] AccountError),
    #[error(
        "--user {requested} is not the account this runs as ({effective}): or2-pair authorizes keys only for the user who runs it, in that user's own ~/.ssh. Run it as {requested} (log in or use su), or leave --user out"
    )]
    UserMismatch {
        requested: String,
        effective: String,
    },
    #[error("{0}")]
    HostKey(#[from] hostkey::NoHostKey),
    #[error(
        "no address to give the phone: this host has no LAN, overlay or public address and no host name; pass --address"
    )]
    NoAddress,
    #[error(
        "pairing needs a terminal to type the code in; run or2-pair in a terminal, or use --manual"
    )]
    NotInteractive,
    #[error(
        "automatic pairing is not possible here (the lines marked fail above say why); fix them, or run or2-pair --manual and add the phone's key by hand"
    )]
    Blocked,
    #[error("{0}")]
    TooLong(#[from] payload::TooLong),
    /// The code would not pass the phone's strict parser: nothing was drawn or printed.
    #[error("{0}")]
    InvalidCode(#[from] payload::Invalid),
    #[error("cannot draw the QR code: {0}")]
    Qr(String),
    #[error("cannot write output: {0}")]
    Output(#[from] io::Error),
    #[error("could not add the temporary pairing key: {0}")]
    Install(io::Error),
    #[error("could not record the pairing state in ~/.ssh/or2-pair: {0}")]
    State(io::Error),
}

/// How the run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// A phone's key replaced the temporary key.
    Paired,
    /// `--manual`: the code was printed.
    CodeOnly,
    /// `--check`.
    Checked,
    /// An empty line (or the end of input) at the code prompt: nothing was changed.
    Cancelled,
    TimedOut,
    /// Ctrl-C, SIGTERM or SIGHUP; the temporary key was removed.
    Interrupted,
    /// The temporary key could not be removed (the line to delete was printed).
    Failed,
}

impl Exit {
    /// 0 for a pairing, a printed code or a check; 1 otherwise.
    pub fn code(self) -> i32 {
        match self {
            Self::Paired | Self::CodeOnly | Self::Checked => 0,
            _ => 1,
        }
    }
}

fn heading(out: &mut dyn Write, text: &str) -> io::Result<()> {
    writeln!(out, "\n{text}")
}

/// How many mistyped codes are taken before the run gives up.
#[cfg(unix)]
const TYPO_LIMIT: usize = 10;

pub fn run(options: &Options, env: &Env<'_>, out: &mut dyn Write) -> Result<Exit, RunError> {
    writeln!(
        out,
        "or2-pair {} - pair a phone with this host",
        env.version
    )?;

    // The login in the code, the name in the prompt and the home that is written are all this
    // account's. A flag cannot pick another one (see `account`).
    if let Some(requested) = &options.user
        && !env.account.is_named(requested)
    {
        return Err(RunError::UserMismatch {
            requested: requested.clone(),
            effective: env.account.name.clone(),
        });
    }

    // Where keys are not installed there is nothing to pair automatically.
    let manual = options.manual || !env.install_keys;

    // Fail before doing anything: with nobody to type the code there is no point in going on.
    if !manual && !options.check_only && !env.can_ask {
        return Err(RunError::NotInteractive);
    }

    let ssh_port = options.ssh_port.unwrap_or_else(|| {
        checks::read_sshd_config(&env.etc_ssh)
            .and_then(|config| config.port)
            .unwrap_or(22)
    });
    let probe = env.net.probe_ssh(ssh_port);

    #[cfg(unix)]
    let stale = if options.check_only && env.install_keys {
        crate::pairing::stale_ids(&env.account, (env.now)().to_unix())
    } else {
        Vec::new()
    };
    #[cfg(not(unix))]
    let stale = Vec::new();

    heading(out, "Checks")?;
    let found = checks::run(&CheckInput {
        account: &env.account,
        ssh_port,
        probe: &probe,
        etc_ssh: &env.etc_ssh,
        exe: &env.exe,
        program_dirs: &env.program_dirs,
        platform: env.platform,
        pairing: !manual,
        manual_keys: !env.install_keys,
        stale: &stale,
    });
    for check in &found {
        writeln!(out, "  {}  {}", check.level.tag(), check.text)?;
    }
    if options.check_only {
        let warnings = found
            .iter()
            .filter(|check| matches!(check.level, Level::Warn | Level::Fail))
            .count();
        writeln!(out, "\n{warnings} warning(s). Nothing was changed.")?;
        return Ok(Exit::Checked);
    }
    if !manual && found.iter().any(|check| check.level == Level::Fail) {
        return Err(RunError::Blocked);
    }

    let user = env.account.name.clone();
    let name = options
        .name
        .clone()
        .or_else(|| env.hostname.clone())
        .map(|name| {
            name.trim()
                .chars()
                .filter(|c| !c.is_control())
                .take(64)
                .collect::<String>()
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "host".to_owned());
    let host_key = hostkey::read_host_key(&env.etc_ssh, env.keyscan, ssh_port)?;
    let addresses = addresses::gather(&env.interfaces, env.hostname.as_deref(), &options.addresses);
    if addresses.is_empty() {
        return Err(RunError::NoAddress);
    }

    let id = (!manual).then(|| PairingId::generate(env.random));
    let mut payload = Payload {
        name: name.clone(),
        user: user.clone(),
        port: ssh_port,
        addresses: addresses.iter().map(|a| a.text.clone()).collect(),
        host_key: host_key.key.openssh(),
        id: id.clone(),
    };
    let dropped = payload.fit()?;
    // Whatever is printed from here on, the phone accepts: refuse here, before anything is
    // asked or written, rather than show a code it would turn away.
    payload.validate()?;
    let text = payload.encode();

    let host = Host {
        name: &name,
        user: &user,
        ssh_port,
        host_key: &host_key,
        addresses: &addresses,
        payload: &payload,
        dropped: &dropped,
    };
    let Some(id) = id else {
        print_host(out, &host)?;
        print_code(out, &text, options, env)?;
        if env.install_keys {
            writeln!(
                out,
                "\n--manual: nothing was changed on this host. After scanning, the phone saves the host with its key trusted and shows its public key: add that line to {}.",
                env.account.keys_path().display()
            )?;
        } else {
            out.write_all(manual_instructions(env.platform, &user, options.manual).as_bytes())?;
        }
        return Ok(Exit::CodeOnly);
    };

    #[cfg(unix)]
    {
        // The checks already refused everything that fails here; these are the values.
        let dialect = bootstrap::dialect(probe.as_ref().ok().map(String::as_str))
            .map_err(|_| RunError::Blocked)?;
        let exe = env
            .exe
            .as_ref()
            .ok()
            .and_then(|path| bootstrap::check_exe_path(path).ok())
            .ok_or(RunError::Blocked)?;
        pair(options, env, out, &host, id, &text, dialect, &exe)
    }
    #[cfg(not(unix))]
    {
        let _ = (id, text);
        Err(RunError::Blocked)
    }
}

/// What is shown about this host.
struct Host<'a> {
    name: &'a str,
    user: &'a str,
    ssh_port: u16,
    host_key: &'a hostkey::HostKeyFound,
    addresses: &'a [addresses::Address],
    payload: &'a Payload,
    dropped: &'a [String],
}

/// The pairing itself: ask for the code, add the temporary key, show the QR, wait, clean up.
#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
fn pair(
    options: &Options,
    env: &Env<'_>,
    out: &mut dyn Write,
    host: &Host<'_>,
    id: PairingId,
    text: &str,
    dialect: bootstrap::Dialect,
    exe: &str,
) -> Result<Exit, RunError> {
    use crate::pairing::{self, Ended, Live};

    writeln!(out, "\nOpen or2 on your phone: Add host > Easy pair.")?;
    let mut mistakes = 0;
    let code = loop {
        write!(out, "Code shown on your phone: ")?;
        out.flush()?;
        let Some(line) = env.prompt.read_line() else {
            writeln!(out, "\nNo code was typed. Nothing was changed.")?;
            return Ok(Exit::Cancelled);
        };
        if line.trim().is_empty() {
            writeln!(out, "Cancelled. Nothing was changed.")?;
            return Ok(Exit::Cancelled);
        }
        match PairCode::parse(&line) {
            Ok(code) => break code,
            Err(error) => {
                mistakes += 1;
                if mistakes >= TYPO_LIMIT {
                    writeln!(out, "{error}. Too many tries; nothing was changed.")?;
                    return Ok(Exit::Cancelled);
                }
                writeln!(out, "{error}. Type it again, or press Enter to cancel.")?;
            }
        }
    };

    // From here the signals are counted instead of killing the process: nothing is written
    // before this, so the default action was fine until now.
    if let Err(error) = env.signals.arm() {
        writeln!(
            out,
            "  warn  could not watch for Ctrl-C ({error}); the next run removes a leftover key"
        )?;
    }
    let now = (env.now)();
    let backup = Backup::new(now);
    for problem in pairing::sweep(&env.account, &backup, now.to_unix()) {
        writeln!(out, "  warn  {problem}")?;
    }
    let mut live = Live::start(
        &env.account,
        &backup,
        &code,
        id,
        dialect,
        exe,
        now,
        env.window,
    )?;
    drop(code);

    print_host(out, host)?;
    let until = live.deadline_clock();
    writeln!(
        out,
        "\nA temporary pairing key was added for {} until {until}. Scan this with the same phone:",
        host.user
    )?;
    print_code(out, text, options, env)?;
    writeln!(
        out,
        "\nWaiting for the phone (until {until}). Ctrl-C removes the temporary key."
    )?;
    out.flush()?;
    if let Some(ready) = env.on_ready {
        ready(&Ready {
            payload: text.to_owned(),
        });
    }

    let ended = live.wait(env.signals, env.poll, env.window);
    let line = live.line().to_owned();
    drop(live);
    let keys = env.account.keys_path();
    let mut report = String::new();
    let exit = match ended {
        Ended::Paired(done) => {
            let (device, fingerprint) = done
                .map(|done| (done.device, done.fingerprint))
                .unwrap_or_else(|| ("the phone".to_owned(), "an unknown key".to_owned()));
            let _ = writeln!(
                report,
                "Paired \"{device}\" ({fingerprint}) as {}. The temporary key was replaced by the phone's key.",
                host.user
            );
            let _ = writeln!(
                report,
                "To undo, delete the line ending or2-{device}-{} in {}.",
                (env.now)().date(),
                keys.display()
            );
            Exit::Paired
        }
        Ended::TimedOut => {
            let _ = writeln!(
                report,
                "Timed out: no phone paired in time. The temporary key was removed. Run or2-pair again to retry."
            );
            Exit::TimedOut
        }
        Ended::Interrupted => {
            let _ = writeln!(report, "Cancelled. The temporary key was removed.");
            Exit::Interrupted
        }
        Ended::RemovalFailed(error) => {
            let _ = writeln!(
                report,
                "Could not remove the temporary pairing key: {error}\nDelete this line from {} (it can no longer be used: the pairing command refuses once the run is over; the next or2-pair also removes it):\n{line}",
                keys.display()
            );
            Exit::Failed
        }
    };
    if let Some(path) = backup.path() {
        let _ = writeln!(report, "The previous file is saved as {}.", path.display());
    }
    writeln!(out)?;
    out.write_all(report.as_bytes())?;
    Ok(exit)
}

fn print_host(out: &mut dyn Write, host: &Host<'_>) -> io::Result<()> {
    heading(out, "This host")?;
    writeln!(
        out,
        "  name       {}      user  {}      ssh port  {}",
        host.name, host.user, host.ssh_port
    )?;
    writeln!(
        out,
        "  host key   {} {}  ({})",
        host.host_key.key.algorithm(),
        host.host_key.key.fingerprint(),
        host.host_key.source
    )?;
    let shown: Vec<String> = host
        .addresses
        .iter()
        .filter(|address| host.payload.addresses.contains(&address.text))
        .map(|address| match address.kind {
            Kind::Mdns => format!("{} (mDNS name, same network only)", address.text),
            kind => format!("{} ({})", address.text, kind.label()),
        })
        .collect();
    writeln!(out, "  addresses  {}", shown.join(", "))?;
    if !host.dropped.is_empty() {
        writeln!(
            out,
            "  note: {} address(es) were left out, the last ones first: the phone takes at most {} addresses and a code of at most {} bytes",
            host.dropped.len(),
            payload::MAX_ADDRESSES,
            payload::MAX_BYTES
        )?;
    }
    Ok(())
}

/// The QR and the same code as text.
fn print_code(
    out: &mut dyn Write,
    text: &str,
    options: &Options,
    env: &Env<'_>,
) -> Result<(), RunError> {
    let style = QrStyle {
        ascii: options.ascii,
        color: env.color && !options.no_color,
        invert: options.invert,
    };
    writeln!(out)?;
    let drawing = qr::render(text, style).map_err(|error| RunError::Qr(error.to_string()))?;
    out.write_all(drawing.as_bytes())?;
    if options.ascii {
        let (width, _) = qr::modules(text).map_err(|error| RunError::Qr(error.to_string()))?;
        writeln!(
            out,
            "  (this drawing is {} columns wide)",
            qr::columns(width, style)
        )?;
    }
    writeln!(
        out,
        "\nOr paste this code into the app (Easy pair > Paste pairing code):"
    )?;
    writeln!(out, "{text}")?;
    Ok(())
}

/// What to do by hand where `or2-pair` installs no key. Only the login is used (never a home or
/// profile directory looked up on the machine): the paths are the ones the platform's SSH server
/// documents.
fn manual_instructions(platform: Platform, user: &str, asked_for_manual: bool) -> String {
    let mut text = String::from("\n");
    if asked_for_manual {
        text.push_str("--manual: nothing was changed on this host.\n");
    } else {
        text.push_str(
            "or2-pair does not pair automatically on this platform: it would have to add a temporary key to authorized_keys, and it has no safe way to check that file's owner, links and permissions here. It changed nothing.\n",
        );
    }
    text.push_str(
        "After scanning, the app shows its public key (one line). Add it to the SSH server's authorized keys by hand:\n",
    );
    if platform == Platform::Windows {
        let _ = writeln!(
            text,
            "  - ordinary account:  C:\\Users\\{user}\\.ssh\\authorized_keys  (create the .ssh folder and the file if they are missing)\n  - member of the Administrators group: C:\\ProgramData\\ssh\\administrators_authorized_keys instead (OpenSSH for Windows ignores the per-user file for administrators); then restrict it:\n      icacls \"C:\\ProgramData\\ssh\\administrators_authorized_keys\" /inheritance:r /grant \"Administrators:F\" /grant \"SYSTEM:F\""
        );
    } else {
        text.push_str(
            "  ~/.ssh/authorized_keys of the account the phone connects as (mode 600, with ~/.ssh mode 700)\n",
        );
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes() {
        for (exit, code) in [
            (Exit::Paired, 0),
            (Exit::CodeOnly, 0),
            (Exit::Checked, 0),
            (Exit::Cancelled, 1),
            (Exit::TimedOut, 1),
            (Exit::Interrupted, 1),
            (Exit::Failed, 1),
        ] {
            assert_eq!(exit.code(), code, "{exit:?}");
        }
    }

    #[test]
    fn manual_instructions_name_the_windows_files() {
        let text = manual_instructions(Platform::Windows, "dev", false);
        assert!(text.contains("C:\\Users\\dev\\.ssh\\authorized_keys") && text.contains("icacls"));
        assert!(text.contains("does not pair automatically"));
        let unix = manual_instructions(Platform::Linux, "dev", true);
        assert!(unix.contains("--manual") && unix.contains("~/.ssh/authorized_keys"));
    }
}
