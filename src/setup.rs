//! Plugin setup — port of `scripts/setup.sh`.
//!
//! Idempotently scaffolds:
//!   - `~/.config/obsidian-memory/` (config.env + secrets.env)
//!   - The vault dir + Tools/Journals/Notes scaffolding
//!   - Templates rendered with `__TODAY__` / `__VAULT_PATH__` substituted
//!   - Statusline wrapper + Claude Code settings.json patch
//!   - Obsidian.app registry entry (skipped if Obsidian.app is running)
//!
//! Reads `templates/` and `examples/` from the plugin root at runtime —
//! `${CLAUDE_PLUGIN_ROOT}` if set, otherwise the binary's parent's parent.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};
use chrono::Local;
use walkdir::WalkDir;

use crate::hook::{ensure_statusline, load_config_env, which};
use crate::vault::walk::vault_display_path;

pub fn run() -> Result<i32> {
    let plugin_root = resolve_plugin_root()?;
    let templates = plugin_root.join("templates");
    let examples = plugin_root.join("examples");

    if !templates.is_dir() {
        eprintln!("error: templates directory not found at {}", templates.display());
        eprintln!("If running outside Claude Code, set CLAUDE_PLUGIN_ROOT to the plugin install path.");
        return Ok(1);
    }

    let config_dir = dirs::home_dir().map(|h| h.join(".config/obsidian-memory")).unwrap_or_default();
    let config_file = config_dir.join("config.env");
    load_config_env();

    let vault_path = vault_display_path();
    let today = Local::now().format("%Y-%m-%d").to_string();

    println!("obsidian-memory setup");
    println!("  plugin root: {}", plugin_root.display());
    println!("  vault:       {vault_path}");
    println!("  config:      {}", config_file.display());
    println!();

    // ----- prerequisites -----
    println!("Checking prerequisites:");
    let missing = 0u32; // reserved for future hard-required prereqs
    // git is now the only optional-but-recommended; jq + python3 dropped.
    if which("git").is_some() {
        println!("  [ok]   git");
    } else {
        println!("  [warn] git — optional (vault history and SessionEnd auto-commit); install: preinstalled on most systems");
    }
    let flock_msg = if cfg!(target_os = "macos") {
        "concurrent-session safety on auto-commit (macOS); install: brew install flock"
    } else {
        "concurrent-session safety on auto-commit; install: your package manager"
    };
    if which("flock").is_some() {
        println!("  [ok]   flock");
    } else {
        println!("  [warn] flock — optional ({flock_msg})");
    }
    if missing > 0 {
        eprintln!();
        eprintln!("error: {missing} required prerequisite(s) missing — install and re-run.");
        return Ok(1);
    }
    println!();

    // ----- config + secrets -----
    if !config_file.is_file() {
        std::fs::create_dir_all(&config_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&config_dir, std::fs::Permissions::from_mode(0o700));
        }
        std::fs::copy(examples.join("config.env.example"), &config_file)?;
        println!("[+] created {} (edit to customize paths)", config_file.display());
    } else {
        println!("[=] config exists, leaving it alone: {}", config_file.display());
    }

    let secrets_file = config_dir.join("secrets.env");
    if !secrets_file.is_file() {
        std::fs::copy(examples.join("secrets.env.example"), &secrets_file)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&secrets_file, std::fs::Permissions::from_mode(0o600));
        }
        println!("[+] created {} (chmod 600; add credentials as needed)", secrets_file.display());
    } else {
        println!("[=] secrets exists, leaving it alone: {}", secrets_file.display());
    }

    // ----- vault scaffold -----
    let vault_path_buf = PathBuf::from(&vault_path);
    std::fs::create_dir_all(&vault_path_buf)?;
    std::fs::create_dir_all(vault_path_buf.join("Tools"))?;
    std::fs::create_dir_all(vault_path_buf.join("Journals"))?;
    std::fs::create_dir_all(vault_path_buf.join("Notes"))?;

    // ----- render templates -----
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(&templates).follow_links(false) {
        let Ok(entry) = entry else { continue };
        if entry.file_type().is_file()
            && entry.path().extension().map(|e| e == "md").unwrap_or(false)
        {
            paths.push(entry.path().to_path_buf());
        }
    }
    paths.sort();
    for src in &paths {
        let Ok(rel) = src.strip_prefix(&templates) else { continue };
        let dst = vault_path_buf.join(rel);
        if dst.is_file() {
            println!("[=] {} exists, skipping", dst.display());
            continue;
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = std::fs::read_to_string(src)?;
        let rendered = content.replace("__TODAY__", &today).replace("__VAULT_PATH__", &vault_path);
        std::fs::write(&dst, rendered)?;
        println!("[+] {}", dst.display());
    }

    // .gitignore at vault root: copy verbatim, no substitution.
    let gi_src = templates.join(".gitignore");
    let gi_dst = vault_path_buf.join(".gitignore");
    if gi_src.is_file() && !gi_dst.is_file() {
        std::fs::copy(&gi_src, &gi_dst)?;
        println!("[+] {}", gi_dst.display());
    }
    println!();

    // ----- statusline wiring -----
    let stable_bin = config_dir.join("obsidian-memory");
    let plugin_bin = plugin_root.join("bin/obsidian-memory");
    if plugin_bin.is_file() {
        let _ = std::fs::remove_file(&stable_bin);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&plugin_bin, &stable_bin)?;
        println!("[+] linked {} → {}", stable_bin.display(), plugin_bin.display());
        let _ = ensure_statusline::verbose(&stable_bin);
    } else {
        println!("[warn] {} not found — skipping status line wiring (expected post-install)", plugin_bin.display());
    }

    // ----- register vault with Obsidian.app -----
    register_with_obsidian(&vault_path_buf);

    println!();
    println!("Done. Next steps:");
    println!("  1. (Optional) Open the vault in Obsidian.app: open -a Obsidian \"{vault_path}\"");
    println!("  2. (Optional but recommended) Init git in the vault for change-tracking + auto-commit:");
    println!("       cd \"{vault_path}\" && git init -b main && git add -A && git commit -m 'Initial commit'");
    println!("  3. Edit {} to override defaults (vault path, gate behavior, autocommit/push).", config_file.display());
    println!("  4. cd into a project repo and start a Claude session — when prompted, answer 'yes'");
    println!("     to register it as a project-vault (or run /obsidian-memory:project enable later).");

    Ok(0)
}

fn resolve_plugin_root() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("CLAUDE_PLUGIN_ROOT") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    let exe = std::env::current_exe()?;
    let resolved = std::fs::canonicalize(&exe).unwrap_or(exe);
    match resolved.parent().and_then(|d| d.parent()) {
        Some(p) => Ok(p.to_path_buf()),
        None => bail!("could not derive plugin root from binary path"),
    }
}

fn register_with_obsidian(vault_path: &Path) {
    let registry = match std::env::consts::OS {
        "macos" => dirs::home_dir().map(|h| h.join("Library/Application Support/obsidian/obsidian.json")),
        "linux" => dirs::home_dir().map(|h| h.join(".config/obsidian/obsidian.json")),
        _ => None,
    };
    let Some(registry) = registry else { return };

    if !registry.is_file() {
        if let Some(parent) = registry.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&registry, "{\"vaults\":{}}");
    }

    let text = match std::fs::read_to_string(&registry) {
        Ok(t) => t,
        Err(_) => return,
    };
    let mut data: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return,
    };

    let target = vault_path.to_string_lossy().into_owned();
    let already = data.get("vaults").and_then(|v| v.as_object())
        .map(|m| m.values().any(|entry| entry.get("path").and_then(|p| p.as_str()) == Some(&target)))
        .unwrap_or(false);
    if already {
        println!("[=] vault already registered with Obsidian.app");
        return;
    }

    // Refuse to patch obsidian.json if Obsidian.app is currently running.
    let running = if cfg!(target_os = "macos") {
        Command::new("pgrep").args(["-x", "Obsidian"]).output()
            .map(|o| o.status.success() && !o.stdout.is_empty())
            .unwrap_or(false)
    } else {
        false
    };
    if running {
        println!("[skip] Obsidian.app is running — vault not auto-registered.");
        println!("       Register the vault by either:");
        println!("         1. Quit Obsidian and re-run this setup script, OR");
        println!("         2. In Obsidian: 'Open folder as vault' → choose {}", vault_path.display());
        return;
    }

    let vault_id = random_hex_token(16);
    let ts_ms = chrono::Utc::now().timestamp_millis() as u64;
    let backup = format!("{}.bak.{}", registry.display(), Local::now().format("%Y%m%d%H%M%S"));
    let _ = std::fs::copy(&registry, &backup);

    let vaults = data.get_mut("vaults").and_then(|v| v.as_object_mut());
    let Some(vaults) = vaults else { return };
    let mut entry = serde_json::Map::new();
    entry.insert("path".into(), serde_json::Value::String(target));
    entry.insert("ts".into(), serde_json::Value::from(ts_ms));
    vaults.insert(vault_id, serde_json::Value::Object(entry));

    let serialized = match serde_json::to_string_pretty(&data) {
        Ok(s) => s,
        Err(_) => return,
    };
    let tmp = registry.with_extension("json.tmp");
    if std::fs::write(&tmp, serialized).is_ok() && std::fs::rename(&tmp, &registry).is_ok() {
        println!("[+] registered vault with Obsidian.app");
    } else {
        println!("[warn] failed to write {} — register the vault manually via Obsidian's 'Open folder as vault'.", registry.display());
    }
}

/// Generate a `bytes`-byte random hex string. Used for the obsidian.json
/// vault ID — unpredictability is the only requirement.
fn random_hex_token(bytes: usize) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos().hash(&mut hasher);
    std::process::id().hash(&mut hasher);

    let mut out = String::with_capacity(bytes * 2);
    for i in 0..bytes {
        let mut h = hasher.clone();
        i.hash(&mut h);
        let v = h.finish();
        out.push_str(&format!("{:02x}", (v & 0xff) as u8));
    }
    out
}
