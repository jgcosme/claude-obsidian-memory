//! YAML-ish frontmatter parser tolerant of BOM/CRLF.
//!
//! Mirrors `_vault.py:parse_frontmatter`, `note_types`, `read_note`, and the
//! `FRONTMATTER_RE` regex. The dict shape is preserved (flat `String -> String`,
//! with multi-line YAML block lists folded into the inline `[a, b, c]` form
//! so callers see one consistent representation).

use std::collections::BTreeMap;
use std::path::Path;

use once_cell::sync::Lazy;
use regex::Regex;

/// Match an optional BOM, then a YAML frontmatter block.
///
/// Equivalent to Python's `^﻿?---\s*\r?\n(.*?)\r?\n---\s*\r?\n` with
/// `DOTALL`. `\A` anchors to start of input; `(?s)` makes `.` match newlines.
pub static FRONTMATTER_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?s)\A\u{feff}?---\s*\r?\n(.*?)\r?\n---\s*\r?\n").expect("frontmatter regex")
});

/// Canonical memory types. Defined here (rather than in `init_project_vault`
/// like the Python module) so audit + project-init + frontmatter validation
/// share one source of truth.
pub const VALID_TYPES: &[&str] = &[
    "preference", "reference", "findings", "decision", "learning", "tool", "journal",
];

/// Parse a YAML-ish frontmatter block into a flat map.
///
/// Returns `None` if no frontmatter is present (no leading `---`).
pub fn parse_frontmatter(text: &str) -> Option<BTreeMap<String, String>> {
    let m = FRONTMATTER_RE.captures(text)?;
    let body = m.get(1)?.as_str();

    let mut fm: BTreeMap<String, String> = BTreeMap::new();
    let mut last_key: Option<String> = None;
    let mut pending_block: Vec<String> = Vec::new();

    let flush = |fm: &mut BTreeMap<String, String>, last_key: &Option<String>, pending: &mut Vec<String>| {
        if let Some(k) = last_key {
            if !pending.is_empty() {
                let inner = pending.join(", ");
                fm.insert(k.clone(), format!("[{inner}]"));
            }
        }
        pending.clear();
    };

    for line in body.split('\n') {
        // Python's splitlines drops the trailing \r; do the same.
        let line = line.strip_suffix('\r').unwrap_or(line);
        let stripped = line.trim_start();
        if stripped.starts_with('#') {
            continue;
        }
        // YAML block-list continuation under the previous key
        let indented = matches!(line.chars().next(), Some(' ') | Some('\t'));
        if last_key.is_some() && indented && stripped.starts_with("- ") {
            let item = stripped[2..].trim();
            // Python: .strip().strip('"').strip("'") — sequential, removes
            // ALL leading/trailing quotes of one kind, then the other.
            let item = trim_quotes(item);
            if !item.is_empty() {
                pending_block.push(item.to_string());
            }
            continue;
        }
        flush(&mut fm, &last_key, &mut pending_block);

        if let Some(idx) = line.find(':') {
            let (k, v) = line.split_at(idx);
            // skip the colon itself
            let v = &v[1..];
            let key = k.trim().to_string();
            let val = v.trim().to_string();
            fm.insert(key.clone(), val);
            last_key = Some(key);
        } else {
            last_key = None;
        }
    }
    flush(&mut fm, &last_key, &mut pending_block);
    Some(fm)
}

/// Return the note's `type:` field as an ordered list. Handles bare string,
/// inline list `[a, b]`, and block-list (already folded by `parse_frontmatter`).
pub fn note_types(fm: Option<&BTreeMap<String, String>>) -> Vec<String> {
    let Some(fm) = fm else { return Vec::new() };
    let raw = fm.get("type").map(|s| s.trim()).unwrap_or("");
    if raw.is_empty() {
        return Vec::new();
    }
    if raw.starts_with('[') && raw.ends_with(']') {
        let inner = &raw[1..raw.len() - 1];
        return inner
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| trim_quotes(t).to_string())
            .collect();
    }
    vec![trim_quotes(raw).to_string()]
}

/// Sequentially strip ALL leading/trailing `"` then `'` chars (matches
/// Python's `s.strip('"').strip("'")`).
pub fn trim_quotes(s: &str) -> &str {
    let s = s.trim_matches('"');
    s.trim_matches('\'')
}

/// Read a note: `(frontmatter | None, body without frontmatter)`.
///
/// `OSError`/decode errors fall back to `(None, "")` to match Python — the
/// caller is iterating over a corpus and must not crash on a single bad file.
pub fn read_note(path: &Path) -> (Option<BTreeMap<String, String>>, String) {
    let Ok(bytes) = std::fs::read(path) else {
        return (None, String::new());
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return (None, String::new());
    };
    if let Some(m) = FRONTMATTER_RE.captures(&text) {
        let whole = m.get(0).expect("group 0").as_str();
        let body = text[whole.len()..].to_string();
        let fm = parse_frontmatter(&text);
        (fm, body)
    } else {
        (None, text)
    }
}
