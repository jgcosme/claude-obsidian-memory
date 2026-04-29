---
type: learning
description: $CLAUDE_PLUGIN_ROOT is not exported into the Bash tool subprocess when a slash-command body runs — self-locate scripts
project: example-project
created: 2026-04-27
---

# `$CLAUDE_PLUGIN_ROOT` not in slash-command shell

Slash-command bodies run in a subprocess that doesn't inherit `$CLAUDE_PLUGIN_ROOT`. Scripts must self-locate via `$0` or use a glob fallback, e.g. `${CLAUDE_PLUGIN_ROOT:-$(ls -d ~/.claude/plugins/cache/*/ | tail -1)}`.
