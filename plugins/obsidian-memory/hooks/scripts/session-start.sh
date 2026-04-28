#!/bin/bash
# SessionStart hook: load Obsidian-backed memory into context.
# Stdout becomes context injected at the start of every Claude session.

set -u

# ---------------------------------------------------------------------------
# Config: load from ~/.config/claude-memory/config.env if present, else use defaults.
# Required: OBSIDIAN_VAULT_PATH, OBSIDIAN_CLI
# ---------------------------------------------------------------------------
CONFIG_FILE="${HOME}/.config/claude-memory/config.env"
if [ -f "$CONFIG_FILE" ]; then
  # shellcheck disable=SC1090
  . "$CONFIG_FILE"
fi

# Defaults
OBSIDIAN_VAULT_PATH="${OBSIDIAN_VAULT_PATH:-$HOME/Documents/Obsidian Vault}"

# Auto-detect Obsidian CLI if not set: macOS app bundle, then PATH.
if [ -z "${OBSIDIAN_CLI:-}" ]; then
  if [ -x "/Applications/Obsidian.app/Contents/MacOS/obsidian" ]; then
    OBSIDIAN_CLI="/Applications/Obsidian.app/Contents/MacOS/obsidian"
  elif command -v obsidian >/dev/null 2>&1; then
    OBSIDIAN_CLI="$(command -v obsidian)"
  fi
fi

# ---------------------------------------------------------------------------
# Defensive: if vault doesn't exist, emit a helpful note and exit cleanly.
# This prevents the hook from breaking new sessions when setup is incomplete.
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
# 1. Ensure Obsidian.app is running (CLI requires it on macOS). No-op if already running.
# ---------------------------------------------------------------------------
if command -v open >/dev/null 2>&1; then
  open -ga Obsidian 2>/dev/null
fi

# 2. Wait briefly for CLI to be responsive (max ~5s).
if [ -n "${OBSIDIAN_CLI:-}" ] && [ -x "$OBSIDIAN_CLI" ]; then
  for _ in 1 2 3 4 5; do
    if "$OBSIDIAN_CLI" files >/dev/null 2>&1; then
      break
    fi
    sleep 1
  done
fi

# 3. Derive current project name. Prefer CLAUDE_PROJECT_DIR (set by Claude Code), fall back to PWD.
PROJECT_DIR="${CLAUDE_PROJECT_DIR:-$PWD}"
PROJECT_NAME=$(basename "$PROJECT_DIR")

# 4. Emit memory bootstrap as injected context.
cat <<INSTRUCTIONS
=== OBSIDIAN MEMORY SYSTEM ===

Your persistent memory lives at $OBSIDIAN_VAULT_PATH.

RECALL via the obsidian CLI (full syntax in Tools/Obsidian.md, loaded below):
  obsidian search query="[type:learning] [created:YYYY-MM-DD]"   — typed/dated recall
  obsidian search query="path:Projects [type:decision]"           — folder + frontmatter
  obsidian read path="Folder/Note.md"                             — read a note in full

REMEMBER (write a NEW note when ALL hold):
  - Information is significant: user correction, validated approach, novel fact, decision, "remember this"
  - No existing note covers it (search vault first with \`obsidian search\`)
  - It will still be useful in future sessions (skip ephemeral session details)

Vault structure:
  Tools/                — CLIs, APIs (always loaded via INDEX)
  General/              — cross-project: identity, preferences, people, admin, references (always loaded via INDEX)
  Projects/<name>/      — per-project: overview, Decisions, Learnings, Research, References, Journal
  Frontmatter required: type, description, created (and \`project\` for project-scoped notes)

UPDATE: if a memory turns out wrong/outdated, fix or remove it. Verify file paths and function names before recommending — they may be stale.

DO NOT write to ~/.claude/projects/*/memory/ — that auto-memory dir is disabled in favor of this vault.

INSTRUCTIONS

[ -f "$OBSIDIAN_VAULT_PATH/INDEX.md" ] && {
  echo "=== ROOT INDEX ==="
  cat "$OBSIDIAN_VAULT_PATH/INDEX.md"
  echo ""
}

[ -f "$OBSIDIAN_VAULT_PATH/Tools/INDEX.md" ] && {
  echo "=== TOOLS INDEX ==="
  cat "$OBSIDIAN_VAULT_PATH/Tools/INDEX.md"
  echo ""
}

[ -f "$OBSIDIAN_VAULT_PATH/General/INDEX.md" ] && {
  echo "=== GENERAL INDEX ==="
  cat "$OBSIDIAN_VAULT_PATH/General/INDEX.md"
  echo ""
}

# 5. Current project scope. If Projects/<name>/ doesn't exist yet, do NOT
#    silently scaffold — instead, instruct Claude to ask the user at the start
#    of the conversation. This avoids polluting the vault with folders for
#    incidental cwds (~/, /tmp, throwaway clones).
PROJECT_VAULT_DIR="$OBSIDIAN_VAULT_PATH/Projects/$PROJECT_NAME"
PROJECT_INDEX="$PROJECT_VAULT_DIR/INDEX.md"
TEMPLATE_DIR="${CLAUDE_PLUGIN_ROOT:-}/templates/Projects/PROJECT_NAME"
TODAY=$(date +%Y-%m-%d)

if [ -f "$PROJECT_INDEX" ]; then
  echo "=== PROJECT: $PROJECT_NAME ==="
  cat "$PROJECT_INDEX"
else
  cat <<EOF
=== PROJECT: $PROJECT_NAME (not yet scaffolded) ===

This cwd ($PROJECT_DIR) has no Projects/$PROJECT_NAME/ folder in the vault.

BEFORE doing anything else this session, ask the user ONCE:
  "Create memory scaffolding for project '$PROJECT_NAME' in the Obsidian vault? (y/n)"

If the user says NO, respect that for the rest of the session: do not create the
folder, and do not write project-scoped notes (Decisions/Learnings/Research/Journal).
General/ and Tools/ notes are still fine.

If the user says YES, scaffold AND prefill from real evidence in the project dir.

STEP 1 — Create folders + base files from templates:

  mkdir -p "$PROJECT_VAULT_DIR"/{Decisions,Learnings,Research,References,Journal}
  sed -e 's|__PROJECT_NAME__|$PROJECT_NAME|g' -e 's|__TODAY__|$TODAY|g' \\
    "$TEMPLATE_DIR/INDEX.md" > "$PROJECT_INDEX"
  sed -e 's|__PROJECT_NAME__|$PROJECT_NAME|g' -e 's|__TODAY__|$TODAY|g' \\
    "$TEMPLATE_DIR/overview.md" > "$PROJECT_VAULT_DIR/overview.md"

STEP 2 — Inspect $PROJECT_DIR for real, grounded context. Read whichever of these exist:
  - README.md / README.rst / README — purpose, goals, current status
  - package.json, pyproject.toml, Cargo.toml, go.mod, Gemfile — stack & deps
  - CHANGELOG.md, ARCHITECTURE.md, CONTRIBUTING.md, /docs (top level only) — context, processes
  - Run \`git -C "$PROJECT_DIR" remote get-url origin 2>/dev/null\` for the remote URL.
  - Run \`git -C "$PROJECT_DIR" branch --show-current 2>/dev/null\` for the current branch.

  Cap the scan: at most ~10 file reads + 2 git commands. Don't recurse into the codebase.

STEP 3 — Replace the placeholder sections in the overview.md you just wrote with synthesized
content. Keep the existing section headings (## What it is, ## Goals, ## Current branch /
focus, ## Stakeholders, ## Notes). Cite the source file in parentheses where helpful
("(from README.md)"). DO NOT invent facts not in the source — leave a section empty if
the evidence is not there. No bullet should restate the project name; aim for terse,
factual prose.

STEP 4 — Seed up to 5 References/ notes for the most useful entry-point docs found in
STEP 2 (e.g., ARCHITECTURE.md, top-level /docs files, OpenAPI specs). Skip README.md
(it's the source for overview). For each, write Projects/$PROJECT_NAME/References/<slug>.md:

  ---
  type: reference
  description: <one-line summary>
  project: $PROJECT_NAME
  created: $TODAY
  ---

  Body: 1-3 sentences on what's in the file + the relative path so it can be reread on
  demand. Then add the new note under "## References" in $PROJECT_INDEX as
  \`- [[References/<slug>]] — <one-line>\`.

STEP 5 — Match vault formatting conventions exactly. Inspect an already-scaffolded
project (e.g. read $OBSIDIAN_VAULT_PATH/Projects/<some-existing>/INDEX.md or overview.md
if any exist) before writing, and mirror its tone, frontmatter keys, and link style:
  - Frontmatter MUST have: type, description, created, project. Do NOT add tags unless
    you observe tags in the existing vault notes.
  - Cross-references via [[wikilinks]] — bare basename when unique, path-qualified
    (\`[[References/foo]]\`) otherwise.
  - Section headings with \`##\`. No emoji, no HTML.

STEP 6 — Optionally append a bullet under "## Projects" in $OBSIDIAN_VAULT_PATH/INDEX.md
listing the new project (\`- [[Projects/$PROJECT_NAME/INDEX|$PROJECT_NAME]] — <one-line>\`).

STEP 7 — Print a short summary to the user: list the files you created/populated and
which source files they were grounded in. Then continue with the user's actual request.
EOF
fi
