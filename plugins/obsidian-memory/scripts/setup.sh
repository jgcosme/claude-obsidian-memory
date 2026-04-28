#!/bin/bash
# obsidian-memory plugin setup — scaffolds the vault, config, and secrets file.
#
# Idempotent: skips files/directories that already exist. Safe to re-run.
#
# Resolves the plugin root (where templates live) via:
#   1. CLAUDE_PLUGIN_ROOT env var (set by Claude Code when invoking plugin scripts)
#   2. ../  relative to this script (when invoked directly)

set -u

# ---------------------------------------------------------------------------
# Resolve plugin root and load config
# ---------------------------------------------------------------------------
if [ -n "${CLAUDE_PLUGIN_ROOT:-}" ]; then
  PLUGIN_ROOT="$CLAUDE_PLUGIN_ROOT"
else
  PLUGIN_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
fi

if [ ! -d "$PLUGIN_ROOT/templates" ]; then
  echo "error: templates directory not found at $PLUGIN_ROOT/templates" >&2
  echo "If running outside Claude Code, set CLAUDE_PLUGIN_ROOT to the plugin install path." >&2
  exit 1
fi

CONFIG_FILE="${HOME}/.config/claude-memory/config.env"
# Source config inside a subshell-style guard so a malformed config.env doesn't
# abort setup mid-way through scaffolding.
if [ -f "$CONFIG_FILE" ]; then
  if ! ( set +u; . "$CONFIG_FILE" ) 2>/dev/null; then
    echo "warning: $CONFIG_FILE failed to source cleanly — using defaults" >&2
  else
    # shellcheck disable=SC1090
    . "$CONFIG_FILE" || true
  fi
fi

VAULT_PATH="${OBSIDIAN_VAULT_PATH:-$HOME/Documents/Obsidian Vault}"
TODAY=$(date +%Y-%m-%d)

echo "obsidian-memory setup"
echo "  plugin root: $PLUGIN_ROOT"
echo "  vault:       $VAULT_PATH"
echo "  config:      $CONFIG_FILE"
echo ""

# ---------------------------------------------------------------------------
# 1. Config file
# ---------------------------------------------------------------------------
if [ ! -f "$CONFIG_FILE" ]; then
  mkdir -p "$(dirname "$CONFIG_FILE")"
  chmod 700 "$(dirname "$CONFIG_FILE")"
  cp "$PLUGIN_ROOT/examples/config.env.example" "$CONFIG_FILE"
  echo "[+] created $CONFIG_FILE (edit to customize paths)"
else
  echo "[=] config exists, leaving it alone: $CONFIG_FILE"
fi

# ---------------------------------------------------------------------------
# 2. Secrets file (empty template)
# ---------------------------------------------------------------------------
SECRETS_FILE="${HOME}/.config/claude-memory/secrets.env"
if [ ! -f "$SECRETS_FILE" ]; then
  cp "$PLUGIN_ROOT/examples/secrets.env.example" "$SECRETS_FILE"
  chmod 600 "$SECRETS_FILE"
  echo "[+] created $SECRETS_FILE (chmod 600; add credentials as needed)"
else
  echo "[=] secrets exists, leaving it alone: $SECRETS_FILE"
fi

# ---------------------------------------------------------------------------
# 3. Vault scaffold — base directory layout
# ---------------------------------------------------------------------------
mkdir -p "$VAULT_PATH"
mkdir -p "$VAULT_PATH/Tools"
mkdir -p "$VAULT_PATH/General/Preferences" "$VAULT_PATH/General/People" "$VAULT_PATH/General/Admin" "$VAULT_PATH/General/References"
mkdir -p "$VAULT_PATH/Projects"

# ---------------------------------------------------------------------------
# v1.1 migration: vaults from earlier versions had hand-maintained INDEX.md
# files. Auto-overview replaces them. Migrate idempotently.
#   - rename root INDEX.md → README.md (if README doesn't already exist)
#   - archive sub-INDEX files to .archive/v1.1-migration/<original-path>
# ---------------------------------------------------------------------------
ARCHIVE_DIR="$VAULT_PATH/.archive/v1.1-migration"
if [ -f "$VAULT_PATH/INDEX.md" ] && [ ! -f "$VAULT_PATH/README.md" ]; then
  mv "$VAULT_PATH/INDEX.md" "$VAULT_PATH/README.md"
  echo "[~] migrated $VAULT_PATH/INDEX.md → $VAULT_PATH/README.md (v1.1)"
fi
moved_any=0
while IFS= read -r idx; do
  rel="${idx#"$VAULT_PATH"/}"
  dest="$ARCHIVE_DIR/$rel"
  mkdir -p "$(dirname "$dest")"
  mv "$idx" "$dest"
  echo "[~] archived $rel → .archive/v1.1-migration/$rel"
  moved_any=1
done < <(find "$VAULT_PATH" -type f -name 'INDEX.md' \
  ! -path "$VAULT_PATH/.archive/*" \
  \( -path "$VAULT_PATH/Tools/INDEX.md" \
     -o -path "$VAULT_PATH/General/INDEX.md" \
     -o -path "$VAULT_PATH/Projects/*/INDEX.md" \) 2>/dev/null)
if [ "$moved_any" = "1" ]; then
  echo "    (the auto-overview at SessionStart now replaces these. Delete the archive once you're sure nothing was lost.)"
fi

# ---------------------------------------------------------------------------
# 4. Render templates recursively, skipping per-project templates and metadata.
# Substitutes __TODAY__ and __VAULT_PATH__ in every rendered note.
# ---------------------------------------------------------------------------
render() {
  local src="$1"
  local dst="$2"
  if [ -f "$dst" ]; then
    echo "[=] $dst exists, skipping"
    return
  fi
  mkdir -p "$(dirname "$dst")"
  sed \
    -e "s|__TODAY__|${TODAY}|g" \
    -e "s|__VAULT_PATH__|${VAULT_PATH}|g" \
    "$src" > "$dst"
  echo "[+] $dst"
}

# Walk templates/ — anything that's not under Projects/PROJECT_NAME/ (per-project
# scaffold, applied lazily by SessionStart) or a .gitignore (handled separately)
# gets rendered into the vault preserving its relative path.
TEMPLATES_DIR="$PLUGIN_ROOT/templates"
while IFS= read -r src; do
  rel="${src#"$TEMPLATES_DIR"/}"
  case "$rel" in
    Projects/*) continue ;;       # per-project, scaffolded by SessionStart
    .gitignore) continue ;;       # handled below
  esac
  render "$src" "$VAULT_PATH/$rel"
done < <(find "$TEMPLATES_DIR" -type f -name '*.md' | sort)

# .gitignore at vault root (no template substitution; copy verbatim)
if [ -f "$TEMPLATES_DIR/.gitignore" ] && [ ! -f "$VAULT_PATH/.gitignore" ]; then
  cp "$TEMPLATES_DIR/.gitignore" "$VAULT_PATH/.gitignore"
  echo "[+] $VAULT_PATH/.gitignore"
fi

echo ""
echo "Done. Next steps:"
echo "  1. (Optional) Open the vault in Obsidian.app: open -a Obsidian \"$VAULT_PATH\""
echo "  2. (Optional but recommended) Init git in the vault for change-tracking + auto-commit:"
echo "       cd \"$VAULT_PATH\" && git init -b main && git add -A && git commit -m 'Initial commit'"
echo "  3. Edit $CONFIG_FILE to override defaults (vault path, gate behavior, autocommit/push)."
echo "  4. cd into a project directory and start a Claude session — when prompted, answer 'yes'"
echo "     to scaffold that project's vault folder. Claude prefills it from real evidence in the repo."
