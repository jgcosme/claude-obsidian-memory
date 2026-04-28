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
=== OBSIDIAN MEMORY SYSTEM ===

Your persistent memory lives at $OBSIDIAN_VAULT_PATH.

This session has access to a "vault overview" (below) — a structured listing
of every note in the vault, generated fresh from frontmatter. There are no
INDEX files; the overview IS the index, and it's always current.

RECALL: prefer the auto-overview to know what exists. For deep reads, three options:
  - Read tool: read "$OBSIDIAN_VAULT_PATH/<path>"
  - Plugin search CLI (works without Obsidian.app):
      python3 "$PLUGIN_ROOT/scripts/_vault.py" search --type <t> --keywords "<k>" --path-prefix <p>
      python3 "$PLUGIN_ROOT/scripts/_vault.py" search --created-after YYYY-MM-DD
  - Obsidian search (only if Obsidian.app is running):
      obsidian search query="[type:decision] keywords" — supports bracket-syntax frontmatter filters

REMEMBER (write a new note when ALL hold):
  - Information is significant: user correction, validated approach, novel fact, decision, "remember this"
  - No existing note covers it (search the vault first to dedupe)
  - It will still be useful in future sessions (skip ephemeral session details)

Frontmatter is required on every note (except README files):
  type, description, created (and \`project\` for project-scoped notes)

UPDATE: if a memory turns out wrong/outdated, fix or remove it. Verify file paths and function names before recommending — they may be stale.

DO NOT write to ~/.claude/projects/*/memory/ — that auto-memory dir is disabled in favor of this vault.

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

This cwd ($PROJECT_DIR) has no Projects/$PROJECT_NAME/ folder in the vault.

BEFORE doing anything else this session, ask the user ONCE:
  "Create memory scaffolding for project '$PROJECT_NAME' in the Obsidian vault? (y/n)"

If the user says NO, respect that for the rest of the session: do not create
the folder, and do not write project-scoped notes. General/ and Tools/ notes
are still fine.

If the user says YES, scaffold AND prefill from real evidence in the project dir.

STEP 1 — Create folders + base overview from the template:

  mkdir -p "$PROJECT_VAULT_DIR"/{Decisions,Learnings,Research,References,Journal}
  sed -e 's|__PROJECT_NAME__|$PROJECT_NAME|g' -e 's|__TODAY__|$TODAY|g' \\
    "$TEMPLATE_DIR/overview.md" > "$PROJECT_VAULT_DIR/overview.md"

(There are no INDEX files to scaffold — the auto-overview is regenerated each
session from frontmatter.)

STEP 2 — Inspect $PROJECT_DIR using your judgment. Read whatever the project
actually has that establishes what it is and how it works: top-level docs
(README, ARCHITECTURE, CONTRIBUTING, CHANGELOG, etc.), package manifests, ADR
folders, runbooks, design docs, RFCs, /docs content, build/CI config —
whatever exists. Don't recurse into source code or vendored deps. Run
\`git -C "$PROJECT_DIR" remote get-url origin 2>/dev/null\` and
\`git -C "$PROJECT_DIR" branch --show-current 2>/dev/null\` for repo metadata.

STEP 3 — Populate overview.md from the synthesized context. Keep the existing
section headings (## What it is, ## Goals, ## Current branch / focus,
## Stakeholders, ## Notes). Cite source files inline ("(from README.md)").
Leave sections empty when there's no grounded evidence — do not invent.

STEP 4 — Seed every relevant subfolder with notes derived from material
already in the project, classified by content type. Skip README.md (it's the
source for overview). There is no count cap — use judgment about whether each
candidate doc is worth a pointer. Skip auto-generated files, license files,
vendored READMEs, etc.

  - References/ — entry-point pointers a future session would want to come
    back to: architecture overviews, API/OpenAPI specs, getting-started,
    contributing guides.
  - Decisions/ — ADRs and design choices with rationale.
  - Learnings/ — runbooks, troubleshooting guides, postmortems.
  - Research/ — design docs, RFCs, exploratory write-ups.

  Each note: 1-3 sentence summary + the relative path to the source file so
  it can be reread on demand. Frontmatter:

    ---
    type: reference | decision | learning | research
    description: <one-line>
    project: $PROJECT_NAME
    created: $TODAY
    ---

  No INDEX file maintenance needed — the next SessionStart's overview will
  pick up these new notes automatically.

  Journal/ stays empty — it's populated by SessionEnd.

STEP 5 — Summarize for the user: list the files you created/populated and
which source files in the project they were grounded in. Then address the
user's original request (which prompted this session).
EOF
fi
