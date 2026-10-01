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
}
