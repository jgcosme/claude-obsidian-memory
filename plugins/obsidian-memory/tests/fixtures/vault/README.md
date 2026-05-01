---
type: reference
description: "Description of the synthetic fixture vault used by gate-prompt evaluation tests"
created: 2026-04-30
project: claude-obsidian-memory
---

# Fixture vault

Synthetic vault used by `tests/run_gate_eval.py` to evaluate the retrieval-gate
system prompt. Notes are fictional; descriptions are crafted so each is
distinguishable from the others by frontmatter alone.

Layout matches the v1.10.0 single-path architecture:

- `Tools/` — pan-vault CLI/API/service notes
- `Notes/` — everything else, scoped by `project:` frontmatter (no folder
  hierarchy)
- `Journals/` — one note per session, also scoped by `project:`

The "current project" for tests is `example-project`. Cases that exercise
cross-project / general-vault retrieval target notes without a `project:`
field (`secrets-env`, `vault-import-conventions`, `user`).

Today's date for fixture purposes: 2026-04-29 (matches the latest journal).
