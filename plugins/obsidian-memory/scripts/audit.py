#!/usr/bin/env python3
"""Full Obsidian vault integrity audit.

Reports:
- Frontmatter issues (missing required keys: type, description, created; project under Projects/)
- Broken wikilinks (target file not found)
- Orphan notes (no incoming wikilink, excluding README.md files)
- Duplicate basenames (multiple notes share the same filename, making bare
  [[wikilinks]] ambiguous — Obsidian picks the closest one, but it's worth
  knowing about).

Requires Python 3.9+.

Usage:
  python3 scripts/audit.py                   # print markdown report to stdout
  python3 scripts/audit.py --vault PATH      # override vault path
  python3 scripts/audit.py --json            # machine-readable output

Vault path resolution: --vault flag > $OBSIDIAN_VAULT_PATH > ~/.config/obsidian-memory/config.env > ~/Documents/Obsidian Vault
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import datetime
from pathlib import Path

try:
    import yaml  # type: ignore
    _HAS_YAML = True
except ImportError:
    _HAS_YAML = False

# Shared modules
sys.path.insert(0, str(Path(__file__).resolve().parent))
from _vault import (  # noqa: E402
    FRONTMATTER_RE,
    collect_md_files,
    parse_frontmatter,
    resolve_vault,
)
from _repo_docs import enumerate_repo_docs  # noqa: E402

WIKILINK_RE = re.compile(r"\[\[([^\]]+)\]\]")

# Files Obsidian / docs / git tooling expect at the vault or folder root —
# they're not "memory notes" and shouldn't be flagged as orphans.
NAVIGATION_NAMES = {"README.md"}


def extract_wikilinks(body: str) -> list[str]:
    targets: list[str] = []
    for m in WIKILINK_RE.finditer(body):
        raw = m.group(1)
        target = raw.split("|", 1)[0].strip()
        target = target.split("#", 1)[0].strip()
        target = target.split("^", 1)[0].strip()
        if target:
            targets.append(target)
    return targets


def resolve_wikilink(
    target: str,
    vault: Path,
    basename_map: dict[str, list[Path]],
    source: Path,
    all_relpaths: list[Path],
) -> list[Path]:
    """Return resolved relative paths (empty list = broken).

    Path-qualified: vault root, then source-relative, then path-suffix match.
    Bare: every file in the vault sharing that basename (we mark all candidates
    as referenced for orphan-detection since we can't replicate Obsidian's
    closest-match rule cheaply).
    """
    needle = target if target.endswith(".md") else f"{target}.md"

    if "/" in target:
        cand = vault / needle
        if cand.is_file():
            return [cand.relative_to(vault)]
        cand = (source.parent / needle).resolve()
        try:
            rel = cand.relative_to(vault)
            if cand.is_file():
                return [rel]
        except ValueError:
            pass
        suffix = "/" + needle.lstrip("/")
        for p in all_relpaths:
            if ("/" + str(p)).endswith(suffix):
                return [p]
        return []

    stem = target[:-3] if target.endswith(".md") else target
    paths = basename_map.get(stem)
    if paths:
        return [p.relative_to(vault) for p in paths]
    return []


def _audit_corpus(label: str, root: Path, files: list[Path], *, project_required: bool) -> dict:
    """Run integrity checks against a single corpus.

    project_required: when True, every note must have `project:` (used for
    repo-vault corpora where save-memory always sets it). When False, only
    notes under `Projects/` need it (the personal-vault legacy convention).
    """
    basename_map: dict[str, list[Path]] = {}
    for f in files:
        basename_map.setdefault(f.stem, []).append(f)
    all_relpaths: list[Path] = [f.relative_to(root) for f in files]

    fm_issues: list[dict] = []
    broken_links: list[dict] = []
    referenced: set[Path] = set()

    for f in files:
        rel = f.relative_to(root)
        try:
            text = f.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            fm_issues.append({"file": str(rel), "issue": "not utf-8 readable"})
            continue

        fm = parse_frontmatter(text)
        if f.name in NAVIGATION_NAMES:
            pass
        elif fm is None:
            fm_issues.append({"file": str(rel), "issue": "no frontmatter block"})
        else:
            required = ["type", "description", "created"]
            if project_required or str(rel).startswith("Projects/"):
                required.append("project")
            for k in required:
                if k not in fm:
                    fm_issues.append({"file": str(rel), "issue": f"missing `{k}`"})
            if _HAS_YAML:
                m = FRONTMATTER_RE.match(text)
                if m is not None:
                    try:
                        yaml.safe_load(m.group(1))
                    except yaml.YAMLError as e:
                        msg = str(e).splitlines()[0]
                        fm_issues.append({"file": str(rel), "issue": f"yaml parse error: {msg}"})

        body = FRONTMATTER_RE.sub("", text, count=1)
        for target in extract_wikilinks(body):
            resolved = resolve_wikilink(target, root, basename_map, f, all_relpaths)
            if not resolved:
                broken_links.append({"file": str(rel), "link": target})
            else:
                for r in resolved:
                    referenced.add(root / r)

    orphans: list[str] = []
    for f in files:
        if f.name in NAVIGATION_NAMES:
            continue
        if f not in referenced:
            orphans.append(str(f.relative_to(root)))

    duplicate_basenames: list[dict] = []
    for stem, paths in basename_map.items():
        if len(paths) > 1:
            duplicate_basenames.append({
                "basename": f"{stem}.md",
                "paths": [str(p.relative_to(root)) for p in paths],
            })

    return {
        "label": label,
        "root": str(root),
        "files_scanned": len(files),
        "frontmatter_issues": fm_issues,
        "broken_wikilinks": broken_links,
        "orphan_notes": orphans,
        "duplicate_basenames": duplicate_basenames,
    }


def _print_corpus(report: dict) -> None:
    label = report["label"]
    suffix = f" ({label})" if label else ""
    print(f"## Frontmatter issues{suffix}\n")
    if report["frontmatter_issues"]:
        for it in report["frontmatter_issues"]:
            print(f"- `{it['file']}` — {it['issue']}")
    else:
        print("_(none)_")
    print()

    print(f"## Broken wikilinks{suffix}\n")
    if report["broken_wikilinks"]:
        for it in report["broken_wikilinks"]:
            print(f"- `{it['file']}` → `[[{it['link']}]]`")
    else:
        print("_(none)_")
    print()

    print(f"## Orphan notes{suffix} (no incoming wikilink, excluding README.md)\n")
    if report["orphan_notes"]:
        for p in report["orphan_notes"]:
            print(f"- `{p}`")
    else:
        print("_(none)_")
    print()

    print(f"## Duplicate basenames{suffix} (bare wikilinks become ambiguous)\n")
    if report["duplicate_basenames"]:
        for d in report["duplicate_basenames"]:
            print(f"- `{d['basename']}` shared by:")
            for p in d["paths"]:
                print(f"  - `{p}`")
    else:
        print("_(none)_")
    print()


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--vault", help="path to Obsidian vault (overrides config)")
    ap.add_argument("--repo-vault", help="also audit this repo-vault corpus (path to project repo)")
    ap.add_argument("--json", action="store_true", help="emit JSON instead of markdown")
    args = ap.parse_args()

    vault = resolve_vault(args.vault)
    if not vault.is_dir():
        print(f"vault not found at: {vault}", file=sys.stderr)
        return 1

    reports: list[dict] = []
    reports.append(_audit_corpus(
        label="personal" if args.repo_vault else "",
        root=vault,
        files=collect_md_files(vault),
        project_required=False,
    ))

    if args.repo_vault:
        repo_root = Path(args.repo_vault).expanduser().resolve()
        if not repo_root.is_dir():
            print(f"repo-vault not found at: {repo_root}", file=sys.stderr)
            return 1
        reports.append(_audit_corpus(
            label=f"repo:{repo_root.name}",
            root=repo_root,
            files=enumerate_repo_docs(repo_root),
            project_required=True,
        ))

    if args.json:
        print(json.dumps({
            "generated": datetime.now().isoformat(timespec="seconds"),
            "corpora": [
                {
                    "label": r["label"],
                    "root": r["root"],
                    "counts": {
                        "files_scanned": r["files_scanned"],
                        "frontmatter_issues": len(r["frontmatter_issues"]),
                        "broken_wikilinks": len(r["broken_wikilinks"]),
                        "orphan_notes": len(r["orphan_notes"]),
                        "duplicate_basenames": len(r["duplicate_basenames"]),
                    },
                    "frontmatter_issues": r["frontmatter_issues"],
                    "broken_wikilinks": r["broken_wikilinks"],
                    "orphan_notes": r["orphan_notes"],
                    "duplicate_basenames": r["duplicate_basenames"],
                }
                for r in reports
            ],
        }, indent=2))
        return 0

    print("# Vault Audit Report\n")
    for r in reports:
        suffix = f" ({r['label']})" if r["label"] else ""
        print(f"Corpus{suffix}: `{r['root']}`")
        print(f"Files scanned: {r['files_scanned']}")
    print(f"Generated: {datetime.now().isoformat(timespec='seconds')}\n")

    for r in reports:
        _print_corpus(r)

    print("## Summary")
    for r in reports:
        suffix = f" ({r['label']})" if r["label"] else ""
        print(f"- Corpus{suffix}: {r['files_scanned']} files, "
              f"{len(r['frontmatter_issues'])} fm issues, "
              f"{len(r['broken_wikilinks'])} broken links, "
              f"{len(r['orphan_notes'])} orphans, "
              f"{len(r['duplicate_basenames'])} dup basenames")

    has_issues = any(
        r["frontmatter_issues"] or r["broken_wikilinks"]
        or r["orphan_notes"] or r["duplicate_basenames"]
        for r in reports
    )
    return 1 if has_issues else 0


if __name__ == "__main__":
    sys.exit(main())
