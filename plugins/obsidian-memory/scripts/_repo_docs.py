#!/usr/bin/env python3
"""Helpers for reconciling vault pointer notes against project-repo *.md docs.

Used by:
  - scripts/audit.py (full pointer-drift audit)
  - hooks/scripts/session-end.sh (diff-scoped pointer reconciliation in review)

Functions:
  resolve_project_root(path)   git toplevel-aware resolution; falls back to path
  list_repo_docs(root)         set of normalized repo-relative *.md (boilerplate filtered)
  find_changed_docs(root, base_sha=None)
                               dict with added/modified/deleted/renamed *.md since base_sha
                               (or HEAD if base_sha is None), incl. untracked + working tree
  pointer_index(vault, project)
                               normalized source-path → list of vault pointer note paths
  normalize_repo_path(s)       strip ./, normalize separators, drop trailing /

Requires Python 3.9+.
"""
from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from _vault import collect_md_files, read_note  # noqa: E402

# Boilerplate filenames are tracked but rarely "memory-worthy docs" — never
# flag them as missing-pointer candidates or as pointer sources.
BOILERPLATE_NAMES = {
    "CODE_OF_CONDUCT.md",
    "SECURITY.md",
    "CHANGELOG.md",
    "PULL_REQUEST_TEMPLATE.md",
    "ISSUE_TEMPLATE.md",
}
# Any path whose first segment starts with "." is treated as IDE/CI/tool
# config, not project documentation (catches .github/, .cursor/, .vscode/,
# .devcontainer/, .claude/, .idea/, etc. in one rule).
# Excluded entirely from the fallback walk (git ls-files already honors gitignore).
WALK_SKIP_DIRS = {
    ".git", ".obsidian", ".trash", ".archive",
    "node_modules", "vendor", "dist", "build", "_site", "site", "out",
    "__pycache__", ".venv", "venv", "env", ".tox", ".mypy_cache",
    ".pytest_cache", "target",
}


def normalize_repo_path(s: str) -> str:
    if not s:
        return ""
    s = s.replace("\\", "/").strip()
    while s.startswith("./"):
        s = s[2:]
    if s.endswith("/"):
        s = s[:-1]
    return s


def _is_boilerplate(rel: str) -> bool:
    if not rel:
        return True
    # Top-level dotfile directories: IDE/CI/tool config (.github/, .cursor/,
    # .vscode/, .claude/, .devcontainer/, ...).
    first_seg = rel.split("/", 1)[0]
    if first_seg.startswith(".") and first_seg != ".":
        return True
    name = rel.rsplit("/", 1)[-1]
    upper = name.upper()
    if upper.startswith("LICENSE") or upper.startswith("LICENCE") or upper.startswith("COPYING"):
        if upper.endswith(".MD"):
            return True
    if name in BOILERPLATE_NAMES:
        return True
    if rel.startswith("ISSUE_TEMPLATE/") or "/ISSUE_TEMPLATE/" in rel:
        return True
    return False


def resolve_project_root(path) -> Path:
    """Resolve the project root: git toplevel if available, else the path itself."""
    p = Path(path).resolve()
    try:
        out = subprocess.run(
            ["git", "-C", str(p), "rev-parse", "--show-toplevel"],
            capture_output=True, text=True, timeout=5,
        )
        if out.returncode == 0:
            top = out.stdout.strip()
            if top:
                return Path(top).resolve()
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass
    return p


def _git_ls_files(root: Path) -> tuple[bool, list[str]]:
    """Run `git ls-files` for tracked + untracked-not-ignored *.md.

    Returns (in_git, paths). Untracked files are included so freshly added
    docs (not yet committed) are still reconciled against vault pointers.
    """
    in_git = False
    paths: list[str] = []
    try:
        # Tracked
        out = subprocess.run(
            ["git", "-C", str(root), "ls-files", "-z", "--", "*.md"],
            capture_output=True, text=True, timeout=15,
        )
        if out.returncode == 0:
            in_git = True
            paths.extend(p for p in out.stdout.split("\0") if p)
        # Untracked, gitignore-respecting
        if in_git:
            out = subprocess.run(
                ["git", "-C", str(root), "ls-files", "--others",
                 "--exclude-standard", "-z", "--", "*.md"],
                capture_output=True, text=True, timeout=15,
            )
            if out.returncode == 0:
                paths.extend(p for p in out.stdout.split("\0") if p)
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass
    return in_git, paths


def list_repo_docs(root) -> set[str]:
    """Return set of normalized repo-relative *.md paths, boilerplate filtered.

    Prefers `git ls-files` (respects .gitignore). Falls back to a walk that
    skips well-known noise dirs when not in a git repo.
    """
    root = Path(root).resolve()
    in_git, paths = _git_ls_files(root)

    if not in_git:
        for r, dirs, files in os.walk(root):
            dirs[:] = [d for d in dirs if d not in WALK_SKIP_DIRS and not d.startswith(".")]
            for name in files:
                if name.endswith(".md"):
                    abs_p = Path(r) / name
                    try:
                        rel = abs_p.relative_to(root)
                        paths.append(str(rel).replace(os.sep, "/"))
                    except ValueError:
                        continue

    out: set[str] = set()
    for p in paths:
        norm = normalize_repo_path(p)
        if not norm or _is_boilerplate(norm):
            continue
        # Tracked-but-deleted files still appear in `git ls-files`; skip them
        # so they aren't reported as missing pointers.
        if not (root / norm).is_file():
            continue
        out.add(norm)
    return out


def _consume_namestatus(out: str, result: dict) -> None:
    """Parse `git diff --name-status -z` (rename-aware) into result buckets."""
    tokens = [t for t in out.split("\0") if t]
    i = 0
    while i < len(tokens):
        status = tokens[i]
        i += 1
        if status.startswith("R") or status.startswith("C"):
            if i + 1 >= len(tokens):
                break
            old = normalize_repo_path(tokens[i]); i += 1
            new = normalize_repo_path(tokens[i]); i += 1
            if _is_boilerplate(old) and _is_boilerplate(new):
                continue
            if status.startswith("R"):
                result["renamed"].append([old, new])
            else:
                result["added"].append(new)
        else:
            if i >= len(tokens):
                break
            p = normalize_repo_path(tokens[i]); i += 1
            if _is_boilerplate(p):
                continue
            if status == "A":
                result["added"].append(p)
            elif status == "M":
                result["modified"].append(p)
            elif status == "D":
                result["deleted"].append(p)


def find_changed_docs(root, base_sha: str | None = None) -> dict:
    """Identify *.md changes between base_sha (or HEAD) and current state.

    Returns {added, modified, deleted, renamed} where renamed is a list of
    [old, new] pairs. Includes uncommitted working-tree changes and untracked
    new files. Boilerplate filenames are dropped.
    """
    result: dict = {"added": [], "modified": [], "deleted": [], "renamed": []}
    root = Path(root).resolve()

    base_ref = base_sha if base_sha else "HEAD"

    # base..HEAD (committed work since session start, when base_sha given)
    if base_sha:
        try:
            out = subprocess.run(
                ["git", "-C", str(root), "diff", "--name-status", "-z", "-M",
                 base_sha, "HEAD", "--", "*.md"],
                capture_output=True, text=True, timeout=15,
            )
            if out.returncode == 0:
                _consume_namestatus(out.stdout, result)
        except (FileNotFoundError, subprocess.TimeoutExpired):
            pass

    # Working tree vs HEAD (uncommitted)
    try:
        out = subprocess.run(
            ["git", "-C", str(root), "diff", "--name-status", "-z", "-M",
             "HEAD", "--", "*.md"],
            capture_output=True, text=True, timeout=15,
        )
        if out.returncode == 0:
            _consume_namestatus(out.stdout, result)
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass

    # Untracked, gitignore-respecting
    try:
        out = subprocess.run(
            ["git", "-C", str(root), "ls-files", "--others", "--exclude-standard",
             "-z", "--", "*.md"],
            capture_output=True, text=True, timeout=15,
        )
        if out.returncode == 0:
            for p in out.stdout.split("\0"):
                if not p:
                    continue
                norm = normalize_repo_path(p)
                if norm and not _is_boilerplate(norm):
                    result["added"].append(norm)
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass

    for k in ("added", "modified", "deleted"):
        result[k] = sorted(set(result[k]))
    seen: set = set()
    deduped: list = []
    for pair in result["renamed"]:
        t = (pair[0], pair[1])
        if t not in seen:
            seen.add(t)
            deduped.append(pair)
    result["renamed"] = deduped

    return result


def pointer_index(vault, project: str) -> dict:
    """Map normalized source path → list of vault pointer note paths.

    Scans Projects/<project>/ for notes with frontmatter `source:`. Substantive
    notes (no `source:`) are skipped — they don't claim to mirror a repo doc.
    """
    vault = Path(vault).resolve()
    project_dir = vault / "Projects" / project
    if not project_dir.is_dir():
        return {}
    out: dict = {}
    for f in collect_md_files(project_dir):
        fm, _ = read_note(f)
        if not fm:
            continue
        src = fm.get("source", "").strip()
        if not src:
            continue
        norm = normalize_repo_path(src)
        if not norm:
            continue
        rel = str(f.relative_to(vault))
        out.setdefault(norm, []).append(rel)
    return out
