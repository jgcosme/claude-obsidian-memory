#!/bin/bash
# SessionStart hook: load Obsidian-backed memory into context.
# Stdout becomes context injected at the start of every Claude session.

set -u

# ---------------------------------------------------------------------------
# Config: load from ~/.config/claude-memory/config.env if present, else use defaults.
# ---------------------------------------------------------------------------
CONFIG_FILE="${HOME}/.config/claude-memory/config.env"
if [ -f "$CONFIG_FILE" ]; then
  # shellcheck disable=SC1090
  . "$CONFIG_FILE" 2>/dev/null || true
fi

OBSIDIAN_VAULT_PATH="${OBSIDIAN_VAULT_PATH:-$HOME/Documents/Obsidian Vault}"
PLUGIN_ROOT="${CLAUDE_PLUGIN_ROOT:-}"

# Auto-detect Obsidian CLI (optional — only used for reactive queries)
if [ -z "${OBSIDIAN_CLI:-}" ]; then
  if [ -x "/Applications/Obsidian.app/Contents/MacOS/obsidian" ]; then
    OBSIDIAN_CLI="/Applications/Obsidian.app/Contents/MacOS/obsidian"
  elif command -v obsidian >/dev/null 2>&1; then
    OBSIDIAN_CLI="$(command -v obsidian)"
  fi
fi

# ---------------------------------------------------------------------------
# Defensive: if vault doesn't exist, emit a helpful note and exit cleanly.
# ---------------------------------------------------------------------------
if [ ! -d "$OBSIDIAN_VAULT_PATH" ]; then
  cat <<EOF
=== OBSIDIAN MEMORY (not yet set up) ===
The plugin is installed but no vault exists at: $OBSIDIAN_VAULT_PATH
Run the setup script to scaffold a vault:
  bash \${CLAUDE_PLUGIN_ROOT}/scripts/setup.sh
Or set OBSIDIAN_VAULT_PATH in ~/.config/claude-memory/config.env to point at an existing vault.
EOF
  exit 0
fi

# ---------------------------------------------------------------------------
# 1. Open Obsidian.app if available (purely for the optional `obsidian` CLI).
# ---------------------------------------------------------------------------
if command -v open >/dev/null 2>&1; then
  open -ga Obsidian 2>/dev/null
fi
if [ -n "${OBSIDIAN_CLI:-}" ] && [ -x "$OBSIDIAN_CLI" ]; then
  for _ in 1 2 3 4 5; do
    if "$OBSIDIAN_CLI" files >/dev/null 2>&1; then
      break
    fi
    sleep 1
  done
fi

# 2. Derive current project name from cwd basename.
PROJECT_DIR="${CLAUDE_PROJECT_DIR:-$PWD}"
PROJECT_NAME=$(basename "$PROJECT_DIR")
TODAY=$(date +%Y-%m-%d)
PROJECT_VAULT_DIR="$OBSIDIAN_VAULT_PATH/Projects/$PROJECT_NAME"
TEMPLATE_DIR="$PLUGIN_ROOT/templates/Projects/PROJECT_NAME"

# 3. Emit memory bootstrap as injected context.
cat <<INSTRUCTIONS
=== OBSIDIAN MEMORY ===

Vault: $OBSIDIAN_VAULT_PATH
Index: the auto-overview below — regenerated each session from frontmatter (no INDEX files).

RECALL — read by path or query:
  Read tool                     "$OBSIDIAN_VAULT_PATH/<path>"
  Python search CLI             python3 "$PLUGIN_ROOT/scripts/_vault.py" search --type <t> --keywords <k> --path-prefix <p> [--created-after YYYY-MM-DD]
  Obsidian search (if running)  obsidian search query="[type:decision] keywords"

REMEMBER — write a note only when ALL hold:
  - Significant: correction, decision, validated approach, novel fact, "remember this"
  - Not already covered (search the vault first)
  - Useful in future sessions

ROUTE before writing. Two questions:
  1. Team-relevant (architecture, gotchas, runbooks, conventions) or personal/cross-project?
  2. Does the project maintain internal docs (docs/, ADRs, mkdocs/sphinx, CONTRIBUTING)?

  team-relevant + has docs   → propose a repo doc edit; on user approval, apply it and add a thin-pointer vault note in Projects/<name>/{Decisions,Learnings}/
  team-relevant + no docs    → substantive vault note in Projects/<name>/{Decisions,Learnings}/
  personal / cross-project   → substantive vault note in General/ or Projects/<name>/

Frontmatter required (except README): type, description, created (+ \`project\` for project-scoped).
UPDATE wrong/outdated notes. Verify paths and function names before recommending — they drift.
Do not write to ~/.claude/projects/*/memory/ (disabled in favor of this vault).

INSTRUCTIONS

# 4. README at vault root (if present) gives prose orientation.
if [ -f "$OBSIDIAN_VAULT_PATH/README.md" ]; then
  echo "=== VAULT README ==="
  cat "$OBSIDIAN_VAULT_PATH/README.md"
  echo ""
fi

# 5. Auto-generated vault overview (the load-bearing piece).
if [ -n "$PLUGIN_ROOT" ] && [ -f "$PLUGIN_ROOT/scripts/_vault.py" ]; then
  echo "=== VAULT OVERVIEW (auto-generated from frontmatter) ==="
  python3 "$PLUGIN_ROOT/scripts/_vault.py" --vault "$OBSIDIAN_VAULT_PATH" overview --project "$PROJECT_NAME" 2>/dev/null || \
    echo "(overview generation failed — check that python3 ≥ 3.9 is available)"
  echo ""
fi

# 6. Project scaffolding prompt (only when the project has no vault folder yet).
if [ ! -d "$PROJECT_VAULT_DIR" ]; then
  cat <<EOF
=== PROJECT: $PROJECT_NAME (not yet scaffolded) ===

cwd: $PROJECT_DIR. No Projects/$PROJECT_NAME/ folder in the vault yet.

Ask once: "Create memory scaffolding for project '$PROJECT_NAME'? (y/n)"
  NO  → no project folder, no project-scoped notes this session. General/ and Tools/ writes are fine.
  YES → scaffold + prefill from evidence in $PROJECT_DIR.

1. Folders + base overview:
     mkdir -p "$PROJECT_VAULT_DIR"/{Decisions,Learnings,Research,References,Journal}
     sed -e 's|__PROJECT_NAME__|$PROJECT_NAME|g' -e 's|__TODAY__|$TODAY|g' \\
       "$TEMPLATE_DIR/overview.md" > "$PROJECT_VAULT_DIR/overview.md"

2. Inspect $PROJECT_DIR. Read top-level docs (README, ARCHITECTURE, CONTRIBUTING, CHANGELOG), package manifests, ADR folders, runbooks, design docs, RFCs, /docs, build/CI config. Skip source and vendored deps.
   Repo metadata: \`git -C "$PROJECT_DIR" remote get-url origin\`, \`git -C "$PROJECT_DIR" branch --show-current\`.

3. Populate overview.md. Keep section headings (## What it is, ## Goals, ## Current branch / focus, ## Stakeholders, ## Notes). Cite sources inline. Leave sections empty when no evidence — do not invent.

4. Seed subfolders with thin pointers (1-3 sentence summary + relative source path). Skip README.md, auto-generated, vendored, and license files.
     References/  entry-point pointers (architecture, API specs, getting-started, contributing)
     Decisions/   ADRs and design choices with rationale
     Learnings/   runbooks, troubleshooting, postmortems
     Research/    design docs, RFCs, options comparisons

   Frontmatter:
     ---
     type: reference | decision | learning | research
     description: <one-line>
     project: $PROJECT_NAME
     created: $TODAY
     ---

   Journal/ stays empty (SessionEnd populates).

5. Summarize files created with their sources, then address the original user request.
EOF
fi
