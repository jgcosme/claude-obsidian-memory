#!/bin/bash
# obsidian-memory status — verifies install, config, vault, scripts, and
# recent activity. Read-only; safe to run any time.

set -u

CONFIG_FILE="${HOME}/.config/claude-memory/config.env"
if [ -f "$CONFIG_FILE" ]; then
  # shellcheck disable=SC1090
  ( set +u; . "$CONFIG_FILE" ) >/dev/null 2>&1 && . "$CONFIG_FILE" 2>/dev/null || true
fi

VAULT="${OBSIDIAN_VAULT_PATH:-$HOME/Documents/Obsidian Vault}"
PLUGIN_ROOT="${CLAUDE_PLUGIN_ROOT:-}"
GATE_ENABLED="${OBSIDIAN_MEMORY_GATE_ENABLED:-true}"
AUTOCOMMIT="${OBSIDIAN_MEMORY_AUTOCOMMIT:-true}"
AUTOPUSH="${OBSIDIAN_MEMORY_AUTOPUSH:-false}"
LOG_REVIEW="${MEMORY_REVIEW_LOG:-/tmp/claude-memory-review.log}"
LOG_GATE="${MEMORY_GATE_LOG:-/tmp/claude-memory-gate.log}"
CACHE_DIR="${MEMORY_OVERVIEW_CACHE_DIR:-/tmp/claude-memory-overview-cache}"

ISSUES=0
ok()   { echo "  [ok]   $*"; }
warn() { echo "  [warn] $*"; }
fail() { echo "  [FAIL] $*"; ISSUES=$((ISSUES+1)); }

echo "obsidian-memory status"
echo ""

echo "Config:"
if [ -f "$CONFIG_FILE" ]; then ok "config: $CONFIG_FILE"; else warn "config missing at $CONFIG_FILE (using defaults)"; fi
echo "  • vault:       $VAULT"
echo "  • gate:        $GATE_ENABLED"
echo "  • autocommit:  $AUTOCOMMIT"
echo "  • autopush:    $AUTOPUSH"
echo ""

echo "Prerequisites:"
for t in jq python3; do
  if command -v "$t" >/dev/null 2>&1; then ok "$t"; else fail "$t (required)"; fi
done
if command -v python3 >/dev/null 2>&1; then
  PYOK=$(python3 -c 'import sys; print(1 if sys.version_info >= (3,9) else 0)' 2>/dev/null || echo 0)
  PYV=$(python3 -c 'import sys; print(f"{sys.version_info.major}.{sys.version_info.minor}")' 2>/dev/null || echo "?")
  if [ "$PYOK" = "1" ]; then ok "python3 $PYV"; else fail "python3 $PYV (need >= 3.9)"; fi
fi
for t in git flock; do
  if command -v "$t" >/dev/null 2>&1; then ok "$t"; else warn "$t (optional)"; fi
done
echo ""

echo "Vault:"
if [ -d "$VAULT" ]; then
  ok "directory exists"
  [ -f "$VAULT/README.md" ] && ok "README.md present" || warn "no README.md (re-run setup.sh)"
  if [ -d "$VAULT/.git" ]; then
    ok "git initialized"
    if [ "$AUTOPUSH" = "true" ]; then
      remote=$(git -C "$VAULT" remote get-url origin 2>/dev/null || echo "")
      [ -n "$remote" ] && ok "git remote: $remote" || warn "autopush=true but no 'origin' remote"
    fi
  else
    warn "not a git repo (auto-commit will no-op)"
  fi
else
  fail "vault not found at $VAULT — run setup.sh"
fi
echo ""

echo "Plugin scripts:"
if [ -z "$PLUGIN_ROOT" ]; then
  warn "CLAUDE_PLUGIN_ROOT unset (running outside Claude Code) — script presence not checked"
else
  for s in scripts/_vault.py scripts/audit.py scripts/setup.sh \
           hooks/scripts/session-start.sh hooks/scripts/session-end.sh \
           hooks/scripts/user-prompt-submit.sh hooks/scripts/_overview.sh; do
    if [ -f "$PLUGIN_ROOT/$s" ]; then ok "$s"; else fail "$s missing"; fi
  done
fi
echo ""

echo "Search smoke test:"
if [ -d "$VAULT" ] && [ -n "$PLUGIN_ROOT" ] && [ -f "$PLUGIN_ROOT/scripts/_vault.py" ]; then
  count=$(python3 "$PLUGIN_ROOT/scripts/_vault.py" --vault "$VAULT" search --json 2>/dev/null | \
          python3 -c 'import json,sys; print(len(json.load(sys.stdin)))' 2>/dev/null || echo "?")
  if [ "$count" = "?" ]; then
    fail "_vault.py search failed"
  else
    ok "_vault.py search returned $count notes"
  fi
else
  warn "skipped (vault or _vault.py not available)"
fi
echo ""

echo "Overview cache:"
if [ -d "$CACHE_DIR" ]; then
  files=$(find "$CACHE_DIR" -name '*.txt' 2>/dev/null | wc -l | tr -d ' ')
  ok "$CACHE_DIR ($files cached)"
else
  warn "$CACHE_DIR not yet populated (created on next SessionStart)"
fi
echo ""

echo "Recent activity:"
if [ -f "$LOG_REVIEW" ]; then
  echo "  • review: $(tail -n 1 "$LOG_REVIEW")"
else
  echo "  • review: no entries yet"
fi
if [ -f "$LOG_GATE" ]; then
  echo "  • gate:   $(tail -n 1 "$LOG_GATE")"
else
  echo "  • gate:   no entries yet"
fi
echo ""

if [ "$ISSUES" -gt 0 ]; then
  echo "$ISSUES issue(s) found — fix and re-run /obsidian-memory:status."
  exit 1
fi
echo "All checks passed."
exit 0
