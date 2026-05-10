---
description: Manage the memory-type vocabulary at ~/.config/obsidian-memory/types.yaml. Verbs: list | add | edit | remove | reset. With no verb, walks the user through a guided picker.
---

Arguments: $ARGUMENTS

Operate on the user types file (`~/.config/obsidian-memory/types.yaml`) via the binary's `types` subcommand. Resolve the binary path once:

```bash
PLUGIN_RUN="${CLAUDE_PLUGIN_ROOT:-$(ls -d ~/.claude/plugins/cache/jgcosme-plugins/obsidian-memory/*/ 2>/dev/null | sort -V | tail -1 | sed 's:/$::')}/bin/run"
```

## Background for the agent

The type vocabulary controls four things downstream of every save: audit validation, overview ordering, personal-vault folder routing (e.g. `tool` → `Tools/`), and project-vault folder probing (e.g. `decision` → `<repo>/decisions/`). The seven built-ins (`preference`, `reference`, `findings`, `decision`, `learning`, `tool`, `journal`) ship as the embedded default. The user file at `~/.config/obsidian-memory/types.yaml` fully overrides this default — it's seeded from the embedded copy on first edit.

Removing or renaming a type that existing notes use will cause audit to flag those notes. The CLI refuses such removals without `--force`. Always surface the impact before removing.

## Parsing $ARGUMENTS

Split on whitespace. The first token is the **verb**: `list`, `add`, `edit`, `remove`, `reset`. Anything else is invalid; show usage and stop.

## No-verb (guided) flow

When `$ARGUMENTS` is empty:

1. Show the current set:
   ```bash
   "$PLUGIN_RUN" types list
   ```
2. Ask: "What would you like to do? `add`, `edit`, `remove`, `reset`, or `q` to quit."
3. Branch into the matching flow below.

## `add` flow

Ask the user, in this order — each answer may be revised before commit:

1. **Type name.** One word, lowercase, no spaces. Reject names that already exist (the CLI will too, but pre-empt for better UX).
2. **One-line description.** What kind of knowledge does this type capture?
3. **Personal-vault folder.** Suggest `Notes` as the default unless the description implies otherwise (e.g. tool-like types might fit `Tools`). Confirm with the user.
4. **Project-vault folder names** (optional, comma-separated). These are folders the plugin will probe under `<repo>/` and `<repo>/docs/` when routing notes of this type in a registered project-vault. Empty list = personal-vault only. Don't prompt for `--system-managed`; it's reserved for the plugin.

Show a preview, ask "save?" (`y`/`n`), then run:

```bash
"$PLUGIN_RUN" types add \
  --name "<name>" \
  --description "<description>" \
  --personal-folder "<folder>" \
  --project-folders "<a,b,c>"   # omit the flag if empty
```

Print the success line. Mention the change takes effect on the next session (overview cache rebuilds at SessionStart).

## `edit` flow

1. List current types, ask which to edit.
2. Show that type's current values.
3. Ask which field(s) to change (`description`, `personal_folder`, `project_folders`). Skip system_managed.
4. Collect the new value(s).
5. Run:
   ```bash
   "$PLUGIN_RUN" types edit --name "<name>" [--description "..."] [--personal-folder "..."] [--project-folders "a,b"]
   ```
   Pass only the flags the user actually changed.

## `remove` flow

**Critical:** before removing, count how many existing vault notes use the type.

1. Ask which type to remove.
2. Refuse outright if it's `journal` or any other `[system-managed]` type unless the user explicitly says they want to break SessionEnd.
3. Run a search to count affected notes:
   ```bash
   "$PLUGIN_RUN" vault search --type "<name>" --json | jq 'length'
   ```
4. If count > 0, show the user:
   ```
   N existing note(s) currently typed `<name>`. After removal, audit will flag them as `unknown type`.
   Options:
     a) Re-type those notes first (recommend), then remove.
     b) Remove anyway — audit warnings will appear until you fix them.
     c) Cancel.
   ```
5. Run with appropriate flag:
   ```bash
   "$PLUGIN_RUN" types remove --name "<name>" [--force]
   ```
   Use `--force` only if the user picked option (b) or it's a system-managed override.

## `reset` flow

Destructive — discards every customization in the user file.

1. Show the current user-defined-or-edited types via `types list`.
2. Confirm explicitly: "This will replace your `types.yaml` with the embedded default (the seven built-ins). Type `RESET` to confirm."
3. Only on exact confirmation:
   ```bash
   "$PLUGIN_RUN" types reset --yes
   ```

## `list`

```bash
"$PLUGIN_RUN" types list
```

Pass through. The first line is `source: user` (file present and read) or `source: embedded` (file absent, defaults shown).

## Limits

- Never call `types add/remove/edit/reset` without confirming the user's intent first — these mutate user config.
- The personal-vault folder is auto-scaffolded on first write to that folder. You don't need to `mkdir` it yourself.
- Don't auto-commit anything. The user's vault git state is theirs to manage.
