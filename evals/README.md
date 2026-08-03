# evals

Experiment harnesses for socai's agent behavior. Nothing here runs in CI yet;
these are lab tools for iterating on prompts/tools before shipping them.

## probe/ — single-step decision probe

**Question it answers:** after a default-ranked `search` result, does the agent
perform the verification hop — an `author_scan` of the official account —
before answering a question that has an authoritative owner — e.g.
"bloc1 攀岩馆上一次换线是什么时候"?

**Why this design:** search ranking is relevance-based, not recency-based. In
the recorded runs (2026-07-30) the official account's *stale* January 换线
notice ranked #1 while the fresh 07-26 notice ranked #5; an answer straight
from the sample is correct only by luck. The probe replays the **exact
production step-2 request** captured in the run dir (`llm/002.request.json`) —
real system prompt, real tool schemas, real tool-result serialization — swaps
in a prompt variant, and samples the model's next action N times. It isolates
the one decision that matters at ~¥0.25/trial with no browser, no XHS contact,
and no anti-bot risk.

**Scenarios** (built by `build_scenarios.py` from local `~/.socai/runs/`
recordings; regenerate after knowledge.md or toolset changes — the builder
re-splices the current knowledge.md and patches the recorded tools array for
known post-recording tool edits; new tool edits need a patch rule in
`build_scenarios.py` or a re-recorded run):

- `bloc1_replay` — the result set as recorded (stale official #1, fresh
  official #5). Both a verification hop and a fresh direct answer count as
  good behavior.
- `bloc1_trap` — the fresh official notice removed: the realistic world where
  ranking sampled only the stale notice. A direct answer here is the failure
  mode this whole effort targets; passing requires the hop.
- `col_replay` — the lucky case (fresh official notice at #1). Measures
  over-triggering cost: v1 should not turn easy cases into extra hops that
  add latency for nothing (a hop is *acceptable* here, just not required).

**Variants:**

- `v0` — current prod prompt (knowledge.md as in the repo now).
- `v1` — replaces knowledge.md's "for deep, complex topic research only …
  skip this extra step for routine searches" paragraph with the
  official-sources policy in `variants/v1_official_sources.md`.

Once the official-sources policy lands in knowledge.md itself (2026-08-01, this
branch), a regenerated `v0` already contains it and `run_probe.py --variants v1`
exits loudly because the old paragraph no longer exists — that's the drift
guard working. From then on, `v0` is the new baseline and `v1` applies only to
archived pre-policy fixtures; its variant file stays as the verbatim record of
the tested wording. Add `v2`… files and a matching replacement rule for the
next iteration.

**Run:**

```bash
python3 probe/build_scenarios.py
python3 probe/run_probe.py --scenarios bloc1_replay,bloc1_trap,col_replay --variants v0 -n 8
```

Qwen API key is read from `~/.socai/auth.json`; raw per-trial responses land in
`probe/results/<stamp>/` with a `summary.json`. Scenario fixtures and results
are gitignored (they embed public XHS note content and short-lived xsec tokens
from local recordings).

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
