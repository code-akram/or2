// SPDX-License-Identifier: GPL-3.0-or-later
// Derived from mosh-rs (https://github.com/wilsonglasser/mosh-rs), commit
// 90b37125f5e4a598be91dec37d23921b6865276e, src/screen.rs. Upstream: GPL-3.0-or-later, copyright
// Wilson Glasser; the protocol logic follows mosh (Keith Winstein and contributors, GPL-3.0-or-later).
// See THIRD_PARTY_NOTICES.md. or2 changes: rewritten around what or2 needs. The cell, colour and
// overlay types, `DiffScreen` and the vt100 screen are gone (prediction is off and the display is
// libghostty). `Screen` no longer requires `Clone`: copies are `snapshot`/`restore`, which can fail
// and which libghostty implements with its terminal snapshot encoding. `adopt_view_of` carries the viewer's scroll position across a restore.

//! What a state number resolves to: a screen.
//!
//! The protocol needs surprisingly little from a terminal emulator, and naming exactly what it
//! needs is what lets an application bring its own. A host diff is escape bytes to be fed
//! somewhere; a state is a screen that a later diff may be computed from, so it must be copyable.
//! That is the whole of [`Screen`].
//!
//! The copy is split into [`Screen::snapshot`] and [`Screen::restore`] rather than `Clone`, for
//! two reasons. Copying a real emulator can fail, and a client that panics on a network-driven
//! condition is not acceptable. And the client keeps a copy of nearly every state it receives but
//! reads almost none of them back: a snapshot can be a compact byte string, restored only on the
//! rare occasion the server diffs from an older state.

/// A screen could not be copied, resized or restored. The session treats it as fatal.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ScreenError(pub String);

/// A screen the client can feed, resize, snapshot and restore.
pub trait Screen: Sized {
    /// A held copy of a screen, kept per state the server may still diff from.
    type Snapshot;

    /// Feed host output. These are ECMA-48 escape bytes rendered by the SERVER's own emulator,
    /// so this is exactly what feeding a PTY would be.
    fn feed(&mut self, bytes: &[u8]);

    /// Change the screen's shape.
    fn resize(&mut self, rows: u16, cols: u16) -> Result<(), ScreenError>;

    // Deliberately two accessors rather than a tuple: every emulator orders that pair
    // differently and a silent transposition is the kind of bug that only shows on a non-square
    // terminal.
    /// How many rows the screen has.
    fn rows(&self) -> u16;
    /// How many columns the screen has.
    fn cols(&self) -> u16;

    /// A copy of the screen as it is now.
    fn snapshot(&self) -> Result<Self::Snapshot, ScreenError>;

    /// A live screen equal to the one `snapshot` was taken from.
    fn restore(snapshot: &Self::Snapshot) -> Result<Self, ScreenError>;

    /// Called on a screen restored from a snapshot just before it replaces the live one:
    /// carry over what belongs to the person looking at the screen rather than to the terminal
    /// state a snapshot records (for libghostty, where the viewport is scrolled to), so that
    /// swapping the engine does not move them. The default carries nothing.
    fn adopt_view_of(&mut self, _replaced: &Self) -> Result<(), ScreenError> {
        Ok(())
    }
}
