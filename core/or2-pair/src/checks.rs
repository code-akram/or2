//! The checks `or2-pair` reports before it pairs. They only look: nothing is changed.
//!
//! - is `sshd` answering on the chosen port, which version it is (the banner decides which
//!   `authorized_keys` options are used; see `bootstrap`), and how to turn it on when it is not,
//! - can `~/.ssh/authorized_keys` be written (and would `sshd` honour it),
//! - a best-effort read of `sshd_config` and the files it `Include`s,
//! - the login shell (sshd runs the pairing command through it) and the path of this program,
//! - leftover pairing keys of earlier runs,
//! - are tmux, herdr and mosh-server installed (all optional; without herdr a warning, since
//!   or2's agents need it),
//! - a firewall hint for mosh's UDP ports.
//!
//! What is missing or failing comes with the exact fix for this host ([`crate::hints`]).
//!
//! A [`Level::Fail`] is something that makes automatic pairing impossible here (the run refuses
//! and points to `--manual`); a [`Level::Warn`] is something that may get in the way.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use crate::account::Account;
use crate::authorized_keys::{self, Writable};
use crate::bootstrap::{self, Dialect, DialectError};
use crate::hints::{self, HostFacts, Program};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    /// Automatic pairing cannot work; with `pairing` off this is reported as a warning.
    Fail,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub level: Level,
    /// The finding on its first line; its fix, when it has one, on the next lines (`\n`), which
    /// the rail draws under it.
    pub text: String,
}

fn check(level: Level, text: impl Into<String>) -> Check {
    Check {
        level,
        text: text.into(),
    }
}

/// The operating system, for the hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
    Windows,
    Other,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other
        }
    }
}

pub struct CheckInput<'a> {
    pub account: &'a Account,
    pub ssh_port: u16,
    /// What asking this machine's sshd on `ssh_port` gave: its banner line, or why not.
    pub probe: &'a io::Result<String>,
    /// `<etc>/sshd_config` is read from here.
    pub etc_ssh: &'a Path,
    /// This program's canonical path, or why it is unknown.
    pub exe: &'a Result<PathBuf, String>,
    /// Directories searched for programs: `PATH` plus the usual user and package-manager ones.
    pub program_dirs: &'a [PathBuf],
    pub platform: Platform,
    /// What the host has (package manager, sshd unit, firewall), for the exact fixes.
    pub facts: &'a HostFacts,
    /// Whether this run pairs automatically (so that what blocks it is a `Fail`). Off for
    /// `--manual` and for platforms that install no keys.
    pub pairing: bool,
    /// Whether keys are installed by hand on this platform: `authorized_keys` is then neither
    /// opened nor inspected.
    pub manual_keys: bool,
    /// Pairing ids of earlier runs that are over: their leftovers are reported, not removed.
    pub stale: &'a [bootstrap::PairingId],
    /// This program's version: what `<exe> --version` must print.
    pub version: &'a str,
    /// Runs the login shell for the shell check.
    pub shell: &'a dyn ShellProbe,
}

/// Runs `<shell> -c <command>` and returns its standard output if it exits 0, or why not. The
/// checks ask the account's login shell to start this program the way sshd will.
pub trait ShellProbe {
    fn run(&self, shell: &str, command: &str) -> Result<String, String>;
}

/// How long the login shell gets to print this program's version.
pub const SHELL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The real login shell, with a time limit (rc files that wait for input or hang are a finding,
/// not a hung check).
///
/// The limit covers the output too: a process an rc file starts in the background can keep the
/// output pipe open long after the shell has exited, so the pipe is read inside the same
/// deadline, and at the limit the shell's process group (on Unix the shell gets one of its own,
/// so that its background jobs are in it) is killed and whatever was read is used. Nothing waits
/// on the reading thread past the limit. SIGINT, SIGTERM, SIGHUP or SIGQUIT while it runs kill
/// that group too before they end `or2-pair` ([`crate::signals::spawn_group`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemShell;

/// At most this much of the shell's output is kept.
const SHELL_OUTPUT_LIMIT: usize = 64 * 1024;

impl ShellProbe for SystemShell {
    fn run(&self, shell: &str, command: &str) -> Result<String, String> {
        use std::io::Read;
        use std::process::{Command, Stdio};
        use std::sync::mpsc::{self, RecvTimeoutError};
        use std::time::{Duration, Instant};

        let deadline = Instant::now() + SHELL_TIMEOUT;
        let mut shell_command = Command::new(shell);
        shell_command
            .arg("-c")
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // On Unix in a process group of its own, which an ending signal kills before it ends
        // this process (the group does not get the terminal's Ctrl-C): `_ending` lives until
        // the shell is done with.
        #[cfg(unix)]
        let (mut child, _ending) = crate::signals::spawn_group(&mut shell_command)
            .map_err(|error| format!("it could not be started: {error}"))?;
        #[cfg(not(unix))]
        let mut child = shell_command
            .spawn()
            .map_err(|error| format!("it could not be started: {error}"))?;
        // Ends the shell and everything it started in its group (background jobs of rc files);
        // a group that is already gone is fine.
        let end = |child: &mut std::process::Child| {
            #[cfg(unix)]
            if let Ok(group) = libc::pid_t::try_from(child.id()) {
                // SAFETY: `kill` has no memory preconditions; the group is the shell's own.
                unsafe { libc::kill(-group, libc::SIGKILL) };
            }
            let _ = child.kill();
            let _ = child.wait();
        };

        let mut stdout = child.stdout.take().expect("piped");
        let (chunks, received) = mpsc::channel::<Vec<u8>>();
        // Ends at the end of the output, or when the receiver is gone and more arrives.
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(count) = stdout.read(&mut buffer) {
                if count == 0 || chunks.send(buffer[..count].to_vec()).is_err() {
                    break;
                }
            }
        });

        let mut output = Vec::new();
        let mut status = None;
        let mut ended = false;
        loop {
            if status.is_none() {
                match child.try_wait() {
                    Ok(found) => status = found,
                    Err(error) => {
                        end(&mut child);
                        return Err(error.to_string());
                    }
                }
            }
            if status.is_some() && ended {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let step = (deadline - now).min(Duration::from_millis(20));
            if ended {
                std::thread::sleep(step);
                continue;
            }
            match received.recv_timeout(step) {
                Ok(chunk) => {
                    let room = SHELL_OUTPUT_LIMIT.saturating_sub(output.len());
                    output.extend_from_slice(&chunk[..chunk.len().min(room)]);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => ended = true,
            }
        }
        if status.is_none() || !ended {
            // The limit: the shell is still running, or something it left behind still holds
            // the output.
            end(&mut child);
        }
        let Some(status) = status else {
            return Err(format!(
                "it did not finish within {} s",
                SHELL_TIMEOUT.as_secs()
            ));
        };
        let output = String::from_utf8_lossy(&output).into_owned();
        if status.success() {
            Ok(output)
        } else {
            Err(match status.code() {
                Some(code) => format!("it exited with status {code}"),
                None => "it was ended by a signal".to_owned(),
            })
        }
    }
}

/// What a host without herdr goes without.
const HERDR_NEEDED: &str = "or2's agents inbox, notifications and Reply need it";

pub fn run(input: &CheckInput<'_>) -> Vec<Check> {
    let mut out = Vec::new();
    let blocking = if input.pairing {
        Level::Fail
    } else {
        Level::Warn
    };
    out.push(sshd(input, blocking));
    if input.manual_keys {
        out.push(check(
            Level::Info,
            "authorized_keys is not checked\nor2-pair does not write it on this platform, so you add the phone's key by hand (the instructions follow the pairing code)",
        ));
    } else {
        out.extend(authorized_keys(input.account, blocking));
        out.extend(config(input, blocking));
        out.extend(shell(input, blocking));
        out.extend(exe(input, blocking));
        if !input.stale.is_empty() {
            out.push(check(
                Level::Info,
                format!(
                    "{} temporary pairing key(s) of earlier runs are still in authorized_keys; the next pairing removes them",
                    input.stale.len()
                ),
            ));
        }
    }
    let mosh = find_program("mosh-server", input.program_dirs);
    let mut found = Vec::new();
    for (name, program, note) in [
        (
            "tmux",
            Program::Tmux,
            "optional: or2 can attach to its sessions",
        ),
        ("herdr", Program::Herdr, HERDR_NEEDED),
        (
            "mosh-server",
            Program::MoshServer,
            "optional: terminals survive network changes",
        ),
    ] {
        let path = if name == "mosh-server" {
            mosh.clone()
        } else {
            find_program(name, input.program_dirs)
        };
        let fix = hints::install(program, input.platform, input.facts);
        match path {
            Some(_) => found.push(name),
            // or2's agents need herdr: without it there is no inbox, no notification, no Reply.
            None if program == Program::Herdr => out.push(check(
                Level::Warn,
                format!("herdr not found: {HERDR_NEEDED}\n{fix}"),
            )),
            None => out.push(check(
                Level::Info,
                format!("{name} not found ({note})\n{fix}"),
            )),
        }
    }
    if !found.is_empty() {
        out.insert(
            out.len() - (3 - found.len()),
            check(Level::Ok, format!("{} found", found.join(", "))),
        );
    }
    // What the macOS firewall does to mosh-server, when it could be asked; else the advice.
    let mac = hints::mac_firewall_checks(input.platform, input.facts, mosh.is_some());
    if !mac.is_empty() {
        out.extend(mac.into_iter().map(|(level, text)| check(level, text)));
    } else if let Some(hint) = hints::firewall(input.platform, input.facts, mosh.is_some()) {
        out.push(check(Level::Info, hint));
    }
    out
}

/// The software part of a banner: `OpenSSH_9.8p1` out of `SSH-2.0-OpenSSH_9.8p1 Ubuntu-1`.
fn software(banner: &str) -> &str {
    banner
        .strip_prefix("SSH-")
        .and_then(|rest| rest.split_once('-'))
        .map_or(banner, |(_, software)| software)
        .split_whitespace()
        .next()
        .unwrap_or(banner)
}

fn sshd(input: &CheckInput<'_>, blocking: Level) -> Check {
    match input.probe {
        Ok(banner) if banner.starts_with("SSH-") => {
            let port = input.ssh_port;
            let what = software(banner);
            match bootstrap::dialect(Some(banner)) {
                Err(DialectError::NotOpenSsh(_)) => check(
                    blocking,
                    format!(
                        "the SSH server on port {port} is {what}, not OpenSSH\npairing needs OpenSSH's authorized_keys options; use --manual"
                    ),
                ),
                Err(DialectError::NoBanner) => check(
                    blocking,
                    format!("sshd on port {port} showed no version banner\nuse --manual"),
                ),
                Ok(Dialect::ExpiryUtc) => check(
                    Level::Ok,
                    format!("sshd is answering on port {port} ({what})"),
                ),
                Ok(_) => check(
                    Level::Warn,
                    format!(
                        "sshd is answering on port {port} ({what})\n{}",
                        bootstrap::OLD_SSHD_NOTE
                    ),
                ),
            }
        }
        Ok(_) => check(
            blocking,
            format!(
                "something answers on port {} but it does not look like sshd\npass --ssh-port if sshd listens elsewhere",
                input.ssh_port
            ),
        ),
        Err(_) => check(
            blocking,
            format!(
                "sshd is not answering on port {}\n{}",
                input.ssh_port,
                hints::sshd(input.platform, input.facts)
            ),
        ),
    }
}

fn authorized_keys(account: &Account, blocking: Level) -> Vec<Check> {
    let file = account.keys_path();
    vec![match authorized_keys::writable(account) {
        Writable::Yes => check(
            Level::Ok,
            format!("{} can be written and sshd will honour it", file.display()),
        ),
        Writable::No(why) => check(blocking, why),
    }]
}

/// The login shell: not one that refuses commands (`nologin`, `false`), and one that really
/// starts this program: `<shell> -c "<exe> --version"` must exit 0 and print this version (rc
/// files may print around it, as they may around the pairing command), within
/// [`SHELL_TIMEOUT`]. An unknown shell is `/bin/sh` (what an empty account database field means).
fn shell(input: &CheckInput<'_>, blocking: Level) -> Vec<Check> {
    let shell = input.account.shell.as_deref();
    if let Err(error) = bootstrap::check_login_shell(shell) {
        return vec![check(blocking, error.to_string())];
    }
    let shell = shell.unwrap_or("/bin/sh");
    // A path that fails its own check is reported there; it is not run.
    let Some(exe) = input
        .exe
        .as_ref()
        .ok()
        .and_then(|path| bootstrap::check_exe_path(path).ok())
    else {
        return Vec::new();
    };
    let expected = format!("or2-pair {}", input.version);
    let command = format!("{exe} --version");
    let why = match input.shell.run(shell, &command) {
        Ok(output) if output.lines().any(|line| line.trim() == expected) => {
            return vec![check(Level::Ok, format!("login shell {shell} runs {exe}"))];
        }
        Ok(_) => format!("it ran but did not print `{expected}`"),
        Err(why) => why,
    };
    vec![check(
        blocking,
        format!(
            "the login shell {shell} could not run `{command}` ({why})\nsshd runs the pairing command through it, so pairing would fail; fix the shell or its startup files, or use --manual"
        ),
    )]
}

fn exe(input: &CheckInput<'_>, blocking: Level) -> Vec<Check> {
    match input.exe {
        Ok(path) => match bootstrap::check_exe_path(path) {
            Ok(text) => vec![check(
                Level::Ok,
                format!("sshd will run {text} for the pairing"),
            )],
            Err(error) => vec![check(blocking, error.to_string())],
        },
        Err(why) => vec![check(
            blocking,
            format!(
                "cannot tell where this program is installed ({why})\nsshd must be given its absolute path"
            ),
        )],
    }
}

// --- sshd_config, best effort -----------------------------------------------------------------

/// The settings of one part of `sshd_config` that matter for pairing (the first value of each
/// keyword in that part, as in sshd).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// `PubkeyAuthentication`: `Some(false)` is `no`.
    pub pubkey_authentication: Option<bool>,
    /// The words of `AuthorizedKeysFile`.
    pub authorized_keys_file: Option<Vec<String>>,
    /// `AuthorizedKeysCommand`: `Some(false)` is `none`.
    pub authorized_keys_command: Option<bool>,
    /// `ForceCommand`: `Some(false)` is `none`.
    pub force_command: Option<bool>,
    pub authentication_methods: Option<String>,
}

/// A `Match` block: its criteria (the words after `Match`) and its settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MatchBlock {
    pub criteria: Vec<String>,
    pub settings: Settings,
}

/// What `sshd_config` (and what it includes) says that matters for pairing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SshdConfig {
    pub port: Option<u16>,
    /// Outside `Match` blocks.
    pub global: Settings,
    /// In file order.
    pub blocks: Vec<MatchBlock>,
    /// An `Include` named a file that could not be read: anything could be in it.
    pub unreadable_include: bool,
}

const INCLUDE_DEPTH: usize = 4;

/// Reads `<etc>/sshd_config` and its `Include`s. `None` when it cannot be read (not root, not
/// there): that is not a finding.
pub fn read_sshd_config(etc_ssh: &Path) -> Option<SshdConfig> {
    let text = std::fs::read_to_string(etc_ssh.join("sshd_config")).ok()?;
    let mut config = SshdConfig::default();
    let mut seen = HashSet::new();
    let mut block = None;
    parse_config(&text, etc_ssh, 0, &mut config, &mut seen, &mut block);
    Some(config)
}

/// Splits `Keyword arg arg`, `Keyword=arg` and `Keyword = arg`.
fn directive(line: &str) -> Option<(String, Vec<String>)> {
    let line = line.split('#').next()?.trim();
    if line.is_empty() {
        return None;
    }
    let end = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let keyword = line[..end].to_ascii_lowercase();
    let rest = line[end..].trim_start_matches(|c: char| c.is_whitespace() || c == '=');
    let words = rest
        .split_whitespace()
        .map(|word| word.trim_matches('"').to_owned())
        .collect();
    Some((keyword, words))
}

/// Whether `name` matches a pattern with `*` and `?` (an `Include` file name, a `Match User`
/// pattern).
fn glob_match(pattern: &[u8], name: &[u8]) -> bool {
    match (pattern.first(), name.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            glob_match(&pattern[1..], name) || (!name.is_empty() && glob_match(pattern, &name[1..]))
        }
        (Some(b'?'), Some(_)) => glob_match(&pattern[1..], &name[1..]),
        (Some(p), Some(n)) if p == n => glob_match(&pattern[1..], &name[1..]),
        _ => false,
    }
}

/// The files an `Include` pattern names; `None` when it names one file that cannot be read.
fn included_files(pattern: &str, etc_ssh: &Path) -> Option<Vec<PathBuf>> {
    let path = if Path::new(pattern).is_absolute() {
        PathBuf::from(pattern)
    } else {
        etc_ssh.join(pattern)
    };
    let (Some(dir), Some(file)) = (path.parent(), path.file_name().and_then(|f| f.to_str())) else {
        return Some(Vec::new());
    };
    if !file.contains(['*', '?']) {
        return Some(vec![path]);
    }
    let entries = std::fs::read_dir(dir).ok()?;
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| glob_match(file.as_bytes(), name.as_bytes()))
        })
        .map(|entry| entry.path())
        .collect();
    files.sort();
    Some(files)
}

/// Records the first value of each keyword that matters into `settings`. The enumerated values
/// (`yes`/`no`, `none`) are compared ignoring case, as sshd does (`sshd -T` reads
/// `PubkeyAuthentication No` as `no` and `ForceCommand None` as `none`).
fn set(settings: &mut Settings, keyword: &str, args: &[String]) {
    let first = args.first().map(String::as_str);
    let is = |value: &str| first.is_some_and(|first| first.eq_ignore_ascii_case(value));
    match keyword {
        "pubkeyauthentication" if settings.pubkey_authentication.is_none() && first.is_some() => {
            settings.pubkey_authentication = Some(!is("no"));
        }
        "authorizedkeysfile" if settings.authorized_keys_file.is_none() => {
            settings.authorized_keys_file = Some(args.to_vec());
        }
        "authorizedkeyscommand"
            if settings.authorized_keys_command.is_none() && first.is_some() =>
        {
            settings.authorized_keys_command = Some(!is("none"));
        }
        "forcecommand" if settings.force_command.is_none() && first.is_some() => {
            settings.force_command = Some(!is("none"));
        }
        "authenticationmethods" if settings.authentication_methods.is_none() => {
            settings.authentication_methods = Some(args.join(" "));
        }
        _ => {}
    }
}

fn parse_config(
    text: &str,
    etc_ssh: &Path,
    depth: usize,
    config: &mut SshdConfig,
    seen: &mut HashSet<PathBuf>,
    block: &mut Option<usize>,
) {
    for line in text.lines() {
        let Some((keyword, args)) = directive(line) else {
            continue;
        };
        match keyword.as_str() {
            "match" => {
                config.blocks.push(MatchBlock {
                    criteria: args,
                    settings: Settings::default(),
                });
                *block = Some(config.blocks.len() - 1);
            }
            "include" if depth < INCLUDE_DEPTH => {
                for pattern in &args {
                    let Some(files) = included_files(pattern, etc_ssh) else {
                        config.unreadable_include = true;
                        continue;
                    };
                    for file in files {
                        if !seen.insert(file.clone()) {
                            continue;
                        }
                        match std::fs::read_to_string(&file) {
                            Ok(included) => {
                                // Included where it stands (inside the current Match block, if
                                // any); a `Match` inside an included file ends with that file.
                                let mut inner = *block;
                                parse_config(
                                    &included,
                                    etc_ssh,
                                    depth + 1,
                                    config,
                                    seen,
                                    &mut inner,
                                );
                            }
                            Err(_) => config.unreadable_include = true,
                        }
                    }
                }
            }
            "port" if block.is_none() && config.port.is_none() => {
                config.port = args
                    .first()
                    .and_then(|p| p.parse().ok())
                    .filter(|p| *p != 0);
            }
            _ => match *block {
                Some(index) => set(&mut config.blocks[index].settings, &keyword, &args),
                None => set(&mut config.global, &keyword, &args),
            },
        }
    }
}

/// Whether a `Match` block applies to `user`: `Some(true)` or `Some(false)` when that can be
/// told from its criteria (`all`, `User` patterns), `None` when it depends on something else
/// (a group, the client's address, a host name, a command).
fn applies(criteria: &[String], user: &str) -> Option<bool> {
    if criteria.len() == 1 && criteria[0].eq_ignore_ascii_case("all") {
        return Some(true);
    }
    if criteria.is_empty() || !criteria.len().is_multiple_of(2) {
        return None;
    }
    let mut known = true;
    for pair in criteria.chunks(2) {
        if pair[0].eq_ignore_ascii_case("user") {
            if !user_matches(&pair[1], user) {
                // All criteria must hold: one that does not settles it.
                return Some(false);
            }
        } else {
            known = false;
        }
    }
    known.then_some(true)
}

/// sshd's pattern list for `Match User`: comma-separated `*`/`?` patterns, a `!` pattern that
/// matches excludes.
fn user_matches(list: &str, user: &str) -> bool {
    let mut matched = false;
    for pattern in list.split(',') {
        match pattern.strip_prefix('!') {
            Some(negated) if glob_match(negated.as_bytes(), user.as_bytes()) => return false,
            Some(_) => {}
            None => matched |= glob_match(pattern.as_bytes(), user.as_bytes()),
        }
    }
    matched
}

/// What sshd would use for one keyword for this account, as far as can be told.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Resolved<T> {
    /// The value from the first `Match` block that applies and sets it, else the global one.
    value: Option<T>,
    /// A `Match` block that may apply (it could not be evaluated) sets it before the value was
    /// found, or an `Include` could not be read: the value may be another.
    uncertain: bool,
    /// The values set in `Match` blocks that may apply, before a block that applies decided it.
    possible: Vec<T>,
}

impl SshdConfig {
    fn resolve<T: Clone>(&self, user: &str, pick: impl Fn(&Settings) -> Option<T>) -> Resolved<T> {
        let mut resolved = Resolved {
            value: None,
            uncertain: self.unreadable_include,
            possible: Vec::new(),
        };
        let mut decided = false;
        for block in &self.blocks {
            let Some(value) = pick(&block.settings) else {
                continue;
            };
            match applies(&block.criteria, user) {
                Some(true) if !decided => {
                    resolved.value = Some(value);
                    decided = true;
                }
                Some(_) => {}
                // Once a block that applies has decided the value, a later one cannot change
                // it, whether it applies or not (external review: a later `Match Group` block
                // made a decided `AuthorizedKeysFile` uncertain again).
                None if !decided => {
                    resolved.uncertain = true;
                    resolved.possible.push(value);
                }
                None => {}
            }
        }
        if !decided {
            resolved.value = pick(&self.global);
        }
        resolved
    }
}

/// How sure a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Fine,
    /// sshd will do it for this account: a `fail` when pairing.
    Certain,
    /// It may: a `warn`.
    Maybe,
}

fn verdict<T>(resolved: &Resolved<T>, bad: impl Fn(&T) -> bool) -> Verdict {
    match &resolved.value {
        Some(value) if bad(value) && !resolved.uncertain => Verdict::Certain,
        Some(value) if bad(value) => Verdict::Maybe,
        _ if resolved.possible.iter().any(&bad) => Verdict::Maybe,
        _ => Verdict::Fine,
    }
}

/// Whether the account's own `~/.ssh/authorized_keys` is one of `AuthorizedKeysFile`'s words.
fn includes_default_keys(words: &[String], account: &Account) -> bool {
    let home = account.home.to_string_lossy();
    let wanted = account.keys_path();
    words.iter().any(|word| {
        let expanded = word
            .replace("%h", &home)
            .replace("%u", &account.name)
            .replace("%%", "%");
        let expanded = match expanded.strip_prefix("~/") {
            Some(rest) => format!("{home}/{rest}"),
            None => expanded,
        };
        let path = if Path::new(&expanded).is_absolute() {
            PathBuf::from(expanded)
        } else {
            account.home.join(expanded)
        };
        path == wanted
    })
}

/// Whether some alternative of `AuthenticationMethods` is a key alone.
fn key_alone_is_enough(methods: &str) -> bool {
    let methods = methods.trim();
    methods.is_empty()
        || methods == "any"
        || methods
            .split_whitespace()
            .any(|alternative| alternative == "publickey")
}

/// The findings of `sshd_config` for this account. What sshd will certainly do that makes
/// pairing impossible is `blocking` (a `fail` when pairing): `PubkeyAuthentication no`, an
/// `AuthorizedKeysFile` without `~/.ssh/authorized_keys` (unless an `AuthorizedKeysCommand` may
/// read that file itself), a `ForceCommand`. What it may do (a `Match` block that cannot be
/// evaluated, an `Include` that cannot be read) and what may get in the way otherwise (an
/// `AuthorizedKeysCommand` when the file may not be read, `AuthenticationMethods` that need more
/// than a key) is a `warn`. An `AuthorizedKeysCommand` next to an `AuthorizedKeysFile` that
/// includes `~/.ssh/authorized_keys` is no finding: sshd reads both.
fn config(input: &CheckInput<'_>, blocking: Level) -> Vec<Check> {
    let Some(config) = read_sshd_config(input.etc_ssh) else {
        return Vec::new();
    };
    let user = input.account.name.as_str();
    let level = |verdict: Verdict| match verdict {
        Verdict::Certain => blocking,
        _ => Level::Warn,
    };
    let maybe = |verdict: Verdict| {
        if verdict == Verdict::Maybe {
            " (in a Match block or an Include or2-pair cannot evaluate, so it may not apply)"
        } else {
            ""
        }
    };
    let mut out = Vec::new();

    let pubkey = verdict(&config.resolve(user, |s| s.pubkey_authentication), |on| !on);
    if pubkey != Verdict::Fine {
        out.push(check(
            level(pubkey),
            format!(
                "sshd_config has `PubkeyAuthentication no`{}\nsshd would not accept the phone's key at all; use --manual after enabling it",
                maybe(pubkey)
            ),
        ));
    }

    let command = config.resolve(user, |s| s.authorized_keys_command);
    let command_verdict = verdict(&command, |on| *on);
    let files = config.resolve(user, |s| s.authorized_keys_file.clone());
    let excluded = verdict(&files, |words| !includes_default_keys(words, input.account));
    if excluded != Verdict::Fine {
        let words = files
            .value
            .as_ref()
            .filter(|words| !includes_default_keys(words, input.account))
            .or_else(|| files.possible.first())
            .map(|words| words.join(" "))
            .unwrap_or_default();
        // An AuthorizedKeysCommand may read that file itself: not certain then. Say which
        // doubt it is: the setting itself is certain when only the command softens it.
        let (verdict, why) = match (excluded, command_verdict) {
            (_, Verdict::Fine) => (excluded, maybe(excluded)),
            (Verdict::Certain, _) => (
                Verdict::Maybe,
                " (an AuthorizedKeysCommand is set too, which might read that file itself)",
            ),
            _ => (Verdict::Maybe, maybe(Verdict::Maybe)),
        };
        out.push(check(
            level(verdict),
            format!(
                "sshd_config's AuthorizedKeysFile ({words}) does not include .ssh/authorized_keys{why}\nsshd would not read the temporary key there; use --manual"
            ),
        ));
    }
    // sshd consults an AuthorizedKeysCommand in addition to the AuthorizedKeysFile, not instead
    // of it: with `.ssh/authorized_keys` among the files (systemd's standard
    // `20-systemd-userdb.conf` snippet, for one) the temporary key is found there whatever the
    // command says. Only when the file may not be read does the command matter.
    if command_verdict != Verdict::Fine && excluded != Verdict::Fine {
        out.push(check(
            Level::Warn,
            "sshd_config sets an AuthorizedKeysCommand and its AuthorizedKeysFile may leave out .ssh/authorized_keys\npairing would likely fail here; use --manual",
        ));
    }

    let force = verdict(&config.resolve(user, |s| s.force_command), |on| *on);
    if force != Verdict::Fine {
        out.push(check(
            level(force),
            format!(
                "sshd_config sets a ForceCommand{}, which would run instead of the pairing command\nuse --manual",
                maybe(force)
            ),
        ));
    }

    let methods = config.resolve(user, |s| s.authentication_methods.clone());
    if verdict(&methods, |methods| !key_alone_is_enough(methods)) != Verdict::Fine {
        let shown = methods
            .value
            .iter()
            .chain(&methods.possible)
            .find(|methods| !key_alone_is_enough(methods))
            .cloned()
            .unwrap_or_default();
        out.push(check(
            Level::Warn,
            format!(
                "sshd_config's AuthenticationMethods ({shown}) needs more than a key\npairing would likely fail here; use --manual"
            ),
        ));
    }
    out
}

/// The first executable called `name` in `dirs`.
pub fn find_program(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().find_map(|dir| {
        let path = dir.join(name);
        is_executable(&path).then_some(path)
    })
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file() || path.with_extension("exe").is_file()
}

/// `PATH` plus where user installs and package managers put things.
pub fn program_dirs(path_var: Option<&std::ffi::OsStr>, home: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = path_var
        .map(|path| std::env::split_paths(path).collect())
        .unwrap_or_default();
    // Without a known home (an account that is only a login) nothing is searched below it, and
    // never a relative directory.
    let user_dirs = if home.as_os_str().is_empty() {
        Vec::new()
    } else {
        vec![home.join(".local/bin"), home.join(".cargo/bin")]
    };
    for extra in user_dirs.into_iter().chain([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ]) {
        if !dirs.contains(&extra) {
            dirs.push(extra);
        }
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn banner(text: &str) -> io::Result<String> {
        Ok(text.to_owned())
    }

    /// A login shell that answers what the test says, and remembers what it was asked.
    struct FakeShell(Result<&'static str, &'static str>);

    impl ShellProbe for FakeShell {
        fn run(&self, _: &str, _: &str) -> Result<String, String> {
            self.0.map(str::to_owned).map_err(str::to_owned)
        }
    }

    struct Setup {
        home: tempfile::TempDir,
        etc: tempfile::TempDir,
        probe: io::Result<String>,
        exe: Result<PathBuf, String>,
        dirs: Vec<PathBuf>,
        platform: Platform,
        facts: HostFacts,
        pairing: bool,
        account_shell: Option<String>,
        shell_says: Result<&'static str, &'static str>,
    }

    impl Setup {
        fn new(probe: io::Result<String>) -> Self {
            Self {
                home: tempfile::tempdir().unwrap(),
                etc: tempfile::tempdir().unwrap(),
                probe,
                exe: Ok(PathBuf::from("/home/dev/.cargo/bin/or2-pair")),
                dirs: Vec::new(),
                platform: Platform::Linux,
                facts: HostFacts::default(),
                pairing: true,
                account_shell: Some("/bin/bash".into()),
                shell_says: Ok("or2-pair test\n"),
            }
        }

        fn run(&self) -> Vec<Check> {
            let mut account = Account::new("dev", self.home.path());
            account.shell = self.account_shell.clone();
            run(&CheckInput {
                account: &account,
                ssh_port: 22,
                probe: &self.probe,
                etc_ssh: self.etc.path(),
                exe: &self.exe,
                program_dirs: &self.dirs,
                platform: self.platform,
                facts: &self.facts,
                pairing: self.pairing,
                manual_keys: false,
                stale: &[],
                version: "test",
                shell: &FakeShell(self.shell_says),
            })
        }

        fn config(&self, text: &str) {
            std::fs::write(self.etc.path().join("sshd_config"), text).unwrap();
        }
    }

    fn texts(checks: &[Check], level: Level) -> Vec<&str> {
        checks
            .iter()
            .filter(|c| c.level == level)
            .map(|c| c.text.as_str())
            .collect()
    }

    #[cfg(unix)]
    fn executable(dir: &Path, name: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_healthy_host_reports_ok_and_changes_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let bin = tempfile::tempdir().unwrap();
        for name in ["tmux", "herdr", "mosh-server"] {
            executable(bin.path(), name);
        }
        let mut setup = Setup::new(banner("SSH-2.0-OpenSSH_9.9"));
        std::fs::set_permissions(setup.home.path(), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        setup.dirs = vec![bin.path().to_path_buf()];
        let checks = setup.run();
        assert_eq!(checks[0].level, Level::Ok);
        assert_eq!(checks[0].text, "sshd is answering on port 22 (OpenSSH_9.9)");
        assert!(texts(&checks, Level::Fail).is_empty(), "{checks:?}");
        assert!(texts(&checks, Level::Warn).is_empty(), "{checks:?}");
        assert!(
            checks
                .iter()
                .any(|c| c.text == "tmux, herdr, mosh-server found" && c.level == Level::Ok),
            "{checks:?}"
        );
        assert!(checks.iter().any(|c| {
            c.text
                .contains("sshd will run /home/dev/.cargo/bin/or2-pair")
        }));
        assert!(
            checks
                .iter()
                .any(|c| c.text.contains("login shell /bin/bash"))
        );
        let firewall = checks.last().unwrap();
        assert!(firewall.text.contains("60000-61000") && firewall.text.contains("ufw"));
        // Looking changed nothing: no ~/.ssh was created.
        assert!(!setup.home.path().join(".ssh").exists());
    }

    #[test]
    fn a_stopped_sshd_gets_a_platform_hint_and_blocks_pairing() {
        for (platform, word) in [
            (Platform::MacOs, "Remote Login"),
            // Nothing known of this Linux host: no systemctl guessed (external review).
            (
                Platform::Linux,
                "start sshd with this host's service manager",
            ),
            (Platform::Windows, "OpenSSH Server"),
        ] {
            let mut setup = Setup::new(Err(io::ErrorKind::ConnectionRefused.into()));
            setup.platform = platform;
            let checks = setup.run();
            assert_eq!(checks[0].level, Level::Fail);
            assert!(checks[0].text.contains(word), "{}", checks[0].text);
            // Not pairing (--manual): a warning only.
            setup.pairing = false;
            assert_eq!(setup.run()[0].level, Level::Warn);
        }
    }

    #[cfg(unix)]
    #[test]
    fn what_is_missing_comes_with_this_hosts_fix() {
        let bin = tempfile::tempdir().unwrap();
        executable(bin.path(), "mosh-server");
        let mut setup = Setup::new(Err(io::ErrorKind::ConnectionRefused.into()));
        setup.dirs = vec![bin.path().to_path_buf()];
        setup.facts = HostFacts {
            package_manager: Some(hints::PackageManager::Apt),
            service_manager: Some(hints::ServiceManager::Systemd),
            sshd_unit: Some("ssh".into()),
            sshd_installed: Some(true),
            sshd_packaged: Some(true),
            declarative: None,
            firewall: Some(hints::Firewall::Ufw),
            mac_firewall: None,
            superuser: false,
        };
        let checks = setup.run();
        assert_eq!(
            checks[0].text,
            "sshd is not answering on port 22\nstart it with `sudo systemctl enable --now ssh`\nif sshd listens on another port, pass --ssh-port"
        );
        let infos = texts(&checks, Level::Info);
        assert!(
            infos.contains(
                &"tmux not found (optional: or2 can attach to its sessions)\ninstall it: `sudo apt install tmux`"
            ),
            "{infos:?}"
        );
        // herdr is a warning: or2's agents need it. Its own installer, recommended, never run.
        assert!(
            texts(&checks, Level::Warn).contains(
                &"herdr not found: or2's agents inbox, notifications and Reply need it\ninstall it: `curl -fsSL https://herdr.dev/install.sh | sh`\n(or Homebrew, mise, Nix: https://herdr.dev/docs/install/)"
            ),
            "{checks:?}"
        );
        assert!(!infos.iter().any(|t| t.contains("herdr")), "{infos:?}");
        assert!(
            infos
                .iter()
                .any(|t| t.contains("ufw is on; open them with:\n`sudo ufw allow 60000:61000/udp`")),
            "{infos:?}"
        );
        // On a Mac with Homebrew.
        setup.platform = Platform::MacOs;
        setup.facts = HostFacts {
            package_manager: Some(hints::PackageManager::Brew),
            ..HostFacts::default()
        };
        let checks = setup.run();
        assert!(checks[0].text.contains("Remote Login"));
        assert!(
            texts(&checks, Level::Info)
                .iter()
                .any(|t| t.starts_with("tmux not found") && t.ends_with("`brew install tmux`")),
            "{checks:?}"
        );
        assert!(
            texts(&checks, Level::Warn).iter().any(|t| t.starts_with(
                "herdr not found: or2's agents inbox, notifications and Reply need it\ninstall it: `curl -fsSL https://herdr.dev/install.sh | sh`"
            )),
            "{checks:?}"
        );
        // Windows: herdr's docs, no installer to pipe into a shell.
        setup.platform = Platform::Windows;
        assert!(
            texts(&setup.run(), Level::Warn).contains(
                &"herdr not found: or2's agents inbox, notifications and Reply need it\nsee herdr's install docs: https://herdr.dev/docs/install/"
            ),
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_mac_firewall_that_blocks_mosh_warns_with_the_fix_and_pairing_goes_on() {
        use std::os::unix::fs::PermissionsExt;
        let bin = tempfile::tempdir().unwrap();
        // herdr here: its absence would be a warning of its own.
        for name in ["mosh-server", "herdr"] {
            executable(bin.path(), name);
        }
        let mut setup = Setup::new(banner("SSH-2.0-OpenSSH_9.9"));
        std::fs::set_permissions(setup.home.path(), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        setup.dirs = vec![bin.path().to_path_buf()];
        setup.platform = Platform::MacOs;
        let cellar = "/opt/homebrew/Cellar/mosh/1.4.0_31/bin/mosh-server";
        setup.facts = HostFacts {
            mac_firewall: Some(hints::MacFirewall::On {
                block_all: Some(false),
                mosh_server: Some(hints::MoshServerRule {
                    path: cellar.into(),
                    rule: Some(hints::AppRule::Blocked),
                }),
            }),
            ..HostFacts::default()
        };
        let checks = setup.run();
        assert!(texts(&checks, Level::Fail).is_empty(), "{checks:?}");
        let warnings = texts(&checks, Level::Warn);
        assert_eq!(warnings.len(), 1, "{checks:?}");
        assert!(
            warnings[0].contains(&format!(
                "sudo /usr/libexec/ApplicationFirewall/socketfilterfw --unblockapp \"{cellar}\""
            )),
            "{}",
            warnings[0]
        );
        // The generic advice is not printed next to it.
        assert!(!checks.iter().any(|c| c.text.contains("60000-61000")));

        // Nothing known (no socketfilterfw, an answer that cannot be read): the advice as before.
        setup.facts = HostFacts::default();
        let checks = setup.run();
        assert!(texts(&checks, Level::Warn).is_empty(), "{checks:?}");
        assert!(
            checks.last().unwrap().level == Level::Info
                && checks
                    .last()
                    .unwrap()
                    .text
                    .contains("allow mosh-server in System Settings > Network > Firewall"),
            "{checks:?}"
        );
    }

    #[test]
    fn something_that_is_not_sshd_blocks_pairing() {
        let checks = Setup::new(banner("HTTP/1.1 400")).run();
        assert_eq!(checks[0].level, Level::Fail);
        assert!(checks[0].text.contains("--ssh-port"));
    }

    #[test]
    fn the_banner_decides_what_pairing_can_do() {
        let dropbear = Setup::new(banner("SSH-2.0-dropbear_2022.83")).run();
        assert_eq!(dropbear[0].level, Level::Fail);
        assert!(dropbear[0].text.contains("not OpenSSH") && dropbear[0].text.contains("--manual"));
        // Below 9.1 the key is written without an expiry, and the note says so (from 7.7 up to
        // 9.0 too: those read an expiry only in sshd's own time zone).
        for (old, version) in [
            ("SSH-2.0-OpenSSH_7.4p1 Debian-10", "OpenSSH_7.4p1"),
            ("SSH-2.0-OpenSSH_8.9p1 Ubuntu-3", "OpenSSH_8.9p1"),
            ("SSH-2.0-OpenSSH_9.0", "OpenSSH_9.0"),
        ] {
            let checks = Setup::new(banner(old)).run();
            assert_eq!(checks[0].level, Level::Warn, "{old}");
            assert!(
                checks[0].text.contains(version) && checks[0].text.contains("without an expiry"),
                "{}",
                checks[0].text
            );
        }
        let current = Setup::new(banner("SSH-2.0-OpenSSH_9.1p1")).run();
        assert_eq!(current[0].level, Level::Ok);
    }

    #[cfg(unix)]
    #[test]
    fn an_unwritable_home_blocks_pairing() {
        let mut setup = Setup::new(banner("SSH-2.0-OpenSSH_9.9"));
        setup.home = tempfile::tempdir_in(setup.home.path()).unwrap();
        // A directory that does not exist is "not writable".
        let missing = Account::new("dev", "/nonexistent-or2-home");
        let checks = run(&CheckInput {
            account: &missing,
            ssh_port: 22,
            probe: &setup.probe,
            etc_ssh: setup.etc.path(),
            exe: &setup.exe,
            program_dirs: &[],
            platform: Platform::Linux,
            facts: &HostFacts::default(),
            pairing: true,
            manual_keys: false,
            stale: &[],
            version: "test",
            shell: &FakeShell(Ok("or2-pair test")),
        });
        assert!(
            checks
                .iter()
                .any(|c| c.level == Level::Fail && c.text.contains("not writable")),
            "{checks:?}"
        );
    }

    #[test]
    fn a_login_shell_that_cannot_run_commands_and_a_bad_path_block_pairing() {
        let mut setup = Setup::new(banner("SSH-2.0-OpenSSH_9.9"));
        setup.account_shell = Some("/usr/sbin/nologin".into());
        let checks = setup.run();
        assert!(
            texts(&checks, Level::Fail)
                .iter()
                .any(|t| t.contains("login shell")),
            "{checks:?}"
        );
        setup.account_shell = None;
        setup.exe = Ok(PathBuf::from("/home/my user/bin/or2-pair"));
        let checks = setup.run();
        assert!(
            texts(&checks, Level::Fail)
                .iter()
                .any(|t| t.contains("a space")),
            "{checks:?}"
        );
        setup.exe = Err("no such file".into());
        assert!(
            texts(&setup.run(), Level::Fail)
                .iter()
                .any(|t| t.contains("cannot tell where"))
        );
    }

    #[test]
    fn the_login_shell_must_really_start_this_program() {
        // Review of the v2 integration: only the names nologin and false were refused; a shell
        // whose startup files fail, or that cannot find the program, passed the checks.
        for (says, why) in [
            (Err("it exited with status 127"), "status 127"),
            (Err("it did not finish within 5 s"), "within 5 s"),
            (Ok("bash: or2-pair: command not found\n"), "did not print"),
            (Ok("or2-pair 0.0.1\n"), "did not print `or2-pair test`"),
        ] {
            let mut setup = Setup::new(banner("SSH-2.0-OpenSSH_9.9"));
            setup.shell_says = says;
            let checks = setup.run();
            let fails = texts(&checks, Level::Fail);
            assert!(
                fails.iter().any(|t| t.contains("login shell /bin/bash")
                    && t.contains("--version")
                    && t.contains(why)
                    && t.contains("--manual")),
                "{says:?}: {checks:?}"
            );
            // With --manual it is a warning.
            setup.pairing = false;
            assert!(texts(&setup.run(), Level::Fail).is_empty());
        }
        // Noise from startup files around the version is fine (the phone skips it too).
        let mut setup = Setup::new(banner("SSH-2.0-OpenSSH_9.9"));
        setup.shell_says = Ok("welcome\nor2-pair test\n");
        assert!(texts(&setup.run(), Level::Fail).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn the_system_shell_runs_the_command_with_a_time_limit() {
        assert_eq!(
            SystemShell.run("/bin/sh", "echo or2-pair test").unwrap(),
            "or2-pair test\n"
        );
        assert!(
            SystemShell
                .run("/bin/sh", "exit 3")
                .unwrap_err()
                .contains("status 3")
        );
        assert!(
            SystemShell
                .run("/nonexistent/shell", "true")
                .unwrap_err()
                .contains("could not be started")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_background_job_that_holds_the_output_does_not_outlast_the_limit() {
        // Fix check of the v2 fixes: the shell exited at once, but a job an rc file started in
        // the background kept the output pipe open, and the check waited for it (8 s here)
        // instead of stopping at the limit.
        let started = std::time::Instant::now();
        let result = SystemShell.run("/bin/sh", "sleep 8 & echo or2-pair X; exit 0");
        let took = started.elapsed();
        assert!(
            took < SHELL_TIMEOUT + std::time::Duration::from_millis(1500),
            "took {took:?}"
        );
        // The shell itself finished, and what it printed counts.
        assert_eq!(result.as_deref(), Ok("or2-pair X\n"));
        // A shell that does not finish is still a timeout, as soon as the limit.
        let started = std::time::Instant::now();
        let result = SystemShell.run("/bin/sh", "echo or2-pair X; sleep 8");
        assert!(
            started.elapsed() < SHELL_TIMEOUT + std::time::Duration::from_millis(1500),
            "{:?}",
            started.elapsed()
        );
        assert!(result.unwrap_err().contains("did not finish"));
    }

    #[test]
    fn leftover_pairing_keys_are_reported() {
        let setup = Setup::new(banner("SSH-2.0-OpenSSH_9.9"));
        let stale = [bootstrap::PairingId::parse("abcdefghijklm").unwrap()];
        let account = Account::new("dev", setup.home.path());
        let checks = run(&CheckInput {
            account: &account,
            ssh_port: 22,
            probe: &setup.probe,
            etc_ssh: setup.etc.path(),
            exe: &setup.exe,
            program_dirs: &[],
            platform: Platform::Linux,
            facts: &HostFacts::default(),
            pairing: true,
            manual_keys: false,
            stale: &stale,
            version: "test",
            shell: &FakeShell(Ok("or2-pair test")),
        });
        assert!(
            checks
                .iter()
                .any(|c| c.level == Level::Info && c.text.starts_with("1 temporary pairing key"))
        );
    }

    #[test]
    fn sshd_config_findings() {
        let setup = Setup::new(banner("SSH-2.0-OpenSSH_9.9"));
        let warns = |text: &str| {
            setup.config(text);
            let checks = setup.run();
            (
                texts(&checks, Level::Fail)
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect::<Vec<_>>(),
                texts(&checks, Level::Warn)
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect::<Vec<_>>(),
            )
        };
        // Nothing special.
        let (fails, warnings) = warns("Port 22\nPasswordAuthentication no\n");
        assert!(
            fails.iter().all(|f| !f.contains("PubkeyAuthentication")),
            "{fails:?}"
        );
        assert!(
            warnings.iter().all(|w| !w.contains("sshd_config")),
            "{warnings:?}"
        );
        // PubkeyAuthentication no refuses.
        let (fails, _) = warns("PubkeyAuthentication no\n");
        assert!(
            fails.iter().any(|f| f.contains("PubkeyAuthentication no")),
            "{fails:?}"
        );
        let (fails, _) = warns("PubkeyAuthentication yes\nPubkeyAuthentication no\n");
        assert!(
            fails.iter().all(|f| !f.contains("PubkeyAuthentication")),
            "first value wins"
        );
        // What sshd will certainly do that makes pairing impossible fails, before the prompt
        // (review of the v2 integration: these were warnings, and the person typed a code for a
        // run that could not work).
        let home = setup.home.path().display().to_string();
        for (config, word) in [
            (
                "AuthorizedKeysFile /etc/ssh/keys/%u\n",
                "AuthorizedKeysFile",
            ),
            ("AuthorizedKeysFile none\n", "AuthorizedKeysFile"),
            ("ForceCommand /bin/true\n", "ForceCommand"),
            ("Match User dev\n  ForceCommand /bin/true\n", "ForceCommand"),
            (
                "Match all\n  PubkeyAuthentication no\n",
                "PubkeyAuthentication",
            ),
            (
                "Match User d?v,other\n  AuthorizedKeysFile /k/%u\n",
                "AuthorizedKeysFile",
            ),
        ] {
            let (fails, _) = warns(config);
            let hit = fails.iter().find(|w| w.contains(word));
            assert!(
                hit.is_some_and(|w| w.contains("--manual")),
                "{config}: {fails:?}"
            );
        }
        // What only may get in the way warns and suggests --manual.
        for (config, word) in [
            // The file may not be read for this account (a block that cannot be evaluated), so
            // the command may be all sshd asks.
            (
                "AuthorizedKeysCommand /usr/bin/fetch %u\nMatch Group wheel\n  AuthorizedKeysFile /k/%u\n",
                "AuthorizedKeysCommand",
            ),
            // The command may read the file itself.
            (
                "AuthorizedKeysFile none\nAuthorizedKeysCommand /usr/bin/fetch %u\n",
                "AuthorizedKeysFile",
            ),
            (
                "AuthenticationMethods publickey,password\n",
                "AuthenticationMethods",
            ),
            (
                "AuthenticationMethods publickey,keyboard-interactive:pam\n",
                "AuthenticationMethods",
            ),
            // A Match block that cannot be evaluated here: it may or may not apply.
            (
                "Match Group wheel\n  ForceCommand /bin/true\n",
                "ForceCommand",
            ),
            (
                "PubkeyAuthentication no\nMatch Address 10.0.0.0/8\n  PubkeyAuthentication yes\n",
                "PubkeyAuthentication",
            ),
            (
                "Match User dev Address 10.0.0.0/8\n  AuthorizedKeysFile /k/%u\n",
                "AuthorizedKeysFile",
            ),
            // An include that cannot be read could hold anything.
            (
                "Include /nonexistent-or2/sshd.conf\nForceCommand /bin/true\n",
                "ForceCommand",
            ),
        ] {
            let (fails, warnings) = warns(config);
            assert!(
                fails.iter().all(|f| !f.contains("sshd_config")),
                "{config}: {fails:?}"
            );
            let hit = warnings.iter().find(|w| w.contains(word));
            assert!(
                hit.is_some_and(|w| w.contains("--manual")),
                "{config}: {warnings:?}"
            );
        }
        // With --manual a certain finding is a warning too.
        let mut manual = Setup::new(banner("SSH-2.0-OpenSSH_9.9"));
        manual.pairing = false;
        manual.config("ForceCommand /bin/true\n");
        let checks = manual.run();
        assert!(texts(&checks, Level::Fail).is_empty(), "{checks:?}");
        assert!(
            texts(&checks, Level::Warn)
                .iter()
                .any(|w| w.contains("ForceCommand")),
            "{checks:?}"
        );
        // And these are fine.
        for config in [
            "AuthorizedKeysFile .ssh/authorized_keys\n".to_owned(),
            "AuthorizedKeysFile .ssh/authorized_keys .ssh/authorized_keys2\n".to_owned(),
            "AuthorizedKeysFile %h/.ssh/authorized_keys\n".to_owned(),
            "AuthorizedKeysFile ~/.ssh/authorized_keys\n".to_owned(),
            format!("AuthorizedKeysFile {home}/.ssh/authorized_keys\n"),
            "AuthorizedKeysCommand none\n".to_owned(),
            // sshd reads the file as well as the command's output: the default file list (or
            // one that includes ~/.ssh/authorized_keys) still finds the temporary key.
            "AuthorizedKeysCommand /usr/bin/fetch %u\n".to_owned(),
            "AuthorizedKeysFile .ssh/authorized_keys\nAuthorizedKeysCommand /usr/bin/fetch %u\n"
                .to_owned(),
            "ForceCommand none\n".to_owned(),
            "AuthenticationMethods any\n".to_owned(),
            "AuthenticationMethods publickey\n".to_owned(),
            "AuthenticationMethods publickey password,publickey\n".to_owned(),
            "authorizedkeysfile=.ssh/authorized_keys\n".to_owned(),
            "# ForceCommand /bin/true\n".to_owned(),
            // Match blocks for someone else.
            "Match User other,!dev\n  ForceCommand /bin/true\n".to_owned(),
            "Match User root\n  PubkeyAuthentication no\n".to_owned(),
            "Match User other Group wheel\n  ForceCommand /bin/true\n".to_owned(),
            "Match User *,!dev\n  AuthorizedKeysFile /k/%u\n".to_owned(),
            // A matching block that sets it right overrides a global value.
            "Match User dev\n  ForceCommand none\nMatch all\n  ForceCommand /bin/true\n".to_owned(),
            "Match User dev\n  PubkeyAuthentication yes\nMatch all\nPubkeyAuthentication no\n"
                .to_owned(),
        ] {
            let (fails, warnings) = warns(&config);
            assert!(
                fails
                    .iter()
                    .chain(&warnings)
                    .all(|w| !w.contains("sshd_config")),
                "{config}: {fails:?} {warnings:?}"
            );
        }
    }

    #[test]
    fn systemds_userdb_snippet_is_not_a_finding() {
        // Owner report: every host with systemd's standard snippet (Arch, Fedora, ...) was
        // warned "sshd_config sets an AuthorizedKeysCommand; pairing would likely fail". sshd
        // consults the command in addition to AuthorizedKeysFile, whose default includes
        // ~/.ssh/authorized_keys, so the temporary key is found there.
        let setup = Setup::new(banner("SSH-2.0-OpenSSH_9.9"));
        let dir = setup.etc.path().join("sshd_config.d");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(
            dir.join("20-systemd-userdb.conf"),
            "# SPDX-License-Identifier: LGPL-2.1-or-later\n#\n# Make sure SSH authorized keys recorded in user records can be consumed by SSH\n#\nAuthorizedKeysCommand /usr/bin/userdbctl ssh-authorized-keys %u\nAuthorizedKeysCommandUser root\n",
        )
        .unwrap();
        for main in [
            "Include sshd_config.d/*.conf\n",
            "Include sshd_config.d/*.conf\nAuthorizedKeysFile .ssh/authorized_keys\nPasswordAuthentication no\n",
        ] {
            setup.config(main);
            assert_eq!(
                read_sshd_config(setup.etc.path())
                    .unwrap()
                    .global
                    .authorized_keys_command,
                Some(true)
            );
            let checks = setup.run();
            assert!(
                checks
                    .iter()
                    .filter(|c| matches!(c.level, Level::Warn | Level::Fail))
                    .all(|c| !c.text.contains("sshd_config")),
                "{main}: {checks:?}"
            );
        }
        // The same command next to a file list without ~/.ssh/authorized_keys still warns: only
        // the command could find the key then (it reads user records, so it will not).
        setup.config("Include sshd_config.d/*.conf\nAuthorizedKeysFile /etc/ssh/keys/%u\n");
        let checks = setup.run();
        let warnings = texts(&checks, Level::Warn);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("AuthorizedKeysFile (/etc/ssh/keys/%u)")),
            "{checks:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("AuthorizedKeysCommand") && w.contains("--manual")),
            "{checks:?}"
        );
        // The file setting is certain there; only the command makes it a warning, and the
        // warning says so rather than blaming a Match block (review: it said "in a Match block or
        // an Include or2-pair cannot evaluate").
        let excluded = warnings
            .iter()
            .find(|w| w.contains("AuthorizedKeysFile (/etc/ssh/keys/%u)"))
            .unwrap();
        assert!(
            excluded.contains(
                "(an AuthorizedKeysCommand is set too, which might read that file itself)"
            ) && !excluded.contains("Match block"),
            "{excluded}"
        );

        // A Match block that applies decides the value; a later block that cannot be evaluated
        // does not make it uncertain again (external review: `Match Group wheel` after
        // `Match User dev` revived the AuthorizedKeysCommand warnings).
        let decided = "Include sshd_config.d/*.conf\nMatch User dev\n  AuthorizedKeysFile .ssh/authorized_keys\nMatch Group wheel\n  AuthorizedKeysFile none\n";
        setup.config(decided);
        let config = read_sshd_config(setup.etc.path()).unwrap();
        let files = config.resolve("dev", |s| s.authorized_keys_file.clone());
        assert_eq!(
            files,
            Resolved {
                value: Some(vec![".ssh/authorized_keys".to_owned()]),
                uncertain: false,
                possible: Vec::new(),
            }
        );
        let checks = setup.run();
        assert!(
            checks
                .iter()
                .filter(|c| matches!(c.level, Level::Warn | Level::Fail))
                .all(|c| !c.text.contains("sshd_config")),
            "{checks:?}"
        );
        // The other order: the block that cannot be evaluated comes first, so it may decide.
        setup.config(
            "Include sshd_config.d/*.conf\nMatch Group wheel\n  AuthorizedKeysFile none\nMatch User dev\n  AuthorizedKeysFile .ssh/authorized_keys\n",
        );
        let checks = setup.run();
        let warnings = texts(&checks, Level::Warn);
        assert!(texts(&checks, Level::Fail).is_empty(), "{checks:?}");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("AuthorizedKeysFile (none)")
                    && w.contains("in a Match block or an Include")),
            "{checks:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("AuthorizedKeysCommand") && w.contains("--manual")),
            "{checks:?}"
        );
    }

    #[test]
    fn sshd_config_values_are_read_ignoring_case_as_sshd_does() {
        // Fix check of the v2 fixes: `PubkeyAuthentication No` was taken for enabled (and the
        // code was asked for a run that could not work), `ForceCommand None` for a command (and
        // a good host was blocked). `sshd -T` reads them as `no` and `none` (tests/sshd.rs).
        let setup = Setup::new(banner("SSH-2.0-OpenSSH_9.9"));
        for config in [
            "PubkeyAuthentication No\n",
            "PubkeyAuthentication NO\n",
            "Match User dev\n  PubkeyAuthentication nO\n",
        ] {
            setup.config(config);
            let checks = setup.run();
            assert!(
                texts(&checks, Level::Fail)
                    .iter()
                    .any(|f| f.contains("PubkeyAuthentication no")),
                "{config}: {checks:?}"
            );
        }
        for config in [
            "ForceCommand None\n",
            "ForceCommand NONE\n",
            "AuthorizedKeysCommand None\n",
            "PubkeyAuthentication Yes\n",
            "Match User dev\n  ForceCommand None\nMatch all\n  ForceCommand /bin/true\n",
        ] {
            setup.config(config);
            let checks = setup.run();
            assert!(
                texts(&checks, Level::Fail)
                    .iter()
                    .chain(&texts(&checks, Level::Warn))
                    .all(|w| !w.contains("sshd_config")),
                "{config}: {checks:?}"
            );
        }
    }

    #[test]
    fn sshd_config_match_blocks_and_includes() {
        let setup = Setup::new(banner("SSH-2.0-OpenSSH_9.9"));
        // Settings inside a Match block are that block's, not global ones.
        setup.config("Port 2200\nMatch Group sftponly\n    ForceCommand internal-sftp\n    PubkeyAuthentication no\n    Port 1\n");
        let config = read_sshd_config(setup.etc.path()).unwrap();
        assert_eq!(config.port, Some(2200));
        assert!(
            config.global.force_command.is_none() && config.global.pubkey_authentication.is_none()
        );
        assert_eq!(config.blocks.len(), 1);
        assert_eq!(config.blocks[0].criteria, ["Group", "sftponly"]);
        assert_eq!(config.blocks[0].settings.force_command, Some(true));
        assert_eq!(config.blocks[0].settings.pubkey_authentication, Some(false));
        assert!(!config.unreadable_include);

        // Includes: relative to /etc/ssh, with a wildcard, in file-name order; the first value
        // of a keyword wins across files.
        let dir = setup.etc.path().join("sshd_config.d");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(
            dir.join("10-first.conf"),
            "Port 2222\nForceCommand /bin/true\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("20-second.conf"),
            "Port 2223\nAuthenticationMethods publickey,password\n",
        )
        .unwrap();
        std::fs::write(dir.join("ignored.txt"), "PubkeyAuthentication no\n").unwrap();
        setup.config("Include sshd_config.d/*.conf\nPort 22\n");
        let config = read_sshd_config(setup.etc.path()).unwrap();
        assert_eq!(config.port, Some(2222));
        assert!(
            config.global.force_command == Some(true)
                && config.global.pubkey_authentication.is_none()
        );
        assert_eq!(
            config.global.authentication_methods.as_deref(),
            Some("publickey,password")
        );

        // A Match block that ends an included file does not reach the main file.
        std::fs::write(dir.join("30-match.conf"), "Match User x\n").unwrap();
        setup.config("Include sshd_config.d/*.conf\nPubkeyAuthentication no\n");
        assert_eq!(
            read_sshd_config(setup.etc.path())
                .unwrap()
                .global
                .pubkey_authentication,
            Some(false)
        );

        // An Include inside a Match block belongs to that block.
        std::fs::remove_file(dir.join("30-match.conf")).unwrap();
        setup.config("Match User dev\n  Include sshd_config.d/10-first.conf\n");
        let config = read_sshd_config(setup.etc.path()).unwrap();
        assert_eq!(config.global.force_command, None);
        assert_eq!(config.blocks[0].settings.force_command, Some(true));

        // An include loop ends; a file that cannot be read is no finding by itself, but makes
        // every other finding uncertain.
        setup.config("Include sshd_config\nInclude missing.conf\nPort 2201\n");
        let config = read_sshd_config(setup.etc.path()).unwrap();
        assert_eq!(config.port, Some(2201));
        assert!(config.unreadable_include);
        std::fs::remove_file(setup.etc.path().join("sshd_config")).unwrap();
        assert_eq!(read_sshd_config(setup.etc.path()), None);
    }

    #[test]
    fn match_user_patterns_follow_sshd() {
        for (criteria, user, expected) in [
            ("all", "dev", Some(true)),
            ("User dev", "dev", Some(true)),
            ("user dev", "dev", Some(true)),
            ("User other", "dev", Some(false)),
            ("User d*", "dev", Some(true)),
            ("User ?ev", "dev", Some(true)),
            ("User a,b,dev", "dev", Some(true)),
            ("User *,!dev", "dev", Some(false)),
            ("User !root", "dev", Some(false)),
            ("User dev Group wheel", "dev", None),
            ("User other Group wheel", "dev", Some(false)),
            ("Group wheel", "dev", None),
            ("Address 10.0.0.0/8", "dev", None),
            ("Host *.example.net", "dev", None),
            ("User", "dev", None),
            ("", "dev", None),
        ] {
            let words: Vec<String> = criteria.split_whitespace().map(str::to_owned).collect();
            assert_eq!(applies(&words, user), expected, "{criteria}");
        }
    }

    #[test]
    fn the_port_comes_from_the_first_port_line() {
        for (config, port) in [
            ("Port 2222\n", Some(2222)),
            ("# Port 22\nport 2200\nPort 2201\n", Some(2200)),
            ("  Port   22  \n", Some(22)),
            ("PasswordAuthentication no\n", None),
            ("Port x\nPort 99\n", Some(99)),
            ("Port 0\n", None),
            ("", None),
            ("PortFoo 22\n", None),
            ("Port=2200\n", Some(2200)),
        ] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("sshd_config"), config).unwrap();
            assert_eq!(
                read_sshd_config(dir.path()).unwrap().port,
                port,
                "{config:?}"
            );
        }
    }

    #[test]
    fn programs_are_found_in_the_given_directories_in_order() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        for dir in [&first, &second] {
            let path = dir.path().join("herdr");
            std::fs::write(&path, "").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let dirs = [first.path().to_path_buf(), second.path().to_path_buf()];
        assert_eq!(
            find_program("herdr", &dirs),
            Some(first.path().join("herdr"))
        );
        assert_eq!(find_program("nope", &dirs), None);
    }

    #[test]
    fn the_program_search_adds_the_usual_places_once() {
        let home = Path::new("/home/x");
        let dirs = program_dirs(Some(std::ffi::OsStr::new("/usr/bin:/custom")), home);
        assert_eq!(dirs[0], PathBuf::from("/usr/bin"));
        assert_eq!(dirs[1], PathBuf::from("/custom"));
        assert_eq!(
            dirs.iter()
                .filter(|d| d.as_path() == Path::new("/usr/bin"))
                .count(),
            1
        );
        assert!(dirs.contains(&home.join(".cargo/bin")));
    }
}
