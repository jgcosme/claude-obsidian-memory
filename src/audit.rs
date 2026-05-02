//! Full Obsidian vault integrity audit — port of scripts/audit.py.
//!
//! Reports:
//!   - Frontmatter completeness (type, description, created; + project for project-vault)
//!   - Broken wikilinks (target file not found)
//!   - Orphan notes (no incoming wikilink, excluding README.md)
//!   - Duplicate basenames (bare wikilinks become ambiguous)
//!
//! Known divergence from Python: the optional `pyyaml` deep-validation pass
//! is not ported. Python only runs it when pyyaml happens to be installed,
//! and the parity harness controls fixtures so no malformed YAML appears.
//! Document if a user's live vault relies on it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::Local;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;

use crate::cli::AuditArgs;
use crate::project_docs::enumerate_project_docs;
use crate::vault::frontmatter::{
    FRONTMATTER_RE, VALID_TYPES, note_types, parse_frontmatter,
};
use crate::vault::walk::{absolute, collect_md_files, expand_user, resolve_vault};

static WIKILINK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[\[([^\]]+)\]\]").expect("wikilink regex"));

/// Files Obsidian / docs / git tooling expect at the vault or folder root —
/// they're not "memory notes" and shouldn't be flagged as orphans.
const NAVIGATION_NAMES: &[&str] = &["README.md"];

#[derive(Debug, Default, Serialize)]
struct CorpusReport {
    label: String,
    root: String,
    files_scanned: usize,
    frontmatter_issues: Vec<FmIssue>,
    broken_wikilinks: Vec<BrokenLink>,
    orphan_notes: Vec<String>,
    duplicate_basenames: Vec<DuplicateBasename>,
}

#[derive(Debug, Serialize)]
struct FmIssue {
    file: String,
    issue: String,
}

#[derive(Debug, Serialize)]
struct BrokenLink {
    file: String,
    link: String,
}

#[derive(Debug, Serialize)]
struct DuplicateBasename {
    basename: String,
    paths: Vec<String>,
}

pub fn run(args: AuditArgs) -> Result<i32> {
    let vault = resolve_vault(args.vault.as_deref());
    if !vault.is_dir() {
        eprintln!("vault not found at: {}", vault.display());
        return Ok(1);
    }

    let has_project_vault = args.project_vault.is_some();
    let mut reports: Vec<CorpusReport> = Vec::new();

    reports.push(audit_corpus(
        if has_project_vault { "personal" } else { "" },
        &vault,
        &collect_md_files(&vault),
        false,
    ));

    if let Some(pv) = args.project_vault.as_ref() {
        let project_root = absolute(&expand_user(pv));
        if !project_root.is_dir() {
            eprintln!("project-vault not found at: {}", project_root.display());
            return Ok(1);
        }
        let label = format!(
            "project:{}",
            project_root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
        );
        reports.push(audit_corpus(
            &label,
            &project_root,
            &enumerate_project_docs(&project_root),
            true,
        ));
    }

    let now = Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();

    if args.json {
        // Emit Python's json.dumps(..., indent=2) shape (ensure_ascii, key order).
        // Hand-format keys to preserve insertion order: Python emits
        // {"generated": ..., "corpora": [{label, root, counts, frontmatter_issues,
        // broken_wikilinks, orphan_notes, duplicate_basenames}]}.
        println!("{}", build_audit_json(&now, &reports)?);
        return Ok(0);
    }

    println!("# Vault Audit Report\n");
    for r in &reports {
        let suffix = if r.label.is_empty() { String::new() } else { format!(" ({})", r.label) };
        println!("Corpus{suffix}: `{}`", r.root);
        println!("Files scanned: {}", r.files_scanned);
    }
    println!("Generated: {now}\n");

    for r in &reports {
        print_corpus(r);
    }

    println!("## Summary");
    for r in &reports {
        let suffix = if r.label.is_empty() { String::new() } else { format!(" ({})", r.label) };
        println!(
            "- Corpus{suffix}: {} files, {} fm issues, {} broken links, {} orphans, {} dup basenames",
            r.files_scanned,
            r.frontmatter_issues.len(),
            r.broken_wikilinks.len(),
            r.orphan_notes.len(),
            r.duplicate_basenames.len(),
        );
    }

    let has_issues = reports.iter().any(|r| {
        !r.frontmatter_issues.is_empty()
            || !r.broken_wikilinks.is_empty()
            || !r.orphan_notes.is_empty()
            || !r.duplicate_basenames.is_empty()
    });
    Ok(if has_issues { 1 } else { 0 })
}

fn audit_corpus(label: &str, root: &Path, files: &[PathBuf], project_required: bool) -> CorpusReport {
    // Build basename map AND track insertion order for parity with Python's
    // dict iteration (insertion order = sorted scan order).
    let mut basename_map: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut basename_order: Vec<String> = Vec::new();
    for f in files {
        let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if !basename_map.contains_key(&stem) {
            basename_order.push(stem.clone());
        }
        basename_map.entry(stem).or_default().push(f.clone());
    }
    let all_relpaths: Vec<PathBuf> = files
        .iter()
        .filter_map(|f| f.strip_prefix(root).ok().map(|p| p.to_path_buf()))
        .collect();

    let mut fm_issues: Vec<FmIssue> = Vec::new();
    let mut broken_links: Vec<BrokenLink> = Vec::new();
    let mut referenced: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    for f in files {
        let rel = match f.strip_prefix(root) {
            Ok(r) => r.to_path_buf(),
            Err(_) => continue,
        };
        let rel_str = rel.to_string_lossy().to_string();
        let bytes = match std::fs::read(f) {
            Ok(b) => b,
            Err(_) => {
                fm_issues.push(FmIssue { file: rel_str.clone(), issue: "not utf-8 readable".into() });
                continue;
            }
        };
        let text = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => {
                fm_issues.push(FmIssue { file: rel_str.clone(), issue: "not utf-8 readable".into() });
                continue;
            }
        };

        let fm = parse_frontmatter(&text);
        let is_navigation = f.file_name().map(|n| NAVIGATION_NAMES.iter().any(|nav| *nav == n)).unwrap_or(false);

        if is_navigation {
            // README.md: skip frontmatter checks entirely.
        } else if fm.is_none() {
            fm_issues.push(FmIssue { file: rel_str.clone(), issue: "no frontmatter block".into() });
        } else {
            let fm_ref = fm.as_ref();
            let mut required: Vec<&str> = vec!["type", "description", "created"];
            if project_required {
                required.push("project");
            }
            for k in &required {
                let present = fm_ref.map(|m| m.contains_key(*k)).unwrap_or(false);
                if !present {
                    fm_issues.push(FmIssue { file: rel_str.clone(), issue: format!("missing `{k}`") });
                }
            }
            // Type validity (after the `missing` check so empty + missing both surface).
            if fm_ref.map(|m| m.contains_key("type")).unwrap_or(false) {
                let types = note_types(fm_ref);
                if types.is_empty() {
                    fm_issues.push(FmIssue { file: rel_str.clone(), issue: "empty `type`".into() });
                } else {
                    for t in &types {
                        if !VALID_TYPES.contains(&t.as_str()) {
                            fm_issues.push(FmIssue {
                                file: rel_str.clone(),
                                issue: format!("unknown type `{t}` (valid: {})", VALID_TYPES.join(", ")),
                            });
                        }
                    }
                }
            }
        }

        // Body = text minus the frontmatter block; mirrors Python's regex sub.
        let body = if let Some(m) = FRONTMATTER_RE.find(&text) {
            &text[m.end()..]
        } else {
            text.as_str()
        };

        for target in extract_wikilinks(body) {
            let resolved = resolve_wikilink(&target, root, &basename_map, f, &all_relpaths);
            if resolved.is_empty() {
                broken_links.push(BrokenLink { file: rel_str.clone(), link: target });
            } else {
                for r in resolved {
                    referenced.insert(root.join(r));
                }
            }
        }
    }

    let mut orphans: Vec<String> = Vec::new();
    for f in files {
        let is_navigation = f.file_name().map(|n| NAVIGATION_NAMES.iter().any(|nav| *nav == n)).unwrap_or(false);
        if is_navigation {
            continue;
        }
        if !referenced.contains(f) {
            if let Ok(rel) = f.strip_prefix(root) {
                orphans.push(rel.to_string_lossy().into_owned());
            }
        }
    }

    let mut duplicates: Vec<DuplicateBasename> = Vec::new();
    for stem in &basename_order {
        let paths = basename_map.get(stem).expect("stem in order list must be in map");
        if paths.len() > 1 {
            duplicates.push(DuplicateBasename {
                basename: format!("{stem}.md"),
                paths: paths
                    .iter()
                    .filter_map(|p| p.strip_prefix(root).ok().map(|r| r.to_string_lossy().into_owned()))
                    .collect(),
            });
        }
    }

    CorpusReport {
        label: label.to_string(),
        root: root.to_string_lossy().into_owned(),
        files_scanned: files.len(),
        frontmatter_issues: fm_issues,
        broken_wikilinks: broken_links,
        orphan_notes: orphans,
        duplicate_basenames: duplicates,
    }
}

fn extract_wikilinks(body: &str) -> Vec<String> {
    let mut targets: Vec<String> = Vec::new();
    for cap in WIKILINK_RE.captures_iter(body) {
        let raw = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let target = raw
            .split('|').next().unwrap_or("").trim()
            .split('#').next().unwrap_or("").trim()
            .split('^').next().unwrap_or("").trim();
        if !target.is_empty() {
            targets.push(target.to_string());
        }
    }
    targets
}

fn resolve_wikilink(
    target: &str,
    vault: &Path,
    basename_map: &HashMap<String, Vec<PathBuf>>,
    source: &Path,
    all_relpaths: &[PathBuf],
) -> Vec<PathBuf> {
    let needle = if target.ends_with(".md") { target.to_string() } else { format!("{target}.md") };

    if target.contains('/') {
        // Vault-root resolution.
        let cand = vault.join(&needle);
        if cand.is_file() {
            if let Ok(rel) = cand.strip_prefix(vault) {
                return vec![rel.to_path_buf()];
            }
        }
        // Source-relative resolution.
        if let Some(parent) = source.parent() {
            let cand = parent.join(&needle);
            // Python: `(source.parent / needle).resolve()` — canonicalize then
            // re-anchor to vault. Use our walk::absolute helper for parity.
            let canonical = absolute(&cand);
            if canonical.is_file() {
                if let Ok(rel) = canonical.strip_prefix(vault) {
                    return vec![rel.to_path_buf()];
                }
            }
        }
        // Path-suffix match.
        let suffix = format!("/{}", needle.trim_start_matches('/'));
        for p in all_relpaths {
            let s = p.to_string_lossy();
            // Anchor to dir boundary: equivalent to ("/" + p).endswith(suffix).
            if format!("/{s}").ends_with(&suffix) {
                return vec![p.clone()];
            }
        }
        return Vec::new();
    }

    // Bare basename.
    let stem = target.strip_suffix(".md").unwrap_or(target);
    if let Some(paths) = basename_map.get(stem) {
        return paths
            .iter()
            .filter_map(|p| p.strip_prefix(vault).ok().map(|r| r.to_path_buf()))
            .collect();
    }
    Vec::new()
}

fn print_corpus(r: &CorpusReport) {
    let suffix = if r.label.is_empty() { String::new() } else { format!(" ({})", r.label) };

    println!("## Frontmatter issues{suffix}\n");
    if r.frontmatter_issues.is_empty() {
        println!("_(none)_");
    } else {
        for it in &r.frontmatter_issues {
            println!("- `{}` — {}", it.file, it.issue);
        }
    }
    println!();

    println!("## Broken wikilinks{suffix}\n");
    if r.broken_wikilinks.is_empty() {
        println!("_(none)_");
    } else {
        for it in &r.broken_wikilinks {
            println!("- `{}` → `[[{}]]`", it.file, it.link);
        }
    }
    println!();

    println!("## Orphan notes{suffix} (no incoming wikilink, excluding README.md)\n");
    if r.orphan_notes.is_empty() {
        println!("_(none)_");
    } else {
        for p in &r.orphan_notes {
            println!("- `{p}`");
        }
    }
    println!();

    println!("## Duplicate basenames{suffix} (bare wikilinks become ambiguous)\n");
    if r.duplicate_basenames.is_empty() {
        println!("_(none)_");
    } else {
        for d in &r.duplicate_basenames {
            println!("- `{}` shared by:", d.basename);
            for p in &d.paths {
                println!("  - `{p}`");
            }
        }
    }
    println!();
}

/// Build the JSON object Python emits, preserving its key order.
fn build_audit_json(now: &str, reports: &[CorpusReport]) -> Result<String> {
    use serde_json::{Map, Value};

    let mut root = Map::new();
    root.insert("generated".into(), Value::String(now.to_string()));

    let corpora: Vec<Value> = reports
        .iter()
        .map(|r| {
            let mut m = Map::new();
            m.insert("label".into(), Value::String(r.label.clone()));
            m.insert("root".into(), Value::String(r.root.clone()));

            let mut counts = Map::new();
            counts.insert("files_scanned".into(), Value::from(r.files_scanned));
            counts.insert("frontmatter_issues".into(), Value::from(r.frontmatter_issues.len()));
            counts.insert("broken_wikilinks".into(), Value::from(r.broken_wikilinks.len()));
            counts.insert("orphan_notes".into(), Value::from(r.orphan_notes.len()));
            counts.insert("duplicate_basenames".into(), Value::from(r.duplicate_basenames.len()));
            m.insert("counts".into(), Value::Object(counts));

            m.insert(
                "frontmatter_issues".into(),
                Value::Array(r.frontmatter_issues.iter().map(|it| {
                    let mut o = Map::new();
                    o.insert("file".into(), Value::String(it.file.clone()));
                    o.insert("issue".into(), Value::String(it.issue.clone()));
                    Value::Object(o)
                }).collect()),
            );
            m.insert(
                "broken_wikilinks".into(),
                Value::Array(r.broken_wikilinks.iter().map(|it| {
                    let mut o = Map::new();
                    o.insert("file".into(), Value::String(it.file.clone()));
                    o.insert("link".into(), Value::String(it.link.clone()));
                    Value::Object(o)
                }).collect()),
            );
            m.insert(
                "orphan_notes".into(),
                Value::Array(r.orphan_notes.iter().cloned().map(Value::String).collect()),
            );
            m.insert(
                "duplicate_basenames".into(),
                Value::Array(r.duplicate_basenames.iter().map(|d| {
                    let mut o = Map::new();
                    o.insert("basename".into(), Value::String(d.basename.clone()));
                    o.insert(
                        "paths".into(),
                        Value::Array(d.paths.iter().cloned().map(Value::String).collect()),
                    );
                    Value::Object(o)
                }).collect()),
            );
            Value::Object(m)
        })
        .collect();

    root.insert("corpora".into(), Value::Array(corpora));
    let v = Value::Object(root);
    Ok(crate::jsonfmt::to_string_pretty_ascii(&v)?)
}
