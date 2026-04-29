# Tests

## Gate-prompt evaluation

`run_gate_eval.py` benchmarks a candidate retrieval-gate system prompt against a synthetic fixture vault. It scores precision and recall on retrieval decisions so prompt changes can be measured rather than guessed at.

### Run

```bash
# Baseline (current shipped prompt, kept in lockstep with hooks/scripts/user-prompt-submit.sh)
python3 tests/run_gate_eval.py tests/prompts/current.txt

# Smoke test (5 positives + 5 negatives + edges)
python3 tests/run_gate_eval.py tests/prompts/current.txt --limit 5
```

Exit 0 only when recall = 100% and negative accuracy = 100%. Reports land at `tests/prompts/report-<tag>.md`.

### Cost

Each full run is ~42 calls to `claude -p`. With a warm prompt cache (the system prompt is reused across calls in the same run), expect roughly **$0.05–$0.30** depending on which model `claude` defaults to and how cold the cache is. Don't run on every commit — invoke when iterating on the gate prompt.

### Iterating on the prompt

```bash
cp tests/prompts/current.txt tests/prompts/v2-stricter.txt
$EDITOR tests/prompts/v2-stricter.txt
python3 tests/run_gate_eval.py tests/prompts/v2-stricter.txt
# compare report-current.md vs report-v2-stricter.md
```

When a candidate beats current on every metric (or wins on some without regressing the rest), copy it to `current.txt` and apply the same change to `hooks/scripts/user-prompt-submit.sh` so the live gate matches.

### Layout

```
tests/
├── README.md
├── run_gate_eval.py         # harness
├── cases.json               # 16 positive + 22 negative + 4 edge
├── prompts/
│   └── current.txt          # mirrors the prompt in user-prompt-submit.sh
└── fixtures/
    └── vault/               # synthetic vault the gate sees
```

### Caveats

- **Fixture-bound.** Cases reference fixture-vault notes by exact path. Editing fixture notes likely breaks expectations — update `cases.json` in the same change.
- **Search calls aren't executed.** The harness scores the gate's *intent* to search (right `type`, right `path_prefix`); it doesn't run `_vault.py search` to verify the search would return useful notes.
- **Model-dependent.** Results vary slightly by model. Re-run baseline + candidate together when comparing — don't compare a baseline number from last week against today's candidate.
- **Not a unit test.** Real network + LLM calls. Costs money. Slow (~50–70 s per run).
