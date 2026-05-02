#!/bin/bash
# Append a usage event to /tmp/claude-memory-usage/<session_id>.jsonl.
# Used by all three hooks to track plugin token / cost overhead per session.
#
# Two call shapes:
#
#   API call (real usage from `claude -p --output-format json`):
#     _usage_log.sh api <session_id> <kind> <usage_json> <cost_usd> <duration_ms>
#       <usage_json> is the .usage object from the result event (a single-line
#       JSON object). <cost_usd> and <duration_ms> may be empty strings.
#
#   Injected text (no API call, just stdout into the user's main session):
#     _usage_log.sh chars <session_id> <kind> <byte_count>
#       approx_tokens is recorded as ceil(bytes/4).
#
# `kind` is free-form but the /obsidian-memory:usage script aggregates by:
#   session_start | gate_call | gate_inject | review_call

set -u

USAGE_DIR="${MEMORY_USAGE_DIR:-/tmp/claude-memory-usage}"
mkdir -p "$USAGE_DIR" 2>/dev/null || exit 0

mode="${1:-}"
session_id="${2:-}"
kind="${3:-}"

[ -z "$mode" ] && exit 0
[ -z "$session_id" ] && exit 0
[ -z "$kind" ] && exit 0

# Sanitize session id for filesystem
safe_id=$(printf '%s' "$session_id" | tr -c 'A-Za-z0-9._-' '_')
out="$USAGE_DIR/$safe_id.jsonl"

# UTC ISO 8601 with Z so timestamps align with the main session transcript
# (~/.claude/projects/.../<session_id>.jsonl), which uses the same format.
# The session-share section in usage.sh compares plugin event ts to transcript
# message ts; mixing local + UTC would mis-attribute injection turns.
ts=$(date -u '+%Y-%m-%dT%H:%M:%SZ')

case "$mode" in
  api)
    usage_json="${4:-{\}}"
    cost="${5:-}"
    duration="${6:-}"
    # Validate the usage JSON is parseable; if not, store as empty object.
    if ! printf '%s' "$usage_json" | jq -e . >/dev/null 2>&1; then
      usage_json='{}'
    fi
    cost_field='null'
    if [ -n "$cost" ]; then
      # numeric check
      if printf '%s' "$cost" | grep -Eq '^-?[0-9]+(\.[0-9]+)?$'; then
        cost_field="$cost"
      fi
    fi
    duration_field='null'
    if [ -n "$duration" ]; then
      if printf '%s' "$duration" | grep -Eq '^[0-9]+$'; then
        duration_field="$duration"
      fi
    fi
    # Build the event with jq so we get safe escaping of the kind/ts strings.
    jq -nc \
      --arg ts "$ts" \
      --arg kind "$kind" \
      --argjson usage "$usage_json" \
      --argjson cost "$cost_field" \
      --argjson duration "$duration_field" \
      '{ts:$ts, kind:$kind, mode:"api", usage:$usage, cost_usd:$cost, duration_ms:$duration}' \
      >> "$out" 2>/dev/null || true
    ;;
  chars)
    bytes="${4:-0}"
    if ! printf '%s' "$bytes" | grep -Eq '^[0-9]+$'; then
      bytes=0
    fi
    approx=$(( (bytes + 3) / 4 ))
    jq -nc \
      --arg ts "$ts" \
      --arg kind "$kind" \
      --argjson chars "$bytes" \
      --argjson approx "$approx" \
      '{ts:$ts, kind:$kind, mode:"chars", chars:$chars, approx_tokens:$approx}' \
      >> "$out" 2>/dev/null || true
    ;;
  *)
    exit 0
    ;;
esac

exit 0
