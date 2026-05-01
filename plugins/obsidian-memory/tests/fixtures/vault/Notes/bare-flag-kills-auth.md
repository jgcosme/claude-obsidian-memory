---
type: learning
description: claude -p --bare disables OAuth/keychain auth; only ANTHROPIC_API_KEY/apiKeyHelper work — use recursion-guard env vars instead
project: example-project
created: 2026-04-27
---

# `claude -p --bare` kills auth

`--bare` strips OAuth and keychain providers, so subprocess `claude -p` calls fail with "Not logged in" for any user without `ANTHROPIC_API_KEY` set. Use recursion-guard env vars (`CLAUDE_MEMORY_GATE=1`) to prevent re-entry instead.
