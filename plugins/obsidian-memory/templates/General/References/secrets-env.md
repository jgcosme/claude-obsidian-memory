---
type: reference
description: Secrets pattern — tokens live in ~/.config/claude-memory/secrets.env, vault notes only reference variable names
created: __TODAY__
---

# Secrets pattern

## Rule
Never commit credentials to the Obsidian vault. The vault is meant to be git-tracked — possibly pushed to a remote, possibly shared — so anything written there is pushable. Keep secrets in a separate, gitignored file.

## Where secrets live
- File: `~/.config/claude-memory/secrets.env`
- Permissions: `chmod 600`
- Format: shell-sourceable `export NAME="value"` lines, with comments above each describing scope, expiry, and owner.

Example:
```bash
# Slack user token — workspace foo, scopes: channels:read+chat:write, expiry: none
export SLACK_USER_TOKEN="xoxp-..."

# OpenAI API key — project bar, billing-capped at $50/mo
export OPENAI_API_KEY="sk-..."
```

## How tool notes reference secrets
In `Tools/<tool>.md`, document:
- The variable name (e.g. `SLACK_USER_TOKEN`)
- That it lives in `~/.config/claude-memory/secrets.env`
- A `source ~/.config/claude-memory/secrets.env` line before any example commands
- Use `$VAR_NAME` in command examples — never paste the literal value

## Adding a new secret
1. Append to `~/.config/claude-memory/secrets.env` (keep `chmod 600`).
2. Update the relevant `Tools/<tool>.md` to reference the variable name.
3. `grep` the vault before committing to confirm the literal value never landed in any note:
   ```bash
   grep -rE "(xoxp-|xoxb-|sk-[A-Za-z0-9]|gho_|ghp_|github_pat_|AKIA|AIza|AIzaSy)" "__VAULT_PATH__"
   ```

## Why this pattern
A vault with secrets in plaintext is one accidental `git push` away from a public leak. The split-file approach makes it *structurally impossible* to leak a secret by editing a note: if a value isn't in any tracked file, it can't be pushed. Vault notes only ever name the variable; the literal value lives outside the repo.
