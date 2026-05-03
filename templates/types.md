---
type: reference
description: "Canonical definitions for the seven memory types used in vault note frontmatter."
created_at: 2026-05-02T00:00:00+08:00
updated_at: 2026-05-03T00:00:00+08:00
updated_by: init
project: claude-obsidian-memory
---

# Memory types

Canonical definitions for the `type:` field in vault note frontmatter.
This file is the single source of truth — `save-memory`, the SessionEnd
review, and `init_project_vault.py` all load it. If you edit one type,
edit it here and only here.

The discriminator is **how the knowledge was produced** — that's what
makes filtering by type useful later.

## The seven types

- **`reference`** — atomic factual lookup. URLs, IDs, configs, channel
  names, dashboards, endpoints. One thing you'd want to copy/paste later.
  Re-read for: *"what was the URL / ID / value of X?"*

- **`findings`** — synthesis from reading multiple sources. Landscape
  maps, comparisons, surveys, "I read 4 docs and here's the takeaway."
  Body shape: question investigated → sources consulted (URLs / paths
  only, not bodies) → synthesis / takeaways → open questions.
  Re-read for: *"remember the territory we already mapped — don't redo
  this investigation."*

- **`learning`** — easy-to-miss gotchas and fixes. Things that bit us
  before and would bite us again if forgotten. Foot-gun database.
  Body shape: the gotcha → cause → how to apply (what to do / avoid).
  Re-read for: *"don't shoot ourselves in the foot the same way twice."*

- **`decision`** — choice rationale. We compared options and picked one;
  here's why, and what we ruled out. Body shape: the choice → alternatives
  considered → reason → conditions under which we'd revisit.
  Re-read for: *"why did we pick Y? would we still pick it?"*

- **`preference`** — behavioral rule the agent should follow. "Always
  do X." "Stop doing Y." "From now on, prefer Z." Body shape: the rule
  → why (the user-given reason) → how to apply (when this kicks in).
  Re-read for: future sessions need to follow the rule.

- **`tool`** — how to use a CLI / API / service. Install path, auth
  variable name, scopes, key commands, gotchas. Cross-project by nature.
  Re-read for: *"how do I use the X tool?"*

- **`journal`** — session summary. **System-managed** — only SessionEnd
  writes these. `save-memory` and human-driven writes never use this
  type. Listed here for completeness.

## Multi-type notes

`type:` may be a single string or an ordered list. A note that genuinely
spans axes can declare both:

```yaml
type: [decision, learning]
# or equivalently:
type:
  - decision
  - learning
```

**Routing precedence:** the first type wins. Order the list so the
type that drives the destination folder comes first. Examples:

- `[decision, learning]` — primarily a decision; also captures the
  gotcha that prompted it. Routes to `decisions/`.
- `[findings, decision]` — research that culminated in a choice.
  Routes to `findings/` (or personal `Notes/` if no folder exists).
- `[learning, reference]` — a gotcha that includes an atomic fact
  worth bookmarking. Routes via `learning`.

**Filter behavior:** `--type X` matches if `X` appears anywhere in
the list. So `--type learning` finds both `type: learning` and
`type: [decision, learning]` notes.

## Picking the right type

Use this decision shape:

1. Did the knowledge come from **reading multiple sources**? → `findings`.
2. Did it come from **doing** (hitting a bug, watching a behavior)?
   → `learning`.
3. Did it come from **comparing options to pick one**? → `decision`.
4. Is it an **atomic fact** you'd look up? → `reference`.
5. Is it a **rule the agent should follow**? → `preference`.
6. Is it about **using a CLI / API / service**? → `tool`.

When a note genuinely covers two axes, list both. The model is
trusted to over-tag when uncertain — recall benefits from generous
tagging more than precision suffers.
