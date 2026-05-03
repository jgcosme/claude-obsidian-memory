---
description: Capture an in-session memory note to the Obsidian vault. Invoke whenever the conversation surfaces information that is stable across sessions, useful in future sessions, and not derivable from the codebase or git history — regardless of source (user-stated or tool-discovered). Covers corrections, preferences, validated approaches, always / from now on / stop doing X rules, decisions and rationale, and novel facts (people, IDs, configs, channels, dashboards, endpoints). Skip agreements (ok, thanks), in-progress task state, and anything visible in the diff.
---

# save-memory

You've judged the latest moment as save-worthy. Now: search first to avoid duplicates, decide where it goes, propose to the user, then write on confirmation.

The personal vault is `$OBSIDIAN_VAULT_PATH` (or `~/Documents/Obsidian Memory` if unset). The plugin binary lives at `${CLAUDE_PLUGIN_ROOT}/bin/run` (a thin wrapper that ensures the binary is installed, then execs it). The current project's project-vault status is in `~/.config/obsidian-memory/projects.json` — check via `obsidian-memory projects lookup` (below).

## 1. Search first

A near-duplicate is more useful than a new note. Build the search command, adding `--project-vault` only when the current project is registered + enabled in `projects.json`:

```bash
PLUGIN_RUN="${CLAUDE_PLUGIN_ROOT}/bin/run"
PROJECT_ROOT=$(git -C "${CLAUDE_PROJECT_DIR:-$PWD}" rev-parse --show-toplevel 2>/dev/null || true)
PROJECT_VAULT_ARG=""
if [ -n "$PROJECT_ROOT" ]; then
  STATUS=$("$PLUGIN_RUN" projects lookup "$PROJECT_ROOT" 2>/dev/null || echo "")
  [ "$STATUS" = "enabled" ] && PROJECT_VAULT_ARG="--project-vault $PROJECT_ROOT"
fi
"$PLUGIN_RUN" vault \
  --vault "$OBSIDIAN_VAULT_PATH" \
  search \
  --keywords "<2-4 keywords from the moment>" \
  $PROJECT_VAULT_ARG \
  --json
```

Results carry a `corpus` field — `personal` or `project`. If a match exists, propose **extending** that note rather than creating a new one. Read the match first; preserve its body and append.

(Always passing `--project-vault $CLAUDE_PROJECT_DIR` would walk an unregistered repo's markdown too — any third-party `.md` with frontmatter would surface as a false dedup hit. Gate on `STATUS = enabled` so dedup only sees real corpus members.)

## 2. Pick the type(s)

The canonical type definitions live in `${CLAUDE_PLUGIN_ROOT}/templates/types.md` — read that file when you need the full semantics. Quick reference:

| Type | What it captures |
|---|---|
| `reference` | atomic factual lookup (URLs, IDs, configs, channels, endpoints) |
| `findings` | synthesis from reading multiple sources — territory map / comparison |
| `learning` | easy-to-miss gotchas and fixes — foot-gun database |
| `decision` | choice rationale ("we chose X because Y") |
| `preference` | behavioral rule ("always do X", "stop doing Y") |
| `tool` | how to use a CLI/API/service |
| `journal` | **never written by this skill** — SessionEnd handles journals |

A note can carry multiple types when it genuinely spans axes — e.g. a research investigation that ended in a choice is `[findings, decision]`. Order by routing precedence (first type drives the destination folder). Single-type notes stay as a bare string.

**Be proactive with `findings`.** When you've spent a turn (or several) reading multiple sources, comparing options, or mapping a territory, capture the synthesis as a `findings` note before moving on — the bar is "would a future session repeat this investigation if I didn't write it down?".

## 3. Route the note

The first type in the list drives routing. Apply this rule:

```
PRIMARY = types[0]

A. PRIMARY == tool
   → $OBSIDIAN_VAULT_PATH/Tools/<slug>.md
     (Tools are always personal-vault and cross-project; no project: tag.)

B. PRIMARY == preference
   → $OBSIDIAN_VAULT_PATH/Notes/<slug>.md
     (Add project: tag only if the rule is narrowly scoped to one project.)

C. PRIMARY in {reference, findings, decision, learning}:
   1. Look up cwd's project-vault status:
        STATUS=$("$PLUGIN_RUN" projects lookup "$CLAUDE_PROJECT_DIR")

   2. If STATUS == enabled, ask whether a matching repo folder exists:
        FOLDER=$("$PLUGIN_RUN" project-docs match-type-folder \
                  "$CLAUDE_PROJECT_DIR" --type <PRIMARY>)
      If exit=0, FOLDER is the repo-relative path (e.g. `docs/decisions`).

   3. Decide:
        STATUS=enabled AND FOLDER non-empty
            → $CLAUDE_PROJECT_DIR/$FOLDER/<slug>.md  (project-vault note)
        otherwise
            → $OBSIDIAN_VAULT_PATH/Notes/<slug>.md   (personal-vault note)

      Add `project:` tag whenever the memory is project-scoped, regardless of
      where the note lands. The tag value comes from `obsidian-memory projects lookup --json`
      when registered, else the repo basename, else omit.
```

The project-vault path is `$CLAUDE_PROJECT_DIR` (or `git -C $CLAUDE_PROJECT_DIR rev-parse --show-toplevel` if cwd is a subdir of the repo).

WIP guard for project-vault writes: before writing to an *existing* file in the repo, run `git -C "$CLAUDE_PROJECT_DIR" status --porcelain -- <target>`. Non-empty → skip and tell the user where you would have written. New files in the repo skip this guard.

## 4. Frontmatter (required on every new note)

Single type:

```yaml
---
type: decision
description: "one-line hook"
created_at: 2026-05-03T22:30:00+08:00   # ISO 8601, current local time with offset
updated_at: 2026-05-03T22:30:00+08:00   # same as created_at on first write
updated_by: skill                        # this skill is the writer
project: <name>                          # only if project-scoped
---
```

Multi-type (note genuinely spans axes — first type drives routing):

```yaml
---
type: [findings, decision]
description: "one-line hook"
created_at: 2026-05-03T22:30:00+08:00
updated_at: 2026-05-03T22:30:00+08:00
updated_by: skill
project: <name>
---
```

`created_at` / `updated_at` are ISO 8601 with local offset — get the value from `date +%Y-%m-%dT%H:%M:%S%z | sed 's/\(..\)$/:\1/'` (or the equivalent helper your shell exposes). Use `updated_by: skill` since save-memory is the actor. When you EXTEND an existing note (smallest edit on user correction), bump `updated_at` to now and set `updated_by: skill`; leave `created_at` alone.

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

For `decision` notes, lead with the choice, then alternatives considered, then the reason, then conditions under which we'd revisit.

For `learning` notes, lead with the gotcha, then the cause, then how to apply (what to do or avoid). The note should make a future session say "ah, I would have hit that — glad we wrote it down."

For `findings` notes, lead with the question investigated, then sources consulted (URLs / paths only — addresses, not bodies), then the synthesis / takeaways, then open questions. The whole point is that a future session can re-read this instead of redoing the research.

For `reference` notes, lead with the fact (URL, ID, command). Add only context that's not derivable from the fact itself.

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
