---
type: decision
description: Use SessionStart + SessionEnd hooks (not CLAUDE.md autoload) to drive the memory system
project: example-project
created: 2026-04-26
---

# Hooks vs CLAUDE.md autoload

Decision: drive memory bootstrapping with SessionStart/SessionEnd hooks rather than embedding the vault index in `CLAUDE.md`. Hooks let us regenerate the index per session and inject only the slice relevant to the current cwd.
