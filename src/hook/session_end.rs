//! SessionEnd hook — port of `hooks/scripts/session-end.sh`.
//!
//! Reviews the session transcript, writes proactive memory notes via a
//! backgrounded `claude -p` subprocess, and auto-commits vault changes.
//! The hook itself returns immediately; the review and commit run detached.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::Result;
use chrono::Local;
use serde_json::Value;

use crate::hook::{
    load_config_env, recursion_guard_active, safe_session_id, usage_log, which,
};
use crate::vault::walk::resolve_vault;

pub fn run() -> Result<i32> {
    if recursion_guard_active() {
        return Ok(0);
    }

    load_config_env();

    let log_path = std::env::var("MEMORY_REVIEW_LOG").unwrap_or_else(|_| "/tmp/claude-memory-review.log".into());
    let log_path = PathBuf::from(log_path);
    rotate_log(&log_path);

    let claude_bin = match std::env::var("CLAUDE_BIN").ok().filter(|s| !s.is_empty()) {
        Some(p) if Path::new(&p).is_file() => p,
        _ => match which("claude") {
            Some(p) => p,
            None => {
                log_line(&log_path, "skipped: `claude` CLI not found in PATH");
                return Ok(0);
            }
        },
    };

    // Read payload (stdin JSON).
    let mut payload_text = String::new();
    let _ = std::io::stdin().read_to_string(&mut payload_text);
    let payload: Value = serde_json::from_str(&payload_text).unwrap_or(Value::Null);
    let transcript = payload.get("transcript_path").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let session_id = payload.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();

    let project_dir = std::env::var("CLAUDE_PROJECT_DIR")
        .ok().filter(|s| !s.is_empty())
        .unwrap_or_else(|| std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
    let project_name = Path::new(&project_dir).file_name()
        .map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let today = Local::now().format("%Y-%m-%d").to_string();
    let now_hm = Local::now().format("%H:%M").to_string();

    if transcript.is_empty() || !Path::new(&transcript).is_file() {
        log_line(&log_path, &format!("skipped: no transcript at '{transcript}'"));
        return Ok(0);
    }

    let vault = resolve_vault(None);
    if !vault.is_dir() {
        log_line(&log_path, &format!("skipped: vault not found at '{}'", vault.display()));
        return Ok(0);
    }

    // Slim the transcript (cuts review token cost ~95% on real sessions). On
    // failure, fall back to the raw transcript.
    let slim_enabled = std::env::var("OBSIDIAN_MEMORY_SLIM_TRANSCRIPT")
        .map(|v| v != "false").unwrap_or(true);
    let mut effective_transcript = transcript.clone();
    let mut slim_path: Option<PathBuf> = None;
    if slim_enabled {
        if let Some(slim) = slim_transcript_to_tmp(&transcript, &log_path) {
            effective_transcript = slim.to_string_lossy().into_owned();
            slim_path = Some(slim);
        }
    }

    let run_review = std::env::var("OBSIDIAN_MEMORY_REVIEW_ENABLED")
        .map(|v| v == "true").unwrap_or(true);
    let autocommit = std::env::var("OBSIDIAN_MEMORY_AUTOCOMMIT")
        .map(|v| v == "true").unwrap_or(true);
    let autopush = std::env::var("OBSIDIAN_MEMORY_AUTOPUSH")
        .map(|v| v == "true").unwrap_or(false);

    if !run_review {
        log_line(&log_path, "OBSIDIAN_MEMORY_REVIEW_ENABLED=false; skipping review, will still commit dirty vault state");
    }

    // Vault HEAD recorded at SessionStart, for backlink reconciliation scoping.
    let session_state_dir = std::env::var("MEMORY_SESSION_STATE_DIR")
        .unwrap_or_else(|_| "/tmp/claude-memory-session".into());
    let session_state_dir = PathBuf::from(session_state_dir);
    let safe_sid = safe_session_id(&session_id);
    let vault_head_file = session_state_dir.join(format!("{safe_sid}.vault_head"));
    let vault_head = std::fs::read_to_string(&vault_head_file).ok()
        .map(|s| s.trim().to_string()).unwrap_or_default();
    let vault_head_display = if vault_head.is_empty() { "(none)".to_string() } else { vault_head.clone() };

    let plugin_root = resolve_plugin_root();
    let review_prompt = build_review_prompt(
        &vault, &effective_transcript, &project_name, &project_dir,
        &today, &now_hm, &vault_head_display, plugin_root.as_deref(),
    );

    // Persist state to a workfile, then spawn a detached worker that does the
    // review + autocommit. The hook itself returns once the worker is on its
    // own — Claude Code shutdown isn't blocked by review wallclock time.
    let state = WorkerState {
        run_review, autocommit, autopush,
        claude_bin: claude_bin.clone(),
        review_prompt,
        log_path: log_path.clone(),
        session_id: session_id.clone(),
        slim_path: slim_path.clone(),
        safe_sid: safe_sid.clone(),
        session_state_dir: session_state_dir.clone(),
        vault: vault.clone(),
        project_name: project_name.clone(),
    };

    if let Some(state_file) = persist_state(&state) {
        let _ = spawn_worker(&state_file);
    }

    Ok(0)
}

fn rotate_log(log_path: &Path) {
    let max: u64 = std::env::var("MEMORY_LOG_MAX_BYTES").ok()
        .and_then(|s| s.parse().ok()).unwrap_or(1_048_576);
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

fn slim_transcript_to_tmp(transcript: &str, log_path: &Path) -> Option<PathBuf> {
    use std::fs::File;
    let dir = std::env::temp_dir();
    let name = format!("claude-memory-slim.{}.txt", std::process::id());
    let out = dir.join(name);
    let f = File::create(&out).ok()?;
    let mut writer = std::io::BufWriter::new(f);
    let bytes_in = std::fs::metadata(transcript).ok().map(|m| m.len()).unwrap_or(0);
    if crate::transcript::slim_to_writer(Path::new(transcript), &mut writer).is_err() {
        log_line(log_path, "slim helper failed; falling back to raw transcript");
        let _ = std::fs::remove_file(&out);
        return None;
    }
    drop(writer); // flush
    let bytes_out = std::fs::metadata(&out).ok().map(|m| m.len()).unwrap_or(0);
    log_line(log_path, &format!("slimmed transcript: {bytes_in} → {bytes_out} bytes"));
    Some(out)
}

fn resolve_plugin_root() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CLAUDE_PLUGIN_ROOT") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    let exe = std::env::current_exe().ok()?;
    let resolved = std::fs::canonicalize(&exe).ok().unwrap_or(exe);
    resolved.parent().and_then(|d| d.parent()).map(|p| p.to_path_buf())
}

#[allow(clippy::too_many_arguments)]
fn build_review_prompt(
    vault: &Path,
    transcript: &str,
    project_name: &str,
    project_dir: &str,
    today: &str,
    now: &str,
    vault_head_display: &str,
    plugin_root: Option<&Path>,
) -> String {
    // Inline types.md so the review prompt sees full type definitions.
    let types_doc = include_str!("../../templates/types.md");
    let pr = plugin_root.map(|p| p.display().to_string()).unwrap_or_default();
    // Prefer `bin/run` so review prompts survive plugin-version upgrades —
    // the wrapper picks up whatever binary version the active cache has.
    let bin = format!("{pr}/bin/run");
    let vault_changes_cmd = if !vault_head_display.is_empty() && vault_head_display != "(none)" {
        format!("\"{bin}\" vault --vault '{}' vault-changes --base-sha {vault_head_display}", vault.display())
    } else {
        format!("\"{bin}\" vault --vault '{}' vault-changes", vault.display())
    };
    format!(
r####"End-of-session memory review.

Vault:        {vault}
Transcript:   {transcript}
Project:      {project_name} (at {project_dir})
Date / time:  {today} {now}
Vault HEAD at session start: {vault_head_display}

Do steps 1-3 first. Step 4 (the journal) is written last so its bullets can reference everything you wrote.

1. PROACTIVE NOTES — capture moments in the transcript where information surfaces that is stable across sessions, useful in future sessions, and not derivable from the codebase or git history. Covers corrections, preferences, validated approaches, always / from now on / stop doing X rules, decisions and rationale, novel facts (people, IDs, configs, channels, dashboards, endpoints), AND the synthesis of any multi-source research the assistant did during the session (read 3+ docs, compared options, mapped a landscape — capture as `findings` so a future session doesn't redo the work). Skip if already covered (verify via `"{bin}" vault --vault {vault} search --type <t> --keywords <k> --json`; extend a near-duplicate rather than creating a new note).

   Type semantics — the canonical definitions live in {pr}/templates/types.md. Read that file before classifying notes; it defines all seven types and the multi-type rules.

=== TYPES.MD (canonical definitions) ===
{types_doc}
=== END TYPES.MD ===

   Pick one or more types per note (never `journal` here — journal is step 4). Multi-type allowed: a note that genuinely spans axes can declare e.g. `[findings, decision]`; the first type drives routing.

   Routing (PRIMARY = types[0]):

     A. PRIMARY == tool       → {vault}/Tools/<slug>.md
     B. PRIMARY == preference → {vault}/Notes/<slug>.md  (add `project: {project_name}` only if narrowly scoped)
     C. PRIMARY ∈ {{reference, findings, decision, learning}}:
        1. If {project_dir} is registered + enabled in projects.json
           (check via `"{bin}" projects lookup {project_dir}`)
           AND has a folder matching the primary type
           (check via `"{bin}" project-docs match-type-folder {project_dir} --type <PRIMARY>`):
             → {project_dir}/<matched-folder>/<slug>.md  (with project: from the registry)
        2. Otherwise → {vault}/Notes/<slug>.md  (with `project: {project_name}` if project-scoped)

   Frontmatter on every new note: type, description, created_at, updated_at, updated_by (+ project when scoped). Set `created_at` and `updated_at` to the current local time in ISO 8601 with offset (e.g. `2026-05-03T22:30:00+08:00`). Set `updated_by: hook` (this review IS the hook). When you MODIFY an existing note in step 2, bump `updated_at` to now and set `updated_by: hook`. `type:` is either a single string (`type: decision`) or a YAML list (`type: [findings, decision]`).

   Always wrap the `description:` value in double quotes (e.g. `description: "one-line hook"`). Descriptions often contain `:`, `[[wikilinks]]`, or `[brackets]` — unquoted, these break YAML parsing. Escape any embedded `"` as `\"`. This rule also applies when you rewrite an existing note's `description` (step 2) or the journal's day-summary `description` (step 4).

2. MODIFY existing notes only on explicit user correction in the transcript. Smallest edit. Inferred staleness → flag in output, do not edit.

   When you extend or correct a non-journal note, check its frontmatter `description` against the new body. If the one-line summary no longer fits, rewrite it (smallest edit). The SessionStart auto-overview is built from these descriptions — stale ones mislead future sessions.

3. INTEGRITY — operates on:
   (a) vault notes touched in steps 1-2 above
   (b) non-journal vault notes referenced (wikilinks/paths) in any prior-session entry of today's journal
   (c) vault *.md files changed since the last commit (for backlink reconciliation on renames/deletes)

   Enumerate (c):
     {vault_changes_cmd}

   Per-source checks:

   - (a) + (b): frontmatter completeness (type, description, created_at; + project when project-scoped); every [[wikilink]] resolves; description-vs-body drift (rewrite description on drift, smallest edit). On any rewrite, bump `updated_at` to now and set `updated_by: hook`.

   - (c) Vault file changes — BACKLINK RECONCILIATION:
       * RENAMED (old → new) → `"{bin}" vault --vault {vault} incoming-wikilinks --target <old>` to find every note linking to the OLD path. Auto-rewrite each occurrence to the NEW path (smallest edit; preserve any |alias text). For bare basename links, prefer the new basename. List rewrites under "## Backlink rewrites".
       * DELETED → same command on the deleted path to find broken backlinks. List under "## Broken backlinks (target deleted)". DO NOT auto-fix — deletion may be intentional or may be a rename the diff couldn't infer.
       * ADDED / MODIFIED → no backlink action needed.

   Auto-fix unambiguous issues (description drift, backlink-rename). List ambiguous and non-fixable items in their dedicated sections.

4. JOURNAL — always, written LAST.
   Path: {vault}/Journals/{project_name}/{today}.md

   Journals are scoped one-file-per-project-per-day: the directory `{project_name}/` segregates this project's day from any other project's day. Use the `Write` tool — it creates parent directories automatically.

   New file: frontmatter (type=journal, description=<one-line day summary>, project={project_name}, created_at=<now ISO 8601 with offset, e.g. {today}T{now}±HH:MM>, updated_at=<same>, updated_by=hook) + "## Session {now}" + 3-6 bullets covering work, decisions, learnings.

   Existing file: append a "## Session {now}" section. Do not edit any prior content (earlier sessions today, earlier days). You MAY (and should) rewrite the frontmatter `description` to summarize the full day now that more sessions exist; whenever you touch frontmatter, bump `updated_at` and set `updated_by: hook`.

   Each bullet that describes a write must include the path:
   - Vault writes (steps 1-2) → vault-relative path.
   - Project-vault writes (step 1, route C.1) → repo-relative path inside {project_dir}.
   The journal is the cross-session anchor; paths in bullets are how future sessions find the work.

OUTPUT sections (in order, omit when empty):
  ## Vault writes              (paths created/appended in the personal vault)
  ## Project-vault writes      (paths in {project_dir}'s registered project-vault)
  ## Backlink rewrites         (notes whose [[wikilinks]] were updated for renames)
  ## Broken backlinks (target deleted)
  ## Integrity flags           (everything ambiguous or deferred)

No narrative outside these sections.
"####,
        vault = vault.display(),
    )
}

#[derive(serde::Serialize, serde::Deserialize)]
struct WorkerState {
    run_review: bool,
    autocommit: bool,
    autopush: bool,
    claude_bin: String,
    review_prompt: String,
    log_path: PathBuf,
    session_id: String,
    slim_path: Option<PathBuf>,
    safe_sid: String,
    session_state_dir: PathBuf,
    vault: PathBuf,
    project_name: String,
}

fn persist_state(state: &WorkerState) -> Option<PathBuf> {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("claude-memory-session-end.{}.json", std::process::id()));
    let json = serde_json::to_string(state).ok()?;
    std::fs::write(&path, json).ok()?;
    Some(path)
}

/// Spawn a fully-detached worker. Mirrors bash's `nohup ... &` semantics.
fn spawn_worker(state_file: &Path) -> Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.arg("hook").arg("session-end-bg").arg("--state-file").arg(state_file);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            // SAFETY: setsid in pre_exec runs after fork() but before execvp() — the
            // child has a single thread, no async-signal-unsafe state. Detaches from
            // the controlling terminal so the worker survives the parent (Claude
            // Code's hook process) exiting.
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    let _ = cmd.spawn();
    Ok(())
}

// ---------------------------------------------------------------------------
// Background worker (invoked via `obsidian-memory hook session-end-bg --state-file ...`).
// ---------------------------------------------------------------------------

/// Entry point for the detached worker. Hidden CLI subcommand.
pub fn run_bg(state_file: &Path) -> Result<i32> {
    let text = std::fs::read_to_string(state_file)?;
    let state: WorkerState = serde_json::from_str(&text)?;

    // Best-effort cleanup of state file once we've read it.
    let _ = std::fs::remove_file(state_file);

    if state.run_review {
        log_line(&state.log_path, &format!(
            "starting review for project={} (worker pid {})",
            state.project_name, std::process::id(),
        ));
        let mut cmd = Command::new(&state.claude_bin);
        cmd.args(["-p", &state.review_prompt,
                  "--tools", "Read,Write,Edit,Bash",
                  "--strict-mcp-config",
                  "--output-format", "json"]);
        cmd.env("CLAUDE_MEMORY_REVIEW", "1");
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        match cmd.output() {
            Ok(out) => {
                if let Ok(events) = serde_json::from_slice::<Value>(&out.stdout) {
                    if let Some(arr) = events.as_array() {
                        if let Some(result) = arr.iter().find(|e| e.get("type").and_then(|t| t.as_str()) == Some("result")) {
                            let usage = result.get("usage").cloned().unwrap_or(Value::Object(serde_json::Map::new()));
                            let cost = result.get("total_cost_usd").and_then(|v| v.as_f64()).map(|f| f.to_string());
                            let duration = result.get("duration_ms").and_then(|v| v.as_u64()).map(|u| u.to_string());
                            let usage_str = serde_json::to_string(&usage).unwrap_or_default();
                            usage_log::append_api(&state.session_id, "review_call", &usage_str, cost.as_deref(), duration.as_deref());
                        }
                    }
                }
                let exit_code = out.status.code().unwrap_or(-1);
                log_line(&state.log_path, &format!("review complete (exit={exit_code})"));
            }
            Err(e) => {
                log_line(&state.log_path, &format!("review subprocess failed: {e}"));
            }
        }
    }

    // Cleanup transcripts + state.
    if let Some(slim) = &state.slim_path {
        let _ = std::fs::remove_file(slim);
    }
    if !state.safe_sid.is_empty() {
        let _ = std::fs::remove_file(state.session_state_dir.join(format!("{}.vault_head", state.safe_sid)));
    }

    if state.autocommit && state.vault.join(".git").is_dir() {
        autocommit(&state.vault, &state.project_name, state.autopush, &state.log_path);
    }

    Ok(0)
}

fn autocommit(vault: &Path, project_name: &str, autopush: bool, log_path: &Path) {
    use fs2::FileExt;
    let lock_path = vault.join(".git/.claude-memory.lock");
    let lock = match OpenOptions::new().create(true).truncate(false).write(true).open(&lock_path) {
        Ok(f) => f,
        Err(_) => { log_line(log_path, "lock open failed, skipping commit"); return; }
    };
    if lock.try_lock_exclusive().is_err() {
        log_line(log_path, "lock timeout, skipping commit");
        return;
    }

    // git status --porcelain — empty → nothing to commit.
    let status = Command::new("git").arg("-C").arg(vault).args(["status", "--porcelain"])
        .output();
    let dirty = match status {
        Ok(o) => !o.stdout.is_empty(),
        Err(_) => { log_line(log_path, "git status failed, skipping commit"); return; }
    };
    if !dirty {
        log_line(log_path, "vault clean — nothing to commit");
        return;
    }

    let now_ts = Local::now().format("%Y-%m-%d %H:%M");
    let _ = Command::new("git").arg("-C").arg(vault).args(["add", "-A"]).status();
    let commit = Command::new("git").arg("-C").arg(vault)
        .args(["commit", "-m", &format!("session writes {now_ts} ({project_name})")])
        .output();
    match commit {
        Ok(o) if o.status.success() => {
            log_line(log_path, "vault auto-committed");
            if autopush {
                let push = Command::new("git").arg("-C").arg(vault).arg("push").output();
                match push {
                    Ok(p) if p.status.success() => log_line(log_path, "vault auto-pushed"),
                    _ => log_line(log_path, "vault push failed"),
                }
            }
        }
        _ => log_line(log_path, "vault commit failed"),
    }
}
