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
CONFIG_FILE="${HOME}/.config/obsidian-memory/config.env"
if [ -f "$CONFIG_FILE" ]; then
  # shellcheck disable=SC1090
  . "$CONFIG_FILE"
fi

VAULT="${OBSIDIAN_VAULT_PATH:-$HOME/Documents/Obsidian Memory}"
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

# Load vault HEAD SHA recorded at SessionStart so the review can scope
# backlink reconciliation to "what changed in the vault during this session"
# (incl. mid-session commits). Empty value is fine — the review falls back
# to working-tree-only diff vs HEAD in that case.
SESSION_STATE_DIR="${MEMORY_SESSION_STATE_DIR:-/tmp/claude-memory-session}"
VAULT_HEAD=""
if [ -n "$SESSION_ID" ]; then
  SAFE_SID=$(echo "$SESSION_ID" | tr -c 'A-Za-z0-9._-' '_')
  VAULT_HEAD_FILE="$SESSION_STATE_DIR/$SAFE_SID.vault_head"
  [ -f "$VAULT_HEAD_FILE" ] && VAULT_HEAD=$(cat "$VAULT_HEAD_FILE" 2>/dev/null || true)
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

RUN_REVIEW=true
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

# Precompute the vault-changes command and HEAD display so the prompt stays
# free of shell conditionals. Single-quote vault/script paths to handle
# spaces; vault paths don't contain single quotes in practice.
VAULT_HEAD_DISPLAY="${VAULT_HEAD:-(none)}"
if [ -n "$VAULT_HEAD" ]; then
  VAULT_CHANGES_CMD="python3 '$PLUGIN_ROOT_PATH/scripts/_vault.py' --vault '$VAULT' vault-changes --base-sha $VAULT_HEAD"
else
  VAULT_CHANGES_CMD="python3 '$PLUGIN_ROOT_PATH/scripts/_vault.py' --vault '$VAULT' vault-changes"
fi

read -r -d '' REVIEW_PROMPT_TMPL <<'PROMPT_EOF' || true
End-of-session memory review.

Vault:        __VAULT__
Transcript:   __TRANSCRIPT__
Project:      __PROJECT_NAME__ (at __PROJECT_DIR__)
Date / time:  __TODAY__ __NOW__
Vault HEAD at session start: __VAULT_HEAD_DISPLAY__

Do steps 1-3 first. Step 4 (the journal) is written last so its bullets can reference everything you wrote.

1. PROACTIVE NOTES — capture moments in the transcript where information surfaces that is stable across sessions, useful in future sessions, and not derivable from the codebase or git history. Covers corrections, preferences, validated approaches, always / from now on / stop doing X rules, decisions and rationale, and novel facts (people, IDs, configs, channels, dashboards, endpoints). Skip if already covered (verify via `python3 __PLUGIN_ROOT__/scripts/_vault.py --vault __VAULT__ search --type <t> --keywords <k> --json`; extend a near-duplicate rather than creating a new note).

   Pick the type first (one of: preference, reference, decision, learning, tool — never journal here, journal is step 4). Then route:

     A. type == tool       → __VAULT__/Tools/<slug>.md
     B. type == preference → __VAULT__/Notes/<slug>.md  (add `project: __PROJECT_NAME__` only if narrowly scoped)
     C. type ∈ {reference, decision, learning}:
        1. If __PROJECT_DIR__ is registered + enabled in projects.json
           (check via `python3 __PLUGIN_ROOT__/scripts/_projects.py lookup __PROJECT_DIR__`)
           AND has a folder matching the type
           (check via `python3 __PLUGIN_ROOT__/scripts/_project_docs.py match-type-folder __PROJECT_DIR__ --type <type>`):
             → __PROJECT_DIR__/<matched-folder>/<slug>.md  (with project: from the registry)
        2. Otherwise → __VAULT__/Notes/<slug>.md  (with `project: __PROJECT_NAME__` if project-scoped)

   Frontmatter on every new note: type, description, created (+ project when scoped). type ∈ {preference, reference, decision, learning, tool}.

   Always wrap the `description:` value in double quotes (e.g. `description: "one-line hook"`). Descriptions often contain `:`, `[[wikilinks]]`, or `[brackets]` — unquoted, these break YAML parsing. Escape any embedded `"` as `\"`. This rule also applies when you rewrite an existing note's `description` (step 2) or the journal's day-summary `description` (step 4).

2. MODIFY existing notes only on explicit user correction in the transcript. Smallest edit. Inferred staleness → flag in output, do not edit.

   When you extend or correct a non-journal note, check its frontmatter `description` against the new body. If the one-line summary no longer fits, rewrite it (smallest edit). The SessionStart auto-overview is built from these descriptions — stale ones mislead future sessions.

3. INTEGRITY — operates on:
   (a) vault notes touched in steps 1-2 above
   (b) non-journal vault notes referenced (wikilinks/paths) in any prior-session entry of today's journal
   (c) vault *.md files changed since the last commit (for backlink reconciliation on renames/deletes)

   Enumerate (c):
     __VAULT_CHANGES_CMD__

   Per-source checks:

   - (a) + (b): frontmatter completeness (type, description, created; + project when project-scoped); every [[wikilink]] resolves; description-vs-body drift (rewrite description on drift, smallest edit).

   - (c) Vault file changes — BACKLINK RECONCILIATION:
       * RENAMED (old → new) → `python3 __PLUGIN_ROOT__/scripts/_vault.py --vault __VAULT__ incoming-wikilinks --target <old>` to find every note linking to the OLD path. Auto-rewrite each occurrence to the NEW path (smallest edit; preserve any |alias text). For bare basename links, prefer the new basename. List rewrites under "## Backlink rewrites".
       * DELETED → same command on the deleted path to find broken backlinks. List under "## Broken backlinks (target deleted)". DO NOT auto-fix — deletion may be intentional or may be a rename the diff couldn't infer.
       * ADDED / MODIFIED → no backlink action needed.

   Auto-fix unambiguous issues (description drift, backlink-rename). List ambiguous and non-fixable items in their dedicated sections.

4. JOURNAL — always, written LAST.
   Path: __VAULT__/Journals/__PROJECT_NAME__/__TODAY__.md

   Journals are scoped one-file-per-project-per-day: the directory `__PROJECT_NAME__/` segregates this project's day from any other project's day. Use the `Write` tool — it creates parent directories automatically.

   New file: frontmatter (type=journal, description=<one-line day summary>, project=__PROJECT_NAME__, created=__TODAY__) + "## Session __NOW__" + 3-6 bullets covering work, decisions, learnings.

   Existing file: append a "## Session __NOW__" section. Do not edit any prior content (earlier sessions today, earlier days). You MAY (and should) rewrite the frontmatter `description` to summarize the full day now that more sessions exist.

   Each bullet that describes a write must include the path:
   - Vault writes (steps 1-2) → vault-relative path.
   - Project-vault writes (step 1, route C.1) → repo-relative path inside __PROJECT_DIR__.
   The journal is the cross-session anchor; paths in bullets are how future sessions find the work.

OUTPUT sections (in order, omit when empty):
  ## Vault writes              (paths created/appended in the personal vault)
  ## Project-vault writes      (paths in __PROJECT_DIR__'s registered project-vault)
  ## Backlink rewrites         (notes whose [[wikilinks]] were updated for renames)
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
  -e "s|__VAULT_HEAD_DISPLAY__|${VAULT_HEAD_DISPLAY}|g" \
  -e "s|__VAULT_CHANGES_CMD__|${VAULT_CHANGES_CMD}|g")

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
    rm -f "$SESSION_STATE_DIR/$SAFE_SID.vault_head" 2>/dev/null || true
  fi
' >/dev/null 2>&1 &

exit 0
