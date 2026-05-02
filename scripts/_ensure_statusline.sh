#!/bin/bash
# Idempotently ensure ~/.claude/settings.json has the obsidian-memory
# statusLine entry pointing at the stable symlink. Shared by setup.sh
# (verbose, user-invoked) and session-start.sh (quiet, automatic) so the
# patch logic stays in one place.
#
# Usage: _ensure_statusline.sh <stable-symlink-path> [--quiet]
#
# Behavior:
#   - If statusline is disabled (OBSIDIAN_MEMORY_STATUSLINE_ENABLED=false), no-op.
#   - If settings.json is missing, create it as {}.
#   - If settings.json is invalid JSON, refuse to touch it.
#   - If .statusLine is absent, write it (with a timestamped backup).
#   - If .statusLine matches our stable command, no-op.
#   - If .statusLine is something else (user customization), leave it alone.
#
# Exit 0 on success or no-op; non-zero only on internal failures.

set -u

STABLE_STATUSLINE="${1:-}"
QUIET="${2:-}"

if [ -z "$STABLE_STATUSLINE" ]; then
  echo "usage: _ensure_statusline.sh <stable-symlink-path> [--quiet]" >&2
  exit 2
fi

_log() {
  [ "$QUIET" = "--quiet" ] && return 0
  echo "$@"
}

STATUSLINE_ENABLED="${OBSIDIAN_MEMORY_STATUSLINE_ENABLED:-true}"
if [ "$STATUSLINE_ENABLED" != "true" ]; then
  _log "[=] status line disabled via OBSIDIAN_MEMORY_STATUSLINE_ENABLED — skipping settings patch"
  exit 0
fi

if ! command -v jq >/dev/null 2>&1; then
  _log "[warn] jq not found — skipping status line patch"
  exit 0
fi

CLAUDE_SETTINGS="${HOME}/.claude/settings.json"
if [ ! -f "$CLAUDE_SETTINGS" ]; then
  mkdir -p "$(dirname "$CLAUDE_SETTINGS")"
  echo '{}' > "$CLAUDE_SETTINGS"
fi

# Validate JSON before touching it. Use `jq empty` (parses input, exits 0 on
# valid JSON) — NOT `jq -e empty`, which always exits non-zero because the
# `empty` filter produces no output and `-e` flags absence-of-output as failure.
if ! jq empty "$CLAUDE_SETTINGS" >/dev/null 2>&1; then
  _log "[warn] $CLAUDE_SETTINGS is not valid JSON — skipping status line patch."
  _log "       Fix the file (or delete it to start fresh) and re-run setup."
  exit 0
fi

EXISTING=$(jq -r '.statusLine.command // empty' "$CLAUDE_SETTINGS" 2>/dev/null || echo "")
EXPECTED="python3 \"$STABLE_STATUSLINE\""

if [ -z "$EXISTING" ]; then
  cp "$CLAUDE_SETTINGS" "${CLAUDE_SETTINGS}.bak.$(date +%Y%m%d%H%M%S)" 2>/dev/null || true
  if jq --arg cmd "$EXPECTED" \
       '.statusLine = {type: "command", command: $cmd}' \
       "$CLAUDE_SETTINGS" > "${CLAUDE_SETTINGS}.tmp" 2>/dev/null \
     && mv "${CLAUDE_SETTINGS}.tmp" "$CLAUDE_SETTINGS"; then
    _log "[+] enabled status line in $CLAUDE_SETTINGS"
  else
    _log "[warn] failed to write $CLAUDE_SETTINGS — left untouched (backup at ${CLAUDE_SETTINGS}.bak.*)"
    rm -f "${CLAUDE_SETTINGS}.tmp"
    exit 1
  fi
elif [ "$EXISTING" = "$EXPECTED" ]; then
  _log "[=] status line already enabled"
else
  _log "[=] status line already configured (left as-is). To use the plugin's:"
  _log "    set statusLine.command in $CLAUDE_SETTINGS to:"
  _log "      $EXPECTED"
fi
