#!/bin/bash
# SessionEnd hook: review transcript, write journal entry + proactive memory updates.
# Backgrounds a `claude -p` subprocess so it doesn't block session shutdown.

set -u

# ---------------------------------------------------------------------------
# Recursion guard: the review subprocess and the gate subprocess both spawn
# `claude -p`, which fires its own SessionEnd on shutdown. Without this guard
# we'd loop forever.
# ---------------------------------------------------------------------------
if [ -n "${CLAUDE_MEMORY_REVIEW:-}" ] || [ -n "${CLAUDE_MEMORY_GATE:-}" ]; then
  exit 0
fi

# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------
CONFIG_FILE="${HOME}/.config/claude-memory/config.env"
if [ -f "$CONFIG_FILE" ]; then
  # shellcheck disable=SC1090
  . "$CONFIG_FILE"
fi

VAULT="${OBSIDIAN_VAULT_PATH:-$HOME/Documents/Obsidian Vault}"
LOG="${MEMORY_REVIEW_LOG:-/tmp/claude-memory-review.log}"
LOG_MAX_BYTES="${MEMORY_LOG_MAX_BYTES:-1048576}"  # 1 MB default
AUTOCOMMIT="${OBSIDIAN_MEMORY_AUTOCOMMIT:-true}"
AUTOPUSH="${OBSIDIAN_MEMORY_AUTOPUSH:-false}"

# Rotate log if it's grown too large (keep one previous as .log.1)
if [ -f "$LOG" ]; then
  bytes=$(wc -c < "$LOG" 2>/dev/null | tr -d ' ' || echo 0)
  if [ "${bytes:-0}" -gt "$LOG_MAX_BYTES" ]; then
    mv -f "$LOG" "${LOG}.1" 2>/dev/null || true
  fi
fi

# Locate `claude` CLI
if [ -n "${CLAUDE_BIN:-}" ] && [ -x "$CLAUDE_BIN" ]; then
  : # use override
elif command -v claude >/dev/null 2>&1; then
  CLAUDE_BIN="$(command -v claude)"
else
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] skipped: \`claude\` CLI not found in PATH" >> "$LOG"
  exit 0
fi

# ---------------------------------------------------------------------------
# Recursion guard: if this IS itself a memory-review subprocess, exit immediately.
# ---------------------------------------------------------------------------
if [ -n "${CLAUDE_MEMORY_REVIEW:-}" ]; then
  exit 0
fi

# Read event payload from stdin
PAYLOAD=$(cat)
TRANSCRIPT=$(echo "$PAYLOAD" | jq -r '.transcript_path // empty' 2>/dev/null || echo "")

PROJECT_DIR="${CLAUDE_PROJECT_DIR:-$PWD}"
PROJECT_NAME=$(basename "$PROJECT_DIR")
TODAY=$(date +%Y-%m-%d)
NOW=$(date +%H:%M)

# Defensive: skip if no transcript available
if [ -z "$TRANSCRIPT" ] || [ ! -f "$TRANSCRIPT" ]; then
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] skipped: no transcript at '$TRANSCRIPT'" >> "$LOG"
  exit 0
fi

# Skip if vault missing (plugin not yet set up)
if [ ! -d "$VAULT" ]; then
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] skipped: vault not found at '$VAULT'" >> "$LOG"
  exit 0
fi

# Whether to run the journal/review step. The user is asked at SessionStart
# before scaffolding Projects/<name>/, so its absence here means they declined
# (or never set it up). In that case, skip the review but still autocommit any
# General/Tools writes the session produced.
RUN_REVIEW=true
if [ ! -d "$VAULT/Projects/$PROJECT_NAME" ]; then
  RUN_REVIEW=false
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] no Projects/$PROJECT_NAME/ folder; skipping review, will still commit dirty vault state" >> "$LOG"
fi

# ---------------------------------------------------------------------------
# Build the review prompt. `read -d ''` captures a quoted heredoc into a
# variable without bash's $() command-substitution parser scanning the body
# for unbalanced quotes (apostrophes in the prompt text would otherwise trip
# it). Placeholders are substituted with sed afterwards.
# ---------------------------------------------------------------------------
PLUGIN_ROOT_PATH="${CLAUDE_PLUGIN_ROOT:-}"
read -r -d '' REVIEW_PROMPT_TMPL <<'PROMPT_EOF' || true
End-of-session memory review.

Vault:       __VAULT__
Transcript:  __TRANSCRIPT__
Project:     __PROJECT_NAME__ (at __PROJECT_DIR__)
Date / time: __TODAY__ __NOW__

1. JOURNAL — always.
   Path: Projects/__PROJECT_NAME__/Journal/__TODAY__.md
   New file: frontmatter (type=journal, description=<one-line day summary>, project=__PROJECT_NAME__, created=__TODAY__) + "## Session __NOW__" + 3-6 bullets covering work, decisions, learnings.
   Existing file: append a new "## Session __NOW__" section. Do not edit prior session bodies. You MAY (and should) rewrite the frontmatter `description` to cover the full day across all sessions now present — the original was a one-line day summary written when only the first session existed, and goes stale as the day grows.

2. PROACTIVE NOTES — write when ALL hold:
   - Significant: correction, decision, validated approach, novel fact, "remember this".
   - Useful in future sessions.
   - Not already covered. Verify with `python3 __PLUGIN_ROOT__/scripts/_vault.py search --type <t> --keywords <k> --json`; read matches; extend a near-duplicate rather than creating a new note.

   Route each candidate (mutually exclusive):

   A. Personal / cross-project — substantive vault note:
      style preference  → General/Preferences/<slug>.md
      external system   → General/References/<slug>.md
      tool reference    → Tools/<slug>.md
      person            → General/People/<slug>.md

   B. Project-scoped (decision, gotcha, how-X-works) — classify first:
      Q1. Team-relevant? (other engineers on the project benefit)
      Q2. Project at __PROJECT_DIR__ has internal docs? (docs/, ADR folders, mkdocs/sphinx, CONTRIBUTING)

      Q1=yes AND Q2=yes → reflect upstream:
        i.   Destination inside __PROJECT_DIR__: extend an existing doc when one fits, else add a new file under the docs tree following its conventions.
        ii.  Allowed paths: docs/ tree, ADR folders, *.md inside docs/. Never source, configs, CI, or manifests.
        iii. Run `git -C __PROJECT_DIR__ status --porcelain -- <target>`. Non-empty → SKIP the project write (would stomp WIP); append a one-liner to the journal noting the deferral (file, suggested location, reason).
        iv.  Else write the doc edit as uncommitted working-tree changes. Do not git add / commit / push / branch.
        v.   Also write a thin-pointer vault note (1-3 sentence summary + relative path of the project doc) at Projects/__PROJECT_NAME__/{Decisions,Learnings}/<slug>.md.
        vi.  List each project-repo write under "## Project repo writes" in the output.

      Otherwise → substantive vault note at Projects/__PROJECT_NAME__/{Decisions,Learnings}/<slug>.md.

   Frontmatter on every new vault note: type, description, created (+ project for project-scoped). type ∈ {preference, reference, decision, learning, tool, people}.

3. MODIFY existing notes only on explicit user correction in the transcript. Smallest edit. Inferred staleness → flag in output, do not edit. Otherwise the only modification allowed is appending to today's journal.

   Whenever you modify or extend a non-journal note (step 2 near-duplicate extension or step 3 correction), check its frontmatter `description` against the new body. If the one-line summary no longer fits, rewrite it. The auto-overview shown at SessionStart is built from these descriptions — stale ones mislead future sessions.

4. INTEGRITY (deltas from steps 1-3 only, plus journal-linked notes):
   - Frontmatter complete (type, description, created; + project under Projects/).
   - Every [[wikilink]] resolves: path-qualified to a file; bare matches a vault basename.
   - For each non-journal vault note referenced in today's journal entry written in step 1 (any [[wikilink]] or vault-relative path mentioned in the bullets you just wrote), re-read it and judge whether its frontmatter `description` still summarizes the body. If drift, rewrite `description` (smallest edit) and add the path to the delta list.
   Auto-fix unambiguous issues. List ambiguous ones under "## Integrity flags". Do not scan files outside the deltas + journal-linked set.

OUTPUT: list of vault paths created/appended, then "## Project repo writes" (paths in __PROJECT_DIR__), then "## Integrity flags". No narrative.
PROMPT_EOF

REVIEW_PROMPT=$(printf '%s' "$REVIEW_PROMPT_TMPL" | sed \
  -e "s|__VAULT__|${VAULT}|g" \
  -e "s|__TRANSCRIPT__|${TRANSCRIPT}|g" \
  -e "s|__PROJECT_NAME__|${PROJECT_NAME}|g" \
  -e "s|__PROJECT_DIR__|${PROJECT_DIR}|g" \
  -e "s|__TODAY__|${TODAY}|g" \
  -e "s|__NOW__|${NOW}|g" \
  -e "s|__PLUGIN_ROOT__|${PLUGIN_ROOT_PATH}|g")

export REVIEW_PROMPT

# ---------------------------------------------------------------------------
# Background the review so the hook returns immediately. After review, autocommit
# any vault changes (no push by default — controlled by AUTOPUSH).
# ---------------------------------------------------------------------------
nohup bash -c '
  ts() { date "+%Y-%m-%d %H:%M:%S"; }
  PROJECT_NAME='"$(printf %q "$PROJECT_NAME")"'
  TRANSCRIPT='"$(printf %q "$TRANSCRIPT")"'
  VAULT='"$(printf %q "$VAULT")"'
  LOG='"$(printf %q "$LOG")"'
  CLAUDE_BIN='"$(printf %q "$CLAUDE_BIN")"'
  RUN_REVIEW='"$RUN_REVIEW"'
  AUTOCOMMIT='"$AUTOCOMMIT"'
  AUTOPUSH='"$AUTOPUSH"'

  if [ "$RUN_REVIEW" = "true" ]; then
    echo "[$(ts)] starting review for project=$PROJECT_NAME transcript=$TRANSCRIPT" >> "$LOG"
    # Note: --bare disables OAuth/keychain auth (see `claude --help`). We rely
    # on the recursion-guard env vars (CLAUDE_MEMORY_REVIEW=1) plus the
    # early-exit checks at the top of the hook scripts to prevent recursion.
    CLAUDE_MEMORY_REVIEW=1 "$CLAUDE_BIN" -p "$REVIEW_PROMPT" \
      --tools "Read,Write,Edit,Bash" \
      >> "$LOG" 2>&1
    echo "[$(ts)] review complete (exit=$?)" >> "$LOG"
  fi

  if [ "$AUTOCOMMIT" = "true" ] && [ -d "$VAULT/.git" ]; then
    cd "$VAULT" || exit 0
    # Serialize git ops across overlapping sessions
    LOCK_FD=9
    LOCK_FILE="$VAULT/.git/.claude-memory.lock"
    exec 9>"$LOCK_FILE" 2>/dev/null
    if command -v flock >/dev/null 2>&1; then
      flock -w 30 9 || { echo "[$(ts)] lock timeout, skipping commit" >> "$LOG"; exit 0; }
    fi
    if [ -n "$(git status --porcelain)" ]; then
      NOW_TS=$(date "+%Y-%m-%d %H:%M")
      git add -A
      if git commit -m "session writes $NOW_TS ($PROJECT_NAME)" >> "$LOG" 2>&1; then
        echo "[$(ts)] vault auto-committed" >> "$LOG"
        if [ "$AUTOPUSH" = "true" ]; then
          if git push >> "$LOG" 2>&1; then
            echo "[$(ts)] vault auto-pushed" >> "$LOG"
          else
            echo "[$(ts)] vault push failed" >> "$LOG"
          fi
        fi
      else
        echo "[$(ts)] vault commit failed" >> "$LOG"
      fi
    else
      echo "[$(ts)] vault clean — nothing to commit" >> "$LOG"
    fi
  fi
' >/dev/null 2>&1 &

exit 0
