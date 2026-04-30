#!/bin/bash
# Helper: print the auto-generated vault overview, using an mtime-invalidated
# cache so the gate doesn't re-walk the vault on every UserPromptSubmit.
#
# Usage: _overview.sh <vault_path> <project_name> [<repo_vault_path>]
# Stdout: overview text (empty on failure)
# Exit:   0 always

set -u

VAULT="${1:-}"
PROJECT="${2:-}"
REPO_VAULT="${3:-}"
PLUGIN_ROOT="${CLAUDE_PLUGIN_ROOT:-}"
VAULT_PY="$PLUGIN_ROOT/scripts/_vault.py"

if [ -z "$VAULT" ] || [ ! -d "$VAULT" ] || [ ! -f "$VAULT_PY" ]; then
  exit 0
fi

CACHE_DIR="${MEMORY_OVERVIEW_CACHE_DIR:-/tmp/claude-memory-overview-cache}"
mkdir -p "$CACHE_DIR" 2>/dev/null || exit 0

# Cache key includes the repo-vault path so different projects get distinct
# cache entries. Empty repo-vault still hashes uniquely from the vault+project.
KEY=$(printf '%s|%s|%s' "$VAULT" "$PROJECT" "$REPO_VAULT" | \
  python3 -c 'import hashlib,sys; print(hashlib.sha1(sys.stdin.buffer.read()).hexdigest())' 2>/dev/null)
[ -z "$KEY" ] && exit 0
CACHE="$CACHE_DIR/$KEY.txt"

# Cache hit: cache exists and no .md file in either corpus is newer than it.
# `find -newer ... -print -quit` stops at the first match, so this is O(N) walk
# in the worst case but bails immediately on the first newer file.
if [ -s "$CACHE" ]; then
  newer=$(find "$VAULT" -name '*.md' -newer "$CACHE" -print -quit 2>/dev/null)
  if [ -z "$newer" ] && [ -n "$REPO_VAULT" ] && [ -d "$REPO_VAULT" ]; then
    newer=$(find "$REPO_VAULT" -name '*.md' -newer "$CACHE" -print -quit 2>/dev/null)
  fi
  if [ -z "$newer" ]; then
    cat "$CACHE"
    exit 0
  fi
fi

# Cache miss: regenerate atomically.
TMP="$CACHE.tmp.$$"
if [ -n "$REPO_VAULT" ] && [ -d "$REPO_VAULT" ]; then
  python3 "$VAULT_PY" --vault "$VAULT" overview --project "$PROJECT" --repo-vault "$REPO_VAULT" > "$TMP" 2>/dev/null
else
  python3 "$VAULT_PY" --vault "$VAULT" overview --project "$PROJECT" > "$TMP" 2>/dev/null
fi

if [ -s "$TMP" ]; then
  mv -f "$TMP" "$CACHE" 2>/dev/null && cat "$CACHE"
else
  rm -f "$TMP" 2>/dev/null
fi

exit 0
