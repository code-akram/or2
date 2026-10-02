//! [`Screen`] over or2's libghostty [`TerminalEngine`], so terminal state stays in libghostty and
//! frames reach Kotlin exactly as for SSH.
//!
//! mosh's server renders its own screen to escape bytes (`Display::new_frame`); fed into
//! libghostty they reproduce that screen, scrollback included (the diff scrolls with newlines,
//! as it would on a real terminal). The state copies the protocol needs are libghostty terminal
//! snapshots, so a copy is an encode of the live terminal and a restore is a decode.

use crate::term::TerminalSize;
use crate::terminal::{TerminalEngine, TerminalError};

use super::ssp::screen::{Screen, ScreenError};

/// Retained bytes of an unfinished escape sequence, so a snapshot never fails on one. The
/// server's diffs end between sequences, so this is a safety margin, not a working buffer.
const CONTINUATION_BYTES: usize = 4096;

/// A mosh state, held in a libghostty terminal.
pub struct GhosttyScreen {
    engine: TerminalEngine,
}

impl GhosttyScreen {
    /// A blank screen. Replies the terminal would send to the host are discarded: mosh's server
    /// has its own emulator, which already answered the program's queries, and its diffs
    /// contain none.
    pub fn new(size: TerminalSize) -> Result<Self, ScreenError> {
        Self::wrap(TerminalEngine::new(size, |_| {}))
    }

    fn wrap(engine: Result<TerminalEngine, TerminalError>) -> Result<Self, ScreenError> {
        let mut engine = engine.map_err(error)?;
        engine
            .track_continuation(CONTINUATION_BYTES)
            .map_err(error)?;
        Ok(Self { engine })
    }

    /// The engine, for frames, key encoding and scrolling.
    pub fn engine(&mut self) -> &mut TerminalEngine {
        &mut self.engine
    }
}

fn error(error: TerminalError) -> ScreenError {
    ScreenError(error.to_string())
}

impl Screen for GhosttyScreen {
    type Snapshot = Vec<u8>;

    fn feed(&mut self, bytes: &[u8]) {
        self.engine.write(bytes);
    }

    fn resize(&mut self, rows: u16, cols: u16) -> Result<(), ScreenError> {
        let size = TerminalSize::new(cols, rows)
            .map_err(|_| ScreenError("a terminal needs at least one row and column".into()))?;
        self.engine.resize(size).map_err(error)
    }

    fn rows(&self) -> u16 {
        self.engine.size().rows()
    }

    fn cols(&self) -> u16 {
        self.engine.size().columns()
    }

    fn snapshot(&self) -> Result<Vec<u8>, ScreenError> {
        self.engine.snapshot().map_err(error)
    }

    fn restore(snapshot: &Vec<u8>) -> Result<Self, ScreenError> {
        Self::wrap(TerminalEngine::from_snapshot(snapshot, |_| {}))
    }

    /// A snapshot restores to the bottom of the scrollback; the reader who had scrolled back
    /// stays where they were.
    fn adopt_view_of(&mut self, replaced: &Self) -> Result<(), ScreenError> {
        let offset = replaced.engine.viewport_offset().map_err(error)?;
        if offset.is_some() {
            self.engine.set_viewport_offset(offset).map_err(error)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::frame::Frame;

    use super::super::ssp::terminal::ClientTerminal;
    use super::*;

    fn size(columns: u16, rows: u16) -> TerminalSize {
        TerminalSize::new(columns, rows).unwrap()
    }

    /// The text of every row of a full frame, trailing blanks trimmed.
    fn text(screen: &mut GhosttyScreen) -> Vec<String> {
        let frame = screen.engine().frame().unwrap();
        rows_of(&frame)
    }

    fn rows_of(frame: &Frame) -> Vec<String> {
        frame
            .rows()
            .iter()
            .map(|row| {
                row.cells()
                    .iter()
                    .map(|cell| cell.text.as_str())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn fed_bytes_reach_the_frame() {
        let mut screen = GhosttyScreen::new(size(20, 3)).unwrap();
        screen.feed(b"hello\r\n\x1b[31mred\x1b[0m");
        assert_eq!(text(&mut screen), ["hello", "red", ""]);
    }

    #[test]
    fn a_restored_snapshot_equals_the_screen_it_was_taken_from() {
        let mut screen = GhosttyScreen::new(size(20, 3)).unwrap();
        screen.feed(b"one\r\ntwo\x1b[2;1H\x1b[1mbold");
        let snapshot = screen.snapshot().unwrap();
        // The original keeps going; the copy is unaffected.
        screen.feed(b"\x1b[3;1Hthree");
        let mut copy = GhosttyScreen::restore(&snapshot).unwrap();
        assert_eq!(text(&mut copy), ["one", "bold", ""]);
        assert_eq!((copy.rows(), copy.cols()), (3, 20));
        assert_eq!(text(&mut screen), ["one", "bold", "three"]);
        // Styles survive too: bold is part of the cell, not just the text.
        copy.engine().request_full_frame();
        let frame = copy.engine().frame().unwrap();
        assert!(frame.rows()[1].cells()[0].style.bold);
    }

    #[test]
    fn a_snapshot_is_valid_in_the_middle_of_an_escape_sequence() {
        let mut screen = GhosttyScreen::new(size(20, 3)).unwrap();
        // Half of a CSI.
        screen.feed(b"ab\x1b[3");
        let mut copy = GhosttyScreen::restore(&screen.snapshot().unwrap()).unwrap();
        copy.feed(b"1mcd");
        copy.engine().request_full_frame();
        let frame = copy.engine().frame().unwrap();
        assert_eq!(rows_of(&frame), ["abcd", "", ""]);
        assert!(!frame.rows()[0].cells()[2].style.bold);
        // The 31 completed to a red foreground, not to printable "1m".
        assert_ne!(
            frame.rows()[0].cells()[2].style.foreground,
            frame.rows()[0].cells()[0].style.foreground
        );

        // Half of a UTF-8 character.
        let mut screen = GhosttyScreen::new(size(20, 3)).unwrap();
        screen.feed(b"ab");
        screen.feed(&"é".as_bytes()[..1]);
        let mut copy = GhosttyScreen::restore(&screen.snapshot().unwrap()).unwrap();
        copy.feed(&"é".as_bytes()[1..]);
        copy.feed(b"cd");
        assert_eq!(text(&mut copy), ["abécd", "", ""]);
    }

    #[test]
    fn resize_changes_the_grid_the_frames_report() {
        let mut screen = GhosttyScreen::new(size(20, 3)).unwrap();
        screen.resize(5, 30).unwrap();
        assert_eq!((screen.rows(), screen.cols()), (5, 30));
        let frame = screen.engine().frame().unwrap();
        assert_eq!(frame.size(), size(30, 5));
        assert!(screen.resize(0, 30).is_err());
    }

    #[test]
    fn scrollback_survives_a_snapshot() {
        let mut screen = GhosttyScreen::new(size(20, 3)).unwrap();
        for line in 0..12 {
            screen.feed(format!("line {line}\r\n").as_bytes());
        }
        let before = screen.engine().frame().unwrap().scrollback();
        assert!(before.total_rows > 3);
        let mut copy = GhosttyScreen::restore(&screen.snapshot().unwrap()).unwrap();
        assert_eq!(copy.engine().frame().unwrap().scrollback(), before);
    }

    #[test]
    fn the_scrollback_a_snapshot_carries_is_bounded() {
        // A paced stream (a log tail) leaves history in libghostty for as long as it runs. The
        // library caps it, which is what bounds the cost of a snapshot per received state:
        // about 70 KB and 0.2 ms at the cap on the development machine (release build).
        let mut screen = GhosttyScreen::new(size(80, 24)).unwrap();
        for line in 0..30_000 {
            screen.feed(
                format!("line {line} some text to fill the width of the row a little\r\n")
                    .as_bytes(),
            );
        }
        let snapshot = screen.snapshot().unwrap();
        assert!(
            snapshot.len() < 1 << 20,
            "a snapshot of an endless stream is {} bytes",
            snapshot.len()
        );
        let total = screen.engine().frame().unwrap().scrollback().total_rows;
        assert!(total > 24, "history exists: {total}");
        assert!(total < 20_000, "history is capped: {total}");
    }

    #[test]
    fn replacing_the_live_screen_keeps_where_the_reader_scrolled_to() {
        use crate::input::ViewportScroll;

        let mut terminal = ClientTerminal::new(GhosttyScreen::new(size(20, 3)).unwrap());
        let history: String = (0..50).map(|line| format!("line {line}\r\n")).collect();
        terminal.apply_diff(0, 1, history.as_bytes(), 0).unwrap();
        terminal.apply_diff(1, 2, b"more\r\n", 0).unwrap();
        // The reader scrolls back into history on the live screen.
        terminal
            .live()
            .engine()
            .scroll(ViewportScroll::Delta(-10))
            .unwrap();
        let scrolled = terminal.live().engine().viewport_offset().unwrap();
        assert!(scrolled.is_some());
        let before = terminal.live().engine().frame().unwrap().scrollback();

        // The server diffs from the older state 1 (our ack for 2 was late): the live screen is
        // rebuilt from a snapshot, which on its own would drop the viewport to the bottom.
        assert!(terminal.apply_diff(1, 3, b"other\r\n", 0).unwrap());
        assert_eq!(terminal.latest(), 3);
        assert_eq!(
            terminal.live().engine().viewport_offset().unwrap(),
            scrolled
        );
        let after = terminal.live().engine().frame().unwrap().scrollback();
        assert_eq!(after.offset, before.offset);

        // A reader at the bottom stays there.
        terminal
            .live()
            .engine()
            .scroll(ViewportScroll::Bottom)
            .unwrap();
        assert!(terminal.apply_diff(3, 4, b"x", 0).unwrap());
        assert!(terminal.apply_diff(3, 5, b"y", 0).unwrap());
        assert_eq!(terminal.live().engine().viewport_offset().unwrap(), None);
    }

    #[test]
    fn the_client_terminal_applies_a_diff_to_the_state_it_names() {
        let mut terminal = ClientTerminal::new(GhosttyScreen::new(size(20, 3)).unwrap());
        assert!(terminal.apply_diff(0, 1, b"hello", 0).unwrap());
        // The server never heard our ack for state 1 and diffs from state 0 again.
        assert!(terminal.apply_diff(0, 2, b"hello world", 0).unwrap());
        assert_eq!(text(terminal.live()), ["hello world", "", ""]);
        // And an older state is still reconstructible from its snapshot.
        assert!(terminal.apply_diff(1, 3, b"!", 0).unwrap());
        assert_eq!(text(terminal.live()), ["hello!", "", ""]);
    }

    #[test]
    fn in_place_diffs_keep_frames_incremental() {
        let mut terminal = ClientTerminal::new(GhosttyScreen::new(size(20, 3)).unwrap());
        terminal
            .apply_diff(0, 1, b"one\r\ntwo\r\nthree", 0)
            .unwrap();
        let first = terminal.live().engine().frame().unwrap();
        assert!(first.is_full());
        terminal.apply_diff(1, 2, b"\x1b[2;1Hxyz", 0).unwrap();
        let delta = terminal.live().engine().frame().unwrap();
        assert!(!delta.is_full(), "the live engine keeps its dirty tracking");
        let row = delta.rows().iter().find(|row| row.index() == 1).unwrap();
        assert_eq!(
            row.cells()
                .iter()
                .map(|c| c.text.as_str())
                .collect::<String>()
                .trim_end(),
            "xyz"
        );
    }

    /// mosh's server relays the program's mouse modes in its diffs (it never relays the
    /// alternate screen, which its own emulator keeps): frames report them as over SSH, they
    /// survive a state replacement, and a swipe becomes wheel events.
    #[test]
    fn mouse_modes_from_the_server_reach_frames_and_wheel_scrolls() {
        use crate::frame::TerminalModes;
        use crate::input::ViewportScroll;
        let mut terminal = ClientTerminal::new(GhosttyScreen::new(size(20, 3)).unwrap());
        terminal
            .apply_diff(0, 1, b"\x1b[?1002h\x1b[?1006hvim", 0)
            .unwrap();
        let modes = terminal.live().engine().frame().unwrap().modes();
        assert_eq!(
            modes,
            TerminalModes {
                mouse_tracking: true,
                alternate_screen: false
            }
        );
        terminal.apply_diff(1, 2, b"\x1b[1;1Hvi", 0).unwrap();
        assert_eq!(terminal.live().engine().frame().unwrap().modes(), modes);
        let wheel = ViewportScroll::Wheel {
            rows: -1,
            column: 3,
            row: 1,
        };
        assert_eq!(
            terminal.live().engine().scroll(wheel).unwrap(),
            b"\x1b[<64;4;2M"
        );
        // A tap is a left click there, as over SSH.
        assert_eq!(
            terminal.live().engine().mouse_click(3, 1).unwrap(),
            b"\x1b[<0;4;2M\x1b[<0;4;2m"
        );
        terminal.apply_diff(2, 3, b"\x1b[?1002l", 0).unwrap();
        assert!(
            !terminal
                .live()
                .engine()
                .frame()
                .unwrap()
                .modes()
                .mouse_tracking
        );
        assert!(
            terminal
                .live()
                .engine()
                .mouse_click(3, 1)
                .unwrap()
                .is_empty()
        );
    }
}
