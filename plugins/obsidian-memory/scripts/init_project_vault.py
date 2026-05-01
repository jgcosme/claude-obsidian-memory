#!/usr/bin/env python3
"""Initialize a project's docs as a project-vault corpus.

For each .md file enumerated by _project_docs.enumerate_project_docs
that lacks ANY frontmatter, prepend an Obsidian-style frontmatter block
with: type, description, created, project

Files that already have a frontmatter block (of any kind — plugin,
Claude Code skill, slash command, etc.) are left untouched. This makes
init idempotent and safe to re-run, and avoids stomping on non-plugin
frontmatter conventions in the repo.

Type inference:
  Default mode → batched `claude -p` call classifies all candidates at once.
                 Output is parsed line-by-line; per-file failure falls back
                 to type=reference. Whole-call failure (no claude binary,
                 timeout, malformed output) falls back to reference for
                 every candidate.
  --no-llm     → skip the LLM call entirely; every candidate gets
                 type=reference + H1-derived description.

Description inference:
  Always anchored on the file's H1 (or first non-blank line). LLM may
  refine this; the fallback always uses the H1.

CLI:
  python3 init_project_vault.py <project_path> --project <name>
  python3 init_project_vault.py <project_path> --project <name> --dry-run
  python3 init_project_vault.py <project_path> --project <name> --no-llm

Requires Python 3.9+.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from datetime import date
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from _project_docs import enumerate_project_docs  # noqa: E402
from _vault import FRONTMATTER_RE  # noqa: E402

VALID_TYPES = ("preference", "reference", "decision", "learning", "tool", "journal")
FALLBACK_TYPE = "reference"
H1_RE = re.compile(r"^#\s+(.+?)\s*$", re.MULTILINE)
MAX_DESCRIPTION_LEN = 120
# Per-file body excerpt budget (chars) when batching for the LLM.
LLM_EXCERPT_CHARS = 600


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
def _has_frontmatter(text: str) -> bool:
    return FRONTMATTER_RE.match(text) is not None


def _derive_description(body: str, fallback_path: str) -> str:
    """H1 → first non-blank line → bare basename. Truncated and cleaned."""
    m = H1_RE.search(body)
    if m:
        desc = m.group(1).strip()
    else:
        for line in body.splitlines():
            stripped = line.strip()
            if stripped:
                desc = stripped
                break
        else:
            desc = Path(fallback_path).stem.replace("-", " ").replace("_", " ")
    # Sanitize: strip wrapping punctuation, collapse whitespace, drop embedded
    # `: ` patterns that would break unquoted YAML descriptions.
    desc = re.sub(r"\s+", " ", desc)
    desc = desc.replace(": ", " - ")
    desc = desc.strip("#").strip().strip("\"'")
    if len(desc) > MAX_DESCRIPTION_LEN:
        desc = desc[: MAX_DESCRIPTION_LEN - 1].rstrip() + "…"
    return desc


def _excerpt(body: str, n: int = LLM_EXCERPT_CHARS) -> str:
    if len(body) <= n:
        return body
    return body[:n].rstrip() + "\n…"


def _build_llm_prompt(candidates: list[tuple[Path, str]]) -> str:
    """One batched classification prompt covering all candidates."""
    type_list = "\n".join(f"- {t}" for t in VALID_TYPES)
    parts = [
        "You are classifying markdown files in a project repository to add "
        "Obsidian-style memory frontmatter. For each file, output ONE JSON "
        "object PER LINE — no prose, no code fences, no commentary.",
        "",
        "Schema per line:",
        '  {"path": "<exact path as given>", "type": "<one of below>", '
        '"description": "<one-line summary, ≤120 chars, no embedded `: `>"}',
        "",
        "Types:",
        type_list,
        "",
        "Rules:",
        "- If unsure, use \"reference\" — it's the safe fallback.",
        "- Description: derive from H1 or first paragraph. One line. Replace "
        "any embedded `: ` with ` - ` so the description parses as unquoted YAML.",
        "- Output exactly one line per file, in the same order.",
        "",
        "FILES:",
    ]
    for i, (rel_path, body) in enumerate(candidates, 1):
        parts.append(f"=== FILE {i}: {rel_path} ===")
        parts.append(_excerpt(body))
        parts.append("")
    return "\n".join(parts)


def _parse_llm_output(raw: str, candidates: list[tuple[Path, str]]) -> dict[str, dict]:
    """Map {rel_path: {type, description}} from line-delimited JSON.

    Tolerates: blank lines, prose lines, malformed JSON. Falls back to
    type=reference for any candidate not classified or classified with an
    invalid type.
    """
    by_path: dict[str, dict] = {}
    for line in raw.splitlines():
        line = line.strip()
        if not line or not line.startswith("{"):
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        path = obj.get("path", "")
        type_ = obj.get("type", "")
        desc = obj.get("description", "")
        if type_ not in VALID_TYPES:
            type_ = FALLBACK_TYPE
        if isinstance(desc, str) and desc:
            desc = re.sub(r"\s+", " ", desc).replace(": ", " - ").strip()
            if len(desc) > MAX_DESCRIPTION_LEN:
                desc = desc[: MAX_DESCRIPTION_LEN - 1].rstrip() + "…"
        else:
            desc = ""
        by_path[path] = {"type": type_, "description": desc}

    # Fallback for any candidate the LLM didn't classify.
    for rel_path, body in candidates:
        rel_str = str(rel_path)
        if rel_str not in by_path or not by_path[rel_str].get("description"):
            fallback_desc = _derive_description(body, rel_str)
            by_path.setdefault(rel_str, {})
            by_path[rel_str].setdefault("type", FALLBACK_TYPE)
            if not by_path[rel_str].get("description"):
                by_path[rel_str]["description"] = fallback_desc
    return by_path


def _llm_classify(
    candidates: list[tuple[Path, str]],
    *,
    log_path: Path | None = None,
) -> dict[str, dict]:
    """Batched claude -p classification. On failure, returns deterministic
    fallback (every file → type=reference, H1-derived description)."""
    claude_bin = os.environ.get("CLAUDE_BIN") or shutil.which("claude")
    if not claude_bin:
        return _deterministic_fallback(candidates)

    prompt = _build_llm_prompt(candidates)
    env = {**os.environ, "CLAUDE_MEMORY_GATE": "1", "CLAUDE_MEMORY_REVIEW": "1"}
    try:
        result = subprocess.run(
            [
                claude_bin,
                "-p", prompt,
                "--tools", "",
                "--strict-mcp-config",
                "--output-format", "json",
            ],
            env=env,
            capture_output=True,
            text=True,
            timeout=180,
        )
    except (subprocess.TimeoutExpired, FileNotFoundError) as e:
        if log_path:
            log_path.write_text(f"llm classify failed: {e}\n", encoding="utf-8")
        return _deterministic_fallback(candidates)

    if result.returncode != 0:
        if log_path:
            log_path.write_text(
                f"llm classify exited {result.returncode}\nstderr: {result.stderr}\n",
                encoding="utf-8",
            )
        return _deterministic_fallback(candidates)

    # Unwrap claude -p JSON envelope.
    try:
        events = json.loads(result.stdout)
        text = ""
        for ev in events if isinstance(events, list) else []:
            if isinstance(ev, dict) and ev.get("type") == "result":
                text = ev.get("result", "")
                break
        if not text:
            text = result.stdout
    except json.JSONDecodeError:
        text = result.stdout

    return _parse_llm_output(text, candidates)


def _deterministic_fallback(candidates: list[tuple[Path, str]]) -> dict[str, dict]:
    return {
        str(rel_path): {
            "type": FALLBACK_TYPE,
            "description": _derive_description(body, str(rel_path)),
        }
        for rel_path, body in candidates
    }


# ---------------------------------------------------------------------------
# Frontmatter writing
# ---------------------------------------------------------------------------
def _format_frontmatter(*, type_: str, description: str, project: str) -> str:
    today = date.today().isoformat()
    # Always quote description — it may contain wikilinks, brackets, or other
    # YAML-ambiguous chars. Escape embedded double quotes.
    safe_desc = description.replace("\\", "\\\\").replace('"', '\\"')
    return (
        f"---\n"
        f"type: {type_}\n"
        f'description: "{safe_desc}"\n'
        f"created: {today}\n"
        f"project: {project}\n"
        f"---\n\n"
    )


def _write_file(path: Path, frontmatter: str, body: str) -> None:
    path.write_text(frontmatter + body, encoding="utf-8")


# ---------------------------------------------------------------------------
# Top-level
# ---------------------------------------------------------------------------
def init_project_vault(
    project_path: Path,
    *,
    project: str,
    dry_run: bool = False,
    use_llm: bool = True,
) -> dict:
    """Add plugin frontmatter to candidate .md files in project_path.

    Returns: {"added": [{path, type, description}], "skipped": [{path, reason}]}
    """
    repo = Path(project_path).expanduser().resolve()
    md_files = enumerate_project_docs(repo)

    candidates: list[tuple[Path, str]] = []
    skipped: list[dict] = []
    bodies: dict[Path, str] = {}

    for f in md_files:
        try:
            text = f.read_text(encoding="utf-8")
        except (UnicodeDecodeError, OSError) as e:
            skipped.append({"path": str(f.relative_to(repo)), "reason": f"unreadable: {e}"})
            continue
        if _has_frontmatter(text):
            skipped.append({"path": str(f.relative_to(repo)), "reason": "has frontmatter"})
            continue
        rel = f.relative_to(repo)
        candidates.append((rel, text))
        bodies[rel] = text

    if not candidates:
        return {"added": [], "skipped": skipped}

    if use_llm:
        classifications = _llm_classify(candidates)
    else:
        classifications = _deterministic_fallback(candidates)

    added: list[dict] = []
    for rel_path, body in candidates:
        rel_str = str(rel_path)
        cls = classifications.get(rel_str) or {
            "type": FALLBACK_TYPE,
            "description": _derive_description(body, rel_str),
        }
        type_ = cls["type"] if cls.get("type") in VALID_TYPES else FALLBACK_TYPE
        desc = cls.get("description") or _derive_description(body, rel_str)

        fm = _format_frontmatter(type_=type_, description=desc, project=project)
        target = repo / rel_path
        if not dry_run:
            _write_file(target, fm, body)
        added.append({"path": rel_str, "type": type_, "description": desc})

    return {"added": added, "skipped": skipped}


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------
def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    ap.add_argument("project_path", help="path to the project's git repo")
    ap.add_argument("--project", required=True, help="project name (e.g., repo basename)")
    ap.add_argument("--dry-run", action="store_true", help="print plan without writing")
    ap.add_argument("--no-llm", action="store_true",
                    help="skip the LLM type-inference call; every candidate gets type=reference")
    ap.add_argument("--json", action="store_true", help="emit JSON result instead of text")
    args = ap.parse_args(argv)

    repo = Path(args.project_path).expanduser().resolve()
    if not (repo / ".git").exists():
        print(f"not a git repo: {repo}", file=sys.stderr)
        return 1

    result = init_project_vault(
        repo,
        project=args.project,
        dry_run=args.dry_run,
        use_llm=not args.no_llm,
    )

    if args.json:
        print(json.dumps(result, indent=2))
        return 0

    verb = "would add" if args.dry_run else "added"
    if result["added"]:
        print(f"{verb} frontmatter to {len(result['added'])} file(s):")
        for item in result["added"]:
            print(f"  + [{item['type']}] {item['path']} — {item['description']}")
    else:
        print("no candidates needed frontmatter")
    if result["skipped"]:
        print(f"\nskipped {len(result['skipped'])} file(s):")
        for item in result["skipped"][:10]:
            print(f"  - {item['path']} ({item['reason']})")
        if len(result["skipped"]) > 10:
            print(f"  … and {len(result['skipped']) - 10} more")
    return 0


if __name__ == "__main__":
    sys.exit(main())
