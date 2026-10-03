//! herdr's agent integrations (contracts.md, "v0.1.3: zero-config Reply"): the hook inside an
//! agent that reports its `agent_session` to herdr, without which a notification has no Reply.
//! Both calls run herdr's own CLI over exec, as one command with an argument vector (no shell
//! string is built from input; the id comes from [`INTEGRATIONS`] only):
//!
//! - [`install_integration`]: `<herdr> integration install <id>`; exit 0 is done.
//! - [`integration_states`]: `<herdr> integration status`, text only, one line per integration:
//!   `<id>[ (experimental)]: <state> (<path>)`, the state `current (vN)`, `not installed`, or
//!   anything else for one that is installed but outdated. Only the ids and states leave this
//!   module: the paths name the user's home.

use crate::host::HostError;
use crate::remote::{ExecOutput, RemoteCommand, RemoteHost};

/// The integrations herdr 0.9.3 offers: the only ids or2 ever asks herdr to install.
pub const INTEGRATIONS: [&str; 18] = [
    "pi",
    "omp",
    "claude",
    "codex",
    "copilot",
    "devin",
    "droid",
    "kimi",
    "opencode",
    "kilo",
    "hermes",
    "qodercli",
    "qwen",
    "cursor",
    "mastracode",
    "antigravity-cli",
    "grok",
    "letta",
];

/// Whether `id` is one of [`INTEGRATIONS`], exactly.
pub fn is_integration(id: &str) -> bool {
    INTEGRATIONS.contains(&id)
}

/// What `herdr integration status` says of one integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationState {
    /// Installed, the version this herdr ships (`current (vN)`).
    Current,
    /// Installed, an older version: installing again updates it.
    Outdated,
    NotInstalled,
}

/// One line of `herdr integration status`, for an id of [`INTEGRATIONS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Integration {
    pub id: String,
    pub state: IntegrationState,
}

/// `<herdr> integration install <id>`.
pub fn install_command(herdr: &str, id: &str) -> RemoteCommand {
    RemoteCommand::new(herdr).args(["integration", "install", id])
}

/// `<herdr> integration status`.
pub fn status_command(herdr: &str) -> RemoteCommand {
    RemoteCommand::new(herdr).args(["integration", "status"])
}

/// Installs herdr's integration `id` with the herdr at `herdr`: one exec, bounded by the exec
/// timeout. An id that is not one of [`INTEGRATIONS`] is `InvalidName` with nothing run. Exit 0
/// is `Ok`; 126 or 127 (herdr gone since the probe) `NotInstalled`; any other exit
/// `CommandFailed` with the first line of its stderr.
pub async fn install_integration<H: RemoteHost>(
    host: &H,
    herdr: &str,
    id: &str,
) -> Result<(), HostError> {
    if !is_integration(id) {
        return Err(HostError::InvalidName);
    }
    let output = host.exec(&install_command(herdr, id)).await?;
    checked(&output)
}

/// The state of every integration of [`INTEGRATIONS`] that `herdr integration status` lists,
/// in its order. Lines for other ids, and lines that are not `<id>: <state>`, are skipped.
/// Failures as for [`install_integration`].
pub async fn integration_states<H: RemoteHost>(
    host: &H,
    herdr: &str,
) -> Result<Vec<Integration>, HostError> {
    let output = host.exec(&status_command(herdr)).await?;
    checked(&output)?;
    Ok(parse_status(&String::from_utf8_lossy(&output.stdout)))
}

fn checked(output: &ExecOutput) -> Result<(), HostError> {
    match output.status {
        Some(0) => Ok(()),
        Some(126 | 127) => Err(HostError::NotInstalled {
            program: "herdr".into(),
        }),
        _ => Err(HostError::CommandFailed {
            message: output.stderr_line(),
        }),
    }
}

/// See [`integration_states`].
pub fn parse_status(text: &str) -> Vec<Integration> {
    text.lines()
        .filter_map(|line| {
            let (head, state) = line.trim().split_once(": ")?;
            let id = head.strip_suffix(" (experimental)").unwrap_or(head).trim();
            let state = state.trim();
            if !is_integration(id) || state.is_empty() {
                return None;
            }
            let state = if state.starts_with("current") {
                IntegrationState::Current
            } else if state.starts_with("not installed") {
                IntegrationState::NotInstalled
            } else {
                IntegrationState::Outdated
            };
            Some(Integration {
                id: id.to_owned(),
                state,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::remote::RemoteError;

    /// `herdr integration status` of herdr 0.9.3, the home directory replaced.
    const STATUS_0_9_3: &str = "\
pi: current (v9) (/home/user/.pi/agent/extensions/herdr-agent-state.ts)
omp: not installed (/home/user/.omp/agent/extensions/herdr-omp-agent-state.ts)
claude: current (v10) (/home/user/.claude/hooks/herdr-agent-state.sh)
codex: current (v8) (/home/user/.codex/herdr-agent-state.sh)
copilot: not installed (/home/user/.copilot/hooks/herdr-agent-state.sh)
devin: not installed (/home/user/.config/devin/herdr-agent-state.sh)
droid: not installed (/home/user/.factory/hooks/herdr-agent-state.sh)
kimi: not installed (/home/user/.kimi-code/hooks/herdr-agent-state.sh)
opencode: outdated (v3, current v5) (/home/user/.config/opencode/plugins/herdr-agent-state.js)
kilo: not installed (/home/user/.config/kilo/plugin/herdr-agent-state.js)
hermes: not installed (/home/user/.hermes/plugins/herdr-agent-state/__init__.py)
qodercli: not installed (/home/user/.qoder/hooks/herdr-agent-state.sh)
qwen: not installed (/home/user/.qwen/hooks/herdr-agent-session.sh)
cursor: not installed (/home/user/.cursor/herdr-agent-state.sh)
mastracode: not installed (/home/user/.mastracode/hooks/herdr-agent-state.sh)
antigravity-cli: not installed (/home/user/.gemini/config/hooks/herdr-agent-state.sh)
grok: not installed (/home/user/.grok/hooks/herdr-agent-state.sh)
letta (experimental): not installed (/home/user/.letta/hooks/herdr-agent-session.sh)
";

    /// Answers each exec with the next scripted output and records the command lines.
    #[derive(Default)]
    struct Scripted {
        replies: Mutex<VecDeque<Result<ExecOutput, RemoteError>>>,
        log: Mutex<Vec<String>>,
    }

    impl Scripted {
        fn reply(&self, status: u32, stdout: &str, stderr: &str) {
            self.replies.lock().unwrap().push_back(Ok(ExecOutput {
                status: Some(status),
                stdout: stdout.as_bytes().into(),
                stderr: stderr.as_bytes().into(),
            }));
        }

        fn fail(&self, error: RemoteError) {
            self.replies.lock().unwrap().push_back(Err(error));
        }

        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    impl RemoteHost for Scripted {
        type Stream = tokio::io::DuplexStream;

        async fn exec_rendered(&self, line: &str) -> Result<ExecOutput, RemoteError> {
            self.log.lock().unwrap().push(line.to_owned());
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("a scripted reply")
        }

        async fn open_unix(&self, _: &str) -> Result<Self::Stream, RemoteError> {
            Err(RemoteError::Closed)
        }
    }

    const HERDR: &str = "/home/user/.local/bin/herdr";

    #[test]
    fn the_allowlist_is_herdrs_integrations_and_nothing_near_them() {
        for id in INTEGRATIONS {
            assert!(is_integration(id), "{id}");
        }
        for id in [
            "",
            "Pi",
            "pi ",
            " pi",
            "pi;rm",
            "cursor-agent",
            "agy",
            "antigravity_cli",
            "amp",
            "gemini",
            "--help",
            "pi\n",
        ] {
            assert!(!is_integration(id), "{id:?}");
        }
    }

    #[test]
    fn the_install_is_one_argument_vector_with_the_probed_herdr() {
        let command = install_command(HERDR, "antigravity-cli");
        assert_eq!(
            command.argv().unwrap(),
            [HERDR, "integration", "install", "antigravity-cli"]
        );
        assert_eq!(
            command.render().unwrap(),
            "'/home/user/.local/bin/herdr' 'integration' 'install' 'antigravity-cli'"
        );
        assert_eq!(
            status_command(HERDR).argv().unwrap(),
            [HERDR, "integration", "status"]
        );
    }

    #[tokio::test]
    async fn an_install_is_ok_on_exit_zero_and_says_why_otherwise() {
        let host = Scripted::default();
        host.reply(0, "installed pi integration\n", "");
        assert_eq!(install_integration(&host, HERDR, "pi").await, Ok(()));
        host.reply(1, "", "error: permission denied\nmore detail\n");
        assert_eq!(
            install_integration(&host, HERDR, "droid").await,
            Err(HostError::CommandFailed {
                message: "error: permission denied".into()
            })
        );
        // An exit with nothing on stderr still says something.
        host.reply(2, "", "");
        assert!(matches!(
            install_integration(&host, HERDR, "kimi").await,
            Err(HostError::CommandFailed { message }) if message.contains('2')
        ));
        assert_eq!(
            host.log(),
            [
                "'/home/user/.local/bin/herdr' 'integration' 'install' 'pi'",
                "'/home/user/.local/bin/herdr' 'integration' 'install' 'droid'",
                "'/home/user/.local/bin/herdr' 'integration' 'install' 'kimi'",
            ]
        );
    }

    #[tokio::test]
    async fn herdr_gone_since_the_probe_is_not_installed() {
        let host = Scripted::default();
        host.reply(127, "", "sh: herdr: not found\n");
        host.reply(126, "", "");
        let not_installed = Err(HostError::NotInstalled {
            program: "herdr".into(),
        });
        assert_eq!(install_integration(&host, HERDR, "pi").await, not_installed);
        assert_eq!(
            integration_states(&host, HERDR).await.map(drop),
            not_installed
        );
    }

    #[tokio::test]
    async fn an_id_off_the_allowlist_runs_nothing() {
        let host = Scripted::default();
        for id in ["", "amp", "pi; reboot", "$(id)", "claude code"] {
            assert_eq!(
                install_integration(&host, HERDR, id).await,
                Err(HostError::InvalidName)
            );
        }
        assert!(host.log().is_empty());
    }

    #[tokio::test]
    async fn a_lost_connection_is_closed_and_a_slow_host_failed() {
        let host = Scripted::default();
        host.fail(RemoteError::Closed);
        host.fail(RemoteError::TimedOut);
        assert_eq!(
            install_integration(&host, HERDR, "pi").await,
            Err(HostError::Closed)
        );
        assert!(matches!(
            integration_states(&host, HERDR).await,
            Err(HostError::CommandFailed { .. })
        ));
    }

    #[tokio::test]
    async fn the_status_of_herdr_0_9_3_reads_every_integration() {
        let host = Scripted::default();
        host.reply(0, STATUS_0_9_3, "");
        let states = integration_states(&host, HERDR).await.unwrap();
        assert_eq!(
            states.iter().map(|it| it.id.as_str()).collect::<Vec<_>>(),
            INTEGRATIONS
        );
        let state = |id: &str| states.iter().find(|it| it.id == id).unwrap().state;
        assert_eq!(state("pi"), IntegrationState::Current);
        assert_eq!(state("claude"), IntegrationState::Current);
        assert_eq!(state("omp"), IntegrationState::NotInstalled);
        assert_eq!(state("opencode"), IntegrationState::Outdated);
        // `(experimental)` is not part of the id.
        assert_eq!(state("letta"), IntegrationState::NotInstalled);
        assert_eq!(
            host.log(),
            ["'/home/user/.local/bin/herdr' 'integration' 'status'"]
        );
    }

    #[test]
    fn garbage_and_unknown_ids_are_skipped() {
        let text = "\
Usage: herdr integration [COMMAND]

amp: not installed (/x)
pi current (v9)
claude:
codex: current (v8) (/x)
  kimi (experimental):   not installed
";
        assert_eq!(
            parse_status(text),
            [
                Integration {
                    id: "codex".into(),
                    state: IntegrationState::Current
                },
                Integration {
                    id: "kimi".into(),
                    state: IntegrationState::NotInstalled
                },
            ]
        );
        assert!(parse_status("").is_empty());
        assert!(parse_status("\u{1b}[31m: boom").is_empty());
    }

    #[tokio::test]
    async fn a_failed_status_says_why() {
        let host = Scripted::default();
        host.reply(2, "", "error: unrecognized subcommand 'integration'\n");
        assert_eq!(
            integration_states(&host, HERDR).await,
            Err(HostError::CommandFailed {
                message: "error: unrecognized subcommand 'integration'".into()
            })
        );
    }
}
