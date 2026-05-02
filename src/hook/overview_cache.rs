//! mtime-invalidated cache wrapping `vault::overview` —
//! port of `hooks/scripts/_overview.sh`.
//!
//! The gate runs every UserPromptSubmit; without this cache it would do a
//! full vault walk per prompt. Cache key is `sha1(vault|project|project_vault)`
//! — sha1 specifically (not blake3 etc.) so caches written by the bash impl
//! and the Rust impl can coexist during the cutover.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sha1::{Digest, Sha1};
use walkdir::WalkDir;

use crate::vault::overview;
use crate::vault::walk::SKIP_DIRS;

/// Return the cached or freshly-built vault overview text. Empty string on
/// any I/O failure (mirrors the bash helper's `exit 0` on errors — the gate
/// must never fail loud).
pub fn get_or_build(vault: &Path, project: &str, project_vault: Option<&Path>) -> String {
    if !vault.is_dir() {
        return String::new();
    }
    let cache_dir = std::env::var("MEMORY_OVERVIEW_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/claude-memory-overview-cache"));
    if std::fs::create_dir_all(&cache_dir).is_err() {
        return build_uncached(vault, project, project_vault).unwrap_or_default();
    }

    let key = cache_key(vault, project, project_vault);
    let cache = cache_dir.join(format!("{key}.txt"));

    // Cache hit: file exists, is non-empty, and no *.md in either corpus is
    // newer than the cache. Bash uses `find -newer ... -print -quit` (bails
    // at the first newer file). Replicate that early-exit behavior.
    if let Ok(meta) = std::fs::metadata(&cache) {
        if meta.len() > 0 {
            if let Ok(cache_mtime) = meta.modified() {
                if !any_md_newer(vault, cache_mtime)
                    && project_vault.map(|pv| !any_md_newer(pv, cache_mtime)).unwrap_or(true)
                {
                    if let Ok(text) = std::fs::read_to_string(&cache) {
                        return text;
                    }
                }
            }
        }
    }

    // Cache miss: regenerate, write atomically.
    let Some(text) = build_uncached(vault, project, project_vault) else { return String::new() };
    if !text.is_empty() {
        let pid = std::process::id();
        let tmp = cache.with_extension(format!("txt.tmp.{pid}"));
        if std::fs::write(&tmp, &text).is_ok() {
            let _ = std::fs::rename(&tmp, &cache);
        }
    }
    text
}

fn build_uncached(vault: &Path, project: &str, project_vault: Option<&Path>) -> Option<String> {
    let project_arg = if project.is_empty() { None } else { Some(project) };
    let mut out = overview::overview(vault, project_arg, "full").ok()?;
    // overview() returns the joined string without trailing newline; the bash
    // wrapper invokes `python3 ... overview` which `print()`s, adding a
    // trailing newline. Match that so the cache file is byte-equal.
    if !out.ends_with('\n') {
        out.push('\n');
    }
    if let Some(pv) = project_vault {
        if pv.is_dir() {
            let pv_text = overview::overview_project(pv, project_arg).ok()?;
            out.push('\n');
            out.push_str(&pv_text);
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    Some(out)
}

fn cache_key(vault: &Path, project: &str, project_vault: Option<&Path>) -> String {
    let pv = project_vault.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let payload = format!("{}|{}|{}", vault.display(), project, pv);
    let mut hasher = Sha1::new();
    hasher.update(payload.as_bytes());
    let bytes = hasher.finalize();
    let mut s = String::with_capacity(40);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// True if any `*.md` file under `root` (skipping SKIP_DIRS) is newer than
/// `threshold`. Short-circuits at the first newer file like `find -quit`.
fn any_md_newer(root: &Path, threshold: SystemTime) -> bool {
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.file_type().is_dir() && e.depth() > 0 {
                let name = e.file_name().to_string_lossy();
                !SKIP_DIRS.iter().any(|s| *s == name)
            } else {
                true
            }
        });
    for entry in walker.flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let p = entry.path();
        if p.extension().map(|e| e == "md").unwrap_or(false) {
            if let Ok(meta) = entry.metadata() {
                if let Ok(mtime) = meta.modified() {
                    if mtime > threshold {
                        return true;
                    }
                }
            }
        }
    }
    false
}
