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
# Source config tolerantly: `set +u` so unset vars referenced in the file
# don't abort, `|| true` so a parse error doesn't kill setup mid-scaffold.
# Defaults below cover anything the file failed to set.
if [ -f "$CONFIG_FILE" ]; then
  set +u
  # shellcheck disable=SC1090
  . "$CONFIG_FILE" || echo "warning: $CONFIG_FILE failed to source cleanly — using defaults" >&2
  set -u
fi

VAULT_PATH="${OBSIDIAN_VAULT_PATH:-$HOME/Documents/Obsidian Memory}"
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
# 3. Vault scaffold — three top-level folders. Project scoping is via the
# `project:` frontmatter tag, not folder hierarchy.
# ---------------------------------------------------------------------------
mkdir -p "$VAULT_PATH"
mkdir -p "$VAULT_PATH/Tools"
mkdir -p "$VAULT_PATH/Journals"
mkdir -p "$VAULT_PATH/Notes"

# ---------------------------------------------------------------------------
# 4. Render templates recursively. Substitutes __TODAY__ and __VAULT_PATH__
# in every rendered note.
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

TEMPLATES_DIR="$PLUGIN_ROOT/templates"
while IFS= read -r src; do
  rel="${src#"$TEMPLATES_DIR"/}"
  case "$rel" in
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
  # Always maintain the stable symlink so toggling the flag later doesn't
  # require re-running setup. The flag only gates the settings.json patch.
  ln -sfn "$PLUGIN_STATUSLINE" "$STABLE_STATUSLINE"
  echo "[+] linked $STABLE_STATUSLINE → $PLUGIN_STATUSLINE"

  bash "$PLUGIN_ROOT/scripts/_ensure_statusline.sh" "$STABLE_STATUSLINE"
fi

# ---------------------------------------------------------------------------
# 6. Register vault with Obsidian.app so `obsidian://open?path=...` resolves
# and the vault appears in Obsidian's vault switcher. Idempotent: skips if
# the path is already registered. Warns (does not fail) if Obsidian is
# running, since concurrent writes to obsidian.json could be clobbered.
# ---------------------------------------------------------------------------
case "$(uname -s)" in
  Darwin) OBSIDIAN_REGISTRY="$HOME/Library/Application Support/obsidian/obsidian.json" ;;
  Linux)  OBSIDIAN_REGISTRY="$HOME/.config/obsidian/obsidian.json" ;;
  *)      OBSIDIAN_REGISTRY="" ;;
esac

if [ -n "$OBSIDIAN_REGISTRY" ] && command -v jq >/dev/null 2>&1; then
  if [ ! -f "$OBSIDIAN_REGISTRY" ]; then
    mkdir -p "$(dirname "$OBSIDIAN_REGISTRY")"
    echo '{"vaults":{}}' > "$OBSIDIAN_REGISTRY"
  fi
  ALREADY=$(jq --arg p "$VAULT_PATH" \
    '[.vaults // {} | to_entries[] | select(.value.path == $p)] | length' \
    "$OBSIDIAN_REGISTRY" 2>/dev/null || echo 0)
  if [ "$ALREADY" = "0" ]; then
    # If Obsidian.app is running, refuse to patch obsidian.json — its quit-time
    # write would race ours and silently lose the registration. Better to ask
    # the user to quit it (or register the vault from Obsidian's UI directly)
    # than to half-write and have it disappear minutes later.
    OBSIDIAN_RUNNING=0
    if pgrep -x Obsidian >/dev/null 2>&1; then OBSIDIAN_RUNNING=1; fi
    if [ "$OBSIDIAN_RUNNING" = "1" ]; then
      echo "[skip] Obsidian.app is running — vault not auto-registered."
      echo "       Register the vault by either:"
      echo "         1. Quit Obsidian and re-run this setup script, OR"
      echo "         2. In Obsidian: 'Open folder as vault' → choose $VAULT_PATH"
    else
      VAULT_ID=$(python3 -c 'import secrets; print(secrets.token_hex(8))')
      TS_MS=$(python3 -c 'import time; print(int(time.time()*1000))')
      cp "$OBSIDIAN_REGISTRY" "${OBSIDIAN_REGISTRY}.bak.$(date +%Y%m%d%H%M%S)"
      if jq --arg id "$VAULT_ID" --arg p "$VAULT_PATH" --argjson ts "$TS_MS" \
           '.vaults = ((.vaults // {}) + {($id): {path: $p, ts: $ts}})' \
           "$OBSIDIAN_REGISTRY" > "${OBSIDIAN_REGISTRY}.tmp" \
         && mv "${OBSIDIAN_REGISTRY}.tmp" "$OBSIDIAN_REGISTRY"; then
        echo "[+] registered vault with Obsidian.app"
      else
        echo "[warn] failed to write $OBSIDIAN_REGISTRY — register the vault manually via Obsidian's 'Open folder as vault'."
      fi
    fi
  else
    echo "[=] vault already registered with Obsidian.app"
  fi
fi

echo ""
echo "Done. Next steps:"
echo "  1. (Optional) Open the vault in Obsidian.app: open -a Obsidian \"$VAULT_PATH\""
echo "  2. (Optional but recommended) Init git in the vault for change-tracking + auto-commit:"
echo "       cd \"$VAULT_PATH\" && git init -b main && git add -A && git commit -m 'Initial commit'"
echo "  3. Edit $CONFIG_FILE to override defaults (vault path, gate behavior, autocommit/push)."
echo "  4. cd into a project repo and start a Claude session — when prompted, answer 'yes'"
echo "     to register it as a project-vault (or run /obsidian-memory:project enable later)."
