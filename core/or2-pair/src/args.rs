//! Command-line arguments, parsed by hand: a handful of flags do not need a framework.

use std::net::IpAddr;

pub const HELP: &str = "\
or2-pair - pair a phone with this host: one command, one QR scan

USAGE:
    or2-pair [OPTIONS]

Prints a QR code (and the same pairing code as text). Scan it with or2 on your phone
(Add host > Easy pair with QR). The phone sends its SSH public key to a one-shot listener on
this machine; you confirm the key's fingerprint here, and it is appended to
~/.ssh/authorized_keys. Nothing is changed before you answer y.

OPTIONS:
    --name <label>        Name shown on the phone (default: this machine's host name)
    --user <user>         Must be the account you are running as (the default): keys are only
                          authorized for the current user, in that user's own ~/.ssh
                          (Windows: required; it names the login for the code, and or2-pair
                          installs no key there: it prints how to add one by hand)
    --ssh-port <port>     Port sshd listens on (default: Port in /etc/ssh/sshd_config, else 22)
    --address <host>      Add an address to the code, ahead of the detected ones (repeatable),
                          e.g. a DNS name that works from anywhere
    --bind <ip>           Listen only on this address (repeatable). By default the listener
                          binds the LAN and overlay (ZeroTier, Tailscale) addresses it lists,
                          never a public one. A public address or 0.0.0.0 is allowed here,
                          with a warning
    --pair-port <port>    Port for the listener (default: a random free one)
    --no-listen           Print the code without a listener. The phone then shows its public
                          key for you to add to authorized_keys by hand
    --check               Run the checks and exit; nothing else is done
    --ascii               Draw the QR with ASCII only (two characters per module; wider)
    --invert              Draw the QR for a light terminal (only without colour)
    --no-color            No ANSI colours (the QR is then drawn for a dark terminal)
    -h, --help            Show this help
    -V, --version         Show the version

The listener waits at most 120 seconds, serves one attempt, and then exits.
";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    pub name: Option<String>,
    pub user: Option<String>,
    pub ssh_port: Option<u16>,
    pub addresses: Vec<String>,
    pub bind: Vec<IpAddr>,
    pub pair_port: u16,
    pub no_listen: bool,
    pub check_only: bool,
    pub ascii: bool,
    pub invert: bool,
    pub no_color: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Run(Options),
    Help,
    Version,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct UsageError(pub String);

fn usage(message: impl Into<String>) -> UsageError {
    UsageError(message.into())
}

fn port(flag: &str, value: &str) -> Result<u16, UsageError> {
    match value.parse::<u16>() {
        Ok(port) if port != 0 => Ok(port),
        _ => Err(usage(format!(
            "{flag} needs a port from 1 to 65535, not {value:?}"
        ))),
    }
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Parsed, UsageError> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => {
                (flag.to_owned(), Some(value.to_owned()))
            }
            _ => (arg, None),
        };
        let value = |args: &mut dyn Iterator<Item = String>| -> Result<String, UsageError> {
            inline
                .clone()
                .or_else(|| args.next())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| usage(format!("{flag} needs a value")))
        };
        match flag.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "-V" | "--version" => return Ok(Parsed::Version),
            "--name" => options.name = Some(value(&mut args)?),
            "--user" => options.user = Some(value(&mut args)?),
            "--ssh-port" => options.ssh_port = Some(port(&flag, &value(&mut args)?)?),
            "--pair-port" => options.pair_port = port(&flag, &value(&mut args)?)?,
            "--address" => options.addresses.push(value(&mut args)?),
            "--bind" => {
                let text = value(&mut args)?;
                let ip = text
                    .parse()
                    .map_err(|_| usage(format!("--bind needs an IP address, not {text:?}")))?;
                options.bind.push(ip);
            }
            "--no-listen" => options.no_listen = true,
            "--check" => options.check_only = true,
            "--ascii" => options.ascii = true,
            "--invert" => options.invert = true,
            "--no-color" => options.no_color = true,
            other => return Err(usage(format!("unknown option {other:?}; see --help"))),
        }
        if inline.is_some()
            && matches!(
                flag.as_str(),
                "--no-listen" | "--check" | "--ascii" | "--invert" | "--no-color"
            )
        {
            return Err(usage(format!("{flag} takes no value")));
        }
    }
    if options.no_listen && (!options.bind.is_empty() || options.pair_port != 0) {
        return Err(usage(
            "--bind and --pair-port have no meaning with --no-listen",
        ));
    }
    Ok(Parsed::Run(options))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Parsed, UsageError> {
        parse(args.iter().map(|arg| (*arg).to_owned()))
    }

    fn options(args: &[&str]) -> Options {
        match run(args).unwrap() {
            Parsed::Run(options) => options,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn no_arguments_is_the_default_run() {
        assert_eq!(options(&[]), Options::default());
    }

    #[test]
    fn flags_with_values_in_both_spellings() {
        let o = options(&[
            "--name",
            "Work Mac",
            "--user=alice",
            "--ssh-port",
            "2222",
            "--pair-port=5000",
            "--address",
            "a.example.org",
            "--address=b.example.org",
            "--bind",
            "10.0.0.5",
            "--bind=fd00::1",
            "--ascii",
            "--invert",
            "--no-color",
        ]);
        assert_eq!(o.name.as_deref(), Some("Work Mac"));
        assert_eq!(o.user.as_deref(), Some("alice"));
        assert_eq!(o.ssh_port, Some(2222));
        assert_eq!(o.pair_port, 5000);
        assert_eq!(o.addresses, ["a.example.org", "b.example.org"]);
        assert_eq!(o.bind.len(), 2);
        assert!(o.ascii && o.invert && o.no_color && !o.no_listen);
    }

    #[test]
    fn help_and_version_win() {
        assert_eq!(run(&["--help"]).unwrap(), Parsed::Help);
        assert_eq!(run(&["-h"]).unwrap(), Parsed::Help);
        assert_eq!(run(&["-V"]).unwrap(), Parsed::Version);
        assert_eq!(run(&["--version"]).unwrap(), Parsed::Version);
    }

    #[test]
    fn bad_input_is_a_usage_error() {
        for args in [
            &["--nope"][..],
            &["--name"],
            &["--name="],
            &["--ssh-port", "0"],
            &["--ssh-port", "70000"],
            &["--ssh-port", "x"],
            &["--bind", "host.example.org"],
            &["--no-listen=1"],
            &["stray"],
            &["--no-listen", "--bind", "10.0.0.1"],
            &["--no-listen", "--pair-port", "5000"],
        ] {
            assert!(run(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn the_help_text_names_every_flag() {
        for flag in [
            "--name",
            "--user",
            "--ssh-port",
            "--address",
            "--bind",
            "--pair-port",
            "--no-listen",
            "--check",
            "--ascii",
            "--invert",
            "--no-color",
            "--help",
            "--version",
        ] {
            assert!(HELP.contains(flag), "{flag}");
        }
    }
}
