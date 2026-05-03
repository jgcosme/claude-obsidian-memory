---
description: Manage project-vault registrations. Verbs: enable | disable | remove | list. With no verb, walks the user through a guided picker.
---

Arguments: $ARGUMENTS

Operate on `~/.config/obsidian-memory/projects.json` via the binary's `projects` and `init-project` subcommands. Resolve the binary path once:

```bash
PLUGIN_RUN="${CLAUDE_PLUGIN_ROOT:-$(ls -d ~/.claude/plugins/cache/jgcosme-plugins/obsidian-memory/*/ 2>/dev/null | sort -V | tail -1 | sed 's:/$::')}/bin/run"
```

## Parsing

Split `$ARGUMENTS` on whitespace. The first token is the **verb**; subsequent tokens compose the **path** (paths may contain spaces, so re-join with spaces if quoted by the user).

Verbs: `enable`, `disable`, `remove`, `list`. Anything else is invalid; show usage and stop.

If the path is omitted, default to the current repo:

```bash
git -C "${CLAUDE_PROJECT_DIR:-$PWD}" rev-parse --show-toplevel
```

If that fails (cwd isn't a git repo), tell the user `<verb>` requires a path argument when not run from inside a git repo, and stop. Don't proceed without a resolved path.

## No-verb (guided) flow

When `$ARGUMENTS` is empty:

1. List the registry:
   ```bash
   "$PLUGIN_RUN" projects list --json
   ```
   Print as a numbered list including a `[N+1]` synthetic entry for "the current cwd" *if* it's a git repo and not already in the list. Format:
   ```
   1) [on]  claude-obsidian-memory  /Users/.../claude-obsidian-memory  ← current
   2) [off] some-other              /tmp/some-other
   3) (current cwd, not registered) /Users/.../another-repo
   ```
2. Ask the user: "Pick a number, or `q` to quit:"
3. After they pick, ask: "What would you like to do? `enable`, `disable`, or `remove`?"
4. Then run the same logic as the verb-form below for the chosen path + action.

Skip the synthetic entry if cwd isn't a git repo or is already in the list.

## `enable [<path>]`

1. Resolve project name:
   - If an entry exists, reuse its `project` field by parsing the JSON output of `"$PLUGIN_RUN" projects lookup <path> --json` (the `"project"` field).
   - Else use the repo basename: `basename "$PATH"`.
2. Run register:
   ```bash
   "$PLUGIN_RUN" projects register "<path>" --enabled --project "<name>"
   ```
3. Run init to backfill frontmatter on any existing markdown without it:
   ```bash
   "$PLUGIN_RUN" init-project "<path>" --project "<name>"
   ```
4. Report what changed:
   - `Enabled '<name>' at <path>.`
   - If init's stdout shows files added: `Init added frontmatter to N file(s).`
   - If init's stdout shows none: `Init had no candidates — all .md files already had frontmatter.`

## `disable [<path>]`

1. Look up to get project name (or fall back to basename if not registered):
   ```bash
   "$PLUGIN_RUN" projects lookup "<path>" --json
   ```
2. Run register with `--no-enabled`:
   ```bash
   "$PLUGIN_RUN" projects register "<path>" --no-enabled --project "<name>"
   ```
3. Report: `Disabled '<name>'. SessionStart will skip it silently. Use /obsidian-memory:project enable to re-enable.`

## `remove [<path>]`

1. Run remove:
   ```bash
   "$PLUGIN_RUN" projects remove "<path>"
   ```
2. Exit code:
   - 0: removed → `Removed '<path>'. Next SessionStart in this repo will offer the registration prompt again.`
   - 1: nothing to remove → `'<path>' was not registered — nothing to remove.`

Note: `remove` does NOT delete any frontmatter that init previously added to files in the repo. Those edits live in the project's git history; clean them up there if you want them gone.

## `list`

```bash
"$PLUGIN_RUN" projects list
```

Pass through. If empty, say `(no repos registered yet)` and tell the user that opening a Claude session in any project repo will offer to register it.

## Limits

- This command never auto-commits anything. Repo files (frontmatter added by `enable`) leave the working tree dirty for the user to review.
- Always confirm with the user before `remove` if there are notes save-memory has written into the repo's matching folders (e.g., `decisions/`, `learnings/`) — `remove` won't delete those, but the user may want to know what's there.
