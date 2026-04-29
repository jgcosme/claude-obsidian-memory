# How it works

The plugin runs three hooks across the Claude Code session lifecycle.

## `SessionStart`

`hooks/scripts/session-start.sh` runs on every new session and:

1. **First-time setup gate.** If `$OBSIDIAN_VAULT_PATH` doesn't exist, the hook injects a consent prompt instructing Claude to ask the user once before running `setup.sh` (and offering an optional `git init`). It then exits early — none of the steps below run until the vault exists. On the next session after scaffolding, the normal flow takes over.
2. Tries to launch Obsidian.app if available (purely so the optional `obsidian` CLI works). No-op on Linux.
3. Derives the project name from `$CLAUDE_PROJECT_DIR` basename (or `$PWD`).
4. Injects into context:
   - **Bootstrap instructions** — vault path + recall paths (Read tool, `_vault.py search`, optional `obsidian search`) + a one-line pointer to the `save-memory` skill for writes. Routing/frontmatter/when-to-save rules live in the skill body, not the bootstrap, so they only load when Claude invokes the skill.
   - **Vault `README.md`** — prose orientation.
   - **Auto-generated vault overview** — produced by the shared `_overview.sh` helper (cached at `$MEMORY_OVERVIEW_CACHE_DIR`, invalidated by vault `*.md` mtimes). Lists Tools, General, and the current project's Decisions/Learnings/Research/References/Journal. Other projects appear as a name list to keep the payload small. The underlying `_vault.py overview` supports `--mode {full,tools-and-general,tools-only}` for testing leaner shapes via the eval harness; production runs use the default `full`.
   - **Project-scaffolding prompt** if `Projects/<name>/` doesn't exist — instructs Claude to ask once before creating the folder.
5. Records the project's and the vault's `git rev-parse HEAD` to `/tmp/claude-memory-session/<session_id>.{project,vault}_head` so `SessionEnd` can diff-scope pointer + backlink reconciliation to "what changed during this session" — including mid-session commits that working-tree-only diff would miss. Best-effort; absent values fall back to working-tree-only diffs at SessionEnd.

Total injection is typically 3–8 KB depending on vault size.

## `UserPromptSubmit` (retrieval gate)

`hooks/scripts/user-prompt-submit.sh` runs on every user message before it reaches the main session:

1. Builds the auto-generated vault overview via the shared `_overview.sh` helper. The helper caches the overview to `/tmp/claude-memory-overview-cache/<sha1(vault|project)>.txt` and invalidates it when any `*.md` file in the vault is newer than the cache file (`find -newer`, fast-path early exit). `SessionStart` populates the cache, so the first user turn already hits a warm cache.
2. Spawns `claude -p --tools "" --strict-mcp-config --system-prompt <overview> --output-format json` with the user's message as the prompt. `--tools ""` disables all tools and `--strict-mcp-config` keeps the subprocess from auto-loading every MCP server in your settings (which would inflate per-call tokens by ~3.7×, ~35.6k → ~9.5k in measurement) — the gate is pure text in / JSON out. `--output-format json` wraps the response so we can extract both the gate's `.result` text and the call's exact `.usage` / `.total_cost_usd` / `.duration_ms` for `/obsidian-memory:usage`. The recursion-guard env vars (`CLAUDE_MEMORY_GATE=1`, `CLAUDE_MEMORY_REVIEW=1`) prevent the subprocess's own `SessionStart`/`SessionEnd`/`UserPromptSubmit` hooks from re-firing. We don't use `--bare` because that flag disables OAuth/keychain auth — see `claude --help`.
3. The gate inherits the user's default model. Anthropic's prompt cache reuses the overview (in `--system-prompt`) across calls within the 5-min TTL.
4. The gate returns JSON: `{"read": [...], "search": [{type, keywords, path_prefix, created_after, created_before}]}`.
5. The hook executes any typed searches via `_vault.py search`, merges read paths + search hits, validates each path exists in the vault and isn't a path-traversal attempt, deduplicates against the per-session injected list, and caps at `OBSIDIAN_MEMORY_GATE_PATH_CAP` (default 3).
6. Surviving paths get their bodies emitted as additional context (truncated per-note at `OBSIDIAN_MEMORY_GATE_NOTE_BYTE_CAP`, default 10 KB). The hook emits the official Claude Code hooks-spec JSON on stdout — `{systemMessage, hookSpecificOutput: {hookEventName: "UserPromptSubmit", additionalContext}}` — where `systemMessage` is shown to the user and `additionalContext` is injected into Claude's context:

   ```text
   [obsidian-memory] vault → Tools/Slack.md, General/References/secrets-env.md
   ```

7. The hook records two telemetry events per turn (when applicable): a `gate_call` event with the API's exact `.usage` block, and — if any notes were injected — a `gate_inject` event with the byte size of the injection. Both are appended to `/tmp/claude-memory-usage/<session_id>.jsonl` via `hooks/scripts/_usage_log.sh`.

**Why paths + searches:** the overview alone handles "what did we decide about auth?" → pick `Decisions/auth.md`. For time-bound queries like "what did I learn last week?" the overview doesn't expose `created` dates, so the gate emits `{"search": [{"type": "learning", "created_after": "2026-04-21"}]}` and the hook runs a typed search.

**Failure mode is loud and non-blocking:** errors print a one-line warning to stderr, log details to `/tmp/claude-memory-gate.log`, and exit `0` so the prompt still reaches the main session.

**Disabling:** set `OBSIDIAN_MEMORY_GATE_ENABLED=false` in `~/.config/claude-memory/config.env`.

**Bootstrap-overview flag:** by default the auto-overview is also injected into the main session's context at SessionStart so Claude can scan it during reasoning. Set `OBSIDIAN_MEMORY_BOOTSTRAP_OVERVIEW=false` to drop that injection — the gate keeps its own copy via `_overview.sh`, so retrieval keeps working; the main session loses its in-context "scan for relevance" map. Saves ~5–15KB cache_read per turn.

## `save-memory` skill (in-session writes)

`skills/save-memory/SKILL.md` is an auto-invoked Claude Code skill. Its description triggers on corrections worth remembering across sessions, validated approaches, "from now on..." preferences, explicit "remember this", and novel cross-session facts (config details, IDs, people). It skips ordinary task work, agreements, refactors, and generic technical questions.

When Claude invokes it, the body provides the routing rules (Personal vs Project, team-relevant + has-docs → upstream-reflect, project-doc WIP guard), the frontmatter schema, and a propose-then-write UX (preview + `save? (y/n)` before any file is written). Search-first dedup against the existing vault prevents duplicate notes.

The skill is the **eager** path for in-session writes — moments are captured at the time they happen, with user confirmation. The `SessionEnd` review (below) is the **retrospective** path — it sees the full transcript and catches anything the skill missed (especially quiet validations like "yeah, that was the right call" that lack a clear linguistic trigger).

**Why a skill, not a prompt rule:** the SessionStart bootstrap previously held the full when-to-save + routing + frontmatter rules and re-injected them every turn. Most turns don't write a memory, so those rules were always loaded but rarely acted on. A skill description (one paragraph in the system prompt) loads the body only on invocation — same coverage, much less always-on context, and the trigger description is sharper than mixed-in prose. Eval data backs this: see `tests/run_write_eval.py` and the corresponding `cases-write.json`.

**Why the gate isn't also a skill:** the retrieval gate runs *before* Claude reasons about the prompt. Several of its categories — "decision rejustification", "implicit user-pref", "imperative-with-guardrail" — fire on prompts that have no surface cue ("set up a cron job" when there's a no-cron decision). A skill would only fire after Claude has already started down a path. The gate's specialization also matters quantitatively: a skill-style framing without the gate's "default to {}" tuning over-injects on meta and generic prompts. See `tests/run_gate_eval.py` results comparing `prompts/current.txt` vs `prompts/skill-style.txt`.

## `SessionEnd`

`hooks/scripts/session-end.sh` backgrounds a `claude -p` subprocess that:

1. Reads the transcript.
2. Writes a journal entry to `Projects/<project>/Journal/YYYY-MM-DD.md` (appends a `## Session HH:MM` section if the file already exists for today, and rewrites the frontmatter `description` to summarize the full day). Skipped if `Projects/<project>/` doesn't exist.
3. Writes new notes proactively when ALL of:
   - the information is significant (correction, validated approach, decision, novel fact),
   - it will be useful in future sessions,
   - and no existing note already covers it (verified by typed search before writing — this is also what dedupes against in-session writes from the `save-memory` skill).
4. Modifies existing notes only on **explicit user correction** in the transcript — not inference. Inferred staleness is flagged for the next session. When a non-journal note is extended or corrected, its frontmatter `description` is rewritten if the one-line summary no longer fits — this keeps the `SessionStart` auto-overview accurate.
5. Runs an integrity + reconciliation pass over four corpora:
   - **(a) own writes** from steps 2–4 — frontmatter complete, wikilinks resolve, `description`-vs-body drift on extends/corrects.
   - **(b) journal-linked notes** — non-journal notes referenced from today's journal entry get the same `description`-vs-body check.
   - **(c) project repo `*.md` changes since session start** (via the `project_head` SHA, falling back to working-tree-only diff) — *pointer reconciliation*. For each changed doc, the review consults the pointer index built from `Projects/<name>/` notes whose frontmatter has `source: <repo-relative path>`:
     - **modified + pointer exists** → re-read source and pointer; rewrite the pointer's description (smallest edit) if it no longer summarizes the source. Skipped if the source is currently dirty in `git status` (defers to next session to avoid syncing to a half-edit).
     - **added + no pointer** → listed under `## New pointer suggestions`. Never auto-created — placement requires judgment (Decisions / Learnings / Research / References).
     - **deleted + pointer exists** → listed under `## Stale pointers (source deleted)`. Never auto-removed — deletion may have been accidental.
     - **renamed + pointer exists** → auto-rewrites the pointer's `source:` frontmatter to the new path; updates the description if content shifted. Listed under `## Pointer rewrites`.
   - **(d) vault `*.md` changes since last commit** (via the `vault_head` SHA, falling back to working-tree-only diff) — *backlink reconciliation*:
     - **renamed (old → new)** → finds every incoming `[[wikilink]]` resolving to the old path (path-qualified literal match, or unambiguous bare basename) via `_vault.py incoming-wikilinks --target <old>` and auto-rewrites each to the new path (smallest edit, `|alias` text preserved). Listed under `## Backlink rewrites`.
     - **deleted** → lists incoming wikilinks under `## Broken backlinks (target deleted)`. Never auto-fixed — deletion may be intentional or a rename the diff couldn't infer.

   Boilerplate filenames (`LICENSE*`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, `CHANGELOG.md`, PR/ISSUE templates) and top-level dotfile dirs (`.github/`, `.cursor/`, `.vscode/`, `.devcontainer/`, …) are filtered automatically when scanning the project repo, so IDE config and CI scaffolding don't show up as missing-pointer candidates.

6. Independently `git add -A && git commit`s any vault changes when `OBSIDIAN_MEMORY_AUTOCOMMIT=true` (default) — including pointer + backlink rewrites from step 5. Push is opt-in (`OBSIDIAN_MEMORY_AUTOPUSH=true`). Wrapped in `flock` to prevent concurrent sessions racing.

The hook returns immediately; the review runs in the background and logs to `/tmp/claude-memory-review.log`. The review subprocess uses `claude -p --strict-mcp-config --output-format json` (same MCP-isolation rationale as the gate); on success the wrapper extracts the call's `.usage` / `.total_cost_usd` / `.duration_ms` and appends a `review_call` event to `/tmp/claude-memory-usage/<session_id>.jsonl`.

**Disabling:** set `OBSIDIAN_MEMORY_REVIEW_ENABLED=false` in `~/.config/claude-memory/config.env` to skip the review entirely. Auto-commit of any vault writes from the in-session save-memory skill still fires. This is the single biggest token-cost lever — review reads the full transcript, which scales with session length.

**Transcript pre-filter:** before launching the reviewer, `session-end.sh` runs `scripts/_slim_transcript.py` to strip `tool_use` and `tool_result` content from the transcript and emit a compact dialogue version (user messages + assistant text + a one-line "used: Read, Bash, …" summary per assistant turn). On real sessions this reduces transcript size 94–96% (a 2.3MB transcript becomes ~95KB) while preserving the signal the reviewer acts on (decisions, corrections, validated approaches, novel facts). The reviewer reads the slim version via `Read` and writes notes from it; tool-call bodies that don't drive memory writes never enter the review's context. Disable via `OBSIDIAN_MEMORY_SLIM_TRANSCRIPT=false` to fall back to the raw transcript.

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
4. Seeds subfolders with thin pointers (1–3 sentence summary + relative source path). Each pointer's frontmatter must include `source: <repo-relative path>` — that field is what `/obsidian-memory:audit` and the SessionEnd reconciliation pass key off to detect drift, broken sources, and renames.
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
