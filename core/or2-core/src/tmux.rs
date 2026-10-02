//! tmux on a host: listing sessions and the command that attaches to one.
//!
//! Everything goes through a [`RemoteHost`] and the absolute tmux path from the capability
//! probe. tmux uses its default socket; tests isolate it with `TMUX_TMPDIR` in the
//! environment the commands run in, not with a production option.

use crate::host::{NavDirection, TargetNav, TmuxSession};
use crate::remote::{RemoteCommand, RemoteError, RemoteHost};

/// Fields joined by `:`, with the name last. tmux turns `:` and `.` in session names into
/// `_`, so a name never contains the delimiter, and the name being last means even a
/// surprising one could not shift the numeric fields. (A control character such as U+001F
/// cannot appear in a command line `RemoteCommand` renders.)
pub const LIST_FORMAT: &str =
    "#{session_windows}:#{session_attached}:#{session_created}:#{session_activity}:#{session_name}";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TmuxError {
    #[error(transparent)]
    Remote(#[from] RemoteError),
    /// tmux ran and failed; the message is its first line of stderr.
    #[error("tmux failed: {0}")]
    Failed(String),
}

/// `<tmux> -u list-sessions -F <LIST_FORMAT>`. `-u` forces UTF-8 so non-ASCII names come back
/// as written, whatever locale the non-interactive login shell has.
pub fn list_command(tmux: &str) -> RemoteCommand {
    RemoteCommand::new(tmux).args(["-u", "list-sessions", "-F", LIST_FORMAT])
}

/// `<tmux> -u new-session -A -s <name>`: attach to the session, creating it if it is missing.
/// `name` must already be valid ([`crate::host::is_valid_tmux_session_name`]).
pub fn attach_command(tmux: &str, name: &str) -> RemoteCommand {
    RemoteCommand::new(tmux).args(["-u", "new-session", "-A", "-s", name])
}

/// Sessions, most recently active first (ties by name). No server, or a server without
/// sessions, is an empty list.
pub async fn list_sessions<H: RemoteHost>(
    host: &H,
    tmux: &str,
) -> Result<Vec<TmuxSession>, TmuxError> {
    let output = host.exec(&list_command(tmux)).await?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.success() {
        return Ok(parse_list(&String::from_utf8_lossy(&output.stdout)));
    }
    if no_server(&stderr) {
        return Ok(Vec::new());
    }
    let first = stderr.lines().next().unwrap_or("").trim();
    Err(TmuxError::Failed(if first.is_empty() {
        format!("exit status {:?}", output.status)
    } else {
        first.chars().take(200).collect()
    }))
}

/// What tmux says when there is nothing to list: no server running, a stale or missing
/// socket, or a server that ended while it was asked.
fn no_server(stderr: &str) -> bool {
    stderr.contains("no server running")
        || stderr.contains("no sessions")
        || stderr.contains("server exited unexpectedly")
        || (stderr.contains("error connecting to") && stderr.contains("No such file or directory"))
}

/// Parses [`LIST_FORMAT`] output; lines that do not match are skipped.
pub fn parse_list(stdout: &str) -> Vec<TmuxSession> {
    let mut sessions: Vec<TmuxSession> = stdout
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(5, ':');
            Some(TmuxSession {
                windows: fields.next()?.parse().ok()?,
                attached_clients: fields.next()?.parse().ok()?,
                created_unix: fields.next()?.parse().ok()?,
                activity_unix: fields.next()?.parse().ok()?,
                name: fields.next().filter(|name| !name.is_empty())?.to_owned(),
            })
        })
        .collect();
    sessions.sort_by(|a, b| {
        b.activity_unix
            .cmp(&a.activity_unix)
            .then_with(|| a.name.cmp(&b.name))
    });
    sessions
}

// --- navigation ------------------------------------------------------------------------------

/// `list-clients` fields, joined by `:` with the client's name last (tmux session names never
/// contain `:`; a client's name is its tty for a terminal client, `client-<pid>` otherwise).
pub const CLIENT_FORMAT: &str = "#{client_activity}:#{client_session}:#{client_name}";

/// One attached tmux client, from [`CLIENT_FORMAT`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxClient {
    pub activity_unix: i64,
    /// The session it shows now.
    pub session: String,
    /// What `switch-client -c` takes: its tty (`/dev/pts/3`), or `client-<pid>` for a client
    /// without one.
    pub name: String,
}

/// `<tmux> -u list-clients -F <CLIENT_FORMAT>`.
pub fn list_clients_command(tmux: &str) -> RemoteCommand {
    RemoteCommand::new(tmux).args(["-u", "list-clients", "-F", CLIENT_FORMAT])
}

/// Parses [`CLIENT_FORMAT`] output; lines that do not match are skipped.
pub fn parse_clients(stdout: &str) -> Vec<TmuxClient> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, ':');
            Some(TmuxClient {
                activity_unix: fields.next()?.parse().ok()?,
                session: fields
                    .next()
                    .filter(|session| !session.is_empty())?
                    .to_owned(),
                name: fields.next().filter(|name| !name.is_empty())?.to_owned(),
            })
        })
        .collect()
}

/// `=<name>`: tmux takes a target session prefixed with `=` as an exact name only, never as a
/// prefix or pattern of another session's (`main` must not reach `main2`).
fn exact(session: &str) -> String {
    format!("={session}")
}

/// The command for a window or pane move in `session` (the one the terminal shows):
/// `next-window -t =<session>`, `previous-window -t =<session>`, or
/// `select-pane -L|-R|-U|-D -t =<session>:` (from the active pane of its current window).
/// `None` for a session move, which needs a client ([`switch_command`]).
pub fn nav_command(tmux: &str, session: &str, nav: TargetNav) -> Option<RemoteCommand> {
    let command = RemoteCommand::new(tmux).arg("-u");
    Some(match nav {
        TargetNav::NextWindow => command.args(["next-window", "-t", &exact(session)]),
        TargetNav::PreviousWindow => command.args(["previous-window", "-t", &exact(session)]),
        TargetNav::Pane { direction } => {
            let flag = match direction {
                NavDirection::Left => "-L",
                NavDirection::Right => "-R",
                NavDirection::Up => "-U",
                NavDirection::Down => "-D",
            };
            command.args(["select-pane", flag, "-t", &format!("{}:", exact(session))])
        }
        TargetNav::NextSession | TargetNav::PreviousSession => return None,
    })
}

/// `switch-client -c <client> -n` (next session) or `-p` (previous).
pub fn switch_command(tmux: &str, client: &str, next: bool) -> RemoteCommand {
    RemoteCommand::new(tmux).args([
        "-u",
        "switch-client",
        "-c",
        client,
        if next { "-n" } else { "-p" },
    ])
}

/// What tmux says when a move has nowhere to go (one window, one session): nothing to do.
fn nothing_to_do(stderr: &str) -> bool {
    stderr.contains("no next window")
        || stderr.contains("no previous window")
        || stderr.contains("can't find next session")
        || stderr.contains("can't find previous session")
}

fn failure(output: &crate::remote::ExecOutput) -> TmuxError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let first = stderr.lines().next().unwrap_or("").trim();
    TmuxError::Failed(if first.is_empty() {
        format!("exit status {:?}", output.status)
    } else {
        first.chars().take(200).collect()
    })
}

/// The tmux clients a session move has moved, per terminal target, for one host connection.
///
/// A terminal attaches to its target session (`new-session -A -s <target>`), so until it
/// switches, its client is one shown on that session and window and pane moves act on the
/// target session. After `switch-client` its client shows another session, which window and
/// pane moves must act on instead, and which no longer tells which client is the terminal's:
/// so the client a switch moved is remembered under the target's name, and found again by its
/// name in `list-clients`. A client that is no longer listed (the terminal reattached, with a
/// new client on the target session) is forgotten.
#[derive(Debug, Default)]
pub struct NavClients(std::sync::Mutex<std::collections::HashMap<String, String>>);

impl NavClients {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, String>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The client remembered for `target`, if any.
    pub fn get(&self, target: &str) -> Option<String> {
        self.lock().get(target).cloned()
    }

    fn remember(&self, target: &str, client: &str) {
        self.lock().insert(target.to_owned(), client.to_owned());
    }

    fn forget(&self, target: &str) {
        self.lock().remove(target);
    }
}

async fn list_clients<H: RemoteHost>(host: &H, tmux: &str) -> Result<Vec<TmuxClient>, TmuxError> {
    let output = host.exec(&list_clients_command(tmux)).await?;
    if output.success() {
        return Ok(parse_clients(&String::from_utf8_lossy(&output.stdout)));
    }
    if no_server(&String::from_utf8_lossy(&output.stderr)) {
        return Ok(Vec::new());
    }
    Err(failure(&output))
}

/// The client of the terminal on `target` among `clients`: the one a switch moved, while it is
/// listed, else the most recently active one showing `target`.
fn terminal_client<'a>(
    clients: &'a [TmuxClient],
    remembered: Option<&str>,
    target: &str,
) -> Option<&'a TmuxClient> {
    remembered
        .and_then(|name| clients.iter().find(|client| client.name == name))
        .or_else(|| {
            clients
                .iter()
                .filter(|client| client.session == target)
                .max_by_key(|client| client.activity_unix)
        })
}

/// Moves what a terminal attached to tmux session `target` shows (see [`TargetNav`]).
///
/// - A window or pane move acts on the session the terminal shows: `target`, or after a session
///   move the session its client was switched to ([`NavClients`]; one `list-clients` more).
/// - A session move switches the terminal's client ([`terminal_client`]) with
///   `switch-client -c <client> -n|-p` and remembers it. With no client found (nothing attached
///   to `target`) it fails: there is no terminal to switch.
///
/// A move with nowhere to go (one window, one session) is `Ok(())`.
pub async fn navigate<H: RemoteHost>(
    host: &H,
    tmux: &str,
    clients: &NavClients,
    target: &str,
    nav: TargetNav,
) -> Result<(), TmuxError> {
    let remembered = clients.get(target);
    let command = match nav {
        TargetNav::NextSession | TargetNav::PreviousSession => {
            let listed = list_clients(host, tmux).await?;
            let Some(client) = terminal_client(&listed, remembered.as_deref(), target) else {
                clients.forget(target);
                return Err(TmuxError::Failed(format!(
                    "no tmux client is attached to {target}"
                )));
            };
            clients.remember(target, &client.name);
            switch_command(tmux, &client.name, nav == TargetNav::NextSession)
        }
        _ => {
            let mut session = target.to_owned();
            if let Some(name) = remembered {
                let listed = list_clients(host, tmux).await?;
                match listed.into_iter().find(|client| client.name == name) {
                    Some(client) => session = client.session,
                    None => clients.forget(target),
                }
            }
            nav_command(tmux, &session, nav).expect("a window or pane move")
        }
    };
    let output = host.exec(&command).await?;
    if output.success() || nothing_to_do(&String::from_utf8_lossy(&output.stderr)) {
        Ok(())
    } else {
        Err(failure(&output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_render_as_single_quoted_argv() {
        assert_eq!(
            list_command("/usr/bin/tmux").render().unwrap(),
            format!("'/usr/bin/tmux' '-u' 'list-sessions' '-F' '{LIST_FORMAT}'")
        );
        assert_eq!(
            attach_command("/usr/bin/tmux", "my work").render().unwrap(),
            "'/usr/bin/tmux' '-u' 'new-session' '-A' '-s' 'my work'"
        );
        // A name is one argument whatever it contains.
        assert_eq!(
            attach_command("/t", "it's $(x)").render().unwrap(),
            r#"'/t' '-u' 'new-session' '-A' '-s' 'it'\''s $(x)'"#
        );
    }

    #[test]
    fn parses_sorts_by_activity_and_skips_malformed_lines() {
        let sessions = parse_list(
            "1:0:100:200:old\n\
             3:1:50:900:main\n\
             junk\n\
             x:0:1:2:bad-number\n\
             2:0:60:900:also-900\n\
             1:0:1:2:\n\
             4:2:70:500:caf\u{e9} \u{1f600}\n",
        );
        let names: Vec<_> = sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["also-900", "main", "caf\u{e9} \u{1f600}", "old"]);
        assert_eq!(
            sessions[1],
            TmuxSession {
                name: "main".into(),
                windows: 3,
                attached_clients: 1,
                created_unix: 50,
                activity_unix: 900,
            }
        );
    }

    #[test]
    fn a_name_with_the_delimiter_stays_in_the_name_field() {
        let sessions = parse_list("1:0:2:3:a:b:c\n");
        assert_eq!(sessions[0].name, "a:b:c");
    }

    #[test]
    fn no_server_messages_are_recognised() {
        for message in [
            "no server running on /tmp/tmux-1000/default",
            "error connecting to /tmp/tmux-1000/default (No such file or directory)",
            "no sessions",
            "server exited unexpectedly",
        ] {
            assert!(no_server(message), "{message}");
        }
        assert!(!no_server("error connecting to /x (Permission denied)"));
        assert!(!no_server("unknown option -- Z"));
    }

    #[test]
    fn navigation_commands_target_the_exact_session() {
        let render = |nav| nav_command("/t", "main", nav).unwrap().render().unwrap();
        assert_eq!(
            render(TargetNav::NextWindow),
            "'/t' '-u' 'next-window' '-t' '=main'"
        );
        assert_eq!(
            render(TargetNav::PreviousWindow),
            "'/t' '-u' 'previous-window' '-t' '=main'"
        );
        for (direction, flag) in [
            (NavDirection::Left, "-L"),
            (NavDirection::Right, "-R"),
            (NavDirection::Up, "-U"),
            (NavDirection::Down, "-D"),
        ] {
            assert_eq!(
                render(TargetNav::Pane { direction }),
                format!("'/t' '-u' 'select-pane' '{flag}' '-t' '=main:'")
            );
        }
        assert!(nav_command("/t", "main", TargetNav::NextSession).is_none());
        assert!(nav_command("/t", "main", TargetNav::PreviousSession).is_none());
        assert_eq!(
            switch_command("/t", "/dev/pts/3", true).render().unwrap(),
            "'/t' '-u' 'switch-client' '-c' '/dev/pts/3' '-n'"
        );
        assert_eq!(
            switch_command("/t", "client-42", false).render().unwrap(),
            "'/t' '-u' 'switch-client' '-c' 'client-42' '-p'"
        );
        assert_eq!(
            list_clients_command("/t").render().unwrap(),
            format!("'/t' '-u' 'list-clients' '-F' '{CLIENT_FORMAT}'")
        );
        // A name with spaces and quotes is still one argument.
        assert_eq!(
            nav_command("/t", "it's x", TargetNav::NextWindow)
                .unwrap()
                .render()
                .unwrap(),
            r#"'/t' '-u' 'next-window' '-t' '=it'\''s x'"#
        );
    }

    #[test]
    fn clients_parse_and_the_terminal_client_is_the_remembered_or_most_active_one() {
        let clients = parse_clients(
            "100:main:/dev/pts/1\n\
             300:main:/dev/pts/2\n\
             junk\n\
             x:main:/dev/pts/9\n\
             500:other:client-77\n\
             1::/dev/pts/5\n\
             2:main:\n",
        );
        assert_eq!(clients.len(), 3);
        assert_eq!(
            clients[2],
            TmuxClient {
                activity_unix: 500,
                session: "other".into(),
                name: "client-77".into()
            }
        );
        // Not remembered: the most recently active client on the target.
        assert_eq!(
            terminal_client(&clients, None, "main").unwrap().name,
            "/dev/pts/2"
        );
        // Remembered and still listed: that one, whatever it shows now.
        assert_eq!(
            terminal_client(&clients, Some("client-77"), "main")
                .unwrap()
                .name,
            "client-77"
        );
        // Remembered but gone: back to the target's clients.
        assert_eq!(
            terminal_client(&clients, Some("/dev/pts/8"), "main")
                .unwrap()
                .name,
            "/dev/pts/2"
        );
        assert!(terminal_client(&clients, None, "nobody").is_none());
    }

    #[test]
    fn a_move_with_nowhere_to_go_is_nothing_to_do() {
        for message in [
            "no next window",
            "no previous window",
            "can't find next session",
            "can't find previous session",
        ] {
            assert!(nothing_to_do(message), "{message}");
        }
        assert!(!nothing_to_do("can't find session: main"));
        assert!(!nothing_to_do("can't find client: /dev/pts/3"));
    }

    /// Answers each exec with the next scripted output and records the command lines.
    #[derive(Default)]
    struct Scripted {
        replies: std::sync::Mutex<std::collections::VecDeque<crate::remote::ExecOutput>>,
        log: std::sync::Mutex<Vec<String>>,
    }

    impl Scripted {
        fn reply(&self, status: u32, stdout: &str, stderr: &str) {
            self.replies
                .lock()
                .unwrap()
                .push_back(crate::remote::ExecOutput {
                    status: Some(status),
                    stdout: stdout.as_bytes().into(),
                    stderr: stderr.as_bytes().into(),
                });
        }

        fn take_log(&self) -> Vec<String> {
            std::mem::take(&mut *self.log.lock().unwrap())
        }
    }

    impl RemoteHost for Scripted {
        type Stream = tokio::io::DuplexStream;

        async fn exec_rendered(
            &self,
            line: &str,
        ) -> Result<crate::remote::ExecOutput, RemoteError> {
            self.log.lock().unwrap().push(line.to_owned());
            Ok(self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("a scripted reply"))
        }

        async fn open_unix(&self, _: &str) -> Result<Self::Stream, RemoteError> {
            Err(RemoteError::Closed)
        }
    }

    #[tokio::test]
    async fn a_session_move_switches_the_terminal_client_and_later_moves_follow_it() {
        let host = Scripted::default();
        let clients = NavClients::new();
        let list = format!("'/t' '-u' 'list-clients' '-F' '{CLIENT_FORMAT}'");

        // Before any session move a window move is one command on the target.
        host.reply(0, "", "");
        navigate(&host, "/t", &clients, "main", TargetNav::NextWindow)
            .await
            .unwrap();
        assert_eq!(host.take_log(), ["'/t' '-u' 'next-window' '-t' '=main'"]);

        // A session move finds the client on the target and switches it.
        host.reply(0, "10:main:/dev/pts/1\n20:other:/dev/pts/2\n", "");
        host.reply(0, "", "");
        navigate(&host, "/t", &clients, "main", TargetNav::NextSession)
            .await
            .unwrap();
        assert_eq!(
            host.take_log(),
            [
                list.clone(),
                "'/t' '-u' 'switch-client' '-c' '/dev/pts/1' '-n'".into()
            ]
        );
        assert_eq!(clients.get("main").as_deref(), Some("/dev/pts/1"));

        // Window and pane moves now act on the session that client shows.
        host.reply(0, "30:third:/dev/pts/1\n20:other:/dev/pts/2\n", "");
        host.reply(0, "", "");
        navigate(
            &host,
            "/t",
            &clients,
            "main",
            TargetNav::Pane {
                direction: NavDirection::Up,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            host.take_log(),
            [
                list.clone(),
                "'/t' '-u' 'select-pane' '-U' '-t' '=third:'".into()
            ]
        );

        // The next session move switches the same client back, although none shows `main`.
        host.reply(0, "30:third:/dev/pts/1\n40:other:/dev/pts/2\n", "");
        host.reply(0, "", "");
        navigate(&host, "/t", &clients, "main", TargetNav::PreviousSession)
            .await
            .unwrap();
        assert_eq!(
            host.take_log()[1],
            "'/t' '-u' 'switch-client' '-c' '/dev/pts/1' '-p'"
        );

        // A remembered client that has gone is forgotten: the move acts on the target again.
        host.reply(0, "50:main:/dev/pts/4\n", "");
        host.reply(0, "", "");
        navigate(&host, "/t", &clients, "main", TargetNav::PreviousWindow)
            .await
            .unwrap();
        assert_eq!(
            host.take_log()[1],
            "'/t' '-u' 'previous-window' '-t' '=main'"
        );
        assert_eq!(clients.get("main"), None);
    }

    #[tokio::test]
    async fn moves_with_nowhere_to_go_succeed_and_real_failures_are_reported() {
        let host = Scripted::default();
        let clients = NavClients::new();
        host.reply(1, "", "no next window\n");
        assert_eq!(
            navigate(&host, "/t", &clients, "main", TargetNav::NextWindow).await,
            Ok(())
        );
        host.reply(1, "", "can't find session: main\n");
        assert_eq!(
            navigate(&host, "/t", &clients, "main", TargetNav::NextWindow).await,
            Err(TmuxError::Failed("can't find session: main".into()))
        );
        // No server, or no client on the target: a session move has no terminal to switch.
        host.reply(1, "", "no server running on /tmp/tmux-1000/default\n");
        assert_eq!(
            navigate(&host, "/t", &clients, "main", TargetNav::NextSession).await,
            Err(TmuxError::Failed(
                "no tmux client is attached to main".into()
            ))
        );
        host.reply(0, "5:other:/dev/pts/2\n", "");
        assert!(matches!(
            navigate(&host, "/t", &clients, "main", TargetNav::NextSession).await,
            Err(TmuxError::Failed(_))
        ));
        assert_eq!(host.take_log().len(), 4, "no switch was attempted");
    }
}
