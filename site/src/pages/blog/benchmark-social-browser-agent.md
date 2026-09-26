---
layout: ../../layouts/BlogPost.astro
lang: en
title: "How to benchmark a social browser agent"
description: "A useful social browser benchmark counts failed runs, measures the complete research loop, groups identical environments, and reports evidence coverage beside p50 and p95 time."
date: 2026-09-25
dateLabel: September 25, 2026
readingTime: 6 min read
alternates:
  - hreflang: zh
    href: /blog/zh/benchmark-social-browser-agent
faq:
  - q: "What should a social browser agent benchmark measure?"
    a: "Measure end-to-end time, decision time, browser-operation time, media time, evidence counts, and the final outcome. Keep failed, interrupted, empty, and access-gated attempts in the dataset."
  - q: "Why report p50 and p95 instead of one fastest run?"
    a: "The median describes a typical run, while p95 exposes slow or unreliable behavior that a best-case demo hides. Both require repeated runs under the same task and environment."
  - q: "Does Jev Social publish a benchmark result?"
    a: "Not yet. Version 0.1.8 publishes the fixed tasks, privacy-safe row schema, collector, and summary gate. Live timing claims wait for reviewed run groups that satisfy the protocol."
---

A browser-agent demo usually shows the run that worked. That is useful for seeing the product, but it is a weak performance claim.

Social sites change while the agent is using them. A login gate can appear. Search can return nothing. A video can load while its comments do not. The same task can require a different number of browser operations on the next attempt.

If a benchmark drops those runs, it measures luck rather than the system.

We added a reproducible benchmark workflow to [Jev Social v0.1.8](https://github.com/socai-io/jev-social/releases/tag/v0.1.8). The code is public now; live benchmark results are not. This is how the protocol is designed and why the distinction matters.

## One fast run is not the result

The Jev Social repository contains several recorded observations:

- one Instagram research loop completed in 63.969 seconds and retained four source-linked records;
- one TikTok CLI search returned five public result URLs in 7.570 seconds;
- one TikTok detail operation downloaded an MP4 and poster in 38.263 seconds, while comments remained unavailable.

Those numbers describe [specific recorded runs](https://github.com/socai-io/jev-social#recorded-evidence). They do not establish typical speed, tail latency, or reliability. The tasks and boundaries differ, so combining them into an average would be misleading.

A benchmark starts only when the task, software versions, browser setup, region label, and cache condition are fixed before repetition.

## Measure the complete research loop

Model latency is only one part of social research. A user waits for the whole job: choose an operation, run it in the browser, collect evidence, and produce a usable result.

Jev Social records these fields separately:

| Measurement | What it answers |
| --- | --- |
| End-to-end time | How long did the initialized research attempt take? |
| Jev decision time | How much time was spent choosing routes and actions? |
| socai/browser time | How much time was spent navigating and reading the live platform? |
| Media-operation time | How long did operations requesting TikTok media take? |
| Records and comments | How much reviewable evidence came back? |
| Outcome and stop reason | Did the run succeed, remain partial, fail, or hit an access gate? |

The component timers are not added blindly. Media work can overlap the browser operation that requested it, while Jev decisions and browser operations are sequential. The schema validates these timing relationships before a row can enter a summary.

## Failures belong in the dataset

An honest benchmark keeps the attempts that are easiest to hide:

- empty search results;
- login, challenge, and rate-limit gates;
- missing browser or CLI capabilities;
- decision, network, CLI, or report failures;
- step-limit stops;
- user or process interruption.

After the runner validates its task, metadata, local configuration, and isolated state, every attempt emits one privacy-safe row. A runtime failure becomes a failed or partial row instead of disappearing.

Initialization errors are different: if the task or declared metadata is invalid, no research attempt started, so no row is written. That distinction prevents a malformed command from being counted as platform performance.

## Compare only identical environments

The summary generator groups runs by the complete comparison boundary:

- fixed task ID and platform;
- Jev Social version and commit;
- immutable Jev model ID;
- socai version and commit;
- result limit and step budget;
- browser-profile mode;
- non-identifying region label;
- cold or warm condition.

Change any one of those values and the run belongs to another group. This prevents a new application commit, a different model, or a warm browser session from silently improving an older result.

Each schema-v2 group needs at least ten distinct runs before the tool marks it `READY`. The report then shows p50 and p95 for total time, Jev time, browser time, media time, and evidence counts, alongside success, partial, failed, and failure-rate totals.

Ten runs are a publication floor, not a claim of statistical certainty. They are enough to stop a single polished demo from masquerading as a benchmark; stronger comparisons should collect more.

## Keep the rows useful without leaking the browser

Benchmark data should be shareable without copying a signed-in browser session into a public file.

The Jev Social row schema rejects goals, evidence text, source URLs, local paths, browser endpoints, account identifiers, cookies, tokens, and credentials. It keeps opaque run IDs, timings, counts, pinned versions, outcomes, and stable environment labels.

That means a reviewer can check whether the comparison is complete and reproducible without receiving the research content or local machine details.

## What v0.1.8 publishes—and what it does not

The release includes:

- fixed Instagram, TikTok, and LinkedIn tasks;
- a collector that retains initialized attempts;
- a strict privacy-safe schema;
- exact-environment grouping;
- deterministic p50/p95 summaries;
- a ten-run publication gate.

It does **not** include a live benchmark dataset or a headline speedup. The [benchmark documentation](https://github.com/socai-io/jev-social/blob/v0.1.8/benchmark/README.md) explains the commands, timing definitions, row contract, and remaining review gate. [Issue #24](https://github.com/socai-io/jev-social/issues/24) stays open until reviewed live groups exist.

You can inspect the workflow locally without running a social task:

```bash
git clone --branch v0.1.8 --depth 1 https://github.com/socai-io/jev-social.git
cd jev-social
npm install
npm run benchmark:run -- --help
npm run benchmark:summarize -- --help
```

If you care about browser-agent performance, review the protocol before the eventual chart. If the boundary is wrong, a precise number will still answer the wrong question.

[Read the source](https://github.com/socai-io/jev-social) · [Review the benchmark contract](https://github.com/socai-io/jev-social/blob/v0.1.8/benchmark/README.md) · [Star Jev Social](https://github.com/socai-io/jev-social)
