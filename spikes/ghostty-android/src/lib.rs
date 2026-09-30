use libghostty_vt::Terminal;

#[unsafe(no_mangle)]
pub extern "C" fn or2_ghostty_smoke() -> u64 {
    let Ok(mut terminal) = Terminal::new(20, 4) else {
        return 0;
    };
    let bytes = b"Android \x1b[31mGhostty\x1b[0m";
    terminal.vt_write(bytes);
    bytes.iter().fold(0_u64, |sum, byte| sum + u64::from(*byte))
}
