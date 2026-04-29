---
description: Run a full vault integrity audit (frontmatter, broken wikilinks, orphans, duplicate basenames, repo-pointer drift). Pass --deep to also flag description-vs-body drift via an LLM pass.
---

Arguments: $ARGUMENTS

## Step 1 — structural audit (always)

Run the deterministic audit script. This works whether or not `$CLAUDE_PLUGIN_ROOT` is set:

```bash
python3 "${CLAUDE_PLUGIN_ROOT:-$(ls -d ~/.claude/plugins/cache/jgcosme-plugins/obsidian-memory/*/ 2>/dev/null | tail -1 | sed 's:/$::')}/scripts/audit.py"
```

The script reports vault-wide checks and — when run from inside a project that has a corresponding `Projects/<cwd-basename>/` folder in the vault — also a repo-pointer audit:

- **Vault-wide (always):** frontmatter completeness, broken `[[wikilinks]]`, orphan notes, duplicate basenames.
- **Repo-pointer (when the project matches):** vault notes whose `source:` no longer exists in the repo (broken pointer sources), and repo `*.md` files with no vault pointer (unreferenced repo docs). Boilerplate filenames (`LICENSE*`, `CHANGELOG.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`, PR/ISSUE templates) and top-level dotfile dirs (`.github/`, `.cursor/`, `.vscode/`, `.devcontainer/`, …) are filtered out automatically. Untracked-but-not-gitignored docs are included.

If the script reports issues, group them by category and call out the highest-impact ones first (broken wikilinks > broken pointer sources > missing frontmatter > unreferenced repo docs > orphans > duplicate basenames). For each issue, suggest the smallest fix — auto-fixes aren't applied, so the user has to act.

If the audit comes back clean (exit 0, no issues), say so plainly.

## Step 2 — deep description-drift pass (only if `$ARGUMENTS` contains `--deep`)

Goal: flag notes whose frontmatter `description` no longer accurately summarizes the body.

1. Resolve the vault path: `$OBSIDIAN_VAULT_PATH` from `~/.config/claude-memory/config.env` if set, else `~/Documents/Obsidian Vault`.
2. Enumerate candidate notes:
   ```bash
   find "$VAULT" -type f -name '*.md' -not -name 'README.md'
   ```
3. For each note, read it and judge: does the frontmatter `description` field still capture what the body is about? Account for substantial appends, scope shifts, or (for journals) sessions added after the description was written. Skip notes with no `description`.
4. Report findings under `## Description drift` with one entry per drifted note:
   - path
   - current `description`
   - suggested replacement (one line, ≤120 chars)
   - one-line rationale for why it drifted
5. **Do not auto-fix.** List only — the user decides which to apply. Offer to apply selected fixes if they ask.

If no drift is found, say so plainly and skip the section.
