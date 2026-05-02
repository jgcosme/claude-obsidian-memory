//! Project-vault registry — port of scripts/_projects.py.
//!
//! Manages `~/.config/obsidian-memory/projects.json`. Stdout text formats are
//! preserved verbatim because hooks and commands parse them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::cli::{ProjectsArgs, ProjectsCmd};
use crate::vault::walk::{absolute, expand_user};

static PROJECT_NAME_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^[A-Za-z0-9_][A-Za-z0-9._\-]{0,63}$").expect("project name regex")
});
const PROJECT_NAME_RE_TEXT: &str = r"^[A-Za-z0-9_][A-Za-z0-9._-]{0,63}$";

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
struct Registry {
    #[serde(default)]
    projects: BTreeMap<String, Entry>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Entry {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    project: String,
}

pub fn projects_path() -> PathBuf {
    if let Ok(v) = std::env::var("OBSIDIAN_MEMORY_PROJECTS_FILE") {
        if !v.is_empty() {
            return absolute(&expand_user(Path::new(&v)));
        }
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".config/obsidian-memory/projects.json");
    }
    PathBuf::from(".config/obsidian-memory/projects.json")
}

fn load(path: &Path) -> Registry {
    let Ok(text) = std::fs::read_to_string(path) else { return Registry::default(); };
    serde_json::from_str::<Registry>(&text).unwrap_or_default()
}

fn save(path: &Path, data: &Registry) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut json = crate::jsonfmt::to_string_pretty_ascii(data)?;
    json.push('\n');
    std::fs::write(&tmp, json).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

fn resolve_repo(path: &str) -> String {
    let expanded = expand_user(Path::new(path));
    absolute(&expanded).to_string_lossy().into_owned()
}

#[derive(Debug, Serialize)]
pub struct LookupResult {
    pub status: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

/// Public alias used by the SessionStart hook.
pub fn lookup_status(path: &str) -> LookupResult {
    lookup(path)
}

fn lookup(path: &str) -> LookupResult {
    let repo = resolve_repo(path);
    let data = load(&projects_path());
    match data.projects.get(&repo) {
        None => LookupResult {
            status: "not_registered".into(),
            path: repo,
            enabled: None,
            project: None,
        },
        Some(entry) => {
            let project_name = if entry.project.is_empty() {
                Path::new(&repo).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
            } else {
                entry.project.clone()
            };
            LookupResult {
                status: if entry.enabled { "enabled".into() } else { "disabled".into() },
                path: repo,
                enabled: Some(entry.enabled),
                project: Some(project_name),
            }
        }
    }
}

#[derive(Debug, Serialize)]
struct RegisterResult {
    status: String,
    path: String,
    enabled: bool,
    project: String,
}

fn register(path: &str, enabled: bool, project: &str) -> Result<RegisterResult> {
    if !PROJECT_NAME_RE.is_match(project) {
        // Match Python's `{project!r}` — repr() uses single quotes.
        bail!(
            "invalid project name '{project}': must match {PROJECT_NAME_RE_TEXT} (safe single path component, ≤64 chars, no separators, no leading dot/dash)"
        );
    }
    let repo = resolve_repo(path);
    let rp = projects_path();
    let mut data = load(&rp);
    data.projects.insert(repo.clone(), Entry { enabled, project: project.to_string() });
    save(&rp, &data)?;
    Ok(RegisterResult {
        status: if enabled { "enabled".into() } else { "disabled".into() },
        path: repo,
        enabled,
        project: project.to_string(),
    })
}

fn remove(path: &str) -> bool {
    let repo = resolve_repo(path);
    let rp = projects_path();
    let mut data = load(&rp);
    if !data.projects.contains_key(&repo) {
        return false;
    }
    data.projects.remove(&repo);
    let _ = save(&rp, &data);
    true
}

#[derive(Debug, Serialize)]
pub struct ListItem {
    pub path: String,
    pub enabled: bool,
    pub status: String,
    pub project: String,
}

/// Public alias used by status.
pub fn list_registered() -> Vec<ListItem> {
    list_all()
}

fn list_all() -> Vec<ListItem> {
    let data = load(&projects_path());
    data.projects
        .into_iter()
        .map(|(path, entry)| {
            let project_name = if entry.project.is_empty() {
                Path::new(&path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
            } else {
                entry.project
            };
            ListItem {
                path: path.clone(),
                enabled: entry.enabled,
                status: if entry.enabled { "enabled".into() } else { "disabled".into() },
                project: project_name,
            }
        })
        .collect()
}

pub fn run(args: ProjectsArgs) -> Result<i32> {
    match args.command {
        ProjectsCmd::Lookup { path, json } => {
            let result = lookup(&path);
            if json {
                println!("{}", crate::jsonfmt::to_string_pretty_ascii(&result)?);
            } else {
                println!("{}", result.status);
            }
            Ok(0)
        }
        ProjectsCmd::Register { path, enabled: _, no_enabled, project, json } => {
            // argparse default is enabled=True; --no-enabled flips to False.
            // clap's `overrides_with` ensures only the last-seen flag is set
            // when both appear, so we just check no_enabled.
            let final_enabled = !no_enabled;
            match register(&path, final_enabled, &project) {
                Ok(result) => {
                    if json {
                        println!("{}", crate::jsonfmt::to_string_pretty_ascii(&result)?);
                    } else {
                        println!("{}: {}", result.status, result.path);
                    }
                    Ok(0)
                }
                Err(e) => {
                    eprintln!("error: {e:#}");
                    Ok(2)
                }
            }
        }
        ProjectsCmd::Remove { path, json } => {
            let removed = remove(&path);
            let resolved = resolve_repo(&path);
            if json {
                // Python: json.dumps({"removed": ..., "path": ...}) — keys in
                // insertion order, default `", "` / `": "` separators. Format
                // manually so the byte stream matches.
                println!(
                    "{{\"removed\": {removed}, \"path\": {}}}",
                    crate::jsonfmt::escape_ascii(&serde_json::to_string(&resolved)?)
                );
            } else if removed {
                println!("removed: {resolved}");
            } else {
                println!("no entry for: {resolved}");
            }
            Ok(if removed { 0 } else { 1 })
        }
        ProjectsCmd::List { json } => {
            let items = list_all();
            if json {
                println!("{}", crate::jsonfmt::to_string_pretty_ascii(&items)?);
            } else if items.is_empty() {
                println!("(no projects registered)");
            } else {
                for item in &items {
                    let mark = if item.enabled { "✓" } else { "✗" };
                    println!("  {mark} [{}] {}", item.project, item.path);
                }
            }
            Ok(0)
        }
        ProjectsCmd::Path => {
            println!("{}", projects_path().display());
            Ok(0)
        }
    }
}
