//! Terminal input from Kotlin: committed text, discrete keys and viewport scrolling.
//!
//! - Committed IME text is typing, not a paste: it is written as UTF-8 with newlines mapped to
//!   carriage return (what Enter sends). Composing text never leaves Kotlin.
//! - Keys (keys row, hardware keyboard, modifier combinations) go through libghostty's key
//!   encoder, which applies the terminal's current modes (application cursor keys, Kitty
//!   keyboard flags). Only presses are sent in M1.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Enter,
    Tab,
    Backspace,
    Escape,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    /// F1 to F12.
    Function(u8),
    /// The unmodified character the key produces, e.g. `c` for Ctrl+C or `[` for Ctrl+[.
    /// ASCII characters map to US-layout physical keys; others encode as text.
    Character(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyInput {
    key: Key,
    modifiers: Modifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InputError {
    #[error("function keys are F1 to F12")]
    FunctionKeyOutOfRange,
    #[error("character keys need nonempty text without control characters")]
    InvalidCharacter,
}

impl KeyInput {
    pub fn new(key: Key, modifiers: Modifiers) -> Result<Self, InputError> {
        match &key {
            Key::Function(n) if !(1..=12).contains(n) => {
                return Err(InputError::FunctionKeyOutOfRange);
            }
            Key::Character(text) if text.is_empty() || text.chars().any(char::is_control) => {
                return Err(InputError::InvalidCharacter);
            }
            _ => {}
        }
        Ok(Self { key, modifiers })
    }

    pub fn key(&self) -> &Key {
        &self.key
    }

    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }
}

/// Bytes written to the PTY for committed text. `\r\n` and `\n` both become `\r`.
pub fn text_bytes(text: &str) -> Vec<u8> {
    text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
}

/// Viewport movement through scrollback. In the alternate screen Rust translates scrolling
/// into what the application expects (for example arrow keys), so Kotlin always sends this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewportScroll {
    Top,
    Bottom,
    /// Rows to move; negative moves up into history.
    Delta(i32),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_maps_newlines_to_carriage_return_and_keeps_utf8() {
        assert_eq!(text_bytes("ls\n"), b"ls\r");
        assert_eq!(text_bytes("a\r\nb\nc\r"), b"a\rb\rc\r");
        assert_eq!(text_bytes("é界😀"), "é界😀".as_bytes());
        assert_eq!(text_bytes("\ttab"), b"\ttab");
        assert!(text_bytes("").is_empty());
    }

    #[test]
    fn validates_function_keys_and_character_text() {
        let none = Modifiers::default();
        assert!(KeyInput::new(Key::Function(1), none).is_ok());
        assert!(KeyInput::new(Key::Function(12), none).is_ok());
        assert_eq!(
            KeyInput::new(Key::Function(0), none),
            Err(InputError::FunctionKeyOutOfRange)
        );
        assert_eq!(
            KeyInput::new(Key::Function(13), none),
            Err(InputError::FunctionKeyOutOfRange)
        );
        let ctrl = Modifiers { ctrl: true, ..none };
        let input = KeyInput::new(Key::Character("c".into()), ctrl).unwrap();
        assert_eq!(input.key(), &Key::Character("c".into()));
        assert!(input.modifiers().ctrl && !input.modifiers().alt);
        assert!(KeyInput::new(Key::Character("é".into()), none).is_ok());
        for bad in ["", "\u{3}", "a\n", "\u{7f}"] {
            assert_eq!(
                KeyInput::new(Key::Character(bad.into()), ctrl),
                Err(InputError::InvalidCharacter),
                "{bad:?}"
            );
        }
    }
}
