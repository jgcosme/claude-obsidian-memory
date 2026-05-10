//! Type-vocabulary loader — single source of truth for memory types.
//!
//! Runtime resolution order:
//!   1. `~/.config/obsidian-memory/types.yaml` (user-owned, seeded by `setup`)
//!   2. Embedded `examples/types.yaml.example` (fallback for first-run, before
//!      setup has copied the file)
//!
//! The file format is the authoritative schema for:
//!   - validation     (replaces `VALID_TYPES` in `frontmatter.rs`)
//!   - overview order (replaces `TYPE_ORDER` in `overview.rs`)
//!   - personal-vault routing (`save-memory` Layer A: `tool` → `Tools/`, etc.)
//!   - project-vault routing (`type_folder_patterns` in `project_docs.rs`)
//!
//! The file is parsed once and cached. Parse failures panic with a clear
//! message — silently falling back would reintroduce the staleness vector
//! that motivated the consolidation in the first place.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

const EMBEDDED_DEFAULT: &str = include_str!("../../examples/types.yaml.example");

#[derive(Debug, Clone, Deserialize)]
struct Raw {
    #[allow(dead_code)]
    schema_version: Option<u32>,
    types: serde_yaml::Mapping,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RawType {
    description: Option<String>,
    personal_folder: Option<String>,
    #[serde(default)]
    project_folders: Vec<String>,
    #[serde(default)]
    system_managed: bool,
}

/// One memory type as seen by the rest of the binary.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TypeDef {
    pub name: String,
    pub description: String,
    pub personal_folder: String,
    pub project_folders: Vec<String>,
    pub system_managed: bool,
}

static CACHE: OnceLock<Vec<TypeDef>> = OnceLock::new();

fn user_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".config/obsidian-memory/types.yaml"))
}

fn load() -> Vec<TypeDef> {
    let (source, text): (String, String) = match user_config_path() {
        Some(p) if p.is_file() => match std::fs::read_to_string(&p) {
            Ok(t) => (p.display().to_string(), t),
            Err(e) => panic!(
                "obsidian-memory: failed to read types config at {}: {}",
                p.display(),
                e
            ),
        },
        _ => ("<embedded examples/types.yaml.example>".to_string(), EMBEDDED_DEFAULT.to_string()),
    };

    let raw: Raw = serde_yaml::from_str(&text).unwrap_or_else(|e| {
        panic!("obsidian-memory: failed to parse types config from {source}: {e}")
    });

    let mut out: Vec<TypeDef> = Vec::with_capacity(raw.types.len());
    let mut seen = std::collections::HashSet::<String>::new();
    for (k, v) in raw.types.iter() {
        let name = k.as_str().unwrap_or_else(|| {
            panic!("obsidian-memory: types.yaml from {source}: type names must be strings")
        });
        if !seen.insert(name.to_string()) {
            panic!("obsidian-memory: types.yaml from {source}: duplicate type `{name}`");
        }
        let rt: RawType = serde_yaml::from_value(v.clone()).unwrap_or_else(|e| {
            panic!("obsidian-memory: types.yaml from {source}: type `{name}` malformed: {e}")
        });
        let description = rt.description.unwrap_or_else(|| {
            panic!("obsidian-memory: types.yaml from {source}: type `{name}` missing `description`")
        });
        let personal_folder = rt.personal_folder.unwrap_or_else(|| {
            panic!(
                "obsidian-memory: types.yaml from {source}: type `{name}` missing `personal_folder`"
            )
        });
        out.push(TypeDef {
            name: name.to_string(),
            description,
            personal_folder,
            project_folders: rt.project_folders,
            system_managed: rt.system_managed,
        });
    }
    if out.is_empty() {
        panic!("obsidian-memory: types.yaml from {source}: empty `types` map");
    }
    out
}

/// All declared types, in the order they appear in the source file.
pub fn all() -> &'static [TypeDef] {
    CACHE.get_or_init(load).as_slice()
}

/// Look up a single type by name.
pub fn get(name: &str) -> Option<&'static TypeDef> {
    all().iter().find(|t| t.name == name)
}

/// Type names in declared order. Replaces the old `VALID_TYPES` constant.
pub fn names() -> Vec<&'static str> {
    all().iter().map(|t| t.name.as_str()).collect()
}

/// Returns true if `name` is a known type.
pub fn is_valid(name: &str) -> bool {
    all().iter().any(|t| t.name == name)
}

/// Personal-vault subfolder for `name`, or `None` if the type is unknown.
#[allow(dead_code)]
pub fn personal_folder(name: &str) -> Option<&'static str> {
    get(name).map(|t| t.personal_folder.as_str())
}

/// Candidate project-vault folder names for `name`. Empty slice for unknown
/// types or types that don't route to a project-vault folder.
pub fn project_folders(name: &str) -> &'static [String] {
    match get(name) {
        Some(t) => t.project_folders.as_slice(),
        None => &[],
    }
}

// ---------------------------------------------------------------------------
// Write-back helpers
// ---------------------------------------------------------------------------

const FILE_HEADER: &str = "\
# Memory type definitions — read by obsidian-memory at runtime.
#
# This file is the authoritative source of truth for the type vocabulary.
# Edit it directly, or use `obsidian-memory types {add,remove,edit,reset}`
# (or the `/obsidian-memory:types` slash command, which drives the same CLI).
#
# Schema reference: examples/types.yaml.example in the plugin source.

";

fn ensure_user_file_seeded() -> anyhow::Result<PathBuf> {
    let path = user_config_path()
        .ok_or_else(|| anyhow::anyhow!("unable to resolve $HOME for ~/.config/obsidian-memory"))?;
    if !path.is_file() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, EMBEDDED_DEFAULT)?;
    }
    Ok(path)
}

fn read_user_types(path: &Path) -> anyhow::Result<Vec<TypeDef>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?;
    let raw: Raw = serde_yaml::from_str(&text)
        .map_err(|e| anyhow::anyhow!("parse {}: {e}", path.display()))?;
    let mut out: Vec<TypeDef> = Vec::with_capacity(raw.types.len());
    let mut seen = std::collections::HashSet::<String>::new();
    for (k, v) in raw.types.iter() {
        let name = k
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("type names must be strings"))?
            .to_string();
        if !seen.insert(name.clone()) {
            anyhow::bail!("duplicate type `{name}`");
        }
        let rt: RawType = serde_yaml::from_value(v.clone())
            .map_err(|e| anyhow::anyhow!("type `{name}` malformed: {e}"))?;
        out.push(TypeDef {
            name,
            description: rt.description.unwrap_or_default(),
            personal_folder: rt.personal_folder.unwrap_or_default(),
            project_folders: rt.project_folders,
            system_managed: rt.system_managed,
        });
    }
    Ok(out)
}

fn write_user_types(path: &Path, types: &[TypeDef]) -> anyhow::Result<()> {
    let mut body = String::from(FILE_HEADER);
    body.push_str("schema_version: 1\n\ntypes:\n");
    for t in types {
        body.push_str(&format!("  {}:\n", t.name));
        body.push_str(&format!("    description: {}\n", yaml_quote(&t.description)));
        body.push_str(&format!("    personal_folder: {}\n", yaml_quote(&t.personal_folder)));
        if t.project_folders.is_empty() {
            body.push_str("    project_folders: []\n");
        } else {
            let inner = t
                .project_folders
                .iter()
                .map(|s| yaml_quote(s))
                .collect::<Vec<_>>()
                .join(", ");
            body.push_str(&format!("    project_folders: [{inner}]\n"));
        }
        if t.system_managed {
            body.push_str("    system_managed: true\n");
        }
    }

    let tmp = path.with_extension("yaml.tmp");
    std::fs::write(&tmp, &body)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Quote a YAML scalar safely. Use double quotes and escape `"` and `\`.
fn yaml_quote(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn count_notes_with_type(name: &str) -> usize {
    let vault = crate::vault::walk::resolve_vault(None);
    if !vault.is_dir() {
        return 0;
    }
    match crate::vault::search::search(
        &vault,
        crate::vault::search::SearchOpts {
            type_: Some(name),
            path_prefix: None,
            keywords: None,
            created_after: None,
            created_before: None,
            updated_after: None,
            updated_before: None,
            limit: usize::MAX,
            project_vault: None,
        },
    ) {
        Ok(hits) => hits.len(),
        Err(_) => 0,
    }
}

// ---------------------------------------------------------------------------
// CLI: `obsidian-memory types {list,validate,path,add,remove,edit,reset}`
// ---------------------------------------------------------------------------

pub fn cli_run(args: crate::cli::TypesArgs) -> anyhow::Result<i32> {
    use crate::cli::TypesCmd;
    match args.command {
        TypesCmd::Path => {
            match user_config_path() {
                Some(p) => println!("{}", p.display()),
                None => println!("(unable to resolve $HOME)"),
            }
            Ok(0)
        }
        TypesCmd::Validate => {
            let path = user_config_path();
            let exists = path.as_ref().map(|p| p.is_file()).unwrap_or(false);
            let source = if exists {
                path.as_ref().map(|p| p.display().to_string()).unwrap_or_default()
            } else {
                "<embedded examples/types.yaml.example>".to_string()
            };
            // `all()` panics on parse error; recover into a Result for nicer reporting.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(all));
            match result {
                Ok(types) => {
                    println!("ok: parsed {} types from {source}", types.len());
                    Ok(0)
                }
                Err(payload) => {
                    let msg = payload
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_else(|| "panic".to_string());
                    eprintln!("error: {msg}");
                    Ok(1)
                }
            }
        }
        TypesCmd::Add { name, description, personal_folder, project_folders, system_managed } => {
            let path = ensure_user_file_seeded()?;
            let mut types = read_user_types(&path)?;
            if types.iter().any(|t| t.name == name) {
                eprintln!("error: type `{name}` already exists. Use `types edit` to modify it.");
                return Ok(1);
            }
            // Drop empty entries that arise from `--project-folders ""` (clap
            // splits on `,` and yields a single empty string for the empty case).
            let project_folders: Vec<String> = project_folders.into_iter().filter(|s| !s.is_empty()).collect();
            types.push(TypeDef {
                name: name.clone(),
                description,
                personal_folder,
                project_folders,
                system_managed,
            });
            write_user_types(&path, &types)?;
            println!("added type `{name}` to {}", path.display());
            Ok(0)
        }
        TypesCmd::Remove { name, force } => {
            let path = ensure_user_file_seeded()?;
            let types = read_user_types(&path)?;
            let Some(pos) = types.iter().position(|t| t.name == name) else {
                eprintln!("error: type `{name}` not found");
                return Ok(1);
            };
            if types[pos].system_managed && !force {
                eprintln!(
                    "error: `{name}` is system-managed; pass --force to remove anyway (will break SessionEnd)"
                );
                return Ok(1);
            }
            let in_use = count_notes_with_type(&name);
            if in_use > 0 && !force {
                eprintln!(
                    "error: {in_use} vault note(s) currently use type `{name}`. Audit will flag them after removal. Pass --force to proceed."
                );
                return Ok(1);
            }
            let mut types = types;
            types.remove(pos);
            write_user_types(&path, &types)?;
            println!("removed type `{name}` from {}", path.display());
            if in_use > 0 {
                println!("note: {in_use} existing note(s) still typed `{name}` will be flagged by audit.");
            }
            Ok(0)
        }
        TypesCmd::Edit { name, description, personal_folder, project_folders } => {
            let path = ensure_user_file_seeded()?;
            let mut types = read_user_types(&path)?;
            let Some(t) = types.iter_mut().find(|t| t.name == name) else {
                eprintln!("error: type `{name}` not found");
                return Ok(1);
            };
            let mut changed: Vec<&str> = Vec::new();
            if let Some(d) = description { t.description = d; changed.push("description"); }
            if let Some(f) = personal_folder { t.personal_folder = f; changed.push("personal_folder"); }
            if let Some(pf) = project_folders {
                t.project_folders = pf.into_iter().filter(|s| !s.is_empty()).collect();
                changed.push("project_folders");
            }
            if changed.is_empty() {
                eprintln!("error: nothing to edit. Pass at least one of --description / --personal-folder / --project-folders.");
                return Ok(1);
            }
            write_user_types(&path, &types)?;
            println!("edited type `{name}`: updated {}", changed.join(", "));
            Ok(0)
        }
        TypesCmd::Reset { yes } => {
            if !yes {
                eprintln!("error: refusing to overwrite without --yes (this discards your customizations)");
                return Ok(1);
            }
            let path = user_config_path()
                .ok_or_else(|| anyhow::anyhow!("unable to resolve $HOME"))?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, EMBEDDED_DEFAULT)?;
            println!("reset {} to embedded defaults", path.display());
            Ok(0)
        }
        TypesCmd::List { json } => {
            let types = all();
            let user_file_present = user_config_path().map(|p| p.is_file()).unwrap_or(false);
            let source = if user_file_present { "user" } else { "embedded" };
            if json {
                let arr: Vec<serde_json::Value> = types
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "name": t.name,
                            "description": t.description,
                            "personal_folder": t.personal_folder,
                            "project_folders": t.project_folders,
                            "system_managed": t.system_managed,
                        })
                    })
                    .collect();
                let payload = serde_json::json!({"source": source, "types": arr});
                println!("{}", crate::jsonfmt::to_string_pretty_ascii(&payload)?);
            } else {
                println!("source: {source}");
                println!();
                for t in types {
                    let sm = if t.system_managed { " [system-managed]" } else { "" };
                    let pf = if t.project_folders.is_empty() {
                        "(none)".to_string()
                    } else {
                        t.project_folders.join(", ")
                    };
                    println!("{}{sm}", t.name);
                    println!("  description:     {}", t.description);
                    println!("  personal_folder: {}", t.personal_folder);
                    println!("  project_folders: {pf}");
                    println!();
                }
            }
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_default_parses() {
        // Bypass the cache by parsing the embedded text directly.
        let raw: Raw = serde_yaml::from_str(EMBEDDED_DEFAULT).expect("embedded YAML must parse");
        assert!(!raw.types.is_empty(), "embedded types map must be non-empty");
    }

    #[test]
    fn embedded_has_journal_system_managed() {
        let raw: Raw = serde_yaml::from_str(EMBEDDED_DEFAULT).expect("embedded YAML must parse");
        let journal = raw.types.get("journal").expect("embedded must define `journal`");
        let rt: RawType = serde_yaml::from_value(journal.clone()).unwrap();
        assert!(rt.system_managed, "embedded `journal` must be system_managed");
    }

    #[test]
    fn embedded_covers_legacy_seven() {
        let raw: Raw = serde_yaml::from_str(EMBEDDED_DEFAULT).unwrap();
        for name in ["preference", "reference", "findings", "decision", "learning", "tool", "journal"] {
            assert!(raw.types.get(name).is_some(), "embedded must define `{name}`");
        }
    }
}
