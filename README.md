# obsidian-memory

A Claude Code plugin that turns a markdown directory into Claude's persistent memory. The vault is git-tracked, frontmatter-driven, and Obsidian-friendly — Obsidian.app is optional. Three lifecycle hooks plus two in-session skills handle recall, journaling, and proactive note-writing.

## Why

Claude Code's per-project auto-memory (`~/.claude/projects/*/memory/`) is siloed and not editable in any UI. This plugin replaces it with a vault-backed system:

- **Transparent memory.** Nothing is hidden in a database or vector index — every fact the agent recalls is a file you can open and correct. Fixing a wrong memory is a text edit, not a prompt negotiation.
- **Frontmatter as source of truth.** Every note declares `type`, `description`, `created` (and `project` when scoped). The plugin writes those fields automatically and derives everything else (recall, summaries, audits) from them — no index to maintain.
- **Git-tracked.** Every memory write is a diff. Auto-commit on by default; auto-push opt-in.
- **Obsidian-friendly, not Obsidian-required.** The vault is just markdown — search runs in pure Python. Obsidian.app adds a UI but no plugin-required functionality.
- **Three lifecycle hooks + two skills.** `SessionStart` loads context, `UserPromptSubmit` retrieves relevant notes per turn (description-anchored, proactive), and two in-session skills handle reads and writes the gate can't catch:
  - **`vault-search`** — body-anchored reactive lookup. Invoked when the conversation needs a project fact, troubleshoots a symptom, or sets up an external tool the gate's description-match missed (e.g., a credential whose location lives in a note body).
  - **`save-memory`** — captures stable cross-session information regardless of source (user-stated or tool-discovered): corrections, preferences, decisions and rationale, novel facts (people, IDs, configs, channels, dashboards, endpoints).

  `SessionEnd` writes the journal entry and backstops anything the skills missed (dedup-by-search, integrity check, auto-commit).

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

The first time you start a Claude session after installing, `SessionStart` detects the missing vault and asks Claude to confirm setup with you (one consent prompt). Answer **yes** and Claude runs `setup.sh` for you, then offers to `git init` the vault so `SessionEnd` can auto-commit.

To run setup manually instead — or to scaffold without starting a Claude session — invoke:

```bash
bash "$CLAUDE_PLUGIN_ROOT/scripts/setup.sh"
```

Idempotent. Verifies prerequisites and creates the vault, config, and a `chmod 600` `secrets.env`.

Optional — push the vault to a private GitHub remote so it follows you across machines:

```bash
gh repo create my-obsidian-vault --private --source "$HOME/Documents/Obsidian Vault" --remote origin --push
```

## Slash commands

| Command | What it does |
|---|---|
| `/obsidian-memory:status` | Health check: config, vault, prereqs, scripts, recent activity. |
| `/obsidian-memory:usage` | Per-kind token breakdown + plugin's share of this session's tokens. |
| `/obsidian-memory:audit` | Frontmatter, broken wikilinks, orphans, duplicate basenames. `--deep` adds an LLM pass for description-vs-body drift. |

## Visibility

When the gate injects vault notes for a turn, you see a one-line system message so you know retrieval ran:

```text
[obsidian-memory] vault → Tools/Slack.md, General/References/secrets-env.md
```

Setup also wires the plugin's token-usage readout into Claude Code's status line — running totals + the plugin's share of this session's tokens, repainted each turn:

```text
obsidian-memory 384.0k tok · 23.4%
```

The status line is set during `setup.sh` only if you don't already have one configured (existing customizations are left alone).

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

## Configuration

Edit `~/.config/claude-memory/config.env`:

| Variable | Default | Purpose |
|---|---|---|
| `OBSIDIAN_VAULT_PATH` | `$HOME/Documents/Obsidian Vault` | vault path |
| `CLAUDE_BIN` | auto-detected | `claude` binary used by `SessionEnd` review and the gate |
| `OBSIDIAN_MEMORY_AUTOCOMMIT` | `true` | git commit vault changes after review |
| `OBSIDIAN_MEMORY_AUTOPUSH` | `false` | push after auto-commit |
| `OBSIDIAN_MEMORY_GATE_ENABLED` | `true` | retrieval gate on/off |
| `OBSIDIAN_MEMORY_REVIEW_ENABLED` | `true` | SessionEnd review on/off (auto-commit still runs when off) |
| `OBSIDIAN_MEMORY_BOOTSTRAP_OVERVIEW` | `true` | inject vault overview into main session at SessionStart (gate keeps its own copy regardless) |
| `OBSIDIAN_MEMORY_SLIM_TRANSCRIPT` | `true` | strip tool_use / tool_result blocks from the transcript before the SessionEnd reviewer reads it (~95% size reduction on real sessions; signal preserved) |
| `OBSIDIAN_MEMORY_GATE_PATH_CAP` | `3` | max paths the gate injects per turn |
| `OBSIDIAN_MEMORY_GATE_NOTE_BYTE_CAP` | `10240` | per-note size cap on gate injection |
| `OBSIDIAN_MEMORY_DEBUG` | `false` | verbose logging |
| `MEMORY_REVIEW_LOG` | `/tmp/claude-memory-review.log` | `SessionEnd` review log |
| `MEMORY_GATE_LOG` | `/tmp/claude-memory-gate.log` | retrieval gate log |
| `MEMORY_LOG_MAX_BYTES` | `1048576` | rotate hook logs at this size |
| `MEMORY_OVERVIEW_CACHE_DIR` | `/tmp/claude-memory-overview-cache` | shared overview cache (mtime-invalidated) |
| `MEMORY_USAGE_DIR` | `/tmp/claude-memory-usage` | per-session token-usage JSONL logs read by `/obsidian-memory:usage` |

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

- **`SessionEnd` review** sends the full transcript of the just-ended session to `claude -p`.
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

`SessionEnd` review and auto-commit run **in the background** — no shell wait time.

## Token telemetry

Both `claude -p` calls (the gate and the SessionEnd review) use `--output-format json` and capture exact `usage` from the API response. Every hook also logs the size of any text it injects into your main session. The four event kinds:

| Event | Source | What it costs |
|---|---|---|
| `session_start` | `SessionStart` hook stdout | text appended to your context once at session start; re-sent on every subsequent turn (mostly cache_read after the first) |
| `gate_inject` | retrieved vault notes appended to a turn | text added to your input on that turn; re-sent on every turn after |
| `gate_call` | `claude -p` for the retrieval gate, every UserPromptSubmit | one separate API call against your rate limit |
| `review_call` | `claude -p` for the SessionEnd review | one separate API call (typically large because it processes the full transcript) |

Run `/obsidian-memory:usage` to see the breakdown plus the plugin's share of total session tokens.

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
rm -rf /tmp/claude-memory-gate-state /tmp/claude-memory-usage
```

If `setup.sh` set Claude Code's status line, remove the `statusLine` block from `~/.claude/settings.json` (a `.bak.<timestamp>` file from the original is alongside it).

## License

MIT — see `LICENSE`.
