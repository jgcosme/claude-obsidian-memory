---
type: reference
description: "Explains the three lifecycle hooks (SessionStart, UserPromptSubmit, SessionEnd) and how the plugin operates"
created: 2026-04-30
project: claude-obsidian-memory
---

# How it works

> **v2.0 note.** As of v2.0.0 (May 2026), all hooks and helpers ship as a single Rust binary at `$PLUGIN_ROOT/bin/obsidian-memory` (lazy-installed by `bin/run` on first session from GitHub Releases). The behavioral descriptions below are unchanged — overview format, gate prompt, review flow, file paths, telemetry — but the implementation references throughout this doc still use the v1 `_vault.py`/`_projects.py`/`session-start.sh`/etc. paths. The Rust modules at `src/vault/`, `src/hook/`, `src/projects.rs`, etc. mirror them 1:1 (e.g. `_vault.py search` → `obsidian-memory vault search`). See [CHANGELOG.md](./CHANGELOG.md) for the full mapping.

The plugin runs three hooks across the Claude Code session lifecycle.

## `SessionStart`

`hooks/scripts/session-start.sh` runs on every new session and:

1. **First-time setup gate.** If `$OBSIDIAN_VAULT_PATH` doesn't exist, the hook injects a consent prompt instructing Claude to ask the user once before running `setup.sh` (and offering an optional `git init`). It then exits early — none of the steps below run until the vault exists. On the next session after scaffolding, the normal flow takes over.
2. Tries to launch Obsidian.app if available (purely so the optional `obsidian` CLI works). No-op on Linux.
3. Derives the project name from `$CLAUDE_PROJECT_DIR` basename (or `$PWD`).
4. **Project-vault registry lookup.** Resolves cwd to its git toplevel and looks the path up in `~/.config/obsidian-memory/projects.json` via `_projects.py lookup`. Three states:
   - `not_registered` + ≥1 candidate `.md` (per `_project_docs.enumerate_project_docs`) → injects a one-time registration prompt instructing Claude to ask the user "Register '<project>' as a project-vault?". On `y`, Claude runs `init_project_vault.py` (adds plugin frontmatter to files lacking any) and `_projects.py register --enabled`. On `n`, just `_projects.py register --no-enabled`. Either answer is durable in `projects.json`.
   - `enabled` → runs `init_project_vault.py` silently and writes the resolved repo path to `/tmp/claude-memory-session/<session_id>.project_vault` so `UserPromptSubmit` can re-use it without re-querying the registry. Init is idempotent on writes (only files missing frontmatter get LLM-classified and written) but reads every candidate `.md` each run to make that decision — typically ~10ms total on a 30-50 file repo, scaling linearly with corpus size. The eager run is what catches teammate-added docs without a manual step.
   - `disabled` → silent.
5. Injects into context:
   - **Bootstrap instructions** — vault path + two-line skill pointers (`vault-search` for body-level lookups the gate's description-match misses, `save-memory` for writes). The CLI syntax, routing rules, frontmatter schema, and when-to-invoke heuristics live in each skill's body, not the bootstrap, so they only load when Claude invokes the skill.
   - **Vault `README.md`** — prose orientation.
   - **Auto-generated vault overview** — produced by the shared `_overview.sh` helper (cached at `$MEMORY_OVERVIEW_CACHE_DIR`, invalidated by vault `*.md` mtimes; cache key spans personal vault + project-vault path). Lists Tools, Notes, Journals from the personal vault, grouped by frontmatter `type:`. When the cwd's project has a registered+enabled project-vault, an additional `# Project vault: <project>` section follows.
6. Records the vault's `git rev-parse HEAD` to `/tmp/claude-memory-session/<session_id>.vault_head` so `SessionEnd` can diff-scope backlink reconciliation to "what changed in the vault during this session" — including mid-session commits that working-tree-only diff would miss. Best-effort; absent values fall back to working-tree-only diff at SessionEnd.

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
   [obsidian-memory] vault → Tools/Slack.md, Notes/secrets-env.md
   ```

7. The hook records two telemetry events per turn (when applicable): a `gate_call` event with the API's exact `.usage` block, and — if any notes were injected — a `gate_inject` event with the byte size of the injection. Both are appended to `/tmp/claude-memory-usage/<session_id>.jsonl` via `hooks/scripts/_usage_log.sh`.

**Why paths + searches:** the overview alone handles "what did we decide about auth?" → pick `Decisions/auth.md`. For time-bound queries like "what did I learn last week?" the overview doesn't expose `created` dates, so the gate emits `{"search": [{"type": "learning", "created_after": "2026-04-21"}]}` and the hook runs a typed search.

**Failure mode is loud and non-blocking:** errors print a one-line warning to stderr, log details to `/tmp/claude-memory-gate.log`, and exit `0` so the prompt still reaches the main session.

**Disabling:** set `OBSIDIAN_MEMORY_GATE_ENABLED=false` in `~/.config/obsidian-memory/config.env`.

**Bootstrap-overview flag:** by default the auto-overview is also injected into the main session's context at SessionStart so Claude can scan it during reasoning. Set `OBSIDIAN_MEMORY_BOOTSTRAP_OVERVIEW=false` to drop that injection — the gate keeps its own copy via `_overview.sh`, so retrieval keeps working; the main session loses its in-context "scan for relevance" map. Saves ~5–15KB cache_read per turn.

## `vault-search` skill (in-session reads)

`skills/vault-search/SKILL.md` is an auto-invoked skill the agent reaches for when the conversation needs project facts (IDs, channels, configs, credentials), troubleshooting context, or external-tool setup. Its trigger is the agent's information need, not the gate's outcome — the two run independently. The skill's *value*, separate from its trigger, is that it searches note **bodies**: the retrieval gate above matches against note **descriptions** only, so a query whose only anchor lives in a note body (e.g., a Bitwarden link captured in `db-access.md`'s body, where the description doesn't mention "Bitwarden") silently misses the gate. The skill closes that recall gap when the agent realizes a lookup is needed.

When Claude invokes `vault-search`, the body provides the `_vault.py search` CLI syntax, decision shape (direct lookup vs troubleshooting vs tool setup vs project decision), and verification guidance (paths drift between sessions; re-read before recommending).

**Why a skill, not just inline documentation:** the bootstrap previously injected a `RECALL` block listing the search CLI and its flags. Empirical: an LLM evaluating "should I search the vault?" with the inline block achieved 37% recall on a 51-case fixture. The same decision with the skill description (loaded into the skill list with explicit trigger criteria) achieved 89% recall. Same neg_acc, slightly better precision. The skill description is a sharper trigger surface than ambient documentation — see `tests/run_write_eval.py --cases tests/cases-search.json`.

## `save-memory` skill (in-session writes)

`skills/save-memory/SKILL.md` is an auto-invoked skill. Its description triggers whenever the conversation surfaces information that is stable across sessions, useful in future sessions, and not derivable from the codebase or git history — *regardless of source*. That covers user-stated facts (corrections, preferences, decisions, "remember this") **and** agent-discovered references surfaced via tool calls (channel IDs, dashboard URLs, external system identifiers). Skips agreements, in-progress task state, and anything visible in the diff.

When Claude invokes it, the body provides the routing rules (Personal vs Project-with-docs vs Project-without-docs), the frontmatter schema, type-specific body shape (preference/feedback/decision/learning/tool), and a propose-then-write UX (preview + `save? (y/n)` before any file is written). Search-first dedup against the existing vault prevents duplicate notes.

The skill is the **eager** path for in-session writes — moments are captured at the time they happen, with user confirmation. The `SessionEnd` review (below) is the **retrospective** path — it sees the full transcript and catches anything the skill missed (especially quiet validations like "yeah, that was the right call" that lack a clear linguistic trigger).

**Why source-agnostic framing:** the original description gated on user actions ("when the user corrects / shares / validates"). That missed agent-discovered facts — channel IDs surfaced via a Slack search, collection UUIDs returned by an API call, configuration paths from a `--help` output. Reframing the trigger around the *nature of the fact* (stable / useful / non-derivable) rather than the *source* lifted recall from 88% to 100% on the existing fixture without negative-side regressions. See `tests/run_write_eval.py` and `cases-write.json`.

**Why the gate isn't also a skill:** the retrieval gate runs *before* Claude reasons about the prompt. Several of its categories — "decision rejustification", "implicit user-pref", "imperative-with-guardrail" — fire on prompts that have no surface cue ("set up a cron job" when there's a no-cron decision). A skill would only fire after Claude has already started down a path. The gate's specialization also matters quantitatively: a skill-style framing without the gate's "default to {}" tuning over-injects on meta and generic prompts. See `tests/run_gate_eval.py` results comparing `prompts/current.txt` vs `prompts/skill-style.txt`. The two skills (`vault-search`, `save-memory`) are the *reactive* layer; the gate is the *proactive* layer — both fire, neither replaces the other.

## `SessionEnd`

`hooks/scripts/session-end.sh` backgrounds a `claude -p` subprocess that:

1. Reads the transcript.
2. Writes a journal entry to `Journals/<project>/YYYY-MM-DD.md` (one file per project per day; appends a `## Session HH:MM` section if the file already exists for today's project, and rewrites the frontmatter `description` to summarize the full day). Cross-project days produce one file per project, each with a single-valued `project:` frontmatter.
3. Writes new notes proactively when ALL of:
   - the information is significant (correction, validated approach, decision, novel fact),
   - it will be useful in future sessions,
   - and no existing note already covers it (verified by typed search before writing — this is also what dedupes against in-session writes from the `save-memory` skill).
4. Modifies existing notes only on **explicit user correction** in the transcript — not inference. Inferred staleness is flagged for the next session. When a non-journal note is extended or corrected, its frontmatter `description` is rewritten if the one-line summary no longer fits — this keeps the `SessionStart` auto-overview accurate.
5. Runs an integrity + reconciliation pass over three corpora:
   - **(a) own writes** from steps 2–4 — frontmatter complete, wikilinks resolve, `description`-vs-body drift on extends/corrects.
   - **(b) journal-linked notes** — non-journal notes referenced from today's journal entry get the same `description`-vs-body check.
   - **(c) personal-vault `*.md` changes since last commit** (via the `vault_head` SHA recorded by `SessionStart`, falling back to working-tree-only diff) — *backlink reconciliation*:
     - **renamed (old → new)** → finds every incoming `[[wikilink]]` resolving to the old path (path-qualified literal match, or unambiguous bare basename) via `_vault.py incoming-wikilinks --target <old>` and auto-rewrites each to the new path (smallest edit, `|alias` text preserved). Listed under `## Backlink rewrites`.
     - **deleted** → lists incoming wikilinks under `## Broken backlinks (target deleted)`. Never auto-fixed — deletion may be intentional or a rename the diff couldn't infer.

   Backlink reconciliation in step (c) is scoped to the **personal vault only** — the project-vault is the user's repo and commits on the user's cadence, so renaming a project-vault note doesn't trigger an automatic rewrite. Run `/obsidian-memory:audit` if you need cross-corpus integrity.

6. Independently `git add -A && git commit`s any vault changes when `OBSIDIAN_MEMORY_AUTOCOMMIT=true` (default) — including backlink rewrites from step 5. Push is opt-in (`OBSIDIAN_MEMORY_AUTOPUSH=true`). Wrapped in `flock` to prevent concurrent sessions racing.

The hook returns immediately; the review runs in the background and logs to `/tmp/claude-memory-review.log`. The review subprocess uses `claude -p --strict-mcp-config --output-format json` (same MCP-isolation rationale as the gate); on success the wrapper extracts the call's `.usage` / `.total_cost_usd` / `.duration_ms` and appends a `review_call` event to `/tmp/claude-memory-usage/<session_id>.jsonl`.

**Disabling:** set `OBSIDIAN_MEMORY_REVIEW_ENABLED=false` in `~/.config/obsidian-memory/config.env` to skip the review entirely. Auto-commit of any vault writes from the in-session save-memory skill still fires. This is the single biggest token-cost lever — review reads the full transcript, which scales with session length.

**Transcript pre-filter:** before launching the reviewer, `session-end.sh` runs `scripts/_slim_transcript.py` to strip `tool_use` and `tool_result` content from the transcript and emit a compact dialogue version (user messages + assistant text + a one-line "used: Read, Bash, …" summary per assistant turn). On real sessions this reduces transcript size 94–96% (a 2.3MB transcript becomes ~95KB) while preserving the signal the reviewer acts on (decisions, corrections, validated approaches, novel facts). The reviewer reads the slim version via `Read` and writes notes from it; tool-call bodies that don't drive memory writes never enter the review's context. Disable via `OBSIDIAN_MEMORY_SLIM_TRANSCRIPT=false` to fall back to the raw transcript.

### Routing rules

Type-driven, single decision tree. Mirrors the save-memory skill (see [Federated project-vaults](#federated-project-vaults) for full details). Notes can carry multiple types (`type: [findings, decision]`); routing uses the first.

```
PRIMARY = types[0]

PRIMARY == journal     → Journals/<project>/<date>.md  (always, SessionEnd-only)
PRIMARY == tool        → Tools/<slug>.md        (always personal)
PRIMARY == preference  → Notes/<slug>.md        (project: tag if scoped)
PRIMARY ∈ {reference, findings, decision, learning}:
  cwd's project registered+enabled AND repo has matching folder
                        → repo-vault <folder>/<slug>.md
  otherwise             → Notes/<slug>.md       (project: tag if scoped)
```

Folder-match for the repo-vault path: case-insensitive on basename, top-level + one level under `docs/`. `decision → decisions/|adr/|decision-records/`; `findings → findings/|research/`; `learning → learnings/|lessons/`; `reference → references/`. Project-vault writes leave the repo's working tree dirty for the user to commit; the personal vault auto-commits at SessionEnd.

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

## Federated project-vaults

A "project-vault" is a project repo whose own markdown files participate as a second corpus alongside the personal vault. Same frontmatter format, same search/overview machinery, no mirroring or persisted cross-corpus state.

**Registration** is per-project, opt-in, one-shot:

- `~/.config/obsidian-memory/projects.json` is the single source of truth. Schema:
  ```json
  { "projects": { "/abs/path": { "enabled": bool, "project": "name" } } }
  ```
- `SessionStart` looks up cwd's git toplevel. If absent from `projects.json` and the repo has ≥1 candidate `.md`, it injects a one-time prompt asking the user once. Either answer (yes or no) is recorded; the prompt won't fire again.
- `_projects.py` (`lookup`/`register`/`list`/`path` subcommands) is the helper that reads and writes the file. Hooks, the audit slash command, and `statusline.py` all go through it.

**Corpus enumeration** is computed fresh each invocation — no persisted file list:

- `_project_docs.py enumerate <path>` runs `git ls-files` (tracked) + `git ls-files --others --exclude-standard` (untracked-not-gitignored), filters to `.md`, drops boilerplate (`LICENSE*`, `CHANGELOG*`, `CODE_OF_CONDUCT*`, `SECURITY*`, top-level dotfile dirs).
- New files added between sessions are picked up automatically; deleted/moved files don't leave stale registry entries.

**Init** (`init_project_vault.py`) runs on registration and silently on every subsequent SessionStart of an enabled repo:

- Idempotent on writes: files with any existing frontmatter (plugin's, SKILL.md, slash-command, etc.) are detected and skipped without further work.
- Reads, however, are paid every run: init opens each candidate `.md` to inspect the first few lines for a frontmatter block. On a typical 30-50 file repo this is ~10ms total; scales linearly with corpus size. We chose eager-on-every-session over a cache so teammate-added docs are surfaced the next session without a manual step. If this becomes a bottleneck on doc-heavy repos, the planned cheap fix is to track `last_init_head` per repo in `projects.json` and skip the file scan entirely when neither HEAD nor the working tree has changed.
- For files lacking frontmatter, batches them into one `claude -p` call to infer `type:` and `description:`. Type defaults to `reference` if the LLM call fails or the response is malformed.
- Never reorganizes the repo's folder structure — only adds frontmatter blocks.
- Skipped files include all SKILL.md, slash-command markdown, plugin templates, and test fixtures (because they all have other frontmatter conventions).

**Read federation:**

- `_vault.py search` and `overview` accept a `--project-vault <path>` flag. When set, both corpora are walked and results carry a `corpus` field (`personal` / `project`).
- The `_overview.sh` helper accepts a third positional arg for the project-vault path. Cache key spans `(personal_vault | project | repo_vault_path)`; cache freshness check (`find -newer`) walks both directories.
- The retrieval gate (`UserPromptSubmit`) resolves the project-vault path freshly per turn by re-reading `projects.json` directly (one `git rev-parse` + one `jq` read). Cheap, and it always reflects the live registry — so `/obsidian-memory:project enable|disable` takes effect immediately without restarting the session.

**Write routing** (save-memory). Notes can carry multiple types; the first drives routing.

```
PRIMARY = types[0]

PRIMARY == journal     → personal Journals/<project>/  (always; SessionEnd-only)
PRIMARY == tool        → personal Tools/      (always; cross-project by nature)
PRIMARY == preference  → personal Notes/      (always; project: tag if scoped)
PRIMARY ∈ {reference, findings, decision, learning}:
  project-vault enabled AND repo has matching folder
                        → project-vault <folder>/
  otherwise             → personal Notes/     (project: tag if scoped)
```

Folder match is case-insensitive on basename, top-level + one level under `docs/`. Patterns: `decision → decisions/|adr/|decision-records/`; `learning → learnings/|lessons/`; `reference → references/`. `_project_docs.py match-type-folder <repo> --type <t>` returns the matched folder (exit 0) or nothing (exit 1).

**Personal vault auto-commits at SessionEnd; project-vault never auto-commits.** The repo's working tree carries the change for you to review and commit on your own cadence.

**Audit** (`/obsidian-memory:audit`) operates on both corpora when the current project is registered + enabled. Wikilinks resolve within a corpus only — no cross-corpus link resolution, by design (avoiding the v1.4.0 mirroring drift problem).

**Status line** appends `• <project>` whenever the cwd's repo is registered + enabled, so the active project tag is visible at a glance:

```text
obsidian-memory • my-project 384.0k tok · 23.4%
```

## Recursion guard

The `SessionEnd` review and the retrieval gate both spawn `claude -p`. The subprocess fires its own `SessionStart`, `UserPromptSubmit`, and `SessionEnd` hooks — which would re-run the gate or the review. To prevent recursion, each hook is invoked with `CLAUDE_MEMORY_REVIEW=1` or `CLAUDE_MEMORY_GATE=1` on the subprocess environment; the affected scripts exit early when either is set.

## Adding a new project

There's nothing to scaffold per project — projects are tags, not folders. To bring a project's existing docs into Claude's awareness as a project-vault, run `/obsidian-memory:project enable` from inside the repo (or just answer `yes` to the registration prompt SessionStart shows the first time).
