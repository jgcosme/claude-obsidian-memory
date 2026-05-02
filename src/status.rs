//! Plugin diagnostic status — port of `scripts/status.sh`.
//!
//! Read-only. Verifies install, config, vault, scripts, and recent activity.
//! Output mirrors the bash original's structure and `[ok]/[warn]/[FAIL]`
//! markers; the prerequisites + scripts sections diverge from bash because
//! the Rust port no longer needs python3 or jq, and ships as a single binary.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;

use crate::hook::{load_config_env, which};
use crate::projects;
use crate::vault::search;
use crate::vault::walk::{resolve_vault, vault_display_path};

pub fn run() -> Result<i32> {
    load_config_env();

    let vault = resolve_vault(None);
    let plugin_root = std::env::var("CLAUDE_PLUGIN_ROOT").ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::current_exe().ok()
                .and_then(|exe| std::fs::canonicalize(&exe).ok().or(Some(exe)))
                .and_then(|resolved| resolved.parent().and_then(|d| d.parent()).map(|p| p.to_path_buf()))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    let config_file = dirs::home_dir()
        .map(|h| h.join(".config/obsidian-memory/config.env"))
        .unwrap_or_default();

    let gate_enabled    = env_default("OBSIDIAN_MEMORY_GATE_ENABLED", "true");
    let review_enabled  = env_default("OBSIDIAN_MEMORY_REVIEW_ENABLED", "true");
    let bootstrap_overv = env_default("OBSIDIAN_MEMORY_BOOTSTRAP_OVERVIEW", "true");
    let autocommit      = env_default("OBSIDIAN_MEMORY_AUTOCOMMIT", "true");
    let autopush        = env_default("OBSIDIAN_MEMORY_AUTOPUSH", "false");
    let log_review      = env_default("MEMORY_REVIEW_LOG", "/tmp/claude-memory-review.log");
    let log_gate        = env_default("MEMORY_GATE_LOG", "/tmp/claude-memory-gate.log");
    let cache_dir       = env_default("MEMORY_OVERVIEW_CACHE_DIR", "/tmp/claude-memory-overview-cache");

    let mut issues: u32 = 0;
    let ok = |m: &str| println!("  [ok]   {m}");
    let warn = |m: &str| println!("  [warn] {m}");
    let fail = |m: &str, issues: &mut u32| { println!("  [FAIL] {m}"); *issues += 1; };

    println!("obsidian-memory status");
    println!();

    println!("Config:");
    if config_file.is_file() {
        ok(&format!("config: {}", config_file.display()));
    } else {
        warn(&format!("config missing at {} (using defaults)", config_file.display()));
    }
    println!("  • vault:              {}", vault_display_path());
    println!("  • gate:               {gate_enabled}");
    println!("  • review:             {review_enabled}");
    println!("  • bootstrap-overview: {bootstrap_overv}");
    println!("  • autocommit:         {autocommit}");
    println!("  • autopush:           {autopush}");
    println!();

    println!("Prerequisites:");
    if which("git").is_some() { ok("git"); } else { warn("git (optional — needed for vault history + autocommit)"); }
    #[cfg(target_os = "macos")]
    {
        if which("flock").is_some() {
            ok("flock");
        } else {
            // Rust uses fs2's advisory lock natively, but we still report flock
            // status because the docs reference it for users not using the
            // bundled binary.
            warn("flock (optional)");
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        if which("flock").is_some() { ok("flock"); } else { warn("flock (optional)"); }
    }
    println!();

    println!("Vault:");
    if vault.is_dir() {
        ok("directory exists");
        if vault.join("README.md").is_file() { ok("README.md present"); }
        else { warn("no README.md (re-run setup)"); }
        if vault.join(".git").is_dir() {
            ok("git initialized");
            if autopush == "true" {
                let remote = Command::new("git").arg("-C").arg(&vault)
                    .args(["remote", "get-url", "origin"]).output().ok()
                    .and_then(|o| if o.status.success() {
                        Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                    } else { None })
                    .unwrap_or_default();
                if !remote.is_empty() { ok(&format!("git remote: {remote}")); }
                else { warn("autopush=true but no 'origin' remote"); }
            }
        } else if autocommit == "true" {
            warn("not a git repo, but autocommit=true — SessionEnd will silently no-op");
            println!("         to enable: cd \"{}\" && git init -b main && git add -A && git commit -m 'Initial commit'", vault_display_path());
        } else {
            warn("not a git repo (auto-commit disabled, so this is fine)");
        }
    } else {
        fail(&format!("vault not found at {} — run setup", vault_display_path()), &mut issues);
    }
    println!();

    println!("Project-vaults:");
    let project_dir = std::env::var("CLAUDE_PROJECT_DIR").ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
    let cwd_root = git_toplevel(&project_dir).unwrap_or_default();
    if cwd_root.is_empty() {
        println!("  • current cwd: (not a git repo — project-vault not applicable)");
    } else {
        let lookup = projects::lookup_status(&cwd_root);
        match lookup.status.as_str() {
            "enabled" => println!("  • current cwd: enabled ({})", lookup.project.unwrap_or_default()),
            "disabled" => println!("  • current cwd: disabled (declined registration)"),
            _ => println!("  • current cwd: not registered (SessionStart will offer to register)"),
        }
    }
    let registered = projects::list_registered();
    if registered.is_empty() {
        println!("  • no projects registered yet");
    } else {
        let total = registered.len();
        let enabled_n = registered.iter().filter(|r| r.enabled).count();
        println!("  • registered: {total} total · {enabled_n} enabled");
        for r in &registered {
            let mark = if r.enabled { "[on] " } else { "[off]" };
            let here = if r.path == cwd_root { "  ← current" } else { "" };
            println!("    {mark} {}  {}{here}", r.project, r.path);
        }
    }
    println!();

    println!("Plugin (root: {}):", plugin_root.display());
    let bin = plugin_root.join("bin/obsidian-memory");
    if bin.is_file() { ok(&format!("bin/obsidian-memory ({} KB)",
        std::fs::metadata(&bin).map(|m| m.len() / 1024).unwrap_or(0))); }
    else { fail("bin/obsidian-memory missing", &mut issues); }
    println!();

    println!("Search smoke test:");
    if vault.is_dir() {
        let opts = search::SearchOpts { limit: 9999, ..search::SearchOpts::default() };
        match search::search(&vault, opts) {
            Ok(hits) => ok(&format!("vault search returned {} notes", hits.len())),
            Err(_) => fail("vault search failed", &mut issues),
        }
    } else {
        warn("skipped (vault not available)");
    }
    println!();

    println!("Overview cache:");
    let cache_path = PathBuf::from(&cache_dir);
    if cache_path.is_dir() {
        let count = std::fs::read_dir(&cache_path).map(|d| d.filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("txt"))
            .count()).unwrap_or(0);
        ok(&format!("{cache_dir} ({count} cached)"));
    } else {
        warn(&format!("{cache_dir} not yet populated (created on next SessionStart)"));
    }
    println!();

    println!("Recent activity:");
    println!("  • review: {}", last_line(Path::new(&log_review)).unwrap_or_else(|| "no entries yet".to_string()));
    println!("  • gate:   {}", last_line(Path::new(&log_gate)).unwrap_or_else(|| "no entries yet".to_string()));
    println!();

    if issues > 0 {
        println!("{issues} issue(s) found — fix and re-run /obsidian-memory:status.");
        return Ok(1);
    }
    println!("All checks passed.");
    Ok(0)
}

fn env_default(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn git_toplevel(cwd: &str) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(cwd).args(["rev-parse", "--show-toplevel"])
        .output().ok()?;
    if !out.status.success() { return None; }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

fn last_line(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().rfind(|l| !l.trim().is_empty()).map(|s| s.to_string())
}
