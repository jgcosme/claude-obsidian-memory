---
type: readme
description: Vault README — how Claude's persistent memory is organized
created: __TODAY__
---

# Obsidian Memory Vault

This vault is Claude's persistent memory. The plugin reads it at SessionStart and writes to it at SessionEnd.

## Layout

- **`Tools/`** — CLIs, APIs, and tools Claude can use. Each note is one tool.
- **`General/`** — cross-project knowledge.
  - `Preferences/` — coding/communication style, validated approaches.
  - `People/` — colleagues, contacts.
  - `Admin/` — recurring tasks, accounts, processes.
  - `References/` — cross-cutting external systems.
  - `user.md` — your profile (identity, role, current focus).
- **`Projects/<name>/`** — per-project memory (one folder per cwd basename).
  - `overview.md` — what the project is, goals, status.
  - `Decisions/` — choices with rationale (ADRs, design decisions).
  - `Learnings/` — gotchas, runbooks, "how X actually works".
  - `Research/` — investigations, options compared, RFCs.
  - `References/` — project-specific external pointers.
  - `Journal/YYYY-MM-DD.md` — written by SessionEnd.

## Frontmatter convention

Every note (except README files) has YAML frontmatter:

```yaml
---
type: tool | user | preference | reference | decision | learning | research | overview | journal | people | admin
description: one-line hook (what's in this note)
created: YYYY-MM-DD
project: <project-name>            # only for project-scoped notes
---
```

The plugin builds Claude's vault overview at SessionStart by walking these
frontmatter blocks. Adding/renaming notes shows up automatically in the next
session.

## Recall

The retrieval gate runs on every user message and decides what (if any) vault
notes are relevant — by reading the auto-generated overview and, when needed,
running typed searches. You can also query the vault directly:

```bash
# Yesterday's learnings across all projects
python3 "$CLAUDE_PLUGIN_ROOT/scripts/_vault.py" search \
  --type learning --created-after "$(date -v-1d +%Y-%m-%d)"

# All decisions for a specific project
python3 "$CLAUDE_PLUGIN_ROOT/scripts/_vault.py" search \
  --type decision --path-prefix "Projects/foo"
```

If Obsidian.app is running and the CLI is registered, `obsidian search` is
also available with bracket-syntax queries (`[type:decision]`, `path:Projects/foo`).
