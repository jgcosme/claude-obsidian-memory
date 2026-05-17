---
type: reference
description: "Overview of obsidian-memory plugin - vault-backed persistent memory for Claude Code"
created_at: 2026-04-30T00:00:00+08:00
created_by: init
updated_at: 2026-05-03T00:00:00+08:00
updated_by: init
project: claude-obsidian-memory
---

# obsidian-memory

A Claude Code plugin that turns a markdown directory into Claude's persistent memory. The vault is git-tracked, frontmatter-driven, and Obsidian-friendly — Obsidian.app is optional. Three lifecycle hooks plus two in-session skills handle recall, journaling, and proactive note-writing.

## Why

Claude Code's per-project auto-memory (`~/.claude/projects/*/memory/`) is siloed and not editable in any UI. This plugin replaces it with a vault-backed system:

- **Transparent memory.** Nothing is hidden in a database or vector index — every fact the agent recalls is a file you can open and correct. Fixing a wrong memory is a text edit, not a prompt negotiation.
- **Frontmatter as source of truth.** Every note declares `type`, `description`, `created_at/by`, `updated_at/by` (and `project` when scoped). The plugin writes those fields automatically and renders them each session into an in-context overview — path + type + one-line hook per note — so the agent retrieves by picking from a known list, not by guessing keywords against a vector store.
- **Git-tracked.** Every memory write is a diff. Auto-commit on by default; auto-push opt-in.
- **Obsidian-friendly, not Obsidian-required.** The vault is just markdown — search runs in a single static Rust binary. Obsidian.app adds a UI but no plugin-required functionality.
- **Three lifecycle hooks + two skills.** `SessionStart` loads context, `UserPromptSubmit` runs a proactive retrieval gate (description-anchored), and two agent-driven skills cover reads and writes:
  - **`vault-search`** — agent-driven vault lookup. Invoked when the conversation needs project facts (IDs, channels, configs, credentials), troubleshooting context, or external-tool setup. Complementary to the gate: the gate matches against note descriptions only; this skill searches body content the gate doesn't see.
  - **`save-memory`** — captures stable cross-session information regardless of source (user-stated or tool-discovered): corrections, preferences, decisions and rationale, novel facts (people, IDs, configs, channels, dashboards, endpoints).

  `SessionEnd` writes the journal entry and backstops anything the skills missed (dedup-by-search, integrity check, auto-commit).
- **Federated project-vaults.** Project repos can be opted in (per-project, one prompt) so their docs are read alongside the personal vault and written to by save-memory when the project's existing structure is the right home for a memory. No mirroring; each vault stays in its own git.

For internals, see [HOW-IT-WORKS.md](./HOW-IT-WORKS.md).

## Prerequisites

| Tool | Required | Why |
|---|---|---|
| Claude Code ≥ 2.0 | yes | runs the plugin |
| `curl` or `wget` | yes (first run only) | downloads the prebuilt binary from GH Releases |
| `tar` | yes (first run only) | extracts the release tarball |
| `git` | recommended | auto-commit (no-op if vault isn't a repo) |
| Obsidian desktop app | optional | only needed for the in-vault `obsidian search` CLI |
| `gh` | optional | pushing the vault to GitHub |
| `flock` | optional | concurrent-session safety on auto-commit |
| Rust toolchain (`cargo`) | optional | only needed on platforms outside the prebuilt matrix — the installer falls back to `cargo build --release` |

Supported platforms:

- **Prebuilt binaries** (no toolchain needed): macOS arm64 (`aarch64-apple-darwin`), Linux x86_64 (`x86_64-unknown-linux-gnu`).
- **Build-from-source fallback**: any platform with a Rust toolchain on PATH. The installer (`bin/install.sh`) auto-detects the platform; if it's not in the prebuilt matrix — or the GitHub download fails — it runs `cargo build --release` against the bundled `Cargo.toml`. Install Rust via [rustup.rs](https://rustup.rs) and re-run the hook.

Either way the plugin runs as a single static binary; no Python or jq runtime dependency.

## Install

```text
/plugin marketplace add jgcosme/claude-obsidian-memory
/plugin install obsidian-memory@jgcosme-plugins
/reload-plugins
```

On first hook fire, the bootstrap (`bin/run`) detects that the binary is missing and downloads the matching prebuilt `obsidian-memory` from GitHub Releases (sha256-verified). Subsequent invocations exec it directly. If the download fails (offline, GH outage), hooks exit cleanly so Claude Code is never blocked — the plugin re-tries on the next event.

The first time you start a Claude session after installing, `SessionStart` detects the missing vault and asks Claude to confirm setup with you (one consent prompt). Answer **yes** and Claude runs `setup` for you, then offers to `git init` the vault so `SessionEnd` can auto-commit.

To run setup manually instead — or to scaffold without starting a Claude session — invoke:

```bash
"$CLAUDE_PLUGIN_ROOT/bin/run" setup
```

Idempotent. Creates the vault, config, and a `chmod 600` `secrets.env`.

Optional — push the vault to a private GitHub remote so it follows you across machines:

```bash
gh repo create my-obsidian-memory --private --source "$HOME/Documents/Obsidian Memory" --remote origin --push
```

## Slash commands

| Command | What it does |
|---|---|
| `/obsidian-memory:status` | Health check: config, vault, prereqs, scripts, recent activity. |
| `/obsidian-memory:usage` | Per-kind token breakdown + plugin's share of this session's tokens. |
| `/obsidian-memory:audit` | Whole-corpus integrity: frontmatter, broken wikilinks, orphans, duplicate basenames, frontmatter backfill, description-vs-body drift. Operates on the personal vault and the current project's project-vault when registered. |
| `/obsidian-memory:project [enable\|disable\|remove\|list]` | Manage project-vault registrations. With no verb, walks a guided picker over the registry + current cwd. See [Federated project-vaults](#federated-project-vaults). |
| `/obsidian-memory:types [list\|add\|edit\|remove\|reset]` | Manage the memory-type vocabulary at `~/.config/obsidian-memory/types.yaml`. With no verb, walks a guided picker. See [Vault structure](#vault-structure). |

## Visibility

When the gate injects vault notes for a turn, you see a one-line system message so you know retrieval ran:

```text
[obsidian-memory] vault → Tools/Slack.md, Notes/secrets-env.md
```

Setup also wires the plugin's token-usage readout into Claude Code's status line — running totals + the plugin's share of this session's tokens, repainted each turn:

```text
obsidian-memory • my-project 384.0k tok · 23.4%
```

The `• <project>` appears whenever the current repo is registered as a project-vault (see [Federated project-vaults](#federated-project-vaults) below). The status line is set during `setup.sh` only if you don't already have one configured (existing customizations are left alone). To turn the plugin's status line off without touching Claude Code settings, set `OBSIDIAN_MEMORY_STATUSLINE_ENABLED=false` in `~/.config/obsidian-memory/config.env` — `statusline.py` will exit silently and the segment renders empty.

## Vault structure

The plugin owns its own Obsidian vault — by default at `~/Documents/Obsidian Memory`. Three top-level folders, plus a README:

```
Obsidian Memory/
├── README.md          — prose orientation (always loaded)
├── Tools/<tool>.md    — CLI/API references; browsed by tool name
├── Journals/<project>/<date>.md — one entry per project per day (sessions append), written by SessionEnd
└── Notes/             — everything else; searched by frontmatter
```

Project scoping is via the `project:` frontmatter field, not folder hierarchy — a `Notes/auth-decision.md` with `project: my-app` is project-scoped without needing a `Projects/my-app/` wrapper.

Frontmatter on every note (except `README.md` files):

```yaml
---
type: preference | reference | findings | decision | learning | tool | journal
description: "one-line hook"
created: YYYY-MM-DD
project: <project-name>     # only when the note is project-scoped
---
```

Seven types — `findings` captures multi-source research synthesis (read several docs, compared options, mapped a territory) so a future session can re-read the synthesis instead of redoing the investigation. Notes that genuinely span axes can declare a list — `type: [findings, decision]` for research that ended in a choice — and appear under each of their types in the overview. The first type drives routing.

`type` lives in frontmatter; the auto-overview groups by `type:` rather than by folder, so the layout stays browsable while staying flat.

Canonical type semantics live in [`templates/types.md`](./templates/types.md). The runtime routing table — type names, personal-vault folder, project-vault folder candidates — is loaded from `~/.config/obsidian-memory/types.yaml` (seeded from [`examples/types.yaml.example`](./examples/types.yaml.example) on first setup). Edit that file directly, or run `/obsidian-memory:types` to add, edit, remove, or reset types interactively.

If you already have your own Obsidian vault, the plugin's vault sits beside it as a separate top-level vault. Open whichever one you want in Obsidian.app — the plugin only reads/writes inside its own vault.

## Federated project-vaults

Project repos can be registered as a "project-vault" — a second corpus searched alongside the personal vault and written to by save-memory when the project's docs are the right home for a memory.

The first time you start a session in a project repo with markdown files, `SessionStart` asks once:

```
Register 'my-project' as a project-vault?
```

- **Yes** → `init_project_vault.py` adds plugin frontmatter (`type/description/created/project`) to `.md` files that don't already have any frontmatter. The repo is recorded in `~/.config/obsidian-memory/projects.json` as `enabled`. Future sessions surface those docs in the auto-overview alongside the personal vault.
- **No** → recorded as `disabled`. No frontmatter added, no overview block. The prompt won't fire again for this repo.

You can also register, disable, or remove a repo at any time with `/obsidian-memory:project enable` (or `disable` / `remove` / `list`) from inside the repo — useful if you said "no" earlier and changed your mind, or want to opt in a repo without starting a fresh session.

The corpus is whatever `git ls-files` surfaces (tracked + untracked-not-gitignored), minus boilerplate (`LICENSE`, `CHANGELOG`, `.github/` templates, etc.). No persisted file list — the corpus is recomputed every session, so docs added or moved between sessions are picked up automatically.

When a repo is registered + enabled:

- **Read**: search and overview span both corpora; results carry a `corpus` field (`personal` / `project`).
- **Write**: save-memory routes by type — `decision` → repo's `decisions/` folder if it exists, else personal `Notes/`; same for `learning`, `reference`. `tool` and `preference` always go to personal. Journals are personal-only, written by SessionEnd.
- **No mirroring**: each vault stays in its own git. The personal vault auto-commits at SessionEnd; the project-vault never auto-commits — its files appear in the repo's working tree for you to review and commit on your own cadence.

## Configuration

Three files live in `~/.config/obsidian-memory/`:

- `config.env` — environment variables (vault path, feature flags, log dirs). See table below.
- `types.yaml` — memory-type vocabulary (validation, overview ordering, folder routing). Seeded from [`examples/types.yaml.example`](./examples/types.yaml.example) at setup; edit directly or via `/obsidian-memory:types`. Removing a type that existing notes use will cause audit to flag those notes — the CLI refuses such removals without `--force`.
- `projects.json` — project-vault registry. Each opted-in project gets one entry:

  ```json
  {
    "projects": {
      "/abs/path/to/your/repo": {
        "enabled": true,
        "project": "your-repo-basename"
      }
    }
  }
  ```

  Edit directly to opt in/out without re-prompting, change a project name, or remove an entry to surface the registration prompt again.

Edit `~/.config/obsidian-memory/config.env`:

| Variable | Default | Purpose |
|---|---|---|
| `OBSIDIAN_VAULT_PATH` | `$HOME/Documents/Obsidian Memory` | vault path |
| `CLAUDE_BIN` | auto-detected | `claude` binary used by `SessionEnd` review and the gate |
| `OBSIDIAN_MEMORY_AUTOCOMMIT` | `true` | git commit vault changes after review |
| `OBSIDIAN_MEMORY_AUTOPUSH` | `false` | push after auto-commit |
| `OBSIDIAN_MEMORY_GATE_ENABLED` | `true` | retrieval gate on/off |
| `OBSIDIAN_MEMORY_REVIEW_ENABLED` | `true` | SessionEnd review on/off (auto-commit still runs when off) |
| `OBSIDIAN_MEMORY_BOOTSTRAP_OVERVIEW` | `true` | inject vault overview into main session at SessionStart (gate keeps its own copy regardless) |
| `OBSIDIAN_MEMORY_SLIM_TRANSCRIPT` | `true` | strip tool_use / tool_result blocks from the transcript before the SessionEnd reviewer reads it (~95% size reduction on real sessions; signal preserved) |
| `OBSIDIAN_MEMORY_GATE_PATH_CAP` | `3` | max paths the gate injects per turn |
| `OBSIDIAN_MEMORY_GATE_NOTE_BYTE_CAP` | `10240` | per-note size cap on gate injection |
| `OBSIDIAN_MEMORY_STATUSLINE_ENABLED` | `true` | show the plugin's token-usage status line; set false to hide it (statusline.py exits silently and `setup.sh` skips patching `~/.claude/settings.json`) |
| `OBSIDIAN_MEMORY_DEBUG` | `false` | verbose logging |
| `MEMORY_REVIEW_LOG` | `/tmp/claude-memory-review.log` | `SessionEnd` review log |
| `MEMORY_GATE_LOG` | `/tmp/claude-memory-gate.log` | retrieval gate log |
| `MEMORY_LOG_MAX_BYTES` | `1048576` | rotate hook logs at this size |
| `MEMORY_OVERVIEW_CACHE_DIR` | `/tmp/claude-memory-overview-cache` | shared overview cache (mtime-invalidated) |
| `MEMORY_USAGE_DIR` | `/tmp/claude-memory-usage` | per-session token-usage JSONL logs read by `/obsidian-memory:usage` |

## Secrets

Never paste credentials into vault notes. Add them to `~/.config/obsidian-memory/secrets.env` and reference by variable name only:

```bash
# secrets.env
export SLACK_USER_TOKEN="xoxp-..."
```

```markdown
# in Tools/slack.md
- Token: stored in `~/.config/obsidian-memory/secrets.env` as `SLACK_USER_TOKEN`. Source the file before use.
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
rm -rf "$HOME/Documents/Obsidian Memory"
rm -rf "$HOME/.config/obsidian-memory"
rm -f /tmp/claude-memory-review.log{,.1} /tmp/claude-memory-gate.log{,.1}
rm -rf /tmp/claude-memory-gate-state /tmp/claude-memory-usage
```

If `setup.sh` set Claude Code's status line, remove the `statusLine` block from `~/.claude/settings.json` (a `.bak.<timestamp>` file from the original is alongside it).

## License

MIT — see `LICENSE`.
