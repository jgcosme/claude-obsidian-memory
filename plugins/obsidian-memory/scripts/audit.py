#!/usr/bin/env python3
"""Full Obsidian vault integrity audit.

Reports:
- Frontmatter issues (missing required keys: type, description, created; project under Projects/)
- Broken wikilinks (target file not found)
- Orphan notes (no incoming wikilink, excluding INDEX.md files)
- Dead INDEX entries (subset of broken wikilinks, surfaced separately)
- Duplicate basenames (multiple notes share the same filename, making bare
  [[wikilinks]] ambiguous — Obsidian picks the closest one, but it's worth
  knowing about).

Requires Python 3.9+.

Usage:
  python3 scripts/audit.py                   # print markdown report to stdout
  python3 scripts/audit.py --vault PATH      # override vault path
  python3 scripts/audit.py --json            # machine-readable output

Vault path resolution: --vault flag > $OBSIDIAN_VAULT_PATH > ~/.config/claude-memory/config.env > ~/Documents/Obsidian Vault
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from datetime import datetime
from pathlib import Path

WIKILINK_RE = re.compile(r"\[\[([^\]]+)\]\]")
# Tolerate optional UTF-8 BOM and CRLF line endings in the frontmatter delimiter.
FRONTMATTER_RE = re.compile(r"^﻿?---\s*\r?\n(.*?)\r?\n---\s*\r?\n", re.DOTALL)
SKIP_DIRS = {".git", ".obsidian", ".trash", "node_modules"}


def resolve_vault(cli_vault: str | None) -> Path:
    if cli_vault:
        return Path(os.path.expanduser(cli_vault)).resolve()
    if os.environ.get("OBSIDIAN_VAULT_PATH"):
        return Path(os.path.expanduser(os.environ["OBSIDIAN_VAULT_PATH"])).resolve()
    config = Path.home() / ".config/claude-memory/config.env"
    if config.is_file():
        for line in config.read_text().splitlines():
            line = line.strip()
            if line.startswith("OBSIDIAN_VAULT_PATH="):
                v = line.split("=", 1)[1].strip().strip('"').strip("'")
                return Path(os.path.expanduser(v)).resolve()
    return (Path.home() / "Documents/Obsidian Vault").resolve()


def parse_frontmatter(text: str) -> dict[str, str] | None:
    m = FRONTMATTER_RE.match(text)
    if not m:
        return None
    fm: dict[str, str] = {}
    for line in m.group(1).splitlines():
        if ":" in line and not line.lstrip().startswith("#"):
            k, v = line.split(":", 1)
            fm[k.strip()] = v.strip()
    return fm


def collect_md_files(vault: Path) -> list[Path]:
    out: list[Path] = []
    for root, dirs, files in os.walk(vault):
        dirs[:] = [d for d in dirs if d not in SKIP_DIRS]
        for name in files:
            if name.endswith(".md"):
                out.append(Path(root) / name)
    return sorted(out)


def extract_wikilinks(body: str) -> list[str]:
    targets: list[str] = []
    for m in WIKILINK_RE.finditer(body):
        raw = m.group(1)
        # Strip alias: [[target|alias]] -> target
        target = raw.split("|", 1)[0].strip()
        # Strip heading anchor: [[target#heading]] -> target
        target = target.split("#", 1)[0].strip()
        # Strip block ref: [[target^block]] -> target
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

    Obsidian wikilink resolution (best-effort):
      - Path-qualified targets (containing `/`):
          1. exact path from vault root
          2. path relative to source note's directory
          3. path-suffix match (any file whose relative path ends with the target)
        Returns the first match found.
      - Bare targets: every file in the vault sharing that basename. We can't
        cheaply replicate Obsidian's "closest unambiguous match" rule, so for
        orphan-detection we mark all candidates as referenced.
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


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--vault", help="path to Obsidian vault (overrides config)")
    ap.add_argument("--json", action="store_true", help="emit JSON instead of markdown")
    args = ap.parse_args()

    vault = resolve_vault(args.vault)
    if not vault.is_dir():
        print(f"vault not found at: {vault}", file=sys.stderr)
        return 1

    md_files = collect_md_files(vault)

    basename_map: dict[str, list[Path]] = {}
    for f in md_files:
        basename_map.setdefault(f.stem, []).append(f)
    all_relpaths: list[Path] = [f.relative_to(vault) for f in md_files]

    fm_issues: list[dict] = []
    broken_links: list[dict] = []
    dead_index_entries: list[dict] = []
    referenced: set[Path] = set()

    for f in md_files:
        rel = f.relative_to(vault)
        try:
            text = f.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            fm_issues.append({"file": str(rel), "issue": "not utf-8 readable"})
            continue

        fm = parse_frontmatter(text)
        if fm is None:
            fm_issues.append({"file": str(rel), "issue": "no frontmatter block"})
        else:
            required = ["type", "description", "created"]
            if str(rel).startswith("Projects/"):
                required.append("project")
            for k in required:
                if k not in fm:
                    fm_issues.append({"file": str(rel), "issue": f"missing `{k}`"})

        body = FRONTMATTER_RE.sub("", text, count=1)
        for target in extract_wikilinks(body):
            resolved = resolve_wikilink(target, vault, basename_map, f, all_relpaths)
            if not resolved:
                entry = {"file": str(rel), "link": target}
                broken_links.append(entry)
                if f.name == "INDEX.md":
                    dead_index_entries.append(entry)
            else:
                for r in resolved:
                    referenced.add(vault / r)

    orphans: list[str] = []
    for f in md_files:
        if f.name == "INDEX.md":
            continue
        if f not in referenced:
            orphans.append(str(f.relative_to(vault)))

    # Duplicate basenames — bare [[wikilinks]] become ambiguous
    duplicate_basenames: list[dict] = []
    for stem, paths in basename_map.items():
        if len(paths) > 1:
            duplicate_basenames.append({
                "basename": f"{stem}.md",
                "paths": [str(p.relative_to(vault)) for p in paths],
            })

    if args.json:
        print(json.dumps({
            "vault": str(vault),
            "generated": datetime.now().isoformat(timespec="seconds"),
            "counts": {
                "files_scanned": len(md_files),
                "frontmatter_issues": len(fm_issues),
                "broken_wikilinks": len(broken_links),
                "dead_index_entries": len(dead_index_entries),
                "orphan_notes": len(orphans),
                "duplicate_basenames": len(duplicate_basenames),
            },
            "frontmatter_issues": fm_issues,
            "broken_wikilinks": broken_links,
            "dead_index_entries": dead_index_entries,
            "orphan_notes": orphans,
            "duplicate_basenames": duplicate_basenames,
        }, indent=2))
        return 0

    print("# Vault Audit Report\n")
    print(f"Vault: `{vault}`")
    print(f"Generated: {datetime.now().isoformat(timespec='seconds')}")
    print(f"Files scanned: {len(md_files)}\n")

    print("## Frontmatter issues\n")
    if fm_issues:
        for it in fm_issues:
            print(f"- `{it['file']}` — {it['issue']}")
    else:
        print("_(none)_")
    print()

    print("## Broken wikilinks\n")
    if broken_links:
        for it in broken_links:
            print(f"- `{it['file']}` → `[[{it['link']}]]`")
    else:
        print("_(none)_")
    print()

    print("## Dead INDEX entries\n")
    if dead_index_entries:
        for it in dead_index_entries:
            print(f"- `{it['file']}` → `[[{it['link']}]]`")
    else:
        print("_(none)_")
    print()

    print("## Orphan notes (no incoming wikilink, excluding INDEX.md)\n")
    if orphans:
        for p in orphans:
            print(f"- `{p}`")
    else:
        print("_(none)_")
    print()

    print("## Duplicate basenames (bare wikilinks become ambiguous)\n")
    if duplicate_basenames:
        for d in duplicate_basenames:
            print(f"- `{d['basename']}` shared by:")
            for p in d["paths"]:
                print(f"  - `{p}`")
    else:
        print("_(none)_")
    print()

    print("## Summary")
    print(f"- Files scanned: {len(md_files)}")
    print(f"- Frontmatter issues: {len(fm_issues)}")
    print(f"- Broken wikilinks: {len(broken_links)}")
    print(f"- Dead INDEX entries: {len(dead_index_entries)}")
    print(f"- Orphan notes: {len(orphans)}")
    print(f"- Duplicate basenames: {len(duplicate_basenames)}")

    has_issues = bool(fm_issues or broken_links or orphans or duplicate_basenames)
    return 1 if has_issues else 0


if __name__ == "__main__":
    sys.exit(main())
