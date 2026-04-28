---
type: index
description: Root index — entry point to Claude's persistent memory
created: __TODAY__
---

# Index

Root entry point for Claude's persistent memory. Sub-indexes hold the actual table of contents for each area; this file orients the SessionStart hook.

## Always-loaded areas
- [[Tools/INDEX|Tools]] — CLIs, APIs, tools available to Claude
- [[General/INDEX|General]] — cross-cutting: identity, preferences, people, admin, references

## Projects
_(populate as projects are scaffolded under Projects/)_

## How memory is organized
- `Tools/` — tool reference; always loaded
- `General/` — cross-project knowledge (identity, preferences, people, admin, references)
- `Projects/<name>/` — per-project: overview + Decisions, Learnings, Research, References, Journal
- Frontmatter required on every note: `type`, `description`, `created` (and `project` where applicable)
- Recall via `obsidian search query="[type:learning] [created:YYYY-MM-DD]"` — see `Tools/Obsidian.md`
