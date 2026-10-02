//! The checks `or2-pair` reports before it pairs. They only look: nothing is changed.
//!
//! - is `sshd` answering on the chosen port, which version it is (the banner decides which
//!   `authorized_keys` options are used; see `bootstrap`), and how to turn it on when it is not,
//! - can `~/.ssh/authorized_keys` be written (and would `sshd` honour it),
//! - a best-effort read of `sshd_config` and the files it `Include`s,
//! - the login shell (sshd runs the pairing command through it) and the path of this program,
//! - leftover pairing keys of earlier runs,
//! - are tmux, herdr and mosh-server installed (all optional),
//! - a firewall hint for mosh's UDP ports.
//!
//! A [`Level::Fail`] is something that makes automatic pairing impossible here (the run refuses
//! and points to `--manual`); a [`Level::Warn`] is something that may get in the way.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use crate::account::Account;
use crate::authorized_keys::{self, Writable};
use crate::bootstrap::{self, Dialect, DialectError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    /// Automatic pairing cannot work; with `pairing` off this is reported as a warning.
    Fail,
    Info,
}

impl Level {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Ok => "ok  ",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Info => "info",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub level: Level,
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
    /// Whether this run pairs automatically (so that what blocks it is a `Fail`). Off for
    /// `--manual` and for platforms that install no keys.
    pub pairing: bool,
    /// Whether keys are installed by hand on this platform: `authorized_keys` is then neither
    /// opened nor inspected.
    pub manual_keys: bool,
    /// Pairing ids of earlier runs that are over: their leftovers are reported, not removed.
    pub stale: &'a [bootstrap::PairingId],
}

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
            "authorized_keys is not checked: or2-pair does not write it on this platform, so you add the phone's key by hand (the instructions follow the pairing code)",
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
    for (name, note) in [
        ("tmux", "optional: or2 can attach to its sessions"),
        ("herdr", "optional: the agents inbox needs it"),
        ("mosh-server", "optional: terminals survive network changes"),
    ] {
        let path = if name == "mosh-server" {
            mosh.clone()
        } else {
            find_program(name, input.program_dirs)
        };
        match path {
            Some(_) => found.push(name),
            None => out.push(check(Level::Info, format!("{name}: not found ({note})"))),
        }
    }
    if !found.is_empty() {
        out.insert(
            out.len() - (3 - found.len()),
            check(Level::Ok, format!("{} found", found.join(", "))),
        );
    }
    if let Some(hint) = firewall_hint(input.platform, mosh.is_some()) {
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
                        "the SSH server on port {port} is {what}, not OpenSSH: pairing needs OpenSSH's authorized_keys options; use --manual"
                    ),
                ),
                Err(DialectError::NoBanner) => check(
                    blocking,
                    format!("sshd on port {port} showed no version banner; use --manual"),
                ),
                Ok(Dialect::Expiry) => check(
                    Level::Ok,
                    format!("sshd is answering on port {port} ({what})"),
                ),
                Ok(_) => check(
                    Level::Warn,
                    format!(
                        "sshd is answering on port {port} ({what}); {}",
                        Dialect::Restrict.note().unwrap_or_default()
                    ),
                ),
            }
        }
        Ok(_) => check(
            blocking,
            format!(
                "something answers on port {} but it does not look like sshd; pass --ssh-port if sshd listens elsewhere",
                input.ssh_port
            ),
        ),
        Err(_) => check(
            blocking,
            format!(
                "sshd is not answering on port {}: {}",
                input.ssh_port,
                sshd_hint(input.platform)
            ),
        ),
    }
}

fn sshd_hint(platform: Platform) -> &'static str {
    match platform {
        Platform::MacOs => {
            "turn on Remote Login (System Settings > General > Sharing > Remote Login)"
        }
        Platform::Linux => {
            "start it (sudo systemctl enable --now sshd, or ssh on Debian and Ubuntu)"
        }
        Platform::Windows => {
            "install and start OpenSSH Server (Settings > Optional features, then Start-Service sshd)"
        }
        Platform::Other => "start your SSH server",
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

fn shell(input: &CheckInput<'_>, blocking: Level) -> Vec<Check> {
    let shell = input.account.shell.as_deref();
    match bootstrap::check_login_shell(shell) {
        Ok(()) => shell
            .map(|shell| {
                check(
                    Level::Ok,
                    format!("login shell {shell} can run the pairing command"),
                )
            })
            .into_iter()
            .collect(),
        Err(error) => vec![check(blocking, error.to_string())],
    }
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
                "cannot tell where this program is installed ({why}); sshd must be given its absolute path"
            ),
        )],
    }
}

// --- sshd_config, best effort -----------------------------------------------------------------

/// What `sshd_config` (and what it includes) says that matters for pairing, outside `Match`
/// blocks and with the first value of a keyword winning as in sshd.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SshdConfig {
    pub port: Option<u16>,
    /// The first `PubkeyAuthentication`: `Some(false)` is `no`.
    pub pubkey_authentication: Option<bool>,
    /// The words of the first `AuthorizedKeysFile`.
    pub authorized_keys_file: Option<Vec<String>>,
    pub authorized_keys_command: bool,
    pub force_command: bool,
    pub authentication_methods: Option<String>,
}

const INCLUDE_DEPTH: usize = 4;

/// Reads `<etc>/sshd_config` and its `Include`s. `None` when it cannot be read (not root, not
/// there): that is not a finding.
pub fn read_sshd_config(etc_ssh: &Path) -> Option<SshdConfig> {
    let text = std::fs::read_to_string(etc_ssh.join("sshd_config")).ok()?;
    let mut config = SshdConfig::default();
    let mut seen = HashSet::new();
    let mut in_match = false;
    parse_config(&text, etc_ssh, 0, &mut config, &mut seen, &mut in_match);
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

/// Whether `name` matches an `Include` pattern's file part (`*` and `?` only).
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

fn included_files(pattern: &str, etc_ssh: &Path) -> Vec<PathBuf> {
    let path = if Path::new(pattern).is_absolute() {
        PathBuf::from(pattern)
    } else {
        etc_ssh.join(pattern)
    };
    let (Some(dir), Some(file)) = (path.parent(), path.file_name().and_then(|f| f.to_str())) else {
        return Vec::new();
    };
    if !file.contains(['*', '?']) {
        return vec![path];
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
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
    files
}

fn parse_config(
    text: &str,
    etc_ssh: &Path,
    depth: usize,
    config: &mut SshdConfig,
    seen: &mut HashSet<PathBuf>,
    in_match: &mut bool,
) {
    for line in text.lines() {
        let Some((keyword, args)) = directive(line) else {
            continue;
        };
        if keyword == "match" {
            *in_match = true;
            continue;
        }
        if *in_match {
            continue;
        }
        let first = args.first().map(String::as_str);
        match keyword.as_str() {
            "include" if depth < INCLUDE_DEPTH => {
                for pattern in &args {
                    for file in included_files(pattern, etc_ssh) {
                        if !seen.insert(file.clone()) {
                            continue;
                        }
                        if let Ok(included) = std::fs::read_to_string(&file) {
                            // A `Match` inside an included file ends with that file.
                            let mut file_match = false;
                            parse_config(
                                &included,
                                etc_ssh,
                                depth + 1,
                                config,
                                seen,
                                &mut file_match,
                            );
                        }
                    }
                }
            }
            "port" if config.port.is_none() => {
                config.port = first.and_then(|p| p.parse().ok()).filter(|p| *p != 0);
            }
            "pubkeyauthentication" if config.pubkey_authentication.is_none() && first.is_some() => {
                config.pubkey_authentication = Some(first != Some("no"));
            }
            "authorizedkeysfile" if config.authorized_keys_file.is_none() => {
                config.authorized_keys_file = Some(args.clone());
            }
            "authorizedkeyscommand" if first.is_some_and(|c| c != "none") => {
                config.authorized_keys_command = true;
            }
            "forcecommand" if first.is_some_and(|c| c != "none") => {
                config.force_command = true;
            }
            "authenticationmethods" if config.authentication_methods.is_none() => {
                config.authentication_methods = Some(args.join(" "));
            }
            _ => {}
        }
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

fn config(input: &CheckInput<'_>, blocking: Level) -> Vec<Check> {
    let Some(config) = read_sshd_config(input.etc_ssh) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let try_manual = "; pairing would likely fail here, use --manual";
    if config.pubkey_authentication == Some(false) {
        out.push(check(
            blocking,
            "sshd_config has `PubkeyAuthentication no`: sshd would not accept the phone's key at all; use --manual after enabling it",
        ));
    }
    if let Some(words) = &config.authorized_keys_file
        && !includes_default_keys(words, input.account)
    {
        out.push(check(
            Level::Warn,
            format!(
                "sshd_config's AuthorizedKeysFile ({}) does not include .ssh/authorized_keys{try_manual}",
                words.join(" ")
            ),
        ));
    }
    if config.authorized_keys_command {
        out.push(check(
            Level::Warn,
            format!("sshd_config sets an AuthorizedKeysCommand{try_manual}"),
        ));
    }
    if config.force_command {
        out.push(check(
            Level::Warn,
            format!(
                "sshd_config sets a ForceCommand, which would run instead of the pairing command{try_manual}"
            ),
        ));
    }
    if let Some(methods) = &config.authentication_methods
        && !key_alone_is_enough(methods)
    {
        out.push(check(
            Level::Warn,
            format!(
                "sshd_config's AuthenticationMethods ({methods}) needs more than a key{try_manual}"
            ),
        ));
    }
    out
}

fn firewall_hint(platform: Platform, mosh: bool) -> Option<String> {
    let ports = "mosh needs UDP ports 60000-61000 open on this host";
    let how = match platform {
        Platform::MacOs => "allow mosh-server in System Settings > Network > Firewall",
        Platform::Linux => {
            "for ufw: sudo ufw allow 60000:61000/udp; for firewalld: sudo firewall-cmd --add-port=60000-61000/udp"
        }
        Platform::Windows | Platform::Other => return None,
    };
    mosh.then(|| format!("{ports} if a firewall is on ({how})"))
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

    struct Setup {
        home: tempfile::TempDir,
        etc: tempfile::TempDir,
        probe: io::Result<String>,
        exe: Result<PathBuf, String>,
        dirs: Vec<PathBuf>,
        platform: Platform,
        pairing: bool,
        account_shell: Option<String>,
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
                pairing: true,
                account_shell: Some("/bin/bash".into()),
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
                pairing: self.pairing,
                manual_keys: false,
                stale: &[],
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
        for name in ["tmux", "mosh-server"] {
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
                .any(|c| c.text == "tmux, mosh-server found" && c.level == Level::Ok),
            "{checks:?}"
        );
        assert!(
            checks
                .iter()
                .any(|c| c.text.starts_with("herdr: not found") && c.level == Level::Info)
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
            (Platform::Linux, "systemctl"),
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
        let old = Setup::new(banner("SSH-2.0-OpenSSH_7.4p1 Debian-10")).run();
        assert_eq!(old[0].level, Level::Warn);
        assert!(
            old[0].text.contains("OpenSSH_7.4p1") && old[0].text.contains("cannot expire"),
            "{}",
            old[0].text
        );
        let current = Setup::new(banner("SSH-2.0-OpenSSH_7.7p1")).run();
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
            pairing: true,
            manual_keys: false,
            stale: &[],
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
            pairing: true,
            manual_keys: false,
            stale: &stale,
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
        // The others warn and suggest --manual.
        let home = setup.home.path().display().to_string();
        for (config, word) in [
            (
                "AuthorizedKeysFile /etc/ssh/keys/%u\n",
                "AuthorizedKeysFile",
            ),
            ("AuthorizedKeysFile none\n", "AuthorizedKeysFile"),
            (
                "AuthorizedKeysCommand /usr/bin/fetch %u\n",
                "AuthorizedKeysCommand",
            ),
            ("ForceCommand /bin/true\n", "ForceCommand"),
            (
                "AuthenticationMethods publickey,password\n",
                "AuthenticationMethods",
            ),
            (
                "AuthenticationMethods publickey,keyboard-interactive:pam\n",
                "AuthenticationMethods",
            ),
        ] {
            let (_, warnings) = warns(config);
            let hit = warnings.iter().find(|w| w.contains(word));
            assert!(
                hit.is_some_and(|w| w.contains("--manual")),
                "{config}: {warnings:?}"
            );
        }
        // And these are fine.
        for config in [
            "AuthorizedKeysFile .ssh/authorized_keys\n".to_owned(),
            "AuthorizedKeysFile .ssh/authorized_keys .ssh/authorized_keys2\n".to_owned(),
            "AuthorizedKeysFile %h/.ssh/authorized_keys\n".to_owned(),
            "AuthorizedKeysFile ~/.ssh/authorized_keys\n".to_owned(),
            format!("AuthorizedKeysFile {home}/.ssh/authorized_keys\n"),
            "AuthorizedKeysCommand none\n".to_owned(),
            "ForceCommand none\n".to_owned(),
            "AuthenticationMethods any\n".to_owned(),
            "AuthenticationMethods publickey\n".to_owned(),
            "AuthenticationMethods publickey password,publickey\n".to_owned(),
            "authorizedkeysfile=.ssh/authorized_keys\n".to_owned(),
            "# ForceCommand /bin/true\n".to_owned(),
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
    fn sshd_config_match_blocks_and_includes() {
        let setup = Setup::new(banner("SSH-2.0-OpenSSH_9.9"));
        // A ForceCommand inside a Match block is for someone else.
        setup.config("Port 2200\nMatch Group sftponly\n    ForceCommand internal-sftp\n    PubkeyAuthentication no\n");
        let config = read_sshd_config(setup.etc.path()).unwrap();
        assert_eq!(config.port, Some(2200));
        assert!(!config.force_command && config.pubkey_authentication.is_none());

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
        assert!(config.force_command && config.pubkey_authentication.is_none());
        assert_eq!(
            config.authentication_methods.as_deref(),
            Some("publickey,password")
        );

        // A Match block that ends an included file does not reach the main file.
        std::fs::write(dir.join("30-match.conf"), "Match User x\n").unwrap();
        setup.config("Include sshd_config.d/*.conf\nPubkeyAuthentication no\n");
        assert_eq!(
            read_sshd_config(setup.etc.path())
                .unwrap()
                .pubkey_authentication,
            Some(false)
        );

        // An include loop ends, and an unreadable file is no finding.
        setup.config("Include sshd_config\nInclude missing.conf\nPort 2201\n");
        assert_eq!(read_sshd_config(setup.etc.path()).unwrap().port, Some(2201));
        std::fs::remove_file(setup.etc.path().join("sshd_config")).unwrap();
        assert_eq!(read_sshd_config(setup.etc.path()), None);
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
