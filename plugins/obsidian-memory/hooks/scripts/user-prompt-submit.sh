#!/bin/bash
# UserPromptSubmit hook: vault retrieval gate.
#
# For each user message, asks the user's default Claude model whether any vault
# notes are worth reading to answer well. If yes, validates the paths and
# injects their bodies as additional context.
#
# Failure mode: loud and non-blocking — errors go to stderr (visible to user)
# and to a log, but the hook always exits 0 so the prompt is never blocked.

set -u

# ---------------------------------------------------------------------------
# Recursion guard. The gate spawns `claude -p`, which itself fires a fresh
# UserPromptSubmit (and SessionEnd on shutdown). We set CLAUDE_MEMORY_GATE=1
# and CLAUDE_MEMORY_REVIEW=1 on the subprocess; this short-circuits both.
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
  . "$CONFIG_FILE"
fi

VAULT="${OBSIDIAN_VAULT_PATH:-$HOME/Documents/Obsidian Vault}"
LOG="${MEMORY_GATE_LOG:-/tmp/claude-memory-gate.log}"
PATH_CAP="${OBSIDIAN_MEMORY_GATE_PATH_CAP:-3}"
GATE_ENABLED="${OBSIDIAN_MEMORY_GATE_ENABLED:-true}"

if [ "$GATE_ENABLED" != "true" ]; then
  exit 0
fi

# Locate the `claude` CLI
if [ -n "${CLAUDE_BIN:-}" ] && [ -x "$CLAUDE_BIN" ]; then
  : # use override
elif command -v claude >/dev/null 2>&1; then
  CLAUDE_BIN="$(command -v claude)"
else
  echo "[gate] claude CLI not found on PATH; vault gate skipped this turn" >&2
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] skipped: no claude CLI" >> "$LOG"
  exit 0
fi

# Vault must exist
if [ ! -d "$VAULT" ]; then
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] skipped: vault not found at '$VAULT'" >> "$LOG"
  exit 0
fi

# ---------------------------------------------------------------------------
# Read payload (JSON on stdin) — extract the user's prompt
# ---------------------------------------------------------------------------
PAYLOAD=$(cat)
USER_MESSAGE=$(echo "$PAYLOAD" | jq -r '.prompt // empty' 2>/dev/null || true)

if [ -z "$USER_MESSAGE" ]; then
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] skipped: no .prompt in payload" >> "$LOG"
  exit 0
fi

PROJECT_DIR="${CLAUDE_PROJECT_DIR:-$PWD}"
PROJECT_NAME=$(basename "$PROJECT_DIR")

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
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] skipped: no indexes found in vault" >> "$LOG"
  exit 0
fi

# ---------------------------------------------------------------------------
# Build the gate prompt
# ---------------------------------------------------------------------------
GATE_PROMPT=$(cat <<PROMPT
You are a retrieval gate for an Obsidian-backed memory vault.

Given the user message below and the vault index excerpts, decide which (if
any) existing notes are worth reading to answer the user's request well.

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

=== USER MESSAGE ===
$USER_MESSAGE

JSON only:
PROMPT
)

# ---------------------------------------------------------------------------
# Call the gate. Inherit the user's default model (no --model flag).
# Disallow tools — gate is pure text in/out.
# ---------------------------------------------------------------------------
GATE_OUTPUT=$(CLAUDE_MEMORY_GATE=1 CLAUDE_MEMORY_REVIEW=1 \
  "$CLAUDE_BIN" -p "$GATE_PROMPT" --allowed-tools "" 2>>"$LOG")
GATE_EXIT=$?

if [ $GATE_EXIT -ne 0 ]; then
  echo "[gate] retrieval gate failed (claude -p exit=$GATE_EXIT) — proceeding without vault context" >&2
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] gate exited $GATE_EXIT; output: $GATE_OUTPUT" >> "$LOG"
  exit 0
fi

# ---------------------------------------------------------------------------
# Extract first balanced {...} block from the model output.
# Robust to surrounding whitespace, prose, or code-fence wrapping.
# (Use -c so stdin stays available for the gate output.)
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
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] no parseable JSON; output (first 200 chars): $TRUNC" >> "$LOG"
  exit 0
fi

# Extract paths, capped client-side as a defensive backstop
PATHS=$(echo "$JSON_BLOB" | jq -r --argjson cap "$PATH_CAP" '.read[:$cap][]?' 2>/dev/null || true)

if [ -z "$PATHS" ]; then
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] gate: no paths returned" >> "$LOG"
  exit 0
fi

# ---------------------------------------------------------------------------
# Validate paths and assemble injection
# ---------------------------------------------------------------------------
TMP=$(mktemp)
trap 'rm -f "$TMP"' EXIT

INJECTED=0
DROPPED=()
while IFS= read -r p; do
  [ -z "$p" ] && continue
  # Reject path-traversal attempts
  case "$p" in
    /*|*..*) DROPPED+=("$p"); continue ;;
  esac
  if [ -f "$VAULT/$p" ]; then
    {
      echo ""
      echo "--- $p ---"
      cat "$VAULT/$p"
    } >> "$TMP"
    INJECTED=$((INJECTED+1))
  else
    DROPPED+=("$p")
  fi
done <<< "$PATHS"

if [ ${#DROPPED[@]} -gt 0 ]; then
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] gate: dropped invalid paths: ${DROPPED[*]}" >> "$LOG"
fi

if [ "$INJECTED" -gt 0 ]; then
  echo ""
  echo "=== VAULT CONTEXT (auto-retrieved by memory gate) ==="
  cat "$TMP"
  echo ""
  echo "=== END VAULT CONTEXT ==="
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] gate: injected $INJECTED notes" >> "$LOG"
else
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] gate: nothing to inject" >> "$LOG"
fi

exit 0
