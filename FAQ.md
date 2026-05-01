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

**Q: I said "no" to the project-vault registration prompt — can I change my mind?**
A: Yes. Edit `~/.config/obsidian-memory/projects.json` and either flip `enabled` to `true`, or delete the entry to surface the registration prompt again next session. `python3 "$CLAUDE_PLUGIN_ROOT/scripts/_projects.py" register <repo-path> --enabled --project <name>` does the same thing from the CLI.

**Q: What's the difference between the personal vault and a project-vault?**
A: The personal vault (`$OBSIDIAN_VAULT_PATH`, default `~/Documents/Obsidian Vault`) is yours alone — cross-project memory like tools, preferences, and learnings that aren't tied to one repo. Each opted-in project repo can also act as a project-vault: its own `decisions/`, `learnings/`, `references/` subdirs (when they exist) become writable destinations for save-memory, and all its frontmattered `.md` files are searchable alongside the personal vault. The project-vault is committed by you (whenever you commit the repo); the personal vault auto-commits at SessionEnd.

**Q: My repo doesn't have a `decisions/` or `learnings/` folder. Will save-memory create one?**
A: No — init never creates folders or reorganizes the repo. It only adds frontmatter to existing `.md` files. If your repo has no folder matching a memory's type, save-memory routes that note to the personal vault's `Notes/` with a `project:` tag instead. Add a `decisions/` folder to your repo manually if you want decisions to live there going forward.

**Q: Why doesn't the project-vault auto-commit?**
A: Two reasons. (1) The personal vault is yours; the project-vault is your project's git history, which often has stricter conventions (commit messages, signed commits, PR review, CI). (2) save-memory writes during a session, and the natural review boundary is whenever you'd commit anyway — the working-tree change is visible in your `git status` until you decide what to do with it.

**Q: Can a project-vault note link to a personal-vault note (or vice versa)?**
A: No — wikilink resolution stays within a corpus by design. The previous attempt at cross-corpus pointers (v1.4.0) introduced a drift surface that the team kept needing to reconcile, and the feature was removed in v1.5.0. If you need to reference cross-corpus content, link to the relative path as plain markdown: `[the auth decision](../../my-app/decisions/0042-auth.md)`.
