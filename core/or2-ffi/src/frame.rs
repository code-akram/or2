//! Frame records for the Canvas renderer. Styles are deduplicated into a per-frame table and
//! cells refer to them by index; colours are `0x00RRGGBB`.

use std::collections::HashMap;

use or2_core::frame as core;

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TerminalFrame {
    /// Starts at 1 and increases by one per taken frame.
    pub sequence: u64,
    pub columns: u16,
    pub rows: u16,
    /// Replaces the whole grid. Otherwise only `changed_rows` changed, at the same size.
    pub full: bool,
    pub styles: Vec<CellStyle>,
    /// Ascending by index. A full frame lists every row.
    pub changed_rows: Vec<TerminalRow>,
    pub cursor: Option<TerminalCursor>,
    pub background: u32,
    pub scrollback: Scrollback,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TerminalRow {
    pub index: u16,
    pub wrapped: bool,
    /// Exactly `columns` cells.
    pub cells: Vec<TerminalCell>,
    /// OSC 8 hyperlinks, ascending by column and not overlapping; empty when none.
    #[uniffi(default)]
    pub links: Vec<CellLink>,
}

/// An OSC 8 hyperlink over the cells `start_column..=end_column` of a row (both inclusive; a
/// wide character's spacer tail is part of the run).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CellLink {
    pub start_column: u16,
    pub end_column: u16,
    pub uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TerminalCell {
    /// One grapheme cluster; empty for blank cells and spacer tails.
    pub text: String,
    pub width: CellWidth,
    /// Index into `TerminalFrame.styles`.
    pub style: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CellWidth {
    Narrow,
    Wide,
    SpacerTail,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CellStyle {
    pub foreground: u32,
    pub background: u32,
    pub underline_color: Option<u32>,
    pub underline: Underline,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub strikethrough: bool,
    pub overline: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Underline {
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TerminalCursor {
    pub column: u16,
    pub row: u16,
    pub wide: bool,
    pub shape: CursorShape,
    pub blinking: bool,
    pub color: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CursorShape {
    Block,
    BlockHollow,
    Bar,
    Underline,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Scrollback {
    pub total_rows: u64,
    pub offset: u64,
}

impl From<core::TakenFrame> for TerminalFrame {
    fn from(taken: core::TakenFrame) -> Self {
        let frame = taken.frame;
        let mut styles = Vec::new();
        let mut index_of = HashMap::new();
        let changed_rows = frame
            .rows()
            .iter()
            .map(|row| TerminalRow {
                index: row.index(),
                wrapped: row.wrapped(),
                cells: row
                    .cells()
                    .iter()
                    .map(|cell| TerminalCell {
                        text: cell.text.clone(),
                        width: cell.width.into(),
                        style: *index_of.entry(cell.style).or_insert_with(|| {
                            styles.push(CellStyle::from(cell.style));
                            u32::try_from(styles.len() - 1).expect("styles are bounded by cells")
                        }),
                    })
                    .collect(),
                links: row
                    .links()
                    .iter()
                    .map(|link| CellLink {
                        start_column: link.start_column,
                        end_column: link.end_column,
                        uri: link.uri.clone(),
                    })
                    .collect(),
            })
            .collect();
        let scrollback = frame.scrollback();
        Self {
            sequence: taken.sequence,
            columns: frame.size().columns(),
            rows: frame.size().rows(),
            full: frame.is_full(),
            styles,
            changed_rows,
            cursor: frame.cursor().map(Into::into),
            background: frame.background().packed(),
            scrollback: Scrollback {
                total_rows: scrollback.total_rows,
                offset: scrollback.offset,
            },
        }
    }
}

impl From<core::CellWidth> for CellWidth {
    fn from(width: core::CellWidth) -> Self {
        match width {
            core::CellWidth::Narrow => Self::Narrow,
            core::CellWidth::Wide => Self::Wide,
            core::CellWidth::SpacerTail => Self::SpacerTail,
        }
    }
}

impl From<core::CellStyle> for CellStyle {
    fn from(style: core::CellStyle) -> Self {
        Self {
            foreground: style.foreground.packed(),
            background: style.background.packed(),
            underline_color: style.underline_color.map(core::Rgb::packed),
            underline: match style.underline {
                core::Underline::None => Underline::None,
                core::Underline::Single => Underline::Single,
                core::Underline::Double => Underline::Double,
                core::Underline::Curly => Underline::Curly,
                core::Underline::Dotted => Underline::Dotted,
                core::Underline::Dashed => Underline::Dashed,
            },
            bold: style.bold,
            italic: style.italic,
            faint: style.faint,
            strikethrough: style.strikethrough,
            overline: style.overline,
        }
    }
}

impl From<core::Cursor> for TerminalCursor {
    fn from(cursor: core::Cursor) -> Self {
        Self {
            column: cursor.column,
            row: cursor.row,
            wide: cursor.wide,
            shape: match cursor.shape {
                core::CursorShape::Block => CursorShape::Block,
                core::CursorShape::BlockHollow => CursorShape::BlockHollow,
                core::CursorShape::Bar => CursorShape::Bar,
                core::CursorShape::Underline => CursorShape::Underline,
            },
            blinking: cursor.blinking,
            color: cursor.color.packed(),
        }
    }
}

#[cfg(test)]
mod tests {
    use or2_core::term::TerminalSize;

    use super::*;

    #[test]
    fn styles_are_deduplicated_in_first_use_order_and_rows_keep_their_indices() {
        let red = core::CellStyle {
            bold: true,
            ..core::CellStyle::plain(core::Rgb::new(0xff, 0, 0), core::Rgb::new(0, 0, 0))
        };
        let plain =
            core::CellStyle::plain(core::Rgb::new(0xcc, 0xcc, 0xcc), core::Rgb::new(0, 0, 0));
        let cell = |text: &str, width, style| core::Cell {
            text: text.into(),
            width,
            style,
        };
        let row = core::Row::new(
            2,
            true,
            vec![
                cell("R", core::CellWidth::Narrow, red),
                cell("界", core::CellWidth::Wide, plain),
                cell("", core::CellWidth::SpacerTail, plain),
                cell("!", core::CellWidth::Narrow, red),
            ],
        )
        .with_links(vec![core::CellLink {
            start_column: 1,
            end_column: 2,
            uri: "https://example.org".into(),
        }]);
        let frame = core::Frame::delta(
            TerminalSize::new(4, 3).unwrap(),
            vec![row],
            None,
            core::Rgb::new(0, 0, 0x10),
            core::Scrollback {
                total_rows: 103,
                offset: 100,
            },
        )
        .unwrap();
        let ffi = TerminalFrame::from(core::TakenFrame { sequence: 7, frame });
        assert_eq!(
            (ffi.sequence, ffi.columns, ffi.rows, ffi.full),
            (7, 4, 3, false)
        );
        assert_eq!(ffi.styles.len(), 2);
        assert_eq!(ffi.styles[0].foreground, 0x00ff_0000);
        assert!(ffi.styles[0].bold && !ffi.styles[1].bold);
        let row = &ffi.changed_rows[0];
        assert_eq!((row.index, row.wrapped), (2, true));
        let styles: Vec<u32> = row.cells.iter().map(|c| c.style).collect();
        assert_eq!(styles, [0, 1, 1, 0]);
        assert_eq!(row.cells[1].width, CellWidth::Wide);
        assert_eq!(row.cells[2].width, CellWidth::SpacerTail);
        assert_eq!(
            row.links,
            [CellLink {
                start_column: 1,
                end_column: 2,
                uri: "https://example.org".into()
            }]
        );
        assert_eq!(ffi.background, 0x10);
        assert_eq!(
            ffi.scrollback,
            Scrollback {
                total_rows: 103,
                offset: 100
            }
        );
        assert!(ffi.cursor.is_none());
    }
}
