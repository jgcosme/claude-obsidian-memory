---
type: reference
description: "Obsidian Memory vault README — how Claude's persistent memory is organized"
created: __TODAY__
---

# Obsidian Memory

This vault is Claude's persistent memory, owned entirely by the obsidian-memory plugin. The plugin reads it at SessionStart, writes to it from the save-memory skill and the SessionEnd review, and never reaches outside this directory.

## Layout

- **`Tools/`** — CLIs, APIs, services. One note per tool. Browsed by name.
- **`Journals/`** — one note per session, written by SessionEnd.
- **`Notes/`** — everything else: preferences, references, decisions, learnings. Searched by frontmatter.

Project scoping is via the `project:` frontmatter field on individual notes — there's no `Projects/<name>/` wrapper. A `Notes/auth-decision.md` with `project: my-app` belongs to that project; the same note without `project:` is cross-project.

## Frontmatter

Every note (except README files) has YAML frontmatter:

```yaml
---
type: preference | reference | decision | learning | tool | journal
description: "one-line hook"
created: YYYY-MM-DD
project: <project-name>            # only when project-scoped
---
```

Six types. The plugin builds Claude's auto-overview by walking these frontmatter blocks each session.

## Federated project-vaults

Project repos can be registered as a "project-vault" — a second corpus searched alongside this one. Per-project opt-in via SessionStart's registration prompt or `/obsidian-memory:project enable`. Registry lives at `~/.config/obsidian-memory/projects.json`.

## Querying directly

```bash
python3 "$CLAUDE_PLUGIN_ROOT/scripts/_vault.py" search \
  --type learning --created-after "$(date -v-1d +%Y-%m-%d)"
```

If Obsidian.app is running and its CLI is registered, `obsidian search` also works with bracket-syntax queries (`[type:decision]`).
