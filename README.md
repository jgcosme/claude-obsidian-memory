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
| **`jq`** | parses hook payloads | `brew install jq` (macOS) / your package manager |
| **`python3`** ≥ 3.9 | the gate parses model output and the audit script reads the vault | usually preinstalled on macOS / Linux |
| **`git`** | vault history + auto-commit (strongly recommended; otherwise auto-commit is a no-op) | preinstalled on most systems |
| **Obsidian** desktop app *(optional)* | only needed if you want the in-vault `obsidian search`/`obsidian read` CLI features. The plugin works without Obsidian — the vault is just a directory of markdown files. | <https://obsidian.md> |
| **Obsidian CLI** registered *(optional)* | enables Claude to call `obsidian search` etc. for typed recall. Without this, Claude can still read notes via `Read`/`Bash` but loses the structured query syntax. | Open Obsidian → Settings → General → "Command line interface" → Register CLI |
| **`gh`** *(optional)* | only if you want to push the vault to GitHub | <https://cli.github.com> |
| **`flock`** *(optional)* | enables concurrent-session safety on the auto-commit step. Preinstalled on Linux; macOS users can `brew install flock` or skip — the plugin just won't serialize git ops between overlapping sessions. | `brew install flock` (macOS) |

**Platform**: tested on macOS. Should work on Linux with the same prerequisites. If Obsidian.app is installed, the SessionStart hook tries to launch it so the CLI is responsive — you may need to grant Accessibility permissions on first run.

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
   - `Projects/<project-name>/INDEX.md` **if it exists**. If it doesn't, the hook injects an instruction telling Claude to ask you once at the start of the session whether to scaffold it (running the same template-substitution commands the setup script uses). If you decline, Claude respects that for the rest of the session — `General/` and `Tools/` writes are still allowed, but no project-scoped notes.
   - **Usage instructions** telling Claude how/when to read and write memory

Claude only sees indexes — small, dense pointers. Deep recall happens on demand via `obsidian search` and `obsidian read`. Total injection is typically 3–6 KB.

### SessionEnd hook

When a session ends, `hooks/scripts/session-end.sh` backgrounds a `claude -p` subprocess that:

1. Reads the transcript.
2. Writes a journal entry to `Projects/<project>/Journal/YYYY-MM-DD.md` (appends a `## Session HH:MM` section if the file already exists for today). **Skipped** if `Projects/<project>/` doesn't exist — i.e., you declined to scaffold at session-start.
3. **Proactively** writes new notes when ALL of these hold:
   - the information is significant (correction, validated approach, decision, novel fact),
   - it will be useful in future sessions,
   - **and** no existing note already covers it (verified via `obsidian search`).
4. **Modifies** existing notes only on **explicit user correction** in the transcript — not on inference. If the transcript merely *suggests* a note might be stale, the review flags it for the next session instead of editing silently.
5. **Delta integrity check**: for each file the review created or modified above, verifies frontmatter is complete, every `[[wikilink]]` resolves, and new notes are listed in the relevant `INDEX.md`. Auto-fixes unambiguous issues; ambiguous ones are surfaced under `## Integrity flags` in the review output. (Vault-wide auditing is a separate script — see "Auditing the vault" below.)
6. `git add -A && git commit` runs **independently** of the review: if the vault is a git repo and dirty (`OBSIDIAN_MEMORY_AUTOCOMMIT=true` by default), any pending writes get committed — including `General/`/`Tools/` writes from sessions where the project journal step was skipped. Push is opt-in (`OBSIDIAN_MEMORY_AUTOPUSH=true`).

The hook returns immediately so it doesn't block your shell; the review runs in the background and logs to `/tmp/claude-memory-review.log`.

### UserPromptSubmit hook (retrieval gate)

On every user message, `hooks/scripts/user-prompt-submit.sh` runs a small "retrieval gate" before the message reaches the main session:

1. Collects the always-loaded indexes (`INDEX.md`, `Tools/INDEX.md`, `General/INDEX.md`, current project's `INDEX.md`).
2. Spawns `claude -p` with a prompt asking the model: "Given these indexes and the user's message, which (if any) notes are worth reading? Return up to N paths as JSON."
3. The gate inherits the user's default Claude model (no `--model` flag), so quality matches whatever model is configured. Tools are disabled (`--allowed-tools ""`) — the gate is pure text in/out.
4. Each returned path is validated against the vault before injection (drops nonexistent paths and any path-traversal attempts).
5. If any valid paths come back, the hook reads each note's body and emits them as additional context appended to the user's prompt.

**Failure mode is loud and non-blocking**: if `claude -p` errors, JSON parsing fails, or the gate returns junk, the hook prints a one-line warning to stderr (visible to the user), logs details to `/tmp/claude-memory-gate.log`, and exits `0` so the prompt still reaches the main session. You'll see the error but never get blocked from working.

**Cost / latency:** each call is ~2 KB cached input + ~30 output tokens. With Anthropic's prompt cache active, that's fractions of a cent per message; without caching it's a few cents per session if you're on Opus 4.7. Latency is whatever your default model takes to respond — typically 500ms–4s before each user message gets processed.

**Disabling:** set `OBSIDIAN_MEMORY_GATE_ENABLED=false` in `~/.config/claude-memory/config.env`. Path cap and log location are also configurable (`OBSIDIAN_MEMORY_GATE_PATH_CAP`, `MEMORY_GATE_LOG`).

### Recursion guard

The SessionEnd review and the retrieval gate both use `claude -p`. The subprocess itself fires a fresh SessionStart (fine — loads vault context), UserPromptSubmit (would re-run the gate on the gate's own prompt — bad), and SessionEnd on shutdown (would re-run the review — bad). Each hook is invoked with `CLAUDE_MEMORY_REVIEW=1` and `CLAUDE_MEMORY_GATE=1` set on the subprocess environment; the affected scripts exit early when either is set.

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
| `CLAUDE_BIN` | auto-detected via `$PATH` | path to the `claude` binary used by SessionEnd review and the gate |
| `OBSIDIAN_MEMORY_AUTOCOMMIT` | `true` | git commit vault changes after review (no-op if vault isn't a git repo) |
| `OBSIDIAN_MEMORY_AUTOPUSH` | `false` | push after auto-commit |
| `OBSIDIAN_MEMORY_GATE_ENABLED` | `true` | UserPromptSubmit retrieval gate on/off |
| `OBSIDIAN_MEMORY_GATE_PATH_CAP` | `3` | max number of vault paths the gate is allowed to inject per turn |
| `OBSIDIAN_MEMORY_GATE_NOTE_BYTE_CAP` | `10240` (10 KB) | per-note size cap on gate injection; oversized notes are truncated with a marker |
| `OBSIDIAN_MEMORY_DEBUG` | `false` | verbose debug logging in both hooks |
| `MEMORY_REVIEW_LOG` | `/tmp/claude-memory-review.log` | where SessionEnd review logs go |
| `MEMORY_GATE_LOG` | `/tmp/claude-memory-gate.log` | where retrieval gate logs go |
| `MEMORY_LOG_MAX_BYTES` | `1048576` (1 MB) | rotate hook logs to `.log.1` when they exceed this size |
| `MEMORY_GATE_DEDUP_DIR` | `/tmp/claude-memory-gate-state` | per-session dedup state for the gate |

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

The setup script chmod-600s the secrets file. If you ever need to verify the vault has no leaked credentials, scan it directly (replace the path with your `OBSIDIAN_VAULT_PATH`):

```bash
grep -rE "(xoxp-|xoxb-|sk-[A-Za-z0-9]|gho_|ghp_|github_pat_|AKIA|AIza|AIzaSy)" "$HOME/Documents/Obsidian Vault"
```

To also catch credentials that were written and later removed (still in git history):

```bash
git -C "$HOME/Documents/Obsidian Vault" log --all -p | \
  grep -E "(xoxp-|xoxb-|sk-[A-Za-z0-9]|gho_|ghp_|github_pat_|AKIA|AIza|AIzaSy)" | head
```

## Adding a new project

The easy path: just `cd` into the project directory and start a Claude session. SessionStart will detect that `Projects/<basename>/` doesn't exist and instruct Claude to ask you once — answer **yes** and Claude scaffolds the folder **and prefills it** from real evidence in the project dir. Claude uses its own judgment about what to read (top-level docs, package manifests, ADR folders, runbooks, design docs, `/docs`, build/CI config, git metadata) without recursing into source code or vendored deps. It then populates the standard headings in `overview.md` (`## What it is`, `## Goals`, `## Current branch / focus`, `## Stakeholders`, `## Notes`) with source-cited content (sections without grounded evidence are left empty), and seeds the appropriate subfolders by content type:

- `References/` — entry-point pointers (architecture overviews, API/OpenAPI specs, getting-started, contributing)
- `Decisions/` — ADRs and rationale-bearing design choices (`docs/adr/*`, etc.)
- `Learnings/` — runbooks, troubleshooting, postmortems, "how X actually works"
- `Research/` — design docs, RFCs, options comparisons

Each note is a 1–3 sentence summary plus the relative path of the source file so it can be reread on demand. There's no count cap — Claude seeds whatever the project genuinely has, skipping auto-generated, vendored, or license-style files. `Journal/` stays empty (SessionEnd populates it). Answer **no** if this is an incidental cwd (`/tmp`, `~/Downloads`, throwaway clone) and you don't want it in the vault.

If you'd rather scaffold by hand (e.g., for a non-interactive setup):

```bash
NAME="<your-project-basename>"
cd "$HOME/Documents/Obsidian Vault/Projects"
mkdir -p "$NAME"/{Journal,Decisions,Learnings,Research,References}

sed "s/__PROJECT_NAME__/$NAME/g; s/__TODAY__/$(date +%Y-%m-%d)/g" \
  "$CLAUDE_PLUGIN_ROOT/templates/Projects/PROJECT_NAME/INDEX.md" \
  > "$NAME/INDEX.md"
sed "s/__PROJECT_NAME__/$NAME/g; s/__TODAY__/$(date +%Y-%m-%d)/g" \
  "$CLAUDE_PLUGIN_ROOT/templates/Projects/PROJECT_NAME/overview.md" \
  > "$NAME/overview.md"
```

## Auditing the vault

`scripts/audit.py` does a full vault integrity scan — separate from the per-session delta check that runs in SessionEnd. It reports:

- **Frontmatter issues** — notes missing required keys (`type`, `description`, `created`; plus `project` under `Projects/`).
- **Broken wikilinks** — `[[target]]` references that don't resolve. Resolution mirrors Obsidian: path-qualified targets try vault-root, source-relative, then path-suffix match; bare targets match by basename anywhere.
- **Dead INDEX entries** — broken wikilinks that originate from an `INDEX.md`, surfaced separately so you can prioritize them.
- **Orphan notes** — files with no incoming wikilink (excluding `INDEX.md` files themselves).

```bash
# Markdown report to stdout
python3 "$CLAUDE_PLUGIN_ROOT/scripts/audit.py"

# JSON for programmatic use
python3 "$CLAUDE_PLUGIN_ROOT/scripts/audit.py" --json

# Override vault path
python3 "$CLAUDE_PLUGIN_ROOT/scripts/audit.py" --vault /path/to/vault
```

Exits non-zero when issues are found, so you can wire it into a pre-push hook or a weekly cron. It does **not** auto-fix — fixes are deliberately manual since orphans and missing frontmatter often need human judgment.

## Performance

The retrieval gate adds latency to **every user message** because it makes a `claude -p` call before the main session sees your prompt. Practical numbers:

| Default model | Per-message added latency | Per-session cost (50 turns) |
|---|---|---|
| Haiku 4.5 | ~500ms–1s | sub-cent |
| Sonnet 4.6 | ~1–2s | ~$0.05 |
| Opus 4.7 | ~2–4s | ~$0.20–0.30 |

Anthropic's prompt cache is on automatically — the gate's static portion (instructions + vault indexes) goes in `--system-prompt` and gets cached for 5 minutes, so back-to-back calls only pay full price for the dynamic user message (~30 input tokens) plus a tiny ~30-token output.

If the latency becomes annoying, set `OBSIDIAN_MEMORY_GATE_ENABLED=false` in your config to disable the gate entirely. You'll still have the always-loaded indexes from SessionStart — Claude can `obsidian search` for context on demand instead of having it auto-injected.

The SessionEnd review and auto-commit run **in the background** after the hook returns, so they don't add any wait time to your shell. The review uses the same `claude -p` mechanism as the gate (and benefits from the same cache).

## Privacy / what gets sent to the model

Be aware that this plugin transmits content beyond your active prompt:

- **SessionEnd review**: the full transcript of the just-ended session is sent to `claude -p`. If the conversation contained sensitive content, the review subprocess sees it.
- **Retrieval gate**: every user message + the vault's index files are sent to `claude -p` on every turn. The bodies of vault notes are *not* sent to the gate (it only sees descriptions in indexes), but they *are* injected into the main session if matched.
- **Vault contents**: notes Claude writes during a session may end up committed and (if `OBSIDIAN_MEMORY_AUTOPUSH=true`) pushed to whatever git remote your vault tracks. Default is `false` — you commit/push manually.

The plugin never makes raw HTTPS calls to Anthropic itself — all model traffic goes through the `claude` CLI you already have authenticated.

## FAQ

**Q: What if I switch projects mid-session by `cd`-ing somewhere else?**
A: The project name is captured at SessionStart (from `$CLAUDE_PROJECT_DIR` or `$PWD`). Mid-session `cd` doesn't change which project's INDEX is loaded. The journal at SessionEnd still writes to the original project's folder. Start a new session if you want to switch contexts.

**Q: Can I share my vault with a teammate?**
A: Yes — push to a private remote and have them clone it. The SessionEnd review will commit their changes too. Be careful with `General/user.md`, which is meant to be your personal profile; either gitignore it or accept that it's shared.

**Q: Does this work on Linux?**
A: Yes, with the same prerequisites. macOS-specific bits: the SessionStart hook tries `open -a Obsidian` (no-op on Linux) and falls back to `obsidian` on `$PATH`. `flock` is preinstalled on Linux but not on macOS.

**Q: What if I don't want Obsidian.app at all?**
A: The plugin works without it. The vault is just markdown files. You'll lose the `obsidian search` typed-query syntax (Claude will fall back to `grep`/`Read`), and the SessionStart "open Obsidian" step is a no-op. Everything else works.

**Q: How do I reset the gate's per-session dedup memory?**
A: Delete `/tmp/claude-memory-gate-state/<session_id>.injected`. Or `rm -rf /tmp/claude-memory-gate-state` to reset all sessions. The directory is recreated automatically.

**Q: Where do logs go?**
A: `/tmp/claude-memory-review.log` and `/tmp/claude-memory-gate.log` by default. Both rotate to `.log.1` once they exceed 1 MB. Override locations via `MEMORY_REVIEW_LOG` / `MEMORY_GATE_LOG`.

**Q: Can I run multiple Claude Code sessions in different cwds at the same time?**
A: Yes. Each session has its own SessionStart/SessionEnd; the gate runs independently per session (with its own dedup state via `session_id`). The autocommit is wrapped in `flock` to prevent concurrent commits from racing — though if you don't have `flock` installed, two simultaneous SessionEnd subprocesses *could* race on `git add -A`.

**Q: How do I rebuild the vault if it gets corrupted?**
A: Re-run `bash "$CLAUDE_PLUGIN_ROOT/scripts/setup.sh"` — it's idempotent. It won't overwrite existing files but will recreate any missing scaffolding from templates. If your vault is also a git repo, `git reset --hard` to a known good commit is faster.

## Troubleshooting

**SessionStart hook doesn't seem to load anything**
- Confirm `~/.config/claude-memory/config.env` points at a real vault.
- Check the vault has at least an `INDEX.md` — re-run `setup.sh` if not.
- Confirm Obsidian.app is running and the CLI is registered (Settings → General → Command line interface).

**SessionEnd review didn't run / didn't write a journal**
- Check `/tmp/claude-memory-review.log`. If it says `no Projects/<name>/ folder; skipping review, will still commit dirty vault state`, you declined to scaffold (or never were asked) — start a new session in that directory and answer **yes** to the scaffolding prompt, or scaffold manually (see "Adding a new project"). Note: any `General/`/`Tools/` writes from the session were still committed.
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
/plugin marketplace remove jgcosme-plugins
```

The plugin uninstall removes the hooks but leaves your data alone. To do a full clean removal:

```bash
# Vault (your memory — keep a backup if you may want it later)
rm -rf "$HOME/Documents/Obsidian Vault"

# Config + secrets
rm -rf "$HOME/.config/claude-memory"

# Logs and gate dedup state
rm -f /tmp/claude-memory-review.log /tmp/claude-memory-review.log.1
rm -f /tmp/claude-memory-gate.log /tmp/claude-memory-gate.log.1
rm -rf /tmp/claude-memory-gate-state
```

If you customized the log/dedup paths in `config.env`, remove those instead. There are no daemon processes, system services, or registered URL handlers to clean up — every plugin behavior runs inside Claude Code's hook lifecycle.

## License

MIT — see `LICENSE`.
