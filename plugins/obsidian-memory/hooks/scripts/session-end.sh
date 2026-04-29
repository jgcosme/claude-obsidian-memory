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
SESSION_ID=$(echo "$PAYLOAD" | jq -r '.session_id // empty' 2>/dev/null || echo "")

PROJECT_DIR="${CLAUDE_PROJECT_DIR:-$PWD}"
PROJECT_NAME=$(basename "$PROJECT_DIR")
TODAY=$(date +%Y-%m-%d)
NOW=$(date +%H:%M)

# Load HEAD SHAs recorded at SessionStart so the review can scope diff-based
# checks (pointer reconciliation + backlink reconciliation) to what actually
# changed during this session. Empty values are fine — the review prompt
# falls back to working-tree-only diff in that case.
SESSION_STATE_DIR="${MEMORY_SESSION_STATE_DIR:-/tmp/claude-memory-session}"
PROJECT_HEAD=""
VAULT_HEAD=""
if [ -n "$SESSION_ID" ]; then
  SAFE_SID=$(echo "$SESSION_ID" | tr -c 'A-Za-z0-9._-' '_')
  PROJECT_HEAD_FILE="$SESSION_STATE_DIR/$SAFE_SID.project_head"
  VAULT_HEAD_FILE="$SESSION_STATE_DIR/$SAFE_SID.vault_head"
  [ -f "$PROJECT_HEAD_FILE" ] && PROJECT_HEAD=$(cat "$PROJECT_HEAD_FILE" 2>/dev/null || true)
  [ -f "$VAULT_HEAD_FILE" ]   && VAULT_HEAD=$(cat "$VAULT_HEAD_FILE" 2>/dev/null || true)
fi

# Defensive: skip if no transcript available
if [ -z "$TRANSCRIPT" ] || [ ! -f "$TRANSCRIPT" ]; then
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] skipped: no transcript at '$TRANSCRIPT'" >> "$LOG"
  exit 0
fi

# Slim the transcript for the reviewer: strip tool_use / tool_result blocks
# and keep only user messages + assistant text + a one-line tool-use summary
# per assistant turn. Cuts review token cost ~95% on real sessions because
# tool_result bodies (file reads, command output, search results) dominate
# transcript size and aren't signal for save-worthy detection.
# OBSIDIAN_MEMORY_SLIM_TRANSCRIPT=false reverts to the raw transcript.
PLUGIN_ROOT_PATH="${CLAUDE_PLUGIN_ROOT:-}"
SLIM_HELPER="$PLUGIN_ROOT_PATH/scripts/_slim_transcript.py"
SLIM_TRANSCRIPT=""
if [ "${OBSIDIAN_MEMORY_SLIM_TRANSCRIPT:-true}" = "true" ] && [ -f "$SLIM_HELPER" ]; then
  SLIM_TRANSCRIPT=$(mktemp -t claude-memory-slim.XXXXXX 2>/dev/null || echo "")
  if [ -n "$SLIM_TRANSCRIPT" ]; then
    if python3 "$SLIM_HELPER" "$TRANSCRIPT" -o "$SLIM_TRANSCRIPT" 2>>"$LOG"; then
      bytes_in=$(wc -c < "$TRANSCRIPT" 2>/dev/null | tr -d ' ' || echo 0)
      bytes_out=$(wc -c < "$SLIM_TRANSCRIPT" 2>/dev/null | tr -d ' ' || echo 0)
      echo "[$(date '+%Y-%m-%d %H:%M:%S')] slimmed transcript: ${bytes_in} → ${bytes_out} bytes" >> "$LOG"
      TRANSCRIPT="$SLIM_TRANSCRIPT"
    else
      echo "[$(date '+%Y-%m-%d %H:%M:%S')] slim helper failed; falling back to raw transcript" >> "$LOG"
      rm -f "$SLIM_TRANSCRIPT"
      SLIM_TRANSCRIPT=""
    fi
  fi
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
if [ "${OBSIDIAN_MEMORY_REVIEW_ENABLED:-true}" != "true" ]; then
  RUN_REVIEW=false
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] OBSIDIAN_MEMORY_REVIEW_ENABLED=false; skipping review, will still commit dirty vault state" >> "$LOG"
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

Vault:        __VAULT__
Transcript:   __TRANSCRIPT__
Project:      __PROJECT_NAME__ (at __PROJECT_DIR__)
Date / time:  __TODAY__ __NOW__
Project HEAD at session start: __PROJECT_HEAD__   (empty = unknown; fall back to working-tree diff vs HEAD)
Vault HEAD at session start:   __VAULT_HEAD__     (empty = unknown; fall back to working-tree diff vs HEAD)

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
        v.   Also write a thin-pointer vault note (1-3 sentence summary + relative path of the project doc) at Projects/__PROJECT_NAME__/{Decisions,Learnings}/<slug>.md. Include `source: <repo-relative path>` in the frontmatter — the audit and reconciliation steps key off this field.
        vi.  List each project-repo write under "## Project repo writes" in the output.

      Otherwise → substantive vault note at Projects/__PROJECT_NAME__/{Decisions,Learnings}/<slug>.md.

   Frontmatter on every new vault note: type, description, created (+ project for project-scoped; + source for thin-pointer notes mirroring a project doc). type ∈ {preference, reference, decision, learning, tool, people}.

3. MODIFY existing notes only on explicit user correction in the transcript. Smallest edit. Inferred staleness → flag in output, do not edit. Otherwise the only modification allowed is appending to today's journal.

   Whenever you modify or extend a non-journal note (step 2 near-duplicate extension or step 3 correction), check its frontmatter `description` against the new body. If the one-line summary no longer fits, rewrite it. The auto-overview shown at SessionStart is built from these descriptions — stale ones mislead future sessions.

4. INTEGRITY — operates on:
   (a) vault notes touched in steps 1-3 above
   (b) non-journal vault notes referenced (wikilinks/paths) in today's journal entry
   (c) project repo *.md files changed during this session
   (d) vault *.md files changed since the last commit

   To enumerate (c) and (d):
     # (c) Project repo doc changes — committed-since-session-start + working tree + untracked
     if [ -n "__PROJECT_HEAD__" ]; then
       git -C __PROJECT_DIR__ diff --name-status -M __PROJECT_HEAD__ HEAD -- '*.md'
     fi
     git -C __PROJECT_DIR__ diff --name-status -M HEAD -- '*.md'
     git -C __PROJECT_DIR__ ls-files --others --exclude-standard -- '*.md'
     # Filter out boilerplate: .github/, .cursor/, .vscode/, any top-level dotfile dir,
     # LICENSE*.md, CODE_OF_CONDUCT.md, SECURITY.md, CHANGELOG.md, PR/ISSUE templates.

     # (d) Vault doc changes — same shape, run inside __VAULT__
     python3 __PLUGIN_ROOT__/scripts/_vault.py vault-changes \
       $( [ -n "__VAULT_HEAD__" ] && echo --base-sha __VAULT_HEAD__ )

   Per-source checks:

   - (a) + (b): frontmatter completeness (type, description, created; + project under Projects/); every [[wikilink]] resolves; description-vs-body drift (rewrite description on drift, smallest edit).

   - (c) Project repo doc changes — POINTER RECONCILIATION:
     Build a pointer index by scanning Projects/__PROJECT_NAME__/ for notes whose frontmatter has `source: <path>`. For each changed project doc:
       * MODIFIED + pointer exists → re-read the source and the pointer; if the pointer's body or description no longer summarizes the source, rewrite the pointer (smallest edit; description first, body only if structure shifted). SKIP if the source file is currently dirty in `git -C __PROJECT_DIR__ status --porcelain -- <path>` — defer to next session and add a note under "## Integrity flags".
       * ADDED + no pointer → list under "## New pointer suggestions" with the proposed category (Decisions / Learnings / Research / References) based on the doc's content. DO NOT auto-create.
       * DELETED + pointer exists → list under "## Stale pointers (source deleted)". DO NOT auto-remove the pointer — deletion may have been accidental.
       * RENAMED (old → new) + pointer exists → rewrite the pointer's `source:` frontmatter to the new path (smallest edit). Update its description if the source's content shifted. List under "## Pointer rewrites".

   - (d) Vault file changes — BACKLINK RECONCILIATION:
       * RENAMED (old → new) → use `python3 __PLUGIN_ROOT__/scripts/_vault.py incoming-wikilinks --target <old>` to find every note linking to the OLD path. Auto-rewrite each occurrence to the NEW path (smallest edit; preserve any |alias text). For bare basename links, prefer the new basename. List rewrites under "## Backlink rewrites".
       * DELETED → use `incoming-wikilinks --target <deleted-path>` to find broken backlinks. List under "## Broken backlinks (target deleted)". DO NOT auto-fix — deletion may be intentional or may be a rename the diff couldn't infer.
       * ADDED / MODIFIED → no backlink action needed.

   Auto-fix unambiguous issues (description drift, source-rename, backlink-rename). List ambiguous and non-fixable items in their dedicated sections.

OUTPUT sections (in order, omit when empty):
  ## Vault writes              (paths created/appended)
  ## Project repo writes       (paths in __PROJECT_DIR__)
  ## Pointer rewrites          (vault pointer notes whose source was renamed)
  ## Backlink rewrites         (vault notes whose [[wikilinks]] were updated for renames)
  ## New pointer suggestions   (added repo docs without a pointer)
  ## Stale pointers (source deleted)
  ## Broken backlinks (target deleted)
  ## Integrity flags           (everything ambiguous or deferred)

No narrative outside these sections.
PROMPT_EOF

REVIEW_PROMPT=$(printf '%s' "$REVIEW_PROMPT_TMPL" | sed \
  -e "s|__VAULT__|${VAULT}|g" \
  -e "s|__TRANSCRIPT__|${TRANSCRIPT}|g" \
  -e "s|__PROJECT_NAME__|${PROJECT_NAME}|g" \
  -e "s|__PROJECT_DIR__|${PROJECT_DIR}|g" \
  -e "s|__TODAY__|${TODAY}|g" \
  -e "s|__NOW__|${NOW}|g" \
  -e "s|__PLUGIN_ROOT__|${PLUGIN_ROOT_PATH}|g" \
  -e "s|__PROJECT_HEAD__|${PROJECT_HEAD}|g" \
  -e "s|__VAULT_HEAD__|${VAULT_HEAD}|g")

export REVIEW_PROMPT

# ---------------------------------------------------------------------------
# Background the review so the hook returns immediately. After review, autocommit
# any vault changes (no push by default — controlled by AUTOPUSH).
# ---------------------------------------------------------------------------
PLUGIN_ROOT_PATH="${CLAUDE_PLUGIN_ROOT:-}"
USAGE_LOGGER="$PLUGIN_ROOT_PATH/hooks/scripts/_usage_log.sh"

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
  SESSION_ID='"$(printf %q "$SESSION_ID")"'
  USAGE_LOGGER='"$(printf %q "$USAGE_LOGGER")"'
  SLIM_TRANSCRIPT='"$(printf %q "$SLIM_TRANSCRIPT")"'
  SAFE_SID='"$(printf %q "${SAFE_SID:-}")"'
  SESSION_STATE_DIR='"$(printf %q "$SESSION_STATE_DIR")"'

  if [ "$RUN_REVIEW" = "true" ]; then
    echo "[$(ts)] starting review for project=$PROJECT_NAME transcript=$TRANSCRIPT" >> "$LOG"
    # Note: --bare disables OAuth/keychain auth (see `claude --help`). We rely
    # on the recursion-guard env vars (CLAUDE_MEMORY_REVIEW=1) plus the
    # early-exit checks at the top of the hook scripts to prevent recursion.
    #
    # --output-format json wraps the response so we can capture real .usage
    # and .total_cost_usd for /obsidian-memory:usage. Output goes to a temp
    # file that is then both appended to LOG and parsed for telemetry.
    REVIEW_OUT=$(mktemp 2>/dev/null || echo "")
    if [ -n "$REVIEW_OUT" ]; then
      CLAUDE_MEMORY_REVIEW=1 "$CLAUDE_BIN" -p "$REVIEW_PROMPT" \
        --tools "Read,Write,Edit,Bash" \
        --strict-mcp-config \
        --output-format json \
        > "$REVIEW_OUT" 2>> "$LOG"
      review_exit=$?
      cat "$REVIEW_OUT" >> "$LOG"
      echo "[$(ts)] review complete (exit=$review_exit)" >> "$LOG"
      if [ -n "$SESSION_ID" ] && [ -x "$USAGE_LOGGER" ] && [ $review_exit -eq 0 ]; then
        usage=$(jq -c ".[]? | select(.type==\"result\") | .usage // {}" "$REVIEW_OUT" 2>/dev/null || echo "{}")
        cost=$(jq -r ".[]? | select(.type==\"result\") | .total_cost_usd // empty" "$REVIEW_OUT" 2>/dev/null || echo "")
        duration=$(jq -r ".[]? | select(.type==\"result\") | .duration_ms // empty" "$REVIEW_OUT" 2>/dev/null || echo "")
        if [ -n "$usage" ] && [ "$usage" != "null" ]; then
          bash "$USAGE_LOGGER" api "$SESSION_ID" review_call "$usage" "$cost" "$duration" 2>>"$LOG" || true
        fi
      fi
      rm -f "$REVIEW_OUT"
    else
      CLAUDE_MEMORY_REVIEW=1 "$CLAUDE_BIN" -p "$REVIEW_PROMPT" \
        --tools "Read,Write,Edit,Bash" \
        --strict-mcp-config \
        >> "$LOG" 2>&1
      echo "[$(ts)] review complete (exit=$?, no telemetry — mktemp failed)" >> "$LOG"
    fi
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

  if [ -n "$SLIM_TRANSCRIPT" ] && [ -f "$SLIM_TRANSCRIPT" ]; then
    rm -f "$SLIM_TRANSCRIPT"
  fi

  # Clean up per-session HEAD SHA files now that the review has consumed them.
  if [ -n "$SAFE_SID" ] && [ -n "$SESSION_STATE_DIR" ]; then
    rm -f "$SESSION_STATE_DIR/$SAFE_SID.project_head" 2>/dev/null || true
    rm -f "$SESSION_STATE_DIR/$SAFE_SID.vault_head" 2>/dev/null || true
  fi
' >/dev/null 2>&1 &

exit 0
