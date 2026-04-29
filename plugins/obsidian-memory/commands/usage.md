---
description: Show this session's obsidian-memory plugin token consumption
---

Run the plugin usage script:

```bash
bash "${CLAUDE_PLUGIN_ROOT:-$(ls -d ~/.claude/plugins/cache/jgcosme-plugins/obsidian-memory/*/ 2>/dev/null | tail -1 | sed 's:/$::')}/scripts/usage.sh"
```

The script lists each event kind with an `[injected]` or `[api]` tag, then totals at the bottom split by category. Tokens meter against the user's Claude rate-limit pool, not their wallet — subscriptions cover usage within rate limits.

If the user asks for context, briefly explain the four event kinds:
- `session_start` — bootstrap context (vault overview, instructions) emitted at session start; **injected** (re-sent each turn)
- `gate_inject` — vault notes the retrieval gate pulls in on a given prompt; **injected** (re-sent each turn)
- `gate_call` — the retrieval gate's own `claude -p` call on every UserPromptSubmit; **api** (one-time per call)
- `review_call` — the SessionEnd journal/memory review's `claude -p` call; **api** (one-time, only appears after SessionEnd fires)

If there is no usage data yet (very fresh session, or no SessionStart event was logged), the script will say so — that is expected, not an error.
