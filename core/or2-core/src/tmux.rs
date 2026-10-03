//! tmux on a host: listing sessions, the command that attaches to one, and the navigation moves.
//!
//! Everything goes through a [`RemoteHost`] and the absolute tmux path from the capability
//! probe. tmux uses its default socket; tests isolate it with `TMUX_TMPDIR` in the
//! environment the commands run in, not with a production option.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::host::{NavDirection, TargetNav, TargetScroll, TmuxSession};
use crate::remote::{ExecOutput, RemoteCommand, RemoteError, RemoteHost};

/// Fields joined by `:`, with the name last. tmux turns `:` and `.` in session names into
/// `_`, so a name never contains the delimiter, and the name being last means even a
/// surprising one could not shift the numeric fields. (A control character such as U+001F
/// cannot appear in a command line `RemoteCommand` renders.)
pub(crate) const LIST_FORMAT: &str =
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
pub(crate) fn list_command(tmux: &str) -> RemoteCommand {
    RemoteCommand::new(tmux).args(["-u", "list-sessions", "-F", LIST_FORMAT])
}

/// `<tmux> -u new-session -A -s <name>`: attach to the session, creating it if it is missing.
/// `name` must already be valid ([`crate::host::is_valid_tmux_session_name`]).
///
/// With a terminal's `client` id ([`new_client_id`]) the same tmux command list then records
/// the attaching client's name under that id, `; set-option -s -F @or2-client-<id>
/// '#{client_name}'` (run by the client that just attached, so `#{client_name}` is its own),
/// which is how navigation later finds exactly this terminal's client ([`navigate`]). Pass
/// `client` only for a tmux that [`records_clients`]: an older one would reject the whole
/// command list, attach included.
pub(crate) fn attach_command(tmux: &str, name: &str, client: Option<&str>) -> RemoteCommand {
    let command = RemoteCommand::new(tmux).args(["-u", "new-session", "-A", "-s", name]);
    match client {
        Some(id) => command.args([
            ";",
            "set-option",
            "-s",
            "-F",
            &client_option(id),
            "#{client_name}",
        ]),
        None => command,
    }
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
pub(crate) fn scroll_command(
    tmux: &str,
    name: &str,
    scroll: TargetScroll,
) -> Option<RemoteCommand> {
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

/// Runs [`scroll_command`] on the session a terminal attached to `target` shows now
/// ([`shown_session`]: `target`, or the one a session move switched its client to). A pane that
/// is not in copy mode (a `Down` or `Bottom` after tmux already left it) is success: there is
/// nothing to scroll back.
pub async fn scroll<H: RemoteHost>(
    host: &H,
    tmux: &str,
    clients: &NavClients,
    target: &str,
    client: Option<&str>,
    scroll: TargetScroll,
) -> Result<(), TmuxError> {
    if matches!(
        scroll,
        TargetScroll::Up { lines: 0 } | TargetScroll::Down { lines: 0 }
    ) {
        return Ok(());
    }
    let session = shown_session(host, tmux, clients, target, client).await?;
    let Some(command) = scroll_command(tmux, &session, scroll) else {
        return Ok(());
    };
    let output = host.exec(&command).await?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.success() || stderr.contains("not in a mode") {
        return Ok(());
    }
    Err(failure(&output))
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
    Err(failure(&output))
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
pub(crate) fn parse_list(stdout: &str) -> Vec<TmuxSession> {
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

/// A new tmux terminal's client id: 32 random lowercase hex digits. Its attach records its
/// tmux client under it ([`attach_command`]), so it must be unique among every terminal on the
/// tmux server: other connections, app processes and devices included.
pub(crate) fn new_client_id() -> String {
    format!("{:032x}", rand::random::<u128>())
}

/// Whether `id` can be a client id ([`new_client_id`]): exactly 32 lowercase hex digits. It
/// becomes part of a tmux option's name, so nothing else is accepted.
pub(crate) fn is_valid_client_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The first tmux release whose `set-option` takes `-F` (formats expanded in the value: tmux
/// CHANGES, "2.5 to 2.6"), which recording a terminal's client needs ([`attach_command`]).
/// `#{client_name}` (2.4), user options (1.8) and server options (1.2) are older.
pub(crate) const RECORDS_CLIENTS_SINCE: (u32, u32) = (2, 6);

/// Whether the tmux whose `tmux -V` printed `version` can record a terminal's client at its
/// attach (`set-option -F`, [`RECORDS_CLIENTS_SINCE`]). Read strictly, since appending the
/// step to the attach of a tmux that cannot parse it would break the attach itself (an older
/// tmux client rejects the whole command list): `tmux X.Y` with any suffix (`3.3a`,
/// `3.0-rc5`), `tmux next-X.Y` (a development build after X.Y), `tmux master`, and OpenBSD's
/// base tmux (`tmux openbsd-X.Y`, from OpenBSD 6.3, which ships a tmux newer than 2.6).
/// Anything else, including no answer, is `false`: the attach stays plain.
pub(crate) fn records_clients(version: &str) -> bool {
    fn major_minor(text: &str) -> Option<(u32, u32)> {
        let (major, rest) = text.split_once('.')?;
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        Some((major.parse().ok()?, rest[..digits].parse().ok()?))
    }
    let Some(release) = version.trim().strip_prefix("tmux ") else {
        return false;
    };
    if release == "master" {
        return true;
    }
    if let Some(openbsd) = release.strip_prefix("openbsd-") {
        return major_minor(openbsd).is_some_and(|release| release >= (6, 3));
    }
    let release = release.strip_prefix("next-").unwrap_or(release);
    major_minor(release).is_some_and(|release| release >= RECORDS_CLIENTS_SINCE)
}

/// The tmux server option a terminal's attach records its client's name in.
fn client_option(id: &str) -> String {
    format!("@or2-client-{id}")
}

/// `list-clients` fields, joined by `:` with the client's name last (tmux session names never
/// contain `:`; a client's name is its tty for a terminal client, `client-<pid>` otherwise).
/// The second field is the client name the asking terminal's attach recorded under its client
/// id ([`attach_command`]), the same on every line; empty without an id, or when nothing was
/// recorded. The client whose name it is, is that terminal's; with nothing recorded no client
/// is.
pub(crate) fn client_format(client: Option<&str>) -> String {
    let recorded = client
        .map(|id| format!("#{{{}}}", client_option(id)))
        .unwrap_or_default();
    format!("#{{client_activity}}:{recorded}:#{{client_session}}:#{{client_name}}")
}

/// One attached tmux client, from [`client_format`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TmuxClient {
    pub activity_unix: i64,
    /// The session it shows now.
    pub session: String,
    /// What `switch-client -c` takes: its tty (`/dev/pts/3`), or `client-<pid>` for a client
    /// without one.
    pub name: String,
    /// It is the client the asking terminal's attach recorded: that terminal's own.
    pub recorded: bool,
}

/// `<tmux> -u list-clients -F <client_format(client)>`.
pub(crate) fn list_clients_command(tmux: &str, client: Option<&str>) -> RemoteCommand {
    RemoteCommand::new(tmux).args(["-u", "list-clients", "-F", &client_format(client)])
}

/// Parses [`client_format`] output; lines that do not match are skipped.
pub(crate) fn parse_clients(stdout: &str) -> Vec<TmuxClient> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(4, ':');
            let activity_unix = fields.next()?.parse().ok()?;
            let recorded = fields.next()?;
            let session = fields.next().filter(|session| !session.is_empty())?;
            let name = fields.next().filter(|name| !name.is_empty())?;
            Some(TmuxClient {
                activity_unix,
                session: session.to_owned(),
                name: name.to_owned(),
                recorded: recorded == name,
            })
        })
        .collect()
}

/// `<tmux> -u set-option -s -q -u @or2-client-<id>`: forgets what a terminal's attach recorded,
/// once the terminal has closed (an option that is not set is no error).
pub(crate) fn release_command(tmux: &str, client: &str) -> RemoteCommand {
    RemoteCommand::new(tmux).args(["-u", "set-option", "-s", "-q", "-u", &client_option(client)])
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
pub(crate) fn nav_command(tmux: &str, session: &str, nav: TargetNav) -> Option<RemoteCommand> {
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
pub(crate) fn switch_command(tmux: &str, client: &str, next: bool) -> RemoteCommand {
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

/// tmux ran and failed: what it said.
fn failure(output: &ExecOutput) -> TmuxError {
    TmuxError::Failed(output.stderr_line())
}

/// The tmux clients that session moves have switched, per terminal (by client id), for one
/// host connection.
///
/// A terminal attaches to its target session (`new-session -A -s <target>`), so until it
/// switches, its client shows that session and window and pane moves act on the target
/// session. After `switch-client` its client shows another session, which window and pane
/// moves must act on instead: so a terminal whose client a switch moved is remembered under
/// its client id (two terminals on one target are two entries), and its client is found again
/// in `list-clients` by what its attach recorded. A record that is no longer listed (the
/// terminal reattached, the server restarted) is forgotten, and a closed terminal's entry is
/// released.
#[derive(Debug, Default)]
pub struct NavClients(Mutex<HashMap<String, String>>);

impl NavClients {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, String>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The client name remembered for the terminal with client id `client`, if any.
    pub fn get(&self, client: &str) -> Option<String> {
        self.lock().get(client).cloned()
    }

    /// Forgets the client of the terminal with client id `client` (it has closed, or its
    /// record is gone).
    pub fn release(&self, client: &str) {
        self.lock().remove(client);
    }

    fn remember(&self, client: &str, name: &str) {
        self.lock().insert(client.to_owned(), name.to_owned());
    }
}

async fn list_clients<H: RemoteHost>(
    host: &H,
    tmux: &str,
    client: &str,
) -> Result<Vec<TmuxClient>, TmuxError> {
    let output = host.exec(&list_clients_command(tmux, Some(client))).await?;
    if output.success() {
        return Ok(parse_clients(&String::from_utf8_lossy(&output.stdout)));
    }
    if no_server(&String::from_utf8_lossy(&output.stderr)) {
        return Ok(Vec::new());
    }
    Err(failure(&output))
}

/// What a [`navigate`] call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NavOutcome {
    /// The move ran (or tmux said it had nowhere to go: one window, one session).
    Ran,
    /// A session move whose terminal's client cannot be identified (no client id, a tmux that
    /// cannot record it, or nothing recorded under it): nothing was switched. The caller
    /// treats it as nothing to do, never as an error.
    ClientUnknown,
}

/// Moves what a terminal attached to tmux session `target` shows (see [`TargetNav`]).
/// `client` is the terminal's client id ([`new_client_id`]) when its attach recorded its
/// client ([`attach_command`]); `None` when it could not (a tmux older than 2.6, see
/// [`records_clients`]).
///
/// - A window or pane move acts on the session the terminal shows: `target`, or after a session
///   move the session its recorded client was switched to ([`NavClients`]; one `list-clients`
///   more).
/// - A session move switches exactly the terminal's own client, the one its attach recorded,
///   with `switch-client -c <client> -n|-p`, and remembers it. When that client cannot be
///   identified it is [`NavOutcome::ClientUnknown`] and nothing is switched: it never guesses
///   among the clients on `target` (the most recently active one need not be the one under
///   the user's fingers: a gesture is not tmux input and leaves no activity).
///
/// A move with nowhere to go (one window, one session) is [`NavOutcome::Ran`].
pub(crate) async fn navigate<H: RemoteHost>(
    host: &H,
    tmux: &str,
    clients: &NavClients,
    target: &str,
    client: Option<&str>,
    nav: TargetNav,
) -> Result<NavOutcome, TmuxError> {
    let command = match nav {
        TargetNav::NextSession | TargetNav::PreviousSession => {
            let Some(id) = client else {
                return Ok(NavOutcome::ClientUnknown);
            };
            let listed = list_clients(host, tmux, id).await?;
            let Some(found) = listed.iter().find(|listed| listed.recorded) else {
                clients.release(id);
                return Ok(NavOutcome::ClientUnknown);
            };
            clients.remember(id, &found.name);
            switch_command(tmux, &found.name, nav == TargetNav::NextSession)
        }
        _ => {
            let session = shown_session(host, tmux, clients, target, client).await?;
            nav_command(tmux, &session, nav).expect("a window or pane move")
        }
    };
    let output = host.exec(&command).await?;
    if output.success() || nothing_to_do(&String::from_utf8_lossy(&output.stderr)) {
        Ok(NavOutcome::Ran)
    } else {
        Err(failure(&output))
    }
}

/// The session a terminal attached to tmux session `target` shows now, for the moves and scrolls
/// that act on a session rather than a client: `target`, or after a session move of its client
/// (remembered in [`NavClients`] under its client id `client`) the session that client shows,
/// found by what its attach recorded (one `list-clients` more). A record that is no longer
/// listed is forgotten and `target` is used again.
async fn shown_session<H: RemoteHost>(
    host: &H,
    tmux: &str,
    clients: &NavClients,
    target: &str,
    client: Option<&str>,
) -> Result<String, TmuxError> {
    let Some((id, name)) = client.and_then(|id| Some((id, clients.get(id)?))) else {
        return Ok(target.to_owned());
    };
    let listed = list_clients(host, tmux, id).await?;
    Ok(match listed.iter().find(|listed| listed.recorded) {
        Some(found) => {
            if found.name != name {
                clients.remember(id, &found.name);
            }
            found.session.clone()
        }
        None => {
            clients.release(id);
            target.to_owned()
        }
    })
}

/// Forgets a closed terminal's client: its [`NavClients`] entry, and (best effort, one exec)
/// what its attach recorded on the tmux server ([`release_command`]).
pub(crate) async fn release_client<H: RemoteHost>(
    host: &H,
    tmux: &str,
    clients: &NavClients,
    client: &str,
) -> Result<(), TmuxError> {
    clients.release(client);
    let output = host.exec(&release_command(tmux, client)).await?;
    if output.success() || no_server(&String::from_utf8_lossy(&output.stderr)) {
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
            attach_command("/usr/bin/tmux", "my work", None)
                .render()
                .unwrap(),
            "'/usr/bin/tmux' '-u' 'new-session' '-A' '-s' 'my work'"
        );
        // A name is one argument whatever it contains.
        assert_eq!(
            attach_command("/t", "it's $(x)", None).render().unwrap(),
            r#"'/t' '-u' 'new-session' '-A' '-s' 'it'\''s $(x)'"#
        );
        // A terminal's client id: the attaching client records its own name under it, in the
        // same tmux command list.
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            attach_command("/t", "work", Some(id)).render().unwrap(),
            format!(
                "'/t' '-u' 'new-session' '-A' '-s' 'work' ';' 'set-option' '-s' '-F' \
                 '@or2-client-{id}' '#{{client_name}}'"
            )
        );
        assert_eq!(
            attach_command("/t", "work", Some(id)).argv().unwrap()[6..],
            [
                ";",
                "set-option",
                "-s",
                "-F",
                &format!("@or2-client-{id}"),
                "#{client_name}"
            ]
        );
        assert_eq!(
            release_command("/t", id).render().unwrap(),
            format!("'/t' '-u' 'set-option' '-s' '-q' '-u' '@or2-client-{id}'")
        );
    }

    #[test]
    fn client_ids_are_random_hex_and_validated() {
        let (a, b) = (new_client_id(), new_client_id());
        assert_ne!(a, b);
        assert!(is_valid_client_id(&a), "{a}");
        assert!(is_valid_client_id(&b), "{b}");
        for bad in [
            "",
            "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdef0",
            "0123456789ABCDEF0123456789abcdef",
            "0123456789abcdef0123456789abcde-",
            "0123456789abcdef 123456789abcdef",
        ] {
            assert!(!is_valid_client_id(bad), "{bad:?}");
        }
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
            list_clients_command("/t", None).render().unwrap(),
            "'/t' '-u' 'list-clients' '-F' '#{client_activity}::#{client_session}:#{client_name}'"
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
    fn clients_parse_and_only_the_recorded_one_is_the_terminals() {
        assert_eq!(
            client_format(None),
            "#{client_activity}::#{client_session}:#{client_name}"
        );
        assert_eq!(
            client_format(Some("0123456789abcdef0123456789abcdef")),
            "#{client_activity}:#{@or2-client-0123456789abcdef0123456789abcdef}:\
             #{client_session}:#{client_name}"
        );
        let clients = parse_clients(
            "100::main:/dev/pts/1\n\
             300::main:/dev/pts/2\n\
             junk\n\
             x::main:/dev/pts/9\n\
             500::other:client-77\n\
             1:::/dev/pts/5\n\
             2::main:\n\
             3:main:/dev/pts/6\n",
        );
        assert_eq!(clients.len(), 3);
        assert_eq!(
            clients[2],
            TmuxClient {
                activity_unix: 500,
                session: "other".into(),
                name: "client-77".into(),
                recorded: false,
            }
        );
        // Nothing recorded: no client is the terminal's, however active.
        assert!(clients.iter().all(|client| !client.recorded));
        // What the attach recorded names exactly one client, whatever it shows now.
        let clients = parse_clients(
            "100:/dev/pts/1:other:/dev/pts/1\n\
             300:/dev/pts/1:main:/dev/pts/2\n",
        );
        assert!(clients[0].recorded && !clients[1].recorded);
    }

    #[test]
    fn only_a_tmux_that_takes_set_option_f_records_clients() {
        for version in [
            "tmux 2.6",
            "tmux 2.9a",
            "tmux 3.0-rc5",
            "tmux 3.3a",
            "tmux 3.7c\n",
            "tmux 10.0",
            "tmux next-3.6",
            "tmux master",
            "tmux openbsd-7.5",
        ] {
            assert!(records_clients(version), "{version:?}");
        }
        for version in [
            "",
            "tmux",
            "tmux 1.8",
            "tmux 2.5",
            "tmux 2.",
            "tmux next-2.5",
            "tmux openbsd-6.2",
            "tmux openbsd-",
            "tmux unknown",
            "usage: tmux [-2CluvV] [-c shell-command]",
            "3.4",
        ] {
            assert!(!records_clients(version), "{version:?}");
        }
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
        replies: Mutex<std::collections::VecDeque<ExecOutput>>,
        log: Mutex<Vec<String>>,
    }

    impl Scripted {
        fn reply(&self, status: u32, stdout: &str, stderr: &str) {
            self.replies.lock().unwrap().push_back(ExecOutput {
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

        async fn exec_rendered(&self, line: &str) -> Result<ExecOutput, RemoteError> {
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

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[tokio::test]
    async fn a_session_move_switches_the_recorded_client_and_later_moves_follow_it() {
        let host = Scripted::default();
        let clients = NavClients::new();
        let list = format!("'/t' '-u' 'list-clients' '-F' '{}'", client_format(Some(A)));

        // Before any session move a window move is one command on the target.
        host.reply(0, "", "");
        let moved = navigate(
            &host,
            "/t",
            &clients,
            "main",
            Some(A),
            TargetNav::NextWindow,
        )
        .await;
        assert_eq!(moved, Ok(NavOutcome::Ran));
        assert_eq!(host.take_log(), ["'/t' '-u' 'next-window' '-t' '=main'"]);

        // A session move switches the client A's attach recorded, although another client on
        // the target is more recently active.
        host.reply(
            0,
            "10:/dev/pts/1:main:/dev/pts/1\n20:/dev/pts/1:main:/dev/pts/2\n",
            "",
        );
        host.reply(0, "", "");
        let moved = navigate(
            &host,
            "/t",
            &clients,
            "main",
            Some(A),
            TargetNav::NextSession,
        )
        .await;
        assert_eq!(moved, Ok(NavOutcome::Ran));
        assert_eq!(
            host.take_log(),
            [
                list.clone(),
                "'/t' '-u' 'switch-client' '-c' '/dev/pts/1' '-n'".into()
            ]
        );
        assert_eq!(clients.get(A).as_deref(), Some("/dev/pts/1"));

        // Window and pane moves now act on the session that client shows.
        host.reply(
            0,
            "30:/dev/pts/1:third:/dev/pts/1\n20:/dev/pts/1:other:/dev/pts/2\n",
            "",
        );
        host.reply(0, "", "");
        navigate(
            &host,
            "/t",
            &clients,
            "main",
            Some(A),
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
        host.reply(
            0,
            "30:/dev/pts/1:third:/dev/pts/1\n40:/dev/pts/1:other:/dev/pts/2\n",
            "",
        );
        host.reply(0, "", "");
        navigate(
            &host,
            "/t",
            &clients,
            "main",
            Some(A),
            TargetNav::PreviousSession,
        )
        .await
        .unwrap();
        assert_eq!(
            host.take_log()[1],
            "'/t' '-u' 'switch-client' '-c' '/dev/pts/1' '-p'"
        );

        // A record that is no longer listed is forgotten: the move acts on the target again.
        host.reply(0, "50::main:/dev/pts/4\n", "");
        host.reply(0, "", "");
        navigate(
            &host,
            "/t",
            &clients,
            "main",
            Some(A),
            TargetNav::PreviousWindow,
        )
        .await
        .unwrap();
        assert_eq!(
            host.take_log()[1],
            "'/t' '-u' 'previous-window' '-t' '=main'"
        );
        assert_eq!(clients.get(A), None);
    }

    /// A swipe scroll after a session move scrolls the session the terminal's client shows,
    /// not the one it was opened on (the bug: it scrolled the target, out of sight).
    #[tokio::test]
    async fn a_scroll_after_a_session_move_scrolls_the_session_shown() {
        let host = Scripted::default();
        let clients = NavClients::new();
        let list = format!("'/t' '-u' 'list-clients' '-F' '{}'", client_format(Some(A)));
        let up = TargetScroll::Up { lines: 2 };

        // Before any session move: one command on the target, no listing.
        host.reply(0, "", "");
        scroll(&host, "/t", &clients, "main", Some(A), up)
            .await
            .unwrap();
        assert_eq!(
            host.take_log(),
            [
                "'/t' '-u' 'copy-mode' '-e' '-t' '=main:' ';' 'send-keys' '-t' '=main:' '-X' \
                 '-N' '2' 'scroll-up'"
            ]
        );

        // The terminal's client is switched to `other`.
        host.reply(0, "10:/dev/pts/1:main:/dev/pts/1\n", "");
        host.reply(0, "", "");
        navigate(
            &host,
            "/t",
            &clients,
            "main",
            Some(A),
            TargetNav::NextSession,
        )
        .await
        .unwrap();
        host.take_log();

        // Now the scroll acts on `other`, found through the client its attach recorded.
        host.reply(0, "20:/dev/pts/1:other:/dev/pts/1\n", "");
        host.reply(0, "", "");
        scroll(&host, "/t", &clients, "main", Some(A), TargetScroll::Bottom)
            .await
            .unwrap();
        assert_eq!(
            host.take_log(),
            [
                list,
                "'/t' '-u' 'send-keys' '-t' '=other:' '-X' 'cancel'".into()
            ]
        );

        // Without the terminal's id the target is all there is; zero lines run nothing.
        host.reply(0, "", "");
        scroll(&host, "/t", &clients, "main", None, up)
            .await
            .unwrap();
        assert!(host.take_log()[0].contains("'=main:'"));
        scroll(
            &host,
            "/t",
            &clients,
            "main",
            Some(A),
            TargetScroll::Up { lines: 0 },
        )
        .await
        .unwrap();
        assert!(host.take_log().is_empty());
    }

    #[tokio::test]
    async fn moves_with_nowhere_to_go_succeed_and_real_failures_are_reported() {
        let host = Scripted::default();
        let clients = NavClients::new();
        host.reply(1, "", "no next window\n");
        assert_eq!(
            navigate(
                &host,
                "/t",
                &clients,
                "main",
                Some(A),
                TargetNav::NextWindow
            )
            .await,
            Ok(NavOutcome::Ran)
        );
        host.reply(1, "", "can't find session: main\n");
        assert_eq!(
            navigate(
                &host,
                "/t",
                &clients,
                "main",
                Some(A),
                TargetNav::NextWindow
            )
            .await,
            Err(TmuxError::Failed("can't find session: main".into()))
        );
        host.reply(0, "1:/dev/pts/1:main:/dev/pts/1\n", "");
        host.reply(1, "", "can't find next session\n");
        assert_eq!(
            navigate(
                &host,
                "/t",
                &clients,
                "main",
                Some(A),
                TargetNav::NextSession
            )
            .await,
            Ok(NavOutcome::Ran)
        );
        // No server: nothing recorded, so no client to switch, and that is no error.
        host.reply(1, "", "no server running on /tmp/tmux-1000/default\n");
        assert_eq!(
            navigate(
                &host,
                "/t",
                &clients,
                "main",
                Some(A),
                TargetNav::NextSession
            )
            .await,
            Ok(NavOutcome::ClientUnknown)
        );
        host.reply(1, "", "server says no\n");
        assert_eq!(
            navigate(
                &host,
                "/t",
                &clients,
                "main",
                Some(A),
                TargetNav::NextSession
            )
            .await,
            Err(TmuxError::Failed("server says no".into()))
        );
        assert_eq!(host.take_log().len(), 6);
    }

    /// No record of the terminal's client (a tmux older than 2.6 attached without the step, so
    /// the caller passes no id; or the record is gone): a session move switches nothing and
    /// never guesses, while window and pane moves still act on the target.
    #[tokio::test]
    async fn without_a_record_a_session_move_switches_nothing() {
        let host = Scripted::default();
        let clients = NavClients::new();
        for nav in [TargetNav::NextSession, TargetNav::PreviousSession] {
            // No id: nothing runs at all.
            assert_eq!(
                navigate(&host, "/t", &clients, "work", None, nav).await,
                Ok(NavOutcome::ClientUnknown)
            );
            assert!(host.take_log().is_empty());
            // An id with nothing recorded under it: the clients on the target are listed, the
            // most recently active is not taken.
            host.reply(0, "10::work:/dev/pts/1\n20::work:/dev/pts/2\n", "");
            assert_eq!(
                navigate(&host, "/t", &clients, "work", Some(A), nav).await,
                Ok(NavOutcome::ClientUnknown)
            );
            assert_eq!(host.take_log().len(), 1, "no switch-client");
            assert_eq!(clients.get(A), None);
        }
        for (client, nav, command) in [
            (None, TargetNav::NextWindow, "'next-window' '-t' '=work'"),
            (
                Some(A),
                TargetNav::PreviousWindow,
                "'previous-window' '-t' '=work'",
            ),
            (
                None,
                TargetNav::Pane {
                    direction: NavDirection::Left,
                },
                "'select-pane' '-L' '-t' '=work:'",
            ),
        ] {
            host.reply(0, "", "");
            assert_eq!(
                navigate(&host, "/t", &clients, "work", client, nav).await,
                Ok(NavOutcome::Ran)
            );
            assert_eq!(host.take_log(), [format!("'/t' '-u' {command}")]);
        }
    }

    /// Two terminals on one target session: each move acts on the client the asking terminal's
    /// attach recorded, never on the other one's, however active it is and wherever either has
    /// been switched to.
    #[tokio::test]
    async fn two_terminals_on_one_target_each_move_their_own_client() {
        let host = Scripted::default();
        let clients = NavClients::new();
        let (a, b) = (
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );
        let list = |id: &str| {
            format!(
                "'/t' '-u' 'list-clients' '-F' '{}'",
                client_format(Some(id))
            )
        };
        // What tmux answers each terminal: A's client is /dev/pts/1, B's /dev/pts/2 (the more
        // recently active, and so the one a guess by the target would pick for both).
        let listing = |id: &str, a_shows: &str, b_shows: &str| {
            let recorded = if id == a { "/dev/pts/1" } else { "/dev/pts/2" };
            format!("10:{recorded}:{a_shows}:/dev/pts/1\n20:{recorded}:{b_shows}:/dev/pts/2\n")
        };

        // A's session move switches A's client although B's is more recently active.
        host.reply(0, &listing(a, "work", "work"), "");
        host.reply(0, "", "");
        navigate(
            &host,
            "/t",
            &clients,
            "work",
            Some(a),
            TargetNav::NextSession,
        )
        .await
        .unwrap();
        assert_eq!(
            host.take_log(),
            [
                list(a),
                "'/t' '-u' 'switch-client' '-c' '/dev/pts/1' '-n'".into()
            ]
        );

        // B's session move switches B's client, not the one A remembered.
        host.reply(0, &listing(b, "other", "work"), "");
        host.reply(0, "", "");
        navigate(
            &host,
            "/t",
            &clients,
            "work",
            Some(b),
            TargetNav::PreviousSession,
        )
        .await
        .unwrap();
        assert_eq!(
            host.take_log(),
            [
                list(b),
                "'/t' '-u' 'switch-client' '-c' '/dev/pts/2' '-p'".into()
            ]
        );
        assert_eq!(clients.get(a).as_deref(), Some("/dev/pts/1"));
        assert_eq!(clients.get(b).as_deref(), Some("/dev/pts/2"));

        // Window moves act on the session each terminal's own client shows.
        host.reply(0, &listing(a, "other", "third"), "");
        host.reply(0, "", "");
        navigate(
            &host,
            "/t",
            &clients,
            "work",
            Some(a),
            TargetNav::NextWindow,
        )
        .await
        .unwrap();
        assert_eq!(host.take_log()[1], "'/t' '-u' 'next-window' '-t' '=other'");
        host.reply(0, &listing(b, "other", "third"), "");
        host.reply(0, "", "");
        navigate(
            &host,
            "/t",
            &clients,
            "work",
            Some(b),
            TargetNav::NextWindow,
        )
        .await
        .unwrap();
        assert_eq!(host.take_log()[1], "'/t' '-u' 'next-window' '-t' '=third'");

        // A terminal that never switched acts on its target in one exec.
        let c = "cccccccccccccccccccccccccccccccc";
        host.reply(0, "", "");
        navigate(
            &host,
            "/t",
            &clients,
            "work",
            Some(c),
            TargetNav::NextWindow,
        )
        .await
        .unwrap();
        assert_eq!(host.take_log(), ["'/t' '-u' 'next-window' '-t' '=work'"]);

        // A remembered tty that is no longer a client is re-resolved by what the attach
        // recorded (A reattached: its new client is /dev/pts/7, on the target again).
        host.reply(
            0,
            "30:/dev/pts/7:work:/dev/pts/7\n20:/dev/pts/7:third:/dev/pts/2\n",
            "",
        );
        host.reply(0, "", "");
        navigate(
            &host,
            "/t",
            &clients,
            "work",
            Some(a),
            TargetNav::PreviousWindow,
        )
        .await
        .unwrap();
        assert_eq!(
            host.take_log()[1],
            "'/t' '-u' 'previous-window' '-t' '=work'"
        );
        assert_eq!(clients.get(a).as_deref(), Some("/dev/pts/7"));

        // A closed terminal is released: its entry and what its attach recorded.
        host.reply(0, "", "");
        release_client(&host, "/t", &clients, a).await.unwrap();
        assert_eq!(
            host.take_log(),
            [format!(
                "'/t' '-u' 'set-option' '-s' '-q' '-u' '@or2-client-{a}'"
            )]
        );
        assert_eq!(clients.get(a), None);
        assert_eq!(clients.get(b).as_deref(), Some("/dev/pts/2"));
    }
}
