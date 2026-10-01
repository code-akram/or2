//! JSON text in the exact shape the checked-in generated files use: object keys sorted, the
//! `", "` / `": "` separators of a compact dump, and an optional per-level indent. serde_json's own
//! serializers differ (no spaces when compact, escapes of their own), so the generated files would
//! not stay stable if they went through them.

use serde_json::Value;

/// Serializes `value` with every object's keys sorted. `indent` is the number of spaces per
/// level (`None` for one line); `ascii` escapes everything outside printable ASCII as `\uXXXX`
/// (surrogate pairs above the BMP).
pub fn dumps(value: &Value, indent: Option<usize>, ascii: bool) -> String {
    let mut out = String::new();
    write_value(&mut out, value, indent, ascii, 0);
    out
}

fn write_value(out: &mut String, value: &Value, indent: Option<usize>, ascii: bool, level: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_string(out, s, ascii),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                separator(out, i, indent, level + 1);
                write_value(out, item, indent, ascii, level + 1);
            }
            close(out, ']', indent, level);
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (i, (key, item)) in entries.into_iter().enumerate() {
                separator(out, i, indent, level + 1);
                write_string(out, key, ascii);
                out.push_str(": ");
                write_value(out, item, indent, ascii, level + 1);
            }
            close(out, '}', indent, level);
        }
    }
}

fn separator(out: &mut String, index: usize, indent: Option<usize>, level: usize) {
    match indent {
        Some(width) => {
            if index > 0 {
                out.push(',');
            }
            out.push('\n');
            out.extend(std::iter::repeat_n(' ', width * level));
        }
        None => {
            if index > 0 {
                out.push_str(", ");
            }
        }
    }
}

fn close(out: &mut String, bracket: char, indent: Option<usize>, level: usize) {
    if let Some(width) = indent {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', width * level));
    }
    out.push(bracket);
}

/// A JSON string literal: `"`, `\` and the C0 controls are escaped, and with `ascii` so is
/// everything that is not printable ASCII.
pub fn write_string(out: &mut String, s: &str, ascii: bool) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (ascii && !(' '..='~').contains(&c)) => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A string literal with non-ASCII text kept as is.
pub fn string(s: &str) -> String {
    let mut out = String::new();
    write_string(&mut out, s, false);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compact_sorts_keys_and_spaces_separators() {
        let value = json!({"b": [1, 2], "a": {"y": null, "x": true}, "c": "é\n"});
        assert_eq!(
            dumps(&value, None, false),
            "{\"a\": {\"x\": true, \"y\": null}, \"b\": [1, 2], \"c\": \"é\\n\"}"
        );
    }

    #[test]
    fn indent_nests_and_keeps_empty_containers_short() {
        let value = json!({"k": [1, {"z": []}], "e": {}});
        assert_eq!(
            dumps(&value, Some(1), true),
            "{\n \"e\": {},\n \"k\": [\n  1,\n  {\n   \"z\": []\n  }\n ]\n}"
        );
    }

    #[test]
    fn ascii_mode_escapes_non_printable_text() {
        let value = json!("a\u{e9}\u{1f600}\u{7f}\u{1}\"\\");
        assert_eq!(
            dumps(&value, None, true),
            "\"a\\u00e9\\ud83d\\ude00\\u007f\\u0001\\\"\\\\\""
        );
        assert_eq!(
            dumps(&value, None, false),
            "\"a\u{e9}\u{1f600}\u{7f}\\u0001\\\"\\\\\""
        );
    }
}
