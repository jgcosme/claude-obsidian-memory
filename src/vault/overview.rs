//! Overview generation (the SessionStart/UserPromptSubmit "what's in the vault" map).
//!
//! Mirrors `_vault.py:overview` and `overview_project`. Markdown layout is
//! load-bearing — the gate model parses bullets to pick paths, so this is a
//! byte-for-byte port.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::project_docs::enumerate_project_docs;
use crate::vault::frontmatter::{note_types, read_note};
use crate::vault::walk::collect_md_files;

const TYPE_ORDER: &[&str] = &[
    "preference", "reference", "findings", "decision", "learning", "tool", "journal",
];
const RECENT_JOURNAL_LIMIT: usize = 5;

type Note = (PathBuf, BTreeMap<String, String>);

fn bullet(path: &Path, fm: &BTreeMap<String, String>, primary_type: Option<&str>) -> String {
    let desc = fm.get("description").map(|s| s.trim()).unwrap_or("");
    let mut base = format!("- {}", path.display());
    let types = note_types(Some(fm));
    if let Some(pt) = primary_type {
        if types.len() > 1 {
            let others: Vec<&String> = types.iter().filter(|t| t.as_str() != pt).collect();
            if !others.is_empty() {
                let joined = others
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                base = format!("{base} [also: {joined}]");
            }
        }
    }
    if desc.is_empty() { base } else { format!("{base} — {desc}") }
}

pub fn overview(vault: &Path, project: Option<&str>, mode: &str) -> Result<String> {
    if !matches!(mode, "full" | "tools-and-general" | "tools-only") {
        bail!("unknown overview mode: {mode}");
    }

    let mut md_files = collect_md_files(vault);
    md_files.retain(|p| p.file_name().map(|n| n != "README.md").unwrap_or(true));

    let mut notes: Vec<Note> = Vec::with_capacity(md_files.len());
    for f in md_files {
        let (fm, _) = read_note(&f);
        notes.push((f, fm.unwrap_or_default()));
    }

    let by_prefix = |prefix: &str| -> Vec<Note> {
        notes
            .iter()
            .filter_map(|(f, fm)| {
                f.strip_prefix(vault).ok().and_then(|rel| {
                    if rel.to_string_lossy().starts_with(prefix) {
                        Some((f.clone(), fm.clone()))
                    } else {
                        None
                    }
                })
            })
            .collect()
    };

    let mut out: Vec<String> = vec!["# Vault overview".into(), "".into()];

    // Tools — flat list (always)
    out.push("## Tools".into());
    let tools = by_prefix("Tools/");
    if tools.is_empty() {
        out.push("_(empty)_".into());
    } else {
        for (f, fm) in &tools {
            out.push(bullet(f, fm, None));
        }
    }
    out.push("".into());

    if mode == "tools-only" {
        out.push(
            "_All non-Tools vault content is searchable via the `search` field \
             (filter by `type`, `keywords`, dates)._"
                .into(),
        );
        out.push("".into());
        return Ok(out.join("\n"));
    }

    // Notes — partition by project: frontmatter
    let notes_files = by_prefix("Notes/");
    let mut current_notes: Vec<Note> = Vec::new();
    let mut general_notes: Vec<Note> = Vec::new();
    let mut other_notes_by_project: BTreeMap<String, Vec<Note>> = BTreeMap::new();
    for (f, fm) in &notes_files {
        let proj = fm.get("project").map(|s| s.trim()).unwrap_or("");
        if proj.is_empty() {
            general_notes.push((f.clone(), fm.clone()));
        } else if Some(proj) == project {
            current_notes.push((f.clone(), fm.clone()));
        } else {
            other_notes_by_project
                .entry(proj.to_string())
                .or_default()
                .push((f.clone(), fm.clone()));
        }
    }

    if mode == "tools-and-general" {
        out.push("## Notes (general)".into());
        if general_notes.is_empty() {
            out.push("_(empty)_".into());
        } else {
            emit_by_type(&general_notes, &mut out);
        }
        out.push("".into());
        if !current_notes.is_empty() || !other_notes_by_project.is_empty() {
            out.push(
                "_Project-scoped notes available via `search` \
                 (filter by `keywords` matching the project name, plus `type`)._"
                    .into(),
            );
            out.push("".into());
        }
        return Ok(out.join("\n"));
    }

    // mode == "full"
    out.push("## Notes".into());

    if let Some(proj) = project {
        out.push(format!("### Current project: {proj}"));
        if current_notes.is_empty() {
            out.push("_(no notes yet for this project)_".into());
        } else {
            emit_by_type(&current_notes, &mut out);
        }
        out.push("".into());
    }

    out.push("### General (cross-project)".into());
    if general_notes.is_empty() {
        out.push("_(empty)_".into());
    } else {
        emit_by_type(&general_notes, &mut out);
    }
    out.push("".into());

    if !other_notes_by_project.is_empty() {
        out.push("### Other projects".into());
        out.push("(use `search` with `keywords: <project-name>` to query)".into());
        for (proj, notes_for) in &other_notes_by_project {
            let count = notes_for.len();
            let plural = if count != 1 { "s" } else { "" };
            out.push(format!("- {proj} ({count} note{plural})"));
        }
        out.push("".into());
    }

    let journals = by_prefix("Journals/");
    if !journals.is_empty() {
        let (mut scoped, label): (Vec<Note>, String) = match project {
            Some(proj) => {
                let scoped = journals
                    .iter()
                    .filter(|(_, fm)| fm.get("project").map(|s| s.trim()).unwrap_or("") == proj)
                    .cloned()
                    .collect();
                (scoped, format!("## Journals (recent — {proj})"))
            }
            None => (journals.clone(), "## Journals (recent)".to_string()),
        };
        scoped.sort_by(|a, b| {
            // Sort by `created:` desc, fall back to filename desc.
            let ka = a.1.get("created").cloned().filter(|s| !s.is_empty()).unwrap_or_else(|| {
                a.0.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
            });
            let kb = b.1.get("created").cloned().filter(|s| !s.is_empty()).unwrap_or_else(|| {
                b.0.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
            });
            kb.cmp(&ka)
        });
        out.push(label);
        for (f, fm) in scoped.iter().take(RECENT_JOURNAL_LIMIT) {
            out.push(bullet(f, fm, None));
        }
        if scoped.len() > RECENT_JOURNAL_LIMIT {
            out.push(format!(
                "_(+{} older — search by `created:` date)_",
                scoped.len() - RECENT_JOURNAL_LIMIT
            ));
        }
        out.push("".into());
    }

    Ok(out.join("\n"))
}

fn emit_by_type(items: &[Note], out: &mut Vec<String>) {
    // Multi-type notes appear under each of their types (matches Python).
    // We need stable insertion-order grouping so the resulting layout is
    // deterministic across runs and identical to Python's dict ordering.
    let mut by_type: BTreeMap<String, Vec<Note>> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for (f, fm) in items {
        let mut types = note_types(Some(fm));
        if types.is_empty() {
            types.push("untyped".to_string());
        }
        for t in types {
            if !by_type.contains_key(&t) {
                order.push(t.clone());
            }
            by_type.entry(t).or_default().push((f.clone(), fm.clone()));
        }
    }

    let mut ordered_types: Vec<String> = TYPE_ORDER
        .iter()
        .filter(|t| by_type.contains_key(**t))
        .map(|t| t.to_string())
        .collect();
    let mut extras: Vec<String> = order
        .into_iter()
        .filter(|t| !TYPE_ORDER.contains(&t.as_str()) && by_type.contains_key(t))
        .collect();
    extras.sort();
    for e in extras {
        if !ordered_types.contains(&e) {
            ordered_types.push(e);
        }
    }

    for type_ in &ordered_types {
        out.push(format!("#### {type_}"));
        if let Some(group) = by_type.get(type_) {
            for (f, fm) in group {
                out.push(bullet(f, fm, Some(type_)));
            }
        }
    }
}

pub fn overview_project(project_vault: &Path, project: Option<&str>) -> Result<String> {
    let md_files = enumerate_project_docs(project_vault);
    let mut by_type: BTreeMap<String, Vec<Note>> = BTreeMap::new();
    let mut insertion_order: Vec<String> = Vec::new();

    for f in md_files {
        let (fm, _) = read_note(&f);
        let Some(fm) = fm else { continue };
        if !fm.contains_key("type") || !fm.contains_key("description") {
            continue;
        }
        let mut types = note_types(Some(&fm));
        if types.is_empty() {
            types.push("untyped".to_string());
        }
        for t in types {
            if !by_type.contains_key(&t) {
                insertion_order.push(t.clone());
            }
            by_type.entry(t).or_default().push((f.clone(), fm.clone()));
        }
    }

    let title = match project {
        Some(p) => format!("Project vault: {p}"),
        None => format!(
            "Project vault: {}",
            project_vault.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
        ),
    };
    let mut out: Vec<String> = vec![format!("# {title}"), "".into()];

    if by_type.is_empty() {
        out.push("_(no notes with plugin frontmatter — run init to backfill)_".into());
        out.push("".into());
        return Ok(out.join("\n"));
    }

    let mut ordered_types: Vec<String> = TYPE_ORDER
        .iter()
        .filter(|t| by_type.contains_key(**t))
        .map(|t| t.to_string())
        .collect();
    let mut extras: Vec<String> = insertion_order
        .into_iter()
        .filter(|t| !TYPE_ORDER.contains(&t.as_str()) && by_type.contains_key(t))
        .collect();
    extras.sort();
    for e in extras {
        if !ordered_types.contains(&e) {
            ordered_types.push(e);
        }
    }

    for type_ in &ordered_types {
        if let Some(items) = by_type.get_mut(type_) {
            items.sort_by(|a, b| a.0.cmp(&b.0));
            out.push(format!("## {type_}"));
            for (f, fm) in items.iter() {
                out.push(bullet(f, fm, Some(type_)));
            }
            out.push("".into());
        }
    }

    Ok(out.join("\n"))
}
