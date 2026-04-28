#!/bin/bash
# obsidian-memory plugin setup — scaffolds the vault, config, and secrets file.
#
# Idempotent: skips files/directories that already exist. Safe to re-run.
#
# Resolves the plugin root (where templates live) via:
#   1. CLAUDE_PLUGIN_ROOT env var (set by Claude Code when invoking plugin scripts)
#   2. ../  relative to this script (when invoked directly)

set -eu

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
if [ -f "$CONFIG_FILE" ]; then
  # shellcheck disable=SC1090
  . "$CONFIG_FILE"
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
# 3. Vault scaffold
# ---------------------------------------------------------------------------
mkdir -p "$VAULT_PATH"
mkdir -p "$VAULT_PATH/Tools"
mkdir -p "$VAULT_PATH/General/Preferences" "$VAULT_PATH/General/People" "$VAULT_PATH/General/Admin" "$VAULT_PATH/General/References"
mkdir -p "$VAULT_PATH/Projects"

# Render a template into the vault, substituting __TODAY__ and __VAULT_PATH__.
# Skip if the destination already exists.
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

render "$PLUGIN_ROOT/templates/INDEX.md"                            "$VAULT_PATH/INDEX.md"
render "$PLUGIN_ROOT/templates/Tools/INDEX.md"                      "$VAULT_PATH/Tools/INDEX.md"
render "$PLUGIN_ROOT/templates/Tools/Obsidian.md"                   "$VAULT_PATH/Tools/Obsidian.md"
render "$PLUGIN_ROOT/templates/General/INDEX.md"                    "$VAULT_PATH/General/INDEX.md"
render "$PLUGIN_ROOT/templates/General/user.md"                     "$VAULT_PATH/General/user.md"
render "$PLUGIN_ROOT/templates/General/References/secrets-env.md"   "$VAULT_PATH/General/References/secrets-env.md"

# .gitignore at vault root
if [ ! -f "$VAULT_PATH/.gitignore" ]; then
  cp "$PLUGIN_ROOT/templates/.gitignore" "$VAULT_PATH/.gitignore"
  echo "[+] $VAULT_PATH/.gitignore"
fi

echo ""
echo "Done. Next steps:"
echo "  1. (Optional) Open the vault in Obsidian.app: \`open -a Obsidian \"$VAULT_PATH\"\`"
echo "  2. (Optional) Init git in the vault for change-tracking + auto-commit:"
echo "     cd \"$VAULT_PATH\" && git init -b main && git add -A && git commit -m 'Initial commit'"
echo "  3. Edit $CONFIG_FILE to override defaults if needed."
echo "  4. Add per-project folders as you start work:"
echo "     mkdir -p \"$VAULT_PATH/Projects/<name>/{Journal,Decisions,Learnings,Research,References}\""
echo "     and create INDEX.md + overview.md from $PLUGIN_ROOT/templates/Projects/PROJECT_NAME/"
