//! Vault library + CLI dispatch — port of scripts/_vault.py.

pub mod changes;
pub mod frontmatter;
pub mod overview;
pub mod search;
pub mod walk;
pub mod wikilinks;

use std::path::Path;

use anyhow::Result;

use crate::cli::{VaultArgs, VaultCmd};

pub fn run(args: VaultArgs) -> Result<i32> {
    let vault = walk::resolve_vault(args.vault.as_deref());
    match args.command {
        VaultCmd::Search(s) => {
            if !vault_present(&vault) {
                return Ok(1);
            }
            // Resolve project-vault Python-style: expand ~, resolve symlinks of
            // existing ancestors, then check is_dir. Python doesn't fail on
            // resolution — it fails on the is_dir check that follows.
            let project_vault = match s.project_vault.as_ref() {
                Some(p) => {
                    let resolved = walk::absolute(&walk::expand_user(p));
                    if !resolved.is_dir() {
                        eprintln!("project-vault not found at: {}", resolved.display());
                        return Ok(1);
                    }
                    Some(resolved)
                }
                None => None,
            };
            let hits = search::search(
                &vault,
                search::SearchOpts {
                    type_: s.r#type.as_deref(),
                    path_prefix: s.path_prefix.as_deref(),
                    keywords: s.keywords.as_deref(),
                    created_after: s.created_after.as_deref(),
                    created_before: s.created_before.as_deref(),
                    limit: s.limit,
                    project_vault: project_vault.as_deref(),
                },
            )?;
            if s.json {
                println!("{}", crate::jsonfmt::to_string_pretty_ascii(&hits)?);
            } else if hits.is_empty() {
                println!("(no matches)");
            } else {
                let multi_corpus = project_vault.is_some();
                for h in &hits {
                    let tag = if multi_corpus { format!("[{}] ", h.corpus) } else { String::new() };
                    let desc = if h.description.is_empty() { String::new() } else { format!(" — {}", h.description) };
                    println!("{tag}{}{desc}", h.path);
                }
            }
            Ok(0)
        }
        VaultCmd::Overview(o) => {
            if !vault_present(&vault) {
                return Ok(1);
            }
            let out = overview::overview(&vault, o.project.as_deref(), &o.mode)?;
            println!("{out}");
            if let Some(pv) = o.project_vault.as_ref() {
                let resolved = walk::absolute(&walk::expand_user(pv));
                if resolved.is_dir() {
                    println!();
                    let out = overview::overview_project(&resolved, o.project.as_deref())?;
                    println!("{out}");
                } else {
                    // Python: print(f"\n_(project-vault not found at: {p})_", file=sys.stderr)
                    // The leading "\n" is part of the printed string, so the
                    // stderr starts with a blank line and ends with `print()`'s
                    // trailing newline.
                    eprintln!();
                    eprintln!("_(project-vault not found at: {})_", resolved.display());
                }
            }
            Ok(0)
        }
        VaultCmd::VaultChanges(c) => {
            if !vault_present(&vault) {
                return Ok(1);
            }
            let result = changes::vault_md_changes(&vault, c.base_sha.as_deref());
            println!("{}", crate::jsonfmt::to_string_pretty_ascii(&result)?);
            Ok(0)
        }
        VaultCmd::IncomingWikilinks(w) => {
            if !vault_present(&vault) {
                return Ok(1);
            }
            let hits = wikilinks::incoming_wikilinks(&vault, &w.target);
            println!("{}", crate::jsonfmt::to_string_pretty_ascii(&hits)?);
            Ok(0)
        }
    }
}

/// Print Python's exact "vault not found" stderr line and return false. The
/// caller turns that into exit 1. Keeping the format here means the four
/// VaultCmd arms all pay the same fee.
fn vault_present(vault: &Path) -> bool {
    if vault.is_dir() {
        true
    } else {
        eprintln!("vault not found at: {}", vault.display());
        false
    }
}

