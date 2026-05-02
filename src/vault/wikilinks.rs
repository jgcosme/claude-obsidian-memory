//! Backlink scan: find every note in the vault that links to TARGET via
//! `[[wikilink]]`, including bare-basename links when unambiguous.
//!
//! Mirrors `_vault.py:incoming_wikilinks` and `WIKILINK_RE`.

use std::collections::HashMap;
use std::path::Path;

use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;

use crate::vault::frontmatter::FRONTMATTER_RE;
use crate::vault::walk::collect_md_files;

static WIKILINK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[\[([^\]]+)\]\]").expect("wikilink regex"));

#[derive(Debug, Serialize)]
pub struct IncomingHit {
    pub source: String,
    pub raw_link: String,
    pub kind: String,
}

pub fn incoming_wikilinks(vault: &Path, target_relpath: &str) -> Vec<IncomingHit> {
    let target = target_relpath
        .replace('\\', "/")
        .trim()
        .trim_start_matches("./")
        .trim_end_matches('/')
        .to_string();
    let target_md = if target.ends_with(".md") { target.clone() } else { format!("{target}.md") };
    let target_stem = Path::new(&target_md).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();

    let md_files = collect_md_files(vault);
    let mut basename_counts: HashMap<String, usize> = HashMap::new();
    for f in &md_files {
        let stem = f.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        *basename_counts.entry(stem).or_insert(0) += 1;
    }

    let mut results: Vec<IncomingHit> = Vec::new();
    for f in &md_files {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let body = strip_frontmatter(&text);
        for cap in WIKILINK_RE.captures_iter(body) {
            let raw = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            // strip alias |, anchor #, block ^
            let link = raw
                .split('|').next().unwrap_or("")
                .split('#').next().unwrap_or("")
                .split('^').next().unwrap_or("")
                .trim();
            if link.is_empty() { continue; }
            let link_norm = link
                .replace('\\', "/")
                .trim_start_matches("./")
                .trim_end_matches('/')
                .to_string();
            let link_md = if link_norm.ends_with(".md") { link_norm.clone() } else { format!("{link_norm}.md") };

            let kind: Option<&str> = if link_norm.contains('/') {
                if link_md == target_md
                    || link_md.ends_with(&format!("/{target_md}"))
                    || target_md.ends_with(&format!("/{link_md}"))
                {
                    Some("path-qualified")
                } else { None }
            } else {
                let link_stem = Path::new(&link_md).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                if link_stem == target_stem && basename_counts.get(&target_stem).copied().unwrap_or(0) == 1 {
                    Some("bare")
                } else { None }
            };

            if let Some(kind) = kind {
                let source = relative_string(f, vault);
                results.push(IncomingHit {
                    source,
                    raw_link: raw.to_string(),
                    kind: kind.to_string(),
                });
            }
        }
    }
    results
}

fn strip_frontmatter(text: &str) -> &str {
    if let Some(m) = FRONTMATTER_RE.find(text) {
        &text[m.end()..]
    } else {
        text
    }
}

fn relative_string(file: &Path, vault: &Path) -> String {
    file.strip_prefix(vault)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| file.to_string_lossy().to_string())
}

