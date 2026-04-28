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

# 5. Current project scope.
PROJECT_INDEX="$OBSIDIAN_VAULT_PATH/Projects/$PROJECT_NAME/INDEX.md"
if [ -f "$PROJECT_INDEX" ]; then
  echo "=== PROJECT: $PROJECT_NAME ==="
  cat "$PROJECT_INDEX"
else
  echo "=== PROJECT: $PROJECT_NAME (no memory scaffolded) ==="
  echo "No Projects/$PROJECT_NAME/ folder exists in the vault."
  echo "Confirm with the user before creating one — verify the project name and scope first."
fi
