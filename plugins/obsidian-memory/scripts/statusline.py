#!/usr/bin/env python3
"""Claude Code statusline: shows obsidian-memory plugin's token share for the
current session. Mirrors the accounting in the plugin's /usage script.

Stdin: Claude Code session JSON (session_id, transcript_path, cwd, ...).
Stdout: one line, e.g. "obsidian-memory 18.2k tok · 4.3%"
       When cwd's repo is registered+enabled in repos.json, the prefix
       becomes "obsidian-memory • <project>" so the project tag is visible
       at a glance.
"""
from __future__ import annotations
import json, os, subprocess, sys
from datetime import datetime, timezone
from pathlib import Path

USAGE_DIR = os.environ.get("MEMORY_USAGE_DIR", "/tmp/claude-memory-usage")
CHARS_KINDS = ("session_start", "gate_inject")


def _project_tag(cwd: str) -> str:
    """Return the registered project name for cwd's repo, or '' if not
    registered+enabled. Best-effort: any error → ''."""
    if not cwd:
        return ""
    try:
        toplevel = subprocess.run(
            ["git", "-C", cwd, "rev-parse", "--show-toplevel"],
            capture_output=True, text=True, timeout=2,
        )
    except (FileNotFoundError, subprocess.SubprocessError):
        return ""
    if toplevel.returncode != 0:
        return ""
    repo_root = toplevel.stdout.strip()
    if not repo_root:
        return ""
    repos_file = os.environ.get(
        "OBSIDIAN_MEMORY_REPOS_FILE",
        str(Path.home() / ".config/obsidian-memory/repos.json"),
    )
    try:
        with open(repos_file, encoding="utf-8") as f:
            data = json.load(f)
    except (FileNotFoundError, json.JSONDecodeError, OSError):
        return ""
    entry = (data.get("repos") or {}).get(str(Path(repo_root).resolve()))
    if not entry or not entry.get("enabled"):
        return ""
    return str(entry.get("project") or "")


def parse_ts(s: str):
    if not s:
        return None
    if s.endswith("Z"):
        s = s[:-1] + "+00:00"
    try:
        dt = datetime.fromisoformat(s)
    except ValueError:
        return None
    if dt.tzinfo is None:
        dt = dt.astimezone(timezone.utc)
    return dt


def fmt_tok(n: int) -> str:
    if n >= 1_000_000:
        return f"{n/1_000_000:.1f}M"
    if n >= 1_000:
        return f"{n/1_000:.1f}k"
    return str(n)


def render(text: str = "", project: str = "") -> None:
    # Obsidian-purple (#A78BFA) prefix via 24-bit truecolor.
    prefix = "\033[38;2;167;139;250mobsidian-memory\033[0m"
    if project:
        # Dim bullet + project name in default color.
        prefix = f"{prefix} \033[2m•\033[0m {project}"
    if text:
        sys.stdout.write(f"{prefix} {text}")
    else:
        sys.stdout.write(f"{prefix} \033[2m—\033[0m")
    sys.stdout.write("\n")


def main() -> None:
    try:
        payload = json.load(sys.stdin)
    except Exception:
        render()
        return

    session_id = payload.get("session_id") or ""
    transcript = payload.get("transcript_path") or ""
    cwd = payload.get("cwd") or os.getcwd()
    project = _project_tag(cwd)

    if not session_id:
        render(project=project)
        return

    safe_id = "".join(c if c.isalnum() or c in "._-" else "_" for c in session_id)
    plugin_log = os.path.join(USAGE_DIR, f"{safe_id}.jsonl")

    # Plugin-side events
    api_sum = 0
    chars_events = []  # (approx_tokens, ts)
    if os.path.isfile(plugin_log):
        try:
            with open(plugin_log) as f:
                for line in f:
                    line = line.strip()
                    if not line:
                        continue
                    try:
                        e = json.loads(line)
                    except Exception:
                        continue
                    kind = e.get("kind", "")
                    if e.get("mode") == "api":
                        u = e.get("usage", {}) or {}
                        api_sum += (
                            int(u.get("input_tokens", 0) or 0)
                            + int(u.get("output_tokens", 0) or 0)
                            + int(u.get("cache_read_input_tokens", 0) or 0)
                            + int(u.get("cache_creation_input_tokens", 0) or 0)
                        )
                    elif kind in CHARS_KINDS:
                        tok = int(e.get("approx_tokens", 0) or 0)
                        ts = parse_ts(e.get("ts", ""))
                        chars_events.append((tok, ts))
        except Exception:
            pass

    # Main transcript usage (deduped by message.id to avoid snapshot/replay double-count)
    main_sum = 0
    main_msgs_ts = []
    if transcript and os.path.isfile(transcript):
        seen = set()
        try:
            with open(transcript) as f:
                for line in f:
                    try:
                        e = json.loads(line)
                    except Exception:
                        continue
                    msg = e.get("message") or {}
                    usage = msg.get("usage")
                    if not usage:
                        continue
                    mid = msg.get("id")
                    if mid:
                        if mid in seen:
                            continue
                        seen.add(mid)
                    t = parse_ts(e.get("timestamp", ""))
                    if t is not None:
                        main_msgs_ts.append(t)
                    main_sum += (
                        int(usage.get("input_tokens", 0) or 0)
                        + int(usage.get("output_tokens", 0) or 0)
                        + int(usage.get("cache_read_input_tokens", 0) or 0)
                        + int(usage.get("cache_creation_input_tokens", 0) or 0)
                    )
        except Exception:
            pass

    # Injection × turns-alive attribution
    n_msgs = len(main_msgs_ts)
    plugin_main_attr = 0
    for tok, ts in chars_events:
        if ts is None:
            turns_alive = n_msgs
        else:
            turns_alive = sum(1 for mt in main_msgs_ts if mt >= ts)
        plugin_main_attr += tok * turns_alive

    plugin_total = plugin_main_attr + api_sum
    total_session = main_sum + api_sum

    if total_session <= 0 or plugin_total <= 0:
        render(project=project)
        return

    share = plugin_total / total_session * 100
    # Color the share dim/yellow/red by magnitude — passive cost signal.
    if share >= 25:
        color = "\033[31m"  # red
    elif share >= 10:
        color = "\033[33m"  # yellow
    else:
        color = "\033[2m"   # dim
    reset = "\033[0m"
    render(f"{fmt_tok(plugin_total)} tok · {color}{share:.1f}%{reset}", project=project)


if __name__ == "__main__":
    main()
