//! Self-heal Claude Code's statusLine wiring —
//! port of `scripts/_ensure_statusline.sh`.
//!
//! Idempotently:
//!   1. Writes a stable wrapper at `~/.config/obsidian-memory/statusline-gate.sh`.
//!      Wrapper contents are version-independent and survive plugin uninstall:
//!      the wrapper checks `enabledPlugins` before invoking the binary, so an
//!      orphan statusLine block (Claude Code has no PluginUninstall hook) becomes
//!      a dormant no-op rather than printing errors.
//!   2. Patches `~/.claude/settings.json`'s `statusLine.command` to invoke the
//!      wrapper. Migrates from the legacy `python3 <statusline.py>` form. Leaves
//!      user-customized commands alone.
//!
//! `target_binary` is the stable symlink path that the wrapper should exec —
//! typically `$CONFIG_DIR/obsidian-memory`, refreshed by session-start each
//! session so plugin-version bumps don't strand the wrapper.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::Local;
use serde_json::Value;

/// Quiet variant of the helper used by session-start. Errors are swallowed
/// (the hook must never block startup); the bash original exits 0 on every
/// failure too.
pub fn quiet(target_binary: &Path) -> Result<()> {
    run(target_binary, true)
}

#[allow(dead_code)] // wired to the (yet-unported) setup CLI in Phase 4
pub fn verbose(target_binary: &Path) -> Result<()> {
    run(target_binary, false)
}

fn run(target_binary: &Path, quiet: bool) -> Result<()> {
    let log = |msg: &str| {
        if !quiet {
            eprintln!("{msg}");
        }
    };

    let enabled = std::env::var("OBSIDIAN_MEMORY_STATUSLINE_ENABLED")
        .map(|v| v != "false")
        .unwrap_or(true);
    if !enabled {
        log("[=] status line disabled via OBSIDIAN_MEMORY_STATUSLINE_ENABLED — skipping settings patch");
        return Ok(());
    }

    let Some(config_dir) = target_binary.parent().map(PathBuf::from) else {
        return Ok(());
    };
    fs::create_dir_all(&config_dir)?;
    let wrapper = config_dir.join("statusline-gate.sh");

    write_wrapper(&wrapper, target_binary)?;
    let _ = fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755));

    let Some(home) = dirs::home_dir() else { return Ok(()); };
    let claude_settings = home.join(".claude/settings.json");
    if !claude_settings.is_file() {
        if let Some(parent) = claude_settings.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&claude_settings, "{}\n");
    }

    let text = match fs::read_to_string(&claude_settings) {
        Ok(t) => t,
        Err(_) => return Ok(()),
    };
    let mut data: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => {
            log(&format!("[warn] {} is not valid JSON — skipping status line patch.", claude_settings.display()));
            log("       Fix the file (or delete it to start fresh) and re-run setup.");
            return Ok(());
        }
    };

    let expected = format!("bash \"{}\"", wrapper.display());
    let legacy = format!("python3 \"{}\"", target_binary.display());

    let existing = data
        .get("statusLine")
        .and_then(|v| v.get("command"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    enum Action { Install, Migrate, Noop, Custom }
    let action = if existing.is_empty() {
        Action::Install
    } else if existing == expected {
        Action::Noop
    } else if existing == legacy {
        Action::Migrate
    } else {
        Action::Custom
    };

    match action {
        Action::Install | Action::Migrate => {
            // Backup before overwriting.
            let stamp = Local::now().format("%Y%m%d%H%M%S");
            let backup = format!("{}.bak.{stamp}", claude_settings.display());
            let _ = fs::copy(&claude_settings, &backup);

            let map = data.as_object_mut();
            let Some(map) = map else {
                log(&format!("[warn] {} is not a JSON object — skipping", claude_settings.display()));
                return Ok(());
            };
            let mut sl = serde_json::Map::new();
            sl.insert("type".into(), Value::String("command".into()));
            sl.insert("command".into(), Value::String(expected));
            map.insert("statusLine".into(), Value::Object(sl));

            // Match Python `json.dumps(indent=2)` shape (Claude Code's settings.json
            // is human-edited; preserving 2-space indent keeps diffs sane).
            let serialized = serde_json::to_string_pretty(&data)?;
            let tmp = claude_settings.with_extension("json.tmp");
            fs::write(&tmp, format!("{serialized}\n"))?;
            fs::rename(&tmp, &claude_settings)?;
            match action {
                Action::Install => log(&format!("[+] enabled status line in {}", claude_settings.display())),
                _ => log(&format!("[+] migrated status line to wrapper-based command in {}", claude_settings.display())),
            }
        }
        Action::Noop => log("[=] status line already enabled"),
        Action::Custom => {
            log("[=] status line already configured (left as-is). To use the plugin's:");
            log(&format!("    set statusLine.command in {} to:", claude_settings.display()));
            log(&format!("      bash \"{}\"", wrapper.display()));
        }
    }
    Ok(())
}

/// Wrapper script body. Self-disables when:
///   - the plugin is no longer in `enabledPlugins`
///   - the symlink target is gone (plugin cache purged)
///
/// In both cases exits 0 silently — Claude Code's status line just shows blank.
fn write_wrapper(path: &Path, target_binary: &Path) -> Result<()> {
    let body = format!(r##"#!/bin/bash
# obsidian-memory statusline gate. Stable wrapper invoked by Claude Code's
# statusLine.command. Exits silently when the plugin is uninstalled so the
# orphan statusLine entry in ~/.claude/settings.json (Claude Code has no
# PluginUninstall hook) becomes a dormant no-op.
#
# Reads stdin from Claude Code (passed through to the binary) and execs the
# `statusline` subcommand of the obsidian-memory binary via its stable symlink.
#
# Written by obsidian-memory's setup / SessionStart (idempotent).

CLAUDE_SETTINGS="${{HOME}}/.claude/settings.json"
TARGET="{target}"

# Self-disable if the binary symlink is gone (plugin cache purged).
[ -e "$TARGET" ] || exit 0
[ -f "$CLAUDE_SETTINGS" ] || exit 0

# Any obsidian-memory@<marketplace> entry enabled? Use a tiny grep — keeps the
# wrapper jq-free so it works on bare systems.
grep -E '"obsidian-memory@[^"]+"[[:space:]]*:[[:space:]]*true' "$CLAUDE_SETTINGS" >/dev/null 2>&1 || exit 0

exec "$TARGET" statusline
"##,
        target = target_binary.display(),
    );
    fs::write(path, body)?;
    Ok(())
}
