#!/usr/bin/env python3
"""Shared vault library + CLI for the obsidian-memory plugin.

Provides:
  - parse_frontmatter()        — YAML-ish frontmatter parser tolerant of BOM/CRLF
  - collect_md_files()         — vault walker, skips .git/.obsidian/.trash/node_modules
  - resolve_vault()            — vault path resolution (CLI flag > env > config > default)
  - search()                   — frontmatter-aware search by type/path/keywords/date range
  - overview()                 — structured "what's in the vault" markdown for SessionStart

CLI subcommands:
  search   filter notes by --type / --path-prefix / --keywords / --created-after / --created-before
  overview emit a vault overview (markdown), optionally scoped to --project NAME

Requires Python 3.9+.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from datetime import date as _date
from pathlib import Path
from typing import Iterable

FRONTMATTER_RE = re.compile(r"^﻿?---\s*\r?\n(.*?)\r?\n---\s*\r?\n", re.DOTALL)
WIKILINK_RE = re.compile(r"\[\[([^\]]+)\]\]")
SKIP_DIRS = {".git", ".obsidian", ".trash", "node_modules", ".archive"}

# Sibling module: project-vault corpus enumeration via git ls-files.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from _project_docs import enumerate_project_docs  # noqa: E402


# ---------------------------------------------------------------------------
# Vault path resolution
# ---------------------------------------------------------------------------
def resolve_vault(cli_vault: str | None = None) -> Path:
    """Resolve the plugin's vault dir.

    Resolution order:
      1. --vault flag
      2. $OBSIDIAN_VAULT_PATH env var
      3. OBSIDIAN_VAULT_PATH= in ~/.config/obsidian-memory/config.env
      4. Default: ~/Documents/Obsidian Memory

    The vault is the plugin's own dir — it owns the whole tree, including
    .git, README, and the three top-level folders (Tools, Journals, Notes).
    Default lives at "Obsidian Memory" so it parallels but doesn't collide
    with a user's existing Obsidian vault at "Obsidian Vault".
    """
    if cli_vault:
        return Path(os.path.expanduser(cli_vault)).resolve()
    if os.environ.get("OBSIDIAN_VAULT_PATH"):
        return Path(os.path.expanduser(os.environ["OBSIDIAN_VAULT_PATH"])).resolve()
    config = Path.home() / ".config/obsidian-memory/config.env"
    if config.is_file():
        for line in config.read_text().splitlines():
            line = line.strip()
            if line.startswith("OBSIDIAN_VAULT_PATH="):
                v = line.split("=", 1)[1].strip().strip('"').strip("'")
                return Path(os.path.expanduser(v)).resolve()
    return (Path.home() / "Documents/Obsidian Memory").resolve()


# ---------------------------------------------------------------------------
# File walking + frontmatter
# ---------------------------------------------------------------------------
def collect_md_files(vault: Path) -> list[Path]:
    out: list[Path] = []
    for root, dirs, files in os.walk(vault):
        dirs[:] = [d for d in dirs if d not in SKIP_DIRS]
        for name in files:
            if name.endswith(".md"):
                out.append(Path(root) / name)
    return sorted(out)


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


def read_note(path: Path) -> tuple[dict[str, str] | None, str]:
    """Return (frontmatter dict or None, body without frontmatter).

    OSError covers project-vault paths returned by `git ls-files` that
    are tracked but missing from the working tree (uncommitted deletions);
    callers walking enumerated corpora must not crash on those.
    """
    try:
        text = path.read_text(encoding="utf-8")
    except (UnicodeDecodeError, OSError):
        return None, ""
    fm = parse_frontmatter(text)
    body = FRONTMATTER_RE.sub("", text, count=1) if fm is not None else text
    return fm, body


# ---------------------------------------------------------------------------
# Search
# ---------------------------------------------------------------------------
def _parse_date(s: str) -> _date | None:
    s = s.strip()
    if not s:
        return None
    try:
        return _date.fromisoformat(s)
    except ValueError:
        return None


def search(
    vault: Path,
    *,
    type_: str | None = None,
    path_prefix: str | None = None,
    keywords: str | None = None,
    created_after: str | None = None,
    created_before: str | None = None,
    limit: int = 50,
    project_vault: Path | None = None,
) -> list[dict]:
    """Frontmatter-aware vault search across one or two corpora.

    Returns a list of {corpus, path, type, description, project, created} dicts
    ranked by simple keyword frequency. All filters AND together. When
    project_vault is provided, results from that corpus are merged into the
    ranking; the `corpus` field disambiguates ("personal" or "project").
    """
    after = _parse_date(created_after) if created_after else None
    before = _parse_date(created_before) if created_before else None
    kw_terms: list[str] = []
    if keywords:
        kw_terms = [t for t in re.split(r"\s+", keywords.strip().lower()) if t]

    hits: list[tuple[int, dict]] = []

    def score_one(corpus_label: str, root: Path, files: Iterable[Path]) -> None:
        for f in files:
            rel = f.relative_to(root)
            rel_str = str(rel)
            if path_prefix:
                normalized = path_prefix.strip("/")
                if not rel_str.startswith(normalized):
                    continue
            fm, body = read_note(f)
            fm = fm or {}
            if type_ is not None and fm.get("type") != type_:
                continue
            if after or before:
                d = _parse_date(fm.get("created", ""))
                if d is None:
                    continue
                if after and d < after:
                    continue
                if before and d > before:
                    continue
            score = 0
            if kw_terms:
                haystack = (rel_str + "\n" + " ".join(fm.values()) + "\n" + body).lower()
                for t in kw_terms:
                    score += haystack.count(t)
                if score == 0:
                    continue
            else:
                score = 1
            hits.append((score, {
                "corpus": corpus_label,
                "path": str(f),
                "type": fm.get("type", ""),
                "description": fm.get("description", ""),
                "project": fm.get("project", ""),
                "created": fm.get("created", ""),
            }))

    score_one("personal", vault, collect_md_files(vault))
    if project_vault is not None:
        score_one("project", project_vault, enumerate_project_docs(project_vault))

    hits.sort(key=lambda x: (-x[0], x[1]["corpus"], x[1]["path"]))
    return [h[1] for h in hits[:limit]]


# ---------------------------------------------------------------------------
# Overview generation (for SessionStart)
# ---------------------------------------------------------------------------
def _bullet(path: Path, fm: dict[str, str]) -> str:
    desc = fm.get("description", "").strip()
    # Emit absolute paths so the gate can copy them verbatim into `read`
    # without per-vault root resolution downstream — collisions become
    # impossible and the validator collapses to `[ -f "$p" ]`.
    base = f"- {path}"
    return f"{base} — {desc}" if desc else base


_TYPE_ORDER = ("preference", "reference", "decision", "learning", "tool", "journal")
_RECENT_JOURNAL_LIMIT = 5


def overview(vault: Path, project: str | None = None, mode: str = "full") -> str:
    """Build a markdown overview of the vault.

    `Tools/` and `Notes/` are flat; `Journals/` is nested one level by
    project (`Journals/<project>/<date>.md`) so multi-project days don't
    collide on a single file. Project scoping for Notes lives in each
    note's `project:` frontmatter field, not the folder hierarchy. Notes
    with no `project:` are pan-vault "general" notes.

    Excludes README.md files (treated as navigation/prose, surfaced separately
    by the SessionStart hook).

    Modes:
      full              Tools + Notes (current-project deep + general + other-project name list) + recent Journals
      tools-and-general Tools + general Notes only; current/other project notes via search
      tools-only        Tools only; gate uses search for everything else
    """
    if mode not in {"full", "tools-and-general", "tools-only"}:
        raise ValueError(f"unknown overview mode: {mode}")
    md_files = [f for f in collect_md_files(vault) if f.name != "README.md"]
    notes: list[tuple[Path, dict[str, str]]] = []
    for f in md_files:
        fm, _ = read_note(f)
        notes.append((f, fm or {}))

    def by_prefix(prefix: str) -> list[tuple[Path, dict[str, str]]]:
        return [(f, fm) for f, fm in notes if str(f.relative_to(vault)).startswith(prefix)]

    out: list[str] = ["# Vault overview", ""]

    # Tools — flat list (always included)
    out.append("## Tools")
    tools = by_prefix("Tools/")
    if tools:
        for f, fm in tools:
            out.append(_bullet(f, fm))
    else:
        out.append("_(empty)_")
    out.append("")

    if mode == "tools-only":
        out.append("_All non-Tools vault content is searchable via the `search` field "
                   "(filter by `type`, `keywords`, dates)._")
        out.append("")
        return "\n".join(out)

    # Notes — partition by project: frontmatter
    notes_files = by_prefix("Notes/")
    current_notes: list[tuple[Path, dict[str, str]]] = []
    general_notes: list[tuple[Path, dict[str, str]]] = []
    other_notes_by_project: dict[str, list[tuple[Path, dict[str, str]]]] = {}
    for f, fm in notes_files:
        proj = fm.get("project", "").strip()
        if not proj:
            general_notes.append((f, fm))
        elif proj == project:
            current_notes.append((f, fm))
        else:
            other_notes_by_project.setdefault(proj, []).append((f, fm))

    def emit_by_type(items: list[tuple[Path, dict[str, str]]]) -> None:
        by_type: dict[str, list[tuple[Path, dict[str, str]]]] = {}
        for f, fm in items:
            by_type.setdefault(fm.get("type", "untyped"), []).append((f, fm))
        ordered_types = [t for t in _TYPE_ORDER if t in by_type]
        ordered_types += sorted(t for t in by_type if t not in _TYPE_ORDER)
        for type_ in ordered_types:
            out.append(f"#### {type_}")
            for f, fm in by_type[type_]:
                out.append(_bullet(f, fm))

    if mode == "tools-and-general":
        out.append("## Notes (general)")
        if general_notes:
            emit_by_type(general_notes)
        else:
            out.append("_(empty)_")
        out.append("")
        if current_notes or other_notes_by_project:
            out.append("_Project-scoped notes available via `search` "
                       "(filter by `keywords` matching the project name, plus `type`)._")
            out.append("")
        return "\n".join(out)

    # mode == "full"
    out.append("## Notes")

    if project:
        out.append(f"### Current project: {project}")
        if current_notes:
            emit_by_type(current_notes)
        else:
            out.append("_(no notes yet for this project)_")
        out.append("")

    out.append("### General (cross-project)")
    if general_notes:
        emit_by_type(general_notes)
    else:
        out.append("_(empty)_")
    out.append("")

    if other_notes_by_project:
        out.append("### Other projects")
        out.append("(use `search` with `keywords: <project-name>` to query)")
        for proj in sorted(other_notes_by_project.keys()):
            count = len(other_notes_by_project[proj])
            out.append(f"- {proj} ({count} note{'s' if count != 1 else ''})")
        out.append("")

    # Journals — recent only, current project preferred when set
    journals = by_prefix("Journals/")
    if journals:
        if project:
            scoped = [(f, fm) for f, fm in journals if fm.get("project", "").strip() == project]
            label = f"## Journals (recent — {project})"
        else:
            scoped = journals
            label = "## Journals (recent)"
        # Sort by `created:` desc, fall back to filename desc
        def _key(item: tuple[Path, dict[str, str]]) -> str:
            return item[1].get("created", "") or item[0].stem
        scoped.sort(key=_key, reverse=True)
        out.append(label)
        for f, fm in scoped[:_RECENT_JOURNAL_LIMIT]:
            out.append(_bullet(f, fm))
        if len(scoped) > _RECENT_JOURNAL_LIMIT:
            out.append(f"_(+{len(scoped) - _RECENT_JOURNAL_LIMIT} older — search by `created:` date)_")
        out.append("")

    return "\n".join(out)


def overview_project(project_vault: Path, project: str | None = None) -> str:
    """Build a markdown overview of a project-vault corpus, grouped by `type:`.

    Project-vaults have no enforced folder structure (init only adds frontmatter,
    it doesn't reorganize). Grouping by frontmatter `type:` gives a stable
    overview shape regardless of how the repo lays out its docs. Files
    without plugin frontmatter are skipped — they're not corpus members yet
    (init backfills them at registration time).
    """
    md_files = enumerate_project_docs(project_vault)
    by_type: dict[str, list[tuple[Path, dict[str, str]]]] = {}
    for f in md_files:
        fm, _ = read_note(f)
        if not fm or "type" not in fm or "description" not in fm:
            continue
        by_type.setdefault(fm.get("type", "untyped"), []).append((f, fm))

    title = f"Project vault: {project}" if project else f"Project vault: {project_vault.name}"
    out: list[str] = [f"# {title}", ""]
    if not by_type:
        out.append("_(no notes with plugin frontmatter — run init to backfill)_")
        out.append("")
        return "\n".join(out)

    for type_ in sorted(by_type.keys()):
        items = sorted(by_type[type_], key=lambda x: x[0])
        out.append(f"## {type_}")
        for f, fm in items:
            out.append(_bullet(f, fm))
        out.append("")

    return "\n".join(out)


# ---------------------------------------------------------------------------
# Vault git change detection + incoming wikilink scan (for backlink reconciliation)
# ---------------------------------------------------------------------------
def _vault_consume_namestatus(out: str, result: dict) -> None:
    tokens = [t for t in out.split("\0") if t]
    i = 0
    while i < len(tokens):
        status = tokens[i]; i += 1
        if status.startswith("R") or status.startswith("C"):
            if i + 1 >= len(tokens):
                break
            old = tokens[i].strip(); i += 1
            new = tokens[i].strip(); i += 1
            if status.startswith("R"):
                result["renamed"].append([old, new])
            else:
                result["added"].append(new)
        else:
            if i >= len(tokens):
                break
            p = tokens[i].strip(); i += 1
            if status == "A":
                result["added"].append(p)
            elif status == "M":
                result["modified"].append(p)
            elif status == "D":
                result["deleted"].append(p)


def vault_md_changes(vault: Path, base_sha: str | None = None) -> dict:
    """Return *.md changes in the vault between base_sha (or HEAD) and current.

    Includes uncommitted working tree + untracked. Returns {added, modified,
    deleted, renamed[old,new]}. Skips paths under SKIP_DIRS.
    """
    import subprocess as _sp
    vault = Path(vault).resolve()
    result: dict = {"added": [], "modified": [], "deleted": [], "renamed": []}

    if not (vault / ".git").exists():
        return result

    if base_sha:
        try:
            out = _sp.run(
                ["git", "-C", str(vault), "diff", "--name-status", "-z", "-M",
                 base_sha, "HEAD", "--", "*.md"],
                capture_output=True, text=True, timeout=15,
            )
            if out.returncode == 0:
                _vault_consume_namestatus(out.stdout, result)
        except (FileNotFoundError, _sp.TimeoutExpired):
            pass

    try:
        out = _sp.run(
            ["git", "-C", str(vault), "diff", "--name-status", "-z", "-M",
             "HEAD", "--", "*.md"],
            capture_output=True, text=True, timeout=15,
        )
        if out.returncode == 0:
            _vault_consume_namestatus(out.stdout, result)
    except (FileNotFoundError, _sp.TimeoutExpired):
        pass

    try:
        out = _sp.run(
            ["git", "-C", str(vault), "ls-files", "--others", "--exclude-standard",
             "-z", "--", "*.md"],
            capture_output=True, text=True, timeout=15,
        )
        if out.returncode == 0:
            for p in out.stdout.split("\0"):
                if p:
                    result["added"].append(p)
    except (FileNotFoundError, _sp.TimeoutExpired):
        pass

    def _keep(p: str) -> bool:
        if not p:
            return False
        for skip in SKIP_DIRS:
            if p == skip or p.startswith(skip + "/") or f"/{skip}/" in p:
                return False
        return True

    for k in ("added", "modified", "deleted"):
        result[k] = sorted({p for p in result[k] if _keep(p)})
    deduped: list = []
    seen: set = set()
    for pair in result["renamed"]:
        if not _keep(pair[0]) and not _keep(pair[1]):
            continue
        t = (pair[0], pair[1])
        if t not in seen:
            seen.add(t)
            deduped.append(pair)
    result["renamed"] = deduped
    return result


def incoming_wikilinks(vault: Path, target_relpath: str) -> list[dict]:
    """Find every note that links to `target_relpath` via [[wikilink]].

    Matches both path-qualified links (vault-relative or path-suffix) and bare
    basename links (when the target's basename is unambiguous in the vault).

    Returns list of {source, raw_link, kind} where kind ∈ {path-qualified, bare}.
    """
    vault = Path(vault).resolve()
    target = target_relpath.replace("\\", "/").strip().lstrip("./").rstrip("/")
    if not target.endswith(".md"):
        target_md = f"{target}.md"
    else:
        target_md = target
    target_stem = Path(target_md).stem

    md_files = collect_md_files(vault)
    basename_counts: dict[str, int] = {}
    for f in md_files:
        basename_counts[f.stem] = basename_counts.get(f.stem, 0) + 1

    results: list[dict] = []
    for f in md_files:
        try:
            text = f.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        body = FRONTMATTER_RE.sub("", text, count=1) if parse_frontmatter(text) else text
        for m in WIKILINK_RE.finditer(body):
            raw = m.group(1)
            link = raw.split("|", 1)[0].strip().split("#", 1)[0].strip().split("^", 1)[0].strip()
            if not link:
                continue
            link_norm = link.replace("\\", "/").lstrip("./").rstrip("/")
            link_md = link_norm if link_norm.endswith(".md") else f"{link_norm}.md"
            kind: str | None = None
            if "/" in link_norm:
                if link_md == target_md:
                    kind = "path-qualified"
                elif link_md.endswith("/" + target_md) or target_md.endswith("/" + link_md):
                    kind = "path-qualified"
            else:
                # bare basename: only attribute as incoming if the target basename
                # is unambiguous in the vault (else we can't tell which note it meant)
                if Path(link_md).stem == target_stem and basename_counts.get(target_stem, 0) == 1:
                    kind = "bare"
            if kind:
                results.append({
                    "source": str(f.relative_to(vault)),
                    "raw_link": raw,
                    "kind": kind,
                })
    return results


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------
def _cmd_search(args: argparse.Namespace) -> int:
    vault = resolve_vault(args.vault)
    if not vault.is_dir():
        print(f"vault not found at: {vault}", file=sys.stderr)
        return 1
    project_vault: Path | None = None
    if args.project_vault:
        project_vault = Path(args.project_vault).expanduser().resolve()
        if not project_vault.is_dir():
            print(f"project-vault not found at: {project_vault}", file=sys.stderr)
            return 1
    results = search(
        vault,
        type_=args.type,
        path_prefix=args.path_prefix,
        keywords=args.keywords,
        created_after=args.created_after,
        created_before=args.created_before,
        limit=args.limit,
        project_vault=project_vault,
    )
    if args.json:
        print(json.dumps(results, indent=2))
    else:
        if not results:
            print("(no matches)")
        for r in results:
            desc = f" — {r['description']}" if r["description"] else ""
            tag = f"[{r['corpus']}] " if project_vault is not None else ""
            print(f"{tag}{r['path']}{desc}")
    return 0


def _cmd_overview(args: argparse.Namespace) -> int:
    vault = resolve_vault(args.vault)
    if not vault.is_dir():
        print(f"vault not found at: {vault}", file=sys.stderr)
        return 1
    print(overview(vault, project=args.project, mode=args.mode))
    if args.project_vault:
        project_vault = Path(args.project_vault).expanduser().resolve()
        if project_vault.is_dir():
            print()
            print(overview_project(project_vault, project=args.project))
        else:
            print(f"\n_(project-vault not found at: {project_vault})_", file=sys.stderr)
    return 0


def _cmd_vault_changes(args: argparse.Namespace) -> int:
    vault = resolve_vault(args.vault)
    if not vault.is_dir():
        print(f"vault not found at: {vault}", file=sys.stderr)
        return 1
    print(json.dumps(vault_md_changes(vault, base_sha=args.base_sha), indent=2))
    return 0


def _cmd_incoming_wikilinks(args: argparse.Namespace) -> int:
    vault = resolve_vault(args.vault)
    if not vault.is_dir():
        print(f"vault not found at: {vault}", file=sys.stderr)
        return 1
    print(json.dumps(incoming_wikilinks(vault, args.target), indent=2))
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--vault", help="vault path override")
    sub = ap.add_subparsers(dest="cmd", required=True)

    sp = sub.add_parser("search", help="search the vault by frontmatter and keywords")
    sp.add_argument("--type", help="filter by frontmatter `type:` (e.g., decision)")
    sp.add_argument("--path-prefix", help="filter by relative path prefix (e.g., Notes, Tools, Journals)")
    sp.add_argument("--keywords", help="space-separated keywords; matched against path, frontmatter, body")
    sp.add_argument("--created-after", help="ISO date YYYY-MM-DD; only notes with frontmatter `created:` >= this date")
    sp.add_argument("--created-before", help="ISO date YYYY-MM-DD; only notes with `created:` <= this date")
    sp.add_argument("--limit", type=int, default=50, help="max results (default 50)")
    sp.add_argument("--json", action="store_true", help="emit JSON instead of text")
    sp.add_argument("--project-vault", help="also search this project-vault corpus (path to project repo)")
    sp.set_defaults(func=_cmd_search)

    op = sub.add_parser("overview", help="emit a markdown vault overview")
    op.add_argument("--project", help="current project name; deep-lists its notes (others appear as a name list)")
    op.add_argument("--mode", choices=["full", "tools-and-general", "tools-only"], default="full",
                    help="overview detail level (default: full)")
    op.add_argument("--project-vault", help="also emit an overview block for this project-vault corpus")
    op.set_defaults(func=_cmd_overview)

    cp = sub.add_parser("vault-changes",
                        help="emit JSON of *.md changes since base SHA (or HEAD), incl. working tree + untracked")
    cp.add_argument("--base-sha", help="git SHA to diff against (default: HEAD)")
    cp.set_defaults(func=_cmd_vault_changes)

    ip = sub.add_parser("incoming-wikilinks",
                        help="find notes linking to TARGET via [[wikilink]] (path-qualified or unambiguous bare basename)")
    ip.add_argument("--target", required=True, help="vault-relative path of the target note (e.g., Notes/bar.md)")
    ip.set_defaults(func=_cmd_incoming_wikilinks)

    args = ap.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
