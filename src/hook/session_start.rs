//! SessionStart hook — port of `hooks/scripts/session-start.sh`.
//!
//! Stdout becomes context injected at the start of every Claude Code session.
//! Output text is load-bearing — the gate model and the user-facing setup
//! flow both parse it. Format is matched byte-equal where the bash original
//! had stable text; references to Python scripts are swapped for the Rust
//! binary's equivalents (e.g. `python3 _projects.py lookup` →
//! `obsidian-memory projects lookup`).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use serde_json::Value;

use crate::hook::{
    ensure_statusline, load_config_env, overview_cache, recursion_guard_active, safe_session_id,
    usage_log,
};
use crate::project_docs::enumerate_project_docs;
use crate::projects;
use crate::vault::walk::{resolve_vault, vault_display_path};

pub fn run() -> Result<i32> {
    if recursion_guard_active() {
        return Ok(0);
    }

    // Stdin payload — only used for session_id (for usage logging).
    let mut payload_text = String::new();
    let _ = std::io::stdin().read_to_string(&mut payload_text);
    let session_id = serde_json::from_str::<Value>(&payload_text)
        .ok()
        .and_then(|v| v.get("session_id").and_then(|s| s.as_str()).map(str::to_string))
        .unwrap_or_default();

    load_config_env();
    // For display/heredoc text, use the un-canonicalized path so the user sees
    // the same string they (or their config.env) typed — `resolve_vault` would
    // resolve `/tmp` → `/private/tmp` and confuse new users. For
    // `is_dir()` checks both forms are equivalent.
    let vault_display = vault_display_path();
    let vault = resolve_vault(None);
    let plugin_root = resolve_plugin_root();
    let project_dir = std::env::var("CLAUDE_PROJECT_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
    let project_name = Path::new(&project_dir)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    // Refresh the stable binary symlink + statusline wrapper. Plugin upgrades
    // move CLAUDE_PLUGIN_ROOT to a new versioned path; this re-points the
    // stable target so existing settings keep working without re-setup.
    if vault.is_dir() {
        if let (Some(plugin_root), Some(home)) = (plugin_root.as_ref(), dirs::home_dir()) {
            let config_dir = home.join(".config/obsidian-memory");
            if config_dir.is_dir() {
                let stable = config_dir.join("obsidian-memory");
                let target_bin = plugin_root.join("bin/obsidian-memory");
                let _ = fs::remove_file(&stable);
                #[cfg(unix)]
                let _ = std::os::unix::fs::symlink(&target_bin, &stable);
                let _ = ensure_statusline::quiet(&stable);
            }
        }
    }

    // Project-vault registration status.
    let mut registration_prompt: Option<RegistrationPromptCtx> = None;
    let mut project_vault_path: Option<PathBuf> = None;
    let project_root_str = git_toplevel(&project_dir).unwrap_or_default();
    if !project_root_str.is_empty() {
        let lookup = projects::lookup_status(&project_root_str);
        match lookup.status.as_str() {
            "enabled" => {
                // Eager init so newly-added docs are surfaced this session.
                let repo_path = PathBuf::from(&project_root_str);
                let _ = crate::project_init::init_project_vault_silent(
                    &repo_path, &project_name,
                );
                project_vault_path = Some(repo_path);
            }
            "not_registered" => {
                let candidate_count = enumerate_project_docs(Path::new(&project_root_str)).len();
                if candidate_count > 0 {
                    registration_prompt = Some(RegistrationPromptCtx {
                        project_root: project_root_str.clone(),
                        candidate_count,
                    });
                }
            }
            _ => { /* disabled or unknown — silent */ }
        }
    }

    // Side effects that only fire when the vault exists.
    if vault.is_dir() {
        try_open_obsidian_app();
        record_vault_head(&vault, &session_id);
    }

    // Buffer everything so we can both stream it and log byte size for /usage.
    let mut buf = Vec::<u8>::new();

    if !vault.is_dir() {
        // First-time-setup branch.
        write!(buf, "{}", first_time_setup_block(&vault_display, plugin_root.as_deref()))?;
        if let Some(ctx) = &registration_prompt {
            write!(buf, "\n{}", registration_prompt_block(ctx, &project_name, true))?;
        }
    } else {
        if let Some(ctx) = &registration_prompt {
            writeln!(buf, "{}", registration_prompt_block(ctx, &project_name, false))?;
        }
        write!(buf, "{}", instructions_block(&vault_display))?;
        if let Ok(readme) = fs::read_to_string(vault.join("README.md")) {
            writeln!(buf, "=== VAULT README ===")?;
            buf.extend_from_slice(readme.as_bytes());
            writeln!(buf)?;
        }
        // Overview (cached).
        let bootstrap_on = std::env::var("OBSIDIAN_MEMORY_BOOTSTRAP_OVERVIEW")
            .map(|v| v != "false").unwrap_or(true);
        let overview = overview_cache::get_or_build(
            &vault, &project_name, project_vault_path.as_deref(),
        );
        if bootstrap_on {
            writeln!(buf, "=== VAULT OVERVIEW (auto-generated from frontmatter) ===")?;
            if !overview.is_empty() {
                buf.extend_from_slice(overview.as_bytes());
                if !overview.ends_with('\n') {
                    writeln!(buf)?;
                }
            } else {
                writeln!(buf, "(overview generation failed — check that the binary is on PATH)")?;
            }
            writeln!(buf)?;
        }
    }

    // Stream and log size.
    let stdout = std::io::stdout();
    let mut h = stdout.lock();
    h.write_all(&buf)?;
    if !session_id.is_empty() {
        usage_log::append_chars(&session_id, "session_start", buf.len() as u64);
    }
    Ok(0)
}

struct RegistrationPromptCtx {
    project_root: String,
    candidate_count: usize,
}

fn first_time_setup_block(vault_display: &str, plugin_root: Option<&Path>) -> String {
    let pr = plugin_root
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "$CLAUDE_PLUGIN_ROOT".to_string());
    // Prefer `bin/run setup` over `bin/obsidian-memory setup`: the wrapper is
    // version-stable across plugin upgrades and lazily reinstalls the binary
    // if a future update purges it.
    let setup_cmd = format!("\"{pr}/bin/run\" setup");
    format!(
r##"=== OBSIDIAN MEMORY (first-time setup) ===

The plugin is installed but no vault exists yet at: {vault}

The plugin owns this vault entirely — three top-level folders (Tools/,
Journals/, Notes/) plus a README. Project scoping is via the project:
frontmatter tag on individual notes, not folder hierarchy.

Before doing anything else this session, ask the user ONCE:
  "Set up the obsidian-memory vault at {vault}? This creates the
   vault directory with Tools/, Journals/, Notes/ subfolders, and writes a
   config to ~/.config/obsidian-memory/. Fully reversible. (y/n)"

If YES:
  1. {setup_cmd}
  2. Ask: "Initialize the vault as a git repo so SessionEnd can auto-commit
     memory writes? (y/n)"
     If yes:
       cd "{vault}" && git init -b main && git add -A && git commit -m "Initial commit"
  3. Summarize what was created, then continue with the user's original request.

If NO, respect that: do not write to the vault this session. The user can
run setup later with:
  {setup_cmd}
or check current state with:
  /obsidian-memory:status

To use a different vault path, set OBSIDIAN_VAULT_PATH in
~/.config/obsidian-memory/config.env first.
"##,
        vault = vault_display,
    )
}

fn registration_prompt_block(ctx: &RegistrationPromptCtx, project_name: &str, after_setup: bool) -> String {
    let header = if after_setup {
        "After completing vault setup above, also ask the user:"
    } else {
        "Before responding to the user's first message this session, ask them:"
    };
    format!(
r##"=== ACTION REQUIRED — project-vault registration (one-time) ===

{header}

  "Register '{project_name}' as a project-vault? This will:
    - Add Obsidian frontmatter (type/description/created/project) to .md files
      that don't already have any frontmatter (idempotent — files with
      frontmatter are skipped)
    - Surface those docs in future SessionStart overviews and vault-search results
    - Route project-scoped save-memory writes to the matching project folder
      when one exists
   Answer y/n."

Context: this project ({project_name} at {project_root}) has {count}
candidate .md file(s) and is not yet registered. The prompt only fires once
per project — do not skip it.

YES → run both:
  /obsidian-memory:project enable "{project_root}"

NO → run:
  /obsidian-memory:project disable "{project_root}"

Either way, the answer is durable. To revisit later, edit
~/.config/obsidian-memory/projects.json.
"##,
        project_root = ctx.project_root,
        count = ctx.candidate_count,
    )
}

fn instructions_block(vault_display: &str) -> String {
    format!(
r##"=== OBSIDIAN MEMORY ===

Vault: {vault}
Index: the auto-overview below — regenerated each session from frontmatter.

RECALL — invoke the `vault-search` skill for body-level lookups (the gate above only matches descriptions).
REMEMBER — invoke the `save-memory` skill to write notes.

"##,
        vault = vault_display,
    )
}

fn resolve_plugin_root() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CLAUDE_PLUGIN_ROOT") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    // Fallback: derive from the binary's location. Binary lives at
    // <PLUGIN_ROOT>/bin/obsidian-memory (post-Phase-5 layout).
    let exe = std::env::current_exe().ok()?;
    let resolved = std::fs::canonicalize(&exe).ok().unwrap_or(exe);
    resolved.parent().and_then(|d| d.parent()).map(|p| p.to_path_buf())
}

fn git_toplevel(cwd: &str) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(cwd).args(["rev-parse", "--show-toplevel"])
        .output().ok()?;
    if !out.status.success() { return None; }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

fn try_open_obsidian_app() {
    // macOS only — silent on Linux.
    if cfg!(target_os = "macos") {
        let _ = Command::new("open").args(["-ga", "Obsidian"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

fn record_vault_head(vault: &Path, session_id: &str) {
    if session_id.is_empty() { return; }
    if !vault.join(".git").exists() { return; }
    let state_dir = std::env::var("MEMORY_SESSION_STATE_DIR")
        .unwrap_or_else(|_| "/tmp/claude-memory-session".into());
    let state_dir = PathBuf::from(state_dir);
    if fs::create_dir_all(&state_dir).is_err() { return; }
    let safe = safe_session_id(session_id);
    let Ok(out) = Command::new("git").arg("-C").arg(vault)
        .args(["rev-parse", "HEAD"]).output() else { return };
    if !out.status.success() { return; }
    let head_file = state_dir.join(format!("{safe}.vault_head"));
    let _ = fs::write(&head_file, out.stdout);
}
