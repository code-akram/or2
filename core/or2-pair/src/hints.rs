//! The exact fix for a prerequisite that is missing or failing, for this host: how to turn sshd
//! on, the install command of its package manager, the rule for its firewall. `or2-pair` prints
//! these; it never runs them, and never `sudo`.
//!
//! What the host has is read once ([`HostFacts::detect`], from files under a root directory so
//! that tests can fake a host) and the hints are pure functions of it. The one exception to
//! "files only" is the macOS firewall, which has no file to read: its read-only
//! `socketfilterfw --get…` queries are run through [`Commands`], which tests replace with
//! captured outputs.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::checks::{Level, Platform, find_program};

/// The package manager that installs things on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Brew,
    Apt,
    Dnf,
    Yum,
    Pacman,
    Zypper,
    Apk,
}

impl PackageManager {
    /// The program each is found by, in the order they are looked for on Linux (a host normally
    /// has one; `dnf` before `yum`, which newer Fedora and RHEL keep as an alias).
    const LINUX: [(&'static str, PackageManager); 6] = [
        ("apt-get", Self::Apt),
        ("dnf", Self::Dnf),
        ("yum", Self::Yum),
        ("zypper", Self::Zypper),
        ("pacman", Self::Pacman),
        ("apk", Self::Apk),
    ];

    /// The command that installs `package`, with `sudo` unless this already runs as root
    /// (Homebrew refuses to run as root, so never with it).
    pub fn install(self, package: &str, superuser: bool) -> String {
        let sudo = if superuser { "" } else { "sudo " };
        match self {
            Self::Brew => format!("brew install {package}"),
            Self::Apt => format!("{sudo}apt install {package}"),
            Self::Dnf => format!("{sudo}dnf install {package}"),
            Self::Yum => format!("{sudo}yum install {package}"),
            Self::Pacman => format!("{sudo}pacman -S {package}"),
            Self::Zypper => format!("{sudo}zypper install {package}"),
            Self::Apk => format!("{sudo}apk add {package}"),
        }
    }

    /// The package of the OpenSSH server.
    fn openssh_server(self) -> &'static str {
        match self {
            Self::Pacman | Self::Apk | Self::Brew => "openssh",
            Self::Apt | Self::Dnf | Self::Yum | Self::Zypper => "openssh-server",
        }
    }

    /// The systemd unit the OpenSSH server package installs: `ssh` on Debian and its
    /// derivatives, `sshd` everywhere else.
    fn sshd_unit(self) -> &'static str {
        match self {
            Self::Apt => "ssh",
            _ => "sshd",
        }
    }

    /// Whether this manager's package database under `root` lists the OpenSSH server: `None`
    /// when it cannot be read here (the RPM database is not a text format, Homebrew is not
    /// asked).
    fn has_openssh_server(self, root: &Path) -> Option<bool> {
        match self {
            // dpkg keeps a file list for every installed package (removed ones lose it).
            Self::Apt => root
                .join("var/lib/dpkg/info")
                .is_dir()
                .then(|| exists(&root.join("var/lib/dpkg/info/openssh-server.list"))),
            // pacman: one directory per installed package, `<name>-<version>-<release>`.
            Self::Pacman => {
                let entries = std::fs::read_dir(root.join("var/lib/pacman/local")).ok()?;
                Some(entries.flatten().any(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .and_then(|name| name.strip_prefix("openssh-"))
                        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
                }))
            }
            // apk: `P:<name>` starts each installed package's record.
            Self::Apk => std::fs::read_to_string(root.join("lib/apk/db/installed"))
                .ok()
                .map(|text| text.lines().any(|line| line == "P:openssh-server")),
            Self::Dnf | Self::Yum | Self::Zypper | Self::Brew => None,
        }
    }
}

/// What starts services on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceManager {
    Systemd,
    OpenRc,
}

/// A distribution configured by one system description, where packages and services are not
/// added one by one and programs live outside the usual directories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Declarative {
    NixOs,
    Guix,
}

/// A host firewall that is on (or enabled to start).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Firewall {
    Ufw,
    Firewalld,
    Nftables,
}

/// What the macOS application firewall does, as `socketfilterfw` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacFirewall {
    Off,
    On {
        /// "Block all incoming connections", which overrides every application rule; `None`
        /// when its answer could not be read.
        block_all: Option<bool>,
        /// The rule for mosh-server, when mosh-server was found.
        mosh_server: Option<MoshServerRule>,
    },
}

/// The firewall's rule for mosh-server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoshServerRule {
    /// mosh-server's real path (symbolic links resolved: Homebrew's `bin/mosh-server` links into
    /// `Cellar/`), which is what the firewall's rules name.
    pub path: PathBuf,
    /// `None` when the answer could not be read.
    pub rule: Option<AppRule>,
}

/// What the firewall's list says of one program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppRule {
    /// Incoming connections are permitted.
    Allowed,
    /// Incoming connections are blocked.
    Blocked,
    /// Not in the list: macOS would ask in a dialog, which nobody answers for a program started
    /// over SSH, so its incoming UDP is dropped.
    NotListed,
}

/// The macOS firewall's command-line tool. Only its read-only queries are run.
pub const SOCKETFILTERFW: &str = "/usr/libexec/ApplicationFirewall/socketfilterfw";

/// Runs a read-only command and returns what it printed (standard output, then standard error),
/// whatever its exit status, or `None` when it could not be run or did not finish in time. The
/// seam that lets tests feed captured outputs instead of running anything.
pub trait Commands {
    fn run(&self, program: &Path, args: &[&OsStr]) -> Option<String>;
}

/// The real [`Commands`]: the program is run directly (no shell), with no input, for at most
/// `timeout`, and at most [`COMMAND_OUTPUT_LIMIT`] bytes of each output stream are kept.
#[derive(Debug, Clone, Copy)]
pub struct SystemCommands {
    pub timeout: Duration,
}

impl Default for SystemCommands {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(3),
        }
    }
}

/// At most this much of a command's output stream is kept.
pub const COMMAND_OUTPUT_LIMIT: u64 = 64 * 1024;

impl Commands for SystemCommands {
    fn run(&self, program: &Path, args: &[&OsStr]) -> Option<String> {
        use std::io::Read;
        use std::process::{Command, Stdio};
        use std::sync::mpsc::{self, Receiver};

        fn reader(stream: impl Read + Send + 'static) -> Receiver<Vec<u8>> {
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let mut text = Vec::new();
                let _ = stream.take(COMMAND_OUTPUT_LIMIT).read_to_end(&mut text);
                let _ = sender.send(text);
            });
            receiver
        }

        let deadline = Instant::now() + self.timeout;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?;
        let outputs = [
            child.stdout.take().map(reader),
            child.stderr.take().map(reader),
        ];
        let finished = loop {
            match child.try_wait() {
                Ok(Some(_)) => break true,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => break false,
            }
        };
        if !finished {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        // Something it left running could still hold a pipe: the reading is inside the same
        // time limit.
        let mut text = Vec::new();
        for output in outputs.into_iter().flatten() {
            let left = deadline.saturating_duration_since(Instant::now());
            text.extend(output.recv_timeout(left).ok()?);
        }
        Some(String::from_utf8_lossy(&text).into_owned())
    }
}

/// The lower-case words of `text` (letters only), for reading answers whose exact wording is not
/// pinned down.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphabetic())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Whether the words say on or off: `Some` only when they say one and not the other.
fn on_or_off(words: &[String]) -> Option<bool> {
    let has = |list: &[&str]| words.iter().any(|word| list.contains(&word.as_str()));
    match (
        has(&["enabled", "on", "active"]),
        has(&["disabled", "off", "inactive"]),
    ) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

/// The lines that are not about stealth mode or logging (which say nothing of what is
/// blocked).
fn relevant_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter(|line| {
        let line = line.to_lowercase();
        !line.contains("stealth") && !line.contains("logging")
    })
}

/// `socketfilterfw --getglobalstate`: whether the firewall is on, and whether that state is
/// "block all" (state 2). `(State = N)` decides when it is there (0 off, 1 on, 2 block all);
/// otherwise the words do (`Firewall is enabled.`). `None` when it cannot be read.
pub fn parse_global_state(text: &str) -> Option<(bool, bool)> {
    let text: String = relevant_lines(text).collect::<Vec<_>>().join("\n");
    let lower = text.to_lowercase();
    if let Some(at) = lower.find("state") {
        let rest = lower[at + "state".len()..].trim_start();
        if let Some(value) = rest.strip_prefix('=').map(str::trim_start) {
            let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
            match digits.as_str() {
                "0" => return Some((false, false)),
                "1" => return Some((true, false)),
                "2" => return Some((true, true)),
                "" => {}
                _ => return None,
            }
        }
    }
    let words = words(&text);
    if words.iter().any(|word| word == "blocking") && words.iter().any(|word| word == "all") {
        return Some((true, true));
    }
    on_or_off(&words).map(|on| (on, false))
}

/// `socketfilterfw --getblockall`: `Firewall has block all state set to enabled.` (older:
/// `Block all ENABLED!`). `None` when it cannot be read.
pub fn parse_block_all(text: &str) -> Option<bool> {
    let words = words(&relevant_lines(text).collect::<Vec<_>>().join("\n"));
    if !words.iter().any(|word| word == "block") {
        return None;
    }
    on_or_off(&words)
}

/// `socketfilterfw --getappblocked <path>`: the program's rule. The path is taken out of the
/// text first (a directory could be called `blocked`). `None` when it cannot be read.
pub fn parse_app_rule(text: &str, path: &Path) -> Option<AppRule> {
    let text = text.replace(&*path.to_string_lossy(), " ");
    let lower = text.to_lowercase();
    if lower.contains("not part of the firewall")
        || lower.contains("not in the firewall")
        || lower.contains("not found in the firewall")
    {
        return Some(AppRule::NotListed);
    }
    let words = words(&text);
    let has = |list: &[&str]| words.iter().any(|word| list.contains(&word.as_str()));
    match (
        has(&["permitted", "allowed", "allow", "unblocked"]),
        has(&["blocked", "blocking", "denied"]),
    ) {
        (true, false) => Some(AppRule::Allowed),
        (false, true) => Some(AppRule::Blocked),
        _ => None,
    }
}

/// Asks `socketfilterfw` (under `root`) what the macOS firewall does to mosh-server, found in
/// `dirs` and resolved to its real path. `None` when the tool is missing or its first answer
/// cannot be read: then nothing is known, and nothing is said.
fn mac_firewall(root: &Path, dirs: &[PathBuf], commands: &dyn Commands) -> Option<MacFirewall> {
    let tool = root.join(SOCKETFILTERFW.trim_start_matches('/'));
    let ask = |args: &[&OsStr]| commands.run(&tool, args);
    let (on, state_blocks_all) = parse_global_state(&ask(&[OsStr::new("--getglobalstate")])?)?;
    if !on {
        return Some(MacFirewall::Off);
    }
    let block_all = ask(&[OsStr::new("--getblockall")])
        .and_then(|text| parse_block_all(&text))
        .map(|blocks| blocks || state_blocks_all)
        .or(state_blocks_all.then_some(true));
    let mosh_server = find_program("mosh-server", dirs)
        .and_then(|path| std::fs::canonicalize(path).ok())
        .map(|path| {
            let rule = ask(&[OsStr::new("--getappblocked"), path.as_os_str()])
                .and_then(|text| parse_app_rule(&text, &path));
            MoshServerRule { path, rule }
        });
    Some(MacFirewall::On {
        block_all,
        mosh_server,
    })
}

/// What this host has, as far as the hints need it. `Default` knows nothing, and the hints then
/// fall back to generic advice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostFacts {
    pub package_manager: Option<PackageManager>,
    pub service_manager: Option<ServiceManager>,
    /// The SSH server's systemd unit as installed (`ssh` or `sshd`), when a unit file was found.
    pub sshd_unit: Option<String>,
    /// Whether an `sshd` program was found in the usual directories; `None` when it was not
    /// looked for.
    pub sshd_installed: Option<bool>,
    /// Whether the package database lists the OpenSSH server; `None` when it could not be read
    /// (or there is none). Only with `Some(false)` here is "not installed" certain.
    pub sshd_packaged: Option<bool>,
    /// NixOS or Guix: sshd is switched on in the system's configuration.
    pub declarative: Option<Declarative>,
    /// The firewall in front of mosh's UDP ports, the first found of ufw (enabled in its
    /// config), firewalld and nftables (their services enabled).
    pub firewall: Option<Firewall>,
    /// macOS: what the application firewall does to mosh-server; `None` when it could not be
    /// asked or its answer read (and on every other system).
    pub mac_firewall: Option<MacFirewall>,
    /// This runs as root: commands are shown without `sudo`.
    pub superuser: bool,
}

/// Where the systemd units of packages and of the administrator live, in the order looked at.
const UNIT_DIRS: [&str; 3] = [
    "usr/lib/systemd/system",
    "lib/systemd/system",
    "etc/systemd/system",
];

impl HostFacts {
    /// Reads the facts of the host whose file system starts at `root` (`/`, or a fake tree in
    /// tests). `program_dirs` are searched for package managers as well as the usual
    /// directories under `root`. Only looks: nothing is written, and the only thing run is the
    /// macOS firewall's read-only queries, through `commands` (never `sudo`).
    pub fn detect(
        platform: Platform,
        root: &Path,
        program_dirs: &[PathBuf],
        superuser: bool,
        commands: &dyn Commands,
    ) -> Self {
        let mut facts = Self {
            superuser,
            ..Self::default()
        };
        let under = |dirs: &[&str]| -> Vec<PathBuf> { dirs.iter().map(|d| root.join(d)).collect() };
        match platform {
            Platform::MacOs => {
                let mut dirs = under(&["opt/homebrew/bin", "usr/local/bin"]);
                dirs.extend_from_slice(program_dirs);
                if find_program("brew", &dirs).is_some() {
                    facts.package_manager = Some(PackageManager::Brew);
                }
                // mosh-server as the checks find it (PATH first), then Homebrew's.
                let mut mosh_dirs = program_dirs.to_vec();
                mosh_dirs.extend(under(&["opt/homebrew/bin", "usr/local/bin"]));
                facts.mac_firewall = mac_firewall(root, &mosh_dirs, commands);
            }
            Platform::Linux => {
                let mut dirs = program_dirs.to_vec();
                dirs.extend(under(&[
                    "usr/local/sbin",
                    "usr/local/bin",
                    "usr/sbin",
                    "usr/bin",
                    "sbin",
                    "bin",
                ]));
                facts.package_manager = PackageManager::LINUX
                    .iter()
                    .find(|(program, _)| find_program(program, &dirs).is_some())
                    .map(|(_, manager)| *manager);
                facts.sshd_installed = Some(find_program("sshd", &dirs).is_some());
                facts.sshd_packaged = facts
                    .package_manager
                    .and_then(|manager| manager.has_openssh_server(root));
                facts.declarative = declarative(root);
                facts.service_manager = if root.join("run/systemd/system").is_dir() {
                    Some(ServiceManager::Systemd)
                } else if root.join("run/openrc").is_dir() {
                    Some(ServiceManager::OpenRc)
                } else {
                    None
                };
                // Debian's package installs `ssh.service` (and an `sshd.service` alias only
                // once it is enabled), so `ssh` wins when both are there.
                facts.sshd_unit = ["ssh", "sshd"]
                    .into_iter()
                    .find(|unit| {
                        UNIT_DIRS
                            .iter()
                            .any(|dir| exists(&root.join(dir).join(format!("{unit}.service"))))
                    })
                    .map(str::to_owned);
                facts.firewall = linux_firewall(root);
            }
            Platform::Windows | Platform::Other => {}
        }
        facts
    }

    fn sudo(&self) -> &'static str {
        if self.superuser { "" } else { "sudo " }
    }
}

/// Whether something (a file, or a symbolic link even to nothing) is at `path`.
fn exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

/// Whether a service is enabled: systemd's `multi-user.target.wants` link, or an OpenRC
/// runlevel entry.
fn service_enabled(root: &Path, name: &str) -> bool {
    exists(&root.join(format!(
        "etc/systemd/system/multi-user.target.wants/{name}.service"
    ))) || ["default", "boot"]
        .iter()
        .any(|level| exists(&root.join(format!("etc/runlevels/{level}/{name}"))))
}

/// NixOS or Guix, by their marker file or the `ID` of `os-release`.
fn declarative(root: &Path) -> Option<Declarative> {
    if exists(&root.join("etc/NIXOS")) {
        return Some(Declarative::NixOs);
    }
    let text = std::fs::read_to_string(root.join("etc/os-release"))
        .or_else(|_| std::fs::read_to_string(root.join("usr/lib/os-release")))
        .ok()?;
    text.lines()
        .find_map(|line| line.trim().strip_prefix("ID="))
        .and_then(|id| match id.trim_matches(['"', '\'']) {
            "nixos" => Some(Declarative::NixOs),
            "guix" => Some(Declarative::Guix),
            _ => None,
        })
}

fn linux_firewall(root: &Path) -> Option<Firewall> {
    // ufw's service is enabled on every Ubuntu while ufw itself is off; its config says.
    let ufw = std::fs::read_to_string(root.join("etc/ufw/ufw.conf")).is_ok_and(|text| {
        text.lines().any(|line| {
            line.trim()
                .strip_prefix("ENABLED=")
                .is_some_and(|value| value.trim_matches(['"', '\'']).eq_ignore_ascii_case("yes"))
        })
    });
    if ufw {
        Some(Firewall::Ufw)
    } else if service_enabled(root, "firewalld") {
        Some(Firewall::Firewalld)
    } else if service_enabled(root, "nftables") {
        Some(Firewall::Nftables)
    } else {
        None
    }
}

/// The end of every sshd hint: the answer may simply be on another port.
const OTHER_PORT: &str = "; if sshd listens on another port, pass --ssh-port";

/// What to do when sshd does not answer: the end of "sshd is not answering on port N: …".
pub fn sshd(platform: Platform, facts: &HostFacts) -> String {
    let hint = match platform {
        Platform::MacOs => "turn on Remote Login (System Settings > General > Sharing > Remote Login), or run `sudo systemsetup -setremotelogin on` (that needs Full Disk Access for this terminal app, in System Settings > Privacy & Security)".to_owned(),
        Platform::Linux => linux_sshd(facts),
        Platform::Windows => {
            "install and start OpenSSH Server (Settings > Optional features, then Start-Service sshd)".to_owned()
        }
        Platform::Other => "start your SSH server".to_owned(),
    };
    format!("{hint}{OTHER_PORT}")
}

fn linux_sshd(facts: &HostFacts) -> String {
    let sudo = facts.sudo();
    match facts.declarative {
        // Neither installs or starts services by command: the system description does.
        Some(Declarative::NixOs) => {
            return format!(
                "on NixOS, set `services.openssh.enable = true;` in /etc/nixos/configuration.nix, then run `{sudo}nixos-rebuild switch`"
            );
        }
        Some(Declarative::Guix) => {
            return format!(
                "on Guix System, add `(service openssh-service-type)` to the services of your system configuration, then run `{sudo}guix system reconfigure` with it"
            );
        }
        None => {}
    }
    let unit = facts
        .sshd_unit
        .as_deref()
        .or_else(|| facts.package_manager.map(PackageManager::sshd_unit));
    let start = match (facts.service_manager, unit) {
        (Some(ServiceManager::OpenRc), _) => {
            format!("start it with `{sudo}rc-update add sshd && {sudo}rc-service sshd start`")
        }
        (Some(ServiceManager::Systemd), Some(unit)) => {
            format!("start it with `{sudo}systemctl enable --now {unit}`")
        }
        (Some(ServiceManager::Systemd), None) => format!(
            "start it with `{sudo}systemctl enable --now sshd` (the unit is `ssh` on Debian and Ubuntu)"
        ),
        // runit, s6, a container, WSL without systemd: no command is guessed.
        (None, _) => "start sshd with this host's service manager".to_owned(),
    };
    let install = facts
        .package_manager
        .map(|manager| {
            format!(
                "`{}`",
                manager.install(manager.openssh_server(), facts.superuser)
            )
        })
        .unwrap_or_else(|| "your package manager".to_owned());
    match (facts.sshd_installed, facts.sshd_packaged) {
        // Not found, and the package database agrees.
        (Some(false), Some(false)) => {
            format!("the OpenSSH server is not installed; install it with {install}, then {start}")
        }
        // Not found where it usually is, and nothing to confirm it.
        (Some(false), _) => format!(
            "the OpenSSH server does not seem to be installed (no sshd in the usual directories); if it is not, install it with {install}, then {start}"
        ),
        _ => start,
    }
}

/// The optional programs the checks look for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program {
    Tmux,
    Herdr,
    MoshServer,
}

/// How to install a program that was not found: what follows "install it: ".
pub fn install(program: Program, platform: Platform, facts: &HostFacts) -> String {
    let package = match program {
        // herdr's own instructions say how to install it; no package name is guessed.
        Program::Herdr => {
            return "see herdr's install docs (https://github.com/herdrdev/herdr)".to_owned();
        }
        Program::Tmux => "tmux",
        // `mosh-server` comes with the `mosh` package everywhere.
        Program::MoshServer => "mosh",
    };
    match (facts.package_manager, platform) {
        (Some(manager), _) => format!("`{}`", manager.install(package, facts.superuser)),
        (None, Platform::MacOs) => {
            format!("with Homebrew (https://brew.sh): `brew install {package}`")
        }
        (None, _) => format!("install the {package} package with your package manager"),
    }
}

/// mosh's UDP ports and how to open them in this host's firewall, when mosh-server is installed.
pub fn firewall(platform: Platform, facts: &HostFacts, mosh: bool) -> Option<String> {
    if !mosh {
        return None;
    }
    let ports = "mosh needs UDP ports 60000-61000 open on this host";
    let sudo = facts.sudo();
    Some(match platform {
        Platform::MacOs => format!(
            "{ports} if the firewall is on (allow mosh-server in System Settings > Network > Firewall)"
        ),
        Platform::Linux => match facts.firewall {
            Some(Firewall::Ufw) => format!("{ports}; ufw is on: `{sudo}ufw allow 60000:61000/udp`"),
            Some(Firewall::Firewalld) => format!(
                "{ports}; firewalld is enabled: `{sudo}firewall-cmd --permanent --add-port=60000-61000/udp && {sudo}firewall-cmd --reload`"
            ),
            Some(Firewall::Nftables) => format!(
                "{ports}; the nftables service is enabled: add a rule such as `{sudo}nft add rule inet filter input udp dport 60000-61000 accept` (with your ruleset's table and chain), and the same to /etc/nftables.conf to keep it"
            ),
            None => format!(
                "{ports}; no enabled ufw, firewalld or nftables was found here, so if a firewall blocks them it is another one (on this host, a router's or a cloud provider's): open them there"
            ),
        },
        Platform::Windows | Platform::Other => return None,
    })
}

/// Starts each further line of a check, under its text (after `  warn  `).
const CONTINUED: &str = "\n        ";

/// `path` for a POSIX shell: in double quotes, or in single quotes when it has a character
/// double quotes do not keep.
fn quoted(path: &Path) -> String {
    let text = path.to_string_lossy();
    if text.contains(['"', '$', '`', '\\', '!']) {
        format!("'{}'", text.replace('\'', r"'\''"))
    } else {
        format!("\"{text}\"")
    }
}

/// What the macOS firewall does to mosh's UDP, when it is known: `Ok` lines when it lets
/// mosh-server through, `Warn` lines with the exact fix when it would block it (pairing goes on:
/// SSH works). Empty when nothing is known (no `socketfilterfw`, an answer that cannot be read,
/// mosh-server not installed, not macOS): the generic [`firewall`] hint is printed then.
pub fn mac_firewall_checks(
    platform: Platform,
    facts: &HostFacts,
    mosh: bool,
) -> Vec<(Level, String)> {
    if platform != Platform::MacOs || !mosh {
        return Vec::new();
    }
    let (block_all, mosh_server) = match &facts.mac_firewall {
        None => return Vec::new(),
        Some(MacFirewall::Off) => {
            return vec![(
                Level::Ok,
                "the macOS firewall is off (it does not block mosh's UDP)".to_owned(),
            )];
        }
        Some(MacFirewall::On {
            block_all,
            mosh_server,
        }) => (*block_all, mosh_server.as_ref()),
    };
    let sudo = facts.sudo();
    let mut out = Vec::new();
    if block_all == Some(true) {
        out.push((
            Level::Warn,
            format!(
                "the macOS firewall blocks all incoming connections, which overrides any rule, so mosh cannot reach this host and terminals use SSH (it also blocks sharing services such as Remote Login from other machines); turn off \"Block all incoming connections\" in System Settings > Network > Firewall > Options, or run:{CONTINUED}{sudo}{SOCKETFILTERFW} --setblockall off"
            ),
        ));
    }
    if let Some(MoshServerRule {
        path,
        rule: Some(rule @ (AppRule::Blocked | AppRule::NotListed)),
    }) = mosh_server
    {
        let shown = path.display();
        let what = match rule {
            AppRule::Blocked => format!("the macOS firewall blocks mosh-server ({shown})"),
            _ => format!("the macOS firewall is on and has no rule for mosh-server ({shown})"),
        };
        let quoted = quoted(path);
        let upgrade = if path.to_string_lossy().contains("/Cellar/") {
            "`brew upgrade mosh` installs a new mosh-server at another path, so add the rule again after an upgrade (or2-pair --check shows it)"
        } else {
            "upgrading mosh replaces this binary, so add the rule again after an upgrade (or2-pair --check shows it)"
        };
        out.push((
            Level::Warn,
            format!(
                "{what}, so mosh cannot reach this host and terminals use SSH; allow it:{CONTINUED}{sudo}{SOCKETFILTERFW} --add {quoted}{CONTINUED}{sudo}{SOCKETFILTERFW} --unblockapp {quoted}{CONTINUED}{upgrade}"
            ),
        ));
    }
    if out.is_empty()
        && block_all == Some(false)
        && let Some(MoshServerRule {
            path,
            rule: Some(AppRule::Allowed),
        }) = mosh_server
    {
        out.push((
            Level::Ok,
            format!(
                "the macOS firewall is on and allows mosh-server ({})",
                path.display()
            ),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake host file system.
    struct Tree(tempfile::TempDir);

    impl Tree {
        fn new() -> Self {
            Self(tempfile::tempdir().unwrap())
        }

        fn root(&self) -> &Path {
            self.0.path()
        }

        fn file(&self, path: &str, text: &str) -> &Self {
            let path = self.root().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            self
        }

        fn dir(&self, path: &str) -> &Self {
            std::fs::create_dir_all(self.root().join(path)).unwrap();
            self
        }

        #[cfg(unix)]
        fn program(&self, path: &str) -> &Self {
            use std::os::unix::fs::PermissionsExt;
            self.file(path, "#!/bin/sh\n");
            std::fs::set_permissions(
                self.root().join(path),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
            self
        }

        #[cfg(unix)]
        fn link(&self, path: &str, target: &str) -> &Self {
            let path = self.root().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(target, path).unwrap();
            self
        }

        fn detect(&self, platform: Platform) -> HostFacts {
            self.detect_with(platform, &Captured::missing())
        }

        fn detect_with(&self, platform: Platform, commands: &Captured) -> HostFacts {
            HostFacts::detect(platform, self.root(), &[], false, commands)
        }
    }

    /// `socketfilterfw`'s answers as captured (by its first argument), or `None` for a missing
    /// tool; remembers every command it was asked to run.
    #[derive(Default)]
    struct Captured {
        global: Option<&'static str>,
        block_all: Option<&'static str>,
        /// `{}` stands for the path asked about.
        app: Option<&'static str>,
        asked: std::cell::RefCell<Vec<(PathBuf, Vec<String>)>>,
    }

    impl Captured {
        /// No `socketfilterfw` on this host.
        fn missing() -> Self {
            Self::default()
        }

        fn new(global: &'static str, block_all: &'static str, app: &'static str) -> Self {
            Self {
                global: Some(global),
                block_all: Some(block_all),
                app: Some(app),
                ..Self::default()
            }
        }

        fn asked(&self) -> Vec<Vec<String>> {
            self.asked
                .borrow()
                .iter()
                .map(|(_, args)| args.clone())
                .collect()
        }
    }

    impl Commands for Captured {
        fn run(&self, program: &Path, args: &[&OsStr]) -> Option<String> {
            let args: Vec<String> = args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            self.asked
                .borrow_mut()
                .push((program.to_path_buf(), args.clone()));
            match args.first().map(String::as_str) {
                Some("--getglobalstate") => self.global.map(str::to_owned),
                Some("--getblockall") => self.block_all.map(str::to_owned),
                Some("--getappblocked") => self.app.map(|text| text.replace("{}", &args[1])),
                _ => None,
            }
        }
    }

    // What `socketfilterfw` prints, as far as it could be gathered (no Mac ran these tests):
    // current macOS, and older wordings, which the parsing reads too.
    const OFF: &str = "Firewall is disabled. (State = 0)\n";
    const ON: &str = "Firewall is enabled. (State = 1)\n";
    const ON_STEALTH: &str = "Firewall is enabled. (State = 1)\nFirewall stealth mode is on\n";
    const STATE_BLOCK_ALL: &str =
        "Firewall is blocking all non-essential connections. (State = 2)\n";
    const BLOCK_ALL_OFF: &str = "Firewall has block all state set to disabled.\n";
    const BLOCK_ALL_ON: &str = "Firewall has block all state set to enabled.\n";
    const APP_ALLOWED: &str = "The application {} is permitted\n";
    const APP_BLOCKED: &str = "The application {} is blocked\n";
    const APP_NOT_LISTED: &str = "The application {} is not part of the firewall\n";

    /// Homebrew's layout on Apple silicon: `bin/mosh-server` is a relative link into `Cellar/`.
    #[cfg(unix)]
    fn homebrew_mosh(tree: &Tree) -> PathBuf {
        tree.program("opt/homebrew/Cellar/mosh/1.4.0_31/bin/mosh-server")
            .program("opt/homebrew/bin/brew")
            .link(
                "opt/homebrew/bin/mosh-server",
                "../Cellar/mosh/1.4.0_31/bin/mosh-server",
            );
        std::fs::canonicalize(
            tree.root()
                .join("opt/homebrew/Cellar/mosh/1.4.0_31/bin/mosh-server"),
        )
        .unwrap()
    }

    fn mac_on(block_all: Option<bool>, rule: Option<AppRule>, path: &str) -> HostFacts {
        HostFacts {
            package_manager: Some(PackageManager::Brew),
            mac_firewall: Some(MacFirewall::On {
                block_all,
                mosh_server: Some(MoshServerRule {
                    path: PathBuf::from(path),
                    rule,
                }),
            }),
            ..HostFacts::default()
        }
    }

    const CELLAR: &str = "/opt/homebrew/Cellar/mosh/1.4.0_31/bin/mosh-server";

    #[test]
    fn socketfilterfw_answers_are_parsed_in_every_wording() {
        for (text, expected) in [
            (OFF, Some((false, false))),
            (ON, Some((true, false))),
            (STATE_BLOCK_ALL, Some((true, true))),
            ("Firewall is enabled. (State = 2)\n", Some((true, true))),
            // Stealth mode drops pings and probes of closed ports, not mosh: ignored.
            (ON_STEALTH, Some((true, false))),
            (
                "Firewall is enabled.\nStealth mode enabled\n",
                Some((true, false)),
            ),
            (
                "Firewall is disabled.\nStealth mode disabled\n",
                Some((false, false)),
            ),
            ("Firewall is enabled.\n", Some((true, false))),
            ("Firewall is disabled.\n", Some((false, false))),
            ("firewall is ON\r\n", Some((true, false))),
            // Garbage, an error, an unknown state: nothing known.
            ("", None),
            (
                "socketfilterfw: unrecognized option `--getglobalstate'\n",
                None,
            ),
            ("Firewall is enabled. (State = 7)\n", None),
            ("Firewall is enabled or disabled\n", None),
            ("\u{fffd}\u{fffd}\u{0}", None),
        ] {
            assert_eq!(parse_global_state(text), expected, "{text:?}");
        }
        for (text, expected) in [
            (BLOCK_ALL_OFF, Some(false)),
            (BLOCK_ALL_ON, Some(true)),
            ("Block all DISABLED!\n", Some(false)),
            ("Block all ENABLED!\n", Some(true)),
            ("", None),
            ("Firewall is enabled. (State = 1)\n", None),
            ("Block all: who knows\n", None),
        ] {
            assert_eq!(parse_block_all(text), expected, "{text:?}");
        }
        let path = Path::new(CELLAR);
        for (text, expected) in [
            (APP_ALLOWED, Some(AppRule::Allowed)),
            (APP_BLOCKED, Some(AppRule::Blocked)),
            (APP_NOT_LISTED, Some(AppRule::NotListed)),
            (
                "Incoming connection to the application is permitted\n",
                Some(AppRule::Allowed),
            ),
            (
                "Incoming connection to the application is blocked\n",
                Some(AppRule::Blocked),
            ),
            (
                "The application at path ( {} ) is not part of the firewall\n",
                Some(AppRule::NotListed),
            ),
            ("", None),
            ("usage: socketfilterfw ...\n", None),
            ("The application {} is permitted and blocked\n", None),
        ] {
            let text = text.replace("{}", CELLAR);
            assert_eq!(parse_app_rule(&text, path), expected, "{text:?}");
        }
        // Captured from a Mac (macOS, Homebrew mosh 1.4.0, 2026-10-02): the firewall on with
        // stealth mode, block-all off, and mosh-server's rule, exit status 0.
        assert_eq!(
            parse_global_state("Firewall is enabled. (State = 1)\nFirewall stealth mode is on\n"),
            parse_global_state(ON)
        );
        assert_eq!(
            parse_block_all("Firewall has block all state set to disabled.\n"),
            Some(false)
        );
        let real = Path::new("/opt/homebrew/Cellar/mosh/1.4.0_43/bin/mosh-server");
        assert_eq!(
            parse_app_rule(
                "Incoming connection to /opt/homebrew/Cellar/mosh/1.4.0_43/bin/mosh-server is permitted.\n",
                real
            ),
            Some(AppRule::Allowed)
        );
        // A word in the path says nothing.
        let odd = Path::new("/Users/dev/blocked/bin/mosh-server");
        assert_eq!(
            parse_app_rule(
                "The application /Users/dev/blocked/bin/mosh-server is permitted\n",
                odd
            ),
            Some(AppRule::Allowed)
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_mac_firewall_is_asked_about_mosh_servers_real_path() {
        let tree = Tree::new();
        let real = homebrew_mosh(&tree);
        let tool = tree
            .root()
            .join("usr/libexec/ApplicationFirewall/socketfilterfw");

        // On, with Homebrew's link: the rule is asked of the Cellar binary.
        let commands = Captured::new(ON, BLOCK_ALL_OFF, APP_BLOCKED);
        let facts = tree.detect_with(Platform::MacOs, &commands);
        assert_eq!(
            facts.mac_firewall,
            Some(MacFirewall::On {
                block_all: Some(false),
                mosh_server: Some(MoshServerRule {
                    path: real.clone(),
                    rule: Some(AppRule::Blocked),
                }),
            })
        );
        assert_eq!(
            commands.asked(),
            [
                vec!["--getglobalstate".to_owned()],
                vec!["--getblockall".to_owned()],
                vec![
                    "--getappblocked".to_owned(),
                    real.to_string_lossy().into_owned()
                ],
            ]
        );
        assert!(
            commands
                .asked
                .borrow()
                .iter()
                .all(|(program, _)| *program == tool)
        );
        assert!(real.ends_with("opt/homebrew/Cellar/mosh/1.4.0_31/bin/mosh-server"));

        // Off: nothing more is asked.
        let commands = Captured::new(OFF, BLOCK_ALL_ON, APP_BLOCKED);
        assert_eq!(
            tree.detect_with(Platform::MacOs, &commands).mac_firewall,
            Some(MacFirewall::Off)
        );
        assert_eq!(commands.asked().len(), 1);

        // Allowed, with stealth mode on.
        let commands = Captured::new(ON_STEALTH, BLOCK_ALL_OFF, APP_ALLOWED);
        let Some(MacFirewall::On { mosh_server, .. }) =
            tree.detect_with(Platform::MacOs, &commands).mac_firewall
        else {
            panic!("on");
        };
        assert_eq!(mosh_server.unwrap().rule, Some(AppRule::Allowed));

        // Block all, by state 2 even when --getblockall cannot be read.
        let commands = Captured::new(STATE_BLOCK_ALL, "garbage", APP_ALLOWED);
        let Some(MacFirewall::On { block_all, .. }) =
            tree.detect_with(Platform::MacOs, &commands).mac_firewall
        else {
            panic!("on");
        };
        assert_eq!(block_all, Some(true));

        // Unreadable answers past the first: unknown, not a verdict.
        let commands = Captured::new(ON, "", "");
        assert_eq!(
            tree.detect_with(Platform::MacOs, &commands).mac_firewall,
            Some(MacFirewall::On {
                block_all: None,
                mosh_server: Some(MoshServerRule {
                    path: real.clone(),
                    rule: None
                }),
            })
        );

        // Missing tool, or a first answer that cannot be read: nothing known.
        assert_eq!(tree.detect(Platform::MacOs).mac_firewall, None);
        let commands = Captured::new("Segmentation fault\n", BLOCK_ALL_ON, APP_BLOCKED);
        assert_eq!(
            tree.detect_with(Platform::MacOs, &commands).mac_firewall,
            None
        );
        assert_eq!(commands.asked().len(), 1);

        // No mosh-server: no rule is asked about.
        let tree = Tree::new();
        let commands = Captured::new(ON, BLOCK_ALL_OFF, APP_BLOCKED);
        assert_eq!(
            tree.detect_with(Platform::MacOs, &commands).mac_firewall,
            Some(MacFirewall::On {
                block_all: Some(false),
                mosh_server: None
            })
        );
        assert_eq!(commands.asked().len(), 2);

        // Not macOS: nothing is run.
        let commands = Captured::new(ON, BLOCK_ALL_ON, APP_BLOCKED);
        assert_eq!(
            tree.detect_with(Platform::Linux, &commands).mac_firewall,
            None
        );
        assert!(commands.asked().is_empty());
    }

    #[test]
    fn the_mac_firewall_prints_the_exact_fix_when_it_would_block_mosh() {
        let tool = SOCKETFILTERFW;
        // Blocked, or not in the list: a warning with both commands and the upgrade note.
        for (rule, opening) in [
            (
                AppRule::Blocked,
                format!("the macOS firewall blocks mosh-server ({CELLAR})"),
            ),
            (
                AppRule::NotListed,
                format!("the macOS firewall is on and has no rule for mosh-server ({CELLAR})"),
            ),
        ] {
            let checks = mac_firewall_checks(
                Platform::MacOs,
                &mac_on(Some(false), Some(rule), CELLAR),
                true,
            );
            assert_eq!(checks.len(), 1, "{checks:?}");
            let (level, text) = &checks[0];
            assert_eq!(*level, Level::Warn);
            assert_eq!(
                *text,
                format!(
                    "{opening}, so mosh cannot reach this host and terminals use SSH; allow it:\n        sudo {tool} --add \"{CELLAR}\"\n        sudo {tool} --unblockapp \"{CELLAR}\"\n        `brew upgrade mosh` installs a new mosh-server at another path, so add the rule again after an upgrade (or2-pair --check shows it)"
                )
            );
        }
        // As root, no sudo; a path outside Homebrew gets the plain upgrade note; a path with
        // a `$` is single-quoted.
        let facts = HostFacts {
            superuser: true,
            ..mac_on(None, Some(AppRule::Blocked), "/opt/local/bin/mosh-server")
        };
        let (_, text) = &mac_firewall_checks(Platform::MacOs, &facts, true)[0];
        assert!(
            text.contains(&format!(
                "\n        {tool} --add \"/opt/local/bin/mosh-server\""
            )),
            "{text}"
        );
        assert!(!text.contains("sudo") && !text.contains("brew"), "{text}");
        assert!(text.ends_with("upgrading mosh replaces this binary, so add the rule again after an upgrade (or2-pair --check shows it)"));
        let odd = mac_on(
            Some(false),
            Some(AppRule::Blocked),
            "/Users/dev/it's $here/mosh-server",
        );
        let (_, text) = &mac_firewall_checks(Platform::MacOs, &odd, true)[0];
        assert!(
            text.contains(r"--add '/Users/dev/it'\''s $here/mosh-server'"),
            "{text}"
        );

        // Block all: its own warning, whatever the rule says, and the rule's too when needed.
        let checks = mac_firewall_checks(
            Platform::MacOs,
            &mac_on(Some(true), Some(AppRule::Allowed), CELLAR),
            true,
        );
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].0, Level::Warn);
        assert!(
            checks[0].1.starts_with(
                "the macOS firewall blocks all incoming connections, which overrides any rule"
            ) && checks[0]
                .1
                .ends_with(&format!("\n        sudo {tool} --setblockall off")),
            "{}",
            checks[0].1
        );
        let checks = mac_firewall_checks(
            Platform::MacOs,
            &mac_on(Some(true), Some(AppRule::NotListed), CELLAR),
            true,
        );
        assert_eq!(checks.len(), 2);
        assert!(checks.iter().all(|(level, _)| *level == Level::Warn));
        assert!(checks[1].1.contains("--unblockapp"));

        // Allowed (stealth mode or not): ok. Off: ok.
        let checks = mac_firewall_checks(
            Platform::MacOs,
            &mac_on(Some(false), Some(AppRule::Allowed), CELLAR),
            true,
        );
        assert_eq!(
            checks,
            [(
                Level::Ok,
                format!("the macOS firewall is on and allows mosh-server ({CELLAR})")
            )]
        );
        let off = HostFacts {
            mac_firewall: Some(MacFirewall::Off),
            ..HostFacts::default()
        };
        assert_eq!(
            mac_firewall_checks(Platform::MacOs, &off, true)[0].0,
            Level::Ok
        );

        // Nothing certain, nothing known, no mosh-server, not macOS: nothing (the generic
        // advice is printed instead).
        for (platform, facts, mosh) in [
            (
                Platform::MacOs,
                mac_on(None, Some(AppRule::Allowed), CELLAR),
                true,
            ),
            (Platform::MacOs, mac_on(Some(false), None, CELLAR), true),
            (Platform::MacOs, HostFacts::default(), true),
            (
                Platform::MacOs,
                mac_on(Some(true), Some(AppRule::Blocked), CELLAR),
                false,
            ),
            (
                Platform::Linux,
                mac_on(Some(true), Some(AppRule::Blocked), CELLAR),
                true,
            ),
        ] {
            assert!(
                mac_firewall_checks(platform, &facts, mosh).is_empty(),
                "{facts:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn system_commands_capture_both_outputs_and_give_up_in_time() {
        let commands = SystemCommands::default();
        let sh = Path::new("/bin/sh");
        let arg = |text: &'static str| OsStr::new(text);
        // Any exit status: socketfilterfw's are not documented.
        assert_eq!(
            commands.run(sh, &[arg("-c"), arg("echo out; echo err >&2; exit 3")]),
            Some("out\nerr\n".to_owned())
        );
        assert_eq!(
            commands.run(Path::new("/nonexistent/socketfilterfw"), &[]),
            None
        );
        let quick = SystemCommands {
            timeout: Duration::from_millis(200),
        };
        let started = Instant::now();
        assert_eq!(quick.run(sh, &[arg("-c"), arg("sleep 10")]), None);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    fn linux(manager: Option<PackageManager>) -> HostFacts {
        HostFacts {
            package_manager: manager,
            service_manager: Some(ServiceManager::Systemd),
            sshd_installed: Some(true),
            ..HostFacts::default()
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_package_manager_is_found_by_its_program() {
        for (program, expected) in [
            ("usr/bin/apt-get", PackageManager::Apt),
            ("usr/bin/dnf", PackageManager::Dnf),
            ("usr/bin/yum", PackageManager::Yum),
            ("usr/bin/pacman", PackageManager::Pacman),
            ("usr/bin/zypper", PackageManager::Zypper),
            // Alpine keeps apk in /sbin.
            ("sbin/apk", PackageManager::Apk),
        ] {
            let tree = Tree::new();
            tree.program(program);
            assert_eq!(
                tree.detect(Platform::Linux).package_manager,
                Some(expected),
                "{program}"
            );
        }
        // dnf wins over its yum alias.
        let tree = Tree::new();
        tree.program("usr/bin/yum").program("usr/bin/dnf");
        assert_eq!(
            tree.detect(Platform::Linux).package_manager,
            Some(PackageManager::Dnf)
        );
        // Homebrew on either kind of Mac; not looked for on Linux.
        for brew in ["opt/homebrew/bin/brew", "usr/local/bin/brew"] {
            let tree = Tree::new();
            tree.program(brew);
            assert_eq!(
                tree.detect(Platform::MacOs).package_manager,
                Some(PackageManager::Brew)
            );
            assert_eq!(tree.detect(Platform::Linux).package_manager, None);
        }
        // Found in PATH too.
        let tree = Tree::new();
        tree.program("custom/bin/pacman");
        let facts = HostFacts::detect(
            Platform::Linux,
            &tree.root().join("nowhere"),
            &[tree.root().join("custom/bin")],
            false,
            &Captured::missing(),
        );
        assert_eq!(facts.package_manager, Some(PackageManager::Pacman));
        // A file that is not executable is not a program.
        let tree = Tree::new();
        tree.file("usr/bin/dnf", "");
        assert_eq!(tree.detect(Platform::Linux).package_manager, None);
    }

    #[cfg(unix)]
    #[test]
    fn the_sshd_unit_and_service_manager_are_read_from_the_tree() {
        // Debian and Ubuntu: ssh.service, and the sshd.service alias once enabled.
        let tree = Tree::new();
        tree.dir("run/systemd/system")
            .file("usr/lib/systemd/system/ssh.service", "")
            .link(
                "etc/systemd/system/sshd.service",
                "/usr/lib/systemd/system/ssh.service",
            )
            .program("usr/sbin/sshd");
        let facts = tree.detect(Platform::Linux);
        assert_eq!(facts.service_manager, Some(ServiceManager::Systemd));
        assert_eq!(facts.sshd_unit.as_deref(), Some("ssh"));
        assert_eq!(facts.sshd_installed, Some(true));
        // Fedora, Arch, openSUSE: sshd.service (older Debian without usrmerge: /lib).
        for dir in ["usr/lib/systemd/system", "lib/systemd/system"] {
            let tree = Tree::new();
            tree.file(&format!("{dir}/sshd.service"), "");
            assert_eq!(
                tree.detect(Platform::Linux).sshd_unit.as_deref(),
                Some("sshd")
            );
        }
        // Alpine: OpenRC, no unit files, and no sshd yet.
        let tree = Tree::new();
        tree.dir("run/openrc").program("sbin/apk");
        let facts = tree.detect(Platform::Linux);
        assert_eq!(facts.service_manager, Some(ServiceManager::OpenRc));
        assert_eq!(facts.sshd_unit, None);
        assert_eq!(facts.sshd_installed, Some(false));
        // macOS: nothing of this is looked for.
        let facts = Tree::new().detect(Platform::MacOs);
        assert_eq!(facts.sshd_installed, None);
        assert_eq!(facts.service_manager, None);
    }

    #[cfg(unix)]
    #[test]
    fn the_active_firewall_is_read_from_the_tree() {
        let tree = Tree::new();
        assert_eq!(tree.detect(Platform::Linux).firewall, None);
        // ufw installed but off (Ubuntu's default) is no firewall.
        tree.file("etc/ufw/ufw.conf", "# comment\nENABLED=no\nLOGLEVEL=low\n");
        assert_eq!(tree.detect(Platform::Linux).firewall, None);
        tree.file("etc/ufw/ufw.conf", "ENABLED=yes\n");
        assert_eq!(tree.detect(Platform::Linux).firewall, Some(Firewall::Ufw));

        let tree = Tree::new();
        tree.link(
            "etc/systemd/system/multi-user.target.wants/firewalld.service",
            "/usr/lib/systemd/system/firewalld.service",
        );
        assert_eq!(
            tree.detect(Platform::Linux).firewall,
            Some(Firewall::Firewalld)
        );

        let tree = Tree::new();
        tree.link(
            "etc/systemd/system/multi-user.target.wants/nftables.service",
            "/usr/lib/systemd/system/nftables.service",
        );
        assert_eq!(
            tree.detect(Platform::Linux).firewall,
            Some(Firewall::Nftables)
        );
        // An enabled ufw is the front end in charge, whatever nftables does.
        tree.file("etc/ufw/ufw.conf", "ENABLED=\"yes\"\n");
        assert_eq!(tree.detect(Platform::Linux).firewall, Some(Firewall::Ufw));

        // OpenRC runlevels.
        let tree = Tree::new();
        tree.link("etc/runlevels/default/nftables", "/etc/init.d/nftables");
        assert_eq!(
            tree.detect(Platform::Linux).firewall,
            Some(Firewall::Nftables)
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_package_database_and_declarative_systems_are_read_from_the_tree() {
        // Debian: dpkg's file list of openssh-server, or none.
        let tree = Tree::new();
        tree.program("usr/bin/apt-get").dir("var/lib/dpkg/info");
        assert_eq!(tree.detect(Platform::Linux).sshd_packaged, Some(false));
        tree.file("var/lib/dpkg/info/openssh-server.list", "/usr/sbin/sshd\n");
        assert_eq!(tree.detect(Platform::Linux).sshd_packaged, Some(true));
        // Arch: pacman's per-package directory (openssh-askpass is another package).
        let tree = Tree::new();
        tree.program("usr/bin/pacman")
            .dir("var/lib/pacman/local/openssh-askpass-2.1.0-1");
        assert_eq!(tree.detect(Platform::Linux).sshd_packaged, Some(false));
        tree.dir("var/lib/pacman/local/openssh-10.0p1-1");
        assert_eq!(tree.detect(Platform::Linux).sshd_packaged, Some(true));
        // Alpine: apk's installed database.
        let tree = Tree::new();
        tree.program("sbin/apk").file(
            "lib/apk/db/installed",
            "P:openssh-client\nV:10.0\n\nP:tmux\n",
        );
        assert_eq!(tree.detect(Platform::Linux).sshd_packaged, Some(false));
        tree.file("lib/apk/db/installed", "P:openssh-server\nV:10.0\n");
        assert_eq!(tree.detect(Platform::Linux).sshd_packaged, Some(true));
        // RPM's database is not read, and no database means nothing is known.
        for tree in [Tree::new(), Tree::new()] {
            tree.program("usr/bin/dnf");
            assert_eq!(tree.detect(Platform::Linux).sshd_packaged, None);
        }
        let tree = Tree::new();
        tree.program("usr/bin/apt-get");
        assert_eq!(tree.detect(Platform::Linux).sshd_packaged, None);

        // NixOS and Guix.
        let tree = Tree::new();
        assert_eq!(tree.detect(Platform::Linux).declarative, None);
        tree.file("etc/os-release", "NAME=Debian\nID=debian\n");
        assert_eq!(tree.detect(Platform::Linux).declarative, None);
        tree.file("etc/os-release", "NAME=NixOS\nID=nixos\n");
        assert_eq!(
            tree.detect(Platform::Linux).declarative,
            Some(Declarative::NixOs)
        );
        let tree = Tree::new();
        tree.file("etc/NIXOS", "");
        assert_eq!(
            tree.detect(Platform::Linux).declarative,
            Some(Declarative::NixOs)
        );
        let tree = Tree::new();
        tree.file("usr/lib/os-release", "NAME=\"Guix System\"\nID=\"guix\"\n");
        assert_eq!(
            tree.detect(Platform::Linux).declarative,
            Some(Declarative::Guix)
        );
    }

    #[test]
    fn sshd_hints_follow_the_os_and_the_unit() {
        let mac = sshd(Platform::MacOs, &HostFacts::default());
        assert!(
            mac.contains("System Settings > General > Sharing > Remote Login")
                && mac.contains("sudo systemsetup -setremotelogin on")
                && mac.contains("Full Disk Access"),
            "{mac}"
        );
        for (facts, expected) in [
            (
                HostFacts {
                    sshd_unit: Some("ssh".into()),
                    ..linux(Some(PackageManager::Apt))
                },
                "start it with `sudo systemctl enable --now ssh`",
            ),
            (
                HostFacts {
                    sshd_unit: Some("sshd".into()),
                    ..linux(Some(PackageManager::Pacman))
                },
                "start it with `sudo systemctl enable --now sshd`",
            ),
            // No unit file found: the package manager's.
            (
                linux(Some(PackageManager::Apt)),
                "`sudo systemctl enable --now ssh`",
            ),
            (
                linux(Some(PackageManager::Dnf)),
                "`sudo systemctl enable --now sshd`",
            ),
            // systemd, and no unit or package manager known: both names.
            (
                linux(None),
                "`sudo systemctl enable --now sshd` (the unit is `ssh` on Debian and Ubuntu)",
            ),
            (
                HostFacts {
                    service_manager: Some(ServiceManager::OpenRc),
                    ..linux(Some(PackageManager::Apk))
                },
                "start it with `sudo rc-update add sshd && sudo rc-service sshd start`",
            ),
            // As root, no sudo.
            (
                HostFacts {
                    sshd_unit: Some("sshd".into()),
                    superuser: true,
                    ..linux(None)
                },
                "start it with `systemctl enable --now sshd`",
            ),
        ] {
            let hint = sshd(Platform::Linux, &facts);
            assert!(hint.contains(expected), "{facts:?}: {hint}");
            assert!(!hint.contains("installed"), "{hint}");
        }

        // An unknown init system (runit, s6, a container, WSL without systemd), even with a
        // unit file or a package manager found: no systemctl or rc-service guessed (external
        // review: systemctl was printed for every host that is not OpenRC).
        for facts in [
            HostFacts::default(),
            HostFacts {
                service_manager: None,
                sshd_unit: Some("sshd".into()),
                ..linux(Some(PackageManager::Apt))
            },
        ] {
            let hint = sshd(Platform::Linux, &facts);
            assert!(
                hint.starts_with("start sshd with this host's service manager"),
                "{hint}"
            );
            assert!(
                !hint.contains("systemctl") && !hint.contains("rc-service"),
                "{hint}"
            );
        }

        // Not found, and the package database says it is not installed: certain.
        for (manager, install, start) in [
            (
                PackageManager::Apt,
                "sudo apt install openssh-server",
                "enable --now ssh`",
            ),
            (
                PackageManager::Pacman,
                "sudo pacman -S openssh",
                "enable --now sshd`",
            ),
        ] {
            let facts = HostFacts {
                sshd_installed: Some(false),
                sshd_packaged: Some(false),
                ..linux(Some(manager))
            };
            let hint = sshd(Platform::Linux, &facts);
            assert!(
                hint.starts_with("the OpenSSH server is not installed; install it with `")
                    && hint.contains(install)
                    && hint.contains(start),
                "{hint}"
            );
        }
        // Not found, and nothing to confirm it (RPM, no database, or the database lists it):
        // hedged.
        for (manager, packaged, install, start) in [
            (
                PackageManager::Dnf,
                None,
                "`sudo dnf install openssh-server`",
                "enable --now sshd`",
            ),
            (
                PackageManager::Zypper,
                None,
                "`sudo zypper install openssh-server`",
                "enable --now sshd`",
            ),
            (
                PackageManager::Apt,
                Some(true),
                "`sudo apt install openssh-server`",
                "enable --now ssh`",
            ),
        ] {
            let facts = HostFacts {
                sshd_installed: Some(false),
                sshd_packaged: packaged,
                ..linux(Some(manager))
            };
            let hint = sshd(Platform::Linux, &facts);
            assert!(
                hint.starts_with("the OpenSSH server does not seem to be installed")
                    && hint.contains(&format!("if it is not, install it with {install}"))
                    && hint.contains(start),
                "{hint}"
            );
            assert!(!hint.contains("is not installed"), "{hint}");
        }
        let alpine = HostFacts {
            sshd_installed: Some(false),
            sshd_packaged: Some(false),
            service_manager: Some(ServiceManager::OpenRc),
            ..linux(Some(PackageManager::Apk))
        };
        assert!(
            sshd(Platform::Linux, &alpine)
                .contains("`sudo apk add openssh`, then start it with `sudo rc-update add sshd")
        );
        let unknown = HostFacts {
            sshd_installed: Some(false),
            ..linux(None)
        };
        let hint = sshd(Platform::Linux, &unknown);
        assert!(
            hint.contains("does not seem to be installed") && hint.contains("your package manager"),
            "{hint}"
        );

        // NixOS and Guix: the system configuration, never a package manager or systemctl.
        for (declarative, expected) in [
            (
                Declarative::NixOs,
                "set `services.openssh.enable = true;` in /etc/nixos/configuration.nix, then run `sudo nixos-rebuild switch`",
            ),
            (Declarative::Guix, "add `(service openssh-service-type)`"),
        ] {
            let facts = HostFacts {
                declarative: Some(declarative),
                sshd_installed: Some(false),
                ..linux(None)
            };
            let hint = sshd(Platform::Linux, &facts);
            assert!(hint.contains(expected), "{hint}");
            assert!(
                !hint.contains("installed") && !hint.contains("systemctl"),
                "{hint}"
            );
        }

        // Every hint, on every system, ends with the --ssh-port advice.
        let everything = [
            sshd(Platform::MacOs, &HostFacts::default()),
            sshd(Platform::Windows, &HostFacts::default()),
            sshd(Platform::Other, &HostFacts::default()),
            sshd(Platform::Linux, &HostFacts::default()),
            sshd(Platform::Linux, &alpine),
            sshd(Platform::Linux, &unknown),
            sshd(
                Platform::Linux,
                &HostFacts {
                    declarative: Some(Declarative::Guix),
                    ..HostFacts::default()
                },
            ),
        ];
        for hint in everything {
            assert!(
                hint.ends_with("; if sshd listens on another port, pass --ssh-port"),
                "{hint}"
            );
        }
        assert!(sshd(Platform::Windows, &HostFacts::default()).contains("OpenSSH Server"));
    }

    #[test]
    fn install_hints_follow_the_package_manager() {
        for (manager, tmux, mosh) in [
            (
                PackageManager::Brew,
                "`brew install tmux`",
                "`brew install mosh`",
            ),
            (
                PackageManager::Apt,
                "`sudo apt install tmux`",
                "`sudo apt install mosh`",
            ),
            (
                PackageManager::Dnf,
                "`sudo dnf install tmux`",
                "`sudo dnf install mosh`",
            ),
            (
                PackageManager::Yum,
                "`sudo yum install tmux`",
                "`sudo yum install mosh`",
            ),
            (
                PackageManager::Pacman,
                "`sudo pacman -S tmux`",
                "`sudo pacman -S mosh`",
            ),
            (
                PackageManager::Zypper,
                "`sudo zypper install tmux`",
                "`sudo zypper install mosh`",
            ),
            (
                PackageManager::Apk,
                "`sudo apk add tmux`",
                "`sudo apk add mosh`",
            ),
        ] {
            let facts = linux(Some(manager));
            assert_eq!(install(Program::Tmux, Platform::Linux, &facts), tmux);
            assert_eq!(install(Program::MoshServer, Platform::Linux, &facts), mosh);
        }
        // Root: no sudo; Homebrew never has it.
        let root = HostFacts {
            superuser: true,
            ..linux(Some(PackageManager::Dnf))
        };
        assert_eq!(
            install(Program::Tmux, Platform::Linux, &root),
            "`dnf install tmux`"
        );
        // No package manager found.
        assert!(
            install(Program::Tmux, Platform::MacOs, &HostFacts::default())
                .contains("brew install tmux")
        );
        assert!(
            install(Program::MoshServer, Platform::Linux, &HostFacts::default())
                .contains("mosh package with your package manager")
        );
        // herdr: its own instructions, whatever the package manager.
        for facts in [linux(Some(PackageManager::Apt)), HostFacts::default()] {
            let hint = install(Program::Herdr, Platform::Linux, &facts);
            assert!(hint.contains("herdr's install docs"), "{hint}");
            assert!(!hint.contains("apt"), "{hint}");
        }
    }

    #[test]
    fn firewall_hints_follow_the_active_firewall() {
        let with = |firewall| HostFacts {
            firewall,
            ..linux(None)
        };
        for (active, expected) in [
            (
                Some(Firewall::Ufw),
                "ufw is on: `sudo ufw allow 60000:61000/udp`",
            ),
            (
                Some(Firewall::Firewalld),
                "`sudo firewall-cmd --permanent --add-port=60000-61000/udp && sudo firewall-cmd --reload`",
            ),
            (
                Some(Firewall::Nftables),
                "`sudo nft add rule inet filter input udp dport 60000-61000 accept`",
            ),
            // Nothing found is not "nothing there": another firewall may still be on.
            (
                None,
                "no enabled ufw, firewalld or nftables was found here, so if a firewall blocks them it is another one (on this host, a router's or a cloud provider's): open them there",
            ),
        ] {
            let hint = firewall(Platform::Linux, &with(active), true).unwrap();
            assert!(
                hint.contains("60000-61000") && hint.contains(expected),
                "{hint}"
            );
        }
        // Without mosh-server there is nothing to open; Windows gets no hint.
        assert_eq!(
            firewall(Platform::Linux, &with(Some(Firewall::Ufw)), false),
            None
        );
        assert_eq!(
            firewall(Platform::Windows, &HostFacts::default(), true),
            None
        );
        assert!(
            firewall(Platform::MacOs, &HostFacts::default(), true)
                .unwrap()
                .contains("System Settings > Network > Firewall")
        );
    }
}
