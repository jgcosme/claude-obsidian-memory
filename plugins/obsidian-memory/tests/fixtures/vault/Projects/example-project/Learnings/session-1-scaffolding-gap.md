---
type: learning
description: On session 1 of a brand-new cwd, the auto-overview and retrieval gate run before scaffolding — they don't see the project's freshly-created notes
project: example-project
created: 2026-04-28
---

# Session 1 scaffolding gap

`SessionStart` runs `_overview.sh` before scaffolding instructions are appended, so on the first session of a brand-new cwd the gate sees a vault overview that excludes the project notes about to be created.
