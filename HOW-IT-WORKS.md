---
type: reference
description: "Explains the three lifecycle hooks (SessionStart, UserPromptSubmit, SessionEnd) and how the plugin operates"
created: 2026-04-30
project: claude-obsidian-memory
---

# How it works

The plugin runs three hooks across the Claude Code session lifecycle.

## `SessionStart`

`hooks/scripts/session-start.sh` runs on every new session and:

1. **First-time setup gate.** If `$OBSIDIAN_VAULT_PATH` doesn't exist, the hook injects a consent prompt instructing Claude to ask the user once before running `setup.sh` (and offering an optional `git init`). It then exits early — none of the steps below run until the vault exists. On the next session after scaffolding, the normal flow takes over.
2. Tries to launch Obsidian.app if available (purely so the optional `obsidian` CLI works). No-op on Linux.
3. Derives the project name from `$CLAUDE_PROJECT_DIR` basename (or `$PWD`).
4. **Repo-vault registry lookup.** Resolves cwd to its git toplevel and looks the path up in `~/.config/obsidian-memory/repos.json` via `_repos.py lookup`. Three states:
   - `not_registered` + ≥1 candidate `.md` (per `_repo_docs.enumerate_repo_docs`) → injects a one-time registration prompt instructing Claude to ask the user "Register '<project>' as a repo-vault?". On `y`, Claude runs `init_repo_vault.py` (adds plugin frontmatter to files lacking any) and `_repos.py register --enabled`. On `n`, just `_repos.py register --no-enabled`. Either answer is durable in `repos.json`.
   - `enabled` → runs `init_repo_vault.py` silently and writes the resolved repo path to `/tmp/claude-memory-session/<session_id>.repo_vault` so `UserPromptSubmit` can re-use it without re-querying the registry. Init is idempotent on writes (only files missing frontmatter get LLM-classified and written) but reads every candidate `.md` each run to make that decision — typically ~10ms total on a 30-50 file repo, scaling linearly with corpus size. The eager run is what catches teammate-added docs without a manual step.
   - `disabled` → silent.
5. Injects into context:
   - **Bootstrap instructions** — vault path + two-line skill pointers (`vault-search` for body-level lookups the gate's description-match misses, `save-memory` for writes). The CLI syntax, routing rules, frontmatter schema, and when-to-invoke heuristics live in each skill's body, not the bootstrap, so they only load when Claude invokes the skill.
   - **Vault `README.md`** — prose orientation.
   - **Auto-generated vault overview** — produced by the shared `_overview.sh` helper (cached at `$MEMORY_OVERVIEW_CACHE_DIR`, invalidated by vault `*.md` mtimes; cache key spans personal vault + repo-vault path). Lists Tools, General, and the current project's notes from the personal vault. When the project has a registered+enabled repo-vault, an additional `# Repo vault: <project>` section follows, grouped by frontmatter `type:` (no enforced folder structure in repo-vaults).
   - **Project-scaffolding prompt** if `Projects/<name>/` doesn't exist (legacy pre-v1.6 personal-vault layout — only fires for vaults still on that structure).
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
   [obsidian-memory] vault → Tools/Slack.md, General/References/secrets-env.md
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
2. Writes a journal entry to `Projects/<project>/Journal/YYYY-MM-DD.md` (appends a `## Session HH:MM` section if the file already exists for today, and rewrites the frontmatter `description` to summarize the full day). Skipped if `Projects/<project>/` doesn't exist.
3. Writes new notes proactively when ALL of:
   - the information is significant (correction, validated approach, decision, novel fact),
   - it will be useful in future sessions,
   - and no existing note already covers it (verified by typed search before writing — this is also what dedupes against in-session writes from the `save-memory` skill).
4. Modifies existing notes only on **explicit user correction** in the transcript — not inference. Inferred staleness is flagged for the next session. When a non-journal note is extended or corrected, its frontmatter `description` is rewritten if the one-line summary no longer fits — this keeps the `SessionStart` auto-overview accurate.
5. Runs an integrity + reconciliation pass over three corpora:
   - **(a) own writes** from steps 2–4 — frontmatter complete, wikilinks resolve, `description`-vs-body drift on extends/corrects.
   - **(b) journal-linked notes** — non-journal notes referenced from today's journal entry get the same `description`-vs-body check.
   - **(c) vault `*.md` changes since last commit** (via the `vault_head` SHA recorded by `SessionStart`, falling back to working-tree-only diff) — *backlink reconciliation*:
     - **renamed (old → new)** → finds every incoming `[[wikilink]]` resolving to the old path (path-qualified literal match, or unambiguous bare basename) via `_vault.py incoming-wikilinks --target <old>` and auto-rewrites each to the new path (smallest edit, `|alias` text preserved). Listed under `## Backlink rewrites`.
     - **deleted** → lists incoming wikilinks under `## Broken backlinks (target deleted)`. Never auto-fixed — deletion may be intentional or a rename the diff couldn't infer.

6. Independently `git add -A && git commit`s any vault changes when `OBSIDIAN_MEMORY_AUTOCOMMIT=true` (default) — including backlink rewrites from step 5. Push is opt-in (`OBSIDIAN_MEMORY_AUTOPUSH=true`). Wrapped in `flock` to prevent concurrent sessions racing.

The hook returns immediately; the review runs in the background and logs to `/tmp/claude-memory-review.log`. The review subprocess uses `claude -p --strict-mcp-config --output-format json` (same MCP-isolation rationale as the gate); on success the wrapper extracts the call's `.usage` / `.total_cost_usd` / `.duration_ms` and appends a `review_call` event to `/tmp/claude-memory-usage/<session_id>.jsonl`.

**Disabling:** set `OBSIDIAN_MEMORY_REVIEW_ENABLED=false` in `~/.config/obsidian-memory/config.env` to skip the review entirely. Auto-commit of any vault writes from the in-session save-memory skill still fires. This is the single biggest token-cost lever — review reads the full transcript, which scales with session length.

**Transcript pre-filter:** before launching the reviewer, `session-end.sh` runs `scripts/_slim_transcript.py` to strip `tool_use` and `tool_result` content from the transcript and emit a compact dialogue version (user messages + assistant text + a one-line "used: Read, Bash, …" summary per assistant turn). On real sessions this reduces transcript size 94–96% (a 2.3MB transcript becomes ~95KB) while preserving the signal the reviewer acts on (decisions, corrections, validated approaches, novel facts). The reviewer reads the slim version via `Read` and writes notes from it; tool-call bodies that don't drive memory writes never enter the review's context. Disable via `OBSIDIAN_MEMORY_SLIM_TRANSCRIPT=false` to fall back to the raw transcript.

### Routing rules

Three buckets:

1. **Personal / cross-project** → vault (`General/Preferences/`, `General/References/`, `Tools/`, `General/People/`).
2. **Project-related AND project has internal docs** (`docs/`, ADR folders, mkdocs/sphinx, CONTRIBUTING) → reflect upstream as an uncommitted doc edit in the repo. No vault note. The journal entry mentions the repo path; that's the cross-session anchor.
3. **Project-related AND no project docs** → substantive vault note at `Projects/<name>/{Decisions,Learnings}/`.

Project-repo writes are restricted to the docs tree — never source, configs, CI, or manifests. WIP-guarded by `git status --porcelain` on the target; if dirty, the write is skipped and recorded under `## Integrity flags`.

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

## Federated repo-vaults

A "repo-vault" is a project repo whose own markdown files participate as a second corpus alongside the personal vault. Same frontmatter format, same search/overview machinery, no mirroring or persisted cross-corpus state.

**Registration** is per-project, opt-in, one-shot:

- `~/.config/obsidian-memory/repos.json` is the single source of truth. Schema:
  ```json
  { "repos": { "/abs/path": { "enabled": bool, "project": "name" } } }
  ```
- `SessionStart` looks up cwd's git toplevel. If absent from `repos.json` and the repo has ≥1 candidate `.md`, it injects a one-time prompt asking the user once. Either answer (yes or no) is recorded; the prompt won't fire again.
- `_repos.py` (`lookup`/`register`/`list`/`path` subcommands) is the helper that reads and writes the file. Hooks, the audit slash command, and `statusline.py` all go through it.

**Corpus enumeration** is computed fresh each invocation — no persisted file list:

- `_repo_docs.py enumerate <path>` runs `git ls-files` (tracked) + `git ls-files --others --exclude-standard` (untracked-not-gitignored), filters to `.md`, drops boilerplate (`LICENSE*`, `CHANGELOG*`, `CODE_OF_CONDUCT*`, `SECURITY*`, top-level dotfile dirs).
- New files added between sessions are picked up automatically; deleted/moved files don't leave stale registry entries.

**Init** (`init_repo_vault.py`) runs on registration and silently on every subsequent SessionStart of an enabled repo:

- Idempotent on writes: files with any existing frontmatter (plugin's, SKILL.md, slash-command, etc.) are detected and skipped without further work.
- Reads, however, are paid every run: init opens each candidate `.md` to inspect the first few lines for a frontmatter block. On a typical 30-50 file repo this is ~10ms total; scales linearly with corpus size. We chose eager-on-every-session over a cache so teammate-added docs are surfaced the next session without a manual step. If this becomes a bottleneck on doc-heavy repos, the planned cheap fix is to track `last_init_head` per repo in `repos.json` and skip the file scan entirely when neither HEAD nor the working tree has changed.
- For files lacking frontmatter, batches them into one `claude -p` call to infer `type:` and `description:`. Type defaults to `reference` if the LLM call fails or the response is malformed.
- Never reorganizes the repo's folder structure — only adds frontmatter blocks.
- Skipped files include all SKILL.md, slash-command markdown, plugin templates, and test fixtures (because they all have other frontmatter conventions).

**Read federation:**

- `_vault.py search` and `overview` accept a `--repo-vault <path>` flag. When set, both corpora are walked and results carry a `corpus` field (`personal` / `repo`).
- The `_overview.sh` helper accepts a third positional arg for the repo-vault path. Cache key spans `(personal_vault | project | repo_vault_path)`; cache freshness check (`find -newer`) walks both directories.
- The retrieval gate (`UserPromptSubmit`) reads the repo-vault path from `/tmp/claude-memory-session/<session_id>.repo_vault` (written by `SessionStart`) so it doesn't re-query the registry per turn.

**Write routing** (save-memory):

```
type == journal     → personal Journals/   (always; SessionEnd-only)
type == tool        → personal Tools/      (always; cross-project by nature)
type == preference  → personal Notes/      (always; project: tag if scoped)
type ∈ {reference, decision, learning}:
  repo-vault enabled AND repo has matching folder
                     → repo-vault <folder>/
  otherwise          → personal Notes/     (project: tag if scoped)
```

Folder match is case-insensitive on basename, top-level + one level under `docs/`. Patterns: `decision → decisions/|adr/|decision-records/`; `learning → learnings/|lessons/`; `reference → references/`. `_repo_docs.py match-type-folder <repo> --type <t>` returns the matched folder (exit 0) or nothing (exit 1).

**Personal vault auto-commits at SessionEnd; repo-vault never auto-commits.** The repo's working tree carries the change for you to review and commit on your own cadence.

**Audit** (`/obsidian-memory:audit`) operates on both corpora when the current project is registered + enabled. Wikilinks resolve within a corpus only — no cross-corpus link resolution, by design (avoiding the v1.4.0 mirroring drift problem).

**Status line** appends `• <project>` whenever the cwd's repo is registered + enabled, so the active project tag is visible at a glance:

```text
obsidian-memory • my-project 384.0k tok · 23.4%
```

## Recursion guard

The `SessionEnd` review and the retrieval gate both spawn `claude -p`. The subprocess fires its own `SessionStart`, `UserPromptSubmit`, and `SessionEnd` hooks — which would re-run the gate or the review. To prevent recursion, each hook is invoked with `CLAUDE_MEMORY_REVIEW=1` or `CLAUDE_MEMORY_GATE=1` on the subprocess environment; the affected scripts exit early when either is set.

## Adding a new project

`cd` into the project and start a session. `SessionStart` detects the missing `Projects/<basename>/` folder and instructs Claude to ask you once. Answer **yes** and Claude:

1. Creates `Projects/<name>/{Decisions,Learnings,Research,References,Journal}` and renders `overview.md` from the template.
2. Inspects the project dir — top-level docs (README, ARCHITECTURE, CONTRIBUTING), package manifests, /docs entry points — enough to populate `overview.md`'s curated sections. Goal: stable conceptual summary, not a doc index.
3. Populates `overview.md` with the standard section headings (`## What it is`, `## Goals`, `## Current branch / focus`, `## Stakeholders`, `## Notes`), citing source files inline. Sections without grounded evidence are left empty. Keeps it short — a concise overview that stays accurate beats a comprehensive one that drifts.
4. Leaves `Decisions/`, `Learnings/`, `Research/`, `References/`, `Journal/` **empty**. They fill organically:
   - `Journal/` — `SessionEnd` writes one entry per day.
   - `Decisions/`, `Learnings/`, `Research/`, `References/` — `save-memory` writes here when significant moments happen in-session.

   Bulk-importing or mirroring repo docs is intentionally **not** part of scaffolding. Repo docs stay in the repo; Claude greps them when needed. Each vault note represents a curated memory moment, not a copy.

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
