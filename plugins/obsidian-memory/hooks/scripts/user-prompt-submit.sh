#!/bin/bash
# UserPromptSubmit hook: vault retrieval gate (design B).
#
# For each user message, asks the user's default Claude model what (if any)
# vault notes are worth reading. The gate may either pick paths directly
# (`read`) or specify typed searches (`search`) for us to execute via the
# plugin's Python search module. Hits from both sources are merged, validated,
# deduped, and injected as additional context — bounded by PATH_CAP total.
#
# Failure mode: loud and non-blocking. Errors go to stderr (visible to user)
# and to a log; the hook always exits 0 so the prompt is never blocked.
#
# Caching: the gate's static portion (instructions + vault overview) is sent
# via --system-prompt so Anthropic's prompt cache reuses it across calls.

set -u

# ---------------------------------------------------------------------------
# Recursion guard
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
PLUGIN_ROOT="${CLAUDE_PLUGIN_ROOT:-}"
LOG="${MEMORY_GATE_LOG:-/tmp/claude-memory-gate.log}"
LOG_MAX_BYTES="${MEMORY_LOG_MAX_BYTES:-1048576}"
PATH_CAP="${OBSIDIAN_MEMORY_GATE_PATH_CAP:-3}"
NOTE_BYTE_CAP="${OBSIDIAN_MEMORY_GATE_NOTE_BYTE_CAP:-10240}"
GATE_ENABLED="${OBSIDIAN_MEMORY_GATE_ENABLED:-true}"
DEBUG="${OBSIDIAN_MEMORY_DEBUG:-false}"

ts() { date '+%Y-%m-%d %H:%M:%S'; }
debug() { [ "$DEBUG" = "true" ] && echo "[$(ts)] DEBUG: $*" >> "$LOG"; }

# Rotate log if oversized
if [ -f "$LOG" ]; then
  bytes=$(wc -c < "$LOG" 2>/dev/null | tr -d ' ' || echo 0)
  if [ "${bytes:-0}" -gt "$LOG_MAX_BYTES" ]; then
    mv -f "$LOG" "${LOG}.1" 2>/dev/null || true
  fi
fi

[ "$GATE_ENABLED" = "true" ] || exit 0

if [ -n "${CLAUDE_BIN:-}" ] && [ -x "$CLAUDE_BIN" ]; then
  :
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

VAULT_PY="$PLUGIN_ROOT/scripts/_vault.py"
if [ ! -f "$VAULT_PY" ]; then
  echo "[$(ts)] skipped: _vault.py not found at $VAULT_PY" >> "$LOG"
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

# Per-session dedup
DEDUP_DIR="${MEMORY_GATE_DEDUP_DIR:-/tmp/claude-memory-gate-state}"
mkdir -p "$DEDUP_DIR" 2>/dev/null || true
DEDUP_FILE=""
if [ -n "$SESSION_ID" ]; then
  SAFE_ID=$(echo "$SESSION_ID" | tr -c 'A-Za-z0-9._-' '_')
  DEDUP_FILE="$DEDUP_DIR/$SAFE_ID.injected"
  touch "$DEDUP_FILE" 2>/dev/null || DEDUP_FILE=""
fi

# ---------------------------------------------------------------------------
# Build the gate's view of the vault: auto-generated overview from frontmatter
# (cacheable — same across calls until vault contents change).
# ---------------------------------------------------------------------------
OVERVIEW=$(python3 "$VAULT_PY" --vault "$VAULT" overview --project "$PROJECT_NAME" 2>/dev/null || true)
if [ -z "$OVERVIEW" ]; then
  echo "[$(ts)] skipped: vault overview empty" >> "$LOG"
  exit 0
fi

# ---------------------------------------------------------------------------
# Build prompts: SYSTEM (cacheable) + USER (per-call)
# ---------------------------------------------------------------------------
GATE_SYSTEM_PROMPT=$(cat <<PROMPT
You are a retrieval gate for an Obsidian-backed memory vault.

Your job: given a user message, decide which (if any) existing notes are
worth reading to answer well. You may either pick paths directly from the
overview below, OR specify typed searches we will execute server-side.

OUTPUT FORMAT: a single JSON object on one line. No prose, no code fences.
Schema:
  {
    "read":   ["relative/path1.md", "relative/path2.md"],
    "search": [
      {"type": "decision", "keywords": "auth", "path_prefix": "Projects/foo"},
      {"created_after": "2026-04-21"}
    ]
  }

Both fields are optional; the empty object {} means "no notes are relevant."

Rules:
- Combined cap: at most $PATH_CAP final paths after we merge \`read\` + search
  hits. Fewer is better.
- \`read\`: use ONLY paths visible in the vault overview below. Do not invent.
- \`search\`: each entry may include any subset of these filters (AND-combined):
    "type"           — frontmatter type (e.g., decision, learning, reference)
    "keywords"       — space-separated keywords (matched anywhere in the note)
    "path_prefix"    — relative path prefix (e.g., "Projects/foo")
    "created_after"  — ISO date YYYY-MM-DD (notes with frontmatter created >= this)
    "created_before" — ISO date YYYY-MM-DD (notes with created <= this)
- Use \`search\` when:
    * the user asks for time-bound info ("yesterday", "last week", "this month") — use created_after
    * the user asks for a category that may have grown beyond the overview's bullets
    * you're not sure which specific note matches but the type/keywords are clear
- Use \`read\` when an overview bullet is an obvious match.

=== VAULT OVERVIEW ===
$OVERVIEW
PROMPT
)

GATE_USER_PROMPT="USER MESSAGE:
$USER_MESSAGE

JSON only:"

# ---------------------------------------------------------------------------
# Call the gate
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
# Extract first balanced {...} block, then merge `read` + `search` results.
# This is a single Python invocation that:
#   - parses the gate JSON
#   - executes any typed searches via the _vault module
#   - returns a final, deduped, capped list of paths
# ---------------------------------------------------------------------------
PATHS=$(
  printf '%s' "$GATE_OUTPUT" | \
  PATH_CAP="$PATH_CAP" \
  VAULT_PY="$VAULT_PY" \
  VAULT="$VAULT" \
  python3 -c '
import json, os, re, subprocess, sys

raw = sys.stdin.read()
start = raw.find("{")
if start < 0:
    sys.exit(0)

depth = 0
in_str = False
esc = False
end = -1
for i in range(start, len(raw)):
    c = raw[i]
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
            end = i
            break
if end < 0:
    sys.exit(0)

try:
    obj = json.loads(raw[start:end+1])
except Exception:
    sys.exit(0)

cap = int(os.environ.get("PATH_CAP", "3"))
vault_py = os.environ["VAULT_PY"]
vault = os.environ["VAULT"]

ordered: list[str] = []
seen: set[str] = set()

def add(path: str) -> None:
    p = path.strip()
    if not p or p in seen:
        return
    seen.add(p)
    ordered.append(p)

for p in obj.get("read", []) or []:
    if isinstance(p, str):
        add(p)

for q in obj.get("search", []) or []:
    if not isinstance(q, dict):
        continue
    if len(ordered) >= cap:
        break
    cmd = ["python3", vault_py, "--vault", vault, "search", "--json", "--limit", str(cap)]
    if q.get("type"):           cmd += ["--type", str(q["type"])]
    if q.get("path_prefix"):    cmd += ["--path-prefix", str(q["path_prefix"])]
    if q.get("keywords"):       cmd += ["--keywords", str(q["keywords"])]
    if q.get("created_after"):  cmd += ["--created-after", str(q["created_after"])]
    if q.get("created_before"): cmd += ["--created-before", str(q["created_before"])]
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=15)
        if result.returncode == 0 and result.stdout.strip():
            for hit in json.loads(result.stdout):
                add(hit.get("path", ""))
                if len(ordered) >= cap:
                    break
    except Exception:
        continue

for p in ordered[:cap]:
    print(p)
' 2>>"$LOG"
)

if [ -z "$PATHS" ]; then
  echo "[$(ts)] gate: no paths after merge" >> "$LOG"
  exit 0
fi

# ---------------------------------------------------------------------------
# Validate paths and assemble injection
# ---------------------------------------------------------------------------
TMP=$(mktemp)
trap 'rm -f "$TMP"' EXIT

is_safe_path() {
  local p="$1"
  case "$p" in /*) return 1 ;; esac
  local IFS='/'
  set -f
  for comp in $p; do
    [ "$comp" = ".." ] && return 1
  done
  set +f
  return 0
}

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
    head -c "$NOTE_BYTE_CAP" "$VAULT/$p"
    note_size=$(wc -c < "$VAULT/$p" 2>/dev/null | tr -d ' ' || echo 0)
    if [ "${note_size:-0}" -gt "$NOTE_BYTE_CAP" ]; then
      printf '\n[…truncated at %s bytes; full content via Read of %s]\n' "$NOTE_BYTE_CAP" "$VAULT/$p"
    fi
  } >> "$TMP"
  mark_injected "$p"
  INJECTED=$((INJECTED+1))
done <<< "$PATHS"

[ ${#DROPPED[@]} -gt 0 ] && echo "[$(ts)] gate: dropped paths: ${DROPPED[*]}" >> "$LOG"
[ ${#DUPED[@]} -gt 0 ] && echo "[$(ts)] gate: skipped already-injected this session: ${DUPED[*]}" >> "$LOG"

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
