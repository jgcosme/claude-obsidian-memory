#!/usr/bin/env python3
"""Project-vault registry for the obsidian-memory plugin.

Manages ~/.config/obsidian-memory/projects.json — the single source of
truth for which project repos are registered as project-vaults and
whether they're enabled. Used by:
  - session-start.sh        → look up cwd's status, prompt or eager-init
  - statusline.py           → append `• <project>` when registered+enabled
  - init_project_vault.py   → write enabled/disabled entry after init
  - save-memory             → route writes when project has a project-vault

Schema:
  {
    "projects": {
      "/abs/path/to/repo": {
        "enabled": true,
        "project": "repo-basename"
      }
    }
  }

CLI:
  _projects.py lookup <path>            # prints "enabled"/"disabled"/"not_registered"
  _projects.py lookup <path> --json     # full entry as JSON, or {"status": "not_registered"}
  _projects.py register <path> --enabled  --project <name>
  _projects.py register <path> --no-enabled --project <name>
  _projects.py remove <path>            # delete entry; SessionStart will re-prompt
  _projects.py list [--json]
  _projects.py path                     # print resolved projects.json path

Requires Python 3.9+.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path


def projects_path() -> Path:
    """Resolve the projects.json path. Honors $OBSIDIAN_MEMORY_PROJECTS_FILE
    for tests; otherwise ~/.config/obsidian-memory/projects.json."""
    override = os.environ.get("OBSIDIAN_MEMORY_PROJECTS_FILE")
    if override:
        return Path(override).expanduser().resolve()
    return Path.home() / ".config/obsidian-memory/projects.json"


def _load(path: Path) -> dict:
    if not path.is_file():
        return {"projects": {}}
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, UnicodeDecodeError):
        return {"projects": {}}
    if not isinstance(data, dict):
        return {"projects": {}}
    data.setdefault("projects", {})
    if not isinstance(data["projects"], dict):
        data["projects"] = {}
    return data


def _save(path: Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    tmp.replace(path)


def _resolve_repo(path: str) -> str:
    """Return absolute resolved path string for a registry key."""
    return str(Path(path).expanduser().resolve())


def lookup(path: str) -> dict:
    """Return the entry for path, or {"status": "not_registered"} if absent.
    Result always includes a "status" key: enabled / disabled / not_registered."""
    repo_path = _resolve_repo(path)
    data = _load(projects_path())
    entry = data["projects"].get(repo_path)
    if entry is None:
        return {"status": "not_registered", "path": repo_path}
    enabled = bool(entry.get("enabled", False))
    return {
        "status": "enabled" if enabled else "disabled",
        "path": repo_path,
        "enabled": enabled,
        "project": entry.get("project", Path(repo_path).name),
    }


def register(path: str, *, enabled: bool, project: str) -> dict:
    """Insert or update the entry for path. Returns the new entry."""
    repo_path = _resolve_repo(path)
    rp = projects_path()
    data = _load(rp)
    data["projects"][repo_path] = {"enabled": enabled, "project": project}
    _save(rp, data)
    return {
        "status": "enabled" if enabled else "disabled",
        "path": repo_path,
        "enabled": enabled,
        "project": project,
    }


def remove(path: str) -> bool:
    """Delete the entry for path. Returns True if an entry existed and was
    removed, False if no entry was present (no-op)."""
    repo_path = _resolve_repo(path)
    rp = projects_path()
    data = _load(rp)
    if repo_path not in data["projects"]:
        return False
    del data["projects"][repo_path]
    _save(rp, data)
    return True


def list_all() -> list[dict]:
    data = _load(projects_path())
    out: list[dict] = []
    for path, entry in sorted(data["projects"].items()):
        enabled = bool(entry.get("enabled", False))
        out.append({
            "path": path,
            "enabled": enabled,
            "status": "enabled" if enabled else "disabled",
            "project": entry.get("project", Path(path).name),
        })
    return out


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------
def _cmd_lookup(args: argparse.Namespace) -> int:
    result = lookup(args.path)
    if args.json:
        print(json.dumps(result, indent=2))
    else:
        print(result["status"])
    return 0


def _cmd_register(args: argparse.Namespace) -> int:
    result = register(args.path, enabled=args.enabled, project=args.project)
    if args.json:
        print(json.dumps(result, indent=2))
    else:
        print(f"{result['status']}: {result['path']}")
    return 0


def _cmd_remove(args: argparse.Namespace) -> int:
    removed = remove(args.path)
    if args.json:
        print(json.dumps({"removed": removed, "path": _resolve_repo(args.path)}))
    else:
        if removed:
            print(f"removed: {_resolve_repo(args.path)}")
        else:
            print(f"no entry for: {_resolve_repo(args.path)}")
    return 0 if removed else 1


def _cmd_list(args: argparse.Namespace) -> int:
    items = list_all()
    if args.json:
        print(json.dumps(items, indent=2))
    else:
        if not items:
            print("(no projects registered)")
        for item in items:
            mark = "✓" if item["enabled"] else "✗"
            print(f"  {mark} [{item['project']}] {item['path']}")
    return 0


def _cmd_path(_args: argparse.Namespace) -> int:
    print(projects_path())
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    sub = ap.add_subparsers(dest="cmd", required=True)

    lp = sub.add_parser("lookup", help="check registration status of a project path")
    lp.add_argument("path")
    lp.add_argument("--json", action="store_true")
    lp.set_defaults(func=_cmd_lookup)

    rp = sub.add_parser("register", help="add or update a project entry")
    rp.add_argument("path")
    rp.add_argument("--enabled", dest="enabled", action="store_true",
                    help="mark this project as enabled (default)")
    rp.add_argument("--no-enabled", dest="enabled", action="store_false",
                    help="mark this project as disabled (declined)")
    rp.set_defaults(enabled=True)
    rp.add_argument("--project", required=True, help="project name (usually repo basename)")
    rp.add_argument("--json", action="store_true")
    rp.set_defaults(func=_cmd_register)

    rmp = sub.add_parser("remove", help="delete a project entry (registration prompt fires again next session)")
    rmp.add_argument("path")
    rmp.add_argument("--json", action="store_true")
    rmp.set_defaults(func=_cmd_remove)

    sp = sub.add_parser("list", help="list all registered projects")
    sp.add_argument("--json", action="store_true")
    sp.set_defaults(func=_cmd_list)

    pp = sub.add_parser("path", help="print the resolved projects.json path")
    pp.set_defaults(func=_cmd_path)

    args = ap.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
