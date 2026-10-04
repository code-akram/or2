//! libghostty-vt adapter. Every object stays on its creating session thread; only owned
//! frames and encoded byte vectors cross thread boundaries.

use libghostty_vt::key::{Action, Encoder, Event, Key as PhysicalKey, Mods};
use libghostty_vt::mouse::{
    Action as MouseAction, Button as MouseButton, Encoder as MouseEncoder, EncoderSize,
    Event as MouseEvent, Position,
};
use libghostty_vt::render::{CellIterator, CursorVisualStyle, Dirty, RowIterator};
use libghostty_vt::screen::{CellWide, Screen};
use libghostty_vt::snapshot::Decoder;
use libghostty_vt::style::{
    Palette, PaletteIndex, RgbColor, StyleColor, Underline as GhosttyUnderline,
};
use libghostty_vt::terminal::{
    ClipboardWrite, ClipboardWriteError, Mode, Point, PointCoordinate, ScrollViewport,
};
use libghostty_vt::{RenderState, Terminal};

use std::cell::RefCell;
use std::rc::Rc;

use crate::frame::{
    Cell, CellLink, CellStyle, CellWidth, Cursor, CursorShape, Frame, Rgb, Row, Scrollback,
    TerminalModes, Underline,
};
use crate::input::{Key, KeyInput, Modifiers, ViewportScroll};
use crate::session::Command;
use crate::term::TerminalSize;

/// Default terminal colours: Tokyo Night (Apache-2.0, folke/tokyonight.nvim `extras/ghostty/
/// tokyonight_night`), the theme herdr uses on the owner's machines, so a herdr pane's content and
/// herdr's own chrome agree. The app UI around the terminal stays Catppuccin Mocha. A remote program
/// can still change any of them (OSC 4, 10 and 11) and reset back to these.
pub const DEFAULT_FOREGROUND: RgbColor = RgbColor {
    r: 0xc0,
    g: 0xca,
    b: 0xf5,
};
pub const DEFAULT_BACKGROUND: RgbColor = RgbColor {
    r: 0x1a,
    g: 0x1b,
    b: 0x26,
};

/// The Tokyo Night ANSI colours for palette indices 0 to 15, as its Ghostty theme sets them.
/// Indices 16 and up keep libghostty's xterm cube and grey ramp. The cursor follows the
/// foreground, as in that theme.
const TOKYO_NIGHT_ANSI: [(PaletteIndex, u32); 16] = [
    (PaletteIndex::BLACK, 0x15161e),
    (PaletteIndex::RED, 0xf7768e),
    (PaletteIndex::GREEN, 0x9ece6a),
    (PaletteIndex::YELLOW, 0xe0af68),
    (PaletteIndex::BLUE, 0x7aa2f7),
    (PaletteIndex::MAGENTA, 0xbb9af7),
    (PaletteIndex::CYAN, 0x7dcfff),
    (PaletteIndex::WHITE, 0xa9b1d6),
    (PaletteIndex::BRIGHT_BLACK, 0x414868),
    (PaletteIndex::BRIGHT_RED, 0xff899d),
    (PaletteIndex::BRIGHT_GREEN, 0x9fe044),
    (PaletteIndex::BRIGHT_YELLOW, 0xfaba4a),
    (PaletteIndex::BRIGHT_BLUE, 0x8db0ff),
    (PaletteIndex::BRIGHT_MAGENTA, 0xc7a9ff),
    (PaletteIndex::BRIGHT_CYAN, 0xa4daff),
    (PaletteIndex::BRIGHT_WHITE, 0xc0caf5),
];

fn tokyo_night_palette(base: Palette) -> Palette {
    let mut palette = base;
    for (index, rgb) in TOKYO_NIGHT_ANSI {
        palette.set(
            index,
            RgbColor {
                r: (rgb >> 16) as u8,
                g: (rgb >> 8) as u8,
                b: rgb as u8,
            },
        );
    }
    palette
}

/// The largest clipboard write from the host (OSC 52, OSC 1337 Copy) passed on, in bytes of
/// decoded text; a larger one is dropped whole.
pub const MAX_CLIPBOARD_BYTES: usize = 1 << 20;

/// The text of a host's clipboard write, or `None` to drop it: a request to clear the
/// clipboard (no representation), no text representation, text that is not UTF-8, or text
/// over [`MAX_CLIPBOARD_BYTES`]. Every destination (clipboard, selection, primary) counts:
/// the phone has one clipboard.
fn clipboard_text(write: &ClipboardWrite<'_>) -> Option<String> {
    let mut contents = write.contents();
    let text = contents
        .clone()
        .find(|content| content.mime.starts_with("text/plain"))
        .or_else(|| contents.find(|content| content.mime.starts_with("text/")))?;
    if text.data.len() > MAX_CLIPBOARD_BYTES {
        return None;
    }
    String::from_utf8(text.data.to_vec()).ok()
}

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
    mouse: MouseEncoder<'static>,
    mouse_event: MouseEvent<'static>,
    size: TerminalSize,
    full: bool,
    colors: Option<(RgbColor, RgbColor)>,
    presentation: Option<Presentation>,
    /// The newest clipboard write from the host not yet taken ([`TerminalEngine::take_clipboard_write`]).
    clipboard: Rc<RefCell<Option<String>>>,
    /// Scratch space for hyperlink URIs while building a frame.
    uri: Vec<u8>,
}

/// Metadata can change without dirty rows (mouse modes, cursor, OSC colours, history).
#[derive(Clone, Copy, PartialEq, Eq)]
struct Presentation {
    cursor: Option<Cursor>,
    background: Rgb,
    scrollback: Scrollback,
    modes: TerminalModes,
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
        terminal.set_default_fg_color(Some(DEFAULT_FOREGROUND))?;
        terminal.set_default_bg_color(Some(DEFAULT_BACKGROUND))?;
        let base = terminal.default_color_palette()?;
        terminal.set_default_color_palette(Some(tokyo_night_palette(base)))?;
        terminal.on_pty_write(move |_, bytes| reply(bytes))?;
        let clipboard = Rc::new(RefCell::new(None));
        let written = clipboard.clone();
        // libghostty decodes OSC 52's base64 (dropping an invalid payload) and never forwards a
        // read request ("?"), so the host never learns the phone's clipboard.
        terminal.on_clipboard_write(move |_, write| {
            if let Some(text) = clipboard_text(&write) {
                *written.borrow_mut() = Some(text);
                Ok(())
            } else {
                Err(ClipboardWriteError::Unsupported)
            }
        })?;
        Ok(Self {
            terminal,
            render: RenderState::new()?,
            row_iter: RowIterator::new()?,
            cell_iter: CellIterator::new()?,
            encoder: Encoder::new()?,
            event: Event::new()?,
            mouse: MouseEncoder::new()?,
            mouse_event: MouseEvent::new()?,
            size,
            full: true,
            colors: None,
            presentation: None,
            clipboard,
            uri: Vec::new(),
        })
    }

    pub fn write(&mut self, bytes: &[u8]) {
        self.terminal.vt_write(bytes);
    }

    /// The newest clipboard write the host made (OSC 52 or OSC 1337 Copy) since the last call,
    /// as text: earlier ones in between are superseded. See [`MAX_CLIPBOARD_BYTES`] for what is
    /// dropped. A snapshot does not carry it.
    pub fn take_clipboard_write(&mut self) -> Option<String> {
        self.clipboard.borrow_mut().take()
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

    /// Where the viewport is scrolled to, as a row offset from the top of the scrollback, or
    /// `None` while it follows the active area. A snapshot does not record it.
    pub fn viewport_offset(&self) -> Result<Option<usize>, TerminalError> {
        if self.terminal.viewport_active()? {
            return Ok(None);
        }
        Ok(Some(self.terminal.scrollbar()?.offset as usize))
    }

    /// Puts the viewport where [`TerminalEngine::viewport_offset`] reported it, `None` being
    /// the bottom. The next frame is a full one.
    pub fn set_viewport_offset(&mut self, offset: Option<usize>) -> Result<(), TerminalError> {
        self.terminal.scroll_viewport(match offset {
            Some(row) => ScrollViewport::Row(row),
            None => ScrollViewport::Bottom,
        });
        self.full = true;
        Ok(())
    }

    pub fn request_full_frame(&mut self) {
        self.full = true;
    }

    /// Consume engine dirtiness, publishing only a local presentation change. In particular,
    /// encoding wheel input alone does not change a frame. Navigation-key fallback can return
    /// the viewport to the bottom, so it must be checked in exactly the same way.
    pub fn frame_if_changed(&mut self) -> Result<Option<Frame>, TerminalError> {
        let previous = self.presentation;
        let frame = self.frame()?;
        Ok(
            (frame.is_full() || !frame.rows().is_empty() || previous != self.presentation)
                .then_some(frame),
        )
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
        let mut rows = Vec::with_capacity(if full {
            usize::from(self.size.rows())
        } else {
            0
        });
        let mut row_iter = self.row_iter.update(&snapshot)?;
        let mut index = 0;
        while let Some(row) = row_iter.next() {
            let changed = full || row.dirty()?;
            let cursor_row = position.is_some_and(|position| position.y == index);
            if changed || cursor_row {
                let raw_row = row.raw_row()?;
                let wrapped = raw_row.is_wrapped()?;
                // May be a false positive; cells are checked one by one.
                let linked = changed && raw_row.has_hyperlink()?;
                let mut links: Vec<CellLink> = Vec::new();
                let mut cell_iter = self.cell_iter.update(row)?;
                let mut cells = Vec::with_capacity(if changed {
                    usize::from(self.size.columns())
                } else {
                    0
                });
                let mut column = 0;
                while let Some(cell) = cell_iter.next() {
                    let raw_cell = cell.raw_cell()?;
                    let wide = raw_cell.wide()?;
                    if linked {
                        let open = links
                            .last_mut()
                            .filter(|link| link.end_column + 1 == column);
                        if matches!(wide, CellWide::SpacerTail) {
                            // A wide character's tail belongs to its head's link.
                            if let Some(link) = open {
                                link.end_column = column;
                            }
                        } else if raw_cell.has_hyperlink()?
                            && let Some(uri) =
                                hyperlink_uri(&self.terminal, &mut self.uri, column, index)?
                        {
                            match open {
                                Some(link) if link.uri == uri => link.end_column = column,
                                _ => links.push(CellLink {
                                    start_column: column,
                                    end_column: column,
                                    uri,
                                }),
                            }
                        }
                    }
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
                    rows.push(Row::new(index, wrapped, cells).with_links(links));
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
        let modes = self.modes()?;
        self.presentation = Some(Presentation {
            cursor,
            background: rgb(colors.background),
            scrollback,
            modes,
        });
        let build = if full { Frame::full } else { Frame::delta };
        Ok(build(self.size, rows, cursor, rgb(colors.background), scrollback)?.with_modes(modes))
    }

    /// The modes a swipe is routed by (whether the program tracks the mouse and which screen
    /// is active) and whether a multi-line send needs confirming (bracketed paste).
    pub fn modes(&self) -> Result<TerminalModes, TerminalError> {
        Ok(TerminalModes {
            mouse_tracking: self.terminal.is_mouse_tracking()?,
            alternate_screen: self.terminal.active_screen()? == Screen::Alternate,
            bracketed_paste: self.bracketed_paste()?,
        })
    }

    /// Whether the program has bracketed paste (DECSET 2004) on.
    pub fn bracketed_paste(&self) -> Result<bool, TerminalError> {
        Ok(self.terminal.mode(Mode::BRACKETED_PASTE)?)
    }

    /// The first write of a submit ([`crate::submit`]) under the terminal's current modes.
    pub fn submit_text_bytes(&self, text: &str) -> Result<Vec<u8>, TerminalError> {
        Ok(crate::submit::submit_text_bytes(
            text,
            self.bracketed_paste()?,
        ))
    }

    /// The Enter that ends a submit, encoded like any other key.
    pub fn submit_enter_bytes(&mut self) -> Result<Vec<u8>, TerminalError> {
        self.encode_key(&crate::submit::enter_key())
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

    /// Primary: move the viewport. Alternate: navigation keys, respecting application mode. A
    /// [`ViewportScroll::Wheel`] while the program tracks the mouse is wheel events instead;
    /// without tracking (or in a mode that reports no wheel) it is a `Delta` of its rows.
    pub fn scroll(&mut self, scroll: ViewportScroll) -> Result<Vec<u8>, TerminalError> {
        let scroll = match scroll {
            ViewportScroll::Wheel { rows, column, row } => {
                let bytes = self.wheel(rows, column, row)?;
                if !bytes.is_empty() || rows == 0 {
                    return Ok(bytes);
                }
                ViewportScroll::Delta(rows)
            }
            other => other,
        };
        if self.terminal.active_screen()? == Screen::Primary {
            self.terminal.scroll_viewport(match scroll {
                ViewportScroll::Bottom => ScrollViewport::Bottom,
                ViewportScroll::Delta(rows) | ViewportScroll::Wheel { rows, .. } => {
                    ScrollViewport::Delta(rows as isize)
                }
            });
            return Ok(Vec::new());
        }
        let (key, count) = match scroll {
            ViewportScroll::Bottom => (Key::End, 1),
            ViewportScroll::Delta(rows) | ViewportScroll::Wheel { rows, .. } if rows < 0 => {
                (Key::ArrowUp, rows.unsigned_abs())
            }
            ViewportScroll::Delta(rows) | ViewportScroll::Wheel { rows, .. } => {
                (Key::ArrowDown, rows as u32)
            }
        };
        let bytes =
            self.encode_key(&KeyInput::new(key, Modifiers::default()).expect("navigation key"))?;
        // A single gesture cannot sensibly navigate more than one viewport at a time.
        Ok(bytes.repeat(count.min(u32::from(self.size.rows())) as usize))
    }

    /// `rows` wheel presses (button 4 up, 5 down) at the cell (`column`, `row`, clamped to the
    /// grid), in the terminal's mouse tracking mode and format; empty when it tracks nothing or
    /// its mode does not report the wheel (X10, DECSET 9). At most one viewport of events.
    fn wheel(&mut self, rows: i32, column: u16, row: u16) -> Result<Vec<u8>, TerminalError> {
        if rows == 0 || !self.terminal.is_mouse_tracking()? {
            return Ok(Vec::new());
        }
        let button = if rows < 0 {
            MouseButton::Four
        } else {
            MouseButton::Five
        };
        let one = self.mouse_bytes(MouseAction::Press, button, column, row)?;
        let count = rows.unsigned_abs().min(u32::from(self.size.rows()));
        Ok(one.repeat(count as usize))
    }

    /// What an input command writes to the program, encoded with the terminal's modes (the one
    /// encoding of the SSH pump and the mosh driver): typed text, a submit's text (its Enter is
    /// the caller's, later) or a paste, a key, a scroll's navigation keys or wheel events, a
    /// click. Possibly empty. `None` for a command that writes nothing (resize, full frame, roam,
    /// disconnect).
    pub fn input_bytes(&mut self, command: &Command) -> Result<Option<Vec<u8>>, TerminalError> {
        Ok(Some(match command {
            Command::Text(text) => crate::input::text_bytes(text),
            Command::Submit(text) | Command::Paste(text) => self.submit_text_bytes(text)?,
            Command::Key(key) => self.encode_key(key)?,
            Command::Scroll(scroll) => self.scroll(*scroll)?,
            Command::MouseClick { column, row } => self.mouse_click(*column, *row)?,
            Command::Resize(_) | Command::FullFrame | Command::Roam | Command::Disconnect => {
                return Ok(None);
            }
        }))
    }

    /// A tap while the program tracks the mouse: a left-button press, then its release, at the
    /// cell (`column`, `row`, clamped to the grid), in the terminal's mouse tracking mode and
    /// format (a mode that reports no releases, X10 tracking, gets the press alone). Empty
    /// when the program tracks nothing.
    pub fn mouse_click(&mut self, column: u16, row: u16) -> Result<Vec<u8>, TerminalError> {
        if !self.terminal.is_mouse_tracking()? {
            return Ok(Vec::new());
        }
        let mut bytes = self.mouse_bytes(MouseAction::Press, MouseButton::Left, column, row)?;
        bytes.extend(self.mouse_bytes(MouseAction::Release, MouseButton::Left, column, row)?);
        Ok(bytes)
    }

    /// One mouse event at the centre of the cell (`column`, `row`, clamped to the grid), encoded
    /// with the terminal's current tracking mode and format.
    fn mouse_bytes(
        &mut self,
        action: MouseAction,
        button: MouseButton,
        column: u16,
        row: u16,
    ) -> Result<Vec<u8>, TerminalError> {
        // The encoder maps surface pixels to cells; a nominal cell size places the event at
        // the centre of the touched cell whatever the phone's real metrics are.
        const CELL: u32 = 10;
        let columns = self.size.columns();
        let grid_rows = self.size.rows();
        self.mouse
            .set_options_from_terminal(&self.terminal)
            .set_size(EncoderSize {
                screen_width: u32::from(columns) * CELL,
                screen_height: u32::from(grid_rows) * CELL,
                cell_width: CELL,
                cell_height: CELL,
                padding_top: 0,
                padding_bottom: 0,
                padding_right: 0,
                padding_left: 0,
            })
            .set_any_button_pressed(false)
            .set_track_last_cell(false);
        let centre =
            |cell: u16, cells: u16| (u32::from(cell.min(cells - 1)) * CELL + CELL / 2) as f32;
        self.mouse_event
            .set_action(action)
            .set_button(Some(button))
            .set_mods(Mods::empty())
            .set_position(Position {
                x: centre(column, columns),
                y: centre(row, grid_rows),
            });
        let mut bytes = Vec::new();
        self.mouse.encode_to_vec(&self.mouse_event, &mut bytes)?;
        Ok(bytes)
    }
}

/// The OSC 8 URI of the viewport cell at `column`, `row`, or `None` when it has none (or it
/// is not UTF-8). `buffer` is reused across calls and grows to the longest URI met.
fn hyperlink_uri(
    terminal: &Terminal<'static, 'static>,
    buffer: &mut Vec<u8>,
    column: u16,
    row: u16,
) -> Result<Option<String>, libghostty_vt::Error> {
    let cell = terminal.grid_ref(Point::Viewport(PointCoordinate {
        x: column,
        y: u32::from(row),
    }))?;
    if buffer.is_empty() {
        buffer.resize(256, 0);
    }
    let length = match cell.hyperlink_uri(buffer) {
        Err(libghostty_vt::Error::OutOfSpace { required }) if required > buffer.len() => {
            buffer.resize(required, 0);
            cell.hyperlink_uri(buffer)?
        }
        result => result?,
    };
    if length == 0 {
        return Ok(None);
    }
    Ok(std::str::from_utf8(&buffer[..length])
        .ok()
        .map(str::to_owned))
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
