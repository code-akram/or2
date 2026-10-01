//! The checks `or2-pair` reports before it pairs. They only look: nothing is changed.
//!
//! - is `sshd` answering on the chosen port (and how to turn it on when it is not),
//! - can `~/.ssh/authorized_keys` be written (and would `sshd` honour it),
//! - are tmux, herdr and mosh-server installed (all optional),
//! - a firewall hint for mosh's UDP ports.

use std::path::{Path, PathBuf};

use crate::account::Account;
use crate::authorized_keys::{self, Writable};
use crate::net::Net;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Info,
}

impl Level {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Ok => "ok  ",
            Self::Warn => "warn",
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
    pub net: &'a dyn Net,
    /// Directories searched for programs: `PATH` plus the usual user and package-manager ones.
    pub program_dirs: &'a [PathBuf],
    pub platform: Platform,
}

pub fn run(input: &CheckInput<'_>) -> Vec<Check> {
    let mut out = Vec::new();
    out.push(sshd(input));
    out.extend(authorized_keys(input.account));
    let mosh = find_program("mosh-server", input.program_dirs);
    for (name, note) in [
        ("tmux", "optional: or2 can attach to its sessions"),
        ("herdr", "optional: the agents inbox needs it"),
        ("mosh-server", "optional: terminals survive network changes"),
    ] {
        let found = if name == "mosh-server" {
            mosh.clone()
        } else {
            find_program(name, input.program_dirs)
        };
        out.push(match found {
            Some(path) => check(Level::Ok, format!("{name}: {}", path.display())),
            None => check(Level::Info, format!("{name}: not found ({note})")),
        });
    }
    if let Some(hint) = firewall_hint(input.platform, mosh.is_some()) {
        out.push(check(Level::Info, hint));
    }
    out
}

fn sshd(input: &CheckInput<'_>) -> Check {
    match input.net.probe_ssh(input.ssh_port) {
        Ok(banner) if banner.starts_with("SSH-") => check(
            Level::Ok,
            format!("sshd is answering on port {} ({banner})", input.ssh_port),
        ),
        Ok(_) => check(
            Level::Warn,
            format!(
                "something answers on port {} but it does not look like sshd; pass --ssh-port if sshd listens elsewhere",
                input.ssh_port
            ),
        ),
        Err(_) => check(
            Level::Warn,
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

fn authorized_keys(account: &Account) -> Vec<Check> {
    let file = authorized_keys::path(&account.home);
    vec![match authorized_keys::writable(account) {
        Writable::Yes => check(
            Level::Ok,
            format!("{} can be written and sshd will honour it", file.display()),
        ),
        Writable::No(why) => check(Level::Warn, why),
    }]
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
    for extra in [
        home.join(".local/bin"),
        home.join(".cargo/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ] {
        if !dirs.contains(&extra) {
            dirs.push(extra);
        }
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::PairListener;
    use std::io;
    use std::net::IpAddr;

    struct FakeNet(io::Result<String>);

    impl Net for FakeNet {
        fn probe_ssh(&self, _: u16) -> io::Result<String> {
            match &self.0 {
                Ok(banner) => Ok(banner.clone()),
                Err(error) => Err(io::Error::new(error.kind(), "x")),
            }
        }
        fn listen(&self, _: &[IpAddr], _: u16) -> io::Result<Box<dyn PairListener>> {
            unreachable!("checks never listen")
        }
    }

    fn input<'a>(
        account: &'a Account,
        net: &'a FakeNet,
        dirs: &'a [PathBuf],
        platform: Platform,
    ) -> CheckInput<'a> {
        CheckInput {
            account,
            ssh_port: 22,
            net,
            program_dirs: dirs,
            platform,
        }
    }

    #[test]
    fn a_healthy_host_reports_ok_and_changes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        for name in ["tmux", "mosh-server"] {
            let path = bin.path().join(name);
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let dirs = [bin.path().to_path_buf()];
        let net = FakeNet(Ok("SSH-2.0-OpenSSH_9.9".into()));
        let checks = run(&input(
            &Account::new("t", home.path()),
            &net,
            &dirs,
            Platform::Linux,
        ));
        assert_eq!(checks[0].level, Level::Ok);
        assert!(checks[0].text.contains("OpenSSH_9.9"));
        assert!(
            checks
                .iter()
                .any(|c| c.text.starts_with("tmux:") && c.level == Level::Ok)
        );
        assert!(
            checks
                .iter()
                .any(|c| c.text.starts_with("herdr: not found") && c.level == Level::Info)
        );
        let firewall = checks.last().unwrap();
        assert!(firewall.text.contains("60000-61000") && firewall.text.contains("ufw"));
        // Looking changed nothing: no ~/.ssh was created.
        assert!(!home.path().join(".ssh").exists());
    }

    #[test]
    fn a_stopped_sshd_gets_a_platform_hint() {
        let home = tempfile::tempdir().unwrap();
        let net = FakeNet(Err(io::ErrorKind::ConnectionRefused.into()));
        for (platform, word) in [
            (Platform::MacOs, "Remote Login"),
            (Platform::Linux, "systemctl"),
            (Platform::Windows, "OpenSSH Server"),
        ] {
            let checks = run(&input(&Account::new("t", home.path()), &net, &[], platform));
            assert_eq!(checks[0].level, Level::Warn);
            assert!(checks[0].text.contains(word), "{}", checks[0].text);
        }
    }

    #[test]
    fn something_that_is_not_sshd_is_a_warning() {
        let home = tempfile::tempdir().unwrap();
        let net = FakeNet(Ok("HTTP/1.1 400".into()));
        let checks = run(&input(
            &Account::new("t", home.path()),
            &net,
            &[],
            Platform::Linux,
        ));
        assert_eq!(checks[0].level, Level::Warn);
        assert!(checks[0].text.contains("--ssh-port"));
    }

    #[test]
    fn an_unwritable_home_is_reported() {
        let net = FakeNet(Ok("SSH-2.0-x".into()));
        let missing = Path::new("/nonexistent-or2-home");
        let checks = run(&input(
            &Account::new("t", missing),
            &net,
            &[],
            Platform::Linux,
        ));
        assert!(
            checks
                .iter()
                .any(|c| c.level == Level::Warn && c.text.contains("not writable"))
        );
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
