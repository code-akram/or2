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
use crate::rail::{Mark, Rail, Style};

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
    /// What the host has, for the exact fix of what is missing ([`crate::hints`]).
    pub facts: crate::hints::HostFacts,
    pub net: &'a dyn Net,
    pub keyscan: &'a dyn Keyscan,
    /// Runs the login shell for the checks ([`checks::SystemShell`]).
    pub shell: &'a dyn checks::ShellProbe,
    /// This program's canonical path (what sshd is told to run), or why it is unknown.
    pub exe: Result<PathBuf, String>,
    /// Where the code is typed.
    pub prompt: &'a dyn CodePrompt,
    /// Whether there is a person to ask. The binary sets it from "standard input is a terminal";
    /// pairing without one is refused.
    pub can_ask: bool,
    /// How the output is drawn ([`crate::rail`]): colours, glyphs, width, redraws. `--no-color`
    /// and `--ascii` are applied on top.
    pub style: Style,
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
        "--user {requested} is not the account this runs as ({effective})\nor2-pair authorizes keys only for the user who runs it, in that user's own ~/.ssh. Run it as {requested} (log in or use su), or leave --user out"
    )]
    UserMismatch {
        requested: String,
        effective: String,
    },
    #[error("{0}")]
    HostKey(#[from] hostkey::NoHostKey),
    #[error(
        "no address to give the phone: this host has no LAN, overlay or public address and no host name\npass --address"
    )]
    NoAddress,
    #[error(
        "pairing needs a terminal to type the code in\nrun or2-pair in a terminal, or use --manual"
    )]
    NotInteractive,
    #[error(
        "automatic pairing is not possible here: the failed checks above say why\nfix them, or run or2-pair --manual and add the phone's key by hand"
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

/// The rail's first line: the program, its version and what this run does.
pub fn open(rail: &Rail, out: &mut dyn Write, version: &str, options: &Options) -> io::Result<()> {
    let what = if options.check_only {
        "check this host for pairing"
    } else if options.manual {
        "pairing code only"
    } else {
        "pair a phone with this host"
    };
    rail.open(out, &format!("or2-pair {version}"), what)
}

/// The end of a run that stopped on `error`: the error, and the rail closed.
pub fn report_error(rail: &Rail, out: &mut dyn Write, error: &RunError) -> io::Result<()> {
    rail.gap(out)?;
    rail.step(out, Mark::Error, &error.to_string())?;
    rail.gap(out)?;
    rail.close(out, "Failed")
}

/// How a check is drawn.
fn mark(level: Level) -> Mark {
    match level {
        Level::Ok => Mark::Ok,
        Level::Info => Mark::Info,
        Level::Warn => Mark::Warn,
        Level::Fail => Mark::Error,
    }
}

/// How many mistyped codes are taken before the run gives up.
#[cfg(unix)]
const TYPO_LIMIT: usize = 10;

/// The question the code is typed at.
#[cfg(unix)]
const QUESTION: &str = "Code shown on your phone";

pub fn run(options: &Options, env: &Env<'_>, out: &mut dyn Write) -> Result<Exit, RunError> {
    let rail = Rail::new(env.style.with_flags(options.no_color, options.ascii));
    open(&rail, out, env.version, options)?;

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

    rail.gap(out)?;
    let found = checks::run(&CheckInput {
        account: &env.account,
        ssh_port,
        probe: &probe,
        etc_ssh: &env.etc_ssh,
        exe: &env.exe,
        program_dirs: &env.program_dirs,
        platform: env.platform,
        facts: &env.facts,
        pairing: !manual,
        manual_keys: !env.install_keys,
        stale: &stale,
        version: env.version,
        shell: env.shell,
    });
    for check in &found {
        rail.step(out, mark(check.level), &check.text)?;
    }
    if options.check_only {
        let warnings = found
            .iter()
            .filter(|check| matches!(check.level, Level::Warn | Level::Fail))
            .count();
        let (mark, counted) = match warnings {
            0 => (Mark::Ok, "No warnings".to_owned()),
            1 => (Mark::Warn, "1 warning".to_owned()),
            n => (Mark::Warn, format!("{n} warnings")),
        };
        rail.gap(out)?;
        rail.step(out, mark, &format!("{counted}. Nothing was changed."))?;
        rail.gap(out)?;
        rail.close(out, "Done")?;
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
        print_host(&rail, out, &host)?;
        rail.gap(out)?;
        rail.step(
            out,
            Mark::Info,
            "Scan this with or2 on your phone (Add host > Easy pair):",
        )?;
        print_code(&rail, out, &text, options)?;
        rail.gap(out)?;
        if env.install_keys {
            rail.step(
                out,
                Mark::Info,
                &format!(
                    "--manual: nothing was changed on this host\nAfter scanning, the phone saves the host with its key trusted and shows its public key: add that line to {}.",
                    env.account.keys_path().display()
                ),
            )?;
        } else {
            rail.step(
                out,
                Mark::Info,
                &manual_instructions(env.platform, &user, options.manual),
            )?;
        }
        rail.gap(out)?;
        rail.close(out, "Done")?;
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
        pair(options, env, &rail, out, &host, id, &text, dialect, &exe)
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
    rail: &Rail,
    out: &mut dyn Write,
    host: &Host<'_>,
    id: PairingId,
    text: &str,
    dialect: bootstrap::Dialect,
    exe: &str,
) -> Result<Exit, RunError> {
    use crate::pairing::{self, Live};

    rail.gap(out)?;
    rail.step(
        out,
        Mark::Info,
        "Open or2 on your phone: Add host > Easy pair",
    )?;
    rail.gap(out)?;
    // Ctrl-C while the code is typed ends the process (nothing is written yet): the rail still
    // ends.
    env.prompt
        .on_ending_signal(&rail.ending_note("Cancelled. Nothing was changed.", "Cancelled"));
    let mut mistakes = 0;
    let code = loop {
        rail.ask(out, QUESTION, "hidden as you type; Enter when done")?;
        let line = env.prompt.read_line();
        // The terminal does not echo the code, not even the Enter that ended it: the answer's
        // line is drawn here, with the code masked.
        let moved = env.prompt.was_stopped();
        let Some(line) = line.filter(|line| !line.trim().is_empty()) else {
            rail.answer(
                out,
                Mark::Error,
                QUESTION,
                "No code was typed. Nothing was changed.",
                moved,
            )?;
            rail.gap(out)?;
            rail.close(out, "Cancelled")?;
            return Ok(Exit::Cancelled);
        };
        match PairCode::parse(&line) {
            Ok(code) => {
                rail.answer(out, Mark::Answered, QUESTION, &rail.mask(&line), moved)?;
                break code;
            }
            Err(error) => {
                mistakes += 1;
                rail.answer(out, Mark::Warn, QUESTION, &rail.mask(&line), moved)?;
                if mistakes >= TYPO_LIMIT {
                    rail.detail(
                        out,
                        &format!("{error}. Too many tries; nothing was changed."),
                    )?;
                    rail.gap(out)?;
                    rail.close(out, "Cancelled")?;
                    return Ok(Exit::Cancelled);
                }
                rail.detail(
                    out,
                    &format!("{error}. Type it again, or press Enter to cancel."),
                )?;
                rail.gap(out)?;
            }
        }
    };

    // From here the signals are counted instead of killing the process: nothing is written
    // before this, so the default action was fine until now.
    let mut warnings = Vec::new();
    if let Err(error) = env.signals.arm() {
        warnings.push(format!(
            "could not watch for Ctrl-C ({error}); the next run removes a leftover key"
        ));
    }
    let now = (env.now)();
    let backup = Backup::new(now);
    warnings.extend(pairing::sweep(&env.account, &backup, now.to_unix()));
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
    warnings.extend(live.take_warnings());
    if !warnings.is_empty() {
        rail.gap(out)?;
        for warning in &warnings {
            rail.step(out, Mark::Warn, warning)?;
        }
    }

    print_host(rail, out, host)?;
    let until = live.deadline_clock();
    rail.gap(out)?;
    rail.step(
        out,
        Mark::Info,
        &format!(
            "A temporary pairing key was added for {} until {until}\nScan this with the same phone:",
            host.user
        ),
    )?;
    print_code(rail, out, text, options)?;
    rail.gap(out)?;
    rail.step(
        out,
        Mark::Info,
        &format!("Waiting for the phone (until {until}). Ctrl-C removes the temporary key."),
    )?;
    out.flush()?;
    if let Some(ready) = env.on_ready {
        ready(&Ready {
            payload: text.to_owned(),
        });
    }

    let ended = live.wait(env.signals, env.poll, env.window, env.now);
    let line = live.line().to_owned();
    let warnings = live.take_warnings();
    drop(live);
    let keys = env.account.keys_path();
    let (report, exit) = report_ending(
        ended,
        &Ending {
            user: host.user,
            keys: &keys,
            line: &line,
            today: (env.now)().date(),
            backup: backup.path(),
            warnings,
        },
    );
    rail.gap(out)?;
    report.draw(rail, out)?;
    Ok(exit)
}

/// What the report of a run's end needs besides how it ended.
#[cfg(unix)]
struct Ending<'a> {
    user: &'a str,
    keys: &'a std::path::Path,
    /// The bootstrap line (shown when it could not be removed).
    line: &'a str,
    /// Today's date, as in the phone's line.
    today: String,
    /// Where the previous `authorized_keys` was saved, when it was.
    backup: Option<PathBuf>,
    /// What went wrong on the way without changing the outcome.
    warnings: Vec<String>,
}

/// One part of the report of a run's end.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    /// A message on the rail ([`Rail::step`]).
    Step(Mark, String),
    /// Lines drawn as they are ([`Rail::block`]): an `authorized_keys` line to find.
    Block(String),
}

/// How a run ended: what is said, then the word the rail closes with.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct Report {
    parts: Vec<Part>,
    close: &'static str,
}

#[cfg(unix)]
impl Report {
    fn draw(&self, rail: &Rail, out: &mut dyn Write) -> io::Result<()> {
        for part in &self.parts {
            match part {
                Part::Step(mark, text) => rail.step(out, *mark, text)?,
                Part::Block(text) => rail.block(out, text)?,
            }
        }
        rail.gap(out)?;
        rail.close(out, self.close)
    }

    /// Everything said, as plain text (tests).
    #[cfg(test)]
    fn text(&self) -> String {
        self.parts
            .iter()
            .map(|part| match part {
                Part::Step(_, text) | Part::Block(text) => format!("{text}\n"),
            })
            .collect()
    }
}

/// The report of how a pairing ended, and the exit. What happened to the temporary key is said
/// only as far as it is known.
#[cfg(unix)]
fn report_ending(ended: crate::pairing::Ended, at: &Ending<'_>) -> (Report, Exit) {
    use crate::pairing::Ended;
    let keys = at.keys.display();
    let key_fate = |removed: bool| {
        if removed {
            "The temporary key was removed."
        } else {
            "The temporary key was already gone from authorized_keys; nothing was removed."
        }
    };
    let not_removed = |error: &io::Error| {
        vec![
            Part::Step(
                Mark::Error,
                format!(
                    "Could not remove the temporary pairing key: {error}\nIf a phone was pairing at this moment it may have finished: look in {keys} for its line. Delete this line from that file (it can no longer be used: the pairing command refuses once the run is over; the next or2-pair also removes it):"
                ),
            ),
            Part::Block(at.line.to_owned()),
        ]
    };
    let (mut parts, close, exit) = match ended {
        Ended::Paired(done) => {
            let mut parts = vec![Part::Step(
                Mark::Ok,
                format!(
                    "\"{}\" can now log in as {} ({})\nIts key replaced the temporary key. To undo, delete the line ending or2-{}-{} in {keys}.",
                    done.device, at.user, done.fingerprint, done.device, at.today
                ),
            )];
            if let Some(warning) = done.warning {
                parts.push(Part::Step(Mark::Warn, format!("{warning}.")));
            }
            (parts, "Paired", Exit::Paired)
        }
        Ended::NotInstalled { done, removed } => {
            let what = done.map_or_else(
                || "A phone's pairing was recorded".to_owned(),
                |done| {
                    format!(
                        "\"{}\" ({}) was recorded as paired",
                        done.device, done.fingerprint
                    )
                },
            );
            let mut parts = Vec::new();
            match &removed {
                Ok(removed) => parts.push(Part::Step(
                    Mark::Error,
                    format!(
                        "{what}, but its key is not in {keys}: nothing was paired\n{} Run or2-pair again to retry.",
                        key_fate(*removed)
                    ),
                )),
                Err(error) => {
                    parts.push(Part::Step(
                        Mark::Error,
                        format!("{what}, but its key is not in {keys}: nothing was paired"),
                    ));
                    parts.extend(not_removed(error));
                }
            }
            (parts, "Not paired", Exit::Failed)
        }
        Ended::TimedOut { removed } => (
            vec![Part::Step(
                Mark::Warn,
                format!(
                    "Timed out: no phone paired in time\n{} Run or2-pair again to retry.",
                    key_fate(removed)
                ),
            )],
            "Timed out",
            Exit::TimedOut,
        ),
        Ended::Interrupted { removed } => (
            vec![Part::Step(
                if removed { Mark::Ok } else { Mark::Warn },
                key_fate(removed).to_owned(),
            )],
            "Cancelled",
            Exit::Interrupted,
        ),
        Ended::RemovalFailed(error) => (not_removed(&error), "Failed", Exit::Failed),
    };
    // Where the file was before this run touched it: under what became of it, unless a line to
    // delete follows that (then on its own, after it).
    if let Some(path) = &at.backup {
        let saved = format!("The previous file is saved as {}.", path.display());
        match parts.as_mut_slice() {
            [Part::Step(_, text)] | [Part::Step(_, text), Part::Step(..), ..] => {
                text.push('\n');
                text.push_str(&saved);
            }
            _ => parts.push(Part::Step(Mark::Info, saved)),
        }
    }
    for warning in &at.warnings {
        parts.push(Part::Step(Mark::Warn, format!("{warning}.")));
    }
    (Report { parts, close }, exit)
}

/// This host, as the phone will know it.
fn print_host(rail: &Rail, out: &mut dyn Write, host: &Host<'_>) -> io::Result<()> {
    rail.gap(out)?;
    rail.step(out, Mark::Info, "This host")?;
    let addresses: Vec<String> = host
        .addresses
        .iter()
        .filter(|address| host.payload.addresses.contains(&address.text))
        .map(|address| match address.kind {
            Kind::Mdns => format!("{} (mDNS name, same network only)", address.text),
            kind => format!("{} ({})", address.text, kind.label()),
        })
        .collect();
    rail.rows(
        out,
        &[
            ("name", host.name.to_owned()),
            ("user", host.user.to_owned()),
            ("ssh port", host.ssh_port.to_string()),
            (
                "host key",
                format!(
                    "{} {}  ({})",
                    host.host_key.key.algorithm(),
                    host.host_key.key.fingerprint(),
                    host.host_key.source
                ),
            ),
            ("addresses", addresses.join("\n")),
        ],
    )?;
    if !host.dropped.is_empty() {
        rail.step(
            out,
            Mark::Warn,
            &format!(
                "{} address(es) were left out, the last ones first: the phone takes at most {} addresses and a code of at most {} bytes",
                host.dropped.len(),
                payload::MAX_ADDRESSES,
                payload::MAX_BYTES
            ),
        )?;
    }
    Ok(())
}

/// The QR on the rail, and the same code as text, off the rail so it copies cleanly.
fn print_code(
    rail: &Rail,
    out: &mut dyn Write,
    text: &str,
    options: &Options,
) -> Result<(), RunError> {
    let style = QrStyle {
        ascii: options.ascii,
        color: rail.style().color,
        invert: options.invert,
    };
    rail.gap(out)?;
    let drawing = qr::render(text, style).map_err(|error| RunError::Qr(error.to_string()))?;
    rail.block(out, &drawing)?;
    if options.ascii {
        let (width, _) = qr::modules(text).map_err(|error| RunError::Qr(error.to_string()))?;
        rail.quiet(
            out,
            &format!(
                "(this drawing is {} columns wide)",
                qr::columns(width, style)
            ),
        )?;
    }
    rail.gap(out)?;
    rail.detail(
        out,
        "Or paste this code into the app (Easy pair > Paste pairing code):",
    )?;
    rail.bare(out, text)?;
    Ok(())
}

/// What to do by hand where `or2-pair` installs no key, as the lines of one message. Only the
/// login is used (never a home or profile directory looked up on the machine): the paths are
/// the ones the platform's SSH server documents.
fn manual_instructions(platform: Platform, user: &str, asked_for_manual: bool) -> String {
    let mut text = String::new();
    if asked_for_manual {
        text.push_str("--manual: nothing was changed on this host\n");
    } else {
        text.push_str(
            "or2-pair does not pair automatically on this platform\nIt would have to add a temporary key to authorized_keys, and it has no safe way to check that file's owner, links and permissions here. It changed nothing.\n",
        );
    }
    text.push_str(
        "After scanning, the app shows its public key (one line). Add it to the SSH server's authorized keys by hand:\n",
    );
    if platform == Platform::Windows {
        let _ = write!(
            text,
            "ordinary account: C:\\Users\\{user}\\.ssh\\authorized_keys (create the .ssh folder and the file if they are missing)\nmember of the Administrators group: C:\\ProgramData\\ssh\\administrators_authorized_keys instead (OpenSSH for Windows ignores the per-user file for administrators); then restrict it:\n`icacls \"C:\\ProgramData\\ssh\\administrators_authorized_keys\" /inheritance:r /grant \"Administrators:F\" /grant \"SYSTEM:F\"`"
        );
    } else {
        text.push_str(
            "~/.ssh/authorized_keys of the account the phone connects as (mode 600, with ~/.ssh mode 700)",
        );
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn text_of((report, exit): (Report, Exit)) -> (String, Exit) {
        (report.text(), exit)
    }

    /// The report drawn on the rail, without colour.
    #[cfg(unix)]
    fn drawn(report: &Report) -> String {
        let mut out = Vec::new();
        report.draw(&Rail::new(Style::plain()), &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn every_ending_closes_the_rail_with_its_word() {
        use crate::pairing::Ended;
        use crate::state::Done;
        let keys = std::path::PathBuf::from("/home/dev/.ssh/authorized_keys");
        let at = Ending {
            user: "dev",
            keys: &keys,
            line: "restrict,command=\"x\" ssh-ed25519 AAAA or2-pair-bootstrap-abcdefghijklm",
            today: "2026-07-01".into(),
            backup: Some("/home/dev/.ssh/authorized_keys.or2-backup-1".into()),
            warnings: vec!["the lock file could not be removed".into()],
        };
        let paired = Done {
            device: "Pixel-8".into(),
            fingerprint: "SHA256:x".into(),
            warning: None,
        };
        let (report, exit) = report_ending(Ended::Paired(paired), &at);
        assert_eq!(exit, Exit::Paired);
        assert_eq!(
            drawn(&report),
            "✔  \"Pixel-8\" can now log in as dev (SHA256:x)\n\
             │  Its key replaced the temporary key. To undo, delete the line ending or2-Pixel-8-2026-07-01 in /home/dev/.ssh/authorized_keys.\n\
             │  The previous file is saved as /home/dev/.ssh/authorized_keys.or2-backup-1.\n\
             ▲  the lock file could not be removed.\n\
             │\n\
             └  Paired\n"
        );
        for (ended, first, close) in [
            (
                Ended::TimedOut { removed: true },
                "▲  Timed out: no phone paired in time\n│  The temporary key was removed. Run or2-pair again to retry.\n",
                "└  Timed out\n",
            ),
            (
                Ended::Interrupted { removed: true },
                "✔  The temporary key was removed.\n",
                "└  Cancelled\n",
            ),
            (
                Ended::Interrupted { removed: false },
                "▲  The temporary key was already gone from authorized_keys; nothing was removed.\n",
                "└  Cancelled\n",
            ),
            (
                Ended::NotInstalled {
                    done: None,
                    removed: Ok(true),
                },
                "■  A phone's pairing was recorded, but its key is not in /home/dev/.ssh/authorized_keys: nothing was paired\n",
                "└  Not paired\n",
            ),
            (
                Ended::RemovalFailed(io::Error::other("the disk is full")),
                "■  Could not remove the temporary pairing key: the disk is full\n",
                "└  Failed\n",
            ),
        ] {
            let text = drawn(&report_ending(ended, &at).0);
            assert!(text.starts_with(first), "{text}");
            assert!(text.ends_with(close), "{text}");
        }
        // The line to delete is drawn whole on the rail, never wrapped, right under the words
        // that point at it.
        let text = drawn(&report_ending(Ended::RemovalFailed(io::Error::other("x")), &at).0);
        assert!(
            text.contains(&format!(
                "the next or2-pair also removes it):\n│  {}\n",
                at.line
            )),
            "{text}"
        );
        assert!(
            text.contains("\n●  The previous file is saved as "),
            "{text}"
        );
    }

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

    #[cfg(unix)]
    #[test]
    fn a_pairing_that_was_not_installed_says_what_became_of_the_temporary_key() {
        // Fix check of the v2 fixes: this ending always said "The temporary key was removed".
        use crate::pairing::Ended;
        use crate::state::Done;
        let keys = std::path::PathBuf::from("/home/dev/.ssh/authorized_keys");
        let at = Ending {
            user: "dev",
            keys: &keys,
            line: "restrict,command=\"x\" ssh-ed25519 AAAA or2-pair-bootstrap-abcdefghijklm",
            today: "2026-07-01".into(),
            backup: None,
            warnings: Vec::new(),
        };
        let done = || {
            Some(Done {
                device: "Pixel-8".into(),
                fingerprint: "SHA256:x".into(),
                warning: None,
            })
        };
        let (removed, exit) = text_of(report_ending(
            Ended::NotInstalled {
                done: done(),
                removed: Ok(true),
            },
            &at,
        ));
        assert_eq!(exit, Exit::Failed);
        assert!(removed.contains("nothing was paired\nThe temporary key was removed."));
        let (gone, _) = text_of(report_ending(
            Ended::NotInstalled {
                done: done(),
                removed: Ok(false),
            },
            &at,
        ));
        assert!(gone.contains("already gone") && !gone.contains("key was removed"));
        let (failed, _) = text_of(report_ending(
            Ended::NotInstalled {
                done: None,
                removed: Err(io::Error::other("the disk is full")),
            },
            &at,
        ));
        assert!(
            failed.contains("Could not remove the temporary pairing key: the disk is full")
                && failed.contains(at.line)
                && !failed.contains("key was removed"),
            "{failed}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_pairing_whose_record_carries_a_warning_says_it() {
        use crate::pairing::Ended;
        use crate::state::Done;
        let keys = std::path::PathBuf::from("/home/dev/.ssh/authorized_keys");
        let at = Ending {
            user: "dev",
            keys: &keys,
            line: "x",
            today: "2026-07-01".into(),
            backup: None,
            warnings: Vec::new(),
        };
        let (report, exit) = text_of(report_ending(
            Ended::Paired(Done {
                device: "Pixel-8".into(),
                fingerprint: "SHA256:x".into(),
                warning: Some("/home/dev/.ssh/authorized_keys was changed, but /home/dev/.ssh could not be synced to disk (EIO)".into()),
            }),
            &at,
        ));
        assert_eq!(exit, Exit::Paired);
        assert!(
            report.starts_with("\"Pixel-8\" can now log in as dev (SHA256:x)\n"),
            "{report}"
        );
        assert!(
            report.contains("\n/home/dev/.ssh/authorized_keys was changed, but"),
            "{report}"
        );
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
