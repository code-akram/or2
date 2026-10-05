//! Differential rendering oracle: coalesced moved rows must equal a fresh engine snapshot.
use or2_core::frame::{Frame, FrameMailbox, Row};
use or2_core::input::ViewportScroll;
use or2_core::term::TerminalSize;
use or2_core::terminal::TerminalEngine;

fn apply(grid: &mut Vec<Row>, frame: &Frame) {
    if frame.is_full() {
        *grid = frame.rows().to_vec();
        return;
    }
    let old = grid.clone();
    for moved in frame.row_moves() {
        let source = &old[usize::from(moved.previous)];
        grid[usize::from(moved.index)] =
            Row::new(moved.index, source.wrapped(), source.cells().to_vec())
                .with_links(source.links().to_vec());
    }
    for row in frame.rows() {
        grid[usize::from(row.index())] = row.clone();
    }
}

#[test]
fn mixed_scroll_regions_edits_viewports_and_slow_consumers_match_full_snapshots() {
    let mut size = TerminalSize::new(56, 23).unwrap();
    let mut engine = TerminalEngine::new(size, |_| {}).unwrap();
    let mut oracle = TerminalEngine::new(size, |_| {}).unwrap();
    let mut mailbox = FrameMailbox::default();
    let mut grid = Vec::new();
    let mut random = 0x13a4_5e9fu32;
    let mut moved_rows = 0;
    let mut full_frames = 0;
    // Drain the final batch too, including resizes/full requests coalesced with later deltas.
    for step in 0..=340 {
        random ^= random << 13;
        random ^= random >> 17;
        random ^= random << 5;
        if step % 47 == 0 {
            let (columns, rows) = [(56, 23), (64, 27), (42, 24)][step / 47 % 3];
            size = TerminalSize::new(columns, rows).unwrap();
            engine.resize(size).unwrap();
            oracle.resize(size).unwrap();
        }
        if step % 53 == 0 {
            engine.request_full_frame();
        }
        let row = random % u32::from(size.rows()) + 1;
        let bytes = match step % 19 {
            0..=4 => format!(
                "\r\n\x1b[{}m{step:08} MiW 界😀e\u{301}\x1b[0m",
                30 + random % 8
            ),
            5 => format!("\x1b[{row};1H\x1b[2Kedit {step}"),
            6 => "\x1b[3;20r\x1b[20;1H\n\x1b[r".into(),
            7 => "\x1b[4;19r\x1b[4;1H\x1bM\x1b[r".into(),
            8 => format!("\x1b[{row};1H\x1b[2L"),
            9 => format!("\x1b[{row};1H\x1b[2M"),
            10 => "\x1b[2S".into(),
            11 => "\x1b[2T".into(),
            12 => {
                format!("\x1b[{row};1H\x1b]8;;https://example.org/{step}\x1b\\linked\x1b]8;;\x1b\\")
            }
            13 => "\x1b[?1049h\x1b[Halternate 界\x1b[?1049l".into(),
            14 => format!("\x1b[{row};1H\x1b[1;3;4;9mstyled\x1b[0m"),
            15 => "\x1b[?1000h\x1b[?2004h".into(),
            16 => "\x1b[?1000l\x1b[?2004l".into(),
            17 => format!("\x1b]11;#{:06x}\x1b\\", random & 0xffffff),
            _ => "\r\n".into(),
        };
        engine.write(bytes.as_bytes());
        oracle.write(bytes.as_bytes());
        if step % 31 == 0 {
            let scroll = if step % 62 == 0 {
                ViewportScroll::Delta(-3)
            } else {
                ViewportScroll::Bottom
            };
            engine.scroll(scroll).unwrap();
            oracle.scroll(scroll).unwrap();
        }
        mailbox.publish(engine.frame().unwrap()).unwrap();
        if step % 5 == 0 {
            let taken = mailbox.take().unwrap();
            moved_rows += taken.frame.row_moves().len();
            full_frames += usize::from(taken.frame.is_full());
            apply(&mut grid, &taken.frame);
            oracle.request_full_frame();
            let expected = oracle.frame().unwrap();
            assert_eq!(grid, expected.rows(), "row content at step {step}");
            assert_eq!(taken.frame.size(), expected.size(), "size at step {step}");
            assert_eq!(
                taken.frame.modes(),
                expected.modes(),
                "modes at step {step}"
            );
            assert_eq!(
                taken.frame.background(),
                expected.background(),
                "background at step {step}"
            );
            assert_eq!(
                taken.frame.cursor(),
                expected.cursor(),
                "cursor at step {step}"
            );
            assert_eq!(
                taken.frame.scrollback(),
                expected.scrollback(),
                "viewport at step {step}"
            );
        }
    }
    assert!(moved_rows > 0, "Workload must exercise moved-row deltas");
    assert!(full_frames > 1, "Workload must exercise full-frame resets");
    assert!(
        mailbox.take().is_none(),
        "Final pending batch was not checked"
    );
}
