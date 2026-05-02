#!/usr/bin/env python3
"""Project-vault corpus enumeration for the obsidian-memory plugin.

Identifies the .md files in a project repo that participate in the
project-vault corpus. Shared by:
  - init_project_vault.py — decides which files need frontmatter backfill
  - _vault.py runtime      — walks the corpus for overview/search
  - audit.py               — flags missing frontmatter, drift, etc.

Algorithm:
  1. git ls-files (tracked) + git ls-files --others --exclude-standard
     (untracked but not gitignored) — same view as `git status` would show
  2. Filter to .md
  3. Drop boilerplate names (LICENSE, CHANGELOG, CODE_OF_CONDUCT, SECURITY)
  4. Drop top-level dotfile dirs (.github, .vscode, .cursor, .idea, ...)

CLI:
  python3 _project_docs.py enumerate <project_path>           # newline-separated
  python3 _project_docs.py enumerate <project_path> --json    # JSON array

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
    # Drop test fixtures wherever they live in the tree. Fixtures often carry
    # plugin-shaped frontmatter to simulate a real vault for the gate-prompt
    # eval tests, so the runtime corpus filter (which trusts frontmatter)
    # can't distinguish them — they have to be excluded by path.
    if "fixtures" in parts:
        return True
    # Drop Claude Code plugin metadata (SKILL.md inside skills/, slash-command
    # markdown inside commands/, marketplace.json's .claude-plugin/ dir).
    # These files carry skill/command frontmatter, not plugin memory
    # frontmatter, so they pollute project-vault audits with false-positive
    # "missing frontmatter" hits. Any-segment match (not just top-level) so
    # nested plugins/<name>/skills/<skill>/SKILL.md are also caught.
    if ".claude-plugin" in parts:
        return True
    if "skills" in parts and rel_path.name == "SKILL.md":
        return True
    if "commands" in parts and rel_path.suffix.lower() == ".md":
        # Heuristic: only skip when there's also a sibling .claude-plugin/
        # or plugins/ marker on the path, to avoid eating a legitimate
        # docs/commands/ directory in unrelated projects.
        if any(p in parts for p in ("plugins", ".claude-plugin", ".claude")):
            return True
    name_upper = rel_path.name.upper()
    for prefix in BOILERPLATE_PREFIXES:
        if name_upper.startswith(prefix):
            return True
    return False


# Common repo-folder names per memory type, lowercase. Order doesn't matter —
# match is on existence, not preference. Types not listed here (preference,
# tool, journal) never have a matching repo folder; their writes always
# route to the personal vault.
TYPE_FOLDER_PATTERNS: dict[str, tuple[str, ...]] = {
    "decision": ("decisions", "adr", "decision-records"),
    "findings": ("findings", "research"),
    "learning": ("learnings", "lessons"),
    "reference": ("references",),
}


def match_type_folder(project_path: Path | str, type_: str) -> Path | None:
    """Find a folder in the repo matching the given memory type.

    Looks at:
      1. Top-level dirs in the repo
      2. Dirs immediately under docs/

    Match is case-insensitive on the directory's basename. Returns the
    absolute path to the first matching folder, or None if no match.
    Used by save-memory's bucket-2 routing to decide whether a write goes
    to the project-vault (matching folder exists) or the personal vault
    Notes/ (no match).
    """
    patterns = TYPE_FOLDER_PATTERNS.get(type_, ())
    if not patterns:
        return None
    repo = Path(project_path).expanduser().resolve()
    if not repo.is_dir():
        return None
    for tier_root in (repo, repo / "docs"):
        if not tier_root.is_dir():
            continue
        try:
            entries = sorted(tier_root.iterdir())
        except OSError:
            continue
        for entry in entries:
            if entry.is_dir() and entry.name.lower() in patterns:
                return entry
    return None


def enumerate_project_docs(project_path: Path | str) -> list[Path]:
    """Return absolute paths of .md files in the repo's vault corpus.

    Sources from git (tracked + untracked-not-gitignored). Filters out
    boilerplate (LICENSE, CHANGELOG, etc.) and top-level dotfile dirs.
    Returns [] if project_path isn't a git repo or git isn't available.

    Enumeration is intentionally broad — runtime corpus filtering (only
    files with plugin frontmatter) happens elsewhere.
    """
    repo = Path(project_path).expanduser().resolve()
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
    repo = Path(args.project_path).expanduser().resolve()
    if not (repo / ".git").exists():
        print(f"not a git repo: {repo}", file=sys.stderr)
        return 1
    paths = enumerate_project_docs(repo)
    if args.json:
        print(json.dumps([str(p.relative_to(repo)) for p in paths], indent=2))
    else:
        for p in paths:
            print(p.relative_to(repo))
    return 0


def _cmd_match_type_folder(args: argparse.Namespace) -> int:
    repo = Path(args.project_path).expanduser().resolve()
    folder = match_type_folder(repo, args.type)
    if folder is None:
        if args.json:
            print(json.dumps({"matched": False, "type": args.type}))
        # Empty stdout + non-zero is a clear "no match" signal for shell consumers.
        return 1
    rel = folder.relative_to(repo)
    if args.json:
        print(json.dumps({"matched": True, "type": args.type, "path": str(rel)}))
    else:
        print(rel)
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    sub = ap.add_subparsers(dest="cmd", required=True)

    ep = sub.add_parser("enumerate", help="list .md files in the repo's vault corpus")
    ep.add_argument("project_path", help="path to the project's git repo")
    ep.add_argument("--json", action="store_true", help="emit JSON array")
    ep.set_defaults(func=_cmd_enumerate)

    mp = sub.add_parser("match-type-folder",
                        help="find a repo folder matching a memory type (decision/learning/reference)")
    mp.add_argument("project_path", help="path to the project's git repo")
    mp.add_argument("--type", required=True,
                    choices=("decision", "findings", "learning", "reference", "preference", "tool", "journal"),
                    help="memory type to match")
    mp.add_argument("--json", action="store_true", help="emit JSON result")
    mp.set_defaults(func=_cmd_match_type_folder)

    args = ap.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
