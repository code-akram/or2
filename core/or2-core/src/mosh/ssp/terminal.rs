// SPDX-License-Identifier: GPL-3.0-or-later
// Derived from mosh-rs (https://github.com/wilsonglasser/mosh-rs), commit
// 90b37125f5e4a598be91dec37d23921b6865276e, src/terminal.rs. Upstream: GPL-3.0-or-later, copyright
// Wilson Glasser; the protocol logic follows mosh (Keith Winstein and contributors, GPL-3.0-or-later).
// See THIRD_PARTY_NOTICES.md. or2 changes: no predictions or overlays, no repaint/diff output and no
// separate "displayed" screen. The newest state is one live screen that is mutated in place (so the
// emulator keeps its own dirty tracking and scrollback); older states are held as snapshots and
// restored on demand. Copies can fail, so the operations that make them return `ScreenError`.

//! The client's own copy of the screen (`completeterminal.cc`).
//!
//! A host diff describes the screen as of the state the SERVER believes the client holds. While
//! an acknowledgement is in flight the server recomputes the same frame from that older state,
//! so writing every diff straight through would paint the same output twice.
//!
//! mosh's answer, and this module's, is to keep the screen locally. Each received state is a
//! screen the client can reconstruct, and a diff is applied to the state it was computed FROM,
//! not to whatever is currently newest.
//!
//! The newest state is the one live [`Screen`]. In the common case the diff starts from exactly
//! that state, so it is snapshotted (the server may diff from it again if our acknowledgement is
//! lost) and then fed the diff in place. Only when the server diffs from an older state is that
//! state restored from its snapshot and the live screen replaced.

use std::collections::HashMap;

use super::screen::{Screen, ScreenError};

/// The client's terminal: the newest state live, the others held as snapshots.
pub struct ClientTerminal<S: Screen> {
    /// The newest state.
    live: S,
    /// Its state number.
    live_num: u64,
    /// Older states the server may still diff from, by state number.
    saved: HashMap<u64, S::Snapshot>,
    /// The shape every state is brought to. Snapshots are resized when restored.
    rows: u16,
    cols: u16,
}

impl<S: Screen> ClientTerminal<S> {
    /// Start from a blank screen both sides agree on: state 0.
    pub fn new(blank: S) -> Self {
        Self {
            rows: blank.rows(),
            cols: blank.cols(),
            live: blank,
            live_num: 0,
            saved: HashMap::new(),
        }
    }

    /// Apply a host diff that takes state `old_num` to `new_num`.
    ///
    /// `Ok(false)` when `old_num` names a screen no longer held, which the transport layer
    /// already filters; keeping the check here means a diff can never be applied to the wrong
    /// screen. States below `keep_from` (the instruction's `throwaway_num`) will never be diffed
    /// from again, so the base is not snapshotted for them.
    pub fn apply_diff(
        &mut self,
        old_num: u64,
        new_num: u64,
        bytes: &[u8],
        keep_from: u64,
    ) -> Result<bool, ScreenError> {
        if old_num == self.live_num && new_num > self.live_num {
            if old_num >= keep_from {
                self.saved.insert(old_num, self.live.snapshot()?);
            }
            self.live.feed(bytes);
            self.live_num = new_num;
            return Ok(true);
        }
        let mut base = if old_num == self.live_num {
            S::restore(&self.live.snapshot()?)?
        } else if let Some(snapshot) = self.saved.get(&old_num) {
            S::restore(snapshot)?
        } else {
            return Ok(false);
        };
        self.fit(&mut base)?;
        base.feed(bytes);
        if new_num > self.live_num {
            // The old live state may still be needed; the new one takes its place.
            if self.live_num >= keep_from {
                self.saved.insert(self.live_num, self.live.snapshot()?);
            }
            self.live = base;
            self.live_num = new_num;
        } else if new_num >= keep_from {
            // Out of order: an older state than the one we are showing.
            self.saved.insert(new_num, base.snapshot()?);
        }
        Ok(true)
    }

    /// Record the shape the session now has, so every state, including ones restored later,
    /// follows it.
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<(), ScreenError> {
        self.rows = rows;
        self.cols = cols;
        self.live.resize(rows, cols)
    }

    /// Forget states the server promised never to diff from again. Without this a long session
    /// keeps a snapshot per state forever.
    pub fn forget_before(&mut self, throwaway: u64) {
        self.saved.retain(|num, _| *num >= throwaway);
    }

    /// The newest state, for whoever draws it.
    pub fn live(&mut self) -> &mut S {
        &mut self.live
    }

    /// Highest state applied.
    pub fn latest(&self) -> u64 {
        self.live_num
    }

    /// How many states are held, the live one included; a session that never prunes would grow
    /// this without bound.
    pub fn held_states(&self) -> usize {
        1 + self.saved.len()
    }

    fn fit(&self, screen: &mut S) -> Result<(), ScreenError> {
        if screen.rows() != self.rows || screen.cols() != self.cols {
            screen.resize(self.rows, self.cols)?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// A screen that records what it was fed, as text, and counts its copies.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct TextScreen {
        pub text: String,
        pub rows: u16,
        pub cols: u16,
        pub generation: u32,
    }

    impl TextScreen {
        pub(crate) fn new(rows: u16, cols: u16) -> Self {
            Self {
                text: String::new(),
                rows,
                cols,
                generation: 0,
            }
        }
    }

    impl Screen for TextScreen {
        type Snapshot = TextScreen;

        fn feed(&mut self, bytes: &[u8]) {
            self.text.push_str(&String::from_utf8_lossy(bytes));
        }

        fn resize(&mut self, rows: u16, cols: u16) -> Result<(), ScreenError> {
            self.rows = rows;
            self.cols = cols;
            Ok(())
        }

        fn rows(&self) -> u16 {
            self.rows
        }

        fn cols(&self) -> u16 {
            self.cols
        }

        fn snapshot(&self) -> Result<TextScreen, ScreenError> {
            Ok(self.clone())
        }

        fn restore(snapshot: &TextScreen) -> Result<Self, ScreenError> {
            Ok(Self {
                generation: snapshot.generation + 1,
                ..snapshot.clone()
            })
        }
    }

    fn terminal() -> ClientTerminal<TextScreen> {
        ClientTerminal::new(TextScreen::new(3, 20))
    }

    #[test]
    fn a_diff_lands_on_the_state_it_was_computed_from() {
        let mut t = terminal();
        assert!(t.apply_diff(0, 1, b"hello", 0).unwrap());
        assert_eq!(t.live().text, "hello");

        // The server had not heard our ack yet, so this second diff starts from state 0 again
        // and repeats the frame.
        assert!(t.apply_diff(0, 2, b"hello world", 0).unwrap());
        // Applied to state 0, NOT to state 1: no doubled text.
        assert_eq!(t.live().text, "hello world");
        assert_eq!(t.latest(), 2);
    }

    #[test]
    fn the_common_case_mutates_the_live_screen_in_place() {
        let mut t = terminal();
        t.apply_diff(0, 1, b"a", 0).unwrap();
        t.apply_diff(1, 2, b"b", 0).unwrap();
        t.apply_diff(2, 3, b"c", 0).unwrap();
        // No restore happened: the live screen is the original, never a copy.
        assert_eq!(t.live().generation, 0);
        assert_eq!(t.live().text, "abc");
    }

    #[test]
    fn an_older_base_restores_a_snapshot_and_replaces_the_live_screen() {
        let mut t = terminal();
        t.apply_diff(0, 1, b"a", 0).unwrap();
        t.apply_diff(1, 2, b"b", 0).unwrap();
        // State 1 was "a"; a diff from it must not see "b".
        assert!(t.apply_diff(1, 3, b"c", 0).unwrap());
        assert_eq!(t.live().text, "ac");
        assert_eq!(t.live().generation, 1);
        // State 2 is still reconstructible.
        assert!(t.apply_diff(2, 4, b"d", 0).unwrap());
        assert_eq!(t.live().text, "abd");
    }

    #[test]
    fn a_diff_from_an_unknown_state_is_refused() {
        let mut t = terminal();
        assert!(!t.apply_diff(9, 10, b"nope", 0).unwrap());
        assert_eq!(t.latest(), 0);
        assert_eq!(t.live().text, "");
    }

    #[test]
    fn an_out_of_order_state_is_kept_without_replacing_the_newest() {
        let mut t = terminal();
        t.apply_diff(0, 1, b"a", 0).unwrap();
        t.apply_diff(1, 3, b"c", 0).unwrap();
        // State 2 arrives late, diffed from state 1.
        assert!(t.apply_diff(1, 2, b"b", 0).unwrap());
        assert_eq!(t.latest(), 3);
        assert_eq!(t.live().text, "ac");
        // And it is usable as a base afterwards.
        assert!(t.apply_diff(2, 4, b"d", 0).unwrap());
        assert_eq!(t.live().text, "abd");
    }

    #[test]
    fn forgetting_prunes_but_keeps_the_live_state() {
        let mut t = terminal();
        t.apply_diff(0, 1, b"a", 0).unwrap();
        t.apply_diff(1, 2, b"b", 0).unwrap();
        t.apply_diff(2, 3, b"c", 0).unwrap();
        assert_eq!(t.held_states(), 4);
        t.forget_before(3);
        assert_eq!(t.held_states(), 1);
        assert_eq!(t.live().text, "abc");
        // Pruned states are gone for good.
        assert!(!t.apply_diff(1, 4, b"x", 0).unwrap());
    }

    #[test]
    fn states_below_the_throwaway_are_never_snapshotted() {
        let mut t = terminal();
        t.apply_diff(0, 1, b"a", 1).unwrap();
        t.apply_diff(1, 2, b"b", 2).unwrap();
        // State 0 and 1 were below the throwaway when they were left: only the live one is held.
        assert_eq!(t.held_states(), 1);
    }

    #[test]
    fn restored_states_follow_the_current_shape() {
        let mut t = terminal();
        t.apply_diff(0, 1, b"a", 0).unwrap();
        t.apply_diff(1, 2, b"b", 0).unwrap();
        t.resize(10, 40).unwrap();
        assert!(t.apply_diff(1, 3, b"c", 0).unwrap());
        assert_eq!((t.live().rows, t.live().cols), (10, 40));
        assert_eq!(t.live().text, "ac");
    }
}
