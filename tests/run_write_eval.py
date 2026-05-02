#!/usr/bin/env python3
"""Write-trigger evaluation harness.

Tests whether a candidate prompt correctly decides "should I save this as a
proactive memory note?" for individual user messages. Compares hook-mode
(SessionEnd review) vs skill-mode (save-memory skill description) on the
same fixture set in `cases-write.json`.

Usage:
  python3 tests/run_write_eval.py tests/prompts/write-hook.txt
  python3 tests/run_write_eval.py tests/prompts/write-skill.txt --tag skill

Output: writes `report-write-<tag>.md` next to the prompt file.
Costs real Anthropic API calls (~$0.03–0.10 per run).
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
DEFAULT_CASES_FILE = TESTS_DIR / "cases-write.json"
CLAUDE_BIN = os.environ.get("CLAUDE_BIN") or "claude"
PARALLELISM = int(os.environ.get("WRITE_EVAL_PARALLELISM", "4"))


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


def call(system_prompt: str, message: str, timeout: int = 30) -> tuple[dict | None, str, float]:
    user_prompt = f"{message}\n\nJSON only:"
    env = {**os.environ, "CLAUDE_MEMORY_GATE": "1", "CLAUDE_MEMORY_REVIEW": "1"}
    t0 = time.time()
    try:
        result = subprocess.run(
            [CLAUDE_BIN, "-p", user_prompt,
             "--system-prompt", system_prompt,
             "--tools", ""],
            capture_output=True, text=True, timeout=timeout, env=env,
        )
        dt = time.time() - t0
        if result.returncode != 0:
            return None, f"[exit {result.returncode}] {result.stderr[:200]}", dt
        return parse_obj(result.stdout), result.stdout.strip()[:200], dt
    except subprocess.TimeoutExpired:
        return None, "[timeout]", time.time() - t0


def evaluate(prompt: str, cases: dict) -> dict:
    pos = cases["positive"]
    neg = cases["negative"]

    def run_one(case: dict, expect_save: bool) -> dict:
        out, raw, dt = call(prompt, case["msg"])
        return {"msg": case["msg"], "expect_save": expect_save, "out": out, "raw": raw, "dt": dt}

    results = {"positive": [], "negative": []}
    with ThreadPoolExecutor(max_workers=PARALLELISM) as ex:
        futs_p = [ex.submit(run_one, c, True) for c in pos]
        futs_n = [ex.submit(run_one, c, False) for c in neg]
        results["positive"] = [f.result() for f in futs_p]
        results["negative"] = [f.result() for f in futs_n]

    m = {"pos_total": len(pos), "pos_hit": 0, "pos_miss": 0,
         "neg_total": len(neg), "neg_correct": 0, "neg_overinject": 0}

    for r in results["positive"]:
        out = r["out"] or {}
        if out.get("save") is True:
            m["pos_hit"] += 1
            r["score"] = "HIT"
        else:
            m["pos_miss"] += 1
            r["score"] = "MISS"

    for r in results["negative"]:
        out = r["out"] or {}
        if out.get("save") is True:
            m["neg_overinject"] += 1
            r["score"] = "OVER-SAVE"
        else:
            m["neg_correct"] += 1
            r["score"] = "OK"

    m["recall"] = m["pos_hit"] / max(1, m["pos_total"])
    m["neg_acc"] = m["neg_correct"] / max(1, m["neg_total"])
    m["precision"] = m["pos_hit"] / max(1, m["pos_hit"] + m["neg_overinject"])
    return {"metrics": m, "results": results}


def render(report: dict, prompt: str, tag: str) -> str:
    m = report["metrics"]
    out = [
        f"# Write-trigger eval: {tag}\n",
        f"**Recall** (positives saved): {m['pos_hit']}/{m['pos_total']} = {m['recall']:.0%}",
        f"**Negative accuracy** (no over-save): {m['neg_correct']}/{m['neg_total']} = {m['neg_acc']:.0%}",
        f"**Precision** (hits / all saves): {m['precision']:.0%}\n",
        f"Counts: hit={m['pos_hit']} miss={m['pos_miss']} over-save={m['neg_overinject']}\n",
        f"Prompt size: {len(prompt)} chars\n",
        "## Failures\n",
    ]
    fails = []
    for r in report["results"]["positive"]:
        if r["score"] == "MISS":
            fails.append(f"- [MISS] `{r['msg']}` → {r['out']}")
    for r in report["results"]["negative"]:
        if r["score"] == "OVER-SAVE":
            fails.append(f"- [OVER-SAVE] `{r['msg']}` → {r['out']}")
    out.extend(fails or ["_(none)_"])
    out.append("\n## All results\n")
    for cat in ("positive", "negative"):
        out.append(f"### {cat}")
        for r in report["results"][cat]:
            out.append(f"- [{r['score']}] `{r['msg']}` → {r['out']} ({r['dt']:.1f}s)")
        out.append("")
    return "\n".join(out)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("prompt_file")
    ap.add_argument("--tag", default=None)
    ap.add_argument("--cases", default=str(DEFAULT_CASES_FILE), help="Path to cases JSON (default: cases-write.json)")
    args = ap.parse_args()
    cases_file = Path(args.cases)

    prompt_path = Path(args.prompt_file)
    if not prompt_path.is_file():
        print(f"error: prompt file not found: {prompt_path}", file=sys.stderr)
        return 2
    prompt = prompt_path.read_text()
    tag = args.tag or prompt_path.stem.replace("write-", "")

    cases = json.loads(cases_file.read_text())
    n = len(cases["positive"]) + len(cases["negative"])
    print(f"[{tag}] running {n} cases...", file=sys.stderr)
    t0 = time.time()
    report = evaluate(prompt, cases)
    dt = time.time() - t0
    print(f"[{tag}] done in {dt:.1f}s", file=sys.stderr)

    md = render(report, prompt, tag)
    out_path = prompt_path.parent / f"report-write-{tag}.md"
    try:
        out_path.write_text(md)
    except OSError:
        out_path = Path.cwd() / f"report-write-{tag}.md"
        out_path.write_text(md)
    print(out_path)

    m = report["metrics"]
    print(f"[{tag}] recall={m['recall']:.0%} neg_acc={m['neg_acc']:.0%} precision={m['precision']:.0%}", file=sys.stderr)
    return 0 if (m["recall"] == 1.0 and m["neg_acc"] == 1.0) else 1


if __name__ == "__main__":
    sys.exit(main())
