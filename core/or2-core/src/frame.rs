//! Changed-row terminal frames: what Kotlin's Canvas renderer draws.
//!
//! A [`Frame`] is either *full* (every viewport row; the renderer drops its previous grid) or a
//! *delta* (only rows that changed since the frame the renderer last took, at the same size).
//! Colours are resolved in Rust: inverse and invisible are already applied, so Kotlin draws the
//! given foreground and background. Rows always contain exactly `columns` cells; a wide
//! character is a [`CellWidth::Wide`] head followed by a [`CellWidth::SpacerTail`].
//!
//! [`FrameMailbox`] is the backpressure point: the session publishes frames into it as output
//! arrives and the renderer takes the merged result when it draws. Nothing queues; memory is
//! bounded by one pending viewport. The engine retains one previous published viewport for
//! exact moved-row detection; row payloads are shared internally, never stored persistently.

use crate::term::TerminalSize;
use std::collections::{BTreeMap, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// `0x00RRGGBB`.
    pub fn packed(self) -> u32 {
        (u32::from(self.r) << 16) | (u32::from(self.g) << 8) | u32::from(self.b)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CellStyle {
    pub foreground: Rgb,
    pub background: Rgb,
    /// `None` draws the underline in the foreground colour.
    pub underline_color: Option<Rgb>,
    pub underline: Underline,
    pub bold: bool,
    pub italic: bool,
    /// Drawn with reduced foreground opacity.
    pub faint: bool,
    pub strikethrough: bool,
    pub overline: bool,
}

impl CellStyle {
    pub fn plain(foreground: Rgb, background: Rgb) -> Self {
        Self {
            foreground,
            background,
            underline_color: None,
            underline: Underline::None,
            bold: false,
            italic: false,
            faint: false,
            strikethrough: false,
            overline: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CellWidth {
    Narrow,
    /// Head of a double-width grapheme; the glyph spans this and the next column.
    Wide,
    /// Second column of a wide grapheme. Never has text; paint only its background.
    SpacerTail,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Cell {
    /// One grapheme cluster (possibly several code points); empty for blank cells and tails.
    pub text: String,
    pub width: CellWidth,
    pub style: CellStyle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    index: u16,
    /// Soft-wrapped into the next row: selection joins them without a newline.
    wrapped: bool,
    cells: Arc<Vec<Cell>>,
    /// OSC 8 hyperlinks, ascending by column and not overlapping; empty when none.
    links: Arc<Vec<CellLink>>,
}

/// An OSC 8 hyperlink over a run of a row's cells: `start_column..=end_column` (both
/// inclusive; a wide character's spacer tail is part of the run).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CellLink {
    pub start_column: u16,
    pub end_column: u16,
    pub uri: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    BlockHollow,
    Bar,
    Underline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    /// Leftmost column; on a wide grapheme this is its head.
    pub column: u16,
    pub row: u16,
    /// Covers a wide grapheme: draw it two columns wide.
    pub wide: bool,
    pub shape: CursorShape,
    pub blinking: bool,
    pub color: Rgb,
}

/// Viewport position in the scrollable area, in rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Scrollback {
    /// History plus the active screen.
    pub total_rows: u64,
    /// First viewport row's offset from the top; `total_rows - rows` means at the bottom.
    pub offset: u64,
}

/// Terminal modes Kotlin routes a vertical swipe by (contracts.md, "Wheel-aware scrolling") and
/// asks before a multi-line send by (bracketed paste).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalModes {
    /// The program asked for mouse reports (DECSET 9, 1000, 1002 or 1003).
    pub mouse_tracking: bool,
    /// The alternate screen is the active one.
    pub alternate_screen: bool,
    /// The program has bracketed paste (DECSET 2004) on: pasted or submitted text arrives as one
    /// paste, not as lines typed one at a time (contracts.md, "One terminal per herdr session").
    pub bracketed_paste: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    size: TerminalSize,
    full: bool,
    rows: Vec<Row>,
    row_moves: Vec<RowMove>,
    /// `None` when hidden or outside the viewport.
    cursor: Option<Cursor>,
    /// Default background for clearing and margins.
    background: Rgb,
    scrollback: Scrollback,
    modes: TerminalModes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("row {row} has {actual} cells, expected {expected}")]
    RowLength {
        row: u16,
        expected: u16,
        actual: usize,
    },
    #[error("row {row} is outside the viewport")]
    RowOutOfRange { row: u16 },
    #[error("rows must be strictly ascending")]
    RowOrder,
    #[error("a full frame must contain every row")]
    IncompleteFullFrame,
    #[error("row {row} column {column}: wide head without a following spacer tail")]
    WideWithoutTail { row: u16, column: u16 },
    #[error("row {row} column {column}: spacer tail without a wide head")]
    OrphanTail { row: u16, column: u16 },
    #[error("row {row} column {column}: spacer tails carry no text")]
    TailText { row: u16, column: u16 },
    #[error("row {row}: links must be inside the row, ascending and not overlapping")]
    LinkRange { row: u16 },
    #[error("cursor is outside the viewport")]
    CursorOutOfRange,
    #[error("delta frame does not match the size of the frame it updates")]
    SizeMismatch,
    #[error("delta frame has no full frame to update")]
    NoBase,
    #[error("row moves require a delta, valid sources and distinct ascending destinations")]
    InvalidRowMoves,
}

/// At publication: source in the previous published grid. At take: source in the last taken
/// grid. The mailbox composes these references; application is simultaneous, never in-place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowMove {
    pub index: u16,
    pub previous: u16,
}

impl Row {
    pub fn new(index: u16, wrapped: bool, cells: Vec<Cell>) -> Self {
        Self {
            index,
            wrapped,
            cells: Arc::new(cells),
            links: Arc::new(Vec::new()),
        }
    }

    /// The row with its OSC 8 hyperlinks.
    pub fn with_links(mut self, links: Vec<CellLink>) -> Self {
        self.links = Arc::new(links);
        self
    }

    pub fn index(&self) -> u16 {
        self.index
    }

    pub fn wrapped(&self) -> bool {
        self.wrapped
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    pub fn links(&self) -> &[CellLink] {
        &self.links
    }

    /// Owned row payload. Unshared cell/link strings move; still-retained payloads clone once
    /// for marshalling so the engine can compare the next publication exactly.
    pub fn into_parts(self) -> (u16, bool, Vec<Cell>, Vec<CellLink>) {
        (
            self.index,
            self.wrapped,
            Arc::unwrap_or_clone(self.cells),
            Arc::unwrap_or_clone(self.links),
        )
    }

    fn at(&self, index: u16) -> Self {
        Self {
            index,
            ..self.clone()
        }
    }

    fn same_content(&self, other: &Self) -> bool {
        self.wrapped == other.wrapped && self.cells == other.cells && self.links == other.links
    }

    fn content_hash(&self) -> u64 {
        let mut hash = DefaultHasher::new();
        self.wrapped.hash(&mut hash);
        self.cells.hash(&mut hash);
        self.links.hash(&mut hash);
        hash.finish()
    }

    fn validate(&self, size: TerminalSize) -> Result<(), FrameError> {
        let row = self.index;
        if row >= size.rows() {
            return Err(FrameError::RowOutOfRange { row });
        }
        if self.cells.len() != usize::from(size.columns()) {
            return Err(FrameError::RowLength {
                row,
                expected: size.columns(),
                actual: self.cells.len(),
            });
        }
        let mut previous = CellWidth::Narrow;
        for (column, cell) in (0u16..).zip(self.cells.iter()) {
            match (previous, cell.width) {
                (CellWidth::Wide, CellWidth::SpacerTail) => {}
                (CellWidth::Wide, _) => {
                    return Err(FrameError::WideWithoutTail {
                        row,
                        column: column - 1,
                    });
                }
                (_, CellWidth::SpacerTail) => return Err(FrameError::OrphanTail { row, column }),
                _ => {}
            }
            if cell.width == CellWidth::SpacerTail && !cell.text.is_empty() {
                return Err(FrameError::TailText { row, column });
            }
            previous = cell.width;
        }
        if previous == CellWidth::Wide {
            return Err(FrameError::WideWithoutTail {
                row,
                column: size.columns() - 1,
            });
        }
        let mut next_free = 0u16;
        for link in self.links.iter() {
            if link.start_column < next_free
                || link.end_column < link.start_column
                || link.end_column >= size.columns()
            {
                return Err(FrameError::LinkRange { row });
            }
            next_free = link.end_column + 1;
        }
        Ok(())
    }
}

impl Frame {
    pub fn full(
        size: TerminalSize,
        rows: Vec<Row>,
        cursor: Option<Cursor>,
        background: Rgb,
        scrollback: Scrollback,
    ) -> Result<Self, FrameError> {
        if rows.len() != usize::from(size.rows()) {
            return Err(FrameError::IncompleteFullFrame);
        }
        Self::checked(size, true, rows, cursor, background, scrollback)
    }

    pub fn delta(
        size: TerminalSize,
        rows: Vec<Row>,
        cursor: Option<Cursor>,
        background: Rgb,
        scrollback: Scrollback,
    ) -> Result<Self, FrameError> {
        Self::checked(size, false, rows, cursor, background, scrollback)
    }

    fn checked(
        size: TerminalSize,
        full: bool,
        rows: Vec<Row>,
        cursor: Option<Cursor>,
        background: Rgb,
        scrollback: Scrollback,
    ) -> Result<Self, FrameError> {
        for row in &rows {
            row.validate(size)?;
        }
        if rows.windows(2).any(|pair| pair[0].index >= pair[1].index) {
            return Err(FrameError::RowOrder);
        }
        if let Some(cursor) = cursor {
            let width = if cursor.wide { 2 } else { 1 };
            if cursor.row >= size.rows()
                || u32::from(cursor.column) + width > u32::from(size.columns())
            {
                return Err(FrameError::CursorOutOfRange);
            }
        }
        Ok(Self {
            size,
            full,
            rows,
            row_moves: Vec::new(),
            cursor,
            background,
            scrollback,
            modes: TerminalModes::default(),
        })
    }

    /// The same frame reporting `modes` (a new frame reports none set).
    pub fn with_modes(mut self, modes: TerminalModes) -> Self {
        self.modes = modes;
        self
    }

    pub fn modes(&self) -> TerminalModes {
        self.modes
    }

    pub fn size(&self) -> TerminalSize {
        self.size
    }

    pub fn is_full(&self) -> bool {
        self.full
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn row_moves(&self) -> &[RowMove] {
        &self.row_moves
    }

    pub fn with_row_moves(mut self, moves: Vec<RowMove>) -> Result<Self, FrameError> {
        if (!moves.is_empty() && self.full)
            || moves.windows(2).any(|pair| pair[0].index >= pair[1].index)
            || moves.iter().any(|m| {
                m.index >= self.size.rows()
                    || m.previous >= self.size.rows()
                    || self.rows.binary_search_by_key(&m.index, Row::index).is_ok()
            })
        {
            return Err(FrameError::InvalidRowMoves);
        }
        self.row_moves = moves;
        Ok(self)
    }

    pub fn into_rows(self) -> Vec<Row> {
        self.rows
    }

    pub fn cursor(&self) -> Option<Cursor> {
        self.cursor
    }

    pub fn background(&self) -> Rgb {
        self.background
    }

    pub fn scrollback(&self) -> Scrollback {
        self.scrollback
    }

    /// Compose simultaneous newer references against our still-pending state, never against
    /// already replaced destinations. A pending full has cells for every source, so remains full.
    fn absorb(&mut self, newer: Frame) {
        #[derive(Clone)]
        enum Update {
            Cells(Row),
            Previous(u16),
        }
        let mut updates = BTreeMap::new();
        for row in std::mem::take(&mut self.rows) {
            updates.insert(row.index, Update::Cells(row));
        }
        for m in std::mem::take(&mut self.row_moves) {
            updates.insert(m.index, Update::Previous(m.previous));
        }
        let moves: Vec<_> = newer
            .row_moves
            .iter()
            .map(|m| {
                let resolved = match updates.get(&m.previous) {
                    Some(Update::Cells(row)) => Update::Cells(row.at(m.index)),
                    Some(Update::Previous(previous)) => Update::Previous(*previous),
                    None => Update::Previous(m.previous),
                };
                (m.index, resolved)
            })
            .collect();
        updates.extend(moves);
        for row in newer.rows {
            updates.insert(row.index, Update::Cells(row));
        }
        for (index, update) in updates {
            match update {
                Update::Cells(row) => self.rows.push(row),
                Update::Previous(previous) => self.row_moves.push(RowMove { index, previous }),
            }
        }
        self.cursor = newer.cursor;
        self.background = newer.background;
        self.scrollback = newer.scrollback;
        self.modes = newer.modes;
    }
}

/// Producer-side moved-row detection. Retains one published viewport with shared row payloads.
/// Hashes select candidates only; resolved cells, wrap and links must also compare equal.
#[derive(Debug, Default)]
pub(crate) struct PublishedRows {
    size: Option<TerminalSize>,
    rows: Vec<Row>,
    hashes: Vec<u64>,
}

impl PublishedRows {
    /// Explicit reset snapshots must pass `false`. A native whole-screen dirty report may pass
    /// `true`: only an actual moved-content match permits converting it to a delta.
    pub(crate) fn encode(&mut self, mut frame: Frame, allow_full_delta: bool) -> Frame {
        // Cursor/mode-only publications (and no-op wheel checks) need no row bookkeeping.
        if frame.rows.is_empty() {
            return frame;
        }
        let compatible = self.size == Some(frame.size);
        if compatible && (!frame.full || allow_full_delta) {
            let mut candidates: HashMap<u64, Vec<usize>> = HashMap::new();
            for (index, &hash) in self.hashes.iter().enumerate() {
                candidates.entry(hash).or_default().push(index);
            }
            let mut moves = Vec::new();
            let mut retained = Vec::new();
            // Snapshot the previous publication before replacing any destination.
            let mut next = self.rows.clone();
            let mut next_hashes = self.hashes.clone();
            for row in frame.rows {
                let index = usize::from(row.index);
                let hash = row.content_hash();
                let source = if row.same_content(&self.rows[index]) {
                    None
                } else {
                    candidates.get(&hash).and_then(|indices| {
                        indices
                            .iter()
                            .copied()
                            .find(|&source| row.same_content(&self.rows[source]))
                    })
                };
                next_hashes[index] = hash;
                if let Some(source) = source {
                    next[index] = self.rows[source].at(row.index);
                    moves.push(RowMove {
                        index: row.index,
                        previous: source as u16,
                    });
                } else {
                    next[index] = row.clone();
                    retained.push(row);
                }
            }
            if frame.full && moves.is_empty() {
                // Preserve full-reset behaviour when no content actually moved.
                frame.rows = retained;
            } else {
                frame.full = false;
                frame.rows = retained;
                frame.row_moves = moves;
            }
            self.rows = next;
            self.hashes = next_hashes;
        } else if frame.full {
            self.rows = frame.rows.clone();
            self.hashes = self.rows.iter().map(Row::content_hash).collect();
        }
        self.size = Some(frame.size);
        frame
    }
}

/// A frame taken by the renderer. `sequence` starts at 1 and increases by one per take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakenFrame {
    pub sequence: u64,
    pub frame: Frame,
}

/// Latest-state frame buffer between the session (producer) and renderer (consumer).
#[derive(Debug, Default)]
pub struct FrameMailbox {
    pending: Option<Frame>,
    /// Size of the grid the renderer holds, once it has taken a frame.
    consumer_size: Option<TerminalSize>,
    sequence: u64,
}

impl FrameMailbox {
    /// Merges `frame` into the pending frame. Returns `true` when the mailbox was empty, i.e.
    /// when the renderer must be told a frame is ready; later publishes coalesce silently.
    pub fn publish(&mut self, frame: Frame) -> Result<bool, FrameError> {
        let was_empty = self.pending.is_none();
        if frame.full {
            self.pending = Some(frame);
            return Ok(was_empty);
        }
        let base = self
            .pending
            .as_ref()
            .map(Frame::size)
            .or(self.consumer_size)
            .ok_or(FrameError::NoBase)?;
        if base != frame.size {
            return Err(FrameError::SizeMismatch);
        }
        match &mut self.pending {
            Some(pending) => pending.absorb(frame),
            None => self.pending = Some(frame),
        }
        Ok(was_empty)
    }

    pub fn take(&mut self) -> Option<TakenFrame> {
        let frame = self.pending.take()?;
        self.consumer_size = Some(frame.size);
        self.sequence += 1;
        Some(TakenFrame {
            sequence: self.sequence,
            frame,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FG: Rgb = Rgb::new(0xdd, 0xdd, 0xdd);
    const BG: Rgb = Rgb::new(0x10, 0x10, 0x10);

    fn size(columns: u16, rows: u16) -> TerminalSize {
        TerminalSize::new(columns, rows).unwrap()
    }

    fn cell(text: &str, width: CellWidth) -> Cell {
        Cell {
            text: text.into(),
            width,
            style: CellStyle::plain(FG, BG),
        }
    }

    fn text_row(index: u16, text: &str, columns: u16) -> Row {
        let mut cells: Vec<Cell> = text
            .chars()
            .map(|c| cell(&c.to_string(), CellWidth::Narrow))
            .collect();
        cells.resize(usize::from(columns), cell("", CellWidth::Narrow));
        Row::new(index, false, cells)
    }

    fn full(columns: u16, rows: u16, label: &str) -> Frame {
        let rows_vec = (0..rows).map(|i| text_row(i, label, columns)).collect();
        Frame::full(
            size(columns, rows),
            rows_vec,
            None,
            BG,
            Scrollback::default(),
        )
        .unwrap()
    }

    fn delta(columns: u16, rows: u16, changed: &[(u16, &str)], cursor_column: u16) -> Frame {
        let changed = changed
            .iter()
            .map(|(i, text)| text_row(*i, text, columns))
            .collect();
        let cursor = Cursor {
            column: cursor_column,
            row: 0,
            wide: false,
            shape: CursorShape::Block,
            blinking: false,
            color: FG,
        };
        Frame::delta(
            size(columns, rows),
            changed,
            Some(cursor),
            BG,
            Scrollback::default(),
        )
        .unwrap()
    }

    fn texts(frame: &Frame) -> Vec<(u16, String)> {
        frame
            .rows()
            .iter()
            .map(|row| {
                let text: String = row.cells().iter().map(|c| c.text.as_str()).collect();
                (row.index(), text)
            })
            .collect()
    }

    #[test]
    fn wide_cells_need_exactly_one_tail() {
        let s = size(4, 1);
        let wide = vec![
            cell("界", CellWidth::Wide),
            cell("", CellWidth::SpacerTail),
            cell("a", CellWidth::Narrow),
            cell("", CellWidth::Narrow),
        ];
        assert!(
            Frame::delta(
                s,
                vec![Row::new(0, false, wide)],
                None,
                BG,
                Scrollback::default()
            )
            .is_ok()
        );

        let orphan = vec![
            cell("a", CellWidth::Narrow),
            cell("", CellWidth::SpacerTail),
            cell("", CellWidth::Narrow),
            cell("", CellWidth::Narrow),
        ];
        assert_eq!(
            Frame::delta(
                s,
                vec![Row::new(0, false, orphan)],
                None,
                BG,
                Scrollback::default()
            ),
            Err(FrameError::OrphanTail { row: 0, column: 1 })
        );

        let headless_end = vec![
            cell("a", CellWidth::Narrow),
            cell("", CellWidth::Narrow),
            cell("", CellWidth::Narrow),
            cell("界", CellWidth::Wide),
        ];
        assert_eq!(
            Frame::delta(
                s,
                vec![Row::new(0, false, headless_end)],
                None,
                BG,
                Scrollback::default()
            ),
            Err(FrameError::WideWithoutTail { row: 0, column: 3 })
        );

        let tail_text = vec![
            cell("界", CellWidth::Wide),
            cell("x", CellWidth::SpacerTail),
            cell("", CellWidth::Narrow),
            cell("", CellWidth::Narrow),
        ];
        assert_eq!(
            Frame::delta(
                s,
                vec![Row::new(0, false, tail_text)],
                None,
                BG,
                Scrollback::default()
            ),
            Err(FrameError::TailText { row: 0, column: 1 })
        );
    }

    #[test]
    fn frame_shape_is_validated_against_asymmetric_size() {
        let s = size(5, 3);
        assert_eq!(
            Frame::delta(s, vec![text_row(3, "", 5)], None, BG, Scrollback::default()),
            Err(FrameError::RowOutOfRange { row: 3 })
        );
        assert_eq!(
            Frame::delta(s, vec![text_row(0, "", 3)], None, BG, Scrollback::default()),
            Err(FrameError::RowLength {
                row: 0,
                expected: 5,
                actual: 3
            })
        );
        assert_eq!(
            Frame::delta(
                s,
                vec![text_row(1, "", 5), text_row(1, "", 5)],
                None,
                BG,
                Scrollback::default()
            ),
            Err(FrameError::RowOrder)
        );
        assert_eq!(
            Frame::full(s, vec![text_row(0, "", 5)], None, BG, Scrollback::default()),
            Err(FrameError::IncompleteFullFrame)
        );
        let cursor = |column, row, wide| Cursor {
            column,
            row,
            wide,
            shape: CursorShape::Bar,
            blinking: true,
            color: FG,
        };
        assert!(
            Frame::delta(
                s,
                vec![],
                Some(cursor(4, 2, false)),
                BG,
                Scrollback::default()
            )
            .is_ok()
        );
        assert!(
            Frame::delta(
                s,
                vec![],
                Some(cursor(3, 2, true)),
                BG,
                Scrollback::default()
            )
            .is_ok()
        );
        for bad in [cursor(5, 0, false), cursor(0, 3, false), cursor(4, 0, true)] {
            assert_eq!(
                Frame::delta(s, vec![], Some(bad), BG, Scrollback::default()),
                Err(FrameError::CursorOutOfRange)
            );
        }
    }

    #[test]
    fn delta_without_a_base_is_rejected() {
        let mut mailbox = FrameMailbox::default();
        assert_eq!(
            mailbox.publish(delta(4, 3, &[(0, "x")], 0)),
            Err(FrameError::NoBase)
        );
        assert!(mailbox.take().is_none());
    }

    #[test]
    fn deltas_merge_by_row_and_notify_once_per_take() {
        let mut mailbox = FrameMailbox::default();
        assert_eq!(mailbox.publish(full(4, 3, "a")), Ok(true));
        // The renderer has not taken the full frame: deltas fold into it and stay full.
        assert_eq!(mailbox.publish(delta(4, 3, &[(1, "b")], 2)), Ok(false));
        let first = mailbox.take().unwrap();
        assert_eq!(first.sequence, 1);
        assert!(first.frame.is_full());
        assert_eq!(
            texts(&first.frame),
            vec![(0, "a".into()), (1, "b".into()), (2, "a".into())]
        );
        assert_eq!(first.frame.cursor().unwrap().column, 2);
        assert!(mailbox.take().is_none());

        // After a take, deltas are relative to what the renderer holds.
        assert_eq!(mailbox.publish(delta(4, 3, &[(2, "c")], 1)), Ok(true));
        assert_eq!(
            mailbox.publish(delta(4, 3, &[(0, "d"), (2, "e")], 3)),
            Ok(false)
        );
        let second = mailbox.take().unwrap();
        assert_eq!(second.sequence, 2);
        assert!(!second.frame.is_full());
        assert_eq!(texts(&second.frame), vec![(0, "d".into()), (2, "e".into())]);
        assert_eq!(second.frame.cursor().unwrap().column, 3);
    }

    #[test]
    fn full_frame_supersedes_and_resize_requires_full() {
        let mut mailbox = FrameMailbox::default();
        mailbox.publish(full(4, 3, "a")).unwrap();
        mailbox.take().unwrap();
        mailbox.publish(delta(4, 3, &[(0, "x")], 0)).unwrap();
        // A resize: the delta at the new size is invalid until a full frame arrives.
        assert_eq!(
            mailbox.publish(delta(6, 2, &[(0, "y")], 0)),
            Err(FrameError::SizeMismatch)
        );
        assert_eq!(mailbox.publish(full(6, 2, "z")), Ok(false));
        assert_eq!(mailbox.publish(delta(6, 2, &[(1, "w")], 0)), Ok(false));
        let taken = mailbox.take().unwrap();
        assert!(taken.frame.is_full());
        assert_eq!(taken.frame.size(), size(6, 2));
        assert_eq!(texts(&taken.frame), vec![(0, "z".into()), (1, "w".into())]);
        // The renderer now holds 6x2, so old-size deltas are rejected.
        assert_eq!(
            mailbox.publish(delta(4, 3, &[(0, "x")], 0)),
            Err(FrameError::SizeMismatch)
        );
    }

    #[test]
    fn merged_deltas_report_the_newest_modes() {
        let mouse = TerminalModes {
            mouse_tracking: true,
            alternate_screen: true,
            bracketed_paste: false,
        };
        let mut mailbox = FrameMailbox::default();
        mailbox.publish(full(4, 3, "a")).unwrap();
        assert_eq!(
            mailbox.take().unwrap().frame.modes(),
            TerminalModes::default()
        );
        mailbox
            .publish(delta(4, 3, &[(0, "b")], 0).with_modes(mouse))
            .unwrap();
        // A delta without rows still carries the modes (a mode change dirties nothing).
        mailbox
            .publish(delta(4, 3, &[], 0).with_modes(mouse))
            .unwrap();
        assert_eq!(mailbox.take().unwrap().frame.modes(), mouse);
        mailbox
            .publish(delta(4, 3, &[(1, "c")], 0).with_modes(mouse))
            .unwrap();
        mailbox.publish(delta(4, 3, &[], 0)).unwrap();
        assert_eq!(
            mailbox.take().unwrap().frame.modes(),
            TerminalModes::default()
        );
    }

    #[test]
    fn links_must_lie_inside_the_row_in_order() {
        let link = |start, end| CellLink {
            start_column: start,
            end_column: end,
            uri: "https://example.org".into(),
        };
        let frame = |links: Vec<CellLink>| {
            Frame::delta(
                size(6, 2),
                vec![text_row(1, "abcdef", 6).with_links(links)],
                None,
                BG,
                Scrollback::default(),
            )
        };
        let ok = frame(vec![link(0, 1), link(2, 5)]).unwrap();
        assert_eq!(ok.rows()[0].links(), [link(0, 1), link(2, 5)]);
        for bad in [
            vec![link(0, 6)],
            vec![link(3, 2)],
            vec![link(0, 2), link(2, 3)],
            vec![link(3, 4), link(0, 1)],
        ] {
            assert_eq!(frame(bad), Err(FrameError::LinkRange { row: 1 }));
        }
    }

    #[test]
    fn packs_rgb_as_0x00rrggbb() {
        assert_eq!(Rgb::new(0x12, 0x34, 0x56).packed(), 0x0012_3456);
        assert_eq!(Rgb::new(0xff, 0, 0).packed(), 0x00ff_0000);
    }

    fn apply_rows(base: &[Row], frame: &Frame) -> Vec<Row> {
        if frame.full {
            return frame.rows.clone();
        }
        let mut next = base.to_vec();
        for m in &frame.row_moves {
            next[usize::from(m.index)] = base[usize::from(m.previous)].at(m.index);
        }
        for row in &frame.rows {
            next[usize::from(row.index)] = row.clone();
        }
        next
    }

    #[test]
    fn moves_validate_and_are_simultaneous_including_cycles_and_duplicates() {
        let base = full(1, 3, "a");
        let valid = vec![
            RowMove {
                index: 0,
                previous: 1,
            },
            RowMove {
                index: 1,
                previous: 0,
            },
        ];
        assert_eq!(
            base.with_row_moves(valid.clone()),
            Err(FrameError::InvalidRowMoves)
        );
        for invalid in [
            vec![RowMove {
                index: 3,
                previous: 0,
            }],
            vec![RowMove {
                index: 0,
                previous: 3,
            }],
            vec![
                RowMove {
                    index: 1,
                    previous: 0,
                },
                RowMove {
                    index: 1,
                    previous: 2,
                },
            ],
        ] {
            assert_eq!(
                delta(1, 3, &[], 0).with_row_moves(invalid),
                Err(FrameError::InvalidRowMoves)
            );
        }
        assert_eq!(
            delta(1, 3, &[(0, "x")], 0).with_row_moves(valid),
            Err(FrameError::InvalidRowMoves)
        );
        // Exhaust all three-source assignments across two coalesced publications. This includes
        // cycles, repeated sources and overwritten sources, with and without a pending full.
        for pending_full in [false, true] {
            for first in 0..27 {
                for second in 0..27 {
                    let mut mailbox = FrameMailbox::default();
                    let initial = Frame::full(
                        size(1, 3),
                        vec![
                            text_row(0, "a", 1),
                            text_row(1, "b", 1),
                            text_row(2, "c", 1),
                        ],
                        None,
                        BG,
                        Scrollback::default(),
                    )
                    .unwrap();
                    let mut expected = initial.rows.clone();
                    mailbox.publish(initial).unwrap();
                    if !pending_full {
                        mailbox.take().unwrap();
                    }
                    for assignment in [first, second] {
                        let moves = (0..3)
                            .map(|index| RowMove {
                                index,
                                previous: (assignment / 3u16.pow(u32::from(index))) % 3,
                            })
                            .collect();
                        let update = delta(1, 3, &[], 0).with_row_moves(moves).unwrap();
                        expected = apply_rows(&expected, &update);
                        mailbox.publish(update).unwrap();
                    }
                    // A new cell followed by a move of that cell must materialize it, because
                    // the consumer never saw the intermediate payload/table.
                    let update = delta(1, 3, &[(1, "x")], 0);
                    expected = apply_rows(&expected, &update);
                    mailbox.publish(update).unwrap();
                    let update = delta(1, 3, &[], 0)
                        .with_row_moves(vec![RowMove {
                            index: 0,
                            previous: 1,
                        }])
                        .unwrap();
                    expected = apply_rows(&expected, &update);
                    mailbox.publish(update).unwrap();
                    let taken = mailbox.take().unwrap();
                    let base = vec![
                        text_row(0, "a", 1),
                        text_row(1, "b", 1),
                        text_row(2, "c", 1),
                    ];
                    assert_eq!(apply_rows(&base, &taken.frame), expected);
                    assert_eq!(taken.frame.full, pending_full);
                    if pending_full {
                        assert!(taken.frame.row_moves.is_empty());
                    }
                }
            }
        }
    }

    #[test]
    fn moved_content_checks_resolved_styles_wrap_links_and_hash_collisions() {
        let base = Frame::full(
            size(1, 3),
            vec![
                text_row(0, "a", 1),
                text_row(1, "b", 1),
                text_row(2, "c", 1),
            ],
            None,
            BG,
            Scrollback::default(),
        )
        .unwrap();
        let mut published = PublishedRows::default();
        published.encode(base.clone(), false);
        let moved = published.encode(delta(1, 3, &[(0, "b"), (1, "c"), (2, "d")], 0), true);
        assert_eq!(
            moved.row_moves,
            [
                RowMove {
                    index: 0,
                    previous: 1
                },
                RowMove {
                    index: 1,
                    previous: 2
                }
            ]
        );
        assert_eq!(texts(&moved), [(2, "d".into())]);
        for change in 0..3 {
            published.encode(base.clone(), false);
            let mut changed = text_row(0, "b", 1);
            match change {
                0 => Arc::make_mut(&mut changed.cells)[0].style.bold = true,
                1 => changed.wrapped = true,
                _ => {
                    changed = changed.with_links(vec![CellLink {
                        start_column: 0,
                        end_column: 0,
                        uri: "https://example.org".into(),
                    }])
                }
            }
            // Force a candidate hash collision: equality must still reject it.
            published.hashes[1] = changed.content_hash();
            let update =
                Frame::delta(size(1, 3), vec![changed], None, BG, Scrollback::default()).unwrap();
            assert!(published.encode(update, true).row_moves.is_empty());
        }
        // Resync and resized snapshots are always self-contained.
        assert!(published.encode(base, false).row_moves.is_empty());
        assert!(published.encode(full(2, 2, "b"), true).is_full());
    }

    #[test]
    fn coalesced_moves_keep_untaken_styles_links_wrap_and_full_resets_drop_references() {
        let mut mailbox = FrameMailbox::default();
        mailbox.publish(full(1, 3, "a")).unwrap();
        mailbox.take().unwrap();
        let mut annotated = text_row(1, "x", 1).with_links(vec![CellLink {
            start_column: 0,
            end_column: 0,
            uri: "https://example.org".into(),
        }]);
        annotated.wrapped = true;
        Arc::make_mut(&mut annotated.cells)[0].style.bold = true;
        let update = Frame::delta(
            size(1, 3),
            vec![annotated.clone()],
            None,
            BG,
            Scrollback::default(),
        )
        .unwrap();
        mailbox.publish(update).unwrap();
        let modes = TerminalModes {
            mouse_tracking: true,
            ..TerminalModes::default()
        };
        mailbox
            .publish(
                delta(1, 3, &[], 0)
                    .with_modes(modes)
                    .with_row_moves(vec![RowMove {
                        index: 0,
                        previous: 1,
                    }])
                    .unwrap(),
            )
            .unwrap();
        let taken = mailbox.take().unwrap();
        assert!(taken.frame.row_moves.is_empty());
        assert_eq!(taken.frame.rows, [annotated.at(0), annotated]);
        assert_eq!(taken.frame.modes, modes);

        mailbox
            .publish(
                delta(1, 3, &[], 0)
                    .with_row_moves(vec![RowMove {
                        index: 0,
                        previous: 1,
                    }])
                    .unwrap(),
            )
            .unwrap();
        mailbox.publish(full(1, 3, "r")).unwrap(); // requestFullFrame supersedes pending moves.
        let reset = mailbox.take().unwrap();
        assert!(reset.frame.full && reset.frame.row_moves.is_empty());
        assert_eq!(
            texts(&reset.frame),
            [(0, "r".into()), (1, "r".into()), (2, "r".into())]
        );
        assert_eq!(
            mailbox.publish(
                delta(1, 2, &[], 0)
                    .with_row_moves(vec![RowMove {
                        index: 0,
                        previous: 1
                    }])
                    .unwrap()
            ),
            Err(FrameError::SizeMismatch)
        );
        mailbox.publish(full(1, 2, "s")).unwrap();
        assert!(mailbox.take().unwrap().frame.row_moves.is_empty());
    }
}
