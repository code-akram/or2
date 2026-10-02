//! The exact fix for a prerequisite that is missing or failing, for this host: how to turn sshd
//! on, the install command of its package manager, the rule for its firewall. `or2-pair` prints
//! these; it never runs them, and never `sudo`.
//!
//! What the host has is read once ([`HostFacts::detect`], from files under a root directory so
//! that tests can fake a host) and the hints are pure functions of it.

use std::path::{Path, PathBuf};

use crate::checks::{Platform, find_program};

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
}

/// What starts services on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceManager {
    Systemd,
    OpenRc,
}

/// A host firewall that is on (or enabled to start).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Firewall {
    Ufw,
    Firewalld,
    Nftables,
}

/// What this host has, as far as the hints need it. `Default` knows nothing, and the hints then
/// fall back to generic advice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostFacts {
    pub package_manager: Option<PackageManager>,
    pub service_manager: Option<ServiceManager>,
    /// The SSH server's systemd unit as installed (`ssh` or `sshd`), when a unit file was found.
    pub sshd_unit: Option<String>,
    /// Whether an `sshd` program is installed; `None` when it was not looked for.
    pub sshd_installed: Option<bool>,
    /// The firewall in front of mosh's UDP ports, the first found of ufw (enabled in its
    /// config), firewalld and nftables (their services enabled).
    pub firewall: Option<Firewall>,
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
    /// directories under `root`. Only looks: nothing is run or written.
    pub fn detect(
        platform: Platform,
        root: &Path,
        program_dirs: &[PathBuf],
        superuser: bool,
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

/// What to do when sshd does not answer: the end of "sshd is not answering on port N: …".
pub fn sshd(platform: Platform, facts: &HostFacts) -> String {
    match platform {
        Platform::MacOs => "turn on Remote Login (System Settings > General > Sharing > Remote Login), or run `sudo systemsetup -setremotelogin on` (that needs Full Disk Access for this terminal app, in System Settings > Privacy & Security); if sshd listens on another port, pass --ssh-port".to_owned(),
        Platform::Linux => {
            let sudo = facts.sudo();
            let unit = facts
                .sshd_unit
                .as_deref()
                .or_else(|| facts.package_manager.map(PackageManager::sshd_unit));
            let start = match (facts.service_manager, unit) {
                (Some(ServiceManager::OpenRc), _) => {
                    format!("`{sudo}rc-update add sshd && {sudo}rc-service sshd start`")
                }
                (_, Some(unit)) => format!("`{sudo}systemctl enable --now {unit}`"),
                (_, None) => format!(
                    "`{sudo}systemctl enable --now sshd` (the unit is `ssh` on Debian and Ubuntu)"
                ),
            };
            match (facts.sshd_installed, facts.package_manager) {
                (Some(false), Some(manager)) => format!(
                    "the OpenSSH server is not installed; install it with `{}`, then start it with {start}",
                    manager.install(manager.openssh_server(), facts.superuser)
                ),
                (Some(false), None) => format!(
                    "the OpenSSH server does not seem to be installed; install it with your package manager, then start it with {start}"
                ),
                _ => format!(
                    "start it with {start}; if sshd listens on another port, pass --ssh-port"
                ),
            }
        }
        Platform::Windows => {
            "install and start OpenSSH Server (Settings > Optional features, then Start-Service sshd)".to_owned()
        }
        Platform::Other => "start your SSH server".to_owned(),
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
                "{ports}; no active ufw, firewalld or nftables service was found, so only another firewall (a router's, a cloud provider's) could block them"
            ),
        },
        Platform::Windows | Platform::Other => return None,
    })
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
            HostFacts::detect(platform, self.root(), &[], false)
        }
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
                "`sudo systemctl enable --now ssh`",
            ),
            (
                HostFacts {
                    sshd_unit: Some("sshd".into()),
                    ..linux(Some(PackageManager::Pacman))
                },
                "`sudo systemctl enable --now sshd`",
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
            // Nothing known: both names.
            (
                HostFacts::default(),
                "`sudo systemctl enable --now sshd` (the unit is `ssh` on Debian and Ubuntu)",
            ),
            (
                HostFacts {
                    service_manager: Some(ServiceManager::OpenRc),
                    ..linux(Some(PackageManager::Apk))
                },
                "`sudo rc-update add sshd && sudo rc-service sshd start`",
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
            assert!(hint.contains("--ssh-port"), "{hint}");
        }
        // Not installed: install, then start.
        for (manager, install, start) in [
            (
                PackageManager::Apt,
                "sudo apt install openssh-server",
                "enable --now ssh`",
            ),
            (
                PackageManager::Dnf,
                "sudo dnf install openssh-server",
                "enable --now sshd`",
            ),
            (
                PackageManager::Pacman,
                "sudo pacman -S openssh",
                "enable --now sshd`",
            ),
            (
                PackageManager::Zypper,
                "sudo zypper install openssh-server",
                "enable --now sshd`",
            ),
        ] {
            let facts = HostFacts {
                sshd_installed: Some(false),
                ..linux(Some(manager))
            };
            let hint = sshd(Platform::Linux, &facts);
            assert!(
                hint.contains("not installed") && hint.contains(install) && hint.contains(start),
                "{hint}"
            );
        }
        let alpine = HostFacts {
            sshd_installed: Some(false),
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
        assert!(sshd(Platform::Linux, &unknown).contains("with your package manager"));
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
            (None, "no active ufw, firewalld or nftables"),
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
