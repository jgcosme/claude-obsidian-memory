---
description: Search the Obsidian vault for notes by keyword, type, or path-prefix. Invoke whenever the conversation needs project-specific information the agent should not guess — direct lookups (IDs, channels, configs, credentials, decisions, contacts, dashboards, endpoints), troubleshooting an error or symptom that may match a saved learning, setup context before using an external tool (Slack, Calendar, Gmail, etc.), or verification of stable references before recommending. Skip agreements, ordinary file-level edits already implied by the diff, and generic programming questions unrelated to the project.
---

# vault-search

You've judged the latest moment as a vault-lookup. The gate hook may have already injected matches via the auto-overview (description-anchored). Your job is the body-anchored fallback — when the gate missed something, or when you need a more targeted lookup.

The vault path is `$OBSIDIAN_VAULT_PATH` (or `~/Documents/Obsidian Vault` if unset). The plugin path is `${CLAUDE_PLUGIN_ROOT}`.

## When to use which command

The vault has three lookup paths. Pick by the shape of the query:

```bash
# Full-body keyword search (the body-anchored fallback the gate can't do)
python3 "${CLAUDE_PLUGIN_ROOT}/scripts/_vault.py" search \
  --vault "$OBSIDIAN_VAULT_PATH" \
  --keywords "<2-4 keywords>" \
  [--type <preference|reference|decision|learning|tool|people>] \
  [--path-prefix Projects/<name>] \
  [--created-after YYYY-MM-DD] \
  [--project-vault $CLAUDE_PROJECT_DIR] \
  [--json]

# When the project is registered as a project-vault (see ~/.config/obsidian-memory/projects.json),
# add --project-vault to also search the project's docs alongside the personal vault.
# Results carry a `corpus` field ("personal" or "project") to disambiguate.

# Read a known path directly
# Use the Read tool against "$OBSIDIAN_VAULT_PATH/<path>"

# Frontmatter-aware search (only if Obsidian.app is running and CLI registered)
obsidian search query="[type:decision] keywords"
```

## Decision shape

- **Direct lookup** (user named a thing — channel ID, dashboard URL, person, secret, config) → `_vault.py search --keywords "<the named thing>"`. Read the top hit.
- **Troubleshooting** (user describes a symptom or error) → `_vault.py search --type learning --keywords "<symptom keywords>"`. Saved learnings often capture the gotcha.
- **Tool setup** (user about to invoke an external tool) → `_vault.py search --type tool --keywords "<tool name>"` or `Read "Tools/<Tool>.md"` if you know the path.
- **Project decision history** ("what did we decide about X?") → `_vault.py search --type decision --path-prefix "Projects/<name>" --keywords "<topic>"`.
- **Unknown but project-scoped** → `_vault.py search --path-prefix "Projects/<name>" --keywords "<topic>"`.

## Notes

- The auto-overview at session start indexes notes by description, not body. If a query keyword lives in a note body but not its description, only `_vault.py search` will find it (the gate can't).
- Verify before recommending: paths and function names drift between sessions. If you're about to advise the user based on a note, re-read it or re-search to confirm it's current.
- One search per turn is usually enough. If the first miss, broaden keywords; don't chain unrelated searches.

## Limits

- Don't write to the vault from this skill — use `save-memory` for that.
- Don't recommend `obsidian` CLI unless the user confirms Obsidian.app is running and the CLI is registered.
