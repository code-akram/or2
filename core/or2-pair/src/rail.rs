//! The look of everything `or2-pair` prints for a person: one continuous rail in a dim left
//! gutter, in the style of `@clack/prompts`.
//!
//! ```text
//! ┌  or2-pair 0.1.1 · pair a phone with this host
//! │
//! ✔  sshd is answering on port 22 (OpenSSH_9.8)
//! ▲  tmux not found (optional: or2 can attach to its sessions)
//! │  install it: `sudo apt install tmux`
//! │
//! ◇  Code shown on your phone
//! │  ••••-••••-••••
//! │
//! └  Paired
//! ```
//!
//! The rail opens with `┌` and a title, and closes with `└` and one short word. Each message
//! swaps the rail glyph for a symbol (● info, ✔ ok, ▲ warning, ■ error, ◆ a question being
//! asked, ◇ an answered one); its further lines continue the rail (`│  `), and long lines wrap
//! to the terminal's width with the rail continued (a word is never split, so a command or a
//! path stays copyable). Blank rail lines separate the steps.
//!
//! Colour only on a terminal, never with `NO_COLOR` or `--no-color` or `TERM=dumb`, and only the
//! 16-colour palette (dim, red, green, yellow, blue, cyan), so it fits any theme. Unicode
//! glyphs only when the locale is UTF-8 (and not with `--ascii`); otherwise the same rail in
//! ASCII: `+ | ` and `* + ! x > >`. Nothing here moves the cursor except an answered question,
//! and only on a terminal (see [`Rail::answer`]).

use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};

/// How the rail is drawn on one output stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    /// ANSI colours (SGR, the 16-colour palette).
    pub color: bool,
    /// The Unicode glyphs; ASCII otherwise.
    pub unicode: bool,
    /// The terminal's width in columns, to wrap at; `None` (not a terminal) wraps nothing.
    pub width: Option<usize>,
    /// The stream is a terminal that takes cursor movement: an answered question is redrawn
    /// in place and an input hint is shown.
    pub live: bool,
}

impl Style {
    /// No colour, no wrapping, nothing redrawn, Unicode glyphs: what tests read.
    pub const fn plain() -> Self {
        Self {
            color: false,
            unicode: true,
            width: None,
            live: false,
        }
    }

    /// The style for standard output or standard error of this process.
    pub fn detect(stream: Stream) -> Self {
        let tty = match stream {
            Stream::Stdout => io::stdout().is_terminal(),
            Stream::Stderr => io::stderr().is_terminal(),
        };
        Self::from_env(
            tty,
            |name| std::env::var_os(name),
            || terminal_width(stream),
        )
    }

    /// The style for a stream that is a terminal or not, with the environment read through
    /// `var` and the terminal's width asked of `width` (only on a terminal).
    pub fn from_env(
        tty: bool,
        var: impl Fn(&str) -> Option<OsString>,
        width: impl FnOnce() -> Option<usize>,
    ) -> Self {
        let set = |name: &str| var(name).filter(|value| !value.is_empty());
        let dumb = set("TERM").is_some_and(|term| term == "dumb");
        let width = tty.then(|| {
            width()
                .or_else(|| {
                    set("COLUMNS").and_then(|columns| columns.to_str()?.trim().parse().ok())
                })
                .unwrap_or(80)
        });
        Self {
            color: tty && set("NO_COLOR").is_none() && !dumb,
            unicode: utf8_locale(&var),
            width,
            live: cfg!(unix) && tty && !dumb,
        }
    }

    /// The same with the command line's say: `--no-color` turns colour off, `--ascii` the
    /// Unicode glyphs.
    pub fn with_flags(self, no_color: bool, ascii: bool) -> Self {
        Self {
            color: self.color && !no_color,
            unicode: self.unicode && !ascii,
            ..self
        }
    }
}

/// Which of this process's output streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// Whether the locale is UTF-8: the first of `LC_ALL`, `LC_CTYPE` and `LANG` that is set and
/// not empty names UTF-8 (as `setlocale` picks it). None set is the C locale: ASCII.
pub fn utf8_locale(var: impl Fn(&str) -> Option<OsString>) -> bool {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .find_map(|name| var(name).filter(|value| !value.is_empty()))
        .is_some_and(|value| {
            let value = value.to_string_lossy().to_ascii_lowercase();
            value.contains("utf-8") || value.contains("utf8")
        })
}

/// The width of the terminal on `stream`, when it says.
#[cfg(unix)]
pub fn terminal_width(stream: Stream) -> Option<usize> {
    let fd = match stream {
        Stream::Stdout => libc::STDOUT_FILENO,
        Stream::Stderr => libc::STDERR_FILENO,
    };
    // SAFETY: `winsize` is plain old data that `TIOCGWINSZ` fills; the descriptor is only read.
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut size) } == 0;
    (ok && size.ws_col > 0).then_some(usize::from(size.ws_col))
}

#[cfg(not(unix))]
pub fn terminal_width(_: Stream) -> Option<usize> {
    None
}

/// What a line of the rail says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// ● (blue): something to know.
    Info,
    /// ✔ (green): a check that passed, a step that is done.
    Ok,
    /// ▲ (yellow): something that may get in the way, or a calm notice.
    Warn,
    /// ■ (red): something that stops the run here.
    Error,
    /// ◆ (cyan): a question being asked.
    Prompt,
    /// ◇ (green): a question that was answered.
    Answered,
}

struct Glyphs {
    open: &'static str,
    bar: &'static str,
    close: &'static str,
    info: &'static str,
    ok: &'static str,
    warn: &'static str,
    error: &'static str,
    prompt: &'static str,
    answered: &'static str,
    /// Stands for each character of a hidden answer.
    mask: char,
    /// Between the title and what the run does.
    dot: &'static str,
}

const UNICODE: Glyphs = Glyphs {
    open: "┌",
    bar: "│",
    close: "└",
    info: "●",
    ok: "✔",
    warn: "▲",
    error: "■",
    prompt: "◆",
    answered: "◇",
    mask: '•',
    dot: "·",
};

const ASCII: Glyphs = Glyphs {
    open: "+",
    bar: "|",
    close: "`",
    info: "*",
    ok: "+",
    warn: "!",
    error: "x",
    prompt: ">",
    answered: ">",
    mask: '*',
    dot: "-",
};

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const BLUE: &str = "\x1b[34m";
const CYAN: &str = "\x1b[36m";

/// The gutter: the glyph, then two spaces before the text.
const GUTTER: usize = 3;

/// Below this many columns for the text, nothing is wrapped (it would only get worse).
const NARROWEST: usize = 16;

/// Draws the rail on whatever it is given to write to.
#[derive(Debug, Clone, Copy)]
pub struct Rail {
    style: Style,
}

impl Rail {
    pub fn new(style: Style) -> Self {
        Self { style }
    }

    pub fn style(&self) -> Style {
        self.style
    }

    fn glyphs(&self) -> &'static Glyphs {
        if self.style.unicode { &UNICODE } else { &ASCII }
    }

    /// `text` in `color` (an SGR sequence), when colours are on.
    fn paint(&self, color: &str, text: &str) -> String {
        if self.style.color && !text.is_empty() {
            format!("{color}{text}{RESET}")
        } else {
            text.to_owned()
        }
    }

    fn bar(&self) -> String {
        self.paint(DIM, self.glyphs().bar)
    }

    fn mark(&self, mark: Mark) -> String {
        let glyphs = self.glyphs();
        let (glyph, color) = match mark {
            Mark::Info => (glyphs.info, BLUE),
            Mark::Ok => (glyphs.ok, GREEN),
            Mark::Warn => (glyphs.warn, YELLOW),
            Mark::Error => (glyphs.error, RED),
            Mark::Prompt => (glyphs.prompt, CYAN),
            Mark::Answered => (glyphs.answered, GREEN),
        };
        self.paint(color, glyph)
    }

    /// The columns there are for text after the gutter, when wrapping.
    fn text_width(&self, indent: usize) -> Option<usize> {
        self.style
            .width
            .map(|width| width.saturating_sub(GUTTER + indent))
            .filter(|width| *width >= NARROWEST)
    }

    /// Whether a line of `text` after the gutter fits the terminal without wrapping.
    fn fits(&self, text: &str) -> bool {
        self.style
            .width
            .is_none_or(|width| GUTTER + text.chars().count() < width)
    }

    /// One line: `lead`, and the text when there is some (no trailing spaces).
    fn line(out: &mut dyn Write, lead: &str, text: &str) -> io::Result<()> {
        if text.is_empty() {
            writeln!(out, "{lead}")
        } else {
            writeln!(out, "{lead}  {text}")
        }
    }

    /// `┌  title · what`: the start of the rail.
    pub fn open(&self, out: &mut dyn Write, title: &str, what: &str) -> io::Result<()> {
        let glyphs = self.glyphs();
        let rest = self.paint(DIM, &format!("{} {what}", glyphs.dot));
        writeln!(out, "{}  {title} {rest}", self.paint(DIM, glyphs.open))
    }

    /// `│`: between steps.
    pub fn gap(&self, out: &mut dyn Write) -> io::Result<()> {
        writeln!(out, "{}", self.bar())
    }

    /// A message: its first line after `mark`, every further line (each `\n` in `text` starts
    /// one; leading spaces are dropped) on the rail. Lines wrap to the terminal's width.
    pub fn step(&self, out: &mut dyn Write, mark: Mark, text: &str) -> io::Result<()> {
        self.lines(out, Some(mark), text, false)
    }

    /// Further lines of the message above, on the rail.
    pub fn detail(&self, out: &mut dyn Write, text: &str) -> io::Result<()> {
        self.lines(out, None, text, false)
    }

    /// Like [`Rail::detail`], dimmed: what matters less.
    pub fn quiet(&self, out: &mut dyn Write, text: &str) -> io::Result<()> {
        self.lines(out, None, text, true)
    }

    fn lines(
        &self,
        out: &mut dyn Write,
        mark: Option<Mark>,
        text: &str,
        dim: bool,
    ) -> io::Result<()> {
        let mut lead = mark.map(|mark| self.mark(mark));
        let mut code = false;
        for paragraph in text.split('\n').map(str::trim) {
            // A command on a line of its own is never broken: it is copied whole.
            let width = if is_command(paragraph) {
                None
            } else {
                self.text_width(0)
            };
            for line in wrap(paragraph, width) {
                let lead = lead.take().unwrap_or_else(|| self.bar());
                let shown = if dim {
                    self.paint(DIM, &line)
                } else {
                    self.code_spans(&line, &mut code)
                };
                Self::line(out, &lead, &shown)?;
            }
        }
        Ok(())
    }

    /// `line` with what is between backticks (a command to type) in cyan; `open` carries an
    /// unclosed span over to the next line.
    fn code_spans(&self, line: &str, open: &mut bool) -> String {
        if !self.style.color {
            return line.to_owned();
        }
        let mut shown = String::new();
        if *open {
            shown.push_str(CYAN);
        }
        for c in line.chars() {
            if c == '`' && !*open {
                shown.push_str(CYAN);
                shown.push(c);
                *open = true;
            } else if c == '`' {
                shown.push(c);
                shown.push_str(RESET);
                *open = false;
            } else {
                shown.push(c);
            }
        }
        if *open {
            shown.push_str(RESET);
        }
        shown
    }

    /// Labelled values on the rail, the labels dimmed in one column; a value of several lines
    /// (`\n`), or one that wraps, goes on under its first line.
    pub fn rows(&self, out: &mut dyn Write, rows: &[(&str, String)]) -> io::Result<()> {
        let column = rows
            .iter()
            .map(|(label, _)| label.chars().count())
            .max()
            .unwrap_or(0)
            + 2;
        for (label, value) in rows {
            let mut first = true;
            for paragraph in value.split('\n') {
                for line in wrap(paragraph, self.text_width(column)) {
                    let head = if first { *label } else { "" };
                    let pad = " ".repeat(column - head.chars().count());
                    let shown = self.paint(DIM, &format!("{head}{pad}"));
                    Self::line(out, &self.bar(), &format!("{shown}{line}"))?;
                    first = false;
                }
            }
        }
        Ok(())
    }

    /// Lines drawn as they are (the QR code) on the rail: nothing wrapped or recoloured.
    pub fn block(&self, out: &mut dyn Write, text: &str) -> io::Result<()> {
        for line in text.lines() {
            Self::line(out, &self.bar(), line)?;
        }
        Ok(())
    }

    /// A line on its own, off the rail, as it is: the pairing code, so that selecting the line
    /// copies exactly it (a rail glyph in front or a wrap would break the paste).
    pub fn bare(&self, out: &mut dyn Write, text: &str) -> io::Result<()> {
        writeln!(out, "{text}")
    }

    /// `└  word`: the end of the rail.
    pub fn close(&self, out: &mut dyn Write, word: &str) -> io::Result<()> {
        writeln!(out, "{}  {word}", self.paint(DIM, self.glyphs().close))
    }

    /// `◆  question`, and the rail of the answer, where the cursor waits. On a terminal a
    /// dimmed `hint` fills the answer's line until it is answered (the cursor at its start: the
    /// answer is not echoed).
    pub fn ask(&self, out: &mut dyn Write, question: &str, hint: &str) -> io::Result<()> {
        Self::line(out, &self.mark(Mark::Prompt), question)?;
        write!(out, "{}  ", self.bar())?;
        if self.style.live && !hint.is_empty() && self.fits(hint) {
            write!(out, "{}\r\x1b[{GUTTER}C", self.paint(DIM, hint))?;
        }
        out.flush()
    }

    /// The answer to the question [`Rail::ask`] put: `shown` on the answer's line. On a
    /// terminal the question's line is drawn again with `mark` (◇ answered, ▲ refused, ■
    /// cancelled), unless `moved` says the screen may have changed under it (the process was
    /// stopped and continued: the shell wrote in between) or the question does not fit one
    /// line; then only the answer's line is drawn. Elsewhere `shown` ends the line the cursor
    /// is on.
    pub fn answer(
        &self,
        out: &mut dyn Write,
        mark: Mark,
        question: &str,
        shown: &str,
        moved: bool,
    ) -> io::Result<()> {
        if !self.style.live {
            return writeln!(out, "{shown}");
        }
        if !moved && self.fits(question) {
            write!(out, "\r\x1b[1A\x1b[2K")?;
            Self::line(out, &self.mark(mark), question)?;
        }
        write!(out, "\r\x1b[2K")?;
        Self::line(out, &self.bar(), shown)
    }

    /// The end of the rail if the process is ended while the question [`Rail::ask`] put waits
    /// (Ctrl-C): `shown` on the answer's line, and the rail closed with `word`. Prepared ahead,
    /// as bytes, for a signal handler to write.
    pub fn ending_note(&self, shown: &str, word: &str) -> Vec<u8> {
        let mut out = Vec::new();
        // Writing to memory cannot fail.
        let _ = self.answer(&mut out, Mark::Error, "", shown, true);
        let _ = self.gap(&mut out);
        let _ = self.close(&mut out, word);
        out
    }

    /// `typed` hidden: each character a mask, but for the separators a code may be typed with.
    pub fn mask(&self, typed: &str) -> String {
        let mask = self.glyphs().mask;
        typed
            .trim()
            .chars()
            .map(|c| if c == '-' || c == ' ' { c } else { mask })
            .collect()
    }
}

/// Whether `line` is one command between backticks and nothing else.
fn is_command(line: &str) -> bool {
    line.len() > 2
        && line.starts_with('`')
        && line.ends_with('`')
        && !line[1..line.len() - 1].contains('`')
}

/// `text` in lines of at most `width` columns, broken at spaces. A word longer than the width
/// stays whole on a line of its own: a command or a path is never split. `None` keeps one line.
pub fn wrap(text: &str, width: Option<usize>) -> Vec<String> {
    let Some(width) = width else {
        return vec![text.to_owned()];
    };
    let mut lines = Vec::new();
    let mut words = text.split(' ');
    let mut line = words.next().unwrap_or_default().to_owned();
    let mut used = line.chars().count();
    for word in words {
        let size = word.chars().count();
        // Spaces at a break are the break; spaces inside a line stay.
        if !word.is_empty() && !line.trim().is_empty() && used + 1 + size > width {
            lines.push(line.trim_end().to_owned());
            line = word.to_owned();
            used = size;
        } else {
            line.push(' ');
            line.push_str(word);
            used += 1 + size;
        }
    }
    lines.push(line.trim_end().to_owned());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styled(color: bool, unicode: bool, width: Option<usize>) -> Rail {
        Rail::new(Style {
            color,
            unicode,
            width,
            live: false,
        })
    }

    fn drawn(rail: &Rail, draw: impl FnOnce(&Rail, &mut Vec<u8>) -> io::Result<()>) -> String {
        let mut out = Vec::new();
        draw(rail, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn a_whole_run_in_unicode_without_colour() {
        let rail = styled(false, true, None);
        let text = drawn(&rail, |rail, out| {
            rail.open(out, "or2-pair 1.2.3", "check this host")?;
            rail.gap(out)?;
            rail.step(out, Mark::Ok, "sshd is answering on port 22")?;
            rail.step(
                out,
                Mark::Info,
                "tmux not found\ninstall it: `apt install tmux`",
            )?;
            rail.step(out, Mark::Warn, "careful")?;
            rail.step(out, Mark::Error, "stopped")?;
            rail.gap(out)?;
            rail.close(out, "Done")
        });
        assert_eq!(
            text,
            "┌  or2-pair 1.2.3 · check this host\n│\n✔  sshd is answering on port 22\n●  tmux not found\n│  install it: `apt install tmux`\n▲  careful\n■  stopped\n│\n└  Done\n"
        );
    }

    #[test]
    fn the_ascii_rail_has_the_same_shape() {
        let rail = styled(false, false, None);
        let text = drawn(&rail, |rail, out| {
            rail.open(out, "or2-pair 1.2.3", "check this host")?;
            rail.gap(out)?;
            for mark in [
                Mark::Info,
                Mark::Ok,
                Mark::Warn,
                Mark::Error,
                Mark::Prompt,
                Mark::Answered,
            ] {
                rail.step(out, mark, "x")?;
            }
            rail.detail(out, "more")?;
            rail.close(out, "Done")
        });
        assert!(text.is_ascii(), "{text}");
        assert_eq!(
            text,
            "+  or2-pair 1.2.3 - check this host\n|\n*  x\n+  x\n!  x\nx  x\n>  x\n>  x\n|  more\n`  Done\n"
        );
        assert_eq!(rail.mask("7KQ4-M2XD-9PTM"), "****-****-****");
        assert_eq!(styled(false, true, None).mask(" 7kq4 m2 "), "•••• ••");
    }

    #[test]
    fn colours_are_the_16_colour_palette_and_only_when_asked() {
        let plain = drawn(&styled(false, true, None), |rail, out| {
            rail.step(out, Mark::Error, "run `this`")?;
            rail.rows(out, &[("name", "x".into())])
        });
        assert!(!plain.contains('\x1b'), "{plain:?}");
        let text = drawn(&styled(true, true, None), |rail, out| {
            rail.open(out, "or2-pair", "x")?;
            rail.gap(out)?;
            for mark in [
                Mark::Info,
                Mark::Ok,
                Mark::Warn,
                Mark::Error,
                Mark::Prompt,
                Mark::Answered,
            ] {
                rail.step(out, mark, "x")?;
            }
            rail.step(out, Mark::Info, "run `apt install\ntmux` now")?;
            rail.quiet(out, "aside")?;
            rail.close(out, "Done")
        });
        assert_eq!(
            text,
            "\x1b[2m┌\x1b[0m  or2-pair \x1b[2m· x\x1b[0m\n\
             \x1b[2m│\x1b[0m\n\
             \x1b[34m●\x1b[0m  x\n\
             \x1b[32m✔\x1b[0m  x\n\
             \x1b[33m▲\x1b[0m  x\n\
             \x1b[31m■\x1b[0m  x\n\
             \x1b[36m◆\x1b[0m  x\n\
             \x1b[32m◇\x1b[0m  x\n\
             \x1b[34m●\x1b[0m  run \x1b[36m`apt install\x1b[0m\n\
             \x1b[2m│\x1b[0m  \x1b[36mtmux`\x1b[0m now\n\
             \x1b[2m│\x1b[0m  \x1b[2maside\x1b[0m\n\
             \x1b[2m└\x1b[0m  Done\n"
        );
        // Only SGR 0, 2 and 31-36: no 256-colour or true-colour sequences.
        for sequence in text.split('\x1b').skip(1) {
            let code = &sequence[1..sequence.find('m').unwrap()];
            assert!(
                ["0", "2", "31", "32", "33", "34", "36"].contains(&code),
                "{code}"
            );
        }
    }

    #[test]
    fn long_lines_wrap_with_the_rail_continued_and_words_whole() {
        let rail = styled(false, true, Some(30));
        let text = drawn(&rail, |rail, out| {
            rail.step(
                out,
                Mark::Warn,
                "the quick brown fox jumps over the lazy dog\nrun /a/very/long/path/that/does/not/fit/anywhere now\nor run:\n`sudo a command that is longer than the terminal`",
            )
        });
        assert_eq!(
            text,
            "▲  the quick brown fox jumps\n│  over the lazy dog\n│  run\n│  /a/very/long/path/that/does/not/fit/anywhere\n│  now\n│  or run:\n│  `sudo a command that is longer than the terminal`\n"
        );
        for line in text.lines() {
            assert!(
                line.chars().count() <= 30 || !line[5..].contains(' ') || line.contains('`'),
                "{line}"
            );
        }
        // Rows wrap under their value, not under the label.
        let rows = drawn(&styled(false, true, Some(40)), |rail, out| {
            rail.rows(
                out,
                &[
                    ("name", "Test Host".into()),
                    ("addresses", "192.0.2.1 (LAN)\n198.51.100.7 (public)".into()),
                    ("key", "ssh-ed25519 SHA256:abc (from a file)".into()),
                ],
            )
        });
        assert_eq!(
            rows,
            "│  name       Test Host\n│  addresses  192.0.2.1 (LAN)\n│             198.51.100.7 (public)\n│  key        ssh-ed25519 SHA256:abc\n│             (from a file)\n"
        );
        // Not a terminal: one line however long.
        let long = "word ".repeat(40);
        let once = drawn(&styled(false, true, None), |rail, out| {
            rail.step(out, Mark::Info, &long)
        });
        assert_eq!(once.lines().count(), 1);
    }

    #[test]
    fn wrap_keeps_inner_spacing_and_drops_spaces_at_breaks() {
        assert_eq!(wrap("a  b", Some(10)), ["a  b"]);
        assert_eq!(wrap("aaaa bbbb cccc", Some(9)), ["aaaa bbbb", "cccc"]);
        assert_eq!(wrap("aaaa  bbbb", Some(5)), ["aaaa", "bbbb"]);
        assert_eq!(wrap("", Some(5)), [""]);
        assert_eq!(wrap("x", None), ["x"]);
    }

    #[test]
    fn blocks_and_bare_lines_are_drawn_as_they_are() {
        let rail = styled(true, true, Some(20));
        let text = drawn(&rail, |rail, out| {
            rail.block(
                out,
                "\x1b[30;107m▀▄ a long line that is not wrapped\x1b[0m\n",
            )?;
            rail.bare(out, "or2-pair:2?name=a long code")
        });
        assert_eq!(
            text,
            "\x1b[2m│\x1b[0m  \x1b[30;107m▀▄ a long line that is not wrapped\x1b[0m\nor2-pair:2?name=a long code\n"
        );
    }

    #[test]
    fn a_question_off_a_terminal_is_answered_on_its_line() {
        let rail = styled(false, true, None);
        let text = drawn(&rail, |rail, out| {
            rail.ask(out, "Code shown on your phone", "hidden as you type")?;
            rail.answer(
                out,
                Mark::Answered,
                "Code shown on your phone",
                "••••",
                false,
            )
        });
        assert_eq!(text, "◆  Code shown on your phone\n│  ••••\n");
    }

    #[test]
    fn a_question_on_a_terminal_is_redrawn_as_answered() {
        let rail = Rail::new(Style {
            color: true,
            unicode: true,
            width: Some(80),
            live: true,
        });
        let asked = drawn(&rail, |rail, out| {
            rail.ask(out, "Code shown on your phone", "hidden as you type")
        });
        assert_eq!(
            asked,
            "\x1b[36m◆\x1b[0m  Code shown on your phone\n\x1b[2m│\x1b[0m  \x1b[2mhidden as you type\x1b[0m\r\x1b[3C"
        );
        let answered = drawn(&rail, |rail, out| {
            rail.answer(
                out,
                Mark::Answered,
                "Code shown on your phone",
                "••••",
                false,
            )
        });
        assert_eq!(
            answered,
            "\r\x1b[1A\x1b[2K\x1b[32m◇\x1b[0m  Code shown on your phone\n\r\x1b[2K\x1b[2m│\x1b[0m  ••••\n"
        );
        // After a stop and `fg` the question may have scrolled away: only the answer's line.
        let moved = drawn(&rail, |rail, out| {
            rail.answer(
                out,
                Mark::Answered,
                "Code shown on your phone",
                "••••",
                true,
            )
        });
        assert_eq!(moved, "\r\x1b[2K\x1b[2m│\x1b[0m  ••••\n");
        // Too narrow for the question on one line: not redrawn either.
        let narrow = Rail::new(Style {
            width: Some(20),
            ..rail.style()
        });
        let text = drawn(&narrow, |rail, out| {
            rail.ask(out, "Code shown on your phone", "hidden as you type")?;
            rail.answer(
                out,
                Mark::Answered,
                "Code shown on your phone",
                "••••",
                false,
            )
        });
        assert!(
            !text.contains("\x1b[1A") && !text.contains("hidden"),
            "{text:?}"
        );
        // The note for an ending signal: the hint's line cleared, the end of the rail.
        assert_eq!(
            String::from_utf8(rail.ending_note("Cancelled.", "Cancelled")).unwrap(),
            "\r\x1b[2K\x1b[2m│\x1b[0m  Cancelled.\n\x1b[2m│\x1b[0m\n\x1b[2m└\x1b[0m  Cancelled\n"
        );
        // Off a terminal it completes the answer's line.
        assert_eq!(
            String::from_utf8(styled(false, true, None).ending_note("Cancelled.", "Cancelled"))
                .unwrap(),
            "Cancelled.\n│\n└  Cancelled\n"
        );
    }

    #[test]
    fn the_style_follows_the_terminal_the_locale_and_no_color() {
        let utf8 = [("LANG", "en_US.UTF-8")];
        let style = Style::from_env(true, env(&utf8), || Some(100));
        assert_eq!(
            style,
            Style {
                color: true,
                unicode: true,
                width: Some(100),
                live: cfg!(unix),
            }
        );
        // Not a terminal: no colour, no width, nothing redrawn; the glyphs still follow the locale.
        let piped = Style::from_env(false, env(&utf8), || panic!("not asked off a terminal"));
        assert_eq!(
            piped,
            Style {
                color: false,
                unicode: true,
                width: None,
                live: false,
            }
        );
        // NO_COLOR (any value but empty) turns colour off, not the rest.
        let no_color = Style::from_env(true, env(&[("NO_COLOR", "1"), utf8[0]]), || Some(80));
        assert!(!no_color.color && no_color.unicode && no_color.width == Some(80));
        let empty = Style::from_env(true, env(&[("NO_COLOR", ""), utf8[0]]), || Some(80));
        assert!(empty.color);
        // TERM=dumb: no colour and no cursor movement.
        let dumb = Style::from_env(true, env(&[("TERM", "dumb"), utf8[0]]), || Some(80));
        assert!(!dumb.color && !dumb.live);
        // The width: the terminal's, else COLUMNS, else 80.
        let columns = Style::from_env(true, env(&[("COLUMNS", "132")]), || None);
        assert_eq!(columns.width, Some(132));
        assert_eq!(Style::from_env(true, env(&[]), || None).width, Some(80));
        // The flags.
        let flagged = style.with_flags(true, true);
        assert!(!flagged.color && !flagged.unicode && flagged.width == Some(100));
        assert_eq!(style.with_flags(false, false), style);
    }

    #[test]
    fn unicode_only_in_a_utf8_locale() {
        for (vars, utf8) in [
            (&[("LANG", "en_US.UTF-8")][..], true),
            (&[("LANG", "de_DE.utf8")], true),
            (&[("LC_CTYPE", "UTF-8")], true),
            (&[("LC_ALL", "C.UTF-8"), ("LANG", "C")], true),
            // LC_ALL wins over LANG, LC_CTYPE too.
            (&[("LC_ALL", "C"), ("LANG", "en_US.UTF-8")], false),
            (&[("LC_CTYPE", "POSIX"), ("LANG", "en_US.UTF-8")], false),
            // An empty value is not set.
            (&[("LC_ALL", ""), ("LANG", "en_US.UTF-8")], true),
            (&[("LANG", "en_US.ISO-8859-1")], false),
            (&[("LANG", "C")], false),
            (&[], false),
        ] {
            assert_eq!(utf8_locale(env(vars)), utf8, "{vars:?}");
            let style = Style::from_env(false, env(vars), || None);
            assert_eq!(style.unicode, utf8, "{vars:?}");
        }
    }
}
