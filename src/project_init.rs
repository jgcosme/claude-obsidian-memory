//! Initialize a project's docs as a project-vault corpus —
//! port of scripts/init_project_vault.py.
//!
//! For each .md file enumerated by `project_docs::enumerate_project_docs`
//! that lacks ANY frontmatter, prepend an Obsidian-style frontmatter block
//! with: type, description, created, project. Files that already have any
//! frontmatter (plugin, skill, slash command, etc.) are left untouched —
//! the `_has_frontmatter` check is intentionally type-agnostic so init is
//! idempotent and never stomps non-plugin frontmatter conventions.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::cli::InitProjectArgs;
use crate::project_docs::enumerate_project_docs;
use crate::vault::frontmatter::{FRONTMATTER_RE, is_valid_type};
use crate::vault::walk::{absolute, expand_user};

const FALLBACK_TYPE: &str = "reference";
const MAX_DESCRIPTION_LEN: usize = 120;
const LLM_EXCERPT_CHARS: usize = 600;
const LLM_BATCH_SIZE: usize = 30;
const LLM_TIMEOUT_SECS: u64 = 180;

/// Embedded copy of templates/types.md so the binary can include the canonical
/// memory-type definitions in its LLM prompt without a runtime path lookup.
const TYPES_DOC: &str = include_str!("../templates/types.md");

static H1_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^#\s+(.+?)\s*$").expect("H1 regex"));
static WHITESPACE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").expect("ws regex"));

#[derive(Debug, Serialize)]
struct AddedItem {
    path: String,
    types: Vec<String>,
    description: String,
}

#[derive(Debug, Serialize)]
struct SkippedItem {
    path: String,
    reason: String,
}

#[derive(Debug, Serialize)]
struct InitResult {
    added: Vec<AddedItem>,
    skipped: Vec<SkippedItem>,
}

pub fn run(args: InitProjectArgs) -> Result<i32> {
    let repo = absolute(&expand_user(Path::new(&args.project_path)));
    if !repo.join(".git").exists() {
        eprintln!("not a git repo: {}", repo.display());
        return Ok(1);
    }

    let result = init_project_vault(
        &repo,
        &args.project,
        args.dry_run,
        !args.no_llm,
    )?;

    if args.json {
        // Python json.dumps(result, indent=2) — preserves insertion order.
        // result keys: ["added", "skipped"]. Each added item: ["path", "types",
        // "description"]. Each skipped: ["path", "reason"].
        let v = serde_json::to_value(&result)?;
        println!("{}", crate::jsonfmt::to_string_pretty_ascii(&v)?);
        return Ok(0);
    }

    let verb = if args.dry_run { "would add" } else { "added" };
    if !result.added.is_empty() {
        println!("{verb} frontmatter to {} file(s):", result.added.len());
        for item in &result.added {
            let type_disp = item.types.join(",");
            println!("  + [{type_disp}] {} — {}", item.path, item.description);
        }
    } else {
        println!("no candidates needed frontmatter");
    }
    if !result.skipped.is_empty() {
        println!();
        println!("skipped {} file(s):", result.skipped.len());
        for item in result.skipped.iter().take(10) {
            println!("  - {} ({})", item.path, item.reason);
        }
        if result.skipped.len() > 10 {
            println!("  … and {} more", result.skipped.len() - 10);
        }
    }
    Ok(0)
}

/// Eager init for the SessionStart hook (`enabled` projects). Runs without
/// printing — caller decides whether to surface results. LLM path skipped to
/// avoid blocking session startup; deterministic-only.
pub fn init_project_vault_silent(repo: &Path, project: &str) -> Result<()> {
    let _ = init_project_vault(repo, project, /* dry_run */ false, /* use_llm */ false)?;
    Ok(())
}

fn init_project_vault(
    repo: &Path,
    project: &str,
    dry_run: bool,
    use_llm: bool,
) -> Result<InitResult> {
    let md_files = enumerate_project_docs(repo);

    let mut candidates: Vec<(PathBuf, String)> = Vec::new(); // (relpath, body)
    let mut skipped: Vec<SkippedItem> = Vec::new();

    for f in &md_files {
        let rel = match f.strip_prefix(repo) {
            Ok(r) => r.to_path_buf(),
            Err(_) => continue,
        };
        let bytes = match std::fs::read(f) {
            Ok(b) => b,
            Err(e) => {
                skipped.push(SkippedItem {
                    path: rel.to_string_lossy().into_owned(),
                    reason: format!("unreadable: {e}"),
                });
                continue;
            }
        };
        let text = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => {
                skipped.push(SkippedItem {
                    path: rel.to_string_lossy().into_owned(),
                    reason: format!("unreadable: {e}"),
                });
                continue;
            }
        };
        if has_frontmatter(&text) {
            skipped.push(SkippedItem {
                path: rel.to_string_lossy().into_owned(),
                reason: "has frontmatter".into(),
            });
            continue;
        }
        candidates.push((rel, text));
    }

    if candidates.is_empty() {
        return Ok(InitResult { added: Vec::new(), skipped });
    }

    let classifications: BTreeMap<String, Classification> = if use_llm {
        llm_classify(&candidates)
    } else {
        deterministic_fallback(&candidates)
    };

    let mut added: Vec<AddedItem> = Vec::new();
    for (rel_path, body) in &candidates {
        let rel_str = rel_path.to_string_lossy().into_owned();
        let cls = classifications.get(&rel_str).cloned().unwrap_or_else(|| Classification {
            types: vec![FALLBACK_TYPE.into()],
            description: derive_description(body, &rel_str),
        });
        let mut types_list: Vec<String> = cls.types.into_iter()
            .filter(|t| is_valid_type(t))
            .collect();
        if types_list.is_empty() {
            types_list.push(FALLBACK_TYPE.into());
        }
        let desc = if cls.description.is_empty() {
            derive_description(body, &rel_str)
        } else {
            cls.description
        };
        if !dry_run {
            let fm = format_frontmatter(&types_list, &desc, project);
            let target = repo.join(rel_path);
            std::fs::write(&target, format!("{fm}{body}"))
                .with_context(|| format!("write {}", target.display()))?;
        }
        added.push(AddedItem {
            path: rel_str,
            types: types_list,
            description: desc,
        });
    }

    Ok(InitResult { added, skipped })
}

fn has_frontmatter(text: &str) -> bool {
    FRONTMATTER_RE.is_match(text)
}

/// H1 → first non-blank line → bare basename. Sanitized + truncated.
fn derive_description(body: &str, fallback_path: &str) -> String {
    let mut desc: String = if let Some(c) = H1_RE.captures(body) {
        c.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default()
    } else {
        body.lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                Path::new(fallback_path)
                    .file_stem()
                    .map(|s| s.to_string_lossy().replace(['-', '_'], " "))
                    .unwrap_or_default()
            })
    };

    desc = WHITESPACE_RE.replace_all(&desc, " ").into_owned();
    desc = desc.replace(": ", " - ");
    desc = desc.trim_matches('#').trim().trim_matches(|c| c == '"' || c == '\'').to_string();
    if desc.chars().count() > MAX_DESCRIPTION_LEN {
        // Python: desc[: MAX-1].rstrip() + "…". Slice by codepoints.
        let truncated: String = desc.chars().take(MAX_DESCRIPTION_LEN - 1).collect();
        desc = format!("{}…", truncated.trim_end());
    }
    desc
}

fn format_frontmatter(types: &[String], description: &str, project: &str) -> String {
    let now = crate::vault::timestamps::now_iso8601_local();
    let safe_desc = description.replace('\\', "\\\\").replace('"', "\\\"");
    let type_field = if types.len() == 1 {
        types[0].clone()
    } else {
        format!("[{}]", types.join(", "))
    };
    format!(
        "---\ntype: {type_field}\ndescription: \"{safe_desc}\"\ncreated_at: {now}\ncreated_by: init\nupdated_at: {now}\nupdated_by: init\nproject: {project}\n---\n\n"
    )
}

#[derive(Debug, Clone)]
struct Classification {
    types: Vec<String>,
    description: String,
}

fn deterministic_fallback(candidates: &[(PathBuf, String)]) -> BTreeMap<String, Classification> {
    candidates
        .iter()
        .map(|(rel, body)| {
            let key = rel.to_string_lossy().into_owned();
            (
                key.clone(),
                Classification {
                    types: vec![FALLBACK_TYPE.into()],
                    description: derive_description(body, &key),
                },
            )
        })
        .collect()
}

/// Chunked claude -p classification. Mirrors Python's `_llm_classify`. Failed
/// chunks fall back deterministically and a one-line stderr note is written
/// so the user can see how many files defaulted.
fn llm_classify(candidates: &[(PathBuf, String)]) -> BTreeMap<String, Classification> {
    let claude_bin = match std::env::var("CLAUDE_BIN").ok().filter(|s| !s.is_empty()) {
        Some(p) => Some(p),
        None => which("claude"),
    };
    let Some(claude_bin) = claude_bin else {
        eprintln!("[init] no claude CLI found — every file will get type=reference");
        return deterministic_fallback(candidates);
    };

    let mut out: BTreeMap<String, Classification> = BTreeMap::new();
    let n_chunks = candidates.len().div_ceil(LLM_BATCH_SIZE);
    let mut failed_chunks = 0usize;
    for chunk in candidates.chunks(LLM_BATCH_SIZE) {
        match llm_classify_batch(chunk, &claude_bin) {
            Some(map) => out.extend(map),
            None => {
                failed_chunks += 1;
                out.extend(deterministic_fallback(chunk));
            }
        }
    }
    if failed_chunks > 0 {
        eprintln!(
            "[init] {failed_chunks}/{n_chunks} LLM chunk(s) failed — those files defaulted to type=reference"
        );
    }
    out
}

fn llm_classify_batch(chunk: &[(PathBuf, String)], claude_bin: &str) -> Option<BTreeMap<String, Classification>> {
    let prompt = build_llm_prompt(chunk);
    let mut cmd = Command::new(claude_bin);
    cmd.args(["-p", &prompt, "--tools", "", "--strict-mcp-config", "--output-format", "json"]);
    cmd.env("CLAUDE_MEMORY_GATE", "1");
    cmd.env("CLAUDE_MEMORY_REVIEW", "1");
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    // best-effort timeout: spawn + wait; we don't kill on timeout in this
    // first pass since rust std lacks a built-in wait_timeout. The Python
    // version uses subprocess.run(..., timeout=180). If this becomes an
    // issue in practice, swap for `wait-timeout` crate.
    let _ = Duration::from_secs(LLM_TIMEOUT_SECS);
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }

    // Unwrap claude -p JSON envelope.
    let text = match serde_json::from_slice::<Value>(&output.stdout) {
        Ok(v) => {
            if let Some(arr) = v.as_array() {
                arr.iter()
                    .find(|ev| ev.get("type").and_then(|t| t.as_str()) == Some("result"))
                    .and_then(|ev| ev.get("result").and_then(|r| r.as_str()))
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| String::from_utf8_lossy(&output.stdout).into_owned())
            } else {
                String::from_utf8_lossy(&output.stdout).into_owned()
            }
        }
        Err(_) => String::from_utf8_lossy(&output.stdout).into_owned(),
    };

    Some(parse_llm_output(&text, chunk))
}

fn build_llm_prompt(chunk: &[(PathBuf, String)]) -> String {
    let mut parts: Vec<String> = vec![
        "You are classifying markdown files in a project repository to add \
         Obsidian-style memory frontmatter. For each file, output ONE JSON \
         object PER LINE — no prose, no code fences, no commentary.".into(),
        String::new(),
        "Schema per line:".into(),
        "  {\"path\": \"<exact path as given>\", \
         \"type\": \"<single type>\" OR [\"<type1>\", \"<type2>\", ...], \
         \"description\": \"<one-line summary, ≤120 chars, no embedded `: `>\"}".into(),
        String::new(),
        TYPES_DOC.to_string(),
        String::new(),
        "Rules:".into(),
        "- If unsure, use \"reference\" — it's the safe fallback.".into(),
        "- Multi-type allowed: a note that genuinely spans axes can use a list. \
         Order by routing precedence (first type drives destination).".into(),
        "- Description: derive from H1 or first paragraph. One line. Replace any \
         embedded `: ` with ` - ` so the description parses as unquoted YAML.".into(),
        "- Output exactly one line per file, in the same order.".into(),
        String::new(),
        "FILES:".into(),
    ];
    for (i, (rel_path, body)) in chunk.iter().enumerate() {
        parts.push(format!("=== FILE {}: {} ===", i + 1, rel_path.display()));
        parts.push(excerpt(body));
        parts.push(String::new());
    }
    parts.join("\n")
}

fn excerpt(body: &str) -> String {
    if body.chars().count() <= LLM_EXCERPT_CHARS {
        return body.to_string();
    }
    let truncated: String = body.chars().take(LLM_EXCERPT_CHARS).collect();
    format!("{}\n…", truncated.trim_end())
}

fn parse_llm_output(raw: &str, chunk: &[(PathBuf, String)]) -> BTreeMap<String, Classification> {
    let mut by_path: BTreeMap<String, Classification> = BTreeMap::new();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || !line.starts_with('{') {
            continue;
        }
        let Ok(obj) = serde_json::from_str::<Value>(line) else { continue };
        let path = obj.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if path.is_empty() {
            continue;
        }

        let raw_type = obj.get("type").cloned().unwrap_or(Value::Null);
        let types_list: Vec<String> = match raw_type {
            Value::Array(arr) => {
                let mut cleaned: Vec<String> = Vec::new();
                for t in arr {
                    if let Some(s) = t.as_str() {
                        if is_valid_type(s) && !cleaned.iter().any(|x| x == s) {
                            cleaned.push(s.to_string());
                        }
                    }
                }
                if cleaned.is_empty() { vec![FALLBACK_TYPE.into()] } else { cleaned }
            }
            Value::String(s) if is_valid_type(&s) => vec![s],
            _ => vec![FALLBACK_TYPE.into()],
        };

        let desc_raw = obj.get("description").and_then(|v| v.as_str()).unwrap_or("");
        let mut desc = if desc_raw.is_empty() {
            String::new()
        } else {
            let collapsed = WHITESPACE_RE.replace_all(desc_raw, " ").into_owned();
            collapsed.replace(": ", " - ").trim().to_string()
        };
        if desc.chars().count() > MAX_DESCRIPTION_LEN {
            let truncated: String = desc.chars().take(MAX_DESCRIPTION_LEN - 1).collect();
            desc = format!("{}…", truncated.trim_end());
        }

        by_path.insert(path, Classification { types: types_list, description: desc });
    }

    // Fallback for any candidate the LLM didn't classify (or classified with no description).
    for (rel_path, body) in chunk {
        let rel_str = rel_path.to_string_lossy().into_owned();
        let needs_default_desc = by_path.get(&rel_str).map(|c| c.description.is_empty()).unwrap_or(true);
        if !by_path.contains_key(&rel_str) {
            by_path.insert(rel_str.clone(), Classification {
                types: vec![FALLBACK_TYPE.into()],
                description: derive_description(body, &rel_str),
            });
        } else if needs_default_desc {
            if let Some(entry) = by_path.get_mut(&rel_str) {
                entry.description = derive_description(body, &rel_str);
            }
        }
    }

    by_path
}

fn which(prog: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(prog);
        if candidate.is_file() {
            // Best-effort executable check via metadata.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = candidate.metadata() {
                    if meta.permissions().mode() & 0o111 == 0 {
                        continue;
                    }
                }
            }
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}
