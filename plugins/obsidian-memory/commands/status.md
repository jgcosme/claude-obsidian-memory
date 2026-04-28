---
description: Check obsidian-memory plugin health (config, vault, scripts, gate, recent activity)
---

Run the plugin status script and summarize the result for the user:

```bash
bash "$CLAUDE_PLUGIN_ROOT/scripts/status.sh"
```

Report any `[FAIL]` or `[warn]` lines as items needing attention. For each one, briefly explain what it means and how to fix it (e.g., `[FAIL] vault not found` → run `bash "$CLAUDE_PLUGIN_ROOT/scripts/setup.sh"`).

If the script exits 0 and there are no warnings, simply say everything looks healthy.
