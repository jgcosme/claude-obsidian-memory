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

CONFIG_DIR="${HOME}/.config/obsidian-memory"
CONFIG_FILE="${CONFIG_DIR}/config.env"
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
# 0. Prerequisite check
# ---------------------------------------------------------------------------
echo "Checking prerequisites:"
MISSING=0

require() {
  local tool="$1"; local why="$2"; local install="$3"
  if command -v "$tool" >/dev/null 2>&1; then
    echo "  [ok]   $tool"
  else
    echo "  [MISS] $tool — required ($why); install: $install"
    MISSING=$((MISSING+1))
  fi
}

recommend() {
  local tool="$1"; local why="$2"; local install="$3"
  if command -v "$tool" >/dev/null 2>&1; then
    echo "  [ok]   $tool"
  else
    echo "  [warn] $tool — optional ($why); install: $install"
  fi
}

require jq        "parse hook payloads"           "brew install jq | apt install jq"
require python3   "search CLI + audit + overview" "preinstalled on most systems"

if command -v python3 >/dev/null 2>&1; then
  PYV=$(python3 -c 'import sys; print(f"{sys.version_info.major}.{sys.version_info.minor}")' 2>/dev/null || echo "?")
  PYOK=$(python3 -c 'import sys; print(1 if sys.version_info >= (3,9) else 0)' 2>/dev/null || echo 0)
  if [ "$PYOK" = "1" ]; then
    echo "  [ok]   python3 version $PYV (>= 3.9)"
  else
    echo "  [MISS] python3 version $PYV — need 3.9 or newer"
    MISSING=$((MISSING+1))
  fi
fi

recommend git   "vault history and SessionEnd auto-commit"  "preinstalled on most systems"
case "$(uname -s)" in
  Darwin) recommend flock "concurrent-session safety on auto-commit (macOS)" "brew install flock" ;;
  *)      recommend flock "concurrent-session safety on auto-commit"          "your package manager" ;;
esac

if [ "$MISSING" -gt 0 ]; then
  echo ""
  echo "error: $MISSING required prerequisite(s) missing — install and re-run." >&2
  exit 1
fi
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
SECRETS_FILE="${CONFIG_DIR}/secrets.env"
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

# ---------------------------------------------------------------------------
# 5. Status line: stable symlink + jq-patch ~/.claude/settings.json so the
# plugin's token usage appears as a Claude Code status line out of the box.
# Skips patching if the user already has a statusLine configured (we don't
# clobber existing customizations).
# ---------------------------------------------------------------------------
STABLE_STATUSLINE="${CONFIG_DIR}/statusline.py"
PLUGIN_STATUSLINE="$PLUGIN_ROOT/scripts/statusline.py"
if [ -f "$PLUGIN_STATUSLINE" ]; then
  ln -sfn "$PLUGIN_STATUSLINE" "$STABLE_STATUSLINE"
  echo "[+] linked $STABLE_STATUSLINE → $PLUGIN_STATUSLINE"

  CLAUDE_SETTINGS="${HOME}/.claude/settings.json"
  if command -v jq >/dev/null 2>&1; then
    if [ ! -f "$CLAUDE_SETTINGS" ]; then
      mkdir -p "$(dirname "$CLAUDE_SETTINGS")"
      echo '{}' > "$CLAUDE_SETTINGS"
    fi
    EXISTING=$(jq -r '.statusLine.command // empty' "$CLAUDE_SETTINGS" 2>/dev/null || echo "")
    if [ -z "$EXISTING" ]; then
      cp "$CLAUDE_SETTINGS" "${CLAUDE_SETTINGS}.bak.$(date +%Y%m%d%H%M%S)"
      jq --arg cmd "python3 \"$STABLE_STATUSLINE\"" \
         '.statusLine = {type: "command", command: $cmd}' \
         "$CLAUDE_SETTINGS" > "${CLAUDE_SETTINGS}.tmp" \
        && mv "${CLAUDE_SETTINGS}.tmp" "$CLAUDE_SETTINGS"
      echo "[+] enabled status line in $CLAUDE_SETTINGS"
    elif [ "$EXISTING" = "python3 \"$STABLE_STATUSLINE\"" ]; then
      echo "[=] status line already enabled"
    else
      echo "[=] status line already configured (left as-is). To use the plugin's:"
      echo "    set statusLine.command in $CLAUDE_SETTINGS to:"
      echo "      python3 \"$STABLE_STATUSLINE\""
    fi
  fi
fi

echo ""
echo "Done. Next steps:"
echo "  1. (Optional) Open the vault in Obsidian.app: open -a Obsidian \"$VAULT_PATH\""
echo "  2. (Optional but recommended) Init git in the vault for change-tracking + auto-commit:"
echo "       cd \"$VAULT_PATH\" && git init -b main && git add -A && git commit -m 'Initial commit'"
echo "  3. Edit $CONFIG_FILE to override defaults (vault path, gate behavior, autocommit/push)."
echo "  4. cd into a project directory and start a Claude session — when prompted, answer 'yes'"
echo "     to scaffold that project's vault folder. Claude prefills it from real evidence in the repo."
