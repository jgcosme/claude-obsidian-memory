---
type: reference
description: "Troubleshooting guide for common obsidian-memory plugin issues with diagnosis steps"
created_at: 2026-04-30T00:00:00+08:00
created_by: init
updated_at: 2026-05-03T00:00:00+08:00
updated_by: init
project: claude-obsidian-memory
---

# Troubleshooting

**Quick diagnosis**

```text
/obsidian-memory:status
```

Reports config, vault, prereqs, binary location, search smoke-test, overview cache, and the latest review/gate log lines. Most issues below are visible from one run.

**`SessionStart` hook doesn't seem to load anything**
- Confirm `~/.config/obsidian-memory/config.env` points at a real vault.
- Check the vault has a `README.md` at the root — re-run `setup` if not.
- If you see a "first-time setup" prompt, accept it (or run `"$CLAUDE_PLUGIN_ROOT/bin/run" setup` manually). The hook intentionally exits early until the vault exists.
- Confirm Obsidian.app is running and the CLI is registered (Settings → General → Command line interface). Optional, only matters if you want the `obsidian` CLI.

**Hooks fail silently / `bin/run` says "binary install failed"**
- The bootstrap fetches the prebuilt binary from GitHub Releases. Check that `curl` (or `wget`) and `tar` are installed, and that you can reach `github.com`. Re-run any hook to retry — the bootstrap is idempotent.
- If you're behind a proxy, set `HTTP_PROXY`/`HTTPS_PROXY` so curl can reach GitHub.
- For airgapped installs, build from source: `cargo build --release` in the plugin dir, then symlink `target/release/obsidian-memory` to `bin/obsidian-memory`.

**`SessionEnd` review didn't run / didn't write a journal**
- Check `/tmp/claude-memory-review.log`. If it says `OBSIDIAN_MEMORY_REVIEW_ENABLED=false`, the review is disabled in `~/.config/obsidian-memory/config.env` (auto-commit still runs).
- If it says `vault not found at '...'`, run `"$CLAUDE_PLUGIN_ROOT/bin/run" setup` to scaffold the vault.
- If you don't see any log at all, the hook may not be registered — try `/reload-plugins` and restart your session.

**Reviews are too aggressive / writing too many notes**
- The dedup check (typed search before write) usually catches duplicates. If something slips through, delete the note and `git commit`. The next review will see the deletion in history and avoid re-creating.
- For finer control, edit the review prompt in `src/hook/session_end.rs` (`build_review_prompt`, search for `PROACTIVE NOTES`) and rebuild.

**Retrieval gate adds too much latency**
- Set `OBSIDIAN_MEMORY_GATE_ENABLED=false` in `~/.config/obsidian-memory/config.env` to disable the gate entirely. `SessionStart` context still loads.

**Gate / `SessionEnd` review fails with "Not logged in"**
- Run `/login` in your interactive Claude Code session. If `/obsidian-memory:status` still reports `gate exited 1; output: Not logged in` after a successful `/login`, your `claude` CLI may be invoked with `--bare`. That flag disables OAuth/keychain auth by design (see `claude --help`). The plugin doesn't use `--bare`; recursion-guard env vars (`CLAUDE_MEMORY_GATE=1`, `CLAUDE_MEMORY_REVIEW=1`) prevent the gate/review subprocesses from re-firing the hooks.

**I want to disable auto-commit**
- Set `OBSIDIAN_MEMORY_AUTOCOMMIT=false` in `~/.config/obsidian-memory/config.env`. Commit manually instead.

**Project-vault docs aren't appearing in the overview**
- Run `/obsidian-memory:project list` and confirm the repo shows `[on]`. If it shows `[off]`, run `/obsidian-memory:project enable` from inside the repo. If it's missing entirely, the registration prompt was never answered — start a fresh session in the repo or run `enable` directly.
- The registry keys on the repo's git toplevel (`git rev-parse --show-toplevel`). If you've moved the repo, the old path is stale; `remove` it and re-`enable` from the new location.
- Confirm the docs aren't all boilerplate (`LICENSE*`, `CHANGELOG*`, `CODE_OF_CONDUCT*`, `SECURITY*`, top-level dotfile dirs) — those are filtered out of the corpus.
- Project-vault notes need plugin frontmatter to appear in the overview. `obsidian-memory init-project` runs silently each SessionStart for enabled repos and backfills any new files; if a file is missing from the overview, check that it has `type:`/`description:`/`created_at:`/`project:` in its frontmatter.

**Scanning the vault for leaked credentials**

```bash
grep -rE "(xoxp-|xoxb-|sk-[A-Za-z0-9]|gho_|ghp_|github_pat_|AKIA|AIza|AIzaSy)" "$HOME/Documents/Obsidian Memory"
```

To also catch credentials still in git history:

```bash
git -C "$HOME/Documents/Obsidian Memory" log --all -p | \
  grep -E "(xoxp-|xoxb-|sk-[A-Za-z0-9]|gho_|ghp_|github_pat_|AKIA|AIza|AIzaSy)" | head
```
