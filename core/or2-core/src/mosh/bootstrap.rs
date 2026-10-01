//! Starting `mosh-server` on the host over an existing exec path, and reading back what it says.
//!
//! mosh authenticates nothing itself: SSH does, and `mosh-server new` prints the UDP port and
//! the session key it generated, then detaches. This module runs that command through a
//! [`RemoteHost`] (so it works over the host's SSH connection, and over `LocalHost` in tests) and
//! parses the answer into [`MoshParams`].

use std::fmt;

use zeroize::{Zeroize, Zeroizing};

use crate::host::HostCapabilities;
use crate::remote::{ExecOutput, RemoteCommand, RemoteError, RemoteHost};
use crate::session::SessionFailure;
use crate::term::TerminalSize;

use super::ssp::error::MoshError;
use super::ssp::key::Base64Key;

/// The session key `mosh-server` printed: a secret. Held in `Zeroizing` memory, redacted from
/// `Debug`, never in `Display`, errors or logs.
#[derive(Clone)]
pub struct MoshKey(Zeroizing<String>);

impl MoshKey {
    /// Validates the 22-character printable form mosh-server prints.
    pub fn parse(printable: &str) -> Result<Self, MoshError> {
        Base64Key::from_printable(printable)?;
        Ok(Self(Zeroizing::new(printable.to_owned())))
    }

    pub(super) fn to_base64_key(&self) -> Result<Base64Key, MoshError> {
        Base64Key::from_printable(&self.0)
    }
}

impl fmt::Debug for MoshKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MoshKey(<redacted>)")
    }
}

/// Everything [`super::start`] needs to attach to a running `mosh-server`.
#[derive(Clone)]
pub struct MoshParams {
    /// The UDP port the server listens on, on the host that ran the bootstrap.
    pub port: u16,
    pub key: MoshKey,
    /// The terminal size to start at. `mosh-server` has no size option (it starts at 80x24 and
    /// learns the real size from the client's first datagram), so this is the client's.
    pub size: TerminalSize,
    /// The detached server's process id, when it printed one. Only meaningful on the host. A
    /// `mosh-server` waits for its client for as long as it lives, so a caller whose session
    /// never connects (it closes `TimedOut` before `Connected`, or is disconnected first) hands
    /// this to [`terminate`].
    pub server_pid: Option<u32>,
}

impl fmt::Debug for MoshParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MoshParams")
            .field("port", &self.port)
            .field("key", &self.key)
            .field("size", &self.size)
            .field("server_pid", &self.server_pid)
            .finish()
    }
}

/// Why the bootstrap failed. Messages are diagnostics without secrets.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BootstrapError {
    /// The capability probe found no `mosh-server` on the host.
    #[error("mosh-server is not installed on the host")]
    NotInstalled,
    /// The command could not be run at all.
    #[error("could not run mosh-server: {0}")]
    Remote(#[from] RemoteError),
    /// `mosh-server` ran and failed, for instance for want of a UTF-8 locale.
    #[error("mosh-server failed (exit status {status:?}): {detail}")]
    Failed { status: Option<u32>, detail: String },
    /// It exited successfully but never printed `MOSH CONNECT`.
    #[error("mosh-server did not print a MOSH CONNECT line: {detail}")]
    NoConnectLine { detail: String },
    #[error("mosh-server printed an invalid port")]
    InvalidPort,
    #[error("mosh-server printed an invalid session key")]
    InvalidKey,
}

impl BootstrapError {
    /// The session failure a terminal that could not start its mosh server closes with.
    pub fn into_failure(self) -> SessionFailure {
        match self {
            Self::NotInstalled => SessionFailure::NotInstalled {
                program: "mosh-server".into(),
            },
            Self::Remote(RemoteError::TimedOut) => SessionFailure::TimedOut,
            Self::Remote(RemoteError::Closed) => {
                SessionFailure::ConnectionLost("the host connection closed".into())
            }
            // The transport under the exec channel broke: reconnecting the host is the
            // recovery, not running the command again.
            Self::Remote(RemoteError::Io(reason)) => {
                SessionFailure::ConnectionLost(format!("could not run mosh-server: {reason}"))
            }
            error => SessionFailure::CommandFailed(error.to_string()),
        }
    }
}

/// How much of the server's stderr an error message may carry.
const DETAIL_BYTES: usize = 400;

/// The `mosh-server` command line: `LANG=<utf8> <mosh-server> new -s -c 256 -l LANG=<utf8> --
/// <target>`. `target` is the program and arguments to run in the session; empty runs the
/// user's login shell. The locale travels twice because the first sets the environment of
/// `mosh-server` itself, which refuses to start without a UTF-8 locale, and the second is what
/// it gives the program it runs.
pub fn command(mosh_server: &str, utf8_locale: &str, target: &[String]) -> RemoteCommand {
    let locale = format!("LANG={utf8_locale}");
    let mut command = RemoteCommand::new(mosh_server)
        .env("LANG", utf8_locale)
        .args(["new", "-s", "-c", "256", "-l"])
        .arg(locale);
    if !target.is_empty() {
        command = command.arg("--").args(target.iter().cloned());
    }
    command
}

/// Starts a `mosh-server` for `target` on `host` and returns how to reach it. Uses the
/// `mosh-server` path and UTF-8 locale from the host's capability probe.
pub async fn bootstrap(
    host: &impl RemoteHost,
    caps: &HostCapabilities,
    size: TerminalSize,
    target: &[String],
) -> Result<MoshParams, BootstrapError> {
    let server = caps
        .mosh_server
        .as_deref()
        .ok_or(BootstrapError::NotInstalled)?;
    let mut output = host
        .exec(&command(server, &caps.utf8_locale, target))
        .await?;
    let params = parse_output(&output, size);
    // The pid may be on either stream, so read it before stdout (which carried the key) is
    // wiped; only the integer is kept.
    let started = if params.is_err() {
        detached_pid(&output)
    } else {
        None
    };
    output.stdout.zeroize();
    if let Some(pid) = started {
        // It started but its answer is unusable: nobody will ever connect to it.
        let _ = terminate(host, pid).await;
    }
    params
}

/// Stops the `mosh-server` with process id `pid` on `host`, which `bootstrap` started and no
/// session reached (or that a disconnect abandoned before it connected). `mosh-server` has no
/// idle timeout of its own (and one set through `MOSH_SERVER_NETWORK_TMOUT` would also end a
/// healthy session whose client was offline for a while), so without this a failed connect
/// leaves it and its shell on the host for good.
///
/// Only a process whose name ends in `mosh-server` is signalled, so an id that has since been
/// reused by something else is left alone; a server that is already gone is not an error.
/// Sends `SIGTERM`, which `mosh-server` handles by ending its session.
pub async fn terminate(host: &impl RemoteHost, pid: u32) -> Result<(), RemoteError> {
    let script = format!(
        "case \"$(ps -p {pid} -o comm= 2>/dev/null)\" in *mosh-server) kill -TERM {pid} ;; esac"
    );
    host.exec_script(&script).await.map(drop)
}

/// The pid from `[mosh-server detached, pid = N]` on either stream.
fn detached_pid(output: &ExecOutput) -> Option<u32> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    stdout
        .lines()
        .chain(stderr.lines())
        .find_map(|line| parse_pid(line.trim()))
}

/// Reads `MOSH CONNECT <port> <key>` (stdout) and `[mosh-server detached, pid = N]` (stderr, but
/// either stream is accepted) from a finished `mosh-server new`.
pub fn parse_output(output: &ExecOutput, size: TerminalSize) -> Result<MoshParams, BootstrapError> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut connect = None;
    let mut server_pid = None;
    for line in stdout.lines().chain(stderr.lines()) {
        let line = line.trim();
        if connect.is_none()
            && let Some(rest) = line
                .strip_prefix("MOSH CONNECT")
                .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
        {
            connect = Some(Zeroizing::new(rest.to_owned()));
        } else if server_pid.is_none() {
            server_pid = parse_pid(line);
        }
    }
    let Some(rest) = connect.as_deref().map(String::as_str) else {
        let detail = detail(&stderr);
        return Err(if output.success() {
            BootstrapError::NoConnectLine { detail }
        } else {
            BootstrapError::Failed {
                status: output.status,
                detail,
            }
        });
    };
    let mut fields = rest.split_whitespace();
    let port = fields
        .next()
        .and_then(|port| port.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .ok_or(BootstrapError::InvalidPort)?;
    let key = fields
        .next()
        .and_then(|key| MoshKey::parse(key).ok())
        .ok_or(BootstrapError::InvalidKey)?;
    Ok(MoshParams {
        port,
        key,
        size,
        server_pid,
    })
}

fn parse_pid(line: &str) -> Option<u32> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    let rest = inner.strip_prefix("mosh-server detached, pid = ")?;
    rest.trim().parse().ok()
}

/// A short, single-line excerpt of the server's stderr. mosh-server never writes the key to
/// stderr, but nothing from stdout is quoted at all.
fn detail(stderr: &str) -> String {
    let mut text: String = stderr.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.len() > DETAIL_BYTES {
        let mut end = DETAIL_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("...");
    }
    text
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tokio::io::DuplexStream;

    use super::*;

    const KEY: &str = "zr0jtuYVKJnfJHP/XOZs7A";

    fn size() -> TerminalSize {
        TerminalSize::new(100, 30).unwrap()
    }

    fn output(status: Option<u32>, stdout: &str, stderr: &str) -> ExecOutput {
        ExecOutput {
            status,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    fn caps(mosh_server: Option<&str>) -> HostCapabilities {
        HostCapabilities {
            tmux: None,
            herdr: None,
            mosh_server: mosh_server.map(str::to_owned),
            utf8_locale: "C.UTF-8".into(),
            herdr_sessions: Vec::new(),
        }
    }

    /// A host that records the command line it is asked to run and answers with a fixed output.
    struct FakeHost {
        ran: Mutex<Vec<String>>,
        answer: Result<ExecOutput, RemoteError>,
    }

    impl FakeHost {
        fn new(answer: Result<ExecOutput, RemoteError>) -> Self {
            Self {
                ran: Mutex::new(Vec::new()),
                answer,
            }
        }
    }

    impl RemoteHost for FakeHost {
        type Stream = DuplexStream;

        async fn exec_rendered(&self, line: &str) -> Result<ExecOutput, RemoteError> {
            self.ran.lock().unwrap().push(line.to_owned());
            self.answer.clone()
        }

        async fn open_unix(&self, _path: &str) -> Result<DuplexStream, RemoteError> {
            Err(RemoteError::Rejected("not supported".into()))
        }
    }

    #[test]
    fn the_command_line_is_mosh_servers_new_with_the_locale_twice() {
        let line = command("/usr/bin/mosh-server", "C.UTF-8", &[])
            .render()
            .unwrap();
        assert_eq!(
            line,
            "env 'LANG=C.UTF-8' '/usr/bin/mosh-server' 'new' '-s' '-c' '256' '-l' 'LANG=C.UTF-8'"
        );
        let target = [
            "tmux".to_owned(),
            "new-session".into(),
            "-A".into(),
            "-s".into(),
            "it's".into(),
        ];
        let line = command("/usr/bin/mosh-server", "en_US.UTF-8", &target)
            .render()
            .unwrap();
        assert_eq!(
            line,
            "env 'LANG=en_US.UTF-8' '/usr/bin/mosh-server' 'new' '-s' '-c' '256' '-l' \
             'LANG=en_US.UTF-8' '--' 'tmux' 'new-session' '-A' '-s' 'it'\\''s'"
        );
    }

    #[test]
    fn a_target_cannot_smuggle_shell_syntax_into_the_line() {
        let target = ["sh".to_owned(), "-c".into(), "x; rm -rf ~".into()];
        let line = command("/usr/bin/mosh-server", "C.UTF-8", &target)
            .render()
            .unwrap();
        // One quoted token, not a command separator.
        assert!(line.ends_with("'sh' '-c' 'x; rm -rf ~'"), "{line}");
    }

    #[test]
    fn parses_the_connect_line_and_the_detached_pid() {
        let parsed = parse_output(
            &output(
                Some(0),
                &format!("\n\nMOSH CONNECT 60101 {KEY}\n\nmosh-server (mosh 1.4.0)\n"),
                "[mosh-server detached, pid = 1913778]\n",
            ),
            size(),
        )
        .unwrap();
        assert_eq!(parsed.port, 60101);
        assert_eq!(parsed.server_pid, Some(1913778));
        assert_eq!(parsed.size, size());
        assert_eq!(
            parsed.key.to_base64_key().unwrap().printable().as_str(),
            KEY
        );
    }

    #[test]
    fn the_pid_line_is_found_on_either_stream_and_may_be_absent() {
        let both = format!("MOSH CONNECT 7 {KEY}\n[mosh-server detached, pid = 42]\n");
        assert_eq!(
            parse_output(&output(Some(0), &both, ""), size())
                .unwrap()
                .server_pid,
            Some(42)
        );
        let none = format!("MOSH CONNECT 7 {KEY}\n");
        assert_eq!(
            parse_output(&output(Some(0), &none, ""), size())
                .unwrap()
                .server_pid,
            None
        );
    }

    #[test]
    fn a_missing_or_malformed_connect_line_is_a_typed_error() {
        assert_eq!(
            parse_output(&output(Some(0), "hello\n", ""), size()).unwrap_err(),
            BootstrapError::NoConnectLine {
                detail: String::new()
            }
        );
        assert_eq!(
            parse_output(&output(Some(0), "MOSH CONNECT\n", ""), size()).unwrap_err(),
            BootstrapError::InvalidPort
        );
        for port in ["0", "65536", "-1", "http"] {
            assert_eq!(
                parse_output(
                    &output(Some(0), &format!("MOSH CONNECT {port} {KEY}\n"), ""),
                    size()
                )
                .unwrap_err(),
                BootstrapError::InvalidPort,
                "{port}"
            );
        }
        for key in [
            "",
            "short",
            "zr0jtuYVKJnfJHP/XOZs7-",
            "AAAAAAAAAAAAAAAAAAAAAB",
        ] {
            assert_eq!(
                parse_output(
                    &output(Some(0), &format!("MOSH CONNECT 60001 {key}\n"), ""),
                    size()
                )
                .unwrap_err(),
                BootstrapError::InvalidKey,
                "{key:?}"
            );
        }
    }

    #[test]
    fn a_failing_server_reports_its_status_and_stderr_but_never_stdout() {
        let error = parse_output(
            &output(
                Some(1),
                &format!("secret-looking {KEY}\n"),
                "mosh-server needs a UTF-8 native locale to run.\nUnfortunately, the local\n",
            ),
            size(),
        )
        .unwrap_err();
        let BootstrapError::Failed { status, detail } = &error else {
            panic!("{error:?}");
        };
        assert_eq!(*status, Some(1));
        assert!(
            detail.starts_with("mosh-server needs a UTF-8 native locale to run. Unfortunately")
        );
        assert!(!error.to_string().contains(KEY));
    }

    #[test]
    fn long_stderr_is_truncated_on_a_character_boundary() {
        let stderr = "é".repeat(1000);
        let error = parse_output(&output(Some(2), "", &stderr), size()).unwrap_err();
        let BootstrapError::Failed { detail, .. } = error else {
            panic!();
        };
        assert!(detail.ends_with("..."));
        assert!(detail.len() <= DETAIL_BYTES + 3);
    }

    #[test]
    fn the_key_is_redacted_in_debug() {
        let parsed = parse_output(
            &output(Some(0), &format!("MOSH CONNECT 60101 {KEY}\n"), ""),
            size(),
        )
        .unwrap();
        for text in [format!("{:?}", parsed), format!("{:?}", parsed.key)] {
            assert!(!text.contains(KEY), "{text}");
            assert!(text.contains("redacted"), "{text}");
        }
    }

    #[tokio::test]
    async fn bootstrap_runs_the_probed_server_with_the_probed_locale() {
        let host = FakeHost::new(Ok(output(
            Some(0),
            &format!("MOSH CONNECT 60222 {KEY}\n"),
            "[mosh-server detached, pid = 5]\n",
        )));
        let target = ["tmux".to_owned(), "attach".into()];
        let params = bootstrap(&host, &caps(Some("/opt/bin/mosh-server")), size(), &target)
            .await
            .unwrap();
        assert_eq!((params.port, params.server_pid), (60222, Some(5)));
        let ran = host.ran.lock().unwrap();
        assert_eq!(ran.len(), 1);
        assert_eq!(
            ran[0],
            "env 'LANG=C.UTF-8' '/opt/bin/mosh-server' 'new' '-s' '-c' '256' '-l' \
             'LANG=C.UTF-8' '--' 'tmux' 'attach'"
        );
    }

    #[tokio::test]
    async fn bootstrap_without_a_probed_server_runs_nothing() {
        let host = FakeHost::new(Ok(output(Some(0), "", "")));
        let error = bootstrap(&host, &caps(None), size(), &[])
            .await
            .unwrap_err();
        assert_eq!(error, BootstrapError::NotInstalled);
        assert!(host.ran.lock().unwrap().is_empty());
        assert_eq!(
            error.into_failure(),
            SessionFailure::NotInstalled {
                program: "mosh-server".into()
            }
        );
    }

    #[tokio::test]
    async fn remote_errors_pass_through_typed() {
        for (error, failure) in [
            (RemoteError::TimedOut, SessionFailure::TimedOut),
            (
                RemoteError::Closed,
                SessionFailure::ConnectionLost("the host connection closed".into()),
            ),
        ] {
            let host = FakeHost::new(Err(error.clone()));
            let got = bootstrap(&host, &caps(Some("/x/mosh-server")), size(), &[])
                .await
                .unwrap_err();
            assert_eq!(got, BootstrapError::Remote(error));
            assert_eq!(got.into_failure(), failure);
        }
        // A broken transport is a lost connection, not a failed command.
        let host = FakeHost::new(Err(RemoteError::Io("boom".into())));
        let got = bootstrap(&host, &caps(Some("/x/mosh-server")), size(), &[])
            .await
            .unwrap_err();
        assert!(matches!(
            got.into_failure(),
            SessionFailure::ConnectionLost(reason) if reason.contains("boom")
        ));
        for error in [
            RemoteError::Rejected("no".into()),
            RemoteError::OutputTooLarge,
        ] {
            let host = FakeHost::new(Err(error));
            let got = bootstrap(&host, &caps(Some("/x/mosh-server")), size(), &[])
                .await
                .unwrap_err();
            assert!(matches!(
                got.into_failure(),
                SessionFailure::CommandFailed(_)
            ));
        }
    }

    #[tokio::test]
    async fn terminate_signals_only_a_process_named_mosh_server() {
        let host = FakeHost::new(Ok(output(Some(0), "", "")));
        terminate(&host, 4242).await.unwrap();
        let ran = host.ran.lock().unwrap();
        assert_eq!(ran.len(), 1);
        let line = &ran[0];
        assert!(line.starts_with("sh -c '"), "{line}");
        // The id is checked against the process name before anything is signalled.
        assert!(line.contains("ps -p 4242 -o comm="), "{line}");
        assert!(line.contains("*mosh-server) kill -TERM 4242"), "{line}");
        assert!(!line.contains("-KILL"), "{line}");
    }

    #[tokio::test]
    async fn a_server_that_started_with_an_unusable_answer_is_stopped() {
        let host = FakeHost::new(Ok(output(
            Some(0),
            &format!("MOSH CONNECT 70000 {KEY}\n"),
            "[mosh-server detached, pid = 77]\n",
        )));
        let error = bootstrap(&host, &caps(Some("/x/mosh-server")), size(), &[])
            .await
            .unwrap_err();
        assert_eq!(error, BootstrapError::InvalidPort);
        {
            let ran = host.ran.lock().unwrap();
            assert_eq!(ran.len(), 2, "the start, then the cleanup");
            assert!(ran[1].contains("kill -TERM 77"), "{}", ran[1]);
        }

        // The detached pid printed on stdout, next to the unusable CONNECT line, is read before
        // stdout is wiped.
        let host = FakeHost::new(Ok(output(
            Some(0),
            &format!("MOSH CONNECT 70000 {KEY}\n[mosh-server detached, pid = 78]\n"),
            "",
        )));
        let error = bootstrap(&host, &caps(Some("/x/mosh-server")), size(), &[])
            .await
            .unwrap_err();
        assert_eq!(error, BootstrapError::InvalidPort);
        {
            let ran = host.ran.lock().unwrap();
            assert_eq!(ran.len(), 2, "the start, then the cleanup");
            assert!(ran[1].contains("kill -TERM 78"), "{}", ran[1]);
        }

        // A failure without a detached server has nothing to stop.
        let host = FakeHost::new(Ok(output(Some(1), "", "no locale")));
        bootstrap(&host, &caps(Some("/x/mosh-server")), size(), &[])
            .await
            .unwrap_err();
        assert_eq!(host.ran.lock().unwrap().len(), 1);
    }
}
