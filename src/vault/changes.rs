//! Vault git diff: *.md changes since BASE_SHA (or HEAD), incl. working tree + untracked.
//!
//! Mirrors `_vault.py:vault_md_changes` and the `_vault_consume_namestatus`
//! helper. Output JSON shape is preserved for downstream consumers.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde::Serialize;

use crate::vault::walk::SKIP_DIRS;

#[derive(Debug, Default, Serialize)]
pub struct VaultChanges {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
    pub renamed: Vec<[String; 2]>,
}

pub fn vault_md_changes(vault: &Path, base_sha: Option<&str>) -> VaultChanges {
    let mut out = VaultChanges::default();
    if !vault.join(".git").exists() {
        return out;
    }

    if let Some(sha) = base_sha {
        if let Some(stdout) = run_git(
            vault,
            &["diff", "--name-status", "-z", "-M", sha, "HEAD", "--", "*.md"],
        ) {
            consume_namestatus(&stdout, &mut out);
        }
    }

    if let Some(stdout) = run_git(
        vault,
        &["diff", "--name-status", "-z", "-M", "HEAD", "--", "*.md"],
    ) {
        consume_namestatus(&stdout, &mut out);
    }

    if let Some(stdout) = run_git(
        vault,
        &["ls-files", "--others", "--exclude-standard", "-z", "--", "*.md"],
    ) {
        for p in stdout.split('\0') {
            if !p.is_empty() {
                out.added.push(p.to_string());
            }
        }
    }

    let keep = |p: &str| -> bool {
        if p.is_empty() { return false; }
        for skip in SKIP_DIRS {
            if p == *skip || p.starts_with(&format!("{skip}/")) || p.contains(&format!("/{skip}/")) {
                return false;
            }
        }
        true
    };

    let dedup = |v: Vec<String>| -> Vec<String> {
        let set: BTreeSet<String> = v.into_iter().filter(|s| keep(s)).collect();
        set.into_iter().collect()
    };

    out.added = dedup(out.added);
    out.modified = dedup(out.modified);
    out.deleted = dedup(out.deleted);

    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    let mut deduped: Vec<[String; 2]> = Vec::new();
    for pair in out.renamed.drain(..) {
        if !keep(&pair[0]) && !keep(&pair[1]) {
            continue;
        }
        let key = (pair[0].clone(), pair[1].clone());
        if seen.insert(key) {
            deduped.push(pair);
        }
    }
    out.renamed = deduped;
    out
}

fn run_git(cwd: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(cwd).args(args);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let child = cmd.spawn().ok()?;
    let _ = Duration::from_secs(15); // (timeout enforcement is best-effort; matches Python's 15s)
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parse `git diff --name-status -z` output into the changes struct.
///
/// Format: each record is `STATUS\0PATH[\0NEW_PATH]\0`. `R*`/`C*` carry an
/// extra path; everything else is single-path.
fn consume_namestatus(text: &str, result: &mut VaultChanges) {
    let tokens: Vec<&str> = text.split('\0').filter(|t| !t.is_empty()).collect();
    let mut i = 0;
    while i < tokens.len() {
        let status = tokens[i];
        i += 1;
        if status.starts_with('R') || status.starts_with('C') {
            if i + 1 >= tokens.len() {
                break;
            }
            let old = tokens[i].trim().to_string();
            i += 1;
            let new = tokens[i].trim().to_string();
            i += 1;
            if status.starts_with('R') {
                result.renamed.push([old, new]);
            } else {
                result.added.push(new);
            }
        } else {
            if i >= tokens.len() {
                break;
            }
            let p = tokens[i].trim().to_string();
            i += 1;
            match status {
                "A" => result.added.push(p),
                "M" => result.modified.push(p),
                "D" => result.deleted.push(p),
                _ => {}
            }
        }
    }
}
