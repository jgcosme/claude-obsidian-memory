#!/bin/bash
# Idempotently ensure:
#   1. A stable wrapper script exists at ~/.config/obsidian-memory/statusline-gate.sh
#   2. ~/.claude/settings.json's statusLine.command invokes that wrapper
#
# The wrapper is a thin gate that exits silently when the plugin is no longer
# in enabledPlugins. Putting the gate in a stable, version-independent
# wrapper (instead of statusline.py itself) means uninstall self-disables
# even when the symlink to statusline.py is stranded at a pre-fix version
# (which happens when /plugin update + /plugin uninstall run in the same
# session — SessionStart fires once per session and can't refresh after
# uninstall).
#
# Shared by setup.sh (verbose, user-invoked) and session-start.sh (quiet,
# automatic).
#
# Usage: _ensure_statusline.sh <stable-symlink-path> [--quiet]
#
# Behavior:
#   - If statusline is disabled (OBSIDIAN_MEMORY_STATUSLINE_ENABLED=false), no-op.
#   - Always (re)writes the wrapper script — it's tiny and version-independent.
#   - If settings.json is missing, create it as {}.
#   - If settings.json is invalid JSON, refuse to touch it.
#   - If .statusLine is absent, write it pointing at the wrapper.
#   - If .statusLine matches the new wrapper-based command, no-op.
#   - If .statusLine matches the old direct-to-statusline.py command (from
#     v1.15.2 and earlier), migrate it to the wrapper-based command.
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

CONFIG_DIR="$(dirname "$STABLE_STATUSLINE")"
WRAPPER="${CONFIG_DIR}/statusline-gate.sh"

# (Re)write the stable wrapper. It's a copy (not a symlink) so it survives
# plugin uninstall — that's the whole point. Always overwrite so future
# plugin versions can refine the wrapper logic.
mkdir -p "$CONFIG_DIR"
cat > "$WRAPPER" <<WRAPPER_EOF
#!/bin/bash
# obsidian-memory statusline gate. Stable wrapper invoked by Claude Code's
# statusLine.command. Exits silently when the plugin is uninstalled so the
# orphan statusLine entry in ~/.claude/settings.json (which Claude Code
# can't auto-clean — no PluginUninstall hook) becomes a dormant no-op.
#
# Reads stdin from Claude Code (passed through to statusline.py) and execs
# the actual rendering script via its stable symlink.
#
# Written by obsidian-memory's setup.sh / SessionStart hook (idempotent).

CLAUDE_SETTINGS="\${HOME}/.claude/settings.json"
TARGET="${STABLE_STATUSLINE}"

# Self-disable if jq is missing (can't check enabledPlugins) — preserves
# setups where jq isn't available.
command -v jq >/dev/null 2>&1 || exit 0
[ -f "\$CLAUDE_SETTINGS" ] || exit 0

# Any obsidian-memory@<marketplace> entry enabled?
INSTALLED=\$(jq -r '
  (.enabledPlugins // {})
  | to_entries
  | map(select(.key | startswith("obsidian-memory@")))
  | map(select(.value == true))
  | length
' "\$CLAUDE_SETTINGS" 2>/dev/null)

[ "\$INSTALLED" = "0" ] || [ -z "\$INSTALLED" ] && exit 0

# Symlink target gone (plugin cache purged) — silent.
[ -e "\$TARGET" ] || exit 0

exec python3 "\$TARGET"
WRAPPER_EOF
chmod +x "$WRAPPER"

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
EXPECTED="bash \"$WRAPPER\""
LEGACY="python3 \"$STABLE_STATUSLINE\""

# Decide action:
#   absent → write wrapper-based command
#   matches EXPECTED → no-op
#   matches LEGACY (v1.15.2 and earlier) → migrate to wrapper-based command
#   anything else → user customization, leave alone
ACTION=""
if [ -z "$EXISTING" ]; then
  ACTION="install"
elif [ "$EXISTING" = "$EXPECTED" ]; then
  ACTION="noop"
elif [ "$EXISTING" = "$LEGACY" ]; then
  ACTION="migrate"
else
  ACTION="custom"
fi

case "$ACTION" in
  install|migrate)
    cp "$CLAUDE_SETTINGS" "${CLAUDE_SETTINGS}.bak.$(date +%Y%m%d%H%M%S)" 2>/dev/null || true
    if jq --arg cmd "$EXPECTED" \
         '.statusLine = {type: "command", command: $cmd}' \
         "$CLAUDE_SETTINGS" > "${CLAUDE_SETTINGS}.tmp" 2>/dev/null \
       && mv "${CLAUDE_SETTINGS}.tmp" "$CLAUDE_SETTINGS"; then
      if [ "$ACTION" = "install" ]; then
        _log "[+] enabled status line in $CLAUDE_SETTINGS"
      else
        _log "[+] migrated status line to wrapper-based command in $CLAUDE_SETTINGS"
      fi
    else
      _log "[warn] failed to write $CLAUDE_SETTINGS — left untouched (backup at ${CLAUDE_SETTINGS}.bak.*)"
      rm -f "${CLAUDE_SETTINGS}.tmp"
      exit 1
    fi
    ;;
  noop)
    _log "[=] status line already enabled"
    ;;
  custom)
    _log "[=] status line already configured (left as-is). To use the plugin's:"
    _log "    set statusLine.command in $CLAUDE_SETTINGS to:"
    _log "      $EXPECTED"
    ;;
esac
