//! Command-line arguments, parsed by hand: a handful of flags do not need a framework.

use crate::bootstrap::PairingId;

pub const HELP: &str = "\
or2-pair - pair a phone with this host: one command, one QR scan

USAGE:
    or2-pair [OPTIONS]

Run it in a terminal. It checks this host, asks for the code shown on your phone (Add host >
Easy pair), then prints a QR code (and the same pairing code as text) for you to scan with that
phone. Pairing runs over this host's own sshd, on the port you connect to anyway: no other port
is opened. The phone's SSH public key lands in ~/.ssh/authorized_keys and a temporary key that
or2-pair added is removed again; if anything goes wrong or you press Ctrl-C the temporary key is
removed too. Nothing is changed before you type the code.

OPTIONS:
    --name <label>        Name shown on the phone (default: this machine's host name)
    --user <user>         Must be the account you are running as (the default): keys are only
                          authorized for the current user, in that user's own ~/.ssh
                          (Windows: required; it names the login for the code, and or2-pair
                          installs no key there: it prints how to add one by hand)
    --ssh-port <port>     Port sshd listens on (default: Port in /etc/ssh/sshd_config, else 22)
    --address <host>      Add an address to the code, ahead of the detected ones (repeatable),
                          e.g. a DNS name that works from anywhere
    --manual              Print the code without pairing: asks for no code and changes nothing.
                          The phone then shows its public key for you to add to authorized_keys
                          by hand (alias: --no-listen)
    --check               Run the checks and exit; nothing else is done
    --ascii               Draw the QR with ASCII only (two characters per module; wider)
    --invert              Draw the QR for a light terminal (only without colour)
    --no-color            No ANSI colours (the QR is then drawn for a dark terminal)
    -h, --help            Show this help
    -V, --version         Show the version

Internal:
    or2-pair enroll <id>  What sshd runs for the temporary key during a pairing. Not for typing.
";

/// What `--bind` and `--pair-port` (version 1's listener options) now say.
pub const REMOVED_OPTIONS: &str = "pairing uses the SSH port now; these options are gone";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    pub name: Option<String>,
    pub user: Option<String>,
    pub ssh_port: Option<u16>,
    pub addresses: Vec<String>,
    pub manual: bool,
    pub check_only: bool,
    pub ascii: bool,
    pub invert: bool,
    pub no_color: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Run(Options),
    /// The forced command: `or2-pair enroll <id>`.
    Enroll(PairingId),
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
    let args: Vec<String> = args.into_iter().collect();
    if args.first().map(String::as_str) == Some("enroll") {
        // Exactly `enroll <id>`, with the id validated: it comes from sshd's forced command.
        return match &args[1..] {
            [id] => PairingId::parse(id)
                .map(Parsed::Enroll)
                .map_err(|error| usage(format!("enroll: {error}"))),
            _ => Err(usage("enroll takes exactly one argument, the pairing id")),
        };
    }
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
            "--address" => options.addresses.push(value(&mut args)?),
            "--bind" | "--pair-port" => return Err(usage(format!("{flag}: {REMOVED_OPTIONS}"))),
            "--manual" | "--no-listen" => options.manual = true,
            "--check" => options.check_only = true,
            "--ascii" => options.ascii = true,
            "--invert" => options.invert = true,
            "--no-color" => options.no_color = true,
            other => return Err(usage(format!("unknown option {other:?}; see --help"))),
        }
        if inline.is_some()
            && matches!(
                flag.as_str(),
                "--manual" | "--no-listen" | "--check" | "--ascii" | "--invert" | "--no-color"
            )
        {
            return Err(usage(format!("{flag} takes no value")));
        }
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
            "--address",
            "a.example.org",
            "--address=b.example.org",
            "--ascii",
            "--invert",
            "--no-color",
        ]);
        assert_eq!(o.name.as_deref(), Some("Work Mac"));
        assert_eq!(o.user.as_deref(), Some("alice"));
        assert_eq!(o.ssh_port, Some(2222));
        assert_eq!(o.addresses, ["a.example.org", "b.example.org"]);
        assert!(o.ascii && o.invert && o.no_color && !o.manual);
    }

    #[test]
    fn manual_and_its_old_alias() {
        assert!(options(&["--manual"]).manual);
        assert!(options(&["--no-listen"]).manual);
        assert!(run(&["--manual=1"]).is_err());
    }

    #[test]
    fn the_listener_options_are_gone_with_a_message() {
        for args in [
            &["--bind", "10.0.0.5"][..],
            &["--bind=10.0.0.5"],
            &["--pair-port", "5000"],
            &["--pair-port=5000"],
            &["--manual", "--bind", "10.0.0.5"],
        ] {
            let error = run(args).unwrap_err().to_string();
            assert!(
                error.contains("pairing uses the SSH port now; these options are gone"),
                "{args:?}: {error}"
            );
        }
    }

    #[test]
    fn help_and_version_win() {
        assert_eq!(run(&["--help"]).unwrap(), Parsed::Help);
        assert_eq!(run(&["-h"]).unwrap(), Parsed::Help);
        assert_eq!(run(&["-V"]).unwrap(), Parsed::Version);
        assert_eq!(run(&["--version"]).unwrap(), Parsed::Version);
    }

    #[test]
    fn enroll_takes_exactly_a_valid_id() {
        assert_eq!(
            run(&["enroll", "abcdefghijklm"]).unwrap(),
            Parsed::Enroll(PairingId::parse("abcdefghijklm").unwrap())
        );
        for args in [
            &["enroll"][..],
            &["enroll", "abcdefghijklm", "extra"],
            &["enroll", "../../x"],
            &["enroll", "ABCDEFGHIJKLM"],
            &["enroll", ""],
            &["enroll=abcdefghijklm"],
        ] {
            assert!(run(args).is_err(), "{args:?}");
        }
        // Only as the first argument.
        assert!(run(&["--check", "enroll", "abcdefghijklm"]).is_err());
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
            &["--check=1"],
            &["stray"],
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
            "--manual",
            "--no-listen",
            "--check",
            "--ascii",
            "--invert",
            "--no-color",
            "--help",
            "--version",
            "Internal:",
            "or2-pair enroll <id>",
        ] {
            assert!(HELP.contains(flag), "{flag}");
        }
        assert!(!HELP.contains("--bind") && !HELP.contains("--pair-port"));
    }
}
