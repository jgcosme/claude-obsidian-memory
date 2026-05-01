---
description: Capture an in-session memory note to the Obsidian vault. Invoke whenever the conversation surfaces information that is stable across sessions, useful in future sessions, and not derivable from the codebase or git history — regardless of source (user-stated or tool-discovered). Covers corrections, preferences, validated approaches, always / from now on / stop doing X rules, decisions and rationale, and novel facts (people, IDs, configs, channels, dashboards, endpoints). Skip agreements (ok, thanks), in-progress task state, and anything visible in the diff.
---

# save-memory

You've judged the latest moment as save-worthy. Now: search first to avoid duplicates, decide where it goes, propose to the user, then write on confirmation.

The personal vault is `$OBSIDIAN_VAULT_PATH` (or `~/Documents/Obsidian Memory` if unset). The plugin path is `${CLAUDE_PLUGIN_ROOT}`. The current project's project-vault status is in `~/.config/obsidian-memory/projects.json` — check via `_projects.py lookup` (below).

## 1. Search first

A near-duplicate is more useful than a new note. Run:

```bash
python3 "${CLAUDE_PLUGIN_ROOT}/scripts/_vault.py" \
  --vault "$OBSIDIAN_VAULT_PATH" \
  search \
  --keywords "<2-4 keywords from the moment>" \
  --project-vault "$CLAUDE_PROJECT_DIR" \
  --json
```

The `--project-vault` arg is harmless when the project isn't registered (no extra results). Results carry a `corpus` field — `personal` or `project`. If a match exists, propose **extending** that note rather than creating a new one. Read the match first; preserve its body and append.

## 2. Pick the type (one of six)

| Type | What it captures |
|---|---|
| `preference` | behavioral rule ("always do X", "stop doing Y") |
| `reference` | factual lookup (URLs, IDs, configs, findings, channels, endpoints) |
| `decision` | choice rationale ("we chose X because Y") |
| `learning` | discovered insight, gotcha, or fix |
| `tool` | how to use a CLI/API/service |
| `journal` | **never written by this skill** — SessionEnd handles journals |

## 3. Route the note

Apply this rule. Pick exactly one destination:

```
A. type == tool
   → $OBSIDIAN_VAULT_PATH/Tools/<slug>.md
     (Tools are always personal-vault and cross-project; no project: tag.)

B. type == preference
   → $OBSIDIAN_VAULT_PATH/Notes/<slug>.md
     (Add project: tag only if the rule is narrowly scoped to one project.)

C. type in {reference, decision, learning}:
   1. Look up cwd's project-vault status:
        STATUS=$(python3 "${CLAUDE_PLUGIN_ROOT}/scripts/_projects.py" lookup "$CLAUDE_PROJECT_DIR")

   2. If STATUS == enabled, ask whether a matching repo folder exists:
        FOLDER=$(python3 "${CLAUDE_PLUGIN_ROOT}/scripts/_project_docs.py" \
                  match-type-folder "$CLAUDE_PROJECT_DIR" --type <type>)
      If exit=0, FOLDER is the repo-relative path (e.g. `docs/decisions`).

   3. Decide:
        STATUS=enabled AND FOLDER non-empty
            → $CLAUDE_PROJECT_DIR/$FOLDER/<slug>.md  (project-vault note)
        otherwise
            → $OBSIDIAN_VAULT_PATH/Notes/<slug>.md   (personal-vault note)

      Add `project:` tag whenever the memory is project-scoped, regardless of
      where the note lands. The tag value comes from `_projects.py lookup --json`
      when registered, else the repo basename, else omit.
```

The project-vault path is `$CLAUDE_PROJECT_DIR` (or `git -C $CLAUDE_PROJECT_DIR rev-parse --show-toplevel` if cwd is a subdir of the repo).

WIP guard for project-vault writes: before writing to an *existing* file in the repo, run `git -C "$CLAUDE_PROJECT_DIR" status --porcelain -- <target>`. Non-empty → skip and tell the user where you would have written. New files in the repo skip this guard.

## 4. Frontmatter (required on every new note)

```yaml
---
type: <preference|reference|decision|learning|tool>
description: "one-line hook"
created: YYYY-MM-DD
project: <name>     # only if project-scoped
---
```

Always wrap `description:` in double quotes — descriptions commonly contain `:`, `[[wikilinks]]`, or `[brackets]`, all of which break unquoted YAML and silently truncate the description in the auto-overview. Escape embedded `"` as `\"`.

## 5. Propose, then write

Before writing, show the user a compact preview:

```
save-memory: would write
  corpus:       personal | project:<name>
  path:         <full path>
  type:         <type>
  description:  <one line>
  body:         <2-4 sentences>

save? (y/n)
```

On `y`, write the file with the Write tool. Don't run `git add` / `git commit` — the SessionEnd hook auto-commits the personal vault. Project-vault writes leave the repo's working tree dirty for the user to review and commit on their own cadence.

On `n` or anything else, drop it and return to the user's task.

## 6. Body shape

For `preference` notes, lead with the rule itself, then a `**Why:**` line (the user-given reason) and a `**How to apply:**` line (when/where this kicks in). Knowing *why* lets future sessions judge edge cases instead of blindly following the rule.

For `decision` / `learning` notes, lead with the decision or finding, then briefly state the alternatives considered or the cause of the gotcha, then how to apply.

For `reference` notes, lead with the fact (URL, ID, command, finding). Add only context that's not derivable from the fact itself.

For `tool` notes, lead with install path / binary location, then auth credential location (variable name only — never the value), scopes/permissions, key commands, and gotchas.

Keep the body short. Three sentences with the right structure beat ten paragraphs of prose.

## When NOT to invoke

- The user is doing ordinary work — coding, refactoring, fixing typos.
- The user said "thanks" / "ok" / "yes" — agreements aren't memory.
- The fact is hyper-local to a single file, PR, or commit (it'll be in the diff).
- The vault already covers this — your search found a match. Extend, don't duplicate.
- You're uncertain whether it's save-worthy. The SessionEnd review acts as a backstop for moments you skip.

## Limits

- Don't invoke more than once per turn.
- Don't write `type: journal` from this skill — that's SessionEnd's job.
- Don't write to `~/.claude/projects/*/memory/` — that path is disabled in favor of this vault.
- Don't modify existing notes unless the user explicitly corrects something. Smallest edit only.
