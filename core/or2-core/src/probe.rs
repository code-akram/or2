//! The capability probe: what a host offers, found with one exec.
//!
//! Non-interactive SSH does not load the user's `PATH`, so [`PROBE_SCRIPT`] looks for `tmux`,
//! `herdr` and `mosh-server` with `command -v` and then in the usual user-local and package
//! manager directories, and picks a UTF-8 locale. It is one fixed `sh` script run with
//! [`RemoteHost::exec_script`], so it works over SSH and over
//! [`LocalHost`](crate::remote::LocalHost) alike. When herdr was found, a second, separately
//! bounded exec lists its sessions ([`herdr_sessions`]): a wedged herdr must not take down the
//! discovery of everything else. The host driver caches the answer for the connection's
//! lifetime (Rust has no storage) and re-reads only the session list when `capabilities()` is
//! queried ([`with_fresh_sessions`]).

use std::time::Duration;

use crate::herdr::{self, DiscoveryError};
use crate::host::{HerdrSessionInfo, HostCapabilities};
use crate::remote::{RemoteError, RemoteHost};

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

/// The locale reported when the host lists no UTF-8 one.
const FALLBACK_LOCALE: &str = "en_US.UTF-8";

/// Runs [`PROBE_SCRIPT`] on `host`, parses the answer and, when herdr was found, lists its
/// sessions. A script that fails to run is an error; a missing program is `None` in the
/// result; a herdr that cannot list its sessions (hung, failing, garbage) is found with no
/// sessions. Only a connection that closes mid-probe fails the second step.
pub async fn probe<H: RemoteHost>(host: &H) -> Result<HostCapabilities, RemoteError> {
    probe_within(host, HERDR_LIST_TIMEOUT).await
}

async fn probe_within<H: RemoteHost>(
    host: &H,
    herdr_limit: Duration,
) -> Result<HostCapabilities, RemoteError> {
    let output = host.exec_script(PROBE_SCRIPT).await?;
    if !output.success() {
        return Err(RemoteError::Io(format!(
            "the capability probe exited with status {:?}",
            output.status
        )));
    }
    let mut caps = parse(&String::from_utf8_lossy(&output.stdout));
    if let Some(herdr) = &caps.herdr {
        caps.herdr_sessions = match list_within(host, herdr, herdr_limit).await {
            Ok(sessions) => sessions,
            Err(RemoteError::Closed) => return Err(RemoteError::Closed),
            Err(_) => Vec::new(),
        };
    }
    Ok(caps)
}

/// Lists herdr's sessions through [`herdr::list_sessions`] (the one parser of
/// `<herdr> session list --json`), giving up after [`HERDR_LIST_TIMEOUT`] (`TimedOut`). A failing
/// herdr, or output that is not herdr's session list, is `Io`; a closed connection stays
/// `Closed`.
pub async fn herdr_sessions<H: RemoteHost>(
    host: &H,
    herdr: &str,
) -> Result<Vec<HerdrSessionInfo>, RemoteError> {
    list_within(host, herdr, HERDR_LIST_TIMEOUT).await
}

async fn list_within<H: RemoteHost>(
    host: &H,
    herdr: &str,
    limit: Duration,
) -> Result<Vec<HerdrSessionInfo>, RemoteError> {
    let entries = tokio::time::timeout(limit, herdr::list_sessions(host, herdr))
        .await
        .map_err(|_| RemoteError::TimedOut)?;
    match entries {
        Ok(entries) => Ok(entries
            .into_iter()
            .map(|entry| HerdrSessionInfo {
                name: entry.name,
                running: entry.running,
                is_default: entry.default,
            })
            .collect()),
        Err(DiscoveryError::Remote(error)) => Err(error),
        Err(other) => Err(RemoteError::Io(other.to_string())),
    }
}

/// `cached` with herdr's session list read again, so `running` and new or stopped sessions
/// show; programs and locale are never searched twice. A listing that fails keeps the cached
/// list; only a closed connection is an error.
pub async fn with_fresh_sessions<H: RemoteHost>(
    host: &H,
    cached: &HostCapabilities,
) -> Result<HostCapabilities, RemoteError> {
    let mut caps = cached.clone();
    if let Some(herdr) = &cached.herdr {
        match herdr_sessions(host, herdr).await {
            Ok(sessions) => caps.herdr_sessions = sessions,
            Err(RemoteError::Closed) => return Err(RemoteError::Closed),
            Err(_) => {}
        }
    }
    Ok(caps)
}

/// Parses the probe's output. Lenient by design: lines that are not the probe's are ignored,
/// an unusable path counts as not installed, and a missing or odd locale falls back to
/// `en_US.UTF-8`. `herdr_sessions` stays empty: [`herdr_sessions`] fills it.
pub fn parse(output: &str) -> HostCapabilities {
    let mut caps = HostCapabilities {
        tmux: None,
        herdr: None,
        mosh_server: None,
        utf8_locale: FALLBACK_LOCALE.into(),
        herdr_sessions: Vec::new(),
    };
    for line in output.lines() {
        let Some(rest) = line.strip_prefix("or2:") else {
            continue;
        };
        match rest.split_once(':') {
            Some(("tmux", path)) => caps.tmux = program_path(path),
            Some(("herdr", path)) => caps.herdr = program_path(path),
            Some(("mosh-server", path)) => caps.mosh_server = program_path(path),
            Some(("locale", locale)) if is_locale(locale) => caps.utf8_locale = locale.into(),
            _ => {}
        }
    }
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
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    impl RemoteHost for Stub {
        type Stream = tokio::io::DuplexStream;

        async fn exec_rendered(&self, line: &str) -> Result<ExecOutput, RemoteError> {
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

    #[tokio::test]
    async fn fresh_sessions_replace_the_cached_list_and_a_failing_listing_keeps_it() {
        let cached = probe(&Stub {
            herdr: Herdr::Sessions(ONE),
        })
        .await
        .unwrap();
        let fresh = with_fresh_sessions(
            &Stub {
                herdr: Herdr::Sessions(r#"{"sessions":[{"name":"other","running":false,"socket_path":"/s/other.sock"}]}"#),
            },
            &cached,
        )
        .await
        .unwrap();
        assert_eq!(fresh.herdr_sessions[0].name, "other");
        assert_eq!(fresh.tmux, cached.tmux, "programs are never searched again");
        for herdr in [Herdr::Fails, Herdr::Sessions("not json")] {
            let kept = with_fresh_sessions(&Stub { herdr }, &cached).await.unwrap();
            assert_eq!(kept, cached);
        }
        assert_eq!(
            with_fresh_sessions(
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
        let same = with_fresh_sessions(
            &Stub {
                herdr: Herdr::ConnectionGone,
            },
            &no_herdr,
        )
        .await
        .unwrap();
        assert_eq!(same, no_herdr);
    }
}
