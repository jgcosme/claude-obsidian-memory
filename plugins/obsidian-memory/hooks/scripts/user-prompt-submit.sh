#!/bin/bash
# UserPromptSubmit hook: vault retrieval gate.
#
# For each user message, asks the user's default Claude model whether any vault
# notes are worth reading to answer well. If yes, validates the paths and
# injects their bodies as additional context.
#
# Failure mode: loud and non-blocking — errors go to stderr (visible to user)
# and to a log, but the hook always exits 0 so the prompt is never blocked.
#
# Caching: the gate's static portion (instructions + vault indexes) is sent via
# --system-prompt so Anthropic's prompt cache can reuse it across calls within
# the 5-minute TTL. The dynamic part (the user message) is the only uncached
# input per call.

set -u

# ---------------------------------------------------------------------------
# Recursion guard. The gate spawns `claude -p --bare` which already skips
# hooks, but we keep the env var guard as a belt-and-suspenders measure in case
# the user invokes the gate hook outside of the bare-mode subprocess somehow.
# ---------------------------------------------------------------------------
if [ -n "${CLAUDE_MEMORY_GATE:-}" ] || [ -n "${CLAUDE_MEMORY_REVIEW:-}" ]; then
  exit 0
fi

# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------
CONFIG_FILE="${HOME}/.config/claude-memory/config.env"
if [ -f "$CONFIG_FILE" ]; then
  # shellcheck disable=SC1090
  . "$CONFIG_FILE" 2>/dev/null || true
fi

VAULT="${OBSIDIAN_VAULT_PATH:-$HOME/Documents/Obsidian Vault}"
LOG="${MEMORY_GATE_LOG:-/tmp/claude-memory-gate.log}"
LOG_MAX_BYTES="${MEMORY_LOG_MAX_BYTES:-1048576}"  # 1 MB default
PATH_CAP="${OBSIDIAN_MEMORY_GATE_PATH_CAP:-3}"
NOTE_BYTE_CAP="${OBSIDIAN_MEMORY_GATE_NOTE_BYTE_CAP:-10240}"  # 10 KB per injected note
GATE_ENABLED="${OBSIDIAN_MEMORY_GATE_ENABLED:-true}"
DEBUG="${OBSIDIAN_MEMORY_DEBUG:-false}"

ts() { date '+%Y-%m-%d %H:%M:%S'; }
debug() {
  [ "$DEBUG" = "true" ] && echo "[$(ts)] DEBUG: $*" >> "$LOG"
}

# Rotate log if oversized (keep one previous as .log.1)
if [ -f "$LOG" ]; then
  bytes=$(wc -c < "$LOG" 2>/dev/null | tr -d ' ' || echo 0)
  if [ "${bytes:-0}" -gt "$LOG_MAX_BYTES" ]; then
    mv -f "$LOG" "${LOG}.1" 2>/dev/null || true
  fi
fi

if [ "$GATE_ENABLED" != "true" ]; then
  debug "gate disabled by config"
  exit 0
fi

# Locate the `claude` CLI
if [ -n "${CLAUDE_BIN:-}" ] && [ -x "$CLAUDE_BIN" ]; then
  : # use override
elif command -v claude >/dev/null 2>&1; then
  CLAUDE_BIN="$(command -v claude)"
else
  echo "[gate] claude CLI not found on PATH; vault gate skipped this turn" >&2
  echo "[$(ts)] skipped: no claude CLI" >> "$LOG"
  exit 0
fi

if [ ! -d "$VAULT" ]; then
  echo "[$(ts)] skipped: vault not found at '$VAULT'" >> "$LOG"
  exit 0
fi

# ---------------------------------------------------------------------------
# Read payload (JSON on stdin) — extract user prompt and session id
# ---------------------------------------------------------------------------
PAYLOAD=$(cat)
USER_MESSAGE=$(echo "$PAYLOAD" | jq -r '.prompt // empty' 2>/dev/null || true)
SESSION_ID=$(echo "$PAYLOAD" | jq -r '.session_id // empty' 2>/dev/null || true)

if [ -z "$USER_MESSAGE" ]; then
  echo "[$(ts)] skipped: no .prompt in payload" >> "$LOG"
  exit 0
fi

debug "session_id=$SESSION_ID prompt_len=${#USER_MESSAGE}"

PROJECT_DIR="${CLAUDE_PROJECT_DIR:-$PWD}"
PROJECT_NAME=$(basename "$PROJECT_DIR")

# Per-session dedup file: tracks which paths have already been injected this
# session so we don't repeatedly inject the same note across consecutive turns.
DEDUP_DIR="${MEMORY_GATE_DEDUP_DIR:-/tmp/claude-memory-gate-state}"
mkdir -p "$DEDUP_DIR" 2>/dev/null || true
DEDUP_FILE=""
if [ -n "$SESSION_ID" ]; then
  # Sanitize session id for filename use
  SAFE_ID=$(echo "$SESSION_ID" | tr -c 'A-Za-z0-9._-' '_')
  DEDUP_FILE="$DEDUP_DIR/$SAFE_ID.injected"
  touch "$DEDUP_FILE" 2>/dev/null || DEDUP_FILE=""
fi

# ---------------------------------------------------------------------------
# Collect always-loaded indexes + the current project's index (if any)
# ---------------------------------------------------------------------------
INDEX_TEXT=""
for idx in \
  "$VAULT/INDEX.md" \
  "$VAULT/Tools/INDEX.md" \
  "$VAULT/General/INDEX.md" \
  "$VAULT/Projects/$PROJECT_NAME/INDEX.md"
do
  if [ -f "$idx" ]; then
    rel="${idx#"$VAULT"/}"
    INDEX_TEXT+=$'\n=== '"$rel"$' ===\n'
    INDEX_TEXT+="$(cat "$idx")"
    INDEX_TEXT+=$'\n'
  fi
done

if [ -z "$INDEX_TEXT" ]; then
  echo "[$(ts)] skipped: no indexes found in vault" >> "$LOG"
  exit 0
fi

# ---------------------------------------------------------------------------
# Build prompts: SYSTEM (cacheable, stable) + USER (per-call)
# ---------------------------------------------------------------------------
GATE_SYSTEM_PROMPT=$(cat <<PROMPT
You are a retrieval gate for an Obsidian-backed memory vault.

Your job: given a user message and the vault index excerpts below, decide
which (if any) existing notes are worth reading to answer the user's request
well.

OUTPUT FORMAT: a single JSON object on one line. No prose, no code fences.
Schema:
  {"read": ["relative/path1.md", "relative/path2.md"]}

Rules:
- List up to $PATH_CAP relative paths from the vault root. Fewer is better.
- Empty array if no note is clearly relevant.
- Use ONLY paths visible in the indexes below — do not invent paths.
- Prefer notes whose description directly addresses the user's topic.

=== VAULT INDEXES ===
$INDEX_TEXT
PROMPT
)

GATE_USER_PROMPT="USER MESSAGE:
$USER_MESSAGE

JSON only:"

# ---------------------------------------------------------------------------
# Call the gate. --bare skips hooks/LSP/plugins/auto-memory in the subprocess.
# --tools \"\" disables all tools. Inherit the user's default model.
# ---------------------------------------------------------------------------
GATE_OUTPUT=$(CLAUDE_MEMORY_GATE=1 CLAUDE_MEMORY_REVIEW=1 \
  "$CLAUDE_BIN" -p "$GATE_USER_PROMPT" \
    --system-prompt "$GATE_SYSTEM_PROMPT" \
    --tools "" \
    --bare \
    2>>"$LOG")
GATE_EXIT=$?

debug "gate exit=$GATE_EXIT output_len=${#GATE_OUTPUT}"

if [ $GATE_EXIT -ne 0 ]; then
  echo "[gate] retrieval gate failed (claude -p exit=$GATE_EXIT) — proceeding without vault context" >&2
  echo "[$(ts)] gate exited $GATE_EXIT; output: $GATE_OUTPUT" >> "$LOG"
  exit 0
fi

# ---------------------------------------------------------------------------
# Extract first balanced {...} block from the model output.
# Robust to surrounding whitespace, prose, or code-fence wrapping.
# ---------------------------------------------------------------------------
JSON_BLOB=$(echo "$GATE_OUTPUT" | python3 -c '
import sys, json
text = sys.stdin.read()
start = text.find("{")
if start < 0:
    sys.exit(1)
depth = 0
in_str = False
esc = False
for i in range(start, len(text)):
    c = text[i]
    if esc:
        esc = False
        continue
    if c == "\\":
        esc = True
        continue
    if c == "\"":
        in_str = not in_str
        continue
    if in_str:
        continue
    if c == "{":
        depth += 1
    elif c == "}":
        depth -= 1
        if depth == 0:
            try:
                obj = json.loads(text[start:i+1])
                print(json.dumps(obj))
                sys.exit(0)
            except Exception:
                sys.exit(2)
sys.exit(3)
' 2>/dev/null)

if [ -z "$JSON_BLOB" ]; then
  TRUNC=$(echo "$GATE_OUTPUT" | head -c 200)
  echo "[gate] could not parse JSON from gate output — proceeding without vault context" >&2
  echo "[$(ts)] no parseable JSON; output (first 200 chars): $TRUNC" >> "$LOG"
  exit 0
fi

# Extract paths, capped client-side as a defensive backstop
PATHS=$(echo "$JSON_BLOB" | jq -r --argjson cap "$PATH_CAP" '.read[:$cap][]?' 2>/dev/null || true)

if [ -z "$PATHS" ]; then
  echo "[$(ts)] gate: no paths returned" >> "$LOG"
  exit 0
fi

# ---------------------------------------------------------------------------
# Validate paths and assemble injection
# ---------------------------------------------------------------------------
TMP=$(mktemp)
trap 'rm -f "$TMP"' EXIT

# Reject path that is absolute or has any `..` component.
is_safe_path() {
  local p="$1"
  case "$p" in /*) return 1 ;; esac
  local IFS='/'
  # shellcheck disable=SC2086
  set -f
  for comp in $p; do
    if [ "$comp" = ".." ]; then
      return 1
    fi
  done
  set +f
  return 0
}

# Has this path already been injected this session?
already_injected() {
  local p="$1"
  [ -n "$DEDUP_FILE" ] && grep -Fxq "$p" "$DEDUP_FILE" 2>/dev/null
}

mark_injected() {
  local p="$1"
  [ -n "$DEDUP_FILE" ] && echo "$p" >> "$DEDUP_FILE"
}

INJECTED=0
DROPPED=()
DUPED=()
while IFS= read -r p; do
  [ -z "$p" ] && continue
  if ! is_safe_path "$p"; then
    DROPPED+=("$p (unsafe)")
    continue
  fi
  if [ ! -f "$VAULT/$p" ]; then
    DROPPED+=("$p (missing)")
    continue
  fi
  if already_injected "$p"; then
    DUPED+=("$p")
    continue
  fi
  {
    echo ""
    echo "--- $p ---"
    # Per-note size cap: head -c is byte-bounded
    head -c "$NOTE_BYTE_CAP" "$VAULT/$p"
    note_size=$(wc -c < "$VAULT/$p" 2>/dev/null | tr -d ' ' || echo 0)
    if [ "${note_size:-0}" -gt "$NOTE_BYTE_CAP" ]; then
      printf '\n[…truncated at %s bytes; full content via obsidian read path="%s"]\n' "$NOTE_BYTE_CAP" "$p"
    fi
  } >> "$TMP"
  mark_injected "$p"
  INJECTED=$((INJECTED+1))
done <<< "$PATHS"

if [ ${#DROPPED[@]} -gt 0 ]; then
  echo "[$(ts)] gate: dropped paths: ${DROPPED[*]}" >> "$LOG"
fi
if [ ${#DUPED[@]} -gt 0 ]; then
  echo "[$(ts)] gate: skipped already-injected this session: ${DUPED[*]}" >> "$LOG"
fi

if [ "$INJECTED" -gt 0 ]; then
  echo ""
  echo "=== VAULT CONTEXT (auto-retrieved by memory gate) ==="
  cat "$TMP"
  echo ""
  echo "=== END VAULT CONTEXT ==="
  echo "[$(ts)] gate: injected $INJECTED notes" >> "$LOG"
else
  echo "[$(ts)] gate: nothing new to inject" >> "$LOG"
fi

exit 0
