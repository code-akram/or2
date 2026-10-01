//! libghostty-vt adapter. Every object stays on its creating session thread; only owned
//! frames and encoded byte vectors cross thread boundaries.

use libghostty_vt::key::{Action, Encoder, Event, Key as PhysicalKey, Mods};
use libghostty_vt::render::{CellIterator, CursorVisualStyle, Dirty, RowIterator};
use libghostty_vt::screen::{CellWide, Screen};
use libghostty_vt::snapshot::Decoder;
use libghostty_vt::style::{RgbColor, StyleColor, Underline as GhosttyUnderline};
use libghostty_vt::terminal::ScrollViewport;
use libghostty_vt::{RenderState, Terminal};

use crate::frame::{
    Cell, CellStyle, CellWidth, Cursor, CursorShape, Frame, Rgb, Row, Scrollback, Underline,
};
use crate::input::{Key, KeyInput, Modifiers, ViewportScroll};
use crate::term::TerminalSize;

#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error(transparent)]
    Engine(#[from] libghostty_vt::Error),
    #[error(transparent)]
    Frame(#[from] crate::frame::FrameError),
}

pub struct TerminalEngine {
    terminal: Terminal<'static, 'static>,
    render: RenderState<'static>,
    row_iter: RowIterator<'static>,
    cell_iter: CellIterator<'static>,
    encoder: Encoder<'static>,
    event: Event<'static>,
    size: TerminalSize,
    full: bool,
    colors: Option<(RgbColor, RgbColor)>,
}

impl TerminalEngine {
    pub fn new(size: TerminalSize, reply: impl Fn(&[u8]) + 'static) -> Result<Self, TerminalError> {
        Self::from_terminal(Terminal::new(size.columns(), size.rows())?, size, reply)
    }

    /// Restores an engine from [`TerminalEngine::snapshot`] bytes, at the geometry they were
    /// taken at; call [`TerminalEngine::resize`] to change it. Callbacks and options are not
    /// part of a snapshot, so `reply` and the default colours are installed again, and the
    /// first frame is a full one. Continuation tracking is off, as on a new engine.
    pub fn from_snapshot(
        snapshot: &[u8],
        reply: impl Fn(&[u8]) + 'static,
    ) -> Result<Self, TerminalError> {
        let terminal = Decoder::new_buf(snapshot)?.decode()?;
        let size = TerminalSize::new(terminal.cols()?, terminal.rows()?)
            .map_err(|_| libghostty_vt::Error::InvalidValue)?;
        Self::from_terminal(terminal, size, reply)
    }

    /// The complete terminal state (screen, scrollback, modes and any unfinished escape
    /// sequence) as an opaque byte string for [`TerminalEngine::from_snapshot`]. An unfinished
    /// sequence needs [`TerminalEngine::track_continuation`] to have been called before the
    /// input that left it unfinished; otherwise this fails.
    pub fn snapshot(&self) -> Result<Vec<u8>, TerminalError> {
        let bytes = self.terminal.encode_snapshot_alloc(None)?;
        Ok(bytes.map(|bytes| bytes.to_vec()).unwrap_or_default())
    }

    /// Makes [`TerminalEngine::snapshot`] work at any point in the input, not only between
    /// complete escape sequences, by retaining up to `max_bytes` of an unfinished one.
    pub fn track_continuation(&mut self, max_bytes: usize) -> Result<(), TerminalError> {
        self.terminal.set_continuation_max_bytes(max_bytes)?;
        Ok(())
    }

    fn from_terminal(
        mut terminal: Terminal<'static, 'static>,
        size: TerminalSize,
        reply: impl Fn(&[u8]) + 'static,
    ) -> Result<Self, TerminalError> {
        terminal.set_default_fg_color(Some(RgbColor {
            r: 255,
            g: 255,
            b: 255,
        }))?;
        terminal.set_default_bg_color(Some(RgbColor { r: 0, g: 0, b: 0 }))?;
        terminal.on_pty_write(move |_, bytes| reply(bytes))?;
        Ok(Self {
            terminal,
            render: RenderState::new()?,
            row_iter: RowIterator::new()?,
            cell_iter: CellIterator::new()?,
            encoder: Encoder::new()?,
            event: Event::new()?,
            size,
            full: true,
            colors: None,
        })
    }

    pub fn write(&mut self, bytes: &[u8]) {
        self.terminal.vt_write(bytes);
    }

    /// The grid size the engine publishes frames at.
    pub fn size(&self) -> TerminalSize {
        self.size
    }

    pub fn resize(&mut self, size: TerminalSize) -> Result<(), TerminalError> {
        self.terminal.resize(size.columns(), size.rows(), 0, 0)?;
        self.size = size;
        self.full = true;
        Ok(())
    }

    pub fn request_full_frame(&mut self) {
        self.full = true;
    }

    pub fn frame(&mut self) -> Result<Frame, TerminalError> {
        // DECCOLM is unsupported: Android, not remote escape sequences, owns the PTY grid.
        if self.terminal.cols()? != self.size.columns() || self.terminal.rows()? != self.size.rows()
        {
            self.terminal
                .resize(self.size.columns(), self.size.rows(), 0, 0)?;
            self.full = true;
        }
        let snapshot = self.render.update(&self.terminal)?;
        let colors = snapshot.colors()?;
        let resolved = (colors.foreground, colors.background);
        // OSC default-colour changes and reverse-screen do not dirty native rows at this pin.
        let full = self.full || snapshot.dirty()? == Dirty::Full || self.colors != Some(resolved);
        let position = if snapshot.cursor_visible()? {
            snapshot.cursor_viewport()?
        } else {
            None
        };
        let mut cursor_wide = position.is_some_and(|position| position.at_wide_tail);
        let mut rows = Vec::new();
        let mut row_iter = self.row_iter.update(&snapshot)?;
        let mut index = 0;
        while let Some(row) = row_iter.next() {
            let changed = full || row.dirty()?;
            let cursor_row = position.is_some_and(|position| position.y == index);
            if changed || cursor_row {
                let wrapped = row.raw_row()?.is_wrapped()?;
                let mut cell_iter = self.cell_iter.update(row)?;
                let mut cells = Vec::new();
                let mut column = 0;
                while let Some(cell) = cell_iter.next() {
                    let wide = cell.raw_cell()?.wide()?;
                    if position.is_some_and(|position| position.y == index && position.x == column)
                    {
                        cursor_wide = matches!(wide, CellWide::Wide | CellWide::SpacerTail);
                    }
                    if changed {
                        let raw = cell.style()?;
                        let mut foreground = rgb(cell.fg_color()?.unwrap_or(colors.foreground));
                        let mut background = rgb(cell.bg_color()?.unwrap_or(colors.background));
                        if raw.inverse {
                            std::mem::swap(&mut foreground, &mut background);
                        }
                        if raw.invisible {
                            foreground = background;
                        }
                        let underline_color = match raw.underline_color {
                            StyleColor::None => None,
                            StyleColor::Palette(index) => {
                                Some(rgb(colors.palette[usize::from(index.0)]))
                            }
                            StyleColor::Rgb(color) => Some(rgb(color)),
                        };
                        let style = CellStyle {
                            foreground,
                            background,
                            underline_color,
                            underline: match raw.underline {
                                GhosttyUnderline::None => Underline::None,
                                GhosttyUnderline::Single => Underline::Single,
                                GhosttyUnderline::Double => Underline::Double,
                                GhosttyUnderline::Curly => Underline::Curly,
                                GhosttyUnderline::Dotted => Underline::Dotted,
                                GhosttyUnderline::Dashed => Underline::Dashed,
                                _ => Underline::None,
                            },
                            bold: raw.bold,
                            italic: raw.italic,
                            faint: raw.faint,
                            strikethrough: raw.strikethrough,
                            overline: raw.overline,
                        };
                        let width = match wide {
                            CellWide::Narrow | CellWide::SpacerHead => CellWidth::Narrow,
                            CellWide::Wide => CellWidth::Wide,
                            CellWide::SpacerTail => CellWidth::SpacerTail,
                        };
                        let text = if matches!(wide, CellWide::SpacerHead | CellWide::SpacerTail) {
                            String::new()
                        } else {
                            cell.graphemes()?.into_iter().collect()
                        };
                        cells.push(Cell { text, width, style });
                    }
                    column += 1;
                }
                if changed {
                    rows.push(Row::new(index, wrapped, cells));
                }
            }
            row.set_dirty(false)?;
            index += 1;
        }
        let cursor = position
            .map(|position| -> Result<Cursor, libghostty_vt::Error> {
                Ok(Cursor {
                    column: position.x - u16::from(position.at_wide_tail),
                    row: position.y,
                    wide: cursor_wide,
                    shape: match snapshot.cursor_visual_style()? {
                        CursorVisualStyle::Bar => CursorShape::Bar,
                        CursorVisualStyle::Underline => CursorShape::Underline,
                        CursorVisualStyle::BlockHollow => CursorShape::BlockHollow,
                        _ => CursorShape::Block,
                    },
                    blinking: snapshot.cursor_blinking()?,
                    color: rgb(colors.cursor.unwrap_or(colors.foreground)),
                })
            })
            .transpose()?;
        let scrollbar = self.terminal.scrollbar()?;
        let scrollback = Scrollback {
            total_rows: scrollbar.total,
            offset: scrollbar.offset,
        };
        snapshot.set_dirty(Dirty::Clean)?;
        self.full = false;
        self.colors = Some(resolved);
        let build = if full { Frame::full } else { Frame::delta };
        Ok(build(
            self.size,
            rows,
            cursor,
            rgb(colors.background),
            scrollback,
        )?)
    }

    pub fn encode_key(&mut self, input: &KeyInput) -> Result<Vec<u8>, TerminalError> {
        self.encoder.set_options_from_terminal(&self.terminal);
        let legacy_control = self.legacy_control_key(input)?;
        let modifiers = input.modifiers();
        let mut mods = Mods::empty();
        mods.set(Mods::SHIFT, modifiers.shift);
        mods.set(Mods::CTRL, modifiers.ctrl);
        mods.set(Mods::ALT, modifiers.alt);
        mods.set(Mods::SUPER, modifiers.meta);
        let mut text = None;
        let mut unshifted = '\0';
        let mut key = match input.key() {
            Key::Enter => PhysicalKey::Enter,
            Key::Tab => PhysicalKey::Tab,
            Key::Backspace => PhysicalKey::Backspace,
            Key::Escape => PhysicalKey::Escape,
            Key::Insert => PhysicalKey::Insert,
            Key::Delete => PhysicalKey::Delete,
            Key::Home => PhysicalKey::Home,
            Key::End => PhysicalKey::End,
            Key::PageUp => PhysicalKey::PageUp,
            Key::PageDown => PhysicalKey::PageDown,
            Key::ArrowUp => PhysicalKey::ArrowUp,
            Key::ArrowDown => PhysicalKey::ArrowDown,
            Key::ArrowLeft => PhysicalKey::ArrowLeft,
            Key::ArrowRight => PhysicalKey::ArrowRight,
            Key::Function(number) => [
                PhysicalKey::F1,
                PhysicalKey::F2,
                PhysicalKey::F3,
                PhysicalKey::F4,
                PhysicalKey::F5,
                PhysicalKey::F6,
                PhysicalKey::F7,
                PhysicalKey::F8,
                PhysicalKey::F9,
                PhysicalKey::F10,
                PhysicalKey::F11,
                PhysicalKey::F12,
            ][usize::from(*number - 1)],
            Key::Character(value) => {
                text = Some(value.clone());
                if value.len() == 1 && value.is_ascii() {
                    let character = value.as_bytes()[0] as char;
                    let (key, base) = physical_key(character);
                    unshifted = base;
                    key
                } else {
                    unshifted = value.chars().next().unwrap();
                    PhysicalKey::Unidentified
                }
            }
        };
        if let Some(legacy_key) = legacy_control {
            key = legacy_key;
            mods.remove(Mods::CTRL);
            text = None;
            unshifted = '\0';
        }
        self.event
            .set_action(Action::Press)
            .set_key(key)
            .set_mods(mods)
            .set_consumed_mods(mods & Mods::SHIFT)
            .set_unshifted_codepoint(unshifted)
            .set_utf8(text);
        let mut bytes = Vec::new();
        self.encoder.encode_to_vec(&self.event, &mut bytes)?;
        Ok(bytes)
    }

    /// xterm-compatible legacy aliases. Apps opting into Kitty or modifyOtherKeys-2 keep
    /// Ghostty's fixterms encoding. This pinned API has no modifyOtherKeys getter, so a
    /// local, never-sent encoder probe reads its actual mode rather than parsing VT ourselves.
    fn legacy_control_key(
        &mut self,
        input: &KeyInput,
    ) -> Result<Option<PhysicalKey>, TerminalError> {
        if !input.modifiers().ctrl || !self.terminal.kitty_keyboard_flags()?.is_empty() {
            return Ok(None);
        }
        let key = match input.key() {
            Key::Character(text) if text == "[" => PhysicalKey::Escape,
            Key::Character(text) if text == "i" => PhysicalKey::Tab,
            Key::Character(text) if text == "m" => PhysicalKey::Enter,
            _ => return Ok(None),
        };
        self.event
            .set_action(Action::Press)
            .set_key(PhysicalKey::BracketLeft)
            .set_mods(Mods::CTRL)
            .set_consumed_mods(Mods::empty())
            .set_unshifted_codepoint('[')
            .set_utf8(Some("["));
        let mut probe = Vec::new();
        self.encoder.encode_to_vec(&self.event, &mut probe)?;
        Ok((probe == b"\x1b[91;5u").then_some(key))
    }

    /// Primary: move the viewport. Alternate: navigation keys, respecting application mode.
    pub fn scroll(&mut self, scroll: ViewportScroll) -> Result<Vec<u8>, TerminalError> {
        if self.terminal.active_screen()? == Screen::Primary {
            self.terminal.scroll_viewport(match scroll {
                ViewportScroll::Top => ScrollViewport::Top,
                ViewportScroll::Bottom => ScrollViewport::Bottom,
                ViewportScroll::Delta(rows) => ScrollViewport::Delta(rows as isize),
            });
            return Ok(Vec::new());
        }
        let (key, count) = match scroll {
            ViewportScroll::Top => (Key::Home, 1),
            ViewportScroll::Bottom => (Key::End, 1),
            ViewportScroll::Delta(rows) if rows < 0 => (Key::ArrowUp, rows.unsigned_abs()),
            ViewportScroll::Delta(rows) => (Key::ArrowDown, rows as u32),
        };
        let bytes =
            self.encode_key(&KeyInput::new(key, Modifiers::default()).expect("navigation key"))?;
        // A single gesture cannot sensibly navigate more than one viewport at a time.
        Ok(bytes.repeat(count.min(u32::from(self.size.rows())) as usize))
    }
}

fn rgb(color: RgbColor) -> Rgb {
    Rgb::new(color.r, color.g, color.b)
}

fn physical_key(character: char) -> (PhysicalKey, char) {
    use PhysicalKey::*;
    if character.is_ascii_alphabetic() {
        let lower = character.to_ascii_lowercase();
        let key = [
            A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
        ][lower as usize - 'a' as usize];
        return (key, lower);
    }
    for (base, shifted, key) in [
        ('0', ')', Digit0),
        ('1', '!', Digit1),
        ('2', '@', Digit2),
        ('3', '#', Digit3),
        ('4', '$', Digit4),
        ('5', '%', Digit5),
        ('6', '^', Digit6),
        ('7', '&', Digit7),
        ('8', '*', Digit8),
        ('9', '(', Digit9),
        ('`', '~', Backquote),
        ('-', '_', Minus),
        ('=', '+', Equal),
        ('[', '{', BracketLeft),
        (']', '}', BracketRight),
        ('\\', '|', Backslash),
        (';', ':', Semicolon),
        ('\'', '"', Quote),
        (',', '<', Comma),
        ('.', '>', Period),
        ('/', '?', Slash),
        (' ', ' ', Space),
    ] {
        if character == base || character == shifted {
            return (key, base);
        }
    }
    (Unidentified, character)
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;
