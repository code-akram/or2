//! Where the pairing code `K` is typed: one line from standard input.
//!
//! The binary asks only when standard input is a terminal (so `yes | or2-pair` or a script cannot
//! answer for the person); the `test-support` host reads the same line from a pipe. The code is
//! not echoed back, written to a file or logged, and every buffer that held it is zeroized
//! (the line is read from descriptor 0 directly, not through `std`'s buffered stdin, whose
//! buffer could not be wiped).

use std::io::{self, IsTerminal};

use zeroize::Zeroizing;

/// The longest line taken (a code is 12 characters plus separators).
pub const MAX_LINE: usize = 256;

/// A source of typed lines.
pub trait CodePrompt {
    /// One line without its terminator; `None` at the end of input (Ctrl-D, a closed pipe).
    fn read_line(&self) -> Option<Zeroizing<String>>;
}

/// Standard input.
#[derive(Debug, Clone, Copy, Default)]
pub struct Stdin;

impl Stdin {
    /// Whether there is a person to ask: standard input must be a terminal.
    pub fn available() -> bool {
        io::stdin().is_terminal()
    }
}

impl CodePrompt for Stdin {
    fn read_line(&self) -> Option<Zeroizing<String>> {
        read_line_from_stdin()
    }
}

#[cfg(unix)]
fn read_line_from_stdin() -> Option<Zeroizing<String>> {
    use std::fs::File;
    use std::io::Read;
    use std::mem::ManuallyDrop;
    use std::os::fd::FromRawFd;

    // SAFETY: descriptor 0 is open for the life of the process; `ManuallyDrop` keeps this
    // `File` from closing it.
    let mut stdin = ManuallyDrop::new(unsafe { File::from_raw_fd(0) });
    let mut line = Zeroizing::new(Vec::<u8>::new());
    // One byte at a time, so that nothing after the newline is consumed (a pipe may hold the
    // next line already).
    let mut chunk = Zeroizing::new([0u8; 1]);
    // Whether a line ended with its newline (an empty one is then an empty answer) or input
    // ended first.
    let mut complete = false;
    loop {
        let count = match stdin.read(&mut *chunk) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        if let Some(end) = chunk[..count].iter().position(|byte| *byte == b'\n') {
            line.extend_from_slice(&chunk[..end]);
            complete = true;
            break;
        }
        line.extend_from_slice(&chunk[..count]);
        if line.len() > MAX_LINE {
            break;
        }
    }
    if line.is_empty() && !complete {
        return None;
    }
    // Lossy: anything that is not text cannot be a code and fails its check.
    let text = Zeroizing::new(String::from_utf8_lossy(&line).into_owned());
    Some(text)
}

#[cfg(not(unix))]
fn read_line_from_stdin() -> Option<Zeroizing<String>> {
    let mut line = Zeroizing::new(String::new());
    match io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line),
    }
}
