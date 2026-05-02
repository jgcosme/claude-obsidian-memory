# Changelog

## v2.0.0 — Rust port

**Breaking change.** All hook scripts and helpers ported from Python + bash to a single static Rust binary, distributed via GitHub Releases.

### What changed for users

- **Prereqs simplified.** `jq` and `python3` are no longer required. First-run install needs `curl` (or `wget`) and `tar` to fetch the prebuilt binary; everything after that is the binary alone.
- **Faster.** Hook hot paths (overview, search, audit, slim-transcript) run ~6–10× faster than the v1 Python+bash implementation.
- **Single binary.** All entry points (`hook session-start`, `audit`, `setup`, `usage`, `status`, `init-project`, `projects`, etc.) live inside one `obsidian-memory` binary at `$PLUGIN_ROOT/bin/obsidian-memory`. The `bin/run` wrapper handles lazy install on first session.
- **Slash commands relocated.** `/obsidian-memory:status`, `:usage`, `:audit`, `:project` now invoke the binary; output format is unchanged.
- **Setup heredoc updated.** First-time-setup prompt now references `bin/run setup` instead of `bash setup.sh`. The new pattern survives plugin-version upgrades.

### What stays the same

- Vault layout (`Tools/`, `Notes/`, `Journals/<project>/<date>.md`).
- Frontmatter schema (`type`, `description`, `created`, `project`).
- All seven memory types and the multi-type `[a, b]` form.
- `~/.config/obsidian-memory/` config + projects.json registry. Existing files carry over.
- `vault-search` and `save-memory` skills.
- Hook output text (verified byte-equal under the v1↔v2 parity harness across 99 test cases before deletion).

### Distribution

Prebuilt binaries are published to GitHub Releases for `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, and `aarch64-unknown-linux-gnu`. Each tarball ships with a `.sha256` checksum. The release workflow (`.github/workflows/release.yml`) cross-builds on tag push.

### Upgrading from v1.x

1. `/plugin update obsidian-memory@jgcosme-plugins` (or `/plugin uninstall` followed by `/plugin install`).
2. Start a new Claude session — `bin/run` fetches the binary on first hook fire.
3. Existing vault, config, and projects.json are picked up unchanged.

If you had `OBSIDIAN_MEMORY_*` env overrides in `~/.config/obsidian-memory/config.env`, they continue to work — the parser is identical.

### Known divergences from v1

- The optional pyyaml deep-validation pass in `audit` is gone (Python-only feature, conditional on pyyaml install). Schema-level frontmatter checks remain.
- The `init-project` LLM batch call has no built-in timeout in v2 (v1 used Python's `subprocess.run(timeout=180)`). Practically unobserved; will add `wait-timeout` if a user reports a hung claude binary.
- `audit`'s `Generated:` timestamp is local time as v1; rendering matches.
