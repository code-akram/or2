//! A tmux or herdr target's history as plain text, read from the host for the app's history
//! sheet ([`crate::host::HostHandle::read_history`]). Reading never moves the pane: tmux is read
//! with `capture-pane` (no copy mode), herdr with `pane.read` (no `pane.scroll`), so what the
//! host shows is unchanged.
//!
//! Both are bounded the same way: at most [`MAX_HISTORY_LINES`] lines are asked for and at most
//! [`MAX_HISTORY_BYTES`] of text are kept. Past the byte cap the oldest lines go, never the
//! newest, and the result says it was cut ([`HistoryText::truncated`]).

use crate::remote::OUTPUT_CAP;

/// The most lines a read asks for; more is clamped ([`history_lines`]).
pub const MAX_HISTORY_LINES: u32 = 5000;

/// The most text a read returns, the same as one exec's output cap: tmux's output passes through
/// exec, and herdr's answer is held to the same size so both read alike.
pub const MAX_HISTORY_BYTES: usize = OUTPUT_CAP;

/// The `CommandFailed` message of a history read of a shell target: only tmux and herdr keep a
/// history the host can read (a shell's own history file is never read).
pub const NO_SHELL_HISTORY: &str = "only tmux and herdr terminals have a history to read";

/// What a history read returns: plain text, oldest line first, without colours or other
/// escape sequences (control characters may remain; the app removes them before display).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryText {
    pub text: String,
    /// Older lines exist that are not in `text`: herdr said so, or the text passed
    /// [`MAX_HISTORY_BYTES`] and its oldest lines were dropped.
    pub truncated: bool,
}

/// `lines` within 1..=[`MAX_HISTORY_LINES`].
pub fn history_lines(lines: u32) -> u32 {
    lines.clamp(1, MAX_HISTORY_LINES)
}

/// The newest whole lines of `text` within [`MAX_HISTORY_BYTES`]. `cut` says `text` already
/// starts part way into the history (tmux's output was cut to the cap on the host), so its
/// first line may be partial and is dropped too. A text cut for either reason is `truncated`;
/// a single line longer than the cap keeps its newest bytes rather than nothing.
pub fn newest_lines(text: &str, cut: bool) -> HistoryText {
    newest_within(text, cut, MAX_HISTORY_BYTES)
}

fn newest_within(text: &str, cut: bool, cap: usize) -> HistoryText {
    let mut start = 0;
    if text.len() > cap {
        start = text.len() - cap;
        while !text.is_char_boundary(start) {
            start += 1;
        }
    }
    let truncated = cut || start > 0;
    // The first line kept is whole only when the cut fell just after a newline.
    let partial = if start > 0 {
        text.as_bytes()[start - 1] != b'\n'
    } else {
        cut
    };
    if partial && let Some(newline) = text[start..].find('\n') {
        start += newline + 1;
    }
    HistoryText {
        text: text[start..].to_owned(),
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_clamped_to_one_to_five_thousand() {
        assert_eq!(history_lines(0), 1);
        assert_eq!(history_lines(1), 1);
        assert_eq!(history_lines(2000), 2000);
        assert_eq!(history_lines(5000), 5000);
        assert_eq!(history_lines(5001), 5000);
        assert_eq!(history_lines(u32::MAX), 5000);
        assert_eq!(MAX_HISTORY_BYTES, 1024 * 1024);
    }

    #[test]
    fn text_within_the_cap_is_kept_whole() {
        let kept = newest_lines("one\ntwo\nthree", false);
        assert_eq!(kept.text, "one\ntwo\nthree");
        assert!(!kept.truncated);
        assert_eq!(newest_lines("", false).text, "");
    }

    #[test]
    fn past_the_cap_the_oldest_lines_go_and_the_newest_stay_whole() {
        let kept = newest_within("aaaa\nbbbb\ncccc\ndddd", false, 12);
        // The last 12 bytes start inside `bbbb`: that partial line goes too.
        assert_eq!(kept.text, "cccc\ndddd");
        assert!(kept.truncated);
        // A cut just after a newline leaves no partial line: nothing more goes.
        let kept = newest_within("aaaa\nbbbb\ncccc", false, 9);
        assert_eq!(kept.text, "bbbb\ncccc");
        assert!(kept.truncated);
        let kept = newest_within("aaaa\nbbbb\ncccc", false, 10);
        assert_eq!(kept.text, "bbbb\ncccc");
        assert!(kept.truncated);
    }

    #[test]
    fn a_cut_start_drops_its_first_line_even_within_the_cap() {
        let kept = newest_lines("bb\ncccc\ndddd", true);
        assert_eq!(kept.text, "cccc\ndddd");
        assert!(kept.truncated);
    }

    #[test]
    fn a_single_line_past_the_cap_keeps_its_newest_bytes_on_a_char_boundary() {
        let kept = newest_within("ééééé", false, 5);
        // Five bytes back lands inside an `é`: the cut moves forward to the next whole one.
        assert_eq!(kept.text, "éé");
        assert!(kept.truncated);
        let line = "x".repeat(MAX_HISTORY_BYTES + 10);
        let kept = newest_lines(&line, false);
        assert_eq!(kept.text.len(), MAX_HISTORY_BYTES);
        assert!(kept.truncated);
    }

    #[test]
    fn the_real_cap_holds_a_megabyte_of_newest_lines() {
        let text: String = (0..200_000).map(|n| format!("line {n}\n")).collect();
        let kept = newest_lines(&text, false);
        assert!(kept.text.len() <= MAX_HISTORY_BYTES);
        assert!(kept.truncated);
        assert!(kept.text.starts_with("line "));
        assert!(kept.text.ends_with("line 199999\n"));
    }
}
