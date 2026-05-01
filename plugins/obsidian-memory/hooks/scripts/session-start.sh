#!/bin/bash
# SessionStart hook: load Obsidian-backed memory into context.
# Stdout becomes context injected at the start of every Claude session.

set -u

# ---------------------------------------------------------------------------
# Recursion guard: the review and gate subprocesses spawn `claude -p`, which
# fires its own SessionStart. We don't want to inject the vault overview into
# their tiny system-prompt context.
# ---------------------------------------------------------------------------
if [ -n "${CLAUDE_MEMORY_REVIEW:-}" ] || [ -n "${CLAUDE_MEMORY_GATE:-}" ]; then
  exit 0
fi

# ---------------------------------------------------------------------------
# Read stdin payload (JSON from Claude Code) — only used for the session_id we
# need to write per-session usage events. Failure here is non-fatal: usage
# tracking is skipped, the rest of the hook continues.
# ---------------------------------------------------------------------------
PAYLOAD=$(cat 2>/dev/null || true)
SESSION_ID=$(printf '%s' "$PAYLOAD" | jq -r '.session_id // empty' 2>/dev/null || true)

# ---------------------------------------------------------------------------
# Config: load from ~/.config/obsidian-memory/config.env if present, else use defaults.
# ---------------------------------------------------------------------------
CONFIG_DIR="${HOME}/.config/obsidian-memory"
CONFIG_FILE="${CONFIG_DIR}/config.env"
if [ -f "$CONFIG_FILE" ]; then
  # shellcheck disable=SC1090
  . "$CONFIG_FILE" 2>/dev/null || true
fi

OBSIDIAN_VAULT_PATH="${OBSIDIAN_VAULT_PATH:-$HOME/Documents/Obsidian Memory}"
PLUGIN_ROOT="${CLAUDE_PLUGIN_ROOT:-}"

# Fallback: derive plugin root from this script's location
# ($PLUGIN_ROOT/hooks/scripts/session-start.sh) when the harness doesn't
# export CLAUDE_PLUGIN_ROOT. Without this, the first-time-setup heredoc
# emits a bare "/scripts/setup.sh" path that fails when Claude runs it.
if [ -z "$PLUGIN_ROOT" ]; then
  _SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd)"
  if [ -n "$_SCRIPT_DIR" ]; then
    PLUGIN_ROOT="$(cd "$_SCRIPT_DIR/../.." 2>/dev/null && pwd)"
  fi
fi

# Refresh the stable status-line symlink each session so plugin upgrades
# (which move CLAUDE_PLUGIN_ROOT to a new versioned path) keep working
# without requiring the user to re-run setup.
if [ -n "$PLUGIN_ROOT" ] && [ -f "$PLUGIN_ROOT/scripts/statusline.py" ] && [ -d "$CONFIG_DIR" ]; then
  ln -sfn "$PLUGIN_ROOT/scripts/statusline.py" "$CONFIG_DIR/statusline.py" 2>/dev/null || true
fi

# Auto-detect Obsidian CLI (optional — only used for reactive queries)
if [ -z "${OBSIDIAN_CLI:-}" ]; then
  if [ -x "/Applications/Obsidian.app/Contents/MacOS/obsidian" ]; then
    OBSIDIAN_CLI="/Applications/Obsidian.app/Contents/MacOS/obsidian"
  elif command -v obsidian >/dev/null 2>&1; then
    OBSIDIAN_CLI="$(command -v obsidian)"
  fi
fi

# Derive current project name from cwd basename. Done before the vault-
# existence check so the project-registration detection (which is independent
# of vault existence) can run on the very first session too.
PROJECT_DIR="${CLAUDE_PROJECT_DIR:-$PWD}"
PROJECT_NAME=$(basename "$PROJECT_DIR")
TODAY=$(date +%Y-%m-%d)

# Project-vault status: look up the cwd's project in projects.json. Three states:
#   - enabled       → run init silently (idempotent), pass --project-vault to overview
#   - disabled      → silent (user explicitly declined)
#   - not_registered + has candidate .md files → capture one-time registration prompt
#
# Done BEFORE the output block so the registration prompt can be emitted at the
# very top of stdout — when the hook output exceeds the persisted-output preview
# threshold (~2KB), only the first chunk is shown to the model. Burying the
# prompt below the vault overview makes it easy to miss.
PROJECT_VAULT_PATH=""
PROJECT_VAULT_STATUS=""
REGISTER_PROMPT_NEEDED=""
CANDIDATE_COUNT=0
PROJECT_ROOT=""
PROJECTS_PY="$PLUGIN_ROOT/scripts/_projects.py"
INIT_PY="$PLUGIN_ROOT/scripts/init_project_vault.py"
PROJECT_DOCS_PY="$PLUGIN_ROOT/scripts/_project_docs.py"
if [ -n "$PLUGIN_ROOT" ] && [ -f "$PROJECTS_PY" ]; then
  PROJECT_ROOT=$(git -C "$PROJECT_DIR" rev-parse --show-toplevel 2>/dev/null || echo "")
  if [ -n "$PROJECT_ROOT" ]; then
    PROJECT_VAULT_STATUS=$(python3 "$PROJECTS_PY" lookup "$PROJECT_ROOT" 2>/dev/null || echo "")
    case "$PROJECT_VAULT_STATUS" in
      enabled)
        # Eager init before overview so newly-added docs are surfaced this session.
        if [ -f "$INIT_PY" ]; then
          python3 "$INIT_PY" "$PROJECT_ROOT" --project "$PROJECT_NAME" >/dev/null 2>&1 || true
        fi
        PROJECT_VAULT_PATH="$PROJECT_ROOT"
        # Persist the resolved project-vault path for this session so the gate
        # (UserPromptSubmit) can re-use it without re-querying the registry.
        if [ -n "${SAFE_SID:-}" ]; then
          printf '%s\n' "$PROJECT_VAULT_PATH" > "$SESSION_STATE_DIR/$SAFE_SID.project_vault" 2>/dev/null || true
        fi
        ;;
      not_registered)
        # Check if there are any candidate .md files worth surfacing.
        if [ -f "$PROJECT_DOCS_PY" ]; then
          CANDIDATE_COUNT=$(python3 "$PROJECT_DOCS_PY" enumerate "$PROJECT_ROOT" 2>/dev/null | wc -l | tr -d ' ')
        fi
        if [ "${CANDIDATE_COUNT:-0}" -gt 0 ]; then
          REGISTER_PROMPT_NEEDED=1
        fi
        ;;
      disabled|*)
        : # silent — user opted out, or registry unreadable
        ;;
    esac
  fi
fi

# Vault-existence-dependent side effects: only do these when the vault
# already exists. The emit block below handles both branches (first-time
# setup vs. normal startup) and is wrapped in a single tee so total bytes
# injected get logged for /obsidian-memory:usage.
if [ -d "$OBSIDIAN_VAULT_PATH" ]; then
  # Open Obsidian.app if available (purely for the optional `obsidian` CLI).
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

  # Record vault HEAD SHA so SessionEnd can diff-scope backlink reconciliation
  # to "what changed during this session" — including mid-session commits,
  # which working-tree-only diff would miss.
  SESSION_STATE_DIR="${MEMORY_SESSION_STATE_DIR:-/tmp/claude-memory-session}"
  mkdir -p "$SESSION_STATE_DIR" 2>/dev/null || true
  if [ -n "$SESSION_ID" ]; then
    SAFE_SID=$(echo "$SESSION_ID" | tr -c 'A-Za-z0-9._-' '_')
    if [ -d "$OBSIDIAN_VAULT_PATH/.git" ]; then
      git -C "$OBSIDIAN_VAULT_PATH" rev-parse HEAD > "$SESSION_STATE_DIR/$SAFE_SID.vault_head" 2>/dev/null || true
    fi
  fi
fi

# Emit memory bootstrap as injected context.
# Wrapped in { ... } | tee so we can record the total bytes injected for
# /obsidian-memory:usage. tee is reliable here (vs. process substitution)
# because it returns only after stdin EOFs.
USAGE_TMP=$(mktemp 2>/dev/null || echo "")
{
if [ ! -d "$OBSIDIAN_VAULT_PATH" ]; then
  # First-time setup: vault doesn't exist yet. Emit the setup prompt so Claude
  # asks the user before scaffolding. setup.sh is idempotent and only writes
  # under $OBSIDIAN_VAULT_PATH and ~/.config/obsidian-memory/.
  cat <<EOF
=== OBSIDIAN MEMORY (first-time setup) ===

The plugin is installed but no vault exists yet at: $OBSIDIAN_VAULT_PATH

The plugin owns this vault entirely — three top-level folders (Tools/,
Journals/, Notes/) plus a README. Project scoping is via the project:
frontmatter tag on individual notes, not folder hierarchy.

Before doing anything else this session, ask the user ONCE:
  "Set up the obsidian-memory vault at $OBSIDIAN_VAULT_PATH? This creates the
   vault directory with Tools/, Journals/, Notes/ subfolders, and writes a
   config to ~/.config/obsidian-memory/. Fully reversible. (y/n)"

If YES:
  1. bash "$PLUGIN_ROOT/scripts/setup.sh"
  2. Ask: "Initialize the vault as a git repo so SessionEnd can auto-commit
     memory writes? (y/n)"
     If yes:
       cd "$OBSIDIAN_VAULT_PATH" && git init -b main && git add -A && git commit -m "Initial commit"
  3. Summarize what was created, then continue with the user's original request.

If NO, respect that: do not write to the vault this session. The user can
run setup later with:
  bash "$PLUGIN_ROOT/scripts/setup.sh"
or check current state with:
  /obsidian-memory:status

To use a different vault path, set OBSIDIAN_VAULT_PATH in
~/.config/obsidian-memory/config.env first.
EOF

  # Project-vault registration prompt also fires on the first session so the
  # user gets both questions (vault setup + project registration) in one pass,
  # rather than only seeing project registration on session #2.
  if [ -n "$REGISTER_PROMPT_NEEDED" ]; then
    cat <<EOF

=== ACTION REQUIRED — project-vault registration (one-time) ===

After completing vault setup above, also ask the user:

  "Register '$PROJECT_NAME' as a project-vault? This will:
    - Add Obsidian frontmatter (type/description/created/project) to .md files
      that don't already have any frontmatter (idempotent — files with
      frontmatter are skipped)
    - Surface those docs in future SessionStart overviews and vault-search results
    - Route project-scoped save-memory writes to the matching project folder
      when one exists
   Answer y/n."

Context: this project ($PROJECT_NAME at $PROJECT_ROOT) has $CANDIDATE_COUNT
candidate .md file(s) and is not yet registered. The prompt only fires once
per project — do not skip it.

YES → run both:
  python3 "$INIT_PY" "$PROJECT_ROOT" --project "$PROJECT_NAME"
  python3 "$PROJECTS_PY" register "$PROJECT_ROOT" --enabled --project "$PROJECT_NAME"

NO → run:
  python3 "$PROJECTS_PY" register "$PROJECT_ROOT" --no-enabled --project "$PROJECT_NAME"

Either way, the answer is durable. To revisit later, edit
~/.config/obsidian-memory/projects.json.
EOF
  fi
else
  # Normal startup: vault exists. Emit registration prompt (if needed),
  # standard instructions, README, and the auto-generated overview.

  # Registration prompt goes FIRST so it survives persisted-output truncation
  # (the harness shows only the first ~2KB of large hook output as preview).
  if [ -n "$REGISTER_PROMPT_NEEDED" ]; then
    cat <<EOF
=== ACTION REQUIRED — project-vault registration (one-time) ===

Before responding to the user's first message this session, ask them:

  "Register '$PROJECT_NAME' as a project-vault? This will:
    - Add Obsidian frontmatter (type/description/created/project) to .md files
      that don't already have any frontmatter (idempotent — files with
      frontmatter are skipped)
    - Surface those docs in future SessionStart overviews and vault-search results
    - Route project-scoped save-memory writes to the matching project folder
      when one exists
   Answer y/n."

Context: this project ($PROJECT_NAME at $PROJECT_ROOT) has $CANDIDATE_COUNT
candidate .md file(s) and is not yet registered. The prompt only fires once
per project — do not skip it.

YES → run both:
  python3 "$INIT_PY" "$PROJECT_ROOT" --project "$PROJECT_NAME"
  python3 "$PROJECTS_PY" register "$PROJECT_ROOT" --enabled --project "$PROJECT_NAME"

NO → run:
  python3 "$PROJECTS_PY" register "$PROJECT_ROOT" --no-enabled --project "$PROJECT_NAME"

Either way, the answer is durable. To revisit later, edit
~/.config/obsidian-memory/projects.json.

EOF
  fi

  cat <<INSTRUCTIONS
=== OBSIDIAN MEMORY ===

Vault: $OBSIDIAN_VAULT_PATH
Index: the auto-overview below — regenerated each session from frontmatter.

RECALL — invoke the \`vault-search\` skill for body-level lookups (the gate above only matches descriptions).
REMEMBER — invoke the \`save-memory\` skill to write notes.

INSTRUCTIONS

  # README at vault root (if present) gives prose orientation.
  if [ -f "$OBSIDIAN_VAULT_PATH/README.md" ]; then
    echo "=== VAULT README ==="
    cat "$OBSIDIAN_VAULT_PATH/README.md"
    echo ""
  fi

  # Auto-generated vault overview (the load-bearing piece).
  # Goes through the shared cache helper so subsequent UserPromptSubmit calls
  # can reuse the same cache file when the vault hasn't changed. We always run
  # the helper to warm the cache for the gate; OBSIDIAN_MEMORY_BOOTSTRAP_OVERVIEW
  # controls whether the overview is also injected into the main session's
  # context. Setting it to false drops ~5–15KB cache_read tokens per turn but
  # removes the main session's in-context "scan for relevance" map — the gate
  # still has its own copy and continues to work.
  OVERVIEW_HELPER="$PLUGIN_ROOT/hooks/scripts/_overview.sh"
  if [ -n "$PLUGIN_ROOT" ] && [ -x "$OVERVIEW_HELPER" ]; then
    OVERVIEW_OUT=$(bash "$OVERVIEW_HELPER" "$OBSIDIAN_VAULT_PATH" "$PROJECT_NAME" "$PROJECT_VAULT_PATH")
    if [ "${OBSIDIAN_MEMORY_BOOTSTRAP_OVERVIEW:-true}" = "true" ]; then
      echo "=== VAULT OVERVIEW (auto-generated from frontmatter) ==="
      if [ -n "$OVERVIEW_OUT" ]; then
        printf '%s\n' "$OVERVIEW_OUT"
      else
        echo "(overview generation failed — check that python3 ≥ 3.9 is available)"
      fi
      echo ""
    fi
  fi
fi

} | { if [ -n "$USAGE_TMP" ]; then tee "$USAGE_TMP"; else cat; fi; }

# Record the total injected-context size for /obsidian-memory:usage.
if [ -n "$SESSION_ID" ] && [ -n "$USAGE_TMP" ] && [ -f "$USAGE_TMP" ]; then
  SIZE=$(wc -c < "$USAGE_TMP" 2>/dev/null | tr -d ' ' || echo 0)
  bash "$PLUGIN_ROOT/hooks/scripts/_usage_log.sh" chars "$SESSION_ID" session_start "${SIZE:-0}" 2>/dev/null || true
fi
[ -n "$USAGE_TMP" ] && rm -f "$USAGE_TMP"
