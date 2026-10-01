//! The capability probe: what a host offers, found with one exec.
//!
//! Non-interactive SSH does not load the user's `PATH`, so [`PROBE_SCRIPT`] looks for `tmux`,
//! `herdr` and `mosh-server` with `command -v` and then in the usual user-local and package
//! manager directories, picks a UTF-8 locale, and lists herdr's sessions. It is one fixed
//! `sh` script run with [`RemoteHost::exec_script`], so it works over SSH and over
//! [`LocalHost`](crate::remote::LocalHost) alike. The host driver caches the answer for the
//! connection's lifetime; Rust has no storage.

use crate::host::{HerdrSessionInfo, HostCapabilities};
use crate::remote::{RemoteError, RemoteHost};

/// Fixed, with no `'` and no `\` (the rendering contract of `render_script`; a test enforces
/// it). Prints `or2:<name>:<value>` lines, then herdr's `session list --json` between
/// `or2:herdr-sessions-begin` and `or2:herdr-sessions-end` when herdr was found. Unknown
/// output is ignored by [`parse`].
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
or2_herdr=$or2_path
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
if [ -n "$or2_herdr" ]; then
  echo "or2:herdr-sessions-begin"
  "$or2_herdr" session list --json 2>/dev/null
  echo
  echo "or2:herdr-sessions-end"
fi
echo "or2:end"
"#;

/// The locale reported when the host lists no UTF-8 one.
const FALLBACK_LOCALE: &str = "en_US.UTF-8";

/// Runs [`PROBE_SCRIPT`] on `host` and parses the answer. A script that fails to run is an
/// error; a missing program is `None` in the result.
pub async fn probe<H: RemoteHost>(host: &H) -> Result<HostCapabilities, RemoteError> {
    let output = host.exec_script(PROBE_SCRIPT).await?;
    if !output.success() {
        return Err(RemoteError::Io(format!(
            "the capability probe exited with status {:?}",
            output.status
        )));
    }
    Ok(parse(&String::from_utf8_lossy(&output.stdout)))
}

/// Parses the probe's output. Lenient by design: lines that are not the probe's are ignored,
/// an unusable path counts as not installed, a missing or odd locale falls back to
/// `en_US.UTF-8`, and herdr's JSON is read for the fields or2 needs and nothing else.
pub fn parse(output: &str) -> HostCapabilities {
    let mut caps = HostCapabilities {
        tmux: None,
        herdr: None,
        mosh_server: None,
        utf8_locale: FALLBACK_LOCALE.into(),
        herdr_sessions: Vec::new(),
    };
    let mut json: Option<String> = None;
    for line in output.lines() {
        if let Some(collected) = &mut json {
            if line == "or2:herdr-sessions-end" {
                caps.herdr_sessions = parse_herdr_sessions(collected);
                json = None;
            } else {
                collected.push_str(line);
                collected.push('\n');
            }
            continue;
        }
        let Some(rest) = line.strip_prefix("or2:") else {
            continue;
        };
        if rest == "herdr-sessions-begin" {
            json = Some(String::new());
            continue;
        }
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

/// `{"sessions":[{"name":..,"running":..,"default":..,..}]}`. Anything else is no sessions;
/// an entry without a string name is skipped; unknown fields are ignored.
fn parse_herdr_sessions(json: &str) -> Vec<HerdrSessionInfo> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(sessions) = value
        .get("sessions")
        .and_then(|sessions| sessions.as_array())
    else {
        return Vec::new();
    };
    sessions
        .iter()
        .filter_map(|session| {
            let flag = |key| session.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
            Some(HerdrSessionInfo {
                name: session.get("name")?.as_str()?.to_owned(),
                running: flag("running"),
                is_default: flag("default"),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::render_script;

    #[test]
    fn the_script_renders_as_sh_dash_c_without_quotes_or_backslashes() {
        assert!(!PROBE_SCRIPT.contains(['\'', '\\']));
        let rendered = render_script(PROBE_SCRIPT).unwrap();
        assert!(rendered.starts_with("sh -c '"));
        assert!(rendered.ends_with("'"));
    }

    #[test]
    fn parses_programs_locale_and_herdr_sessions_ignoring_unknown_fields() {
        let caps = parse(
            "Welcome banner\n\
             or2:tmux:/usr/bin/tmux\n\
             or2:herdr:/home/u/.local/bin/herdr\n\
             or2:mosh-server:\n\
             or2:locale:C.UTF-8\n\
             or2:herdr-sessions-begin\n\
             {\"sessions\":[{\"default\":true,\"name\":\"default\",\"running\":true,\"future\":{\"x\":1}},\n\
             {\"name\":\"or2-spike\",\"running\":false,\"default\":false},{\"running\":true}]}\n\
             \n\
             or2:herdr-sessions-end\n\
             or2:end\n",
        );
        assert_eq!(caps.tmux.as_deref(), Some("/usr/bin/tmux"));
        assert_eq!(caps.herdr.as_deref(), Some("/home/u/.local/bin/herdr"));
        assert_eq!(caps.mosh_server, None);
        assert_eq!(caps.utf8_locale, "C.UTF-8");
        assert_eq!(
            caps.herdr_sessions,
            [
                HerdrSessionInfo {
                    name: "default".into(),
                    running: true,
                    is_default: true
                },
                HerdrSessionInfo {
                    name: "or2-spike".into(),
                    running: false,
                    is_default: false
                },
            ]
        );
    }

    #[test]
    fn unusable_answers_fall_back_instead_of_failing() {
        let caps = parse(
            "or2:tmux:tmux\n\
             or2:herdr:/opt/he rdr\\bin\n\
             or2:mosh-server:/usr/bin/a=b\n\
             or2:locale:bad locale\n\
             or2:herdr-sessions-begin\n\
             not json\n\
             or2:herdr-sessions-end\n",
        );
        assert_eq!(caps.tmux, None, "relative paths are not installs");
        assert_eq!(caps.herdr, None, "backslashes cannot be quoted");
        assert_eq!(
            caps.mosh_server, None,
            "= would be read as an env assignment"
        );
        assert_eq!(caps.utf8_locale, "en_US.UTF-8");
        assert!(caps.herdr_sessions.is_empty());
        // A path with a space is a fine absolute path: it is one quoted token.
        let caps = parse("or2:tmux:/opt/my tools/tmux\n");
        assert_eq!(caps.tmux.as_deref(), Some("/opt/my tools/tmux"));
        // Truncated output: the unterminated JSON is dropped, earlier answers stay.
        let caps = parse("or2:tmux:/usr/bin/tmux\nor2:herdr-sessions-begin\n{\"sessions\":[");
        assert_eq!(caps.tmux.as_deref(), Some("/usr/bin/tmux"));
        assert!(caps.herdr_sessions.is_empty());
        assert_eq!(parse("").utf8_locale, "en_US.UTF-8");
    }
}
