#!/usr/bin/env python3
"""Slim a Claude Code session transcript for SessionEnd review.

Reads a transcript JSONL file and emits a plain-text dialogue version on
stdout. Strips tool_use / tool_result content (which dominates transcript
size and is mostly noise for save-worthy detection) and keeps only what a
human reading the transcript needs to understand the conversation: who said
what, in what order, with a one-line summary of which tools each assistant
turn invoked.

Usage:
    python3 _slim_transcript.py <transcript.jsonl>          # to stdout
    python3 _slim_transcript.py <transcript.jsonl> -o slim.txt

Designed to keep the review's signal (decisions, corrections, validated
approaches, novel facts) while dropping content the reviewer doesn't act on
(file contents read mid-session, bash command output, search results).
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

# Tool calls that are noise on their own but are interesting if the user
# reacted to their output. We never include the tool RESULT body, but we
# always log that the call happened.
TEXT_PREVIEW_CHARS = 4000  # cap per assistant text block; long pastes still get truncated


def fmt_ts(ts: str) -> str:
    if not ts:
        return ""
    # Trim to seconds: 2026-04-29T12:24:51.697Z → 2026-04-29T12:24:51Z
    if "." in ts:
        head, tail = ts.split(".", 1)
        return (head + "Z") if tail else head
    return ts


def extract_text(content) -> tuple[str, list[str]]:
    """Return (concatenated text, list of tool names used).

    `content` is either a string or a list of content blocks per the API
    schema. We keep `text` blocks (the visible-to-user portion), drop
    `thinking` blocks (verbose, not user-visible), and list `tool_use`
    block names without their inputs. `tool_result` blocks live on user
    messages and are dropped by `slim_user_content`.
    """
    if isinstance(content, str):
        return content, []
    if not isinstance(content, list):
        return "", []

    text_parts: list[str] = []
    tools: list[str] = []
    for block in content:
        if not isinstance(block, dict):
            continue
        bt = block.get("type")
        if bt == "text":
            t = block.get("text", "")
            if t:
                text_parts.append(t[:TEXT_PREVIEW_CHARS] + ("…" if len(t) > TEXT_PREVIEW_CHARS else ""))
        elif bt == "thinking":
            # Drop thinking content; it's verbose and not user-visible
            continue
        elif bt == "tool_use":
            name = block.get("name", "?")
            tools.append(name)
        # tool_result blocks live in user messages (not assistant); we drop them
        # entirely below by skipping non-text content in user messages.

    return "\n".join(text_parts).strip(), tools


def slim_user_content(content) -> str:
    """User messages can be a plain string OR a list with tool_result blocks
    (when Claude Code feeds tool output back to the model). We keep only the
    string portion; tool_result bodies are the bulk of transcript bloat.
    """
    if isinstance(content, str):
        return content.strip()
    if isinstance(content, list):
        parts = []
        for block in content:
            if isinstance(block, dict) and block.get("type") == "text":
                parts.append(str(block.get("text", "")).strip())
        return "\n".join(p for p in parts if p)
    return ""


def slim(transcript_path: Path, out: object) -> dict:
    stats = {"events_in": 0, "events_out": 0, "user": 0, "assistant": 0,
             "bytes_in": 0, "bytes_out": 0, "tool_calls": 0}
    bytes_out = 0

    with transcript_path.open() as f:
        for line in f:
            stats["bytes_in"] += len(line)
            line = line.strip()
            if not line:
                continue
            stats["events_in"] += 1
            try:
                e = json.loads(line)
            except Exception:
                continue

            etype = e.get("type")
            if etype not in ("user", "assistant"):
                continue
            msg = e.get("message") or {}
            ts = fmt_ts(e.get("timestamp", ""))

            if etype == "user":
                text = slim_user_content(msg.get("content"))
                if not text:
                    continue
                line_out = f"[{ts}] USER: {text}\n\n"
                out.write(line_out)
                bytes_out += len(line_out)
                stats["user"] += 1
                stats["events_out"] += 1
            elif etype == "assistant":
                text, tools = extract_text(msg.get("content"))
                pieces: list[str] = []
                if text:
                    pieces.append(f"[{ts}] ASSISTANT: {text}")
                if tools:
                    pieces.append(f"[{ts}] ASSISTANT used: {', '.join(tools)}")
                    stats["tool_calls"] += len(tools)
                if not pieces:
                    continue
                line_out = "\n".join(pieces) + "\n\n"
                out.write(line_out)
                bytes_out += len(line_out)
                stats["assistant"] += 1
                stats["events_out"] += 1

    stats["bytes_out"] = bytes_out
    return stats


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("transcript", help="path to a Claude Code transcript .jsonl")
    ap.add_argument("-o", "--out", help="output file (default: stdout)")
    ap.add_argument("--stats", action="store_true", help="print byte-reduction stats to stderr")
    args = ap.parse_args()

    p = Path(args.transcript)
    if not p.is_file():
        print(f"error: transcript not found: {p}", file=sys.stderr)
        return 2

    if args.out:
        with open(args.out, "w") as f:
            stats = slim(p, f)
    else:
        stats = slim(p, sys.stdout)

    if args.stats and stats["bytes_in"] > 0:
        ratio = stats["bytes_out"] / stats["bytes_in"]
        print(
            f"events: {stats['events_in']} → {stats['events_out']} "
            f"(user={stats['user']}, assistant={stats['assistant']}, tools={stats['tool_calls']})  "
            f"bytes: {stats['bytes_in']:,} → {stats['bytes_out']:,} ({ratio:.1%})",
            file=sys.stderr,
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
