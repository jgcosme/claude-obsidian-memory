//! Filesystem walking + vault path resolution.
//!
//! Mirrors `_vault.py:resolve_vault`, `collect_md_files`, and `SKIP_DIRS`.

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

pub const SKIP_DIRS: &[&str] = &[".git", ".obsidian", ".trash", "node_modules", ".archive"];

pub fn collect_md_files(vault: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let walker = WalkDir::new(vault)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.file_type().is_dir() {
                let name = e.file_name().to_string_lossy();
                // Always allow the root entry through (its file_name is the
                // last path component of the root, which could in theory
                // collide with a SKIP_DIR — keep the root unfiltered).
                if e.depth() == 0 {
                    return true;
                }
                !SKIP_DIRS.iter().any(|s| *s == name)
            } else {
                true
            }
        });
    for entry in walker.flatten() {
        if entry.file_type().is_file() {
            let p = entry.path();
            if p.extension().map(|e| e == "md").unwrap_or(false) {
                out.push(p.to_path_buf());
            }
        }
    }
    out.sort();
    out
}

/// Mirror bash's `OBSIDIAN_VAULT_PATH="${OBSIDIAN_VAULT_PATH:-$HOME/Documents/Obsidian Memory}"`
/// — return the env-or-default string WITHOUT canonicalization. Used for
/// user-facing display: `resolve_vault` would resolve `/var` → `/private/var`
/// and confuse new users on macOS. For filesystem ops use `resolve_vault`.
pub fn vault_display_path() -> String {
    if let Ok(v) = std::env::var("OBSIDIAN_VAULT_PATH") {
        if !v.is_empty() {
            return v;
        }
    }
    if let Some(home) = dirs::home_dir() {
        return home.join("Documents/Obsidian Memory").to_string_lossy().into_owned();
    }
    "Obsidian Memory".to_string()
}

/// Resolve the plugin's vault dir.
///
/// Resolution order (matches Python):
///   1. `cli_vault` flag
///   2. `$OBSIDIAN_VAULT_PATH`
///   3. `OBSIDIAN_VAULT_PATH=` line in `~/.config/obsidian-memory/config.env`
///   4. `~/Documents/Obsidian Memory`
///
/// The returned path is canonicalized when it exists, else returned as-is
/// (resolved against `$HOME` for `~` expansion). Python uses `.resolve()`
/// which on POSIX returns the absolute path even when the target is absent;
/// we replicate that with a manual fallback.
pub fn resolve_vault(cli_vault: Option<&Path>) -> PathBuf {
    if let Some(v) = cli_vault {
        return absolute(&expand_user(v));
    }
    if let Ok(v) = std::env::var("OBSIDIAN_VAULT_PATH") {
        if !v.is_empty() {
            return absolute(&expand_user(Path::new(&v)));
        }
    }
    if let Some(home) = dirs::home_dir() {
        let config = home.join(".config/obsidian-memory/config.env");
        if let Ok(text) = std::fs::read_to_string(&config) {
            for line in text.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("OBSIDIAN_VAULT_PATH=") {
                    let rest = rest.trim();
                    let unquoted = rest
                        .strip_prefix('"').and_then(|s| s.strip_suffix('"'))
                        .or_else(|| rest.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
                        .unwrap_or(rest);
                    if !unquoted.is_empty() {
                        return absolute(&expand_user(Path::new(unquoted)));
                    }
                }
            }
        }
        return absolute(&home.join("Documents/Obsidian Memory"));
    }
    PathBuf::from("Obsidian Memory")
}

/// Expand a leading `~` against `$HOME`. Mirrors `os.path.expanduser`.
pub fn expand_user(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    if s == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    p.to_path_buf()
}

/// Resolve the path the way Python's `Path(p).resolve()` does on POSIX: walk
/// up to the deepest *existing* ancestor, canonicalize that (so symlinks like
/// `/tmp` → `/private/tmp` get resolved), then re-attach the missing tail.
///
/// Plain `std::fs::canonicalize` requires the *entire* path to exist, which
/// would skip symlink resolution on first-time-write paths and diverge from
/// the Python registry's resolved keys.
pub fn absolute(p: &Path) -> PathBuf {
    let abs: PathBuf = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().map(|c| c.join(p)).unwrap_or_else(|_| p.to_path_buf())
    };

    let mut existing = abs.clone();
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        match (existing.file_name().map(|s| s.to_os_string()), existing.parent().map(|p| p.to_path_buf())) {
            (Some(name), Some(parent)) => {
                missing.push(name);
                existing = parent;
            }
            _ => return abs, // hit root without finding an existing ancestor
        }
    }
    let canonical = std::fs::canonicalize(&existing).unwrap_or(existing);
    let mut result = canonical;
    for c in missing.into_iter().rev() {
        result.push(c);
    }
    result
}
