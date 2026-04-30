#!/usr/bin/env python3
"""Repo-vault corpus enumeration for the obsidian-memory plugin.

Identifies the .md files in a project repo that participate in the
repo-vault corpus. Shared by:
  - init_repo_vault.py — decides which files need frontmatter backfill
  - _vault.py runtime — walks the corpus for overview/search
  - audit.py — flags missing frontmatter, drift, etc.

Algorithm:
  1. git ls-files (tracked) + git ls-files --others --exclude-standard
     (untracked but not gitignored) — same view as `git status` would show
  2. Filter to .md
  3. Drop boilerplate names (LICENSE, CHANGELOG, CODE_OF_CONDUCT, SECURITY)
  4. Drop top-level dotfile dirs (.github, .vscode, .cursor, .idea, ...)

CLI:
  python3 _repo_docs.py enumerate <repo_path>           # newline-separated
  python3 _repo_docs.py enumerate <repo_path> --json    # JSON array

Requires Python 3.9+.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

# Repo-meta files that never carry memory. Case-insensitive prefix match
# on basename. CONTRIBUTING and README are intentionally NOT here — they
# often carry repo-specific memory worth surfacing.
BOILERPLATE_PREFIXES = (
    "LICENSE",
    "CHANGELOG",
    "CODE_OF_CONDUCT",
    "SECURITY",
)

# Top-level dotfile dirs to skip. Most are gitignored already; this is
# defensive for repos that track their own .vscode/, .github/ templates,
# etc. Match is on the first path component.
SKIP_DOTFILE_DIRS = (
    ".github",
    ".cursor",
    ".vscode",
    ".devcontainer",
    ".idea",
    ".claude",
)


def _git_ls(repo: Path, args: list[str]) -> list[str]:
    """Run `git ls-files [args]` in repo, return non-empty lines."""
    try:
        result = subprocess.run(
            ["git", "ls-files", *args],
            cwd=repo,
            capture_output=True,
            text=True,
            check=True,
        )
    except (subprocess.CalledProcessError, FileNotFoundError):
        return []
    return [line for line in result.stdout.splitlines() if line]


def _is_boilerplate(rel_path: Path) -> bool:
    """True if this relative path is repo metadata, not memory."""
    parts = rel_path.parts
    if parts and parts[0] in SKIP_DOTFILE_DIRS:
        return True
    name_upper = rel_path.name.upper()
    for prefix in BOILERPLATE_PREFIXES:
        if name_upper.startswith(prefix):
            return True
    return False


def enumerate_repo_docs(repo_path: Path | str) -> list[Path]:
    """Return absolute paths of .md files in the repo's vault corpus.

    Sources from git (tracked + untracked-not-gitignored). Filters out
    boilerplate (LICENSE, CHANGELOG, etc.) and top-level dotfile dirs.
    Returns [] if repo_path isn't a git repo or git isn't available.

    Enumeration is intentionally broad — runtime corpus filtering (only
    files with plugin frontmatter) happens elsewhere.
    """
    repo = Path(repo_path).expanduser().resolve()
    if not repo.is_dir() or not (repo / ".git").exists():
        return []

    tracked = _git_ls(repo, [])
    untracked = _git_ls(repo, ["--others", "--exclude-standard"])
    candidates = sorted(set(tracked) | set(untracked))

    out: list[Path] = []
    for rel in candidates:
        rel_path = Path(rel)
        if rel_path.suffix.lower() != ".md":
            continue
        if _is_boilerplate(rel_path):
            continue
        out.append(repo / rel_path)
    return out


def _cmd_enumerate(args: argparse.Namespace) -> int:
    repo = Path(args.repo_path).expanduser().resolve()
    if not (repo / ".git").exists():
        print(f"not a git repo: {repo}", file=sys.stderr)
        return 1
    paths = enumerate_repo_docs(repo)
    if args.json:
        print(json.dumps([str(p.relative_to(repo)) for p in paths], indent=2))
    else:
        for p in paths:
            print(p.relative_to(repo))
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    sub = ap.add_subparsers(dest="cmd", required=True)

    ep = sub.add_parser("enumerate", help="list .md files in the repo's vault corpus")
    ep.add_argument("repo_path", help="path to the project repo")
    ep.add_argument("--json", action="store_true", help="emit JSON array")
    ep.set_defaults(func=_cmd_enumerate)

    args = ap.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
