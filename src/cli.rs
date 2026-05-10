use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "obsidian-memory", version, about = "Obsidian-backed persistent memory for Claude Code")]
pub struct Cli {
    #[command(subcommand)]
    pub command: TopLevel,
}

#[derive(Subcommand, Debug)]
pub enum TopLevel {
    /// Vault operations (search, overview, change detection, wikilink scan).
    Vault(VaultArgs),
    /// Project-vault registry (~/.config/obsidian-memory/projects.json).
    Projects(ProjectsArgs),
    /// Project repo .md enumeration + memory-type folder matching.
    ProjectDocs(ProjectDocsArgs),
    /// Slim a Claude Code session transcript (JSONL → human-readable text).
    SlimTranscript(SlimTranscriptArgs),
    /// Full vault integrity audit (frontmatter, wikilinks, orphans, duplicate basenames).
    Audit(AuditArgs),
    /// Initialize a project's docs as a project-vault corpus (adds frontmatter).
    InitProject(InitProjectArgs),
    /// Render the obsidian-memory status line (reads Claude Code session JSON on stdin).
    Statusline,
    /// Lifecycle hook entry points (invoked by Claude Code via hooks.json).
    Hook(HookArgs),
    /// Aggregate the current session's plugin token usage and print a summary.
    Usage,
    /// Print plugin diagnostic status (config, vault, scripts, recent activity).
    Status,
    /// Scaffold the vault, config, and Claude Code statusline integration.
    Setup,
    /// Inspect the memory-type vocabulary (~/.config/obsidian-memory/types.yaml).
    Types(TypesArgs),
}

// ---------------------------------------------------------------------------
// types
// ---------------------------------------------------------------------------
#[derive(Args, Debug)]
pub struct TypesArgs {
    #[command(subcommand)]
    pub command: TypesCmd,
}

#[derive(Subcommand, Debug)]
pub enum TypesCmd {
    /// Print the effective type set (with source: user file vs embedded default).
    List {
        /// Emit JSON instead of human-readable rows.
        #[arg(long)]
        json: bool,
    },
    /// Parse the user types file (or embedded default) and report errors.
    Validate,
    /// Print the path of the user types file (whether or not it exists).
    Path,
    /// Append a new type to the user file. Errors on duplicate name.
    Add {
        #[arg(long)]
        name: String,
        #[arg(long)]
        description: String,
        #[arg(long)]
        personal_folder: String,
        /// Comma-separated list of project-vault folder names to probe.
        #[arg(long, value_delimiter = ',', default_value = "")]
        project_folders: Vec<String>,
        /// Mark as system-managed (rare — only the plugin should normally write these).
        #[arg(long)]
        system_managed: bool,
    },
    /// Remove a type by name. Refuses if vault notes use it (override with --force).
    Remove {
        #[arg(long)]
        name: String,
        /// Remove even if existing vault notes use the type.
        #[arg(long)]
        force: bool,
    },
    /// Patch one or more fields of an existing type.
    Edit {
        #[arg(long)]
        name: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        personal_folder: Option<String>,
        /// Replace project-folders entirely. Pass an empty string to clear.
        #[arg(long, value_delimiter = ',')]
        project_folders: Option<Vec<String>>,
    },
    /// Restore the user file to the embedded default (destructive).
    Reset {
        /// Skip the confirmation prompt (always required for non-interactive use).
        #[arg(long)]
        yes: bool,
    },
}

// ---------------------------------------------------------------------------
// vault
// ---------------------------------------------------------------------------
#[derive(Args, Debug)]
pub struct VaultArgs {
    /// Vault path override (else $OBSIDIAN_VAULT_PATH, else config.env, else ~/Documents/Obsidian Memory).
    #[arg(long, global = true)]
    pub vault: Option<PathBuf>,
    #[command(subcommand)]
    pub command: VaultCmd,
}

#[derive(Subcommand, Debug)]
pub enum VaultCmd {
    /// Search the vault by frontmatter and keywords.
    Search(VaultSearchArgs),
    /// Emit a markdown vault overview.
    Overview(VaultOverviewArgs),
    /// JSON of *.md changes since BASE_SHA (incl. working tree + untracked).
    VaultChanges(VaultChangesArgs),
    /// Find notes linking to TARGET via [[wikilink]].
    IncomingWikilinks(VaultIncomingWikilinksArgs),
}

#[derive(Args, Debug)]
pub struct VaultSearchArgs {
    /// Filter by frontmatter `type:` (e.g., decision).
    #[arg(long)]
    pub r#type: Option<String>,
    /// Filter by relative path prefix (e.g., Notes, Tools, Journals).
    #[arg(long = "path-prefix")]
    pub path_prefix: Option<String>,
    /// Space-separated keywords; matched against path, frontmatter, body.
    #[arg(long)]
    pub keywords: Option<String>,
    /// ISO 8601 datetime (`2026-05-02T14:30:00+08:00`) or date (`2026-05-02`);
    /// only notes with frontmatter `created_at:` >= this. A bare date is
    /// interpreted as local-midnight. Legacy `created:` (date-only) still matches.
    #[arg(long = "created-after")]
    pub created_after: Option<String>,
    /// ISO 8601 datetime or date; only notes with `created_at:` <= this.
    #[arg(long = "created-before")]
    pub created_before: Option<String>,
    /// ISO 8601 datetime or date; only notes with frontmatter `updated_at:` >= this.
    #[arg(long = "updated-after")]
    pub updated_after: Option<String>,
    /// ISO 8601 datetime or date; only notes with `updated_at:` <= this.
    #[arg(long = "updated-before")]
    pub updated_before: Option<String>,
    /// Max results (default 50).
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
    /// Emit JSON instead of text.
    #[arg(long)]
    pub json: bool,
    /// Also search this project-vault corpus (path to project repo).
    #[arg(long = "project-vault")]
    pub project_vault: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct VaultOverviewArgs {
    /// Current project name; deep-lists its notes (others appear as a name list).
    #[arg(long)]
    pub project: Option<String>,
    /// Overview detail level.
    #[arg(long, default_value = "full", value_parser = ["full", "tools-and-general", "tools-only"])]
    pub mode: String,
    /// Also emit an overview block for this project-vault corpus.
    #[arg(long = "project-vault")]
    pub project_vault: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct VaultChangesArgs {
    /// Git SHA to diff against (default: HEAD).
    #[arg(long = "base-sha")]
    pub base_sha: Option<String>,
}

#[derive(Args, Debug)]
pub struct VaultIncomingWikilinksArgs {
    /// Vault-relative path of the target note (e.g., Notes/bar.md).
    #[arg(long, required = true)]
    pub target: String,
}

// ---------------------------------------------------------------------------
// projects
// ---------------------------------------------------------------------------
#[derive(Args, Debug)]
pub struct ProjectsArgs {
    #[command(subcommand)]
    pub command: ProjectsCmd,
}

#[derive(Subcommand, Debug)]
pub enum ProjectsCmd {
    /// Check registration status of a project path.
    Lookup {
        path: String,
        #[arg(long)]
        json: bool,
    },
    /// Add or update a project entry.
    Register {
        path: String,
        /// Mark this project as enabled (default).
        #[arg(long, overrides_with = "no_enabled")]
        enabled: bool,
        /// Mark this project as disabled (declined).
        #[arg(long = "no-enabled", overrides_with = "enabled")]
        no_enabled: bool,
        /// Project name (usually repo basename).
        #[arg(long, required = true)]
        project: String,
        #[arg(long)]
        json: bool,
    },
    /// Delete a project entry (registration prompt fires again next session).
    Remove {
        path: String,
        #[arg(long)]
        json: bool,
    },
    /// List all registered projects.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Print the resolved projects.json path.
    Path,
}

// ---------------------------------------------------------------------------
// project-docs
// ---------------------------------------------------------------------------
#[derive(Args, Debug)]
pub struct ProjectDocsArgs {
    #[command(subcommand)]
    pub command: ProjectDocsCmd,
}

#[derive(Subcommand, Debug)]
pub enum ProjectDocsCmd {
    /// List .md files in the repo's vault corpus.
    Enumerate {
        project_path: String,
        #[arg(long)]
        json: bool,
    },
    /// Find a repo folder matching a memory type.
    MatchTypeFolder {
        project_path: String,
        #[arg(long, required = true,
              value_parser = ["decision", "findings", "learning", "reference", "preference", "tool", "journal"])]
        r#type: String,
        #[arg(long)]
        json: bool,
    },
}

// ---------------------------------------------------------------------------
// slim-transcript
// ---------------------------------------------------------------------------
#[derive(Args, Debug)]
pub struct SlimTranscriptArgs {
    /// Path to a Claude Code transcript .jsonl.
    pub transcript: String,
    /// Output file (default: stdout).
    #[arg(short = 'o', long = "out")]
    pub out: Option<PathBuf>,
    /// Print byte-reduction stats to stderr.
    #[arg(long)]
    pub stats: bool,
}

// ---------------------------------------------------------------------------
// audit
// ---------------------------------------------------------------------------
#[derive(Args, Debug)]
pub struct AuditArgs {
    /// Path to Obsidian vault (overrides config).
    #[arg(long)]
    pub vault: Option<PathBuf>,
    /// Also audit this project-vault corpus (path to the project's git repo).
    #[arg(long = "project-vault")]
    pub project_vault: Option<PathBuf>,
    /// Emit JSON instead of markdown.
    #[arg(long)]
    pub json: bool,
    /// Migrate frontmatter on each note: rename legacy `created:` (date-only) to
    /// `created_at:` (ISO 8601 with local offset, sourced from git first-commit
    /// timestamp; falls back to file mtime). Adds `updated_at` + `updated_by: audit`
    /// when missing. Rewrites only the frontmatter block.
    #[arg(long = "fix-frontmatter")]
    pub fix_frontmatter: bool,
}

// ---------------------------------------------------------------------------
// init-project
// ---------------------------------------------------------------------------
#[derive(Args, Debug)]
pub struct InitProjectArgs {
    /// Path to the project's git repo.
    pub project_path: String,
    /// Project name (e.g., repo basename).
    #[arg(long, required = true)]
    pub project: String,
    /// Print plan without writing.
    #[arg(long = "dry-run")]
    pub dry_run: bool,
    /// Skip the LLM type-inference call; every candidate gets type=reference.
    #[arg(long = "no-llm")]
    pub no_llm: bool,
    /// Emit JSON result instead of text.
    #[arg(long)]
    pub json: bool,
}

// ---------------------------------------------------------------------------
// hook
// ---------------------------------------------------------------------------
#[derive(Args, Debug)]
pub struct HookArgs {
    #[command(subcommand)]
    pub command: HookCmd,
}

#[derive(Subcommand, Debug)]
pub enum HookCmd {
    /// SessionStart: emit vault overview / first-time-setup prompt.
    SessionStart,
    /// SessionEnd: background a memory review; auto-commit vault.
    SessionEnd,
    /// UserPromptSubmit: per-prompt vault retrieval gate.
    UserPromptSubmit,
    /// Internal: append a usage event (hidden — no doc).
    #[command(hide = true)]
    UsageLog(HookUsageLogArgs),
    /// Internal: detached SessionEnd worker (hidden).
    #[command(hide = true)]
    SessionEndBg {
        #[arg(long = "state-file")]
        state_file: PathBuf,
    },
}

#[derive(Args, Debug)]
pub struct HookUsageLogArgs {
    /// Event shape: "api" or "chars".
    pub mode: String,
    pub session_id: String,
    pub kind: String,
    /// For api: usage JSON object (single-line). For chars: byte count.
    pub field4: Option<String>,
    /// For api only: cost USD.
    pub field5: Option<String>,
    /// For api only: duration ms.
    pub field6: Option<String>,
}

impl Cli {
    pub fn run(self) -> Result<i32> {
        match self.command {
            TopLevel::Vault(args) => crate::vault::run(args),
            TopLevel::Projects(args) => crate::projects::run(args),
            TopLevel::ProjectDocs(args) => crate::project_docs::run(args),
            TopLevel::SlimTranscript(args) => crate::transcript::run(args),
            TopLevel::Audit(args) => crate::audit::run(args),
            TopLevel::InitProject(args) => crate::project_init::run(args),
            TopLevel::Statusline => crate::statusline::run(),
            TopLevel::Hook(args) => crate::hook::run(args),
            TopLevel::Usage => crate::usage::run(),
            TopLevel::Status => crate::status::run(),
            TopLevel::Setup => crate::setup::run(),
            TopLevel::Types(args) => crate::vault::types::cli_run(args),
        }
    }
}
