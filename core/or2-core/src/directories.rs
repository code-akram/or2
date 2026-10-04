//! Recent project paths from bounded agent history reads. No daemon, writes or local storage.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde::Deserialize;

use crate::remote::{RemoteCommand, RemoteError, RemoteHost};

/// At most this many distinct paths, newest first.
pub const MAX_DIRECTORIES: usize = 20;

// Fixed script, never interpolated. Claude's tail may begin with a partial JSON line (ignored).
// Codex rollout names start with an ISO date: reverse lexical order finds recent sessions,
// independently of when a file was copied. Read only their first (session_meta) record.
// find/sort can be expensive on a huge tree, so the entire query has its own 5 s deadline.
// Complete Codex metadata can include large instructions. Allow 64 KiB per header, plus
// one overflow byte so an overlong record is rejected, and cap their combined output at
// 512 KiB. With Claude's 256 KiB tail and framing this stays below the 1 MiB exec cap.
// Markers separate the two JSONL formats; prompts never leave this module or get logged.
const CLAUDE_LINE_CAP: usize = 262144;
const CODEX_LINE_CAP: usize = 65536;
const READ_HISTORY: &str = r#"
printf "CLAUDE
"
tail -c 262144 "${CLAUDE_CONFIG_DIR:-$HOME/.claude}/history.jsonl" 2>/dev/null
printf "
CODEX
"
root=${CODEX_HOME:-$HOME/.codex}
find "$root/sessions" -type f -name "rollout-*.jsonl" 2>/dev/null |
    LC_ALL=C sort -r | head -n 64 |
    while IFS= read -r file; do
        case "$file" in
            "$root"/sessions/*/rollout-*.jsonl) ;;
            *) continue ;;
        esac
        case "$file" in *"/../"*|*"/./"*) continue ;; esac
        head -c 65537 "$file" 2>/dev/null | head -n 1
        printf "
"
    done | head -c 524288
"#;

/// Missing histories are an empty list; exec failures remain errors. Dropping cancels the exec.
pub async fn recent<H: RemoteHost>(host: &H) -> Result<Vec<String>, RemoteError> {
    let output = tokio::time::timeout(Duration::from_secs(5), host.exec_script(READ_HISTORY))
        .await
        .map_err(|_| RemoteError::TimedOut)??;
    if !output.success() {
        return Err(RemoteError::Failed(
            "could not read recent directories".into(),
        ));
    }
    Ok(parse(&output.stdout))
}

/// A literal, absolute Unix path the common login-shell quoting can carry, and whose display
/// cannot hide/reorder characters. Reject rather than sanitize: the path must name exactly
/// what the user saw. Quotes, spaces, Unicode and shell metacharacters are fine as arguments.
pub fn valid_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= 4096
        && !path.contains(['\\', '\u{2028}', '\u{2029}'])
        && crate::herdr::display_text(path) == path
}

/// Merge this host's current live paths before its newest-first history. Exact deduplication,
/// stable source order and one shared cap; reject unsafe paths without changing them. Pure:
/// the caller supplies already-watched metadata, so no extra query or storage is needed.
pub fn merge<'a>(
    live: impl IntoIterator<Item = &'a str>,
    history: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    live.into_iter()
        .chain(history)
        .filter(|path| valid_path(path))
        .filter(|path| seen.insert(*path))
        .take(MAX_DIRECTORIES)
        .map(str::to_owned)
        .collect()
}

/// Shared by SSH and mosh. The path is an argument, never part of the script. A stale path
/// exits visibly instead of silently opening in home; login startup files remain the user's.
pub fn shell_command(path: &str) -> RemoteCommand {
    RemoteCommand::new("sh").args([
        "-c",
        "cd -- \"$1\" || exit 1; exec \"${SHELL:-/bin/sh}\" -l",
        "or2-shell",
        path,
    ])
}

#[derive(Deserialize)]
struct ClaudeEntry {
    project: String,
    timestamp: i64,
}

#[derive(Deserialize)]
struct CodexEntry {
    r#type: String,
    timestamp: String,
    payload: CodexMetadata,
}

#[derive(Deserialize)]
struct CodexMetadata {
    cwd: String,
}

fn parse(bytes: &[u8]) -> Vec<String> {
    let mut codex = false;
    let mut paths = BTreeMap::<String, i64>::new();
    for line in bytes.split(|&byte| byte == b'\n') {
        match line {
            b"CLAUDE" => {
                codex = false;
                continue;
            }
            b"CODEX" => {
                codex = true;
                continue;
            }
            _ => {}
        }
        let line_cap = if codex {
            CODEX_LINE_CAP
        } else {
            CLAUDE_LINE_CAP
        };
        if line.len() > line_cap {
            continue;
        }
        let entry = if codex {
            serde_json::from_slice::<CodexEntry>(line)
                .ok()
                .and_then(|entry| {
                    (entry.r#type == "session_meta")
                        .then(|| utc_millis(&entry.timestamp).map(|at| (entry.payload.cwd, at)))
                        .flatten()
                })
        } else {
            serde_json::from_slice::<ClaudeEntry>(line)
                .ok()
                .map(|entry| (entry.project, entry.timestamp))
        };
        if let Some((path, at)) = entry.filter(|(path, at)| valid_path(path) && *at >= 0) {
            paths
                .entry(path)
                .and_modify(|previous| *previous = (*previous).max(at))
                .or_insert(at);
        }
    }
    let mut paths: Vec<_> = paths.into_iter().collect();
    paths.sort_by(|(a_path, a_at), (b_path, b_at)| b_at.cmp(a_at).then(a_path.cmp(b_path)));
    paths
        .into_iter()
        .take(MAX_DIRECTORIES)
        .map(|(path, _)| path)
        .collect()
}

// Codex writes UTC RFC3339, not local time. Support whole seconds and fractional seconds;
// refuse other formats instead of guessing. No date dependency needed for this narrow form.
fn utc_millis(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes.last() != Some(&b'Z')
    {
        return None;
    }
    let number = |start, end| {
        let digits = bytes.get(start..end)?;
        if !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(digits).ok()?.parse::<i64>().ok()
    };
    let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
    let (hour, minute, second) = (number(11, 13)?, number(14, 16)?, number(17, 19)?);
    if !(1970..=2100).contains(&year)
        || !(1..=12).contains(&month)
        || !(0..24).contains(&hour)
        || !(0..60).contains(&minute)
        || !(0..60).contains(&second)
    {
        return None;
    }
    let leap = |y| y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=months[(month - 1) as usize]).contains(&day) {
        return None;
    }
    let mut days = 0;
    for y in 1970..year {
        days += if leap(y) { 366 } else { 365 };
    }
    days += months[..(month - 1) as usize].iter().sum::<i64>() + day - 1;
    let fraction = &bytes[19..bytes.len() - 1];
    let millis = if fraction.is_empty() {
        0
    } else {
        if fraction[0] != b'.'
            || fraction.len() < 2
            || !fraction[1..].iter().all(u8::is_ascii_digit)
        {
            return None;
        }
        fraction[1..]
            .iter()
            .take(3)
            .chain(std::iter::repeat(&b'0'))
            .take(3)
            .fold(0_i64, |value, digit| value * 10 + i64::from(digit - b'0'))
    };
    Some(((days * 24 + hour) * 60 * 60 + minute * 60 + second) * 1000 + millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histories_merge_by_recency_not_source_and_deduplicate() {
        let history = br#"CLAUDE
truncated fragment
{"project":"/work/older","timestamp":1000,"display":"never expose this prompt"}
{"project":"/work/shared","timestamp":3000}
{"project":"/work/newest","timestamp":5000,"unknown":true}
CODEX
{"type":"session_meta","timestamp":"1970-01-01T00:00:04.001Z","payload":{"cwd":"/work/shared","other":"ignored"}}
{"type":"session_meta","timestamp":"1970-01-01T00:00:02Z","payload":{"cwd":"/work/codex"}}
{"type":"turn_context","timestamp":"1970-01-01T00:00:09Z","payload":{"cwd":"/wrong"}}
{"type":"session_meta","timestamp":"garbage","payload":{"cwd":"/wrong"}}
"#;
        assert_eq!(
            parse(history),
            ["/work/newest", "/work/shared", "/work/codex", "/work/older"]
        );
    }

    #[test]
    fn paths_are_literal_absolute_and_visible() {
        for path in ["/", "/a b/it's $(touch nope);é", "/--option"] {
            assert!(valid_path(path), "{path}");
        }
        for path in [
            "",
            "relative",
            "~/work",
            "/a\nb",
            "/a\\b",
            "/a\0b",
            "/a\u{202e}b",
            "/a\u{200b}b",
            "/a\u{2028}b",
            "/a\u{2029}b",
        ] {
            assert!(!valid_path(path), "{path:?}");
        }
        assert!(!valid_path(&format!("/{}", "a".repeat(4096))));
    }

    #[test]
    fn live_paths_precede_history_and_only_exact_duplicates_are_removed() {
        assert_eq!(
            merge(
                [
                    "/work/pi",
                    "/work/shared",
                    "/work/pi",
                    "/work/it's $(literal)",
                    "/Work/pi"
                ],
                ["/work/shared", "/work/history", "/work/history"]
            ),
            [
                "/work/pi",
                "/work/shared",
                "/work/it's $(literal)",
                "/Work/pi",
                "/work/history"
            ]
        );
    }

    #[test]
    fn merging_rejects_unsafe_paths_in_both_sources_before_dedup_and_the_cap() {
        let unsafe_paths = [
            "relative",
            "~/project",
            "/hidden\u{202e}path",
            "/hidden\u{200b}path",
            "/new\nline",
            "/back\\slash",
        ];
        assert!(merge(unsafe_paths, unsafe_paths).is_empty());
        let paths: Vec<_> = (0..30).map(|i| format!("/live/{i}")).collect();
        let live = unsafe_paths
            .into_iter()
            .chain(std::iter::repeat_n("/live/0", 25))
            .chain(paths.iter().map(String::as_str));
        let merged = merge(live, ["/history"]);
        assert_eq!(merged.len(), MAX_DIRECTORIES);
        assert_eq!(merged[0], "/live/0");
        assert_eq!(merged[19], "/live/19");
    }

    #[test]
    fn malformed_or_unsafe_records_are_skipped_and_the_list_is_capped() {
        let mut history = String::from(
            "CLAUDE\n{\"project\":\"relative\",\"timestamp\":5}\n{\"project\":\"/negative\",\"timestamp\":-1}\n",
        );
        for n in 0..50 {
            history.push_str(&format!(
                "{{\"project\":\"/work/{n}\",\"timestamp\":{n}}}\n"
            ));
        }
        let paths = parse(history.as_bytes());
        assert_eq!(paths.len(), MAX_DIRECTORIES);
        assert_eq!(paths[0], "/work/49");
        assert_eq!(paths[19], "/work/30");
    }

    #[test]
    fn utc_dates_validate_and_fractional_seconds_match_claude_millis() {
        assert_eq!(utc_millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            utc_millis("2000-02-29T12:34:56.123456Z"),
            Some(951827696123)
        );
        assert_eq!(utc_millis("2026-10-04T00:00:00.1Z"), Some(1791072000100));
        for bad in [
            "2025-02-29T00:00:00Z",
            "2026-10-04T24:00:00Z",
            "2026-+1-04T00:00:00Z",
            "2026-10-04T00:00:00.Z",
            "2026-10-04T00:00:00+01:00",
            "éééééééééééé",
        ] {
            assert_eq!(utc_millis(bad), None, "{bad}");
        }
    }

    struct HungHost;

    impl RemoteHost for HungHost {
        type Stream = tokio::io::DuplexStream;
        async fn exec_rendered(&self, _: &str) -> Result<crate::remote::ExecOutput, RemoteError> {
            std::future::pending().await
        }
        async fn open_unix(&self, _: &str) -> Result<Self::Stream, RemoteError> {
            unreachable!()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_hung_history_read_is_bounded_independently_of_connect() {
        let started = tokio::time::Instant::now();
        assert_eq!(recent(&HungHost).await, Err(RemoteError::TimedOut));
        assert_eq!(started.elapsed(), Duration::from_secs(5));
    }

    #[test]
    fn scripts_and_paths_are_quotable_without_interpolation() {
        crate::remote::render_script(READ_HISTORY).unwrap();
        let path = "/work/it's $(touch nope);é";
        let command = shell_command(path);
        assert_eq!(command.args.last().unwrap(), path);
        command.render().unwrap();
        assert_eq!(command.argv().unwrap().last().unwrap(), path);
    }
}
