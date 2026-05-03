//! Full Obsidian vault integrity audit — port of scripts/audit.py.
//!
//! Reports:
//!   - Frontmatter completeness (type, description, created_at; + project for project-vault)
//!   - Broken wikilinks (target file not found)
//!   - Orphan notes (no incoming wikilink, excluding README.md)
//!   - Duplicate basenames (bare wikilinks become ambiguous)
//!
//! With `--fix-frontmatter`, migrates legacy `created:` (date-only) to
//! `created_at:` (ISO 8601 with local offset, sourced from each note's git
//! first-commit timestamp; falls back to file mtime), and adds
//! `updated_at` + `updated_by: audit` when missing.
//!
//! Known divergence from Python: the optional `pyyaml` deep-validation pass
//! is not ported. Python only runs it when pyyaml happens to be installed,
//! and the parity harness controls fixtures so no malformed YAML appears.
//! Document if a user's live vault relies on it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use chrono::{DateTime, Local};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;

use crate::cli::AuditArgs;
use crate::project_docs::enumerate_project_docs;
use crate::vault::frontmatter::{
    FRONTMATTER_RE, VALID_TYPES, note_types, parse_frontmatter,
};
use crate::vault::timestamps;
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

    let personal_files = collect_md_files(&vault);
    if args.fix_frontmatter {
        let n = migrate_corpus(&vault, &personal_files);
        if n > 0 {
            eprintln!("[audit --fix-frontmatter] migrated {n} note(s) in {}", vault.display());
        }
    }
    reports.push(audit_corpus(
        if has_project_vault { "personal" } else { "" },
        &vault,
        &personal_files,
        false,
    ));

    if let Some(pv) = args.project_vault.as_ref() {
        let project_root = absolute(&expand_user(pv));
        if !project_root.is_dir() {
            eprintln!("project-vault not found at: {}", project_root.display());
            return Ok(1);
        }
        let project_files = enumerate_project_docs(&project_root);
        if args.fix_frontmatter {
            let n = migrate_corpus(&project_root, &project_files);
            if n > 0 {
                eprintln!("[audit --fix-frontmatter] migrated {n} note(s) in {}", project_root.display());
            }
        }
        let label = format!(
            "project:{}",
            project_root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
        );
        reports.push(audit_corpus(
            &label,
            &project_root,
            &project_files,
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
            // (canonical, legacy_aliases). `created_at` accepts the legacy
            // `created:` (date-only) so unmigrated notes don't all flag missing.
            let mut required: Vec<(&str, &[&str])> = vec![
                ("type", &[][..]),
                ("description", &[][..]),
                ("created_at", &["created"][..]),
            ];
            if project_required {
                required.push(("project", &[][..]));
            }
            for (canonical, aliases) in &required {
                let present = fm_ref
                    .map(|m| {
                        m.contains_key(*canonical) || aliases.iter().any(|a| m.contains_key(*a))
                    })
                    .unwrap_or(false);
                if !present {
                    fm_issues.push(FmIssue {
                        file: rel_str.clone(),
                        issue: format!("missing `{canonical}`"),
                    });
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

/// Rewrite each note's frontmatter to the new schema where applicable:
///   - rename legacy `created:` (date-only) to `created_at:` (datetime + offset)
///     using git first-commit timestamp; fall back to file mtime
///   - add `updated_at` + `updated_by: audit` when missing
///
/// Returns the number of files written. Skips README.md (no frontmatter).
fn migrate_corpus(root: &Path, files: &[PathBuf]) -> usize {
    let now = timestamps::now_iso8601_local();
    let mut written = 0usize;
    for f in files {
        if f.file_name().map(|n| NAVIGATION_NAMES.iter().any(|nav| *nav == n)).unwrap_or(false) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let Some(m) = FRONTMATTER_RE.captures(&text) else { continue };
        let whole = m.get(0).expect("group 0").as_str();
        let inner = m.get(1).expect("group 1").as_str();
        let body = &text[whole.len()..];

        let fm = match parse_frontmatter(&text) {
            Some(fm) => fm,
            None => continue,
        };
        let has_created_at = fm.contains_key("created_at");
        let has_legacy_created = fm.contains_key("created");
        let has_updated_at = fm.contains_key("updated_at");
        let has_updated_by = fm.contains_key("updated_by");

        let needs_rename = !has_created_at && has_legacy_created;
        let needs_updated = !has_updated_at || !has_updated_by;
        if !needs_rename && !needs_updated {
            continue;
        }

        let created_at_value = if needs_rename {
            git_first_commit_iso(root, f).unwrap_or_else(|| {
                file_mtime_iso(f).unwrap_or_else(|| now.clone())
            })
        } else {
            String::new()
        };

        let new_inner = rewrite_frontmatter(
            inner,
            needs_rename,
            &created_at_value,
            !has_updated_at,
            &now,
            !has_updated_by,
            "audit",
        );

        // Preserve the original block delimiters exactly: only the inner text
        // is rewritten. We always emit a trailing newline before `---` to keep
        // the YAML well-formed.
        let new_block = format!("---\n{new_inner}\n---\n");
        let new_text = format!("{new_block}{body}");
        if new_text != text && std::fs::write(f, new_text.as_bytes()).is_ok() {
            written += 1;
        }
    }
    written
}

/// Line-level rewrite that preserves key order and untouched lines. Renames
/// `created:` → `created_at:` when `rename_created`, and appends `updated_at`
/// / `updated_by` lines when those keys are missing.
fn rewrite_frontmatter(
    inner: &str,
    rename_created: bool,
    created_at_value: &str,
    add_updated_at: bool,
    updated_at_value: &str,
    add_updated_by: bool,
    actor: &str,
) -> String {
    let mut out_lines: Vec<String> = Vec::new();
    for raw_line in inner.split('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if rename_created {
            if let Some(rest) = strip_key_prefix(line, "created") {
                // Drop the legacy date value, emit the new key with the
                // datetime sourced from git. Discard `rest` (the old date)
                // so we don't double-record it.
                let _ = rest;
                out_lines.push(format!("created_at: {created_at_value}"));
                continue;
            }
        }
        out_lines.push(line.to_string());
    }
    // Strip trailing blank lines so appended fields sit flush against the block.
    while matches!(out_lines.last().map(|s| s.trim().is_empty()), Some(true)) {
        out_lines.pop();
    }
    if add_updated_at {
        out_lines.push(format!("updated_at: {updated_at_value}"));
    }
    if add_updated_by {
        out_lines.push(format!("updated_by: {actor}"));
    }
    out_lines.join("\n")
}

/// Match `^(\s*)<key>\s*:\s*(.*)$` and return the value tail. Returns None if
/// the line is indented (it would be a block-list item, not a top-level key).
fn strip_key_prefix<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    if line.starts_with(' ') || line.starts_with('\t') {
        return None;
    }
    let rest = line.strip_prefix(key)?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':')?;
    Some(rest.trim_start())
}

/// Get the ISO 8601 (local-offset) timestamp of the commit that first added
/// this file, by shelling out to `git log`. Returns None if the file is
/// untracked or git is unavailable.
fn git_first_commit_iso(repo_root: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(repo_root).ok()?;
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["log", "--diff-filter=A", "--follow", "--format=%aI", "--"])
        .arg(rel)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    // `--diff-filter=A --follow` can still emit multiple lines for renames; the
    // last line is the original-add commit.
    let ts = stdout.lines().last()?.trim().to_string();
    if ts.is_empty() {
        return None;
    }
    // Re-format to %:z (`+HH:MM`) so emitted timestamps are uniform.
    DateTime::parse_from_rfc3339(&ts)
        .ok()
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string())
}

fn file_mtime_iso(file: &Path) -> Option<String> {
    let meta = std::fs::metadata(file).ok()?;
    let mt = meta.modified().ok()?;
    let dt: DateTime<Local> = mt.into();
    Some(dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string())
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
