#!/bin/bash
# SessionEnd hook: review transcript, write journal entry + proactive memory updates.
# Backgrounds a `claude -p` subprocess so it doesn't block session shutdown.

set -u

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
AUTOCOMMIT="${OBSIDIAN_MEMORY_AUTOCOMMIT:-true}"
AUTOPUSH="${OBSIDIAN_MEMORY_AUTOPUSH:-false}"

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
# Build the review prompt. Use a quoted heredoc (no expansion) so angle brackets
# and pipes in the prompt body are treated as literal text. Substitute the
# placeholders with sed afterwards.
# ---------------------------------------------------------------------------
REVIEW_PROMPT=$(cat <<'PROMPT_EOF' | sed \
  -e "s|__VAULT__|${VAULT}|g" \
  -e "s|__TRANSCRIPT__|${TRANSCRIPT}|g" \
  -e "s|__PROJECT_NAME__|${PROJECT_NAME}|g" \
  -e "s|__TODAY__|${TODAY}|g" \
  -e "s|__NOW__|${NOW}|g"
You are doing an end-of-session memory review for the Obsidian vault at __VAULT__.

Transcript: __TRANSCRIPT__
Project: __PROJECT_NAME__
Date/time: __TODAY__ __NOW__

DO BOTH OF THE FOLLOWING:

1. JOURNAL (always do this):
   Write/append a journal entry at Projects/__PROJECT_NAME__/Journal/__TODAY__.md.
   - If the file does not exist, create it with frontmatter:
       ---
       type: journal
       description: one-line summary of the day's sessions
       project: __PROJECT_NAME__
       created: __TODAY__
       ---
     Then add a "## Session __NOW__" heading and 3-6 bullets summarizing what was done, key decisions, and notable learnings.
   - If the file already exists, APPEND a new "## Session __NOW__" section with the same bullet structure. Do NOT modify existing content above.

2. PROACTIVE NOTES (only when worth writing AND not already covered):
   Scan the transcript for items that meet ALL of these:
   - Significant: user correction, validated approach, novel fact, decision, or explicit "remember this"
   - Useful in future sessions (not ephemeral session detail like one-off command output)
   - Not already in the vault — VERIFY by running obsidian search with keywords from the candidate, and reading any matches before deciding to write.

   For each that qualifies, write a new note in the appropriate folder:
   - User correction about coding/communication style: General/Preferences/SLUG.md
   - Cross-project external system: General/References/SLUG.md
   - Project decision (choice + rationale): Projects/__PROJECT_NAME__/Decisions/SLUG.md
   - Project gotcha / how-X-actually-works: Projects/__PROJECT_NAME__/Learnings/SLUG.md
   - Tool reference (CLI/API usage you did not know before): Tools/SLUG.md
   - Person info: General/People/SLUG.md

   Every new note MUST have frontmatter with: type, description, created. Add project for project-scoped notes. Type is one of: preference, reference, decision, learning, tool, people.

   After writing, update the relevant INDEX.md to list the new note.

3. MODIFY existing notes ONLY when the transcript contains an explicit correction by the user — they directly state that some prior fact, instruction, or memory is wrong or outdated. Make the smallest edit that fixes the issue. Do NOT modify based on inference or implication.

   If the transcript merely suggests an existing note might be stale (without explicit user correction), leave the note alone and flag it in your output summary so the next session can verify.

   Outside of explicit corrections, the only allowed modifications are: appending to today's journal, and updating INDEX files to list new notes.

4. INTEGRITY CHECK (deltas only — only the notes YOU created or modified in steps 1-3, not the whole vault):
   For each such file, verify:
   a. Frontmatter has: `type`, `description`, `created`. Files under `Projects/` also need `project`.
   b. Every `[[wikilink]]` in the body resolves. Path-qualified links (`[[Folder/Note]]`) must point at an existing file. Bare links (`[[note-name]]`) must match the basename of some note in the vault.
   c. New notes (not journal appends) must be listed in the relevant INDEX.md — the INDEX of the folder they live in (e.g., `Projects/__PROJECT_NAME__/INDEX.md` for project notes, `General/INDEX.md` for cross-project).

   Auto-fix what is unambiguous (add the missing INDEX entry, fix an obvious typo in a wikilink). For anything ambiguous, list it under "## Integrity flags" in your output and leave it. Do NOT scan or fix files outside the deltas — that is the job of the full audit script (`scripts/audit.py`).

OUTPUT FORMAT: at the end, print a short list of files created or appended, then any "## Integrity flags". No narrative.
PROMPT_EOF
)

export REVIEW_PROMPT

# ---------------------------------------------------------------------------
# Background the review so the hook returns immediately. After review, autocommit
# any vault changes (no push by default — controlled by AUTOPUSH).
# ---------------------------------------------------------------------------
nohup bash -c '
  if [ "'"$RUN_REVIEW"'" = "true" ]; then
    echo "[$(date "+%Y-%m-%d %H:%M:%S")] starting review for project='"$PROJECT_NAME"' transcript='"$TRANSCRIPT"'" >> "'"$LOG"'"
    CLAUDE_MEMORY_REVIEW=1 "'"$CLAUDE_BIN"'" -p "$REVIEW_PROMPT" \
      --allowed-tools "Read,Write,Edit,Bash" \
      >> "'"$LOG"'" 2>&1
    echo "[$(date "+%Y-%m-%d %H:%M:%S")] review complete (exit=$?)" >> "'"$LOG"'"
  fi

  if [ "'"$AUTOCOMMIT"'" = "true" ] && [ -d "'"$VAULT"'/.git" ]; then
    cd "'"$VAULT"'" || exit 0
    if [ -n "$(git status --porcelain)" ]; then
      git add -A
      if git commit -m "session writes '"$TODAY $NOW"' ('"$PROJECT_NAME"')" >> "'"$LOG"'" 2>&1; then
        echo "[$(date "+%Y-%m-%d %H:%M:%S")] vault auto-committed" >> "'"$LOG"'"
        if [ "'"$AUTOPUSH"'" = "true" ]; then
          if git push >> "'"$LOG"'" 2>&1; then
            echo "[$(date "+%Y-%m-%d %H:%M:%S")] vault auto-pushed" >> "'"$LOG"'"
          else
            echo "[$(date "+%Y-%m-%d %H:%M:%S")] vault push failed" >> "'"$LOG"'"
          fi
        fi
      else
        echo "[$(date "+%Y-%m-%d %H:%M:%S")] vault commit failed" >> "'"$LOG"'"
      fi
    else
      echo "[$(date "+%Y-%m-%d %H:%M:%S")] vault clean — nothing to commit" >> "'"$LOG"'"
    fi
  fi
' >/dev/null 2>&1 &

exit 0
