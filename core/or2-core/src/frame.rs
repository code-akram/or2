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
//! bounded by one viewport.

use crate::term::TerminalSize;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellWidth {
    Narrow,
    /// Head of a double-width grapheme; the glyph spans this and the next column.
    Wide,
    /// Second column of a wide grapheme. Never has text; paint only its background.
    SpacerTail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    cells: Vec<Cell>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    size: TerminalSize,
    full: bool,
    rows: Vec<Row>,
    /// `None` when hidden or outside the viewport.
    cursor: Option<Cursor>,
    /// Default background for clearing and margins.
    background: Rgb,
    scrollback: Scrollback,
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
    #[error("cursor is outside the viewport")]
    CursorOutOfRange,
    #[error("delta frame does not match the size of the frame it updates")]
    SizeMismatch,
    #[error("delta frame has no full frame to update")]
    NoBase,
}

impl Row {
    pub fn new(index: u16, wrapped: bool, cells: Vec<Cell>) -> Self {
        Self {
            index,
            wrapped,
            cells,
        }
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
        for (column, cell) in (0u16..).zip(&self.cells) {
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
            cursor,
            background,
            scrollback,
        })
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

    pub fn cursor(&self) -> Option<Cursor> {
        self.cursor
    }

    pub fn background(&self) -> Rgb {
        self.background
    }

    pub fn scrollback(&self) -> Scrollback {
        self.scrollback
    }

    /// Applies a newer delta of the same size: its rows replace ours, its scalars win.
    fn absorb(&mut self, newer: Frame) {
        let mut merged = Vec::with_capacity(self.rows.len() + newer.rows.len());
        let mut older = std::mem::take(&mut self.rows).into_iter().peekable();
        for row in newer.rows {
            while older.peek().is_some_and(|old| old.index < row.index) {
                merged.extend(older.next());
            }
            if older.peek().is_some_and(|old| old.index == row.index) {
                older.next();
            }
            merged.push(row);
        }
        merged.extend(older);
        self.rows = merged;
        self.cursor = newer.cursor;
        self.background = newer.background;
        self.scrollback = newer.scrollback;
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
    fn packs_rgb_as_0x00rrggbb() {
        assert_eq!(Rgb::new(0x12, 0x34, 0x56).packed(), 0x0012_3456);
        assert_eq!(Rgb::new(0xff, 0, 0).packed(), 0x00ff_0000);
    }
}
