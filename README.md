# obsidian-memory

A Claude Code plugin that turns a markdown directory into Claude's persistent memory. The vault is git-tracked, frontmatter-driven, and Obsidian-friendly — Obsidian.app is optional. Three hooks handle recall, journaling, and proactive note-writing across the session lifecycle.

## Why

Claude Code's per-project auto-memory (`~/.claude/projects/*/memory/`) is siloed and not editable in any UI. This plugin replaces it with a vault-backed system:

- **Frontmatter as source of truth.** Every note declares `type`, `description`, `created` (and `project` when scoped). The session-start overview, retrieval gate, and audit all derive from frontmatter — nothing to hand-maintain.
- **Git-tracked.** Every memory write is a diff. Auto-commit on by default; auto-push opt-in.
- **Obsidian-friendly, not Obsidian-required.** The vault is just markdown — search runs in pure Python. Obsidian.app adds a UI but no plugin-required functionality.
- **Three lifecycle hooks.** SessionStart loads context, UserPromptSubmit retrieves relevant notes per turn, SessionEnd journals and writes new notes proactively (with dedup-by-search, integrity check, and auto-commit).

For internals, see [HOW-IT-WORKS.md](./HOW-IT-WORKS.md).

## Prerequisites

| Tool | Required | Why |
|---|---|---|
| Claude Code ≥ 2.0 | yes | runs the plugin |
| `jq` | yes | parses hook payloads |
| `python3` ≥ 3.9 | yes | search CLI + audit |
| `git` | recommended | auto-commit (no-op if vault isn't a repo) |
| Obsidian desktop app | optional | only needed for the in-vault `obsidian search` CLI |
| `gh` | optional | pushing the vault to GitHub |
| `flock` | optional | concurrent-session safety on auto-commit |

Tested on macOS; Linux works with the same prerequisites.

## Install

```text
/plugin marketplace add jgcosme/claude-obsidian-memory
/plugin install obsidian-memory@jgcosme-plugins
/reload-plugins
```

The first time you start a Claude session after installing, SessionStart detects the missing vault and asks Claude to confirm setup with you (one consent prompt). Answer **yes** and Claude runs `setup.sh` for you, then offers to `git init` the vault so SessionEnd can auto-commit.

To run setup manually instead — or to scaffold without starting a Claude session — invoke:

```bash
bash "$CLAUDE_PLUGIN_ROOT/scripts/setup.sh"
```

It's idempotent. It first verifies prerequisites (`jq`, `python3 ≥ 3.9`, `git`, optional `flock`) and exits with a clear message if any required tool is missing. It then creates `~/Documents/Obsidian Vault/` (or wherever `OBSIDIAN_VAULT_PATH` points), `~/.config/claude-memory/config.env`, and a `chmod 600` `secrets.env`.

To verify the install at any time:

```text
/obsidian-memory:status
```

Reports config, vault, prereqs, plugin scripts, search smoke-test, overview cache, and the latest review/gate log lines.

Optional — push the vault to a private GitHub remote so it follows you across machines:

```bash
gh repo create my-obsidian-vault --private --source "$HOME/Documents/Obsidian Vault" --remote origin --push
```

## Vault structure

```
Obsidian Vault/
├── README.md                  — prose orientation (always loaded)
├── Tools/<tool>.md            — CLI/API references
├── General/                   — cross-project
│   ├── user.md                — your profile
│   ├── Preferences/
│   ├── People/
│   ├── Admin/
│   └── References/
└── Projects/<name>/           — per-project (one folder per cwd basename)
    ├── overview.md
    ├── Journal/YYYY-MM-DD.md  — written by SessionEnd
    ├── Decisions/
    ├── Learnings/
    ├── Research/
    └── References/
```

Frontmatter on every note (except `README.md` files):

```yaml
---
type: tool | user | preference | people | admin | reference | overview | journal | decision | learning | research
description: one-line hook
created: YYYY-MM-DD
project: <project-name>     # only for project-scoped notes
---
```

## Querying the vault

Pure-Python search, works without Obsidian:

```bash
# Yesterday's learnings across all projects
YESTERDAY=$(date -v-1d +%Y-%m-%d)
python3 "$CLAUDE_PLUGIN_ROOT/scripts/_vault.py" search --type learning --created-after "$YESTERDAY"

# All decisions for project foo
python3 "$CLAUDE_PLUGIN_ROOT/scripts/_vault.py" search --type decision --path-prefix "Projects/foo"

# Notes mentioning "auth" anywhere
python3 "$CLAUDE_PLUGIN_ROOT/scripts/_vault.py" search --keywords "auth"
```

If Obsidian.app is running and the CLI is registered, `obsidian search` is also available with bracket-syntax filters (`[type:decision]`, `path:Projects/foo`); it doesn't support date-range queries — use the Python CLI for those.

Full vault audit (frontmatter, broken wikilinks, orphans, duplicate basenames):

```bash
python3 "$CLAUDE_PLUGIN_ROOT/scripts/audit.py"
```

Exits non-zero on issues; suitable for a pre-push hook or weekly cron. Does not auto-fix.

The `/obsidian-memory:audit` slash command wraps this and summarizes the output. Pass `--deep` to add an LLM pass that flags `description`-vs-body drift across the vault — useful when notes have been heavily extended since their description was set. Lists candidates with suggested replacements; does not auto-fix.

## Configuration

Edit `~/.config/claude-memory/config.env`:

| Variable | Default | Purpose |
|---|---|---|
| `OBSIDIAN_VAULT_PATH` | `$HOME/Documents/Obsidian Vault` | vault path |
| `CLAUDE_BIN` | auto-detected | `claude` binary used by SessionEnd review and the gate |
| `OBSIDIAN_MEMORY_AUTOCOMMIT` | `true` | git commit vault changes after review |
| `OBSIDIAN_MEMORY_AUTOPUSH` | `false` | push after auto-commit |
| `OBSIDIAN_MEMORY_GATE_ENABLED` | `true` | retrieval gate on/off |
| `OBSIDIAN_MEMORY_GATE_PATH_CAP` | `3` | max paths the gate injects per turn |
| `OBSIDIAN_MEMORY_GATE_NOTE_BYTE_CAP` | `10240` | per-note size cap on gate injection |
| `OBSIDIAN_MEMORY_DEBUG` | `false` | verbose logging |
| `MEMORY_REVIEW_LOG` | `/tmp/claude-memory-review.log` | SessionEnd review log |
| `MEMORY_GATE_LOG` | `/tmp/claude-memory-gate.log` | retrieval gate log |
| `MEMORY_LOG_MAX_BYTES` | `1048576` | rotate hook logs at this size |
| `MEMORY_OVERVIEW_CACHE_DIR` | `/tmp/claude-memory-overview-cache` | shared overview cache (mtime-invalidated) |

## Secrets

Never paste credentials into vault notes. Add them to `~/.config/claude-memory/secrets.env` and reference by variable name only:

```bash
# secrets.env
export SLACK_USER_TOKEN="xoxp-..."
```

```markdown
# in Tools/slack.md
- Token: stored in `~/.config/claude-memory/secrets.env` as `SLACK_USER_TOKEN`. Source the file before use.
```

The setup script `chmod 600`s the file. See [TROUBLESHOOTING.md](./TROUBLESHOOTING.md) for credential-leak scans.

## Privacy / what gets sent to the model

- **SessionEnd review** sends the full transcript of the just-ended session to `claude -p`.
- **Retrieval gate** sends each user message + the vault overview to `claude -p` per turn. Note bodies are *not* sent to the gate; only descriptions in the overview.
- **Vault contents** may be auto-committed and (if `OBSIDIAN_MEMORY_AUTOPUSH=true`) pushed to whatever remote your vault tracks. Default is opt-in.

All model traffic goes through your authenticated `claude` CLI — no raw HTTPS calls.

## Performance

The retrieval gate adds latency to every user message because it makes a `claude -p` call before the main session sees your prompt:

| Default model | Per-message added latency | Per-session cost (50 turns) |
|---|---|---|
| Haiku 4.5 | ~500ms–1s | sub-cent |
| Sonnet 4.6 | ~1–2s | ~$0.05 |
| Opus 4.7 | ~2–4s | ~$0.20–0.30 |

Anthropic's prompt cache amortizes the static portion (overview in `--system-prompt`) across calls within a 5-min window. Disable the gate via `OBSIDIAN_MEMORY_GATE_ENABLED=false` if the latency isn't worth it.

SessionEnd review and auto-commit run **in the background** — no shell wait time.

## Documentation

- [HOW-IT-WORKS.md](./HOW-IT-WORKS.md) — hook lifecycle, retrieval gate internals, routing rules, project scaffolding
- [FAQ.md](./FAQ.md) — common questions
- [TROUBLESHOOTING.md](./TROUBLESHOOTING.md) — debugging the hooks

## Uninstall

```text
/plugin uninstall obsidian-memory@jgcosme-plugins
/plugin marketplace remove jgcosme-plugins
```

For full cleanup (data, config, logs):

```bash
rm -rf "$HOME/Documents/Obsidian Vault"
rm -rf "$HOME/.config/claude-memory"
rm -f /tmp/claude-memory-review.log{,.1} /tmp/claude-memory-gate.log{,.1}
rm -rf /tmp/claude-memory-gate-state
```

## License

MIT — see `LICENSE`.
