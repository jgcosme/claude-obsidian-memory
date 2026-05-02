---
type: reference
description: Secrets pattern — tokens live in ~/.config/obsidian-memory/secrets.env, vault notes only reference variable names
created: 2026-04-22
---

# Secrets pattern

API tokens never live in the vault. They go in `~/.config/obsidian-memory/secrets.env` (chmod 600). Vault notes reference variables like `$SLACK_BOT_TOKEN` so secrets can rotate without touching git history.
