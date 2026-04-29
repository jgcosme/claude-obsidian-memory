# How it works

The plugin runs three hooks across the Claude Code session lifecycle.

## `SessionStart`

`hooks/scripts/session-start.sh` runs on every new session and:

1. **First-time setup gate.** If `$OBSIDIAN_VAULT_PATH` doesn't exist, the hook injects a consent prompt instructing Claude to ask the user once before running `setup.sh` (and offering an optional `git init`). It then exits early — none of the steps below run until the vault exists. On the next session after scaffolding, the normal flow takes over.
2. Tries to launch Obsidian.app if available (purely so the optional `obsidian` CLI works). No-op on Linux.
3. Derives the project name from `$CLAUDE_PROJECT_DIR` basename (or `$PWD`).
4. Injects into context:
   - **Bootstrap instructions** — recall/remember/route guidance.
   - **Vault `README.md`** — prose orientation.
   - **Auto-generated vault overview** — produced by the shared `_overview.sh` helper (cached at `$MEMORY_OVERVIEW_CACHE_DIR`, invalidated by vault `*.md` mtimes). Lists Tools, General, and the current project's Decisions/Learnings/Research/References/Journal. Other projects appear as a name list to keep the payload small. The underlying `_vault.py overview` supports `--mode {full,tools-and-general,tools-only}` for testing leaner shapes via the eval harness; production runs use the default `full`.
   - **Project-scaffolding prompt** if `Projects/<name>/` doesn't exist — instructs Claude to ask once before creating the folder.

Total injection is typically 3–8 KB depending on vault size.

## `UserPromptSubmit` (retrieval gate)

`hooks/scripts/user-prompt-submit.sh` runs on every user message before it reaches the main session:

1. Builds the auto-generated vault overview via the shared `_overview.sh` helper. The helper caches the overview to `/tmp/claude-memory-overview-cache/<sha1(vault|project)>.txt` and invalidates it when any `*.md` file in the vault is newer than the cache file (`find -newer`, fast-path early exit). `SessionStart` populates the cache, so the first user turn already hits a warm cache.
2. Spawns `claude -p --tools "" --system-prompt <overview> --output-format json` with the user's message as the prompt. `--tools ""` disables all tools — the gate is pure text in / JSON out. `--output-format json` wraps the response so we can extract both the gate's `.result` text and the call's exact `.usage` / `.total_cost_usd` / `.duration_ms` for `/obsidian-memory:usage`. The recursion-guard env vars (`CLAUDE_MEMORY_GATE=1`, `CLAUDE_MEMORY_REVIEW=1`) prevent the subprocess's own `SessionStart`/`SessionEnd`/`UserPromptSubmit` hooks from re-firing. We don't use `--bare` because that flag disables OAuth/keychain auth — see `claude --help`.
3. The gate inherits the user's default model. Anthropic's prompt cache reuses the overview (in `--system-prompt`) across calls within the 5-min TTL.
4. The gate returns JSON: `{"read": [...], "search": [{type, keywords, path_prefix, created_after, created_before}]}`.
5. The hook executes any typed searches via `_vault.py search`, merges read paths + search hits, validates each path exists in the vault and isn't a path-traversal attempt, deduplicates against the per-session injected list, and caps at `OBSIDIAN_MEMORY_GATE_PATH_CAP` (default 3).
6. Surviving paths get their bodies emitted as additional context (truncated per-note at `OBSIDIAN_MEMORY_GATE_NOTE_BYTE_CAP`, default 10 KB). The hook prints a single status line — bold cyan on stderr for terminal visibility, plain on stdout for the agent's context — naming the matched paths:

   ```text
   [obsidian-memory] Obsidian search found 2 document matches: Tools/Slack.md, General/References/secrets-env.md
   ```

7. The hook records two telemetry events per turn (when applicable): a `gate_call` event with the API's exact `.usage` block, and — if any notes were injected — a `gate_inject` event with the byte size of the injection. Both are appended to `/tmp/claude-memory-usage/<session_id>.jsonl` via `hooks/scripts/_usage_log.sh`.

**Why paths + searches:** the overview alone handles "what did we decide about auth?" → pick `Decisions/auth.md`. For time-bound queries like "what did I learn last week?" the overview doesn't expose `created` dates, so the gate emits `{"search": [{"type": "learning", "created_after": "2026-04-21"}]}` and the hook runs a typed search.

**Failure mode is loud and non-blocking:** errors print a one-line warning to stderr, log details to `/tmp/claude-memory-gate.log`, and exit `0` so the prompt still reaches the main session.

**Disabling:** set `OBSIDIAN_MEMORY_GATE_ENABLED=false` in `~/.config/claude-memory/config.env`.

## `SessionEnd`

`hooks/scripts/session-end.sh` backgrounds a `claude -p` subprocess that:

1. Reads the transcript.
2. Writes a journal entry to `Projects/<project>/Journal/YYYY-MM-DD.md` (appends a `## Session HH:MM` section if the file already exists for today, and rewrites the frontmatter `description` to summarize the full day). Skipped if `Projects/<project>/` doesn't exist.
3. Writes new notes proactively when ALL of:
   - the information is significant (correction, validated approach, decision, novel fact),
   - it will be useful in future sessions,
   - and no existing note already covers it (verified by typed search before writing).
4. Modifies existing notes only on **explicit user correction** in the transcript — not inference. Inferred staleness is flagged for the next session. When a non-journal note is extended or corrected, its frontmatter `description` is rewritten if the one-line summary no longer fits — this keeps the `SessionStart` auto-overview accurate.
5. Runs a delta integrity check on its own writes (frontmatter complete, wikilinks resolve), plus a `description`-vs-body check on any non-journal note linked from today's journal entry.
6. Independently `git add -A && git commit`s any vault changes when `OBSIDIAN_MEMORY_AUTOCOMMIT=true` (default). Push is opt-in (`OBSIDIAN_MEMORY_AUTOPUSH=true`). Wrapped in `flock` to prevent concurrent sessions racing.

The hook returns immediately; the review runs in the background and logs to `/tmp/claude-memory-review.log`. The review subprocess uses `claude -p --output-format json`; on success the wrapper extracts the call's `.usage` / `.total_cost_usd` / `.duration_ms` and appends a `review_call` event to `/tmp/claude-memory-usage/<session_id>.jsonl`.

### Routing rules

When `SessionEnd` identifies a memory candidate, it routes by category:

- **Personal / cross-project** (style preference, external system, tool, person)
  → vault note in `General/Preferences|References|People` or `Tools/`.
- **Project-scoped + team-relevant + project repo has internal docs** (docs/, ADR folders, mkdocs/sphinx, CONTRIBUTING)
  → reflect upstream as a doc edit in the project repo (uncommitted working-tree change, WIP-guarded by `git status --porcelain` on the target), plus a thin-pointer vault note at `Projects/<name>/{Decisions,Learnings}/`.
- **Project-scoped otherwise**
  → substantive vault note at `Projects/<name>/{Decisions,Learnings}/`.

Project-repo writes are restricted to the docs tree — never source, configs, CI, or manifests. If the target is dirty, the write is skipped and the deferral is recorded in the journal.

## Token telemetry (`/obsidian-memory:usage`)

`hooks/scripts/_usage_log.sh` is a tiny JSONL appender shared by all three hooks. It writes per-session usage events to `/tmp/claude-memory-usage/<session_id>.jsonl` (override with `MEMORY_USAGE_DIR`). Two modes:

- **`api`** — for the gate and review `claude -p` calls. Records the API's exact `usage` object (input/output/cache_read/cache_creation tokens), `cost_usd`, and `duration_ms`. Captured by setting `--output-format json` and unwrapping the `.result` event.
- **`chars`** — for stdout injections (SessionStart bootstrap, gate retrieval injects). Records the byte count and an `approx_tokens = ceil(bytes/4)` estimate. There is no API call to read tokens from for these — the text becomes part of your main session's input.

Four event kinds:

| `kind` | Mode | When |
|---|---|---|
| `session_start` | chars | once per session, captured via `tee` of `SessionStart` stdout |
| `gate_inject` | chars | every UserPromptSubmit where the gate decided to inject ≥1 note |
| `gate_call` | api | every UserPromptSubmit (when the gate is enabled) |
| `review_call` | api | once per session at SessionEnd (in the backgrounded subprocess) |

Timestamps are written in UTC ISO 8601 (`date -u '+%Y-%m-%dT%H:%M:%SZ'`) so they line up with the main session transcript at `~/.claude/projects/<encoded-cwd>/<session_id>.jsonl`, which uses the same format. `scripts/usage.sh` joins the two files to compute injection × turns-alive attribution and a per-session plugin share. See the `Session share` block in `/obsidian-memory:usage` output.

When reading the main session transcript, **dedup by `.message.id`** — Claude Code re-emits the same assistant message multiple times via snapshot/replay events, and naive summing over-counts tokens by ~2×.

## Recursion guard

The `SessionEnd` review and the retrieval gate both spawn `claude -p`. The subprocess fires its own `SessionStart`, `UserPromptSubmit`, and `SessionEnd` hooks — which would re-run the gate or the review. To prevent recursion, each hook is invoked with `CLAUDE_MEMORY_REVIEW=1` or `CLAUDE_MEMORY_GATE=1` on the subprocess environment; the affected scripts exit early when either is set.

## Adding a new project

`cd` into the project and start a session. `SessionStart` detects the missing `Projects/<basename>/` folder and instructs Claude to ask you once. Answer **yes** and Claude:

1. Creates `Projects/<name>/{Decisions,Learnings,Research,References,Journal}` and renders `overview.md` from the template.
2. Inspects the project dir — top-level docs (README, ARCHITECTURE, CONTRIBUTING, CHANGELOG), package manifests, ADR folders, runbooks, design docs, RFCs, /docs, build/CI config. Skips source and vendored deps.
3. Populates `overview.md` with the standard section headings (`## What it is`, `## Goals`, `## Current branch / focus`, `## Stakeholders`, `## Notes`), citing source files inline. Sections without grounded evidence are left empty.
4. Seeds subfolders with thin pointers (1–3 sentence summary + relative source path):
   - `References/` — entry-point pointers (architecture, API specs, getting-started, contributing)
   - `Decisions/` — ADRs and design choices
   - `Learnings/` — runbooks, troubleshooting, postmortems
   - `Research/` — design docs, RFCs, options comparisons
5. Leaves `Journal/` empty (`SessionEnd` populates it).

Answer **no** for incidental cwds (`/tmp`, throwaway clones); General/Tools writes still work.

To scaffold by hand (e.g., for non-interactive setup):

```bash
NAME="<your-project-basename>"
cd "$HOME/Documents/Obsidian Vault/Projects"
mkdir -p "$NAME"/{Journal,Decisions,Learnings,Research,References}
sed "s/__PROJECT_NAME__/$NAME/g; s/__TODAY__/$(date +%Y-%m-%d)/g" \
  "$CLAUDE_PLUGIN_ROOT/templates/Projects/PROJECT_NAME/overview.md" \
  > "$NAME/overview.md"
```
