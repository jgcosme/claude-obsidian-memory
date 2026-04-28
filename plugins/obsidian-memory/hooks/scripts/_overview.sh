#!/bin/bash
# Helper: print the auto-generated vault overview, using an mtime-invalidated
# cache so the gate doesn't re-walk the vault on every UserPromptSubmit.
#
# Usage: _overview.sh <vault_path> <project_name>
# Stdout: overview text (empty on failure)
# Exit:   0 always

set -u

VAULT="${1:-}"
PROJECT="${2:-}"
PLUGIN_ROOT="${CLAUDE_PLUGIN_ROOT:-}"
VAULT_PY="$PLUGIN_ROOT/scripts/_vault.py"

if [ -z "$VAULT" ] || [ ! -d "$VAULT" ] || [ ! -f "$VAULT_PY" ]; then
  exit 0
fi

CACHE_DIR="${MEMORY_OVERVIEW_CACHE_DIR:-/tmp/claude-memory-overview-cache}"
mkdir -p "$CACHE_DIR" 2>/dev/null || exit 0

KEY=$(printf '%s|%s' "$VAULT" "$PROJECT" | \
  python3 -c 'import hashlib,sys; print(hashlib.sha1(sys.stdin.buffer.read()).hexdigest())' 2>/dev/null)
[ -z "$KEY" ] && exit 0
CACHE="$CACHE_DIR/$KEY.txt"

# Cache hit: cache exists and no .md file is newer than it.
# `find -newer ... -print -quit` stops at the first match, so this is O(N) walk
# in the worst case but bails immediately on the first newer file.
if [ -s "$CACHE" ]; then
  newer=$(find "$VAULT" -name '*.md' -newer "$CACHE" -print -quit 2>/dev/null)
  if [ -z "$newer" ]; then
    cat "$CACHE"
    exit 0
  fi
fi

# Cache miss: regenerate atomically.
TMP="$CACHE.tmp.$$"
if python3 "$VAULT_PY" --vault "$VAULT" overview --project "$PROJECT" > "$TMP" 2>/dev/null && [ -s "$TMP" ]; then
  mv -f "$TMP" "$CACHE" 2>/dev/null && cat "$CACHE"
else
  rm -f "$TMP" 2>/dev/null
fi

exit 0
