//! Submitting text to a terminal program: the text, a pause, then Enter as its own write.
//!
//! Agent TUIs (Codex, Claude Code) detect pastes by timing: a burst of characters that
//! contains Enter is a paste, and its Enter becomes a literal newline instead of a submit. A
//! composer that writes `text + CR` in one go is therefore never a submit for them. This is
//! what herdr's `agent prompt` does, and what [`Command::Submit`] does for every session
//! driver:
//!
//! 1. Write the text. With bracketed paste mode (DECSET 2004) on in the terminal, that is
//!    `ESC [ 200 ~ text ESC [ 201 ~` (newlines as typed, any end-of-paste marker in the text
//!    removed so it cannot close the paste early); otherwise the text as typing, with M1's
//!    newline mapping ([`text_bytes`]).
//! 2. After [`SUBMIT_ENTER_DELAY`], as a separate write, Enter encoded by the terminal's key
//!    encoder (so Kitty keyboard and modifyOtherKeys modes apply, as for `send_key`).
//!
//! The delay must not stall the driver: [`SubmitSequencer`] holds the deadline and the input
//! that arrives meanwhile, so the driver keeps serving output, resizes and disconnects, and
//! input stays in order behind the Enter.

use std::collections::VecDeque;
use std::time::Duration;

use tokio::time::{Instant, sleep_until};

use crate::input::{Key, KeyInput, Modifiers, text_bytes};
use crate::session::Command;

/// Pause between the text and its Enter. Long enough to end a paste burst, short enough to
/// feel instant.
pub const SUBMIT_ENTER_DELAY: Duration = Duration::from_millis(100);

const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &str = "\x1b[201~";

/// The bytes of a submit's first write: empty for empty text.
pub fn submit_text_bytes(text: &str, bracketed_paste: bool) -> Vec<u8> {
    if text.is_empty() {
        return Vec::new();
    }
    if !bracketed_paste {
        return text_bytes(text);
    }
    // Removing a marker can join its neighbours into a new one.
    let mut text = text.to_owned();
    while text.contains(PASTE_END) {
        text = text.replace(PASTE_END, "");
    }
    let mut bytes = Vec::with_capacity(PASTE_START.len() + text.len() + PASTE_END.len());
    bytes.extend_from_slice(PASTE_START);
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(PASTE_END.as_bytes());
    bytes
}

/// The key a submit ends with.
pub fn enter_key() -> KeyInput {
    KeyInput::new(Key::Enter, Modifiers::default()).expect("Enter is always valid")
}

/// Orders a driver's input around pending Enters. Not tied to any I/O: the driver feeds it
/// commands, awaits [`SubmitSequencer::due`] and performs the writes.
///
/// Driver loop shape: `if let Some(command) = sequencer.admit(command) { run(command) }` for
/// every command; run a `Submit` by writing its text and calling [`SubmitSequencer::arm`];
/// on `due()` call [`SubmitSequencer::disarm`], write Enter, then run
/// [`SubmitSequencer::next_deferred`] until it returns `None`.
#[derive(Debug, Default)]
pub struct SubmitSequencer {
    due_at: Option<Instant>,
    deferred: VecDeque<Command>,
}

impl SubmitSequencer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hands back `command` when it may run now. While an Enter is pending, input that writes
    /// to the terminal (`Text`, `Paste`, `Key`, `Submit`, `Scroll`, `MouseClick`) is held until
    /// the Enter is out; everything else (resize, full frame) is independent of input order.
    pub fn admit(&mut self, command: Command) -> Option<Command> {
        let ordered = matches!(
            command,
            Command::Text(_)
                | Command::Paste(_)
                | Command::Key(_)
                | Command::Submit(_)
                | Command::Scroll(_)
                | Command::MouseClick { .. }
        );
        if self.due_at.is_some() && ordered {
            self.deferred.push_back(command);
            None
        } else {
            Some(command)
        }
    }

    /// An Enter is now owed [`SUBMIT_ENTER_DELAY`] from now.
    pub fn arm(&mut self) {
        self.due_at = Some(Instant::now() + SUBMIT_ENTER_DELAY);
    }

    pub fn is_armed(&self) -> bool {
        self.due_at.is_some()
    }

    /// Resolves when the pending Enter is due; never while none is pending. Owns its deadline,
    /// so it does not borrow the sequencer.
    pub fn due(&self) -> impl Future<Output = ()> + use<> {
        let due_at = self.due_at;
        async move {
            match due_at {
                Some(at) => sleep_until(at).await,
                None => std::future::pending().await,
            }
        }
    }

    /// The Enter is being written now.
    pub fn disarm(&mut self) {
        self.due_at = None;
    }

    /// The next held command, once no Enter is pending. Running a held `Submit` arms again,
    /// which holds back the rest.
    pub fn next_deferred(&mut self) -> Option<Command> {
        if self.due_at.is_some() {
            return None;
        }
        self.deferred.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_uses_the_typing_newline_mapping() {
        assert_eq!(submit_text_bytes("a\nb\r\nc", false), b"a\rb\rc");
        assert!(submit_text_bytes("", false).is_empty());
    }

    #[test]
    fn bracketed_text_is_one_paste_with_newlines_as_typed() {
        assert_eq!(
            submit_text_bytes("a\nb\r\nc", true),
            b"\x1b[200~a\nb\r\nc\x1b[201~"
        );
        assert!(submit_text_bytes("", true).is_empty());
    }

    #[test]
    fn an_end_of_paste_marker_in_the_text_cannot_close_the_paste() {
        assert_eq!(
            submit_text_bytes("a\x1b[201~b", true),
            b"\x1b[200~ab\x1b[201~"
        );
        // Removal must not assemble a new marker from the halves.
        assert_eq!(
            submit_text_bytes("a\x1b[20\x1b[201~1~b", true),
            b"\x1b[200~ab\x1b[201~"
        );
        // A start marker is harmless inside a paste and stays.
        assert_eq!(
            submit_text_bytes("\x1b[200~x", true),
            b"\x1b[200~\x1b[200~x\x1b[201~"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn holds_ordered_input_until_the_enter_is_out() {
        let mut sequencer = SubmitSequencer::new();
        let text = Command::Text("a".into());
        assert_eq!(sequencer.admit(text.clone()), Some(text.clone()));
        sequencer.arm();
        assert!(sequencer.is_armed());
        assert_eq!(sequencer.admit(text.clone()), None);
        assert_eq!(
            sequencer.admit(Command::Submit("b".into())),
            None,
            "a second submit waits behind the first"
        );
        let full = Command::FullFrame;
        assert_eq!(sequencer.admit(full.clone()), Some(full));
        assert_eq!(sequencer.next_deferred(), None);
        let start = Instant::now();
        sequencer.due().await;
        assert_eq!(start.elapsed(), SUBMIT_ENTER_DELAY);
        sequencer.disarm();
        assert_eq!(sequencer.next_deferred(), Some(text));
        sequencer.arm();
        assert_eq!(
            sequencer.next_deferred(),
            None,
            "re-armed by the held submit"
        );
        sequencer.disarm();
        assert_eq!(sequencer.next_deferred(), Some(Command::Submit("b".into())));
        assert_eq!(sequencer.next_deferred(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn due_never_resolves_without_a_pending_enter() {
        let sequencer = SubmitSequencer::new();
        let waited = tokio::time::timeout(Duration::from_secs(60), sequencer.due()).await;
        assert!(waited.is_err());
    }
}
