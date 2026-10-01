//! The on-host confirmation: the person at this machine, not the phone, decides.
//!
//! The binary only ever uses [`StdinConfirm`], which asks on the terminal and refuses to run
//! without one. The [`Confirm`] trait is public so tests can answer for it; there is no flag, no
//! environment variable and no default that says yes.

use std::io::{self, BufRead, IsTerminal, Write};
use std::sync::mpsc;
use std::time::Instant;

/// What the person is asked to authorize.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmRequest {
    pub user: String,
    /// The phone's label, already reduced to safe ASCII.
    pub device: String,
    /// `SHA256:…` of the phone's key.
    pub fingerprint: String,
    pub algorithm: String,
    /// The phone's network address, for the screen.
    pub peer: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    /// Nobody answered before the deadline.
    TimedOut,
}

pub trait Confirm {
    /// Asks, and returns when answered or at `deadline`.
    fn confirm(&self, request: &ConfirmRequest, deadline: Instant) -> Answer;
}

/// Only `y` and `yes` (any case) are a yes; everything else, including an empty line, is a no.
pub fn is_yes(line: &str) -> bool {
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Asks on the terminal: `Authorize this key for <user>? [y/N]`.
pub struct StdinConfirm;

impl StdinConfirm {
    /// Whether there is a person to ask: standard input must be a terminal, so that `yes |
    /// or2-pair` or a script cannot answer for the user.
    pub fn available() -> bool {
        io::stdin().is_terminal()
    }
}

impl Confirm for StdinConfirm {
    fn confirm(&self, request: &ConfirmRequest, deadline: Instant) -> Answer {
        let mut out = io::stdout();
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "A phone called \"{}\" ({}) is asking for access as {}.",
            request.device, request.peer, request.user
        );
        let _ = writeln!(
            out,
            "Its key: {} ({})",
            request.fingerprint, request.algorithm
        );
        let _ = write!(out, "Authorize this key for {}? [y/N] ", request.user);
        let _ = out.flush();
        let (sender, receiver) = mpsc::channel();
        // The thread may outlive the answer (a read cannot be cancelled); the process exits
        // right after the exchange, which ends it.
        std::thread::spawn(move || {
            let mut line = String::new();
            let read = io::stdin().lock().read_line(&mut line);
            let _ = sender.send(read.ok().map(|_| line));
        });
        let wait = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(wait) {
            Ok(Some(line)) if is_yes(&line) => Answer::Yes,
            Ok(_) => Answer::No,
            Err(_) => {
                let _ = writeln!(out);
                Answer::TimedOut
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_y_and_yes_are_a_yes() {
        for line in ["y", "Y", "yes", "YES", " y \n", "Yes\r\n"] {
            assert!(is_yes(line), "{line:?}");
        }
        for line in [
            "", "\n", "n", "N", "no", "yy", "yeah", "ok", "1", "true", "y n", "ye",
        ] {
            assert!(!is_yes(line), "{line:?}");
        }
    }
}
