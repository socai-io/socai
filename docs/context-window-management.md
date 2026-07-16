# Context Window Management

This document describes how socai builds, bounds, and compacts the agent
context used by the CLI/TUI and desktop app. The shared implementation lives in
`core/src/agent/` and is used by every entrypoint.

## Terminology

- A **conversation turn** is one persisted user request and its final assistant
  report. A follow-up turn seeds the next agent run with two messages per prior
  turn: the user request and the assistant Markdown report.
- A **step** is one iteration of the in-run agent loop and normally corresponds
  to one LLM request and response.
- A **tool call** is one action requested by the model inside a step. One step
  may contain multiple tool calls.

When a step contains tool calls, socai appends one assistant message containing
all `tool_use` blocks, executes the tools sequentially in model-specified order,
and appends one user message containing all matching `tool_result` blocks. The
next step receives all of those results together. A tool-using step therefore
adds two full messages regardless of whether it contains one or several tool
calls.

The base system prompt tells the model to issue at most two tool calls in one
assistant step and to wait for their results before planning more work. This is
a model-visible scheduling policy, not a parallel executor or a hard runtime
truncation rule. The runtime remains sequential and does not silently discard
additional calls if a provider violates the instruction.

## Context layers

The request context has four relevant layers:

1. The system prompt, site playbook, current date, available tool names, and
   entrypoint-specific instructions.
2. Seed messages from earlier conversation turns.
3. Full assistant/tool-result messages from the current agent run.
4. A deterministic compacted-context message after the history crosses the
   sawtooth threshold.

Run artifacts are the durable evidence store. Context compaction changes only
what is replayed to the model; it does not rewrite or truncate the JSON, media,
LLM request/response, or tool-call records saved under the run directory.

## Tool-result bounds before history compaction

Every raw tool result is written to its tool-call directory before the result is
bounded for model history. The LLM-facing text has a 30,000-character ceiling.
If an oversized result is JSON, degradation happens in this order:

1. Cap each `ocr_text` value to 1,000 characters in total. For string arrays,
   keep complete leading entries until the budget is exhausted and mark the
   truncation.
2. Replace `top_comments` with an artifact pointer marker if the result is still
   too large.
3. Compact JSON objects, arrays, and string leaves while keeping note body text
   longer than generic strings.
4. Apply a final character truncation only as a last resort.

Xiaohongshu search and author-scan results are already artifact-first before
this generic limit runs. Their full artifacts retain per-image OCR. The lean
tool result exposes OCR from at most the first two cover-first images per note,
with each returned image text capped at 1,200 characters.

## Sawtooth message window

The default window uses two values:

- compact after the transcript grows beyond 20 full messages;
- retain the most recent 10 full messages verbatim.

For a fresh run with one initial user message and one tool-using step at a time,
the transcript grows as `1 + 2 * steps`. Ten completed tool-using steps produce
21 messages, so compaction runs immediately before the next model request.

At a compaction point, socai rewrites the in-memory transcript to:

1. the original first message;
2. one deterministic compacted-context message;
3. the most recent 10 full messages.

The recent tail then grows normally until the next threshold. Rewriting only at
these points creates a sawtooth window and keeps the request prefix stable
between compactions, which is friendlier to provider prompt caching than
regenerating a different summary before every request.

## Compacting earlier conversation turns

Prior turns are seeded as user text followed by an assistant Markdown report.
When those messages move into the compacted region, socai emits compact Markdown
for each recognized report containing:

- up to 500 characters of its associated user request, when available;
- the first 2,000 characters of the assistant report;
- every note citation found in the full report using the canonical
  `[title](note:NOTE_ID)` form;
- artifact links found in the full report when their targets point into known
  run artifact locations such as `artifacts/`, `tools/`, `snapshots/`,
  `site_media/`, or an absolute `.socai/runs/` path.

Evidence extraction scans the complete Markdown report, not only its
2,000-character excerpt. Earlier compacted Markdown is carried forward when the
next sawtooth compaction occurs.

Markdown without canonical note links cannot yield a reliable note ID or title.
Likewise, a run path that never appears as a Markdown link is not inferred from
free-form prose. The desktop/TUI conversation preamble separately lists earlier
run directories and `notes.json` evidence so the agent can use `read_file` when
it needs full cross-turn details.

## Compacting earlier structured tool results

Older `tool_result` text is parsed as JSON. When the result contains an
`artifact.path`, the compacted context retains:

- the artifact path;
- note IDs and titles from `notes` or `cards`, accepting either direct entities
  or `{ "entity": ... }` wrappers;
- an author ID and available profile name for author-scan results.

Body text, OCR, comments, timing details, engagement metadata, and other large
fields are omitted from this compact representation because the artifact path
is the durable lookup key. Duplicate entity entries are removed
deterministically.

## Source locations

- `core/src/agent/loop.rs`: message lifecycle, sequential tool execution, and
  compaction trigger points.
- `core/src/agent/memory.rs`: sawtooth rewrite and compact Markdown/JSON evidence
  extraction.
- `core/src/agent/compaction.rs`: per-tool-result character and JSON bounds.
- `core/src/agent/conversation.rs`: persisted turns and seed-message creation.
- `core/src/agent/system_prompt.rs`: model-visible two-tool-call policy.
- `core/src/sites/xhs/tools.rs`: artifact-first XHS result shaping and OCR lean
  output.
