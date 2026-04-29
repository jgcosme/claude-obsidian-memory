---
description: Capture an in-session memory note to the user's Obsidian vault. Invoke when the user makes a correction worth remembering across sessions, validates a non-obvious approach, sets a preference ("from now on...", "always...", "stop doing X"), explicitly asks to remember something, or shares a novel cross-session fact (config detail, person, ID, GROUP_ID). Skip ordinary task instructions, agreements ("ok", "thanks"), ephemeral edits, refactors, and generic technical questions.
---

# save-memory

You've judged the latest moment as save-worthy. Now: search first to avoid duplicates, decide where it goes, propose to the user, then write on confirmation.

The vault path is `$OBSIDIAN_VAULT_PATH` (or `~/Documents/Obsidian Vault` if unset). The plugin path is `${CLAUDE_PLUGIN_ROOT}`.

## 1. Search the vault first

A near-duplicate is more useful than a new note. Run:

```bash
python3 "${CLAUDE_PLUGIN_ROOT}/scripts/_vault.py" search \
  --vault "$OBSIDIAN_VAULT_PATH" \
  --keywords "<2-4 keywords from the moment>" \
  --json
```

If a match exists, propose **extending** that note rather than creating a new one. Read the match first; preserve its body and append.

## 2. Route the note

Three buckets. Pick exactly one.

1. **Personal / cross-project** → vault.
   Pick the subfolder that fits: `General/Preferences/`, `General/References/`, `Tools/`, `General/People/`.

2. **Project-related AND the project has internal docs** (`docs/`, ADR folders, mkdocs/sphinx, CONTRIBUTING) → reflect upstream in the repo. **No vault note.**
   Allowed paths: `docs/` tree, ADR folders, `*.md` under `docs/`. Never source, configs, CI, or manifests. The journal entry written by `SessionEnd` will mention the repo path — that's the cross-session anchor.
   WIP guard: run `git -C "$CLAUDE_PROJECT_DIR" status --porcelain -- <target>`. Non-empty → skip the write and tell the user where you would have written so they can revisit.

3. **Project-related AND no project docs** → substantive vault note at `Projects/<name>/{Decisions,Learnings}/<slug>.md`.

## 3. Frontmatter (required on every new note)

```yaml
---
type: <preference|reference|decision|learning|tool|people|feedback>
description: one-line hook
created: YYYY-MM-DD
project: <name>     # only if project-scoped
---
```

`type` must match the route:

- `General/Preferences/` → `preference`
- `General/References/`  → `reference`
- `Tools/`               → `tool`
- `General/People/`      → `people`
- `Projects/<n>/Decisions/` → `decision`
- `Projects/<n>/Learnings/` → `learning`

For correction-style memories (user told you to stop doing something or to do it differently from now on), prefer `feedback` as the type and route to `General/Preferences/` or `Projects/<n>/Decisions/` depending on scope.

## 4. Propose, then write

Before writing, show the user a compact preview:

```
save-memory: would write
  path:         <full path>
  type:         <type>
  description:  <one line>
  body:         <2-4 sentences>

save? (y/n)
```

On `y`, write the file with the Write tool. Don't run `git add` / `git commit` — the SessionEnd hook auto-commits the vault after review.

On `n` or anything else, drop it and return to the user's task.

## 5. Body shape

For `preference` / `feedback` notes, lead with the rule itself, then a `**Why:**` line (the user-given reason) and a `**How to apply:**` line (when/where this kicks in). Knowing *why* lets future sessions judge edge cases instead of blindly following the rule.

For `decision` / `learning` notes, lead with the decision or finding, then briefly state the alternatives considered or the cause of the gotcha, then how to apply.

Keep the body short. Three sentences with the right structure beat ten paragraphs of prose.

## When NOT to invoke

- The user is doing ordinary work — coding, refactoring, fixing typos.
- The user said "thanks" / "ok" / "yes" — agreements aren't memory.
- The fact is hyper-local to a single file, PR, or commit (it'll be in the diff).
- The vault already covers this — your search found a match. Extend, don't duplicate.
- You're uncertain whether it's save-worthy. The SessionEnd review acts as a backstop for moments you skip.

## Limits

- Don't invoke more than once per turn.
- Don't write to `~/.claude/projects/*/memory/` — that path is disabled in favor of this vault.
- Don't modify existing non-journal notes unless the user explicitly corrects something. Smallest edit only.
