---
description: Check obsidian-memory plugin health (config, vault, scripts, gate, recent activity)
---

Run the plugin status command and summarize the result for the user. Works whether or not `$CLAUDE_PLUGIN_ROOT` is set in the calling shell:

```bash
"${CLAUDE_PLUGIN_ROOT:-$(ls -td ~/.claude/plugins/cache/jgcosme-plugins/obsidian-memory/*/ 2>/dev/null | head -1 | sed 's:/$::')}/bin/run" status
```

Report any `[FAIL]` or `[warn]` lines as items needing attention. For each one, briefly explain what it means and how to fix it (e.g., `[FAIL] vault not found` → run `"$CLAUDE_PLUGIN_ROOT/bin/run" setup`).

If the command exits 0 and there are no warnings, simply say everything looks healthy.
