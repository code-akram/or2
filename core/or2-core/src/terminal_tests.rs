use super::*;

fn engine(columns: u16, rows: u16) -> TerminalEngine {
    TerminalEngine::new(TerminalSize::new(columns, rows).unwrap(), |_| {}).unwrap()
}

fn text(row: &Row) -> String {
    row.cells()
        .iter()
        .map(|cell| {
            if cell.text.is_empty() {
                " "
            } else {
                &cell.text
            }
        })
        .collect()
}

#[test]
fn resolved_styles_golden() {
    let mut terminal = engine(12, 3);
    terminal
        .write(b"\x1b]10;#eeeeee\x07\x1b]11;#010203\x07\x1b]4;1;#123456\x07\x1b]4;2;#abcdef\x07");
    terminal.write(b"\x1b[1;2;3;4:3;9;53;38;2;17;34;51;48;5;196;58;5;2mA\x1b[0m\x1b[31mP\x1b[0m\x1b[38;2;17;34;51;48;2;68;85;102;7mI\x1b[8mV\x1b[0mD");
    let frame = terminal.frame().unwrap();
    assert!(frame.is_full());
    assert_eq!(frame.background().packed(), 0x010203);
    let cells = frame.rows()[0].cells();
    assert_eq!(cells[0].text, "A");
    assert_eq!(
        cells[0].style,
        CellStyle {
            foreground: Rgb::new(17, 34, 51),
            background: Rgb::new(255, 0, 0),
            underline_color: Some(Rgb::new(171, 205, 239)),
            underline: Underline::Curly,
            bold: true,
            italic: true,
            faint: true,
            strikethrough: true,
            overline: true,
        }
    );
    assert_eq!(cells[1].text, "P");
    assert_eq!(
        cells[1].style,
        CellStyle::plain(Rgb::new(18, 52, 86), Rgb::new(1, 2, 3))
    );
    assert_eq!(cells[2].text, "I");
    assert_eq!(cells[2].style.foreground.packed(), 0x445566);
    assert_eq!(cells[2].style.background.packed(), 0x112233);
    assert_eq!(cells[3].text, "V");
    assert_eq!(cells[3].style.foreground.packed(), 0x112233);
    assert_eq!(cells[3].style.background.packed(), 0x112233);
    assert_eq!(
        cells[4].style,
        CellStyle::plain(Rgb::new(238, 238, 238), Rgb::new(1, 2, 3))
    );
    // An OSC palette change is global: unchanged palette-indexed cells must redraw.
    terminal.write(b"\x1b]4;1;#fedcba\x07");
    let frame = terminal.frame().unwrap();
    assert!(frame.is_full());
    assert_eq!(
        frame.rows()[0].cells()[1].style.foreground.packed(),
        0xfedcba
    );
}

#[test]
fn wide_graphemes_cursor_tail_and_spacer_head_golden() {
    let mut terminal = engine(9, 3);
    terminal.write("a界😀e\u{301}\x1b[1;3H\x1b[6 q".as_bytes());
    let frame = terminal.frame().unwrap();
    let cells = frame.rows()[0].cells();
    assert_eq!(
        cells
            .iter()
            .take(6)
            .map(|cell| (cell.text.as_str(), cell.width))
            .collect::<Vec<_>>(),
        [
            ("a", CellWidth::Narrow),
            ("界", CellWidth::Wide),
            ("", CellWidth::SpacerTail),
            ("😀", CellWidth::Wide),
            ("", CellWidth::SpacerTail),
            ("e\u{301}", CellWidth::Narrow),
        ]
    );
    let cursor = frame.cursor().unwrap();
    assert_eq!(
        (
            cursor.column,
            cursor.row,
            cursor.wide,
            cursor.shape,
            cursor.blinking
        ),
        (1, 0, true, CursorShape::Bar, false)
    );
    // Cursor-only movement must also inspect the wide cell on an otherwise clean row.
    terminal.write(b"\x1b[1;4H");
    let cursor = terminal.frame().unwrap().cursor().unwrap();
    assert_eq!((cursor.column, cursor.wide), (3, true));
    terminal.write(b"\x1b[?25l");
    assert!(terminal.frame().unwrap().cursor().is_none());

    let mut terminal = engine(4, 3);
    terminal.write("abc界".as_bytes());
    let frame = terminal.frame().unwrap();
    assert_eq!(text(&frame.rows()[0]), "abc ");
    assert!(frame.rows()[0].wrapped());
    assert_eq!(frame.rows()[0].cells()[3].width, CellWidth::Narrow);
    assert_eq!(frame.rows()[0].cells()[3].text, "");
    assert_eq!(frame.rows()[1].cells()[0].width, CellWidth::Wide);
}

#[test]
fn dirty_rows_are_consumed_and_resync_and_resize_are_full() {
    let mut terminal = engine(8, 3);
    assert!(terminal.frame().unwrap().is_full());
    assert!(terminal.frame().unwrap().rows().is_empty());
    terminal.write(b"\x1b[2;3HX");
    let delta = terminal.frame().unwrap();
    assert!(!delta.is_full());
    // Moving the cursor dirties its old row as well as the new content row.
    assert_eq!(
        delta.rows().iter().map(Row::index).collect::<Vec<_>>(),
        [0, 1]
    );
    assert_eq!(text(&delta.rows()[0]), "        ");
    assert_eq!(text(&delta.rows()[1]), "  X     ");
    assert!(terminal.frame().unwrap().rows().is_empty());
    terminal.request_full_frame();
    assert_eq!(terminal.frame().unwrap().rows().len(), 3);

    let mut terminal = engine(8, 3);
    terminal.write(b"abcdefghijk");
    terminal.frame().unwrap();
    terminal.resize(TerminalSize::new(5, 4).unwrap()).unwrap();
    let frame = terminal.frame().unwrap();
    assert!(frame.is_full());
    assert_eq!(frame.size(), TerminalSize::new(5, 4).unwrap());
    assert_eq!(
        frame.rows().iter().map(text).collect::<Vec<_>>(),
        ["abcde", "fghij", "k    ", "     "]
    );
    assert!(frame.rows()[0].wrapped() && frame.rows()[1].wrapped());
    assert!(!frame.rows()[2].wrapped());
    assert_eq!(
        (frame.cursor().unwrap().column, frame.cursor().unwrap().row),
        (1, 2)
    );
}

#[test]
fn alternate_screen_and_scrollback_golden() {
    let mut terminal = engine(8, 3);
    terminal.write(b"L0\r\nL1\r\nL2\r\nL3\r\nL4");
    let frame = terminal.frame().unwrap();
    assert_eq!(
        frame.scrollback(),
        Scrollback {
            total_rows: 5,
            offset: 2
        }
    );
    assert_eq!(
        frame.rows().iter().map(text).collect::<Vec<_>>(),
        ["L2      ", "L3      ", "L4      "]
    );
    assert!(terminal.scroll(ViewportScroll::Top).unwrap().is_empty());
    let frame = terminal.frame().unwrap();
    assert_eq!(
        frame.scrollback(),
        Scrollback {
            total_rows: 5,
            offset: 0
        }
    );
    assert_eq!(
        frame.rows().iter().map(text).collect::<Vec<_>>(),
        ["L0      ", "L1      ", "L2      "]
    );
    assert!(frame.cursor().is_none());
    terminal.scroll(ViewportScroll::Delta(1)).unwrap();
    assert_eq!(terminal.frame().unwrap().scrollback().offset, 1);
    terminal.scroll(ViewportScroll::Bottom).unwrap();
    assert_eq!(terminal.frame().unwrap().scrollback().offset, 2);
    terminal.write(b"\x1b[?1049h\x1b[HALT\x1b[?1h");
    let frame = terminal.frame().unwrap();
    assert!(frame.is_full());
    assert_eq!(text(&frame.rows()[0]), "ALT     ");
    assert_eq!(
        frame.scrollback(),
        Scrollback {
            total_rows: 3,
            offset: 0
        }
    );
    assert_eq!(
        terminal.scroll(ViewportScroll::Delta(-2)).unwrap(),
        b"\x1bOA\x1bOA"
    );
    assert_eq!(
        terminal.scroll(ViewportScroll::Delta(1)).unwrap(),
        b"\x1bOB"
    );
    terminal.write(b"\x1b[?1049l");
    let frame = terminal.frame().unwrap();
    assert!(frame.is_full());
    assert_eq!(text(&frame.rows()[2]), "L4      ");
}

#[test]
fn frames_report_mouse_tracking_and_the_alternate_screen() {
    let mut terminal = engine(8, 3);
    assert_eq!(terminal.frame().unwrap().modes(), TerminalModes::default());
    // A mode change dirties no row; the next (delta) frame still reports it.
    terminal.write(b"\x1b[?1000h");
    let frame = terminal.frame().unwrap();
    assert!(frame.rows().is_empty());
    assert_eq!(
        frame.modes(),
        TerminalModes {
            mouse_tracking: true,
            alternate_screen: false,
            bracketed_paste: false,
        }
    );
    terminal.write(b"\x1b[?1049h");
    assert_eq!(
        terminal.frame().unwrap().modes(),
        TerminalModes {
            mouse_tracking: true,
            alternate_screen: true,
            bracketed_paste: false,
        }
    );
    terminal.write(b"\x1b[?1000l");
    assert_eq!(
        terminal.frame().unwrap().modes(),
        TerminalModes {
            mouse_tracking: false,
            alternate_screen: true,
            bracketed_paste: false,
        }
    );
    terminal.write(b"\x1b[?1002h\x1b[?1049l");
    assert!(terminal.frame().unwrap().modes().mouse_tracking);
    terminal.write(b"\x1b[?1002l\x1b[?1003h");
    assert!(terminal.frame().unwrap().modes().mouse_tracking);
    terminal.write(b"\x1b[?1003l");
    assert_eq!(terminal.frame().unwrap().modes(), TerminalModes::default());
}

/// Kotlin asks before a multi-line send only while the program has bracketed paste off (contracts.md,
/// "One terminal per herdr session"): frames report the mode both ways, a change alone included.
#[test]
fn frames_report_bracketed_paste_on_and_off() {
    let mut terminal = engine(8, 3);
    assert!(!terminal.frame().unwrap().modes().bracketed_paste);
    terminal.write(b"\x1b[?2004h");
    let frame = terminal.frame().unwrap();
    assert!(frame.rows().is_empty());
    assert_eq!(
        frame.modes(),
        TerminalModes {
            mouse_tracking: false,
            alternate_screen: false,
            bracketed_paste: true,
        }
    );
    terminal.write(b"\x1b[?1049hvim");
    assert!(terminal.frame().unwrap().modes().bracketed_paste);
    terminal.write(b"\x1b[?2004l\x1b[?1049l");
    assert_eq!(terminal.frame().unwrap().modes(), TerminalModes::default());
}

#[test]
fn wheel_scrolls_are_sgr_wheel_events_at_the_touched_cell() {
    let mut terminal = engine(20, 6);
    terminal.write(b"\x1b[?1049h\x1b[?1000h\x1b[?1006h");
    let wheel = |rows, column, row| ViewportScroll::Wheel { rows, column, row };
    // Up is button 64, down 65; coordinates are 1-based.
    assert_eq!(terminal.scroll(wheel(-1, 0, 0)).unwrap(), b"\x1b[<64;1;1M");
    assert_eq!(
        terminal.scroll(wheel(2, 7, 3)).unwrap(),
        b"\x1b[<65;8;4M\x1b[<65;8;4M"
    );
    // A cell past the grid is its last cell; a gesture is at most one viewport of events.
    assert_eq!(
        terminal.scroll(wheel(-50, 99, 99)).unwrap(),
        b"\x1b[<64;20;6M".repeat(6)
    );
    assert!(terminal.scroll(wheel(0, 1, 1)).unwrap().is_empty());
    // Any-event tracking reports the wheel the same way.
    terminal.write(b"\x1b[?1000l\x1b[?1003h");
    assert_eq!(terminal.scroll(wheel(-1, 2, 1)).unwrap(), b"\x1b[<64;3;2M");
}

#[test]
fn wheel_scrolls_use_the_x10_format_when_no_extended_format_is_on() {
    let mut terminal = engine(20, 6);
    terminal.write(b"\x1b[?1000h");
    // ESC [ M, then 32 + button, 32 + column, 32 + row (1-based).
    assert_eq!(
        terminal
            .scroll(ViewportScroll::Wheel {
                rows: -1,
                column: 4,
                row: 2
            })
            .unwrap(),
        [0x1b, b'[', b'M', 32 + 64, 32 + 5, 32 + 3]
    );
    assert_eq!(
        terminal
            .scroll(ViewportScroll::Wheel {
                rows: 1,
                column: 0,
                row: 5
            })
            .unwrap(),
        [0x1b, b'[', b'M', 32 + 65, 32 + 1, 32 + 6]
    );
    // The primary viewport did not move: the program got the wheel.
    assert_eq!(terminal.viewport_offset().unwrap(), None);
}

#[test]
fn a_click_is_a_left_press_and_release_at_the_tapped_cell_in_sgr() {
    let mut terminal = engine(20, 6);
    terminal.write(b"\x1b[?1049h\x1b[?1000h\x1b[?1006h");
    // Button 0 (left); press ends in M, release in m; coordinates are 1-based.
    assert_eq!(
        terminal.mouse_click(7, 3).unwrap(),
        b"\x1b[<0;8;4M\x1b[<0;8;4m"
    );
    assert_eq!(
        terminal.mouse_click(0, 0).unwrap(),
        b"\x1b[<0;1;1M\x1b[<0;1;1m"
    );
    // A cell past the grid is its last cell.
    assert_eq!(
        terminal.mouse_click(99, 99).unwrap(),
        b"\x1b[<0;20;6M\x1b[<0;20;6m"
    );
    // Button-event and any-event tracking report clicks the same way.
    terminal.write(b"\x1b[?1000l\x1b[?1002h");
    assert_eq!(
        terminal.mouse_click(2, 1).unwrap(),
        b"\x1b[<0;3;2M\x1b[<0;3;2m"
    );
    terminal.write(b"\x1b[?1002l\x1b[?1003h");
    assert_eq!(
        terminal.mouse_click(2, 1).unwrap(),
        b"\x1b[<0;3;2M\x1b[<0;3;2m"
    );
}

#[test]
fn a_click_uses_the_x10_format_when_no_extended_format_is_on() {
    let mut terminal = engine(20, 6);
    terminal.write(b"\x1b[?1000h");
    // ESC [ M, then 32 + button, 32 + column, 32 + row (1-based); a release is button 3.
    assert_eq!(
        terminal.mouse_click(4, 2).unwrap(),
        [
            0x1b,
            b'[',
            b'M',
            32,
            32 + 5,
            32 + 3,
            0x1b,
            b'[',
            b'M',
            32 + 3,
            32 + 5,
            32 + 3
        ]
    );
    // X10 tracking (DECSET 9) reports presses only.
    terminal.write(b"\x1b[?1000l\x1b[?9h");
    assert_eq!(
        terminal.mouse_click(4, 2).unwrap(),
        [0x1b, b'[', b'M', 32, 32 + 5, 32 + 3]
    );
}

#[test]
fn a_click_without_mouse_tracking_sends_nothing() {
    let mut terminal = engine(20, 6);
    assert!(terminal.mouse_click(3, 3).unwrap().is_empty());
    terminal.write(b"\x1b[?1049h");
    assert!(terminal.mouse_click(3, 3).unwrap().is_empty());
    // The format alone is not tracking.
    terminal.write(b"\x1b[?1006h");
    assert!(terminal.mouse_click(3, 3).unwrap().is_empty());
    terminal.write(b"\x1b[?1000h");
    assert!(!terminal.mouse_click(3, 3).unwrap().is_empty());
    terminal.write(b"\x1b[?1000l");
    assert!(terminal.mouse_click(3, 3).unwrap().is_empty());
}

#[test]
fn a_wheel_scroll_without_mouse_tracking_is_a_delta() {
    let mut terminal = engine(8, 3);
    terminal.write(b"L0\r\nL1\r\nL2\r\nL3\r\nL4");
    let wheel = ViewportScroll::Wheel {
        rows: -2,
        column: 1,
        row: 1,
    };
    // Primary: the viewport moves.
    assert!(terminal.scroll(wheel).unwrap().is_empty());
    assert_eq!(terminal.frame().unwrap().scrollback().offset, 0);
    terminal.scroll(ViewportScroll::Bottom).unwrap();
    // Alternate in application cursor mode: arrow keys.
    terminal.write(b"\x1b[?1049h\x1b[?1h");
    assert_eq!(terminal.scroll(wheel).unwrap(), b"\x1bOA\x1bOA");
    // X10 tracking (DECSET 9) reports no wheel: the swipe still does something.
    terminal.write(b"\x1b[?9h");
    assert!(terminal.frame().unwrap().modes().mouse_tracking);
    assert_eq!(terminal.scroll(wheel).unwrap(), b"\x1bOA\x1bOA");
}

#[test]
fn key_encoding_golden_uses_current_modes_and_shifted_us_physical_keys() {
    let mut terminal = engine(80, 24);
    let none = Modifiers::default();
    let ctrl = Modifiers { ctrl: true, ..none };
    let shift = Modifiers {
        shift: true,
        ..none
    };
    let alt = Modifiers { alt: true, ..none };
    for (key, modifiers, expected) in [
        (Key::Character("c".into()), ctrl, b"\x03".as_slice()),
        (Key::Character("A".into()), shift, b"A"),
        (Key::Character("!".into()), shift, b"!"),
        (Key::Character("?".into()), shift, b"?"),
        (Key::Character("[".into()), ctrl, b"\x1b"),
        (Key::Character("i".into()), ctrl, b"\t"),
        (Key::Character("m".into()), ctrl, b"\r"),
        (
            Key::Character("[".into()),
            Modifiers { alt: true, ..ctrl },
            b"\x1b\x1b",
        ),
        (Key::Character("a".into()), none, b"a"),
        (Key::Character("x".into()), alt, b"\x1bx"),
        (Key::Character("é".into()), none, "é".as_bytes()),
        (Key::Enter, none, b"\r"),
        (Key::Tab, none, b"\t"),
        (Key::Backspace, none, b"\x7f"),
        (Key::ArrowUp, none, b"\x1b[A"),
        (Key::ArrowRight, none, b"\x1b[C"),
        (Key::Function(1), none, b"\x1bOP"),
        (Key::Function(5), none, b"\x1b[15~"),
        (Key::Function(12), none, b"\x1b[24~"),
    ] {
        let input = KeyInput::new(key, modifiers).unwrap();
        assert_eq!(terminal.encode_key(&input).unwrap(), expected, "{input:?}");
    }
    terminal.write(b"\x1b[?1h");
    assert_eq!(
        terminal
            .encode_key(&KeyInput::new(Key::ArrowUp, none).unwrap())
            .unwrap(),
        b"\x1bOA"
    );
    terminal.write(b"\x1b[?1l");
    assert_eq!(
        terminal
            .encode_key(&KeyInput::new(Key::ArrowUp, none).unwrap())
            .unwrap(),
        b"\x1b[A"
    );
}

#[test]
fn opted_in_key_protocols_keep_the_real_control_keys_and_can_be_disabled() {
    let none = Modifiers::default();
    let ctrl = Modifiers { ctrl: true, ..none };
    let shift = Modifiers {
        shift: true,
        ..none
    };
    for (enable, disable, bracket, i, m, c, capital, bang, question) in [
        (
            b"\x1b[>1u".as_slice(),
            b"\x1b[<u".as_slice(),
            b"\x1b[91;5u".as_slice(),
            b"\x1b[105;5u".as_slice(),
            b"\x1b[109;5u".as_slice(),
            b"\x1b[99;5u".as_slice(),
            b"A".as_slice(),
            b"!".as_slice(),
            b"?".as_slice(),
        ),
        (
            b"\x1b[>4;2m",
            b"\x1b[>4;0m",
            b"\x1b[27;5;91~",
            b"\x1b[27;5;105~",
            b"\x1b[27;5;109~",
            b"\x03",
            b"\x1b[27;2;65~",
            b"!",
            b"?",
        ),
    ] {
        let mut terminal = engine(80, 24);
        terminal.write(enable);
        for (value, modifiers, expected) in [
            ("[", ctrl, bracket),
            ("i", ctrl, i),
            ("m", ctrl, m),
            ("c", ctrl, c),
            ("A", shift, capital),
            ("!", shift, bang),
            ("?", shift, question),
            ("a", none, b"a"),
        ] {
            let input = KeyInput::new(Key::Character(value.into()), modifiers).unwrap();
            assert_eq!(terminal.encode_key(&input).unwrap(), expected, "{input:?}");
        }
        terminal.write(disable);
        assert_eq!(
            terminal
                .encode_key(&KeyInput::new(Key::Character("[".into()), ctrl).unwrap())
                .unwrap(),
            b"\x1b"
        );
    }
}

#[test]
fn terminal_queries_write_replies_back() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let replies = Rc::new(RefCell::new(Vec::new()));
    let recorded = replies.clone();
    let mut terminal = TerminalEngine::new(TerminalSize::new(80, 24).unwrap(), move |bytes| {
        recorded.borrow_mut().extend_from_slice(bytes)
    })
    .unwrap();
    terminal.write(b"\x1b[3;7H\x1b[6n");
    assert_eq!(*replies.borrow(), b"\x1b[3;7R");
}

#[test]
fn default_colours_set_reset_query_and_reverse_screen_resync_clean_rows() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let replies = Rc::new(RefCell::new(Vec::new()));
    let recorded = replies.clone();
    let mut terminal = TerminalEngine::new(TerminalSize::new(7, 3).unwrap(), move |bytes| {
        recorded.borrow_mut().extend_from_slice(bytes)
    })
    .unwrap();
    terminal.write(b"\x1b]10;?\x07\x1b]11;?\x07");
    assert_eq!(
        *replies.borrow(),
        b"\x1b]10;rgb:cdcd/d6d6/f4f4\x07\x1b]11;rgb:1e1e/1e1e/2e2e\x07"
    );
    terminal.write(b"cached");
    terminal.frame().unwrap();
    for (sequence, foreground, background) in [
        (b"\x1b]10;#123456\x07".as_slice(), 0x123456, 0x1e1e2e),
        (b"\x1b]11;#abcdef\x07".as_slice(), 0x123456, 0xabcdef),
        (b"\x1b[?5h".as_slice(), 0xabcdef, 0x123456),
        (b"\x1b[?5l".as_slice(), 0x123456, 0xabcdef),
        (b"\x1b]110\x07".as_slice(), 0xcdd6f4, 0xabcdef),
        (b"\x1b]111\x07".as_slice(), 0xcdd6f4, 0x1e1e2e),
    ] {
        assert!(terminal.frame().unwrap().rows().is_empty());
        terminal.write(sequence);
        let frame = terminal.frame().unwrap();
        assert!(frame.is_full());
        assert_eq!(frame.rows().len(), 3);
        assert_eq!(text(&frame.rows()[0]), "cached ");
        assert_eq!(frame.background().packed(), background);
        for row in frame.rows() {
            for cell in row.cells() {
                assert_eq!(cell.style.foreground.packed(), foreground);
                assert_eq!(cell.style.background.packed(), background);
            }
        }
    }
    replies.borrow_mut().clear();
    terminal.write(b"\x1b]10;?\x07\x1b]11;?\x07");
    assert_eq!(
        *replies.borrow(),
        b"\x1b]10;rgb:cdcd/d6d6/f4f4\x07\x1b]11;rgb:1e1e/1e1e/2e2e\x07"
    );
}

#[test]
fn remote_column_switching_is_contained_at_the_embedder_geometry() {
    let mut terminal = engine(79, 23);
    terminal.frame().unwrap();
    for sequence in [b"\x1b[?40h\x1b[?3h".as_slice(), b"\x1b[?3l".as_slice()] {
        terminal.write(sequence);
        let frame = terminal.frame().unwrap();
        assert!(frame.is_full());
        assert_eq!(frame.size(), TerminalSize::new(79, 23).unwrap());
        assert_eq!(frame.rows().len(), 23);
        assert!(frame.rows().iter().all(|row| row.cells().len() == 79));
    }
    terminal.write(b"still usable");
    assert!(text(&terminal.frame().unwrap().rows()[0]).starts_with("still usable"));
}

#[test]
fn default_colours_and_ansi_palette_are_catppuccin_mocha() {
    let mut terminal = engine(40, 2);
    // Plain text, the 16 ANSI colours as foregrounds, two cube/grey entries, then SGR 41.
    terminal.write(b"a");
    for index in 0..16u8 {
        terminal.write(format!("\x1b[38;5;{index}m{}", char::from(b'A' + index)).as_bytes());
    }
    terminal.write(b"\x1b[38;5;16mx\x1b[38;5;231my\x1b[38;5;232mz\x1b[0m\x1b[41mB\x1b[0m");
    let frame = terminal.frame().unwrap();
    assert_eq!(frame.background().packed(), 0x1e1e2e);
    let cells = frame.rows()[0].cells();
    assert_eq!(cells[0].style.foreground.packed(), 0xcdd6f4);
    assert_eq!(cells[0].style.background.packed(), 0x1e1e2e);
    let expected = [
        0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de, 0x585b70,
        0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
    ];
    for (index, colour) in expected.into_iter().enumerate() {
        assert_eq!(
            cells[index + 1].style.foreground.packed(),
            colour,
            "palette index {index}"
        );
    }
    // The xterm cube and grey ramp are untouched.
    assert_eq!(cells[17].style.foreground.packed(), 0x000000);
    assert_eq!(cells[18].style.foreground.packed(), 0xffffff);
    assert_eq!(cells[19].style.foreground.packed(), 0x080808);
    assert_eq!(cells[20].style.background.packed(), 0xf38ba8);
    // An OSC 4 override still wins; resetting the palette (OSC 104) returns to Mocha.
    terminal.write(b"\x1b]4;1;#010203\x07\x1b[31mC\x1b[0m");
    let frame = terminal.frame().unwrap();
    assert_eq!(
        frame.rows()[0].cells()[21].style.foreground.packed(),
        0x010203
    );
    terminal.write(b"\x1b]104\x07");
    let frame = terminal.frame().unwrap();
    assert_eq!(
        frame.rows()[0].cells()[21].style.foreground.packed(),
        0xf38ba8
    );
}

#[test]
fn submit_follows_the_bracketed_paste_and_key_modes() {
    let mut terminal = engine(20, 3);
    assert!(!terminal.bracketed_paste().unwrap());
    assert_eq!(terminal.submit_text_bytes("a\nb").unwrap(), b"a\rb");
    assert_eq!(terminal.submit_enter_bytes().unwrap(), b"\r");
    terminal.write(b"\x1b[?2004h");
    assert!(terminal.bracketed_paste().unwrap());
    assert_eq!(
        terminal.submit_text_bytes("a\nb").unwrap(),
        b"\x1b[200~a\nb\x1b[201~"
    );
    assert!(terminal.submit_text_bytes("").unwrap().is_empty());
    terminal.write(b"\x1b[>8u");
    assert_eq!(terminal.submit_enter_bytes().unwrap(), b"\x1b[13u");
    terminal.write(b"\x1b[?2004l\x1b[<u");
    assert!(!terminal.bracketed_paste().unwrap());
    assert_eq!(terminal.submit_enter_bytes().unwrap(), b"\r");
}

#[test]
fn osc8_hyperlinks_reach_the_row_as_column_runs() {
    let mut terminal = engine(20, 3);
    terminal.write(b"see \x1b]8;;https://example.org/a\x1b\\docs\x1b]8;;\x1b\\ and ");
    terminal.write("\x1b]8;id=x;https://example.org/b\x07界z\x1b]8;;\x07!".as_bytes());
    let frame = terminal.frame().unwrap();
    let row = &frame.rows()[0];
    assert_eq!(text(row).trim_end(), "see docs and 界 z!");
    assert_eq!(
        row.links(),
        [
            CellLink {
                start_column: 4,
                end_column: 7,
                uri: "https://example.org/a".into()
            },
            // The wide character's tail is part of the run.
            CellLink {
                start_column: 13,
                end_column: 15,
                uri: "https://example.org/b".into()
            },
        ]
    );
    assert!(frame.rows()[1].links().is_empty());

    // Two adjacent links stay two runs; a long URI is read whole.
    let long = format!("https://example.org/{}", "x".repeat(600));
    terminal.write(
        format!("\r\n\x1b]8;;https://a.example\x07ab\x1b]8;;{long}\x07cd\x1b]8;;\x07").as_bytes(),
    );
    let frame = terminal.frame().unwrap();
    let row = frame.rows().iter().find(|row| row.index() == 1).unwrap();
    let runs: Vec<(u16, u16, &str)> = row
        .links()
        .iter()
        .map(|link| (link.start_column, link.end_column, link.uri.as_str()))
        .collect();
    assert_eq!(runs, [(0, 1, "https://a.example"), (2, 3, long.as_str())]);
}

#[test]
fn osc52_clipboard_writes_are_decoded_and_bad_ones_dropped() {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use std::cell::RefCell;
    use std::rc::Rc;

    let mut terminal = engine(20, 3);
    assert_eq!(terminal.take_clipboard_write(), None);
    // "hello world" in base64, BEL-terminated.
    terminal.write(b"\x1b]52;c;aGVsbG8gd29ybGQ=\x07");
    assert_eq!(
        terminal.take_clipboard_write().as_deref(),
        Some("hello world")
    );
    assert_eq!(terminal.take_clipboard_write(), None, "taken once");
    // The newest of several wins, ST-terminated, whichever destination they name.
    terminal.write(b"\x1b]52;c;b25l\x1b\\\x1b]52;p;dHdv\x1b\\");
    assert_eq!(terminal.take_clipboard_write().as_deref(), Some("two"));
    // A read request is never answered and is not a write.
    let replies = Rc::new(RefCell::new(Vec::new()));
    let sent = replies.clone();
    let mut asked = TerminalEngine::new(TerminalSize::new(20, 3).unwrap(), move |bytes| {
        sent.borrow_mut().extend_from_slice(bytes)
    })
    .unwrap();
    asked.write(b"\x1b]52;c;?\x07");
    assert_eq!(asked.take_clipboard_write(), None);
    assert!(replies.borrow().is_empty());
    // Invalid base64, bytes that are not UTF-8 ("//4=" is FF FE) and a clear request.
    for bad in [
        &b"\x1b]52;c;!!!not base64\x07"[..],
        b"\x1b]52;c;//4=\x07",
        b"\x1b]52;c;\x07",
    ] {
        terminal.write(bad);
        assert_eq!(terminal.take_clipboard_write(), None, "{bad:?}");
    }
    // Over the cap is dropped whole; at the cap passes.
    let encode = |length: usize| {
        let mut sequence = b"\x1b]52;c;".to_vec();
        sequence.extend(STANDARD.encode(vec![b'a'; length]).bytes());
        sequence.push(0x07);
        sequence
    };
    terminal.write(&encode(MAX_CLIPBOARD_BYTES + 3));
    assert_eq!(terminal.take_clipboard_write(), None);
    terminal.write(&encode(MAX_CLIPBOARD_BYTES));
    assert_eq!(
        terminal.take_clipboard_write().map(|text| text.len()),
        Some(MAX_CLIPBOARD_BYTES)
    );
    // The screen is untouched by any of it.
    let frame = terminal.frame().unwrap();
    assert!(frame.rows().iter().all(|row| text(row).trim().is_empty()));
}

#[test]
fn a_restored_engine_still_reports_clipboard_writes() {
    let terminal = engine(20, 3);
    let mut restored =
        TerminalEngine::from_snapshot(&terminal.snapshot().unwrap(), |_| {}).unwrap();
    restored.write(b"\x1b]52;c;aGk=\x07");
    assert_eq!(restored.take_clipboard_write().as_deref(), Some("hi"));
}
