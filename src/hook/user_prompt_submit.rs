//! UserPromptSubmit hook — port of `hooks/scripts/user-prompt-submit.sh`.
//!
//! Per-prompt vault retrieval gate. Reads stdin payload, asks `claude -p` what
//! (if anything) to inject, validates + caps + dedupes, and emits hook-spec
//! JSON containing a user-visible `systemMessage` plus `additionalContext`
//! that Claude Code attaches to the user message.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use chrono::Local;
use serde_json::Value;

use crate::hook::{
    load_config_env, overview_cache, recursion_guard_active, safe_session_id, usage_log, which,
};
use crate::vault::walk::resolve_vault;

pub fn run() -> Result<i32> {
    if recursion_guard_active() {
        return Ok(0);
    }

    load_config_env();

    let vault = resolve_vault(None);
    let log_path = std::env::var("MEMORY_GATE_LOG").unwrap_or_else(|_| "/tmp/claude-memory-gate.log".into());
    let log_path = PathBuf::from(log_path);
    rotate_log_if_oversized(&log_path);

    let path_cap: usize = std::env::var("OBSIDIAN_MEMORY_GATE_PATH_CAP")
        .ok().and_then(|s| s.parse().ok()).unwrap_or(3);
    let note_byte_cap: u64 = std::env::var("OBSIDIAN_MEMORY_GATE_NOTE_BYTE_CAP")
        .ok().and_then(|s| s.parse().ok()).unwrap_or(10240);
    let gate_enabled = std::env::var("OBSIDIAN_MEMORY_GATE_ENABLED")
        .map(|v| v != "false").unwrap_or(true);
    if !gate_enabled {
        return Ok(0);
    }

    let claude_bin = match std::env::var("CLAUDE_BIN").ok().filter(|s| !s.is_empty()) {
        Some(p) if Path::new(&p).is_file() => p,
        _ => match which("claude") {
            Some(p) => p,
            None => {
                eprintln!("[gate] claude CLI not found on PATH; vault gate skipped this turn");
                log_line(&log_path, "skipped: no claude CLI");
                return Ok(0);
            }
        },
    };

    if !vault.is_dir() {
        log_line(&log_path, &format!("skipped: vault not found at '{}'", vault.display()));
        return Ok(0);
    }

    // Stdin payload.
    let mut payload_text = String::new();
    let _ = std::io::stdin().read_to_string(&mut payload_text);
    let payload: Value = match serde_json::from_str(&payload_text) {
        Ok(v) => v,
        Err(_) => {
            log_line(&log_path, "skipped: payload not JSON");
            return Ok(0);
        }
    };
    let user_message = payload.get("prompt").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let session_id = payload.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();

    if user_message.is_empty() {
        log_line(&log_path, "skipped: no .prompt in payload");
        return Ok(0);
    }

    let project_dir = std::env::var("CLAUDE_PROJECT_DIR")
        .ok().filter(|s| !s.is_empty())
        .unwrap_or_else(|| std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
    let project_name = Path::new(&project_dir).file_name()
        .map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();

    // Per-session dedup
    let dedup_dir = std::env::var("MEMORY_GATE_DEDUP_DIR")
        .unwrap_or_else(|_| "/tmp/claude-memory-gate-state".into());
    let dedup_dir = PathBuf::from(dedup_dir);
    let _ = std::fs::create_dir_all(&dedup_dir);
    let dedup_file = if !session_id.is_empty() {
        let p = dedup_dir.join(format!("{}.injected", safe_session_id(&session_id)));
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(&p);
        Some(p)
    } else { None };

    // Resolve project-vault from registry (live read on every turn so the
    // /obsidian-memory:project enable|disable command takes effect immediately).
    let project_vault_path = resolve_active_project_vault(&project_dir);

    let overview = overview_cache::get_or_build(&vault, &project_name, project_vault_path.as_deref());
    if overview.is_empty() {
        log_line(&log_path, "skipped: vault overview empty");
        return Ok(0);
    }

    let system_prompt = build_system_prompt(&overview, path_cap);
    let user_prompt = format!("USER MESSAGE:\n{user_message}\n\nJSON only:");

    let gate_raw = match run_claude_gate(&claude_bin, &system_prompt, &user_prompt, &log_path) {
        Some(s) => s,
        None => return Ok(0),
    };

    let gate_text = unwrap_gate_envelope(&gate_raw);

    // Telemetry: extract usage / cost / duration from the JSON envelope.
    if !session_id.is_empty() {
        if let Ok(events) = serde_json::from_str::<Value>(&gate_raw) {
            if let Some(arr) = events.as_array() {
                if let Some(result_ev) = arr.iter().find(|e| e.get("type").and_then(|t| t.as_str()) == Some("result")) {
                    let usage = result_ev.get("usage").cloned().unwrap_or(Value::Object(serde_json::Map::new()));
                    let cost = result_ev.get("total_cost_usd").and_then(|v| v.as_f64()).map(|f| f.to_string());
                    let duration = result_ev.get("duration_ms").and_then(|v| v.as_u64()).map(|u| u.to_string());
                    let usage_str = serde_json::to_string(&usage).unwrap_or_default();
                    usage_log::append_api(&session_id, "gate_call", &usage_str, cost.as_deref(), duration.as_deref());
                }
            }
        }
    }

    // Parse the gate's JSON answer + execute searches.
    let paths = match select_paths(&gate_text, &vault, path_cap) {
        Some(p) => p,
        None => {
            log_line(&log_path, "gate: no paths after merge");
            return Ok(0);
        }
    };

    if paths.is_empty() {
        log_line(&log_path, "gate: no paths after merge");
        return Ok(0);
    }

    // Validate + dedupe + injection.
    let mut injected_paths: Vec<String> = Vec::new();
    let mut injection_body = String::new();
    let mut dropped: Vec<String> = Vec::new();
    let mut duped: Vec<String> = Vec::new();
    for p in &paths {
        if !is_safe_path(p, &vault, project_vault_path.as_deref()) {
            dropped.push(format!("{p} (unsafe)"));
            continue;
        }
        let pp = Path::new(p);
        if !pp.is_file() {
            dropped.push(format!("{p} (missing)"));
            continue;
        }
        if already_injected(dedup_file.as_deref(), p) {
            duped.push(p.clone());
            continue;
        }
        let bytes = std::fs::read(pp).unwrap_or_default();
        let total_size = bytes.len() as u64;
        let truncated = total_size > note_byte_cap;
        let take = (note_byte_cap as usize).min(bytes.len());
        let head = String::from_utf8_lossy(&bytes[..take]).into_owned();
        injection_body.push('\n');
        injection_body.push_str(&format!("--- {p} ---\n"));
        injection_body.push_str(&head);
        if truncated {
            injection_body.push_str(&format!(
                "\n[…truncated at {note_byte_cap} bytes; full content via Read of {p}]\n"
            ));
        }
        mark_injected(dedup_file.as_deref(), p);
        injected_paths.push(p.clone());
    }
    if !dropped.is_empty() {
        log_line(&log_path, &format!("gate: dropped paths: {}", dropped.join(" ")));
    }
    if !duped.is_empty() {
        log_line(&log_path, &format!("gate: skipped already-injected this session: {}", duped.join(" ")));
    }

    if injected_paths.is_empty() {
        log_line(&log_path, "gate: nothing new to inject");
        return Ok(0);
    }

    let joined = injected_paths.join(", ");
    let user_msg = format!("vault → {joined}");
    let additional_context = format!(
        "\n=== VAULT CONTEXT (auto-retrieved by memory gate) ===\n{injection_body}\n=== END VAULT CONTEXT ===\n"
    );

    // Emit jq -n equivalent: pretty-printed JSON with indent=2.
    let hook_output = serde_json::json!({
        "systemMessage": user_msg,
        "hookSpecificOutput": {
            "hookEventName": "UserPromptSubmit",
            "additionalContext": additional_context,
        }
    });
    let stdout = std::io::stdout();
    let mut h = stdout.lock();
    writeln!(h, "{}", serde_json::to_string_pretty(&hook_output)?)?;

    log_line(&log_path, &format!("gate: injected {} notes ({joined})", injected_paths.len()));

    if !session_id.is_empty() {
        usage_log::append_chars(&session_id, "gate_inject", injection_body.len() as u64);
    }
    Ok(0)
}

fn rotate_log_if_oversized(log_path: &Path) {
    let max: u64 = std::env::var("MEMORY_LOG_MAX_BYTES")
        .ok().and_then(|s| s.parse().ok()).unwrap_or(1_048_576);
    if let Ok(meta) = std::fs::metadata(log_path) {
        if meta.len() > max {
            let rotated = log_path.with_extension(format!(
                "{}.1",
                log_path.extension().and_then(|s| s.to_str()).unwrap_or("log")
            ));
            let _ = std::fs::rename(log_path, rotated);
        }
    }
}

fn log_line(path: &Path, msg: &str) {
    let ts = Local::now().format("%Y-%m-%d %H:%M:%S");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[{ts}] {msg}");
    }
}

fn resolve_active_project_vault(project_dir: &str) -> Option<PathBuf> {
    // git toplevel of project_dir.
    let out = Command::new("git").arg("-C").arg(project_dir).args(["rev-parse", "--show-toplevel"])
        .output().ok()?;
    if !out.status.success() { return None; }
    let project_root = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if project_root.is_empty() { return None; }

    let projects_file = match std::env::var("OBSIDIAN_MEMORY_PROJECTS_FILE") {
        Ok(p) => PathBuf::from(p),
        Err(_) => dirs::home_dir()?.join(".config/obsidian-memory/projects.json"),
    };
    let text = std::fs::read_to_string(&projects_file).ok()?;
    let data: Value = serde_json::from_str(&text).ok()?;
    let resolved_key = crate::vault::walk::absolute(Path::new(&project_root))
        .to_string_lossy().into_owned();
    let entry = data.get("projects")?.as_object()?.get(&resolved_key)?;
    if entry.get("enabled").and_then(|v| v.as_bool()) != Some(true) {
        return None;
    }
    Some(PathBuf::from(project_root))
}

fn build_system_prompt(overview: &str, path_cap: usize) -> String {
    format!(
r##"Retrieval gate for an Obsidian vault. Default to {{}}; inject only when a specific note demonstrably helps. Output ONE JSON object on a single line, no prose. `read` paths are absolute filesystem paths copied verbatim from the overview below.

{{"read":["/abs/path/to/note.md"], "search":[{{"type":"...","keywords":"...","path_prefix":"...","created_after":"YYYY-MM-DD","created_before":"YYYY-MM-DD","updated_after":"YYYY-MM-DD","updated_before":"YYYY-MM-DD"}}]}}

Both optional. Cap {path_cap} paths.

INJECT when:
- user references prior work or names a topic an overview bullet captures
- user describes a symptom whose cause an overview bullet covers
- user proposes or imperatively does something covered by a guardrail in the overview (decision/learning/preference) OR names a task a tool note covers ("send a message" → Slack)
- user asks about a category over time (use `search` with `type`)

SKIP: greetings or short replies; generic tech questions; clean imperatives with no overview-flagged constraint; hypotheticals about absent topics; anything the overview itself answers. If 50/50, output {{}}.

Examples (replace /abs/path with the actual absolute paths shown in the overview):
"thanks" → {{}}
"how would you tune X?" → {{}}
"delete the foo note" → {{}}
"add a colors module" → {{}}
"what would we decide if we needed kubernetes?" → {{}}
"send a message to #eng" → {{"read":["/abs/path/Tools/Slack.md"]}}
"add the api token to Tools/X.md" → {{"read":["/abs/path/Notes/secrets-env.md"]}}
"remind me about the secrets pattern" → {{"read":["/abs/path/Notes/secrets-env.md"]}}
"what learnings this week?" → {{"search":[{{"type":"learning","created_after":"2026-04-22"}}]}}

=== VAULT OVERVIEW ===
{overview}"##
    )
}

fn run_claude_gate(claude_bin: &str, system_prompt: &str, user_prompt: &str, log_path: &Path) -> Option<String> {
    let mut cmd = Command::new(claude_bin);
    cmd.args(["-p", user_prompt,
              "--system-prompt", system_prompt,
              "--tools", "",
              "--strict-mcp-config",
              "--output-format", "json"]);
    cmd.env("CLAUDE_MEMORY_GATE", "1");
    cmd.env("CLAUDE_MEMORY_REVIEW", "1");
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let output = cmd.output().ok()?;
    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        eprintln!("[gate] retrieval gate failed (claude -p exit={code}) — proceeding without vault context");
        log_line(log_path, &format!("gate exited {code}"));
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Unwrap the result event from `claude -p --output-format json`. Falls back
/// to the raw output on parse failure (keeps retrieval working when telemetry
/// breaks).
fn unwrap_gate_envelope(raw: &str) -> String {
    if let Ok(events) = serde_json::from_str::<Value>(raw) {
        if let Some(arr) = events.as_array() {
            if let Some(result_ev) = arr.iter().find(|e| e.get("type").and_then(|t| t.as_str()) == Some("result")) {
                if let Some(s) = result_ev.get("result").and_then(|v| v.as_str()) {
                    return s.to_string();
                }
            }
        }
    }
    raw.to_string()
}

/// Find the first balanced `{...}` in the gate output, parse it, then collect
/// `read` paths plus path lists from each `search` query (executed via the
/// vault search module). Mirrors the embedded `python3 -c` block in the bash hook.
fn select_paths(gate_text: &str, vault: &Path, cap: usize) -> Option<Vec<String>> {
    let bytes = gate_text.as_bytes();
    let start = bytes.iter().position(|&b| b == b'{')?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    let mut end = None;
    for (i, &c) in bytes.iter().enumerate().skip(start) {
        if esc { esc = false; continue; }
        if c == b'\\' { esc = true; continue; }
        if c == b'"' { in_str = !in_str; continue; }
        if in_str { continue; }
        if c == b'{' { depth += 1; }
        else if c == b'}' {
            depth -= 1;
            if depth == 0 { end = Some(i); break; }
        }
    }
    let end = end?;
    let obj: Value = serde_json::from_slice(&bytes[start..=end]).ok()?;

    let mut ordered: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    fn add(ordered: &mut Vec<String>, seen: &mut std::collections::HashSet<String>, p: &str) {
        let p = p.trim();
        if p.is_empty() || !seen.insert(p.to_string()) { return; }
        ordered.push(p.to_string());
    }

    if let Some(arr) = obj.get("read").and_then(|v| v.as_array()) {
        for v in arr {
            if let Some(s) = v.as_str() { add(&mut ordered, &mut seen, s); }
        }
    }

    if let Some(arr) = obj.get("search").and_then(|v| v.as_array()) {
        for q in arr {
            if ordered.len() >= cap { break; }
            let Some(qobj) = q.as_object() else { continue };
            let opts = crate::vault::search::SearchOpts {
                type_: qobj.get("type").and_then(|v| v.as_str()),
                path_prefix: qobj.get("path_prefix").and_then(|v| v.as_str()),
                keywords: qobj.get("keywords").and_then(|v| v.as_str()),
                created_after: qobj.get("created_after").and_then(|v| v.as_str()),
                created_before: qobj.get("created_before").and_then(|v| v.as_str()),
                updated_after: qobj.get("updated_after").and_then(|v| v.as_str()),
                updated_before: qobj.get("updated_before").and_then(|v| v.as_str()),
                limit: cap,
                project_vault: None,
            };
            if let Ok(hits) = crate::vault::search::search(vault, opts) {
                for h in hits {
                    add(&mut ordered, &mut seen, &h.path);
                    if ordered.len() >= cap { break; }
                }
            }
        }
    }

    Some(ordered.into_iter().take(cap).collect())
}

fn is_safe_path(p: &str, vault: &Path, project_vault: Option<&Path>) -> bool {
    if !p.starts_with('/') { return false; }
    // Reject `..` as a path component.
    if p.split('/').any(|seg| seg == "..") { return false; }
    let vault_prefix = format!("{}/", vault.display());
    if p.starts_with(&vault_prefix) { return true; }
    if let Some(pv) = project_vault {
        let pv_prefix = format!("{}/", pv.display());
        if p.starts_with(&pv_prefix) { return true; }
    }
    false
}

fn already_injected(dedup_file: Option<&Path>, p: &str) -> bool {
    let Some(path) = dedup_file else { return false };
    let Ok(text) = std::fs::read_to_string(path) else { return false };
    text.lines().any(|l| l == p)
}

fn mark_injected(dedup_file: Option<&Path>, p: &str) {
    let Some(path) = dedup_file else { return };
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{p}");
    }
}
