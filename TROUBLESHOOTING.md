# Troubleshooting

**SessionStart hook doesn't seem to load anything**
- Confirm `~/.config/claude-memory/config.env` points at a real vault.
- Check the vault has a `README.md` at the root — re-run `setup.sh` if not.
- Confirm Obsidian.app is running and the CLI is registered (Settings → General → Command line interface). Optional, only matters if you want the `obsidian` CLI.

**SessionEnd review didn't run / didn't write a journal**
- Check `/tmp/claude-memory-review.log`. If it says `no Projects/<name>/ folder; skipping review, will still commit dirty vault state`, you declined to scaffold (or were never asked) — start a new session in that directory and answer **yes** to the scaffolding prompt, or scaffold manually (see [HOW-IT-WORKS.md](./HOW-IT-WORKS.md#adding-a-new-project)). Any `General/`/`Tools/` writes from the session were still committed.
- If it says `no transcript at ''`, check that `jq` is installed.
- If you don't see any log at all, the hook may not be registered — try `/reload-plugins` and restart your session.

**Reviews are too aggressive / writing too many notes**
- The dedup check (typed search before write) usually catches duplicates. If something slips through, delete the note and `git commit`. The next review will see the deletion in history and avoid re-creating.
- For finer control, edit the prompt in `hooks/scripts/session-end.sh` (search for `PROACTIVE NOTES`).

**Retrieval gate adds too much latency**
- Set `OBSIDIAN_MEMORY_GATE_ENABLED=false` in `~/.config/claude-memory/config.env` to disable the gate entirely. SessionStart context still loads.

**I want to disable auto-commit**
- Set `OBSIDIAN_MEMORY_AUTOCOMMIT=false` in `~/.config/claude-memory/config.env`. Commit manually instead.

**Scanning the vault for leaked credentials**

```bash
grep -rE "(xoxp-|xoxb-|sk-[A-Za-z0-9]|gho_|ghp_|github_pat_|AKIA|AIza|AIzaSy)" "$HOME/Documents/Obsidian Vault"
```

To also catch credentials still in git history:

```bash
git -C "$HOME/Documents/Obsidian Vault" log --all -p | \
  grep -E "(xoxp-|xoxb-|sk-[A-Za-z0-9]|gho_|ghp_|github_pat_|AKIA|AIza|AIzaSy)" | head
```
