use std::{fs, io::Read, thread, time::Duration};

use libghostty_vt::{
    RenderState, Terminal,
    key::{Encoder, Event, Key, Mods},
    render::{CellIterator, RowIterator},
    style::{PaletteIndex, StyleColor},
};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn grid(terminal: &Terminal<'_, '_>) -> Result<(Vec<Vec<String>>, String)> {
    let mut state = RenderState::new()?;
    let mut rows = RowIterator::new()?;
    let mut cells = CellIterator::new()?;
    let snapshot = state.update(terminal)?;
    let mut row_iter = rows.update(&snapshot)?;
    let mut matrix = Vec::new();
    let mut dump = String::new();
    while let Some(row) = row_iter.next() {
        let mut cell_iter = cells.update(row)?;
        let mut line = Vec::new();
        while let Some(cell) = cell_iter.next() {
            let text: String = cell.graphemes()?.into_iter().collect();
            line.push(text);
        }
        for text in &line {
            dump.push_str(if text.is_empty() { " " } else { text });
        }
        dump.push('\n');
        matrix.push(line);
    }
    Ok((matrix, dump))
}

fn handwritten() -> Result<()> {
    let mut term = Terminal::new(20, 5)?;
    term.vt_write(b"base\x1b[2;3H\x1b[1;31mRED\x1b[0m \xe7\x95\x8c \xf0\x9f\x98\x80");
    let (cells, before) = grid(&term)?;
    assert_eq!(cells[0][0], "b");
    assert_eq!(cells[1][2], "R");
    assert_eq!(cells[1][6], "界");
    assert_eq!(cells[1][7], "", "CJK wide-cell tail must be empty");
    assert_eq!(cells[1][9], "😀");
    assert_eq!(cells[1][10], "", "emoji wide-cell tail must be empty");
    assert!(matches!(
        {
            let mut state = RenderState::new()?;
            let mut rows = RowIterator::new()?;
            let mut columns = CellIterator::new()?;
            let snapshot = state.update(&term)?;
            let mut row_iter = rows.update(&snapshot)?;
            let _ = row_iter.next().unwrap();
            let row = row_iter.next().unwrap();
            let mut iter = columns.update(row)?;
            iter.select(2)?;
            iter.style()?.fg_color
        },
        StyleColor::Palette(PaletteIndex::RED)
    ));
    term.vt_write(b"\x1b[?1049hALT\x1b[3;4HZ\x1b[?1049l");
    let (after_cells, after) = grid(&term)?;
    assert_eq!(after_cells[0][0], "b", "primary screen survives alt screen");
    fs::write(
        "handwritten-grid.txt",
        format!("before alt screen:\n{before}\nafter returning:\n{after}"),
    )?;
    Ok(())
}

fn key_bytes() -> Result<()> {
    let mut encoder = Encoder::new()?;
    let mut event = Event::new()?;
    let mut output = String::new();
    for (name, key, mods, text) in [
        ("Ctrl+C", Key::C, Mods::CTRL, Some("c")),
        ("ArrowUp", Key::ArrowUp, Mods::empty(), None),
        ("Enter", Key::Enter, Mods::empty(), None),
    ] {
        event.set_key(key).set_mods(mods).set_utf8(text);
        let mut bytes = Vec::new();
        encoder.encode_to_vec(&event, &mut bytes)?;
        output.push_str(&format!("{name}: {bytes:?}\n"));
    }
    fs::write("key-encoder.txt", &output)?;
    print!("{output}");
    Ok(())
}

fn tui() -> Result<()> {
    let pair = native_pty_system().openpty(PtySize {
        rows: 30,
        cols: 100,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut command = CommandBuilder::new("top");
    command.env("TERM", "xterm-256color");
    command.env("LC_ALL", "C.UTF-8");
    let mut child = pair.slave.spawn_command(command)?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader()?;
    let reader_thread = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = reader.read_to_end(&mut bytes);
        bytes
    });
    thread::sleep(Duration::from_secs(2));
    child.kill()?;
    let _ = child.wait();
    drop(pair.master);
    let bytes = reader_thread.join().map_err(|_| "PTY reader panicked")?;
    fs::write("top-raw.bin", &bytes)?;
    let mut terminal = Terminal::new(100, 30)?;
    terminal.vt_write(&bytes);
    let (_, dump) = grid(&terminal)?;
    fs::write("top-grid.txt", &dump)?;
    assert!(
        dump.contains("Tasks:") && dump.contains("PID USER"),
        "top grid lacks expected headings"
    );
    println!("captured {} PTY bytes", bytes.len());
    Ok(())
}

fn main() -> Result<()> {
    handwritten()?;
    key_bytes()?;
    tui()?;
    Ok(())
}
