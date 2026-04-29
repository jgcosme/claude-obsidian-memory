#!/usr/bin/env python3
"""Bucket-routing evaluation harness.

Compares one umbrella save-skill prompt against three specialized prompts
(save-tool, save-general, save-project) on the same bucket-labeled fixture.
Reports per-skill metrics + system-level metrics (any-skill-fired recall,
no-skill-fired neg_acc, routing accuracy).

Usage:
  python3 tests/run_bucket_eval.py \\
    --umbrella tests/prompts/write-skill.txt \\
    --tool     tests/prompts/save-tool.txt \\
    --general  tests/prompts/save-general.txt \\
    --project  tests/prompts/save-project.txt \\
    --cases    tests/cases-bucket.json

Output: tests/prompts/report-bucket-eval.md. ~$0.20-0.40 per run.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

THIS = Path(__file__).resolve()
TESTS_DIR = THIS.parent
CLAUDE_BIN = os.environ.get("CLAUDE_BIN") or "claude"
PARALLELISM = int(os.environ.get("BUCKET_EVAL_PARALLELISM", "4"))


def parse_obj(raw: str) -> dict | None:
    if not raw:
        return None
    start = raw.find("{")
    if start < 0:
        return None
    depth = 0
    in_str = False
    esc = False
    end = -1
    for i in range(start, len(raw)):
        c = raw[i]
        if esc:
            esc = False
            continue
        if c == "\\":
            esc = True
            continue
        if c == '"':
            in_str = not in_str
            continue
        if in_str:
            continue
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                end = i
                break
    if end < 0:
        return None
    try:
        return json.loads(raw[start:end + 1])
    except Exception:
        return None


def call(system_prompt: str, message: str, timeout: int = 30) -> dict | None:
    user_prompt = f"{message}\n\nJSON only:"
    env = {**os.environ, "CLAUDE_MEMORY_GATE": "1", "CLAUDE_MEMORY_REVIEW": "1"}
    try:
        result = subprocess.run(
            [CLAUDE_BIN, "-p", user_prompt,
             "--system-prompt", system_prompt,
             "--tools", ""],
            capture_output=True, text=True, timeout=timeout, env=env,
        )
        if result.returncode != 0:
            return None
        return parse_obj(result.stdout)
    except subprocess.TimeoutExpired:
        return None


def fired(out: dict | None) -> bool:
    return bool(out and out.get("save") is True)


def evaluate(prompts: dict[str, str], cases: list[dict]) -> dict:
    """Run every (skill, case) cell. Returns dict[skill][case_idx] = bool fired."""
    results: dict[str, list[bool]] = {name: [None] * len(cases) for name in prompts}

    def run_cell(name: str, idx: int):
        out = call(prompts[name], cases[idx]["msg"])
        results[name][idx] = fired(out)

    cells = [(n, i) for n in prompts for i in range(len(cases))]
    with ThreadPoolExecutor(max_workers=PARALLELISM) as ex:
        list(ex.map(lambda nc: run_cell(*nc), cells))
    return results


def metrics(results: dict[str, list[bool]], cases: list[dict]) -> dict:
    """Compute per-skill and system-level metrics."""
    n = len(cases)
    out = {"per_skill": {}, "system": {}}

    # Per-skill metrics: each specialized skill should fire on its own bucket
    # (positive) and NOT fire on others (negative for it).
    for name, fires in results.items():
        if name == "umbrella":
            target = lambda b: b is not None  # any non-null bucket
        else:
            target = lambda b: b == name      # only own bucket

        tp = fp = tn = fn = 0
        for c, f in zip(cases, fires):
            should = target(c["bucket"])
            if f and should:    tp += 1
            elif f and not should: fp += 1
            elif not f and should: fn += 1
            else:                 tn += 1
        positives = tp + fn
        recall    = tp / positives if positives else 1.0
        neg_acc   = tn / (tn + fp) if (tn + fp) else 1.0
        precision = tp / (tp + fp) if (tp + fp) else 1.0
        out["per_skill"][name] = {
            "tp": tp, "fp": fp, "tn": tn, "fn": fn,
            "recall": recall, "neg_acc": neg_acc, "precision": precision,
        }

    # System-level metrics for the multi-skill setup: aggregate over the three
    # specialized skills. Any-skill-fired = recall; no-skill-fired-on-null = neg_acc.
    spec_names = [n for n in results if n != "umbrella"]
    sys_recall_hit = sys_recall_total = 0
    sys_neg_correct = sys_neg_total = 0
    routing_correct = routing_total = 0
    multi_fire = 0
    for i, c in enumerate(cases):
        any_fired = any(results[n][i] for n in spec_names)
        which = [n for n in spec_names if results[n][i]]
        if c["bucket"] is None:
            sys_neg_total += 1
            if not any_fired:
                sys_neg_correct += 1
        else:
            sys_recall_total += 1
            if any_fired:
                sys_recall_hit += 1
                routing_total += 1
                if c["bucket"] in which:
                    routing_correct += 1
                if len(which) > 1:
                    multi_fire += 1
    out["system"] = {
        "multi_recall":     sys_recall_hit / sys_recall_total if sys_recall_total else 1.0,
        "multi_neg_acc":    sys_neg_correct / sys_neg_total if sys_neg_total else 1.0,
        "routing_accuracy": routing_correct / routing_total if routing_total else 1.0,
        "multi_fires":      multi_fire,  # cases where >1 specialized skill fired
        "totals":           {"recall_pos": sys_recall_total, "neg": sys_neg_total},
    }
    return out


def render(m: dict, results: dict[str, list[bool]], cases: list[dict], prompts: dict[str, str]) -> str:
    out = ["# Bucket-routing eval\n"]

    out.append("## Per-skill metrics\n")
    out.append("| skill | recall | neg_acc | precision | tp | fp | tn | fn | prompt chars |")
    out.append("|---|---|---|---|---|---|---|---|---|")
    for name in ("umbrella", "tool", "general", "project"):
        if name not in m["per_skill"]:
            continue
        s = m["per_skill"][name]
        out.append(f"| {name} | {s['recall']:.0%} | {s['neg_acc']:.0%} | {s['precision']:.0%} | {s['tp']} | {s['fp']} | {s['tn']} | {s['fn']} | {len(prompts[name])} |")
    out.append("")

    sys = m["system"]
    out.append("## System-level (multi-skill = union of tool/general/project)\n")
    out.append(f"- **Multi-skill recall** (any specialized skill fired on positive): {sys['multi_recall']:.0%}")
    out.append(f"- **Multi-skill neg_acc** (no specialized skill fired on null): {sys['multi_neg_acc']:.0%}")
    out.append(f"- **Routing accuracy** (when fired, the right skill fired): {sys['routing_accuracy']:.0%}")
    out.append(f"- **Multi-fire count** (cases where >1 specialized skill fired): {sys['multi_fires']}")
    out.append("")

    out.append("## Comparison\n")
    u = m["per_skill"]["umbrella"]
    out.append(f"- Umbrella recall: {u['recall']:.0%}  vs  Multi recall: {sys['multi_recall']:.0%}")
    out.append(f"- Umbrella neg_acc: {u['neg_acc']:.0%}  vs  Multi neg_acc: {sys['multi_neg_acc']:.0%}")
    umbrella_tokens = len(prompts["umbrella"])
    multi_tokens = sum(len(prompts[n]) for n in ("tool", "general", "project"))
    out.append(f"- Description budget: umbrella {umbrella_tokens} chars  vs  multi {multi_tokens} chars")
    out.append("")

    out.append("## Per-case detail\n")
    out.append("| bucket | msg | umbrella | tool | general | project |")
    out.append("|---|---|---|---|---|---|")
    for i, c in enumerate(cases):
        b = c["bucket"] or "—"
        msg = c["msg"][:80].replace("|", "\\|")
        cells = []
        for n in ("umbrella", "tool", "general", "project"):
            if n not in results:
                cells.append("—")
            else:
                cells.append("✓" if results[n][i] else "·")
        out.append(f"| {b} | {msg} | {cells[0]} | {cells[1]} | {cells[2]} | {cells[3]} |")
    return "\n".join(out)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--umbrella", required=True)
    ap.add_argument("--tool", required=True)
    ap.add_argument("--general", required=True)
    ap.add_argument("--project", required=True)
    ap.add_argument("--cases", required=True)
    ap.add_argument("--output", default=str(TESTS_DIR / "prompts" / "report-bucket-eval.md"))
    args = ap.parse_args()

    prompts = {
        "umbrella": Path(args.umbrella).read_text(),
        "tool":     Path(args.tool).read_text(),
        "general":  Path(args.general).read_text(),
        "project":  Path(args.project).read_text(),
    }
    cases = json.loads(Path(args.cases).read_text())["cases"]

    n_calls = len(prompts) * len(cases)
    print(f"running {n_calls} calls ({len(prompts)} prompts × {len(cases)} cases)...", file=sys.stderr)
    t0 = time.time()
    results = evaluate(prompts, cases)
    dt = time.time() - t0
    print(f"done in {dt:.1f}s", file=sys.stderr)

    m = metrics(results, cases)
    md = render(m, results, cases, prompts)
    Path(args.output).write_text(md)
    print(args.output)

    sys_m = m["system"]
    u = m["per_skill"]["umbrella"]
    print(f"umbrella: recall={u['recall']:.0%} neg_acc={u['neg_acc']:.0%}", file=sys.stderr)
    print(f"multi:    recall={sys_m['multi_recall']:.0%} neg_acc={sys_m['multi_neg_acc']:.0%} routing={sys_m['routing_accuracy']:.0%}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
