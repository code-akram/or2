//! The capability probe: what a host offers, found with two concurrent execs.
//!
//! Non-interactive SSH does not load the user's `PATH`, so [`PROBE_SCRIPT`] looks for `tmux`,
//! `herdr` and `mosh-server` with `command -v` and then in the usual user-local and package
//! manager directories, and picks a UTF-8 locale. It is one fixed `sh` script run with
//! [`RemoteHost::exec_script`], so it works over SSH and over
//! [`LocalHost`](crate::remote::LocalHost) alike. [`HERDR_SCRIPT`] runs beside it, in its own
//! channel and its own bound ([`HERDR_LIST_TIMEOUT`]): it finds herdr the same way and lists its
//! sessions, so the listing costs no round trips after the first script and a wedged herdr
//! cannot take down the discovery of everything else (the first script still reports herdr's
//! path). The host driver caches the answer for the connection's lifetime (Rust has no
//! storage), seeds the connection's [`Directory`] with the listing, and re-reads only the
//! session list when `capabilities()` is queried ([`SessionsCache`]).
//!
//! The two halves are published apart: the program probe ([`probe_programs`]) is cached on its
//! own the moment [`PROBE_SCRIPT`] returns, and everything that needs only a program's path (a
//! tmux or herdr terminal, a mosh start, `mosh_server()`) waits for it alone, never for herdr's
//! listing ([`probe_within`]).

use std::sync::Arc;
use std::time::Duration;

use crate::herdr::{self, Directory, DiscoveryError, SessionEntry};
use crate::host::{HerdrSessionInfo, HostCapabilities};
use crate::remote::{RemoteError, RemoteHost};
use crate::tmux;

/// How long herdr's session listing may take before it counts as failed.
pub const HERDR_LIST_TIMEOUT: Duration = Duration::from_secs(5);

/// Fixed, with no `'` and no `\` (the rendering contract of `render_script`; a test enforces
/// it). Prints `or2:<name>:<value>` lines. Unknown output is ignored by [`parse`].
pub const PROBE_SCRIPT: &str = r#"
or2_find() {
  or2_path=$(command -v "$1" 2>/dev/null)
  case "$or2_path" in
    /*) ;;
    *)
      or2_path=
      for or2_dir in "$HOME/.local/bin" "$HOME/.cargo/bin" /opt/homebrew/bin /usr/local/bin /usr/bin /bin "$HOME/.nix-profile/bin" /run/current-system/sw/bin; do
        if [ -f "$or2_dir/$1" ] && [ -x "$or2_dir/$1" ]; then or2_path="$or2_dir/$1"; break; fi
      done
      ;;
  esac
}
or2_find tmux
echo "or2:tmux:$or2_path"
if [ -n "$or2_path" ]; then echo "or2:tmux-version:$("$or2_path" -V 2>/dev/null </dev/null)"; fi
or2_find herdr
echo "or2:herdr:$or2_path"
or2_find mosh-server
echo "or2:mosh-server:$or2_path"
or2_loc=en_US.UTF-8
or2_found=
or2_first=
for or2_x in $(locale -a 2>/dev/null); do
  case "$or2_x" in
    C.[Uu][Tt][Ff]8|C.[Uu][Tt][Ff]-8) or2_found=C.UTF-8 ;;
    *.[Uu][Tt][Ff]8|*.[Uu][Tt][Ff]-8) if [ -z "$or2_first" ]; then or2_first=$or2_x; fi ;;
  esac
done
if [ -n "$or2_found" ]; then or2_loc=$or2_found; elif [ -n "$or2_first" ]; then or2_loc=$or2_first; fi
echo "or2:locale:$or2_loc"
echo "or2:end"
"#;

/// Finds herdr like [`PROBE_SCRIPT`] and runs its `session list --json` in the same exec:
/// `or2:herdr:<path>`, then (when found) `or2:list-begin`, herdr's stdout, and
/// `or2:list-end:<status>`. Same rendering contract: no `'`, no `\`.
pub const HERDR_SCRIPT: &str = r#"
or2_find() {
  or2_path=$(command -v "$1" 2>/dev/null)
  case "$or2_path" in
    /*) ;;
    *)
      or2_path=
      for or2_dir in "$HOME/.local/bin" "$HOME/.cargo/bin" /opt/homebrew/bin /usr/local/bin /usr/bin /bin "$HOME/.nix-profile/bin" /run/current-system/sw/bin; do
        if [ -f "$or2_dir/$1" ] && [ -x "$or2_dir/$1" ]; then or2_path="$or2_dir/$1"; break; fi
      done
      ;;
  esac
}
or2_find herdr
echo "or2:herdr:$or2_path"
if [ -n "$or2_path" ]; then
  echo "or2:list-begin"
  "$or2_path" session list --json 2>/dev/null
  or2_status=$?
  echo
  echo "or2:list-end:$or2_status"
fi
"#;

/// The locale reported when the host lists no UTF-8 one.
const FALLBACK_LOCALE: &str = "en_US.UTF-8";

/// Runs [`PROBE_SCRIPT`] and [`HERDR_SCRIPT`] on `host` at once and parses the answers. A
/// script that fails to run is an error; a missing program is `None` in the result; a herdr that
/// cannot list its sessions (hung, failing, garbage) is found with no sessions. Only a
/// connection that closes mid-probe fails the second script.
pub async fn probe<H: RemoteHost>(host: &H) -> Result<HostCapabilities, RemoteError> {
    probe_entries(host).await.map(|(caps, _)| caps)
}

/// [`probe`], also returning the session listing the second script read (`None` when herdr is
/// missing or its listing failed), with the sockets the host driver's [`Directory`] needs.
pub async fn probe_entries<H: RemoteHost>(
    host: &H,
) -> Result<(HostCapabilities, Option<Vec<SessionEntry>>), RemoteError> {
    probe_within(host, HERDR_LIST_TIMEOUT, probe_programs(host)).await
}

/// The program probe alone: [`PROBE_SCRIPT`]'s programs and locale, `herdr_sessions` empty.
/// One exec round trip, never held up by herdr's session listing: the host driver caches it
/// apart from the listing, so a terminal open (which needs only a program's path) and
/// `mosh_server()` resolve as soon as this script returns, however long herdr takes.
pub async fn probe_programs<H: RemoteHost>(host: &H) -> Result<HostCapabilities, RemoteError> {
    let output = host.exec_script(PROBE_SCRIPT).await?;
    if !output.success() {
        return Err(RemoteError::Io(format!(
            "the capability probe exited with status {:?}",
            output.status
        )));
    }
    Ok(parse(&String::from_utf8_lossy(&output.stdout)))
}

/// The whole probe: `programs` (the program probe: [`probe_programs`], or the host driver's
/// cache of it, which publishes its answer to every waiter the moment [`PROBE_SCRIPT`] returns)
/// and [`HERDR_SCRIPT`] in its own exec at the same time, the listing bounded by
/// `herdr_limit`. The listing only fills `herdr_sessions`; nothing that needs a program's path
/// waits for it.
pub(crate) async fn probe_within<H: RemoteHost>(
    host: &H,
    herdr_limit: Duration,
    programs: impl Future<Output = Result<HostCapabilities, RemoteError>>,
) -> Result<(HostCapabilities, Option<Vec<SessionEntry>>), RemoteError> {
    let (programs, herdr) = tokio::join!(
        programs,
        tokio::time::timeout(herdr_limit, host.exec_script(HERDR_SCRIPT)),
    );
    let mut caps = programs?;
    let mut entries = None;
    if caps.herdr.is_some() {
        match herdr {
            Ok(Ok(output)) => {
                entries = parse_herdr_listing(&String::from_utf8_lossy(&output.stdout));
            }
            Ok(Err(RemoteError::Closed)) => return Err(RemoteError::Closed),
            // A herdr that hangs or cannot run: found, with no sessions.
            Ok(Err(_)) | Err(_) => {}
        }
    }
    if let Some(entries) = &entries {
        caps.herdr_sessions = entries.iter().map(session_info).collect();
    }
    Ok((caps, entries))
}

fn session_info(entry: &SessionEntry) -> HerdrSessionInfo {
    HerdrSessionInfo {
        name: entry.name.clone(),
        running: entry.running,
        is_default: entry.default,
    }
}

/// The listing [`HERDR_SCRIPT`] printed between its markers, read like a `session list --json`
/// of its own. `None` when herdr was not found, the script was cut short, or the listing failed
/// (any [`DiscoveryError`]).
fn parse_herdr_listing(output: &str) -> Option<Vec<SessionEntry>> {
    let (_, rest) = output.split_once("or2:list-begin\n")?;
    let (listing, end) = rest.rsplit_once("\nor2:list-end:")?;
    let status: u32 = end.lines().next()?.trim().parse().ok()?;
    herdr::parse_listing(Some(status), listing.as_bytes(), b"").ok()
}

/// Lists herdr's sessions through [`herdr::list_sessions`] (the one parser of
/// `<herdr> session list --json`), giving up after [`HERDR_LIST_TIMEOUT`] (`TimedOut`). A failing
/// herdr, or output that is not herdr's session list, is `Io`; a closed connection stays
/// `Closed`.
pub async fn herdr_sessions<H: RemoteHost>(
    host: &H,
    herdr: &str,
) -> Result<Vec<HerdrSessionInfo>, RemoteError> {
    let entries = tokio::time::timeout(HERDR_LIST_TIMEOUT, herdr::list_sessions(host, herdr))
        .await
        .map_err(|_| RemoteError::TimedOut)?;
    match entries {
        Ok(entries) => Ok(entries.iter().map(session_info).collect()),
        Err(DiscoveryError::Remote(error)) => Err(error),
        Err(other) => Err(RemoteError::Io(other.to_string())),
    }
}

/// The last session list read successfully, kept apart from the probe's immutable programs and
/// locale, in the connection's [`Directory`] (which the herdr watches and pane focuses take their
/// sockets from). Until a read succeeds, the list the probe itself found is the one reported.
///
/// [`SessionsCache::capabilities`] reads the list again on every call, so `running` and new or
/// stopped sessions show, except for the first call after the probe seeded the directory: that
/// list is as fresh as a read made now. A listing that fails (herdr gone, hung, garbage)
/// reports the LAST successful list, not the one from connect time: the app treats the list as
/// authoritative (a session missing from it has its watch stopped), so a transient failure must
/// neither drop a session found since connecting nor bring back one that has gone. Reads can
/// overlap; the directory applies only the newest (see [`Directory`]).
#[derive(Debug, Default)]
pub struct SessionsCache {
    directory: Arc<Directory>,
}

impl SessionsCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The directory the herdr watches and focuses discover sockets through.
    pub fn directory(&self) -> &Arc<Directory> {
        &self.directory
    }

    /// `cached` with herdr's session list read again and remembered; programs and locale are
    /// never searched twice. A listing that fails reports the last successful list; only a
    /// closed connection is an error. Without herdr nothing is listed or run.
    pub async fn capabilities<H: RemoteHost>(
        &self,
        host: &H,
        cached: &HostCapabilities,
    ) -> Result<HostCapabilities, RemoteError> {
        let mut caps = cached.clone();
        let Some(herdr) = &cached.herdr else {
            return Ok(caps);
        };
        if !self.directory.take_unread() {
            let read =
                tokio::time::timeout(HERDR_LIST_TIMEOUT, self.directory.refresh(host, herdr)).await;
            if let Ok(Err(DiscoveryError::Remote(RemoteError::Closed))) = read {
                return Err(RemoteError::Closed);
            }
            // Any other failure, or a read that lost to a newer one, leaves the stored list.
        }
        if let Some(entries) = self.directory.entries() {
            caps.herdr_sessions = entries.iter().map(session_info).collect();
        }
        Ok(caps)
    }
}

/// Parses the probe's output. Lenient by design: lines that are not the probe's are ignored,
/// an unusable path counts as not installed, a missing or unreadable `tmux -V` means tmux
/// cannot record a terminal's client (`tmux_records_clients`), and a missing or odd locale
/// falls back to `en_US.UTF-8`. `herdr_sessions` stays empty: [`herdr_sessions`] fills it.
pub fn parse(output: &str) -> HostCapabilities {
    let mut caps = HostCapabilities {
        tmux: None,
        herdr: None,
        mosh_server: None,
        tmux_records_clients: false,
        utf8_locale: FALLBACK_LOCALE.into(),
        herdr_sessions: Vec::new(),
    };
    for line in output.lines() {
        let Some(rest) = line.strip_prefix("or2:") else {
            continue;
        };
        match rest.split_once(':') {
            Some(("tmux", path)) => caps.tmux = program_path(path),
            Some(("tmux-version", version)) => {
                caps.tmux_records_clients = tmux::records_clients(version);
            }
            Some(("herdr", path)) => caps.herdr = program_path(path),
            Some(("mosh-server", path)) => caps.mosh_server = program_path(path),
            Some(("locale", locale)) if is_locale(locale) => caps.utf8_locale = locale.into(),
            _ => {}
        }
    }
    // A version says nothing without the tmux it belongs to.
    caps.tmux_records_clients &= caps.tmux.is_some();
    caps
}

/// An absolute path every login shell can be handed as one quoted token.
fn program_path(path: &str) -> Option<String> {
    let usable = path.starts_with('/')
        && !path.contains('=')
        && !path.chars().any(|c| c.is_control() || c == '\\');
    usable.then(|| path.to_owned())
}

fn is_locale(locale: &str) -> bool {
    (1..=64).contains(&locale.len())
        && locale
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '@'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{ExecOutput, render_script};
    use std::sync::Mutex;

    #[test]
    fn the_script_renders_as_sh_dash_c_without_quotes_or_backslashes() {
        assert!(!PROBE_SCRIPT.contains(['\'', '\\']));
        let rendered = render_script(PROBE_SCRIPT).unwrap();
        assert!(rendered.starts_with("sh -c '"));
        assert!(rendered.ends_with("'"));
    }

    #[test]
    fn parses_programs_and_locale_ignoring_unknown_lines() {
        let caps = parse(
            "Welcome banner\n\
             or2:tmux:/usr/bin/tmux\n\
             or2:herdr:/home/u/.local/bin/herdr\n\
             or2:mosh-server:\n\
             or2:future:value\n\
             or2:locale:C.UTF-8\n\
             or2:end\n",
        );
        assert_eq!(caps.tmux.as_deref(), Some("/usr/bin/tmux"));
        assert_eq!(caps.herdr.as_deref(), Some("/home/u/.local/bin/herdr"));
        assert_eq!(caps.mosh_server, None);
        assert_eq!(caps.utf8_locale, "C.UTF-8");
        assert!(
            caps.herdr_sessions.is_empty(),
            "the session list is separate"
        );
        assert!(!caps.tmux_records_clients, "no version line");
    }

    #[test]
    fn tmux_records_clients_only_from_a_version_that_can() {
        let records = |version: &str| {
            parse(&format!(
                "or2:tmux:/usr/bin/tmux\nor2:tmux-version:{version}\nor2:end\n"
            ))
            .tmux_records_clients
        };
        assert!(records("tmux 3.4"));
        assert!(records("tmux 2.6"));
        // Too old for `set-option -F`, or `-V` said nothing readable: a plain attach.
        assert!(!records("tmux 2.5"));
        assert!(!records(""));
        assert!(!parse("or2:tmux:/usr/bin/tmux\nor2:end\n").tmux_records_clients);
        // A version line without a usable tmux path counts for nothing.
        assert!(!parse("or2:tmux:tmux\nor2:tmux-version:tmux 3.4\n").tmux_records_clients);
    }

    #[test]
    fn unusable_answers_fall_back_instead_of_failing() {
        let caps = parse(
            "or2:tmux:tmux\n\
             or2:herdr:/opt/he rdr\\bin\n\
             or2:mosh-server:/usr/bin/a=b\n\
             or2:locale:bad locale\n",
        );
        assert_eq!(caps.tmux, None, "relative paths are not installs");
        assert_eq!(caps.herdr, None, "backslashes cannot be quoted");
        assert_eq!(
            caps.mosh_server, None,
            "= would be read as an env assignment"
        );
        assert_eq!(caps.utf8_locale, "en_US.UTF-8");
        // A path with a space is a fine absolute path: it is one quoted token.
        let caps = parse("or2:tmux:/opt/my tools/tmux\n");
        assert_eq!(caps.tmux.as_deref(), Some("/opt/my tools/tmux"));
        assert_eq!(parse("").utf8_locale, "en_US.UTF-8");
    }

    /// A host that answers the probe script, then lets herdr's listing do `herdr`.
    struct Stub {
        herdr: Herdr,
    }

    #[derive(Clone)]
    enum Herdr {
        Sessions(&'static str),
        Hangs,
        Fails,
        ConnectionGone,
    }

    fn output(status: u32, stdout: &str) -> ExecOutput {
        ExecOutput {
            status: Some(status),
            stdout: stdout.as_bytes().into(),
            stderr: Default::default(),
        }
    }

    impl RemoteHost for Stub {
        type Stream = tokio::io::DuplexStream;

        async fn exec_rendered(&self, line: &str) -> Result<ExecOutput, RemoteError> {
            if line.starts_with("sh -c") && line.contains("or2:list-begin") {
                // The probe's herdr script: path and listing in one exec.
                let listed = |json: &str, status: u32| {
                    Ok(output(
                        0,
                        &format!(
                            "or2:herdr:/fake/herdr\nor2:list-begin\n{json}\nor2:list-end:{status}\n"
                        ),
                    ))
                };
                return match &self.herdr {
                    Herdr::Sessions(json) => listed(json, 0),
                    Herdr::Hangs => std::future::pending().await,
                    Herdr::Fails => listed("", 1),
                    Herdr::ConnectionGone => Err(RemoteError::Closed),
                };
            }
            if line.starts_with("sh -c") {
                return Ok(output(
                    0,
                    "or2:tmux:/fake/tmux\nor2:herdr:/fake/herdr\nor2:locale:C.UTF-8\nor2:end\n",
                ));
            }
            assert_eq!(line, "'/fake/herdr' 'session' 'list' '--json'");
            match &self.herdr {
                Herdr::Sessions(json) => Ok(output(0, json)),
                Herdr::Hangs => std::future::pending().await,
                Herdr::Fails => Ok(output(1, "")),
                Herdr::ConnectionGone => Err(RemoteError::Closed),
            }
        }

        async fn open_unix(&self, _: &str) -> Result<Self::Stream, RemoteError> {
            Err(RemoteError::Closed)
        }
    }

    const ONE: &str = r#"{"sessions":[{"name":"default","running":true,"default":true,"socket_path":"/s/default.sock","future":1}]}"#;

    #[tokio::test(start_paused = true)]
    async fn a_hung_herdr_costs_the_session_list_not_the_other_capabilities() {
        let caps = probe(&Stub {
            herdr: Herdr::Hangs,
        })
        .await
        .unwrap();
        assert_eq!(caps.tmux.as_deref(), Some("/fake/tmux"));
        assert_eq!(caps.herdr.as_deref(), Some("/fake/herdr"));
        assert_eq!(caps.utf8_locale, "C.UTF-8");
        assert!(caps.herdr_sessions.is_empty());
        assert_eq!(
            herdr_sessions(
                &Stub {
                    herdr: Herdr::Hangs
                },
                "/fake/herdr"
            )
            .await,
            Err(RemoteError::TimedOut)
        );
    }

    #[tokio::test]
    async fn a_herdr_that_fails_or_prints_garbage_is_found_with_no_sessions() {
        for herdr in [Herdr::Fails, Herdr::Sessions("not json")] {
            let caps = probe(&Stub { herdr }).await.unwrap();
            assert_eq!(caps.herdr.as_deref(), Some("/fake/herdr"));
            assert!(caps.herdr_sessions.is_empty());
        }
        let caps = probe(&Stub {
            herdr: Herdr::Sessions(ONE),
        })
        .await
        .unwrap();
        assert_eq!(
            caps.herdr_sessions,
            [HerdrSessionInfo {
                name: "default".into(),
                running: true,
                is_default: true
            }],
            "name, running and default come from herdr::list_sessions; the rest is dropped"
        );
    }

    #[tokio::test]
    async fn a_listing_failure_is_io_and_a_closed_connection_stays_closed() {
        for herdr in [Herdr::Fails, Herdr::Sessions("{\"sessions\":7}")] {
            assert!(matches!(
                herdr_sessions(&Stub { herdr }, "/fake/herdr").await,
                Err(RemoteError::Io(_))
            ));
        }
        assert_eq!(
            herdr_sessions(
                &Stub {
                    herdr: Herdr::ConnectionGone
                },
                "/fake/herdr"
            )
            .await,
            Err(RemoteError::Closed)
        );
    }

    #[tokio::test]
    async fn a_connection_that_closes_mid_probe_fails_it() {
        let result = probe(&Stub {
            herdr: Herdr::ConnectionGone,
        })
        .await;
        assert_eq!(result, Err(RemoteError::Closed));
    }

    const OTHER: &str =
        r#"{"sessions":[{"name":"other","running":false,"socket_path":"/s/other.sock"}]}"#;

    fn names(caps: &HostCapabilities) -> Vec<&str> {
        caps.herdr_sessions
            .iter()
            .map(|session| session.name.as_str())
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn fresh_sessions_replace_the_list_and_a_failing_listing_keeps_the_last_one() {
        let cached = probe(&Stub {
            herdr: Herdr::Sessions(ONE),
        })
        .await
        .unwrap();
        let sessions = SessionsCache::new();
        // Before any read succeeded, a failing listing reports what the probe found.
        let kept = sessions
            .capabilities(
                &Stub {
                    herdr: Herdr::Fails,
                },
                &cached,
            )
            .await
            .unwrap();
        assert_eq!(kept, cached);

        let fresh = sessions
            .capabilities(
                &Stub {
                    herdr: Herdr::Sessions(OTHER),
                },
                &cached,
            )
            .await
            .unwrap();
        assert_eq!(names(&fresh), ["other"]);
        assert_eq!(fresh.tmux, cached.tmux, "programs are never searched again");
        // The list found since connecting is kept through a failing listing, not the probe's.
        for herdr in [Herdr::Fails, Herdr::Sessions("not json"), Herdr::Hangs] {
            let kept = sessions
                .capabilities(&Stub { herdr }, &cached)
                .await
                .unwrap();
            assert_eq!(names(&kept), ["other"]);
        }
        // A session that has gone does not come back from the connect-time list either.
        let gone = sessions
            .capabilities(
                &Stub {
                    herdr: Herdr::Sessions(r#"{"sessions":[]}"#),
                },
                &cached,
            )
            .await
            .unwrap();
        assert!(gone.herdr_sessions.is_empty());
        let kept = sessions
            .capabilities(
                &Stub {
                    herdr: Herdr::Fails,
                },
                &cached,
            )
            .await
            .unwrap();
        assert!(kept.herdr_sessions.is_empty());

        assert_eq!(
            sessions
                .capabilities(
                    &Stub {
                        herdr: Herdr::ConnectionGone
                    },
                    &cached
                )
                .await,
            Err(RemoteError::Closed)
        );
        // Without herdr there is nothing to list and nothing is run.
        let no_herdr = HostCapabilities {
            herdr: None,
            herdr_sessions: Vec::new(),
            ..cached
        };
        let same = sessions
            .capabilities(
                &Stub {
                    herdr: Herdr::ConnectionGone,
                },
                &no_herdr,
            )
            .await
            .unwrap();
        assert_eq!(same, no_herdr);
    }

    /// A host whose herdr listings are answered in the order they start, each held until its
    /// gate opens.
    struct Gated {
        replies:
            Mutex<std::collections::VecDeque<(tokio::sync::oneshot::Receiver<()>, &'static str)>>,
    }

    impl RemoteHost for Gated {
        type Stream = tokio::io::DuplexStream;

        async fn exec_rendered(&self, line: &str) -> Result<ExecOutput, RemoteError> {
            assert_eq!(line, "'/fake/herdr' 'session' 'list' '--json'");
            let (gate, json) = self.replies.lock().unwrap().pop_front().expect("a reply");
            let _ = gate.await;
            Ok(output(0, json))
        }

        async fn open_unix(&self, _: &str) -> Result<Self::Stream, RemoteError> {
            Err(RemoteError::Closed)
        }
    }

    #[tokio::test]
    async fn an_older_read_that_finishes_last_cannot_overwrite_a_newer_list() {
        let cached = probe(&Stub {
            herdr: Herdr::Sessions(ONE),
        })
        .await
        .unwrap();
        let (open_older, older_gate) = tokio::sync::oneshot::channel();
        let (open_newer, newer_gate) = tokio::sync::oneshot::channel();
        // The older read (started first) is answered with the stale list, after the newer one.
        let host = Gated {
            replies: Mutex::new(
                [(older_gate, ONE), (newer_gate, OTHER)]
                    .into_iter()
                    .collect(),
            ),
        };
        let sessions = SessionsCache::new();
        let (older, newer, ()) = tokio::join!(
            sessions.capabilities(&host, &cached),
            sessions.capabilities(&host, &cached),
            async {
                tokio::task::yield_now().await;
                open_newer.send(()).unwrap();
                for _ in 0..8 {
                    tokio::task::yield_now().await;
                }
                open_older.send(()).unwrap();
            }
        );
        assert_eq!(names(&newer.unwrap()), ["other"]);
        assert_eq!(
            names(&older.unwrap()),
            ["other"],
            "the stale read reports the newer list it lost to"
        );
        let after = sessions
            .capabilities(
                &Stub {
                    herdr: Herdr::Fails,
                },
                &cached,
            )
            .await
            .unwrap();
        assert_eq!(names(&after), ["other"]);
    }

    #[test]
    fn the_herdr_script_obeys_the_rendering_contract_and_finds_herdr_like_the_probe() {
        assert!(!HERDR_SCRIPT.contains(['\'', '\\']));
        assert!(render_script(HERDR_SCRIPT).unwrap().starts_with("sh -c '"));
        // The same search order, so the two scripts agree on where herdr is.
        let search = |script: &str| {
            let from = script.find("for or2_dir in").unwrap();
            script[from..].lines().next().unwrap().to_owned()
        };
        assert_eq!(search(HERDR_SCRIPT), search(PROBE_SCRIPT));
    }

    #[test]
    fn the_listing_between_the_markers_is_read_like_a_session_list_of_its_own() {
        let listed = |json: &str, status: u32| {
            format!("or2:herdr:/h\nor2:list-begin\n{json}\nor2:list-end:{status}\n")
        };
        let entries = parse_herdr_listing(&listed(ONE, 0)).unwrap();
        assert_eq!(entries[0].name, "default");
        assert_eq!(entries[0].socket_path, "/s/default.sock");
        // herdr may pretty-print.
        let pretty = "{\n \"sessions\": [\n  {\"name\": \"a\", \"socket_path\": \"/a\"}\n ]\n}";
        assert_eq!(parse_herdr_listing(&listed(pretty, 0)).unwrap().len(), 1);
        // A failing herdr, garbage, a script cut short and no herdr at all are no listing.
        assert!(parse_herdr_listing(&listed("", 1)).is_none());
        assert!(parse_herdr_listing(&listed("", 127)).is_none());
        assert!(parse_herdr_listing(&listed("not json", 0)).is_none());
        assert!(parse_herdr_listing("or2:herdr:/h\nor2:list-begin\n{\"sess").is_none());
        assert!(parse_herdr_listing("or2:herdr:\n").is_none());
        assert!(parse_herdr_listing("").is_none());
    }

    /// Both probe scripts wait for each other: serial execution would never get past the
    /// barrier.
    struct Barrier {
        both: tokio::sync::Barrier,
    }

    impl RemoteHost for Barrier {
        type Stream = tokio::io::DuplexStream;

        async fn exec_rendered(&self, line: &str) -> Result<ExecOutput, RemoteError> {
            self.both.wait().await;
            Ok(if line.contains("or2:list-begin") {
                output(
                    0,
                    &format!("or2:herdr:/fake/herdr\nor2:list-begin\n{ONE}\nor2:list-end:0\n"),
                )
            } else {
                output(
                    0,
                    "or2:tmux:/fake/tmux\nor2:herdr:/fake/herdr\nor2:locale:C.UTF-8\nor2:end\n",
                )
            })
        }

        async fn open_unix(&self, _: &str) -> Result<Self::Stream, RemoteError> {
            Err(RemoteError::Closed)
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_probe_runs_its_two_scripts_at_once_and_hands_back_the_sockets() {
        let host = Barrier {
            both: tokio::sync::Barrier::new(2),
        };
        let (caps, entries) = tokio::time::timeout(Duration::from_secs(1), probe_entries(&host))
            .await
            .expect("the scripts did not run at the same time")
            .unwrap();
        assert_eq!(caps.tmux.as_deref(), Some("/fake/tmux"));
        assert_eq!(caps.herdr_sessions.len(), 1);
        assert_eq!(entries.unwrap()[0].socket_path, "/s/default.sock");
    }

    #[tokio::test(start_paused = true)]
    async fn the_first_capabilities_call_after_the_probe_does_not_list_again() {
        let (caps, entries) = probe_entries(&Stub {
            herdr: Herdr::Sessions(ONE),
        })
        .await
        .unwrap();
        let cache = SessionsCache::new();
        cache.directory().seed(entries.unwrap());
        // A listing that would fail if it ran: the first call must not run one.
        let host = Stub {
            herdr: Herdr::Sessions(OTHER),
        };
        let first = cache.capabilities(&host, &caps).await.unwrap();
        assert_eq!(first.herdr_sessions, caps.herdr_sessions);
        // The next call reads the listing.
        let again = cache.capabilities(&host, &caps).await.unwrap();
        assert_eq!(names(&again), ["other"]);
    }

    #[tokio::test(start_paused = true)]
    async fn the_program_probe_is_published_when_its_script_returns_not_with_the_listing() {
        let host = Stub {
            herdr: Herdr::Hangs,
        };
        let programs = tokio::sync::OnceCell::new();
        let start = tokio::time::Instant::now();
        let (whole, published) = tokio::join!(
            probe_within(&host, HERDR_LIST_TIMEOUT, async {
                programs
                    .get_or_try_init(|| probe_programs(&host))
                    .await
                    .cloned()
            }),
            async {
                // A terminal open waiting on the same cache while the probe runs.
                tokio::task::yield_now().await;
                let caps = programs
                    .get_or_try_init(|| probe_programs(&host))
                    .await
                    .unwrap()
                    .clone();
                (caps, start.elapsed())
            },
        );
        let (caps, waited) = published;
        assert_eq!(caps.tmux.as_deref(), Some("/fake/tmux"));
        assert_eq!(
            waited,
            Duration::ZERO,
            "the programs did not wait for herdr"
        );
        assert!(start.elapsed() >= HERDR_LIST_TIMEOUT, "the listing hung");
        let (whole, entries) = whole.unwrap();
        assert_eq!(whole.herdr.as_deref(), Some("/fake/herdr"));
        assert!(entries.is_none() && whole.herdr_sessions.is_empty());
        // The program probe alone never runs herdr's script.
        let alone = tokio::time::timeout(Duration::from_secs(1), probe_programs(&host))
            .await
            .expect("the program probe ran the hung listing")
            .unwrap();
        assert_eq!(alone, caps);
    }
}
