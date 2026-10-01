//! Finding sessions and their sockets: `<herdr> session list --json` over the host's exec
//! channel. Paths are never guessed; the listing is the only source. [`list_sessions`] is the
//! one parser: the watch, `focus_pane` and the capability probe all read the listing through it.
//!
//! A host connection keeps one [`Directory`]: the capability probe seeds it with the listing it
//! read, and the watches and pane focuses take a session's socket from it instead of running
//! `session list` again. A socket that cannot be reached is the signal to read the listing
//! afresh ([`Directory::invalidate`]); nothing else makes a client re-discover.

use std::sync::{Mutex, MutexGuard, PoisonError};

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
    /// Never reported to Kotlin: the herdr client takes it from the connection's [`Directory`].
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

/// Reads a finished `session list --json` (`status`, what it printed on stdout and stderr):
/// exit status 126/127 is [`DiscoveryError::NotInstalled`], 2 is [`DiscoveryError::Unsupported`];
/// another failure or unreadable output is [`DiscoveryError::Failed`]. The one place that
/// classifies a listing, whether it ran as its own exec or inside the capability probe's script.
pub(crate) fn parse_listing(
    status: Option<u32>,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<Vec<SessionEntry>, DiscoveryError> {
    match status {
        Some(0) => {}
        Some(126 | 127) => return Err(DiscoveryError::NotInstalled),
        Some(USAGE_STATUS) => return Err(DiscoveryError::Unsupported),
        status => {
            let stderr = String::from_utf8_lossy(stderr);
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
    let listing: Listing = serde_json::from_slice(stdout)
        .map_err(|error| DiscoveryError::Failed(format!("unreadable session list: {error}")))?;
    Ok(listing.sessions)
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
    parse_listing(output.status, &output.stdout, &output.stderr)
}

/// The entry for `session` (`None` is the one marked `default`) as a socket, which must be
/// running; a missing or stopped session is [`DiscoveryError::NotRunning`].
fn socket_of(entries: &[SessionEntry], session: Option<&str>) -> Result<String, DiscoveryError> {
    let entry = entries.iter().find(|entry| match session {
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
        Some(entry) => Ok(entry.socket_path.clone()),
    }
}

/// The socket of `session` (`None` is the entry marked `default`), from a listing read now.
/// The session must be running; a missing or stopped one is [`DiscoveryError::NotRunning`].
#[cfg(test)]
pub async fn locate<H: RemoteHost>(
    host: &H,
    herdr: &str,
    session: Option<&str>,
) -> Result<String, DiscoveryError> {
    socket_of(&list_sessions(host, herdr).await?, session)
}

/// What one host connection knows about herdr's sessions: the last listing that was read
/// successfully, and whether it may still be trusted. Every method takes `&self`.
///
/// Reads can overlap, so each takes a ticket when it starts and a result is applied only if no
/// read that started later has already been applied: a slow older read can never overwrite a
/// newer list.
#[derive(Debug, Default)]
pub struct Directory {
    state: Mutex<DirectoryState>,
}

#[derive(Debug, Default)]
struct DirectoryState {
    /// The last ticket handed out.
    issued: u64,
    /// The ticket of the read whose list is stored.
    applied: u64,
    /// `None` until a read succeeds or the probe seeds it.
    entries: Option<Vec<SessionEntry>>,
    /// A socket failed since the list was read: the next lookup reads the listing again.
    stale: bool,
    /// The list was seeded by the capability probe and no `capabilities()` call has reported it
    /// yet, so that first call does not read it a second time.
    unread: bool,
}

impl Directory {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, DirectoryState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The listing the capability probe read: stored as the newest, and left unread for the
    /// first `capabilities()` call.
    pub fn seed(&self, entries: Vec<SessionEntry>) {
        let mut state = self.lock();
        state.issued += 1;
        state.applied = state.issued;
        state.entries = Some(entries);
        state.stale = false;
        state.unread = true;
    }

    /// True once after a [`Directory::seed`]: that list is as fresh as a read made now.
    pub fn take_unread(&self) -> bool {
        std::mem::take(&mut self.lock().unread)
    }

    /// The last list that was read successfully.
    pub fn entries(&self) -> Option<Vec<SessionEntry>> {
        self.lock().entries.clone()
    }

    /// A socket from this list could not be used (it did not open): the next lookup reads the
    /// listing again instead of trusting it.
    pub fn invalidate(&self) {
        self.lock().stale = true;
    }

    /// Reads the listing and stores it unless a read that started later is already stored.
    /// Returns the stored list, which is a newer read's when this one lost. A failed read
    /// leaves the stored list alone.
    pub async fn refresh<H: RemoteHost>(
        &self,
        host: &H,
        herdr: &str,
    ) -> Result<Vec<SessionEntry>, DiscoveryError> {
        let ticket = {
            let mut state = self.lock();
            state.issued += 1;
            state.issued
        };
        let read = list_sessions(host, herdr).await?;
        let mut state = self.lock();
        if ticket > state.applied {
            state.applied = ticket;
            state.entries = Some(read);
            state.stale = false;
            state.unread = false;
        }
        Ok(state.entries.clone().unwrap_or_default())
    }

    /// The socket of a session the stored list calls running, when it can be trusted. A session
    /// the list does not know or calls stopped is not answered from it: it may have started
    /// since, which only a new read can tell.
    pub(crate) fn cached_socket(&self, session: Option<&str>) -> Option<String> {
        let state = self.lock();
        if state.stale {
            return None;
        }
        socket_of(state.entries.as_deref()?, session).ok()
    }

    /// The socket of `session`: from the stored list when it is trusted and calls the session
    /// running, else from a listing read now (which is then stored).
    pub async fn locate<H: RemoteHost>(
        &self,
        host: &H,
        herdr: &str,
        session: Option<&str>,
    ) -> Result<String, DiscoveryError> {
        if let Some(socket) = self.cached_socket(session) {
            return Ok(socket);
        }
        self.locate_fresh(host, herdr, session).await
    }

    /// [`Directory::locate`] that always reads the listing: the caller just saw the cached
    /// socket fail.
    pub async fn locate_fresh<H: RemoteHost>(
        &self,
        host: &H,
        herdr: &str,
        session: Option<&str>,
    ) -> Result<String, DiscoveryError> {
        socket_of(&self.refresh(host, herdr).await?, session)
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

    #[tokio::test]
    async fn the_directory_serves_running_sessions_from_its_list_until_a_socket_fails() {
        let host = FakeHost::new();
        host.set_listing(LISTING);
        let directory = Directory::new();
        // Nothing is known yet: the first lookup reads the listing, the next ones do not.
        assert_eq!(
            directory.locate(&host, "h", None).await.unwrap(),
            "/home/user/.config/herdr/herdr.sock"
        );
        assert_eq!(
            directory.locate(&host, "h", Some("work")).await.unwrap(),
            "/home/user/.config/herdr/sessions/work/herdr.sock"
        );
        assert_eq!(host.exec_log().len(), 1);
        // A stopped or unknown session is not answered from the list: it may have started since.
        for name in ["idle", "nope"] {
            assert!(directory.cached_socket(Some(name)).is_none());
            assert!(matches!(
                directory.locate(&host, "h", Some(name)).await,
                Err(DiscoveryError::NotRunning(_))
            ));
        }
        assert_eq!(host.exec_log().len(), 3);
        // A socket that failed makes the next lookup read the listing again.
        directory.invalidate();
        assert!(directory.cached_socket(None).is_none());
        directory.locate(&host, "h", None).await.unwrap();
        assert_eq!(host.exec_log().len(), 4);
        assert!(directory.cached_socket(None).is_some());
    }

    #[tokio::test]
    async fn a_seeded_list_is_unread_once_and_a_failed_read_keeps_it() {
        let host = FakeHost::new();
        host.set_listing(LISTING);
        let directory = Directory::new();
        assert!(!directory.take_unread());
        directory.seed(list_sessions(&host, "h").await.unwrap());
        assert!(directory.take_unread(), "the first capabilities() call");
        assert!(!directory.take_unread(), "and only that one");
        assert_eq!(directory.entries().unwrap().len(), 3);
        // A listing that fails changes nothing.
        host.set_exec(1, "", "boom");
        assert!(directory.refresh(&host, "h").await.is_err());
        assert_eq!(directory.entries().unwrap().len(), 3);
        assert!(directory.cached_socket(None).is_some());
        // A successful read replaces the list and clears the unread mark of an earlier seed.
        directory.seed(Vec::new());
        host.set_listing(
            r#"{"sessions":[{"name":"x","default":true,"running":true,"socket_path":"/s"}]}"#,
        );
        directory.refresh(&host, "h").await.unwrap();
        assert!(!directory.take_unread());
        assert_eq!(directory.cached_socket(None).as_deref(), Some("/s"));
    }
}
