//! tmux on a host: listing sessions and the command that attaches to one.
//!
//! Everything goes through a [`RemoteHost`] and the absolute tmux path from the capability
//! probe. tmux uses its default socket; tests isolate it with `TMUX_TMPDIR` in the
//! environment the commands run in, not with a production option.

use crate::host::{TargetScroll, TmuxSession};
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

/// The tmux command that scrolls the active pane of session `name` (contracts.md, "Wheel-aware
/// scrolling"), as one exec of commands joined by tmux's `;`:
///
/// - `Up`: `copy-mode -e -t =<name>:` then `send-keys -t =<name>: -X -N <lines> scroll-up`
///   (`-e` leaves copy mode once a scroll down reaches the bottom; a pane already in copy mode
///   stays where it is);
/// - `Down`: `send-keys -X -N <lines> scroll-down`;
/// - `Bottom`: `send-keys -X cancel`.
///
/// `=<name>:` is the exact session, never a prefix or pattern match. `name` must already be
/// valid. `None` for zero lines.
pub fn scroll_command(tmux: &str, name: &str, scroll: TargetScroll) -> Option<RemoteCommand> {
    let target = format!("={name}:");
    let send = |args: &[&str]| -> Vec<String> {
        ["send-keys", "-t", target.as_str(), "-X"]
            .iter()
            .chain(args)
            .map(|arg| (*arg).to_owned())
            .collect()
    };
    let args: Vec<String> = match scroll {
        TargetScroll::Up { lines: 0 } | TargetScroll::Down { lines: 0 } => return None,
        TargetScroll::Up { lines } => ["copy-mode", "-e", "-t", target.as_str(), ";"]
            .iter()
            .map(|arg| (*arg).to_owned())
            .chain(send(&["-N", &lines.to_string(), "scroll-up"]))
            .collect(),
        TargetScroll::Down { lines } => send(&["-N", &lines.to_string(), "scroll-down"]),
        TargetScroll::Bottom => send(&["cancel"]),
    };
    Some(RemoteCommand::new(tmux).arg("-u").args(args))
}

/// Runs [`scroll_command`]. A pane that is not in copy mode (a `Down` or `Bottom` after tmux
/// already left it) is success: there is nothing to scroll back.
pub async fn scroll<H: RemoteHost>(
    host: &H,
    tmux: &str,
    name: &str,
    scroll: TargetScroll,
) -> Result<(), TmuxError> {
    let Some(command) = scroll_command(tmux, name, scroll) else {
        return Ok(());
    };
    let output = host.exec(&command).await?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.success() || stderr.contains("not in a mode") {
        return Ok(());
    }
    let first = stderr.lines().next().unwrap_or("").trim();
    Err(TmuxError::Failed(if first.is_empty() {
        format!("exit status {:?}", output.status)
    } else {
        first.chars().take(200).collect()
    }))
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
}
