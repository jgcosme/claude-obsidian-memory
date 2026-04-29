---
description: Run a full vault integrity audit (frontmatter, broken wikilinks, orphans, duplicate basenames)
---

Run the audit script and summarize the results for the user. Use this command — it works whether or not `$CLAUDE_PLUGIN_ROOT` is set in the calling shell:

```bash
python3 "${CLAUDE_PLUGIN_ROOT:-$(ls -d ~/.claude/plugins/cache/jgcosme-plugins/obsidian-memory/*/ 2>/dev/null | tail -1 | sed 's:/$::')}/scripts/audit.py"
```

If the script reports issues, group them by category and call out the highest-impact ones first (broken wikilinks > missing frontmatter > orphans > duplicate basenames). For each issue, suggest the smallest fix — auto-fixes aren't applied, so the user has to act.

If the audit comes back clean (exit 0, no issues), say so plainly.
