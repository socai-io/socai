# evals

Benchmarks and experiment harnesses for socai's agent behavior. The decision
benchmark runs in CI (`.github/workflows/agent-benchmark.yml`) on changes that
can shift agent decisions.

## probe/ — decision benchmark (cassette smoke test)

**What it is:** a benchmark, not a comparison. Each case pairs a real user
search prompt with a **cassette** — the recorded `search` tool response from a
production run — and declares the expected next action(s) in its `expect`
block. The runner replays that frozen moment against the **current** system
prompt N times per case and gates each case on its expected-action pass rate
(default ≥75%), exiting non-zero on any failure. Run it after any knowledge.md
or tool-description change to catch decision-policy regressions — e.g. "bloc1
攀岩馆上一次换线是什么时候" must route to an `author_scan` of the official
account when the cassette lacks the fresh official notice.

**Why this design:** search ranking is relevance-based, not recency-based. In
the recorded runs (2026-07-30) the official account's *stale* January 换线
notice ranked #1 while the fresh 07-26 notice ranked #5; an answer straight
from the sample is correct only by luck. The probe replays the **exact
production step-2 request** as captured in the run dir (`llm/002.request.json`)
— real tool schemas, real tool-result serialization, with the current
knowledge.md spliced into the system prompt — and samples the model's next
action N times. It isolates the one decision that matters at ~¥0.25/trial with
no browser, no XHS contact, and no anti-bot risk.

**Layout:** a checked-in case (`probe/cases/<name>.json`) holds only what is
specific to the situation — the user prompt, the search call the agent issued,
and the cassette (recorded search result), plus its recorded date. Everything
identical across cases lives once in `probe/shared/` (system-prompt scaffold
with placeholders, tool schemas, request params), captured from a real
production request at import time. `build_scenarios.py` assembles runnable
scenarios by filling the scaffold with the repo's **current** knowledge.md —
regenerate after any prompt or toolset change (after a tools.rs change,
refresh `shared/` by re-importing a run recorded with the new binary):

- `bloc1_replay` — the result set as recorded (stale official #1, fresh
  official #5). Both a verification hop and a fresh direct answer count as
  good behavior.
- `bloc1_trap` — the fresh official notice removed: the realistic world where
  ranking sampled only the stale notice. A direct answer here is the failure
  mode this whole effort targets; passing requires the hop.

**Testing a candidate prompt wording:** edit `core/src/sites/xhs/knowledge.md`
on a branch, rebuild scenarios, rerun the benchmark — git is the variant
switch. (The original v0-vs-v1 experiment that produced this policy is
preserved in PR #244 and its report artifact.)

**Run:**

```bash
python3 probe/build_scenarios.py
python3 probe/run_probe.py            # full benchmark, n=8 per case, gated
```

`-n 3` for a quick smoke; `--dry-run` builds requests without API calls.

Qwen API key: `$DASHSCOPE_API_KEY` / `$QWEN_API_KEY` (CI) or
`~/.socai/auth.json` (local). Raw per-trial responses land in
`probe/results/<stamp>/` with a `summary.json`. Generated `scenarios/` and
`results/` are gitignored; the checked-in sources of truth are `probe/cases/`
and `probe/shared/` (home paths and xsec tokens scrubbed at import time —
note content itself is public XHS material).

**CI:** `.github/workflows/agent-benchmark.yml` runs the gated benchmark on
PRs touching `knowledge.md` / `tools.rs` / `evals/**` and on manual dispatch.
It needs the `DASHSCOPE_API_KEY` repo secret; without it (e.g. fork PRs) the
job skips with a warning. If GitHub runners can't reach the mainland
endpoint, set `DASHSCOPE_BASE_URL` to the intl endpoint with an intl key.

**Contributing a case:**

1. Reproduce the situation in the app/CLI so a run dir exists
   (`~/.socai/runs/<run>/turn-*/llm/002.request.json`).
2. `python3 probe/build_scenarios.py --import <turn-dir> --name <case>` —
   writes the sanitized `cases/<case>.json` and refreshes `shared/`.
3. Add a scenarios-dict entry in `build_scenarios.py` with `meta` (official
   author, date patterns) and an `expect` block (expected actions +
   `min_pass_rate`).

**Reading the table:** `scan✓` = author_scan with the correct official
author_id — the only action that counts as verification; `recency` = re-search
with `sort=最新`/`publish_time` (tracked, but not verification: the policy
keeps XHS's default relevance ranking and handles stale no-official-source
evidence with an explicit date caveat instead); `plain` = plain re-search
(spends a step without fixing the sampling bias); `direct` = answered from the
sample (`stale!` = with the stale date — the incident this guards against).

**Known limits:** single-step only — it scores the *decision*, not whether the
follow-up produces the right final answer (that needs the full-loop fixture
harness, future work); N=8 per cell is directional, not significant; scenarios
snapshot one recorded ranking each.
