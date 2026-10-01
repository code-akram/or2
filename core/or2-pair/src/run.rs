//! `or2-pair` end to end: checks, gather, print the code, listen once, ask, authorize.
//!
//! Everything the run touches comes in through [`Env`] (home directory, interfaces, sockets,
//! the confirmation, the clock), so the whole flow runs against a temporary home and loopback
//! in tests. `main` builds the real `Env`.

use std::fmt::Write as _;
use std::io::{self, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::account::{Account, AccountError};
use crate::addresses::{self, Address, BindWarning, Iface, Kind};
use crate::args::Options;
use crate::authorized_keys::{self, Added};
use crate::checks::{self, CheckInput, Level, Platform};
use crate::confirm::Confirm;
use crate::date::DateTime;
use crate::exchange::{self, Outcome, Session, Stats};
use crate::hostkey::{self, Keyscan};
use crate::net::Net;
use crate::payload::{self, Payload};
use crate::qr::{self, QrStyle};

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
    pub confirm: &'a dyn Confirm,
    /// Whether there is a person to ask. The binary sets it from "standard input is a terminal";
    /// listening without one is refused.
    pub can_ask: bool,
    /// Whether the output takes ANSI colours.
    pub color: bool,
    pub random: &'a dyn Fn(&mut [u8]),
    pub now: &'a dyn Fn() -> DateTime,
    /// How long to listen: [`WINDOW`] (120 s); tests shorten it.
    pub window: Duration,
    /// Called once the code is printed and the listener is up (tests use it to find the code).
    pub on_ready: Option<&'a dyn Fn(&Ready)>,
}

/// What a test needs to play the phone.
#[derive(Debug, Clone)]
pub struct Ready {
    pub payload: String,
    pub listening: Vec<SocketAddr>,
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
        "no address to give the phone: this host has no LAN, overlay or public IPv4 address and no host name; pass --address"
    )]
    NoAddress,
    #[error(
        "no LAN or overlay address to listen on; connect this host to the phone's network, name an address with --bind, or use --no-listen"
    )]
    NoBindAddress,
    #[error(
        "listening needs an interactive terminal to confirm in; run or2-pair in a terminal, or use --no-listen"
    )]
    NotInteractive,
    #[error("cannot listen: {0}")]
    Listen(io::Error),
    #[error("{0}")]
    TooLong(#[from] payload::TooLong),
    #[error("cannot draw the QR code: {0}")]
    Qr(String),
    #[error("cannot write output: {0}")]
    Output(#[from] io::Error),
}

/// How the run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// A phone's key was added (or was already there).
    Paired,
    /// `--no-listen`: the code was printed.
    CodeOnly,
    /// `--check`.
    Checked,
    Declined,
    TimedOut,
    /// A wrong code, an unreadable request or a refused key.
    Refused,
    /// Authorization failed on this host.
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

/// `Port N` from `sshd_config`, when it has one.
pub fn sshd_config_port(config: &str) -> Option<u16> {
    config.lines().find_map(|line| {
        let line = line.trim();
        let mut words = line.split_whitespace();
        if !words.next()?.eq_ignore_ascii_case("port") {
            return None;
        }
        words.next()?.parse().ok().filter(|port| *port != 0)
    })
}

/// Expands what the listener is bound to into what a phone can dial: a wildcard becomes the
/// host's own overlay and LAN addresses on that port.
fn advertised(listening: &[SocketAddr], addresses: &[Address]) -> Vec<SocketAddr> {
    let mut out: Vec<SocketAddr> = Vec::new();
    for endpoint in listening {
        if endpoint.ip().is_unspecified() {
            for ip in addresses::bindable(addresses) {
                if ip.is_ipv4() == endpoint.ip().is_ipv4() {
                    out.push(SocketAddr::new(ip, endpoint.port()));
                }
            }
        } else {
            out.push(*endpoint);
        }
    }
    out.dedup();
    out.truncate(4);
    out
}

fn heading(out: &mut dyn Write, text: &str) -> io::Result<()> {
    writeln!(out, "\n{text}")
}

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

    // Fail before doing anything: with nobody to confirm there is no point in showing a code.
    if !options.no_listen && !options.check_only && !env.can_ask {
        return Err(RunError::NotInteractive);
    }

    let ssh_port = options.ssh_port.unwrap_or_else(|| {
        std::fs::read_to_string(env.etc_ssh.join("sshd_config"))
            .ok()
            .and_then(|config| sshd_config_port(&config))
            .unwrap_or(22)
    });

    heading(out, "Checks")?;
    let found = checks::run(&CheckInput {
        account: &env.account,
        ssh_port,
        net: env.net,
        program_dirs: &env.program_dirs,
        platform: env.platform,
    });
    for check in &found {
        writeln!(out, "  {}  {}", check.level.tag(), check.text)?;
    }
    if options.check_only {
        let warnings = found
            .iter()
            .filter(|check| check.level == Level::Warn)
            .count();
        writeln!(out, "\n{warnings} warning(s). Nothing was changed.")?;
        return Ok(Exit::Checked);
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

    // Bind first, so the code names the port that is really open.
    let mut listener = None;
    let mut otp = None;
    let mut pair = Vec::new();
    if !options.no_listen {
        let bind: Vec<IpAddr> = if options.bind.is_empty() {
            addresses::bindable(&addresses)
        } else {
            options.bind.clone()
        };
        if bind.is_empty() {
            return Err(RunError::NoBindAddress);
        }
        for ip in &options.bind {
            match addresses::bind_warning(*ip) {
                Some(BindWarning::Wildcard) => writeln!(
                    out,
                    "  warn  --bind {ip} listens on every interface, public ones included"
                )?,
                Some(BindWarning::Public) => {
                    writeln!(out, "  warn  --bind {ip} is a public address")?;
                }
                None => {}
            }
        }
        let bound = env
            .net
            .listen(&bind, options.pair_port)
            .map_err(RunError::Listen)?;
        pair = advertised(&bound.endpoints(), &addresses);
        let mut secret = [0u8; 16];
        (env.random)(&mut secret);
        otp = Some(secret);
        listener = Some(bound);
    }

    let mut payload = Payload {
        name: name.clone(),
        user: user.clone(),
        port: ssh_port,
        addresses: addresses.iter().map(|a| a.text.clone()).collect(),
        host_key: host_key.key.openssh(),
        pair,
        otp,
    };
    let dropped = payload.fit()?;

    heading(out, "This host")?;
    writeln!(out, "  name       {name}")?;
    writeln!(out, "  user       {user}")?;
    writeln!(out, "  ssh port   {ssh_port}")?;
    writeln!(
        out,
        "  host key   {} {}  ({})",
        host_key.key.algorithm(),
        host_key.key.fingerprint(),
        host_key.source
    )?;
    writeln!(out, "  addresses  (the phone tries them in this order)")?;
    for (index, address) in addresses.iter().enumerate() {
        let used = payload.addresses.contains(&address.text);
        let what = match (&address.interface, address.kind) {
            (Some(interface), kind) => format!("{} ({interface})", kind.label()),
            (None, Kind::Mdns) => "mDNS name, same network only".to_owned(),
            (None, kind) => kind.label().to_owned(),
        };
        let note = if used {
            ""
        } else {
            "  [left out: the code would be too long]"
        };
        writeln!(out, "    {}. {:<18} {what}{note}", index + 1, address.text)?;
    }
    if !dropped.is_empty() {
        writeln!(
            out,
            "  note: {} address(es) were left out to keep the code under {} bytes",
            dropped.len(),
            payload::MAX_BYTES
        )?;
    }

    let text = payload.encode();
    let style = QrStyle {
        ascii: options.ascii,
        color: env.color && !options.no_color,
        invert: options.invert,
    };
    heading(out, "Scan this with or2 (Add host > Easy pair with QR)")?;
    writeln!(out)?;
    let drawing = qr::render(&text, style).map_err(|error| RunError::Qr(error.to_string()))?;
    out.write_all(drawing.as_bytes())?;
    if options.ascii {
        let (width, _) = qr::modules(&text).map_err(|error| RunError::Qr(error.to_string()))?;
        writeln!(
            out,
            "  (this drawing is {} columns wide)",
            qr::columns(width, style)
        )?;
    }
    heading(
        out,
        "Or paste this pairing code into the app (Easy pair > Paste pairing code)",
    )?;
    writeln!(out, "{text}")?;

    let Some(mut listener) = listener else {
        writeln!(
            out,
            "\nNo listener (--no-listen). After scanning, the phone shows its public key: add it to {} on this host.",
            authorized_keys::path(&env.account.home).display()
        )?;
        return Ok(Exit::CodeOnly);
    };

    let listening = listener.endpoints();
    let ends: Vec<String> = listening.iter().map(ToString::to_string).collect();
    writeln!(
        out,
        "\nListening on {} for {} s. Press Ctrl-C to cancel.",
        ends.join(", "),
        env.window.as_secs()
    )?;
    out.flush()?;
    if let Some(ready) = env.on_ready {
        ready(&Ready {
            payload: text.clone(),
            listening,
        });
    }

    let session = Session {
        account: &env.account,
        otp: otp.unwrap_or_default(),
        confirm: env.confirm,
        random: env.random,
        now: env.now,
    };
    let served = exchange::serve(listener.as_mut(), &session, Instant::now() + env.window);
    report(&served.outcome, served.stats, &user, &env.account.home, out)
}

fn report(
    outcome: &Outcome,
    stats: Stats,
    user: &str,
    home: &Path,
    out: &mut dyn Write,
) -> Result<Exit, RunError> {
    let mut text = String::new();
    let exit = match outcome {
        Outcome::Authorized {
            added,
            device,
            fingerprint,
        } => {
            match added {
                Added::Added { created, backup } => {
                    let _ = writeln!(
                        text,
                        "Authorized {device} ({fingerprint}) for {user} in {}.",
                        authorized_keys::path(home).display()
                    );
                    if *created {
                        let _ = writeln!(
                            text,
                            "(The file did not exist; it was created with mode 600.)"
                        );
                    }
                    if let Some(backup) = backup {
                        let _ =
                            writeln!(text, "The previous file is saved as {}.", backup.display());
                    }
                }
                Added::AlreadyPresent => {
                    let _ = writeln!(
                        text,
                        "{device} ({fingerprint}) was already authorized for {user}; nothing was changed."
                    );
                }
            }
            let _ = writeln!(text, "Done. The phone connects as {user} now.");
            Exit::Paired
        }
        Outcome::Declined {
            device,
            fingerprint,
        } => {
            let _ = writeln!(
                text,
                "Declined: {device} ({fingerprint}) was not authorized. Nothing was changed."
            );
            Exit::Declined
        }
        Outcome::KeyRejected => {
            let _ = writeln!(
                text,
                "The phone sent a key this tool does not authorize (it takes ssh-ed25519, ecdsa and ssh-rsa keys). Nothing was changed."
            );
            Exit::Refused
        }
        Outcome::TimedOut => {
            let _ = writeln!(
                text,
                "Timed out: no phone paired in time. Nothing was changed. Run or2-pair again to retry."
            );
            Exit::TimedOut
        }
        Outcome::WriteFailed(error) => {
            let _ = writeln!(
                text,
                "Could not write {}: {error}",
                authorized_keys::path(home).display()
            );
            Exit::Failed
        }
        Outcome::ListenFailed(error) => {
            let _ = writeln!(text, "The listener failed: {error}");
            Exit::Failed
        }
    };
    if stats.rejected > 0 || stats.dropped > 0 {
        let _ = writeln!(
            text,
            "{} connection(s) were refused (not a pairing request, or a wrong or already used code) and {} more were closed without an answer; none of them could use up the attempt.",
            stats.rejected, stats.dropped
        );
    }
    writeln!(out)?;
    out.write_all(text.as_bytes())?;
    Ok(exit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_first_port_line_of_sshd_config() {
        for (config, port) in [
            ("Port 2222\n", Some(2222)),
            ("# Port 22\nport 2200\nPort 2201\n", Some(2200)),
            ("  Port   22  \n", Some(22)),
            ("PasswordAuthentication no\n", None),
            ("Port x\nPort 99\n", Some(99)),
            ("Port 0\n", None),
            ("", None),
            ("PortFoo 22\n", None),
        ] {
            assert_eq!(sshd_config_port(config), port, "{config:?}");
        }
    }

    #[test]
    fn a_wildcard_listener_advertises_the_hosts_own_addresses() {
        let addresses = addresses::gather(
            &[
                Iface {
                    name: "eth0".into(),
                    ip: "192.168.1.20".parse().unwrap(),
                },
                Iface {
                    name: "eth1".into(),
                    ip: "203.0.113.9".parse().unwrap(),
                },
            ],
            Some("box"),
            &[],
        );
        let listening: Vec<SocketAddr> = vec!["0.0.0.0:4000".parse().unwrap()];
        let shown = advertised(&listening, &addresses);
        assert_eq!(shown, ["192.168.1.20:4000".parse::<SocketAddr>().unwrap()]);
        let specific: Vec<SocketAddr> =
            vec!["10.0.0.1:5".parse().unwrap(), "10.0.0.1:5".parse().unwrap()];
        assert_eq!(advertised(&specific, &addresses).len(), 1);
    }
}
