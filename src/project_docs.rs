//! Project-vault corpus enumeration — port of scripts/_project_docs.py.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use crate::cli::{ProjectDocsArgs, ProjectDocsCmd};
use crate::vault::walk::expand_user;

const BOILERPLATE_PREFIXES: &[&str] = &["LICENSE", "CHANGELOG", "CODE_OF_CONDUCT", "SECURITY"];

const SKIP_DOTFILE_DIRS: &[&str] = &[
    ".github",
    ".cursor",
    ".vscode",
    ".devcontainer",
    ".idea",
    ".claude",
];

/// `(decision/findings/learning/reference) → repo folder names to look for`.
fn type_folder_patterns(t: &str) -> &'static [&'static str] {
    match t {
        "decision" => &["decisions", "adr", "decision-records"],
        "findings" => &["findings", "research"],
        "learning" => &["learnings", "lessons"],
        "reference" => &["references"],
        _ => &[],
    }
}

fn git_ls(repo: &Path, args: &[&str]) -> Vec<String> {
    let mut cmd = Command::new("git");
    cmd.arg("ls-files");
    cmd.args(args);
    cmd.current_dir(repo);
    let Ok(output) = cmd.output() else { return Vec::new(); };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect()
}

fn is_boilerplate(rel_path: &Path) -> bool {
    let parts: Vec<&str> = rel_path.iter().filter_map(|s| s.to_str()).collect();
    if let Some(first) = parts.first() {
        if SKIP_DOTFILE_DIRS.contains(first) {
            return true;
        }
    }
    if parts.contains(&"fixtures") {
        return true;
    }
    if parts.contains(&".claude-plugin") {
        return true;
    }
    if parts.contains(&"skills") && rel_path.file_name().map(|f| f == "SKILL.md").unwrap_or(false) {
        return true;
    }
    if parts.contains(&"commands")
        && rel_path.extension().map(|e| e.eq_ignore_ascii_case("md")).unwrap_or(false)
        && parts.iter().any(|p| matches!(*p, "plugins" | ".claude-plugin" | ".claude"))
    {
        return true;
    }
    let name_upper = rel_path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.to_ascii_uppercase())
        .unwrap_or_default();
    for prefix in BOILERPLATE_PREFIXES {
        if name_upper.starts_with(prefix) {
            return true;
        }
    }
    false
}

pub fn enumerate_project_docs(project_path: &Path) -> Vec<PathBuf> {
    let repo = match std::fs::canonicalize(expand_user(project_path)) {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    if !repo.is_dir() || !repo.join(".git").exists() {
        return Vec::new();
    }
    let mut tracked = git_ls(&repo, &[]);
    tracked.extend(git_ls(&repo, &["--others", "--exclude-standard"]));
    tracked.sort();
    tracked.dedup();

    let mut out: Vec<PathBuf> = Vec::new();
    for rel in tracked {
        let rel_path = PathBuf::from(&rel);
        let ext_md = rel_path.extension()
            .map(|e| e.eq_ignore_ascii_case("md"))
            .unwrap_or(false);
        if !ext_md { continue; }
        if is_boilerplate(&rel_path) { continue; }
        out.push(repo.join(rel_path));
    }
    out
}

fn match_type_folder(project_path: &Path, type_: &str) -> Option<PathBuf> {
    let patterns = type_folder_patterns(type_);
    if patterns.is_empty() { return None; }
    let repo = std::fs::canonicalize(expand_user(project_path)).ok()?;
    if !repo.is_dir() { return None; }
    for tier_root in [repo.clone(), repo.join("docs")] {
        if !tier_root.is_dir() { continue; }
        let Ok(entries) = std::fs::read_dir(&tier_root) else { continue; };
        let mut entries: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for entry in entries {
            if entry.is_dir() {
                if let Some(name) = entry.file_name().and_then(|n| n.to_str()) {
                    let name_lower = name.to_ascii_lowercase();
                    if patterns.contains(&name_lower.as_str()) {
                        return Some(entry);
                    }
                }
            }
        }
    }
    None
}

pub fn run(args: ProjectDocsArgs) -> Result<i32> {
    match args.command {
        ProjectDocsCmd::Enumerate { project_path, json } => {
            let repo = std::fs::canonicalize(expand_user(Path::new(&project_path)))
                .unwrap_or_else(|_| PathBuf::from(&project_path));
            if !repo.join(".git").exists() {
                eprintln!("not a git repo: {}", repo.display());
                return Ok(1);
            }
            let paths = enumerate_project_docs(&repo);
            let rels: Vec<String> = paths
                .iter()
                .map(|p| p.strip_prefix(&repo).unwrap_or(p).to_string_lossy().into_owned())
                .collect();
            if json {
                println!("{}", crate::jsonfmt::to_string_pretty_ascii(&rels)?);
            } else {
                for r in &rels {
                    println!("{r}");
                }
            }
            Ok(0)
        }
        ProjectDocsCmd::MatchTypeFolder { project_path, r#type, json } => {
            let repo = std::fs::canonicalize(expand_user(Path::new(&project_path)))
                .unwrap_or_else(|_| PathBuf::from(&project_path));
            match match_type_folder(&repo, &r#type) {
                None => {
                    if json {
                        // Match Python's json.dumps default separators (", " / ": ").
                        let t_json = crate::jsonfmt::escape_ascii(&serde_json::to_string(&r#type)?);
                        println!("{{\"matched\": false, \"type\": {t_json}}}");
                    }
                    Ok(1)
                }
                Some(folder) => {
                    let rel = folder.strip_prefix(&repo).unwrap_or(&folder).to_string_lossy().into_owned();
                    if json {
                        let t_json = crate::jsonfmt::escape_ascii(&serde_json::to_string(&r#type)?);
                        let p_json = crate::jsonfmt::escape_ascii(&serde_json::to_string(&rel)?);
                        println!("{{\"matched\": true, \"type\": {t_json}, \"path\": {p_json}}}");
                    } else {
                        println!("{rel}");
                    }
                    Ok(0)
                }
            }
        }
    }
}

