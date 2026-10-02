//! Terminal QR codes.
//!
//! The default is UTF-8 half blocks (two module rows per text row). With colour the code is
//! drawn black on white whatever the terminal's theme (SGR 30/107), so it scans on dark and
//! light terminals alike; without colour (piped output, `NO_COLOR`, `--no-color`) the blocks
//! are drawn for a dark terminal, or for a light one with `--invert`. `--ascii` draws every
//! module as two ASCII characters (`##` and spaces, or two spaces on a coloured background):
//! twice as wide, but it works wherever UTF-8 does not.

use qrcode::{Color, EcLevel, QrCode};

/// Light modules around the code. The standard asks for four; two scan fine on a screen and keep
/// the code narrow enough for an 80-column terminal.
pub const QUIET_ZONE: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QrStyle {
    /// ASCII characters only (two per module) instead of UTF-8 half blocks.
    pub ascii: bool,
    /// ANSI colours: black on white, independent of the terminal's theme.
    pub color: bool,
    /// Without colour: draw for a light terminal instead of a dark one.
    pub invert: bool,
}

/// The modules of the code for `text`, with the quiet zone: `width` and row-major darkness.
pub fn modules(text: &str) -> Result<(usize, Vec<bool>), qrcode::types::QrError> {
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::L)?;
    let size = code.width();
    let width = size + 2 * QUIET_ZONE;
    let mut grid = vec![false; width * width];
    for y in 0..size {
        for x in 0..size {
            grid[(y + QUIET_ZONE) * width + x + QUIET_ZONE] = code[(x, y)] == Color::Dark;
        }
    }
    Ok((width, grid))
}

/// Text lines (each ending in a newline) that draw the code.
pub fn render(text: &str, style: QrStyle) -> Result<String, qrcode::types::QrError> {
    let (width, grid) = modules(text)?;
    Ok(draw(width, &grid, style))
}

/// How many terminal columns a code of `width` modules needs in `style`.
pub fn columns(width: usize, style: QrStyle) -> usize {
    if style.ascii { width * 2 } else { width }
}

const RESET: &str = "\x1b[0m";
const BLACK_ON_WHITE: &str = "\x1b[30;107m";
const BLACK: &str = "\x1b[40m";
const WHITE: &str = "\x1b[107m";

pub fn draw(width: usize, grid: &[bool], style: QrStyle) -> String {
    let dark = |x: usize, y: usize| y < width && grid[y * width + x];
    let mut out = String::new();
    if style.ascii {
        for y in 0..width {
            if style.color {
                // Runs of one colour share one escape.
                let mut current: Option<bool> = None;
                for x in 0..width {
                    let is_dark = dark(x, y);
                    if current != Some(is_dark) {
                        out.push_str(if is_dark { BLACK } else { WHITE });
                        current = Some(is_dark);
                    }
                    out.push_str("  ");
                }
                out.push_str(RESET);
            } else {
                for x in 0..width {
                    // A dark terminal shows text as light on dark: draw the light modules.
                    let ink = dark(x, y) == style.invert;
                    out.push_str(if ink { "##" } else { "  " });
                }
            }
            out.push('\n');
        }
        return out;
    }
    for pair in (0..width).step_by(2) {
        if style.color {
            out.push_str(BLACK_ON_WHITE);
        }
        for x in 0..width {
            let (top, bottom) = (dark(x, pair), dark(x, pair + 1));
            // The block drawn is the *ink*: black with colour (a dark module is ink), or the
            // terminal's foreground, which is the light colour on a dark theme.
            let (top, bottom) = if style.color || style.invert {
                (top, bottom)
            } else {
                (!top, !bottom)
            };
            out.push(match (top, bottom) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        if style.color {
            out.push_str(RESET);
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "or2-pair:1?name=test&user=u&port=22&a=192.168.1.2";

    fn style(ascii: bool, color: bool, invert: bool) -> QrStyle {
        QrStyle {
            ascii,
            color,
            invert,
        }
    }

    #[test]
    fn the_matrix_has_a_light_quiet_zone_and_finder_patterns() {
        let (width, grid) = modules(TEXT).unwrap();
        for i in 0..width {
            for edge in 0..QUIET_ZONE {
                assert!(!grid[edge * width + i], "top edge");
                assert!(!grid[(width - 1 - edge) * width + i], "bottom edge");
                assert!(!grid[i * width + edge], "left edge");
                assert!(!grid[i * width + width - 1 - edge], "right edge");
            }
        }
        // The top-left finder pattern: a 7x7 ring, so its corner and centre are dark.
        let q = QUIET_ZONE;
        assert!(grid[q * width + q]);
        assert!(grid[(q + 3) * width + q + 3]);
        assert!(!grid[(q + 1) * width + q + 1]);
    }

    #[test]
    fn unicode_with_colour_is_black_on_white_and_resets_each_line() {
        let text = render(TEXT, style(false, true, false)).unwrap();
        let (width, _) = modules(TEXT).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), width.div_ceil(2));
        for line in lines {
            assert!(line.starts_with(BLACK_ON_WHITE) && line.ends_with(RESET));
            let body = &line[BLACK_ON_WHITE.len()..line.len() - RESET.len()];
            assert_eq!(body.chars().count(), width);
            assert!(body.chars().all(|c| " ▀▄█".contains(c)));
        }
        // The quiet zone is white: the first row is all spaces.
        let first = text.lines().next().unwrap();
        assert!(first[BLACK_ON_WHITE.len()..].starts_with("  "));
    }

    #[test]
    fn plain_unicode_is_inverted_for_a_dark_terminal_and_not_for_a_light_one() {
        let dark_terminal = render(TEXT, style(false, false, false)).unwrap();
        let light_terminal = render(TEXT, style(false, false, true)).unwrap();
        assert!(!dark_terminal.contains('\x1b'));
        // Where one has ink the other has none.
        let first_dark = dark_terminal.lines().next().unwrap();
        let first_light = light_terminal.lines().next().unwrap();
        assert!(
            first_dark.chars().all(|c| c == '█'),
            "quiet zone is lit on a dark terminal"
        );
        assert!(first_light.chars().all(|c| c == ' '));
    }

    #[test]
    fn ascii_is_two_characters_per_module_and_plain_ascii_without_colour() {
        let (width, _) = modules(TEXT).unwrap();
        let text = render(TEXT, style(true, false, false)).unwrap();
        assert!(text.is_ascii());
        for line in text.lines() {
            assert_eq!(line.len(), width * 2);
        }
        assert_eq!(text.lines().count(), width);
        assert_eq!(columns(width, style(true, false, false)), width * 2);
        assert_eq!(columns(width, style(false, false, false)), width);
        let coloured = render(TEXT, style(true, true, false)).unwrap();
        assert!(coloured.contains("\x1b[40m") && coloured.contains("\x1b[107m"));
        assert!(coloured.lines().all(|line| line.ends_with(RESET)));
    }

    /// What a camera sees of `drawing` on a terminal: one row of modules per text row (two for
    /// half blocks), `true` for a dark module. `dark_terminal` is the background around it.
    fn seen(drawing: &str, style: QrStyle, dark_terminal: bool) -> Vec<Vec<bool>> {
        let mut rows = Vec::new();
        for line in drawing.lines() {
            if style.ascii && style.color {
                let mut row = Vec::new();
                let mut dark = false;
                let mut rest = line;
                while !rest.is_empty() {
                    if let Some(after) = rest.strip_prefix('\x1b') {
                        let end = after.find('m').unwrap();
                        match &after[..=end] {
                            "[40m" => dark = true,
                            "[107m" => dark = false,
                            _ => {}
                        }
                        rest = &after[end + 1..];
                    } else {
                        assert!(rest.starts_with("  "), "{rest:?}");
                        row.push(dark);
                        rest = &rest[2..];
                    }
                }
                rows.push(row);
            } else if style.ascii {
                let chars: Vec<char> = line.chars().collect();
                // Ink is the terminal's foreground: light on a dark terminal.
                rows.push(
                    chars
                        .chunks(2)
                        .map(|pair| (pair[0] == '#') != dark_terminal)
                        .collect(),
                );
            } else {
                let plain: String = line.replace(BLACK_ON_WHITE, "").replace(RESET, "");
                // With colour ink is black; without, the terminal's foreground.
                let ink_is_dark = style.color || !dark_terminal;
                let (mut top, mut bottom) = (Vec::new(), Vec::new());
                for c in plain.chars() {
                    let (t, b) = match c {
                        '█' => (true, true),
                        '▀' => (true, false),
                        '▄' => (false, true),
                        _ => (false, false),
                    };
                    top.push(t == ink_is_dark);
                    bottom.push(b == ink_is_dark);
                }
                rows.push(top);
                rows.push(bottom);
            }
        }
        rows
    }

    /// Reads `rows` with an independent decoder (rqrr), with a margin of the terminal's
    /// background around them.
    fn decode(rows: &[Vec<bool>], dark_terminal: bool) -> String {
        let (margin, scale) = (4, 4);
        let width = rows.iter().map(Vec::len).max().unwrap() + 2 * margin;
        let height = rows.len() + 2 * margin;
        let dark_at = |x: usize, y: usize| {
            y.checked_sub(margin)
                .and_then(|y| rows.get(y))
                .and_then(|row| x.checked_sub(margin).and_then(|x| row.get(x)))
                .copied()
                .unwrap_or(dark_terminal)
        };
        let mut image =
            rqrr::PreparedImage::prepare_from_greyscale(width * scale, height * scale, |x, y| {
                if dark_at(x / scale, y / scale) {
                    0
                } else {
                    255
                }
            });
        let grids = image.detect_grids();
        assert_eq!(grids.len(), 1, "one QR code in the drawing");
        grids[0].decode().expect("the QR decodes").1
    }

    #[test]
    fn every_style_decodes_on_the_rail() {
        use crate::rail::{Rail, Style};
        let code = format!("or2-pair:2?{}", "name=Test%20Host&a=192.0.2.1&".repeat(6));
        for (ascii, color, invert) in [
            (false, false, false),
            (false, false, true),
            (false, true, false),
            (true, false, false),
            (true, false, true),
            (true, true, false),
        ] {
            let style = style(ascii, color, invert);
            let rail = Rail::new(Style {
                color,
                unicode: !ascii,
                ..Style::plain()
            });
            let mut out = Vec::new();
            rail.block(&mut out, &render(&code, style).unwrap())
                .unwrap();
            let drawn = String::from_utf8(out).unwrap();
            // Each row after the rail's gutter, the code exactly as drawn.
            let gutter = if color {
                if ascii {
                    "\x1b[2m|\x1b[0m  "
                } else {
                    "\x1b[2m│\x1b[0m  "
                }
            } else if ascii {
                "|  "
            } else {
                "│  "
            };
            let rows: Vec<&str> = drawn
                .lines()
                .map(|line| line.strip_prefix(gutter).expect(line))
                .collect();
            assert_eq!(rows.join("\n") + "\n", render(&code, style).unwrap());
            // On the terminal it was drawn for: a dark one, or a light one with --invert.
            let dark_terminal = !invert;
            let modules = seen(&rows.join("\n"), style, dark_terminal);
            assert_eq!(decode(&modules, dark_terminal), code, "{style:?}");
        }
    }

    #[test]
    fn a_thousand_bytes_still_makes_a_code() {
        let long = format!("or2-pair:1?{}", "a".repeat(1000));
        let (width, _) = modules(&long).unwrap();
        assert!(width < 150, "{width}");
    }
}
