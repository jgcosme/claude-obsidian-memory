//! JSON formatting helpers tuned for byte-for-byte parity with Python's
//! `json.dumps(default=...)`.
//!
//! Python defaults to `ensure_ascii=True`, escaping every non-ASCII char as
//! `\uXXXX`. serde_json keeps raw UTF-8. This module provides drop-in
//! replacements that perform the post-pass escape so the parity harness can
//! diff stdout byte-for-byte.

use serde::Serialize;

/// Equivalent of Python `json.dumps(value, indent=2)` (ensure_ascii=True).
pub fn to_string_pretty_ascii<T: Serialize>(value: &T) -> serde_json::Result<String> {
    let raw = serde_json::to_string_pretty(value)?;
    Ok(escape_ascii(&raw))
}

/// Equivalent of Python `json.dumps(value)` (compact, ensure_ascii=True, but
/// uses default `", "` / `": "` separators rather than serde's no-space form).
#[allow(dead_code)]
pub fn to_string_compact_ascii<T: Serialize>(value: &T) -> serde_json::Result<String> {
    let raw = serde_json::to_string(value)?;
    let with_spaces = expand_separators(&raw);
    Ok(escape_ascii(&with_spaces))
}

/// Re-add Python's default `", "` and `": "` separators to a serde_json
/// compact string. Walks the byte stream tracking whether we're inside a
/// string literal so we don't munge string contents.
#[allow(dead_code)]
fn expand_separators(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(bytes.len() + 16);
    let mut in_str = false;
    let mut esc = false;
    for &b in bytes {
        if in_str {
            out.push(b as char);
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => {
                in_str = true;
                out.push('"');
            }
            b',' => out.push_str(", "),
            b':' => out.push_str(": "),
            _ => out.push(b as char),
        }
    }
    out
}

/// Replace non-ASCII chars with `\uXXXX` (or surrogate pairs for non-BMP),
/// matching Python's `ensure_ascii=True`. Walks the string char-by-char; the
/// JSON syntax is pure ASCII, so non-ASCII chars only ever appear inside
/// string literals where this escape is valid.
pub fn escape_ascii(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let cp = c as u32;
        if cp < 0x80 {
            out.push(c);
        } else if cp <= 0xFFFF {
            out.push_str(&format!("\\u{:04x}", cp));
        } else {
            // Surrogate pair.
            let v = cp - 0x10000;
            let hi = 0xD800 + (v >> 10);
            let lo = 0xDC00 + (v & 0x3FF);
            out.push_str(&format!("\\u{:04x}\\u{:04x}", hi, lo));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn em_dash_escapes() {
        assert_eq!(escape_ascii("—"), "\\u2014");
    }

    #[test]
    fn non_bmp_surrogate_pair() {
        // U+1F600 GRINNING FACE → 😀
        assert_eq!(escape_ascii("😀"), "\\ud83d\\ude00");
    }

    #[test]
    fn ascii_passthrough() {
        assert_eq!(escape_ascii("hello world"), "hello world");
    }

    #[test]
    fn separators_expand_outside_strings() {
        assert_eq!(expand_separators(r#"{"a":1,"b":2}"#), r#"{"a": 1, "b": 2}"#);
    }

    #[test]
    fn separators_preserve_string_contents() {
        // colons and commas inside strings stay untouched
        assert_eq!(expand_separators(r#"{"k":"a:b,c"}"#), r#"{"k": "a:b,c"}"#);
    }
}
