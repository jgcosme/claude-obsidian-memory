# Changelog

## v2.1.1 — Actor enum validation + save-memory tightening

Patch release fixing one out-of-enum drift and hardening the writer + audit so it can't recur silently.

- **`save-memory` SKILL.md** — explicit "MUST be the literal string `skill`, not `save-memory`" callout for `created_by` / `updated_by`. Previous wording let the LLM drift to writing the skill's filename instead of the actor token.
- **`audit`** — now flags `created_by` / `updated_by` values outside the `{skill, hook, audit, init}` enum (same shape as the existing `unknown type` check). Optional fields, so legacy notes without them stay clean.
- **One-off vault data fix** — single note `Notes/slack-sessions-todo-vs-issues-policy.md` had `updated_by: save-memory` (LLM drift); normalized to `skill`.

## v2.1.0 — Frontmatter timestamps + actor attribution

Adds datetime-aware `created_at` / `updated_at` and per-actor `created_by` / `updated_by` fields to vault note frontmatter, replacing the date-only `created` field.

### What's new

- **`created_at`** — ISO 8601 with local offset (e.g. `2026-05-03T22:30:00+08:00`). Replaces the date-only `created`.
- **`updated_at`** — ISO 8601 with local offset. Bumped on every plugin-driven write. Manual edits in Obsidian intentionally leave it stale (`git log` is authoritative for true last-edit semantics).
- **`updated_by`** — last plugin-write actor: `skill | hook | audit | init`.
- **`created_by`** — original author at note creation, same vocabulary. Set once and never bumped.
- **`audit --fix-frontmatter`** — one-shot migration. Sources `created_at` from each note's git first-commit timestamp (falls back to file mtime); adds `updated_at` + `updated_by: audit` when missing; preserves frontmatter key order.
- **New search filters** — `--updated-after` / `--updated-before` accept ISO 8601 datetimes or bare dates (date-only input is treated as local midnight).

### Compatibility

- Reader path still accepts legacy `created:` (date-only) so unmigrated notes keep working. Filters fall back transparently.
- `SearchHit` JSON now exposes `created_at`, `updated_at`, `updated_by`, `created_by` (empty string when absent).
- Audit's required-fields check accepts either `created_at` or legacy `created`. `created_by` is intentionally not required (would falsely flag every legacy note).
- No backfill of `created_by` — the original author of legacy notes is unknown; `git blame` is the fallback.

### Migrating an existing vault

```
obsidian-memory audit --fix-frontmatter
```

Idempotent, safe to re-run. Migrates `created` → `created_at` and adds `updated_at` + `updated_by: audit`. Preserves any existing user edits.

Closes #4, #7.

## v2.0.0 — Rust port

**Breaking change.** All hook scripts and helpers ported from Python + bash to a single static Rust binary, distributed via GitHub Releases.

### What changed for users

- **Prereqs simplified.** `jq` and `python3` are no longer required. First-run install needs `curl` (or `wget`) and `tar` to fetch the prebuilt binary; everything after that is the binary alone.
- **Faster.** Hook hot paths (overview, search, audit, slim-transcript) run ~6–10× faster than the v1 Python+bash implementation.
- **Single binary.** All entry points (`hook session-start`, `audit`, `setup`, `usage`, `status`, `init-project`, `projects`, etc.) live inside one `obsidian-memory` binary at `$PLUGIN_ROOT/bin/obsidian-memory`. The `bin/run` wrapper handles lazy install on first session.
- **Slash commands relocated.** `/obsidian-memory:status`, `:usage`, `:audit`, `:project` now invoke the binary; output format is unchanged.
- **Setup heredoc updated.** First-time-setup prompt now references `bin/run setup` instead of `bash setup.sh`. The new pattern survives plugin-version upgrades.

### What stays the same

- Vault layout (`Tools/`, `Notes/`, `Journals/<project>/<date>.md`).
- Frontmatter schema (`type`, `description`, `created`, `project`).
- All seven memory types and the multi-type `[a, b]` form.
- `~/.config/obsidian-memory/` config + projects.json registry. Existing files carry over.
- `vault-search` and `save-memory` skills.
- Hook output text (verified byte-equal under the v1↔v2 parity harness across 99 test cases before deletion).

### Distribution

Prebuilt binaries are published to GitHub Releases for `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, and `aarch64-unknown-linux-gnu`. Each tarball ships with a `.sha256` checksum. The release workflow (`.github/workflows/release.yml`) cross-builds on tag push.

### Upgrading from v1.x

1. `/plugin update obsidian-memory@jgcosme-plugins` (or `/plugin uninstall` followed by `/plugin install`).
2. Start a new Claude session — `bin/run` fetches the binary on first hook fire.
3. Existing vault, config, and projects.json are picked up unchanged.

If you had `OBSIDIAN_MEMORY_*` env overrides in `~/.config/obsidian-memory/config.env`, they continue to work — the parser is identical.

### Known divergences from v1

- The optional pyyaml deep-validation pass in `audit` is gone (Python-only feature, conditional on pyyaml install). Schema-level frontmatter checks remain.
- The `init-project` LLM batch call has no built-in timeout in v2 (v1 used Python's `subprocess.run(timeout=180)`). Practically unobserved; will add `wait-timeout` if a user reports a hung claude binary.
- `audit`'s `Generated:` timestamp is local time as v1; rendering matches.
