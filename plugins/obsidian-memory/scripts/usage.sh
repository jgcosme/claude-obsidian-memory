#!/bin/bash
# Aggregate the current session's usage JSONL and print a summary table.
#
# Locates the per-session log file at /tmp/claude-memory-usage/<id>.jsonl,
# preferring $CLAUDE_SESSION_ID when set, otherwise falling back to the most
# recently modified file in that directory.

set -u

USAGE_DIR="${MEMORY_USAGE_DIR:-/tmp/claude-memory-usage}"

if [ ! -d "$USAGE_DIR" ]; then
  echo "No usage data yet — $USAGE_DIR does not exist."
  echo "(The plugin records per-session usage events as the SessionStart, gate, and SessionEnd hooks fire.)"
  exit 0
fi

# Pick the session log file.
TARGET=""
if [ -n "${CLAUDE_SESSION_ID:-}" ]; then
  safe_id=$(printf '%s' "$CLAUDE_SESSION_ID" | tr -c 'A-Za-z0-9._-' '_')
  if [ -f "$USAGE_DIR/$safe_id.jsonl" ]; then
    TARGET="$USAGE_DIR/$safe_id.jsonl"
  fi
fi
if [ -z "$TARGET" ]; then
  # Newest *.jsonl by mtime
  TARGET=$(ls -t "$USAGE_DIR"/*.jsonl 2>/dev/null | head -1)
fi

if [ -z "$TARGET" ] || [ ! -f "$TARGET" ]; then
  echo "No usage data yet for any session."
  echo "(Looked in $USAGE_DIR. The SessionStart hook writes the first event of each session.)"
  exit 0
fi

python3 - "$TARGET" <<'PY'
import json, sys, os, glob
from collections import defaultdict
from datetime import datetime, timezone

path = sys.argv[1]

def parse_ts(s):
    """Parse ISO timestamp from either plugin log (UTC-Z) or main transcript
    (UTC-Z with millis). For legacy plugin entries written before the UTC
    migration (naive, no Z), treat as local time and convert to UTC so they
    can be compared with transcript timestamps."""
    if not s:
        return None
    if s.endswith("Z"):
        s = s[:-1] + "+00:00"
    try:
        dt = datetime.fromisoformat(s)
    except ValueError:
        return None
    if dt.tzinfo is None:
        # Naive datetime — assume local time, convert to UTC.
        # .astimezone() on a naive dt interprets it as local in Python 3.
        dt = dt.astimezone(timezone.utc)
    return dt

def find_transcript(session_id):
    """Locate the main session transcript jsonl. Claude Code stores it at
    ~/.claude/projects/<encoded-cwd>/<session_id>.jsonl; we glob across all
    project dirs since we don't know which cwd the session ran in."""
    if not session_id:
        return None
    pattern = os.path.expanduser(f"~/.claude/projects/*/{session_id}.jsonl")
    matches = glob.glob(pattern)
    return matches[0] if matches else None

events = []
with open(path) as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        try:
            events.append(json.loads(line))
        except Exception:
            continue

if not events:
    print(f"No parseable events in {path}")
    sys.exit(0)

# Bucket by kind
by_kind = defaultdict(list)
for e in events:
    by_kind[e.get("kind", "unknown")].append(e)

CHARS_KINDS = ("session_start", "gate_inject")
API_KINDS = ("gate_call", "review_call")

def fmt_int(n):
    return f"{n:,}"

print("\033[1mObsidian-memory plugin token usage\033[0m")
session_id = os.path.basename(path).removesuffix(".jsonl")
print(f"  session: {session_id}")
print(f"  events:  {len(events)}")
print()

# Per-event-kind blocks, in display order: injected first, then API.
# Tag each block so the conceptual difference (one-time vs re-billed each turn)
# is visible even though the two categories share one flow.
ORDER = (
    ("session_start", "injected"),
    ("gate_inject",   "injected"),
    ("gate_call",     "api"),
    ("review_call",   "api"),
)

inj_tok_total = 0
api_in = api_out = api_cache_r = api_cache_w = 0
api_calls = 0

for kind, category in ORDER:
    items = by_kind.get(kind, [])
    n = len(items)
    suffix = "" if n == 1 else "s"
    if category == "injected":
        if n == 0:
            print(f"  \033[2m{kind:<14}\033[0m [injected]  0 events")
            continue
        chars = sum(int(e.get("chars", 0) or 0) for e in items)
        tok = sum(int(e.get("approx_tokens", 0) or 0) for e in items)
        inj_tok_total += tok
        print(f"  {kind:<14} [injected]  {n} event{suffix}, {fmt_int(chars)} chars (~{fmt_int(tok)} tok)")
    else:  # api
        if n == 0:
            note = "fires at SessionEnd" if kind == "review_call" else "no calls yet"
            print(f"  \033[2m{kind:<14}\033[0m [api]       0 calls ({note})")
            continue
        in_tok = sum(int(e.get("usage", {}).get("input_tokens", 0) or 0) for e in items)
        out_tok = sum(int(e.get("usage", {}).get("output_tokens", 0) or 0) for e in items)
        cache_r = sum(int(e.get("usage", {}).get("cache_read_input_tokens", 0) or 0) for e in items)
        cache_w = sum(int(e.get("usage", {}).get("cache_creation_input_tokens", 0) or 0) for e in items)
        api_in += in_tok
        api_out += out_tok
        api_cache_r += cache_r
        api_cache_w += cache_w
        api_calls += n
        print(f"  {kind:<14} [api]       {n} call{suffix}")
        print(f"    input:    {fmt_int(in_tok)}")
        print(f"    output:   {fmt_int(out_tok)}")
        print(f"    cache_r:  {fmt_int(cache_r)}")
        print(f"    cache_w:  {fmt_int(cache_w)}")
print()

# Two totals, kept distinct because they meter differently against your pool.
print("\033[1mTotals\033[0m")
print(f"  injected  (re-sent each turn — mostly cache_read after first turn):")
print(f"    {fmt_int(inj_tok_total)} tok per turn")
print(f"  api       (one-time consumption from plugin's claude -p calls):")
print(f"    input:    {fmt_int(api_in)}")
print(f"    output:   {fmt_int(api_out)}")
print(f"    cache_r:  {fmt_int(api_cache_r)}")
print(f"    cache_w:  {fmt_int(api_cache_w)}")
plugin_separate = api_in + api_out + api_cache_r + api_cache_w
print(f"    sum:      {fmt_int(plugin_separate)}")
print()

# --- Session share section ---------------------------------------------------
# Plugin's share of THIS session's tokens. /usage's 33% bar tracks this same
# session's weighted consumption against the 5h pool, so this share × /usage%
# = plugin's contribution to your quota.
#
# Math:
#   plugin_main_attr  = Σ over chars events  approx_tokens × turns_alive
#                        where turns_alive = transcript msgs with ts >= event.ts
#                        (counted via .message.id dedup so we don't double-count
#                         the snapshot/replay events Claude Code emits)
#   plugin_total      = plugin_main_attr + plugin_separate
#   total_session     = main_session_total + plugin_separate    (no double-count)
#   plugin_share      = plugin_total / total_session
session_id = os.path.basename(path).removesuffix(".jsonl")
transcript = find_transcript(session_id)

print("\033[1mSession share\033[0m  (this session's tokens vs plugin overhead)")
if transcript:
    main_msgs_ts = []
    main_total = {"input": 0, "output": 0, "cache_r": 0, "cache_w": 0}
    # Dedup by .message.id — Claude Code's session JSONL re-emits the same
    # assistant message multiple times (snapshot/replay events). Counting all
    # would over-attribute by ~2× compared to /usage's reported numbers.
    seen_msg_ids = set()
    with open(transcript) as f:
        for line in f:
            try:
                e = json.loads(line)
            except Exception:
                continue
            msg = e.get("message") or {}
            usage = msg.get("usage")
            if not usage:
                continue
            mid = msg.get("id")
            if mid:
                if mid in seen_msg_ids:
                    continue
                seen_msg_ids.add(mid)
            t = parse_ts(e.get("timestamp", ""))
            if t is not None:
                main_msgs_ts.append(t)
            main_total["input"]   += int(usage.get("input_tokens", 0) or 0)
            main_total["output"]  += int(usage.get("output_tokens", 0) or 0)
            main_total["cache_r"] += int(usage.get("cache_read_input_tokens", 0) or 0)
            main_total["cache_w"] += int(usage.get("cache_creation_input_tokens", 0) or 0)
    main_sum = sum(main_total.values())
    n_msgs = len(main_msgs_ts)

    plugin_main_attr = 0
    for kind in CHARS_KINDS:
        for ev in by_kind.get(kind, []):
            tok = int(ev.get("approx_tokens", 0) or 0)
            ts = parse_ts(ev.get("ts", ""))
            if ts is None:
                turns_alive = n_msgs  # fallback: assume injected for full session
            else:
                turns_alive = sum(1 for mt in main_msgs_ts if mt >= ts)
            plugin_main_attr += tok * turns_alive

    plugin_total = plugin_main_attr + plugin_separate
    total_session = main_sum + plugin_separate
    share = (plugin_total / total_session * 100) if total_session > 0 else 0.0

    print(f"  main session:    {fmt_int(main_sum)} tok ({n_msgs} assistant msgs)")
    print(f"    plugin-attributable (injection × turns): ~{fmt_int(plugin_main_attr)}")
    print(f"  plugin separate: {fmt_int(plugin_separate)} tok (gate + review)")
    print(f"  ─────────")
    print(f"  total session:   {fmt_int(total_session)} tok")
    print(f"  \033[1mplugin share:    {share:.1f}%\033[0m")
    print()
    print("\033[2m  Multiply plugin share by Claude's /usage % to estimate the\033[0m")
    print("\033[2m  plugin's pts of your rate-limit quota for this session.\033[0m")
else:
    print(f"  (transcript not found at ~/.claude/projects/*/{session_id}.jsonl —")
    print(f"   could not compute session share)")
print()
print("\033[2m  Tokens meter against your Claude rate-limit pool, not your wallet —\033[0m")
print("\033[2m  subscriptions cover usage within rate limits.\033[0m")
PY
