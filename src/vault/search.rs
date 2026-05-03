//! Frontmatter-aware vault search.
//!
//! Mirrors `_vault.py:search`. Result shape matches the Python JSON output
//! field-for-field, including the `type` collapse rule: bare string when one
//! type, bracketed `[a, b]` when multi-type, empty string when no type.

use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, FixedOffset};
use serde::Serialize;

use crate::project_docs;
use crate::vault::frontmatter::{note_types, read_note};
use crate::vault::timestamps;
use crate::vault::walk::collect_md_files;

#[derive(Debug, Default, Clone, Copy)]
pub struct SearchOpts<'a> {
    pub type_: Option<&'a str>,
    pub path_prefix: Option<&'a str>,
    pub keywords: Option<&'a str>,
    pub created_after: Option<&'a str>,
    pub created_before: Option<&'a str>,
    pub updated_after: Option<&'a str>,
    pub updated_before: Option<&'a str>,
    pub limit: usize,
    pub project_vault: Option<&'a Path>,
}

#[derive(Debug, Serialize)]
pub struct SearchHit {
    pub corpus: String,
    pub path: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub types: Vec<String>,
    pub description: String,
    pub project: String,
    pub created_at: String,
    pub created_by: String,
    pub updated_at: String,
    pub updated_by: String,
}

pub fn search(vault: &Path, opts: SearchOpts<'_>) -> Result<Vec<SearchHit>> {
    let created_after = opts.created_after.and_then(timestamps::parse_filter);
    let created_before = opts.created_before.and_then(timestamps::parse_filter);
    let updated_after = opts.updated_after.and_then(timestamps::parse_filter);
    let updated_before = opts.updated_before.and_then(timestamps::parse_filter);

    let kw_terms: Vec<String> = opts
        .keywords
        .map(|k| {
            k.split_whitespace()
                .map(|t| t.to_lowercase())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let mut hits: Vec<(i64, SearchHit)> = Vec::new();

    let bounds = TimeBounds {
        created_after: created_after.as_ref(),
        created_before: created_before.as_ref(),
        updated_after: updated_after.as_ref(),
        updated_before: updated_before.as_ref(),
    };

    score_corpus(
        "personal",
        vault,
        &collect_md_files(vault),
        opts.type_,
        opts.path_prefix,
        &kw_terms,
        bounds,
        &mut hits,
    );

    if let Some(pv) = opts.project_vault {
        let pv_files = project_docs::enumerate_project_docs(pv);
        score_corpus(
            "project",
            pv,
            &pv_files,
            opts.type_,
            opts.path_prefix,
            &kw_terms,
            bounds,
            &mut hits,
        );
    }

    // Python: hits.sort(key=lambda x: (-x[0], x[1]["corpus"], x[1]["path"]))
    hits.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.corpus.cmp(&b.1.corpus))
            .then_with(|| a.1.path.cmp(&b.1.path))
    });

    Ok(hits.into_iter().take(opts.limit).map(|(_, h)| h).collect())
}

#[derive(Debug, Default, Clone, Copy)]
struct TimeBounds<'a> {
    created_after: Option<&'a DateTime<FixedOffset>>,
    created_before: Option<&'a DateTime<FixedOffset>>,
    updated_after: Option<&'a DateTime<FixedOffset>>,
    updated_before: Option<&'a DateTime<FixedOffset>>,
}

#[allow(clippy::too_many_arguments)]
fn score_corpus(
    corpus: &str,
    root: &Path,
    files: &[PathBuf],
    type_: Option<&str>,
    path_prefix: Option<&str>,
    kw_terms: &[String],
    bounds: TimeBounds<'_>,
    hits: &mut Vec<(i64, SearchHit)>,
) {
    for f in files {
        let rel = match f.strip_prefix(root) {
            Ok(r) => r.to_path_buf(),
            Err(_) => continue,
        };
        let rel_str = rel.to_string_lossy().to_string();

        if let Some(prefix) = path_prefix {
            let normalized = prefix.trim_matches('/');
            if !rel_str.starts_with(normalized) {
                continue;
            }
        }

        let (fm, body) = read_note(f);
        let fm_ref = fm.as_ref();
        let types = note_types(fm_ref);

        if let Some(t) = type_ {
            if !types.iter().any(|x| x == t) {
                continue;
            }
        }

        if bounds.created_after.is_some() || bounds.created_before.is_some() {
            // Prefer `created_at`; fall back to legacy `created` so unmigrated
            // notes still match a date filter.
            let raw = fm_ref
                .and_then(|m| m.get("created_at").or_else(|| m.get("created")))
                .map(|s| s.as_str())
                .unwrap_or("");
            let Some(d) = timestamps::parse_value(raw) else { continue };
            if let Some(a) = bounds.created_after { if &d < a { continue; } }
            if let Some(b) = bounds.created_before { if &d > b { continue; } }
        }

        if bounds.updated_after.is_some() || bounds.updated_before.is_some() {
            let raw = fm_ref
                .and_then(|m| m.get("updated_at"))
                .map(|s| s.as_str())
                .unwrap_or("");
            let Some(d) = timestamps::parse_value(raw) else { continue };
            if let Some(a) = bounds.updated_after { if &d < a { continue; } }
            if let Some(b) = bounds.updated_before { if &d > b { continue; } }
        }

        let score: i64 = if !kw_terms.is_empty() {
            // Python: haystack = rel_str + "\n" + " ".join(fm.values()) + "\n" + body
            // BTreeMap's value iteration is sorted-by-key; Python's dict is
            // insertion-ordered. Both yield the same characters, so substring
            // counts are identical.
            let mut hay = String::with_capacity(rel_str.len() + body.len() + 64);
            hay.push_str(&rel_str);
            hay.push('\n');
            if let Some(m) = fm_ref {
                let mut first = true;
                for v in m.values() {
                    if !first { hay.push(' '); }
                    hay.push_str(v);
                    first = false;
                }
            }
            hay.push('\n');
            hay.push_str(&body);
            let hay = hay.to_lowercase();
            let mut s: i64 = 0;
            for t in kw_terms {
                s += count_substr(&hay, t) as i64;
            }
            if s == 0 { continue; }
            s
        } else {
            1
        };

        let type_field = match types.len() {
            0 => String::new(),
            1 => types[0].clone(),
            _ => format!("[{}]", types.join(", ")),
        };

        let description = fm_ref.and_then(|m| m.get("description")).cloned().unwrap_or_default();
        let project = fm_ref.and_then(|m| m.get("project")).cloned().unwrap_or_default();
        let created_at = fm_ref
            .and_then(|m| m.get("created_at").or_else(|| m.get("created")))
            .cloned()
            .unwrap_or_default();
        let created_by = fm_ref.and_then(|m| m.get("created_by")).cloned().unwrap_or_default();
        let updated_at = fm_ref.and_then(|m| m.get("updated_at")).cloned().unwrap_or_default();
        let updated_by = fm_ref.and_then(|m| m.get("updated_by")).cloned().unwrap_or_default();

        hits.push((
            score,
            SearchHit {
                corpus: corpus.to_string(),
                path: f.to_string_lossy().to_string(),
                type_: type_field,
                types,
                description,
                project,
                created_at,
                created_by,
                updated_at,
                updated_by,
            },
        ));
    }
}


/// Count non-overlapping occurrences of `needle` in `haystack`.
/// Matches Python's `str.count(sub)`.
fn count_substr(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let mut count = 0usize;
    let mut start = 0usize;
    while let Some(pos) = haystack[start..].find(needle) {
        count += 1;
        start += pos + needle.len();
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_basic() {
        assert_eq!(count_substr("abcabcabc", "abc"), 3);
        assert_eq!(count_substr("aaaa", "aa"), 2); // non-overlapping, like Python
        assert_eq!(count_substr("abc", "z"), 0);
        assert_eq!(count_substr("abc", ""), 0);
    }
}
