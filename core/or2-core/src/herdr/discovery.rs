//! Finding sessions and their sockets: `<herdr> session list --json` over the host's exec
//! channel. Paths are never guessed; the listing is the only source. [`list_sessions`] is the
//! one parser: the watch, `focus_pane` and the capability probe all read the listing through it.

use serde::Deserialize;

use crate::remote::{RemoteCommand, RemoteError, RemoteHost};

/// What stderr snippet a diagnostic may carry, in characters.
const STDERR_SNIPPET: usize = 200;

/// The exit status herdr's argument parser uses for a usage error. `session list --json` is a
/// usage error only on a herdr that predates it.
const USAGE_STATUS: u32 = 2;

/// `herdr session list --json`. Only the fields or2 uses; everything else is ignored.
#[derive(Debug, Deserialize)]
struct Listing {
    sessions: Vec<SessionEntry>,
}

/// One session of `herdr session list --json`, in herdr's order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SessionEntry {
    pub name: String,
    /// The session herdr uses when none is named.
    #[serde(default)]
    pub default: bool,
    /// Its server is up.
    #[serde(default)]
    pub running: bool,
    /// Never reported to Kotlin: the herdr client rediscovers it on every attempt.
    pub(crate) socket_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiscoveryError {
    #[error(transparent)]
    Remote(#[from] RemoteError),
    /// The command is not there (shell status 126 or 127).
    #[error("herdr is not installed")]
    NotInstalled,
    /// herdr is there but rejected `session list --json` as a usage error (exit status 2): a
    /// release that predates it, so older than the supported protocol.
    #[error("this herdr does not support `session list --json`")]
    Unsupported,
    /// No such session, or its server is stopped.
    #[error("{0}")]
    NotRunning(String),
    #[error("{0}")]
    Failed(String),
}

/// Every session herdr lists, in its order. Exit status 126/127 is
/// [`DiscoveryError::NotInstalled`], 2 is [`DiscoveryError::Unsupported`]; another failure or
/// unreadable output is [`DiscoveryError::Failed`].
pub async fn list_sessions<H: RemoteHost>(
    host: &H,
    herdr: &str,
) -> Result<Vec<SessionEntry>, DiscoveryError> {
    let output = host
        .exec(&RemoteCommand::new(herdr).args(["session", "list", "--json"]))
        .await?;
    match output.status {
        Some(0) => {}
        Some(126 | 127) => return Err(DiscoveryError::NotInstalled),
        Some(USAGE_STATUS) => return Err(DiscoveryError::Unsupported),
        status => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let line: String = stderr
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(STDERR_SNIPPET)
                .collect();
            let status = status.map_or_else(|| "no status".to_owned(), |code| code.to_string());
            return Err(DiscoveryError::Failed(format!(
                "herdr session list failed ({status}): {line}"
            )));
        }
    }
    let listing: Listing = serde_json::from_slice(&output.stdout)
        .map_err(|error| DiscoveryError::Failed(format!("unreadable session list: {error}")))?;
    Ok(listing.sessions)
}

/// The socket of `session` (`None` is the entry marked `default`). The session must be
/// running; a missing or stopped one is [`DiscoveryError::NotRunning`].
pub async fn locate<H: RemoteHost>(
    host: &H,
    herdr: &str,
    session: Option<&str>,
) -> Result<String, DiscoveryError> {
    let entry = list_sessions(host, herdr)
        .await?
        .into_iter()
        .find(|entry| match session {
            None => entry.default,
            Some(name) => entry.name == name,
        });
    match entry {
        None => Err(DiscoveryError::NotRunning(match session {
            None => "herdr lists no default session".into(),
            Some(_) => "herdr has no such session".into(),
        })),
        Some(entry) if !entry.running => Err(DiscoveryError::NotRunning(
            "the herdr session is not running".into(),
        )),
        Some(entry) => Ok(entry.socket_path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::testing::FakeHost;

    const LISTING: &str = r#"{"sessions":[
        {"default":true,"name":"default","running":true,"session_dir":"/home/user/.config/herdr","socket_path":"/home/user/.config/herdr/herdr.sock","future":1},
        {"default":false,"name":"work","running":true,"session_dir":"/x","socket_path":"/home/user/.config/herdr/sessions/work/herdr.sock"},
        {"default":false,"name":"idle","running":false,"session_dir":"/y","socket_path":"/home/user/.config/herdr/sessions/idle/herdr.sock"}
    ]}"#;

    #[tokio::test]
    async fn the_default_session_is_the_entry_marked_default() {
        let host = FakeHost::new();
        host.set_listing(LISTING);
        assert_eq!(
            locate(&host, "/opt/herdr", None).await.unwrap(),
            "/home/user/.config/herdr/herdr.sock"
        );
        assert_eq!(
            locate(&host, "/opt/herdr", Some("work")).await.unwrap(),
            "/home/user/.config/herdr/sessions/work/herdr.sock"
        );
        // The path is passed as one argument, not through a shell string.
        assert_eq!(
            host.exec_log(),
            [
                "'/opt/herdr' 'session' 'list' '--json'",
                "'/opt/herdr' 'session' 'list' '--json'"
            ]
        );
    }

    #[tokio::test]
    async fn the_listing_reports_name_default_and_running_in_herdrs_order() {
        let host = FakeHost::new();
        host.set_listing(LISTING);
        let sessions = list_sessions(&host, "/opt/herdr").await.unwrap();
        let summary: Vec<_> = sessions
            .iter()
            .map(|entry| (entry.name.as_str(), entry.default, entry.running))
            .collect();
        assert_eq!(
            summary,
            [
                ("default", true, true),
                ("work", false, true),
                ("idle", false, false)
            ]
        );
        assert_eq!(
            sessions[1].socket_path,
            "/home/user/.config/herdr/sessions/work/herdr.sock"
        );
        // Missing flags read as false; an empty listing is not an error.
        host.set_listing(r#"{"sessions":[{"name":"x","socket_path":"/s"}]}"#);
        let sessions = list_sessions(&host, "h").await.unwrap();
        assert!(!sessions[0].default && !sessions[0].running);
        host.set_listing(r#"{"sessions":[]}"#);
        assert!(list_sessions(&host, "h").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn missing_and_stopped_sessions_are_not_running() {
        let host = FakeHost::new();
        host.set_listing(LISTING);
        for name in ["idle", "nope"] {
            assert!(
                matches!(
                    locate(&host, "h", Some(name)).await,
                    Err(DiscoveryError::NotRunning(_))
                ),
                "{name}"
            );
        }
        host.set_listing(r#"{"sessions":[]}"#);
        assert!(matches!(
            locate(&host, "h", None).await,
            Err(DiscoveryError::NotRunning(_))
        ));
    }

    #[tokio::test]
    async fn command_failures_are_classified() {
        let host = FakeHost::new();
        host.set_exec(127, "", "sh: herdr: not found");
        assert_eq!(
            locate(&host, "h", None).await,
            Err(DiscoveryError::NotInstalled)
        );
        // herdr's usage-error status: `session list --json` is not a command of this herdr.
        host.set_exec(2, "", "herdr: unknown command");
        assert_eq!(
            locate(&host, "h", None).await,
            Err(DiscoveryError::Unsupported)
        );
        assert_eq!(
            list_sessions(&host, "h").await,
            Err(DiscoveryError::Unsupported)
        );
        host.set_exec(1, "", "boom\nsecond line");
        assert_eq!(
            locate(&host, "h", None).await,
            Err(DiscoveryError::Failed(
                "herdr session list failed (1): boom".into()
            ))
        );
        host.set_exec(0, "not json", "");
        assert!(matches!(
            locate(&host, "h", None).await,
            Err(DiscoveryError::Failed(_))
        ));
        host.set_exec_error(RemoteError::Closed);
        assert_eq!(
            locate(&host, "h", None).await,
            Err(DiscoveryError::Remote(RemoteError::Closed))
        );
    }
}
