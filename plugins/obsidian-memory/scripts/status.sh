#!/bin/bash
# obsidian-memory status — verifies install, config, vault, scripts, and
# recent activity. Read-only; safe to run any time.

set -u

CONFIG_FILE="${HOME}/.config/obsidian-memory/config.env"
if [ -f "$CONFIG_FILE" ]; then
  # shellcheck disable=SC1090
  ( set +u; . "$CONFIG_FILE" ) >/dev/null 2>&1 && . "$CONFIG_FILE" 2>/dev/null || true
fi

VAULT="${OBSIDIAN_VAULT_PATH:-$HOME/Documents/Obsidian Memory}"
# Self-locate: prefer $CLAUDE_PLUGIN_ROOT, otherwise derive from this script's path.
# This makes status.sh work whether invoked through a slash command (env may or
# may not be set) or directly from a shell.
if [ -n "${CLAUDE_PLUGIN_ROOT:-}" ]; then
  PLUGIN_ROOT="$CLAUDE_PLUGIN_ROOT"
else
  PLUGIN_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
fi
GATE_ENABLED="${OBSIDIAN_MEMORY_GATE_ENABLED:-true}"
REVIEW_ENABLED="${OBSIDIAN_MEMORY_REVIEW_ENABLED:-true}"
BOOTSTRAP_OVERVIEW="${OBSIDIAN_MEMORY_BOOTSTRAP_OVERVIEW:-true}"
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
echo "  • vault:              $VAULT"
echo "  • gate:               $GATE_ENABLED"
echo "  • review:             $REVIEW_ENABLED"
echo "  • bootstrap-overview: $BOOTSTRAP_OVERVIEW"
echo "  • autocommit:         $AUTOCOMMIT"
echo "  • autopush:           $AUTOPUSH"
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

echo "Project-vaults:"
PROJECTS_PY="$PLUGIN_ROOT/scripts/_projects.py"
if [ -f "$PROJECTS_PY" ]; then
  PROJECTS_JSON=$(python3 "$PROJECTS_PY" list --json 2>/dev/null || echo "[]")
  CWD_ROOT=$(git -C "${CLAUDE_PROJECT_DIR:-$PWD}" rev-parse --show-toplevel 2>/dev/null || echo "")

  # Current-cwd one-liner — quickly answers "is this session opted in?"
  if [ -z "$CWD_ROOT" ]; then
    echo "  • current cwd: (not a git repo — project-vault not applicable)"
  else
    CWD_STATUS=$(python3 "$PROJECTS_PY" lookup "$CWD_ROOT" 2>/dev/null || echo "not_registered")
    case "$CWD_STATUS" in
      enabled)
        CWD_PROJECT=$(python3 "$PROJECTS_PY" lookup "$CWD_ROOT" --json 2>/dev/null | \
                      jq -r '.project // ""' 2>/dev/null || echo "")
        echo "  • current cwd: enabled ($CWD_PROJECT)"
        ;;
      disabled)
        echo "  • current cwd: disabled (declined registration)"
        ;;
      *)
        echo "  • current cwd: not registered (SessionStart will offer to register)"
        ;;
    esac
  fi

  # Registered list — empty case + enumerated case
  COUNT=$(echo "$PROJECTS_JSON" | jq 'length' 2>/dev/null || echo 0)
  if [ "${COUNT:-0}" = "0" ]; then
    echo "  • no projects registered yet"
  else
    ENABLED=$(echo "$PROJECTS_JSON" | jq '[.[] | select(.enabled)] | length' 2>/dev/null || echo 0)
    echo "  • registered: $COUNT total · $ENABLED enabled"
    echo "$PROJECTS_JSON" | jq -r --arg cwd "$CWD_ROOT" '.[] |
      "    \(if .enabled then "[on] " else "[off]" end) \(.project)  \(.path)\(if .path == $cwd then "  ← current" else "" end)"' 2>/dev/null
  fi
else
  warn "_projects.py missing (federation not available)"
fi
echo ""

echo "Plugin scripts (root: $PLUGIN_ROOT):"
for s in scripts/_vault.py scripts/audit.py scripts/setup.sh \
         hooks/scripts/session-start.sh hooks/scripts/session-end.sh \
         hooks/scripts/user-prompt-submit.sh hooks/scripts/_overview.sh; do
  if [ -f "$PLUGIN_ROOT/$s" ]; then ok "$s"; else fail "$s missing"; fi
done
echo ""

echo "Search smoke test:"
if [ -d "$VAULT" ] && [ -f "$PLUGIN_ROOT/scripts/_vault.py" ]; then
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
