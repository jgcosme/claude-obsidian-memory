---
description: Run a full vault integrity audit — frontmatter, broken wikilinks, orphans, duplicate basenames, frontmatter backfill for missing notes, and description-vs-body drift. Operates on the personal vault and (when registered) the current project's project-vault.
---

Whole-corpus integrity check. Mirrors what SessionEnd does for the current session, but across the entire vault(s) — large blast radius, so anything LLM-judged is propose-then-confirm.

## Step 1 — structural audit (deterministic, always)

Run the audit script. Includes the current project's project-vault if registered + enabled.

```bash
PROJECT_DIR="${CLAUDE_PROJECT_DIR:-$PWD}"
PROJECT_ROOT=$(git -C "$PROJECT_DIR" rev-parse --show-toplevel 2>/dev/null || true)
PLUGIN_RUN="${CLAUDE_PLUGIN_ROOT:-$(ls -d ~/.claude/plugins/cache/jgcosme-plugins/obsidian-memory/*/ 2>/dev/null | sort -V | tail -1 | sed 's:/$::')}/bin/run"
PROJECT_VAULT_ARG=""
if [ -n "$PROJECT_ROOT" ]; then
  STATUS=$("$PLUGIN_RUN" projects lookup "$PROJECT_ROOT" 2>/dev/null || echo "")
  if [ "$STATUS" = "enabled" ]; then
    PROJECT_VAULT_ARG="--project-vault $PROJECT_ROOT"
  fi
fi
"$PLUGIN_RUN" audit $PROJECT_VAULT_ARG
```

Group the script's output by category, highest-impact first (broken wikilinks > missing frontmatter > orphans > duplicate basenames). For each issue, propose the smallest fix. Auto-fixes aren't applied at this step — the user picks which to act on.

If the structural audit comes back clean, say so plainly and continue to Step 2.

## Step 2 — frontmatter backfill (LLM-judged, propose then apply)

For files flagged in Step 1 with `no frontmatter block`, propose plugin frontmatter using the binary's `init-project` heuristics. This is mostly relevant for the project-vault corpus (personal-vault writes go through save-memory and won't reach audit missing-frontmatter unless something went wrong).

When the missing-frontmatter list is non-empty AND scoped to the project-vault:

```bash
PROJECT_NAME=$("$PLUGIN_RUN" projects lookup "$PROJECT_ROOT" --json | grep -o '"project":[[:space:]]*"[^"]*"' | sed -E 's/.*"([^"]+)"$/\1/')
"$PLUGIN_RUN" init-project "$PROJECT_ROOT" --project "$PROJECT_NAME" --dry-run
```

Show the proposed frontmatter for each file. Ask the user once: "Apply frontmatter to N files? (y/n)". On `y`, re-run the same command without `--dry-run`. On `n`, list the paths and skip.

When missing-frontmatter is in the personal vault: list the paths under `## Personal-vault frontmatter to fix`. Do NOT auto-write — the user fixes those manually (these are usually outliers worth investigating, not bulk backfill).

## Step 3 — description drift (LLM-judged, propose then apply)

For every note with frontmatter, judge whether the `description:` field still summarizes the body. Skip notes with no `description`. Account for substantial appends, scope shifts, or (for journals) sessions added after the description was written.

Walk both corpora — start with the personal vault, then the project-vault if registered.

Report findings under `## Description drift` with one entry per drifted note:

- path
- corpus (`personal` / `project`)
- current `description`
- suggested replacement (one line, ≤120 chars)
- one-line rationale for why it drifted

After listing, ask the user: "Apply N description rewrites? (y/n)". On `y`, edit each flagged file's frontmatter line via Edit. Don't touch other fields. On `n`, skip — the report alone is value.

If no drift is found, say so plainly and skip the section.

## What this command does NOT do

- **Auto-rewrite broken wikilinks**: report only. Audit is a snapshot — it can't tell whether a broken link is a typo or a since-renamed file. Renames during a session are handled by SessionEnd's backlink reconciliation.
- **Cross-corpus operations**: wikilinks resolve within a corpus. Personal-vault notes can't link to project-vault docs (or vice versa) — that's by design, to avoid the v1.4.0 drift surface that mirroring introduced.
- **Auto-commit fixes**: any writes from Step 2 or Step 3 leave the working tree dirty for the user to review and commit on their own cadence (personal vault gets auto-committed at SessionEnd; project-vault never).
