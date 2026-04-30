---
type: reference
description: "FAQ about plugin behavior - session scoping, vault sharing, and related questions"
created: 2026-04-30
project: claude-obsidian-memory
---

# FAQ

**Q: What if I switch projects mid-session by `cd`-ing somewhere else?**
A: The project name is captured at `SessionStart` (from `$CLAUDE_PROJECT_DIR` or `$PWD`). Mid-session `cd` doesn't change which project's overview is loaded. The journal at `SessionEnd` still writes to the original project's folder. Start a new session if you want to switch contexts.

**Q: Can I share my vault with a teammate?**
A: Yes — push to a private remote and have them clone it. The `SessionEnd` review will commit their changes too. Be careful with `General/user.md`, which is meant to be your personal profile; either gitignore it or accept that it's shared.

**Q: Does this work on Linux?**
A: Yes, with the same prerequisites. macOS-specific bits: the `SessionStart` hook tries `open -a Obsidian` (no-op on Linux) and falls back to `obsidian` on `$PATH`. `flock` is preinstalled on Linux but not on macOS.

**Q: What if I don't want Obsidian.app at all?**
A: The plugin works without it. The vault is just markdown files. You'll lose the `obsidian search` typed-query syntax (Claude will fall back to `grep`/`Read`), and the `SessionStart` "open Obsidian" step is a no-op. Everything else works.

**Q: How do I reset the gate's per-session dedup memory?**
A: Delete `/tmp/claude-memory-gate-state/<session_id>.injected`. Or `rm -rf /tmp/claude-memory-gate-state` to reset all sessions. The directory is recreated automatically.

**Q: Where do logs go?**
A: `/tmp/claude-memory-review.log` and `/tmp/claude-memory-gate.log` by default. Both rotate to `.log.1` once they exceed 1 MB. Override locations via `MEMORY_REVIEW_LOG` / `MEMORY_GATE_LOG`.

**Q: Can I run multiple Claude Code sessions in different cwds at the same time?**
A: Yes. Each session has its own `SessionStart`/`SessionEnd`; the gate runs independently per session (with its own dedup state via `session_id`). Auto-commit is wrapped in `flock` to prevent concurrent commits from racing — though without `flock` installed, two simultaneous `SessionEnd` subprocesses *could* race on `git add -A`.

**Q: How do I rebuild the vault if it gets corrupted?**
A: Re-run `bash "$CLAUDE_PLUGIN_ROOT/scripts/setup.sh"` — it's idempotent. It won't overwrite existing files but will recreate any missing scaffolding from templates. If your vault is also a git repo, `git reset --hard` to a known-good commit is faster.
