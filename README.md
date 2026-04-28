# obsidian-memory

A Claude Code plugin that turns an Obsidian vault into Claude's persistent memory. Vault content is loaded into context at session start; session-end runs an automated review that journals what happened and proactively writes new notes when something significant was learned. All writes are git-trackable, so nothing changes without a diff you can review.

## Why

Claude Code has a per-project auto-memory directory (`~/.claude/projects/*/memory/`), but it's siloed by project, not browsable, and not editable in Obsidian's UI. This plugin replaces it with an Obsidian-backed system that:

- Loads only **index files** at session start (small payload, deep recall via `obsidian search`).
- Scopes per-project memory by `cwd` basename, while keeping cross-project knowledge (tools, identity, preferences, people) always loaded.
- Writes new memory automatically at session end, with a dedup check (`obsidian search`) to avoid redundant notes.
- Is git-tracked: every memory write is a diff. Auto-commit is on by default; auto-push is opt-in.

## Prerequisites

| Tool | Why | Install |
|---|---|---|
| **Claude Code** ≥ 2.0 | runs the plugin | <https://docs.claude.com/en/docs/claude-code/setup> |
| **Obsidian** desktop app | the vault is an Obsidian vault | <https://obsidian.md> |
| **Obsidian CLI** registered | the plugin uses `obsidian search`, `obsidian read`, etc. | Open Obsidian → Settings → General → "Command line interface" → Register CLI |
| **`jq`** | parses the SessionEnd payload | `brew install jq` (macOS) / your package manager |
| **`git`** | for vault history (optional but strongly recommended) | preinstalled on most systems |
| **`gh`** *(optional)* | only if you want to push the vault to GitHub | <https://cli.github.com> |

**Platform**: tested on macOS. Should work on Linux with Obsidian installed and the CLI on `$PATH`. Obsidian.app must be running for the CLI to respond — the SessionStart hook tries to launch it but you may need to grant Accessibility permissions on first run.

## Install

The plugin lives in this repository, which doubles as a marketplace via `.claude-plugin/marketplace.json`.

```text
# 1. Add this repo as a marketplace
/plugin marketplace add jgcosme/claude-obsidian-memory

# 2. Install the plugin
/plugin install obsidian-memory@jgcosme-plugins

# 3. Restart hooks
/reload-plugins
```

## First-time setup

After install, run the setup script to scaffold the vault, config file, and secrets file:

```bash
bash "$CLAUDE_PLUGIN_ROOT/scripts/setup.sh"
```

This is **idempotent** — re-running it won't overwrite existing files. It creates:

- `~/Documents/Obsidian Vault/` (or whatever `OBSIDIAN_VAULT_PATH` points at)
  - `INDEX.md`, `Tools/INDEX.md`, `General/INDEX.md`, `General/user.md`
  - `Projects/` (empty; per-project folders added on demand)
  - `.gitignore` for Obsidian state files
- `~/.config/claude-memory/config.env` — paths and behavior toggles
- `~/.config/claude-memory/secrets.env` — empty template, `chmod 600`

**Recommended next step:** `git init` the vault.

```bash
cd "$HOME/Documents/Obsidian Vault"
git init -b main && git add -A && git commit -m "Initial commit"
# Optionally push to a private repo:
gh repo create my-obsidian-vault --private --source . --remote origin --push
```

Once the vault is a git repo, the SessionEnd hook auto-commits all memory writes.

## How it works

### SessionStart hook

Every new Claude session triggers `hooks/scripts/session-start.sh`, which:

1. Opens Obsidian.app (so the CLI is responsive).
2. Derives the **project name** from `$CLAUDE_PROJECT_DIR` basename (or `$PWD`).
3. Injects into context:
   - The root `INDEX.md` (entry point + organization rules)
   - `Tools/INDEX.md` (cross-cutting tool reference)
   - `General/INDEX.md` (identity, preferences, people, admin, references)
   - `Projects/<project-name>/INDEX.md` if it exists (project-scoped recall)
   - **Usage instructions** telling Claude how/when to read and write memory

Claude only sees indexes — small, dense pointers. Deep recall happens on demand via `obsidian search` and `obsidian read`. Total injection is typically 3–6 KB.

### SessionEnd hook

When a session ends, `hooks/scripts/session-end.sh` backgrounds a `claude -p` subprocess that:

1. Reads the transcript.
2. **Always** writes a journal entry to `Projects/<project>/Journal/YYYY-MM-DD.md`. If the file already exists for today (multiple sessions), appends a `## Session HH:MM` section.
3. **Proactively** writes new notes when ALL of these hold:
   - the information is significant (correction, validated approach, decision, novel fact),
   - it will be useful in future sessions,
   - **and** no existing note already covers it (verified via `obsidian search`).
4. **Modifies** existing notes only on **explicit user correction** in the transcript — not on inference. If the transcript merely *suggests* a note might be stale, the review flags it for the next session instead of editing silently.
5. After all writes, `git add -A && git commit` if the vault is a git repo (`OBSIDIAN_MEMORY_AUTOCOMMIT=true` by default). Push is opt-in (`OBSIDIAN_MEMORY_AUTOPUSH=true`).

The hook returns immediately so it doesn't block your shell; the review runs in the background and logs to `/tmp/claude-memory-review.log`.

### Recursion guard

The SessionEnd review uses `claude -p`, which itself triggers a SessionStart hook (fine — it loads vault context for the review) and would otherwise trigger another SessionEnd (bad — infinite loop). The hook sets `CLAUDE_MEMORY_REVIEW=1` for the subprocess; the SessionEnd script exits early when that env var is set.

## Vault structure

```
Obsidian Vault/
├── INDEX.md                       — root index (always loaded)
├── Tools/                         — CLI/API/tool reference (always loaded)
│   ├── INDEX.md
│   └── <tool>.md                  — frontmatter: type=tool
├── General/                       — cross-project (always loaded)
│   ├── INDEX.md
│   ├── user.md                    — your profile
│   ├── Preferences/               — coding/communication style, validated approaches
│   ├── People/                    — colleagues, contacts
│   ├── Admin/                     — recurring tasks, accounts, processes
│   └── References/                — cross-cutting external systems
└── Projects/<name>/               — per-project (loaded by cwd basename)
    ├── INDEX.md
    ├── overview.md                — what the project is, goals, status
    ├── Journal/YYYY-MM-DD.md      — written by SessionEnd
    ├── Decisions/                 — choice + rationale
    ├── Learnings/                 — gotchas, "how X actually works"
    ├── Research/                  — investigations, options compared
    └── References/                — project-specific external pointers
```

### Frontmatter convention

Every note has YAML frontmatter:

```yaml
---
type: tool | user | preference | people | admin | reference | overview | journal | decision | learning | research | index
description: one-line hook (what's in this note)
created: YYYY-MM-DD
project: <project-name>            # only for project-scoped notes
---
```

This enables typed recall via the Obsidian CLI's bracket syntax:

```bash
# Yesterday's learnings across all projects
YESTERDAY=$(date -v-1d +%Y-%m-%d)
obsidian search query="path:Projects [type:learning] [created:$YESTERDAY]"

# All decisions for project X
obsidian search query="path:Projects/foo [type:decision]"

# Cross-project user preferences
obsidian search query="path:General/Preferences"
```

## Configuration

Edit `~/.config/claude-memory/config.env` to override defaults:

| Variable | Default | Purpose |
|---|---|---|
| `OBSIDIAN_VAULT_PATH` | `$HOME/Documents/Obsidian Vault` | absolute path to your vault |
| `OBSIDIAN_CLI` | auto-detected | path to the Obsidian CLI binary |
| `CLAUDE_BIN` | auto-detected via `$PATH` | path to the `claude` binary used by SessionEnd review |
| `MEMORY_REVIEW_LOG` | `/tmp/claude-memory-review.log` | where SessionEnd review logs go |
| `OBSIDIAN_MEMORY_AUTOCOMMIT` | `true` | git commit vault changes after review (no-op if vault isn't a git repo) |
| `OBSIDIAN_MEMORY_AUTOPUSH` | `false` | push after auto-commit |

## Secrets

Never paste credentials directly into vault notes — the vault is meant to be git-tracked, possibly pushed to a remote, and shared as memory across sessions. Instead:

1. Add the secret to `~/.config/claude-memory/secrets.env`:
   ```bash
   export SLACK_USER_TOKEN="xoxp-..."
   ```
2. Reference it in the relevant `Tools/<tool>.md` note by **variable name only**:
   ```markdown
   - Token: stored in `~/.config/claude-memory/secrets.env` as `SLACK_USER_TOKEN`. Source the file before use.
   ```
3. Use `$SLACK_USER_TOKEN` in command examples — never the literal value.

The setup script chmod-600s the secrets file. If you ever need to verify the vault is clean before pushing:

```bash
git -C "$HOME/Documents/Obsidian Vault" log --all -p | grep -E "(xoxp-|sk-|gho_|ghp_)" | head
```

## Adding a new project

When you start work in a new project directory:

```bash
# Replace <name> with your project's directory basename
cd "$HOME/Documents/Obsidian Vault/Projects"
mkdir -p "<name>"/{Journal,Decisions,Learnings,Research,References}

# Use the templates as a starting point
sed "s/__PROJECT_NAME__/<name>/g; s/__TODAY__/$(date +%Y-%m-%d)/g" \
  "$CLAUDE_PLUGIN_ROOT/templates/Projects/PROJECT_NAME/INDEX.md" \
  > "<name>/INDEX.md"
sed "s/__PROJECT_NAME__/<name>/g; s/__TODAY__/$(date +%Y-%m-%d)/g" \
  "$CLAUDE_PLUGIN_ROOT/templates/Projects/PROJECT_NAME/overview.md" \
  > "<name>/overview.md"
```

Then `cd` into the project dir (so cwd basename matches) and start a Claude session — the SessionStart hook will pick up the new project automatically.

## Troubleshooting

**SessionStart hook doesn't seem to load anything**
- Confirm `~/.config/claude-memory/config.env` points at a real vault.
- Check the vault has at least an `INDEX.md` — re-run `setup.sh` if not.
- Confirm Obsidian.app is running and the CLI is registered (Settings → General → Command line interface).

**SessionEnd review didn't run / didn't write a journal**
- Check `/tmp/claude-memory-review.log`. If it says "skipped: no Projects/<name>/ folder", scaffold that project (see "Adding a new project").
- If it says "no transcript at ''", check that `jq` is installed.
- If you don't see any log at all, the hook may not be registered — try `/reload-plugins` and restart your session.

**Reviews are too aggressive / writing too many notes**
- The dedup check (`obsidian search` before write) usually catches duplicates. If something slips through, delete the note and `git commit`. The next review will see the deletion in history and avoid re-creating.
- For finer control, edit the prompt in `hooks/scripts/session-end.sh` (search for `PROACTIVE NOTES`).

**I want to disable auto-commit**
- Set `OBSIDIAN_MEMORY_AUTOCOMMIT=false` in `~/.config/claude-memory/config.env`. You'll then commit manually.

## Uninstall

```text
/plugin uninstall obsidian-memory@jgcosme-plugins
```

The vault and config files at `~/Documents/Obsidian Vault` and `~/.config/claude-memory/` are left in place — delete them manually if you want a full removal.

## License

MIT — see `LICENSE`.
