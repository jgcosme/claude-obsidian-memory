---
type: tool
description: Obsidian CLI commands, vault path, frontmatter search syntax
created: __TODAY__
---

# Obsidian

- CLI: `/Applications/Obsidian.app/Contents/MacOS/obsidian` (macOS); on Linux, whatever's on `$PATH` as `obsidian`
- **Requires Obsidian to be running** and CLI registered (Settings → General → Command line interface → Register CLI)
- Vault: `__VAULT_PATH__`
- Syntax: `obsidian <command> [param=value] [--flag]`
- Commands:
  - `obsidian files [vault="..."]` — list notes
  - `obsidian read path="Notes/foo.md"` — read a note
  - `obsidian create path="Notes/foo.md" content="..."` — create a note
  - `obsidian append path="Notes/foo.md" content="..."` — append to a note
  - `obsidian move path="old.md" newPath="new.md"` — move/rename (rewrites wikilinks)
  - `obsidian delete path="Notes/foo.md"` — delete a note
  - `obsidian search query="..."` — full-text search (supports frontmatter and path filters, see below)
  - `obsidian search:context query="..."` — search with surrounding context
  - `obsidian daily` / `obsidian daily:read` / `obsidian daily:append content="..."` — daily notes
  - `obsidian properties path="..."` — view frontmatter
  - `obsidian property:set path="..." key="..." value="..."` — set frontmatter field
  - `obsidian tags` / `obsidian tags:rename old="..." new="..."` — tag management
  - `obsidian links path="..."` / `obsidian backlinks path="..."` / `obsidian orphans` — link analysis
  - `obsidian plugins` / `obsidian plugin:enable id="..."` — plugin management
- Output format: append `format=json` (or `csv`, `yaml`, `paths`, `markdown`) to most commands

## Filtering by frontmatter

`obsidian search` supports Obsidian's native query syntax, including frontmatter property filters with bracket syntax `[property:value]`. This is the primary way to do typed recall across the vault.

**Syntax:**
- `[type:learning]` — notes where `type: learning` in frontmatter
- `[created:2026-04-27]` — notes with that exact `created` value
- `[type:learning] [created:2026-04-27]` — AND-combined frontmatter filters
- `path:Projects` — limit to a folder
- `path:Projects [type:learning]` — combine path + frontmatter

**Example — yesterday's learnings across all projects:**
```bash
YESTERDAY=$(date -v-1d +%Y-%m-%d)
obsidian search query="path:Projects [type:learning] [created:$YESTERDAY]" format=json
```

**Related commands:**
- `obsidian properties name=<key> counts` — count notes with a given frontmatter key
- `obsidian property:read name=<key> path=<file>` — read a single field
- `obsidian property:set name=<key> value=<v> type=date path=<file>` — set a field

**Limitations:**
- No native range syntax (e.g., `created:>=2026-04-01`). For ranges, loop dates and union results, or fall back to shell `grep` over frontmatter blocks.
- Search works only on frontmatter that's been indexed by Obsidian — Obsidian.app must be running, and a `reload` may be needed after writing many files programmatically.
