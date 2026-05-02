#!/usr/bin/env python3
"""Retrieval-gate evaluation harness.

Runs a candidate gate system prompt against a fixture vault and the cases in
`cases.json`, scoring precision/recall on retrieval decisions. Costs real
Anthropic API calls (~$0.05–0.30 per run depending on the model claude-cli
defaults to and prompt-cache state).

Usage:
  python3 tests/run_gate_eval.py tests/prompts/current.txt
  python3 tests/run_gate_eval.py path/to/candidate.txt --tag v2
  python3 tests/run_gate_eval.py path/to/candidate.txt --limit 5  # smoke test

Output: writes `report-<tag>.md` next to the prompt file (or in cwd if not
writable). Prints a one-line summary to stderr.

Exit: 0 if recall=100% and neg_acc=100%, else 1.
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
PLUGIN_ROOT = TESTS_DIR.parent
FIXTURE_VAULT = TESTS_DIR / "fixtures" / "vault"
VAULT_PY = PLUGIN_ROOT / "scripts" / "_vault.py"
CASES_FILE = TESTS_DIR / "cases.json"
FIXTURE_PROJECT = "example-project"
PATH_CAP = 3
CLAUDE_BIN = os.environ.get("CLAUDE_BIN") or "claude"
PARALLELISM = int(os.environ.get("GATE_EVAL_PARALLELISM", "4"))


def parse_gate_output(raw: str) -> dict | None:
    """Extract first balanced JSON object — same logic as the live gate."""
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


def check_heredoc_safe(prompt: str) -> str | None:
    """Return an error message if the prompt would break the live hook's
    `$(cat <<PROMPT ... PROMPT)` heredoc, else None.

    The live `user-prompt-submit.sh` wraps the prompt in command substitution,
    which makes bash scan the body for matched quote pairs even though
    heredocs normally don't honor quotes. An odd number of single quotes
    (apostrophes) breaks the script with a syntax error far away from the
    actual offending line.
    """
    wrapper = f'X=$(cat <<PROMPT\n{prompt}\nPROMPT\n)\n'
    result = subprocess.run(["bash", "-n"], input=wrapper, capture_output=True, text=True)
    if result.returncode != 0:
        bad_lines = [
            f"  L{i + 1}: {ln}" for i, ln in enumerate(prompt.splitlines()) if "'" in ln
        ]
        hint = "\n".join(bad_lines) if bad_lines else "(no apostrophes found — different cause)"
        return (
            "prompt would break the live hook's heredoc:\n"
            f"  {result.stderr.strip()}\n"
            "lines containing apostrophes (likely culprit):\n"
            f"{hint}"
        )
    return None


def generate_overview(mode: str = "full") -> str:
    out = subprocess.check_output(
        ["python3", str(VAULT_PY), "--vault", str(FIXTURE_VAULT),
         "overview", "--project", FIXTURE_PROJECT, "--mode", mode],
        text=True,
    )
    if not out.strip():
        raise RuntimeError("fixture vault overview is empty")
    return out


def call_gate(system_prompt: str, message: str, timeout: int = 30) -> tuple[dict | None, str, float]:
    user_prompt = f"USER MESSAGE:\n{message}\n\nJSON only:"
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
        return parse_gate_output(result.stdout), result.stdout.strip()[:200], dt
    except subprocess.TimeoutExpired:
        return None, "[timeout]", time.time() - t0


def evaluate(prompt: str, overview: str, cases: dict, limit: int | None = None) -> dict:
    sp = f"{prompt.strip()}\n\n=== VAULT OVERVIEW ===\n{overview}"
    pos = cases["positive"][:limit] if limit else cases["positive"]
    neg = cases["negative"][:limit] if limit else cases["negative"]
    edge = cases["edge"]

    def run_one(case: dict) -> dict:
        out, raw, dt = call_gate(sp, case["msg"])
        return {"msg": case["msg"], "case": case, "out": out, "raw": raw, "dt": dt}

    results: dict[str, list] = {"positive": [], "negative": [], "edge": []}
    with ThreadPoolExecutor(max_workers=PARALLELISM) as ex:
        results["positive"] = [f.result() for f in [ex.submit(run_one, c) for c in pos]]
        results["negative"] = [f.result() for f in [ex.submit(run_one, c) for c in neg]]
        results["edge"] = [f.result() for f in [ex.submit(run_one, c) for c in edge]]

    m = {
        "pos_total": len(results["positive"]),
        "pos_hit": 0, "pos_partial": 0, "pos_miss": 0, "pos_search_ok": 0,
        "neg_total": len(results["negative"]),
        "neg_correct": 0, "neg_overinject": 0,
        "edge_total": len(results["edge"]),
        "edge_empty": 0, "edge_inject": 0,
    }

    for r in results["positive"]:
        out = r["out"] or {}
        read_paths = set(out.get("read") or [])
        searches = out.get("search") or []
        # cases.json keeps relative paths for portability; rebase to absolute
        # against the fixture vault so they match the overview's paths.
        expect = {str(FIXTURE_VAULT / p) for p in (r["case"].get("expect_any_of") or [])}
        expect_search = r["case"].get("expect_search")
        search_ok = r["case"].get("search_ok", False)

        hit = bool(expect & read_paths)
        if expect_search:
            for s in searches:
                if all(s.get(k) == v for k, v in expect_search.items()):
                    hit = True
                    break

        if hit:
            m["pos_hit"] += 1
            r["score"] = "HIT"
        elif read_paths or searches:
            if search_ok and searches:
                m["pos_search_ok"] += 1
                r["score"] = "SEARCH-OK"
            else:
                m["pos_partial"] += 1
                r["score"] = "WRONG-INJECT"
        else:
            m["pos_miss"] += 1
            r["score"] = "MISS"

    for r in results["negative"]:
        out = r["out"] or {}
        if out.get("read") or out.get("search"):
            m["neg_overinject"] += 1
            r["score"] = "OVER-INJECT"
        else:
            m["neg_correct"] += 1
            r["score"] = "OK"

    for r in results["edge"]:
        out = r["out"] or {}
        if out.get("read") or out.get("search"):
            m["edge_inject"] += 1
            r["score"] = "INJECT"
        else:
            m["edge_empty"] += 1
            r["score"] = "EMPTY"

    m["precision"] = m["pos_hit"] / max(1, m["pos_hit"] + m["pos_partial"] + m["neg_overinject"])
    m["recall"] = (m["pos_hit"] + m["pos_search_ok"]) / max(1, m["pos_total"])
    m["neg_acc"] = m["neg_correct"] / max(1, m["neg_total"])
    return {"metrics": m, "results": results}


def render(report: dict, prompt: str, tag: str) -> str:
    m = report["metrics"]
    out = [
        f"# Gate eval: {tag}\n",
        f"**Recall** (positives hit): {m['pos_hit'] + m['pos_search_ok']}/{m['pos_total']} = {m['recall']:.0%}",
        f"**Negative accuracy** (no over-inject): {m['neg_correct']}/{m['neg_total']} = {m['neg_acc']:.0%}",
        f"**Precision** (hits / all injections): {m['precision']:.0%}",
        f"**Edge** (inject/empty): {m['edge_inject']}/{m['edge_total']}\n",
        f"Counts: hit={m['pos_hit']} search-ok={m['pos_search_ok']} wrong-inject={m['pos_partial']} miss={m['pos_miss']} over-inject={m['neg_overinject']}\n",
        f"Prompt size: {len(prompt)} chars\n",
        "## Failures\n",
    ]
    fails = []
    for r in report["results"]["positive"]:
        if r["score"] in ("MISS", "WRONG-INJECT"):
            fails.append(f"- [{r['score']}] `{r['msg']}` → {r['out']}")
    for r in report["results"]["negative"]:
        if r["score"] == "OVER-INJECT":
            fails.append(f"- [{r['score']}] `{r['msg']}` → {r['out']}")
    out.extend(fails or ["_(none)_"])
    out.append("\n## All results\n")
    for cat in ("positive", "negative", "edge"):
        out.append(f"### {cat}")
        for r in report["results"][cat]:
            out.append(f"- [{r['score']}] `{r['msg']}` → {r['out']} ({r['dt']:.1f}s)")
        out.append("")
    return "\n".join(out)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("prompt_file", help="path to a candidate gate system prompt")
    ap.add_argument("--limit", type=int, default=None, help="run only N positive + N negative cases")
    ap.add_argument("--tag", default=None, help="report tag (defaults to prompt filename stem)")
    ap.add_argument("--overview-mode", choices=["full", "tools-and-general", "tools-only"],
                    default="full", help="vault overview mode passed to _vault.py (default: full)")
    args = ap.parse_args()

    if not VAULT_PY.is_file():
        print(f"error: _vault.py not found at {VAULT_PY}", file=sys.stderr)
        return 2
    if not FIXTURE_VAULT.is_dir():
        print(f"error: fixture vault not found at {FIXTURE_VAULT}", file=sys.stderr)
        return 2

    prompt_path = Path(args.prompt_file)
    if not prompt_path.is_file():
        print(f"error: prompt file not found: {prompt_path}", file=sys.stderr)
        return 2
    prompt = prompt_path.read_text()
    tag = args.tag or prompt_path.stem
    if args.overview_mode != "full" and not args.tag:
        tag = f"{tag}-{args.overview_mode}"

    err = check_heredoc_safe(prompt)
    if err:
        print(f"error: {err}", file=sys.stderr)
        return 2

    cases = json.loads(CASES_FILE.read_text())
    overview = generate_overview(args.overview_mode)

    n = len(cases["positive"]) + len(cases["negative"]) + len(cases["edge"])
    if args.limit:
        n = min(n, args.limit * 2 + len(cases["edge"]))
    print(f"[{tag}] running ~{n} cases against fixture vault...", file=sys.stderr)
    t0 = time.time()
    report = evaluate(prompt, overview, cases, args.limit)
    dt = time.time() - t0
    print(f"[{tag}] done in {dt:.1f}s", file=sys.stderr)

    md = render(report, prompt, tag)
    out_path = prompt_path.parent / f"report-{tag}.md"
    try:
        out_path.write_text(md)
    except OSError:
        out_path = Path.cwd() / f"report-{tag}.md"
        out_path.write_text(md)
    print(out_path)

    m = report["metrics"]
    print(f"[{tag}] recall={m['recall']:.0%} neg_acc={m['neg_acc']:.0%} precision={m['precision']:.0%}", file=sys.stderr)

    return 0 if (m["recall"] == 1.0 and m["neg_acc"] == 1.0) else 1


if __name__ == "__main__":
    sys.exit(main())
