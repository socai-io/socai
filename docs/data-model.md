# socai persisted execution model

Socai stores execution data in three ownership layers. The rule is simple:
persist a fact once, in the lowest layer that owns it. Parent layers keep
references instead of copying child records.

## L1: conversation session

```text
~/.socai/sessions/<session-id>/
└── session.json
```

`session.json` is the ordered conversation index. It stores the selected model,
timestamps, and one entry per user interaction:

- user text;
- L2 run directory;
- interaction status;
- an assistant fallback only if the run produced no `report.md`.

The session id comes from the directory name. The final answer is read from the
referenced run's `report.md`; the effective system prompt is in that run's
first LLM request. Neither is copied into `session.json`.

The TUI can append multiple interactions to one session. Desktop currently
creates one session per task, but uses the same format so it can add follow-up
interactions later.

The session root is `SOCAI_SESSIONS_DIR`, then `SOCAI_HOME/sessions`, then
`~/.socai/sessions`.

## L2: one agent execution

```text
~/.socai/runs/<timestamp>_<task>/
├── run.json
├── report.md
├── llm/
│   ├── 001.request.json
│   ├── 001.response.json
│   └── ...
└── tools/
    ├── turn-001-call-01-search/
    └── ...
```

`run.json` owns only run-level facts:

- run id and optional parent session id;
- task and model;
- status, start time, and total duration;
- step count and aggregate usage: logical/uncached input tokens, output tokens,
  cache reads, cache writes, reasoning tokens when reported, and estimated cost
  when the selected model has catalog pricing;
- an error only for failed/interrupted runs.

`report.md` is the single durable copy of the final user-facing answer.

Each `llm/NNN.request.json` is the actual JSON body sent after context
preparation and provider-specific translation, excluding authentication
headers. Its shape therefore follows the active API (Anthropic Messages,
OpenAI-compatible Chat Completions, or Responses).

The matching response contains text, exposed reasoning, tool calls, stop
reason, normalized `usage`, the provider's original `usage` object, request
duration, and completion time. Normalized usage separates ordinary input,
cache-read input, cache-creation input, output, and reasoning output where the
provider exposes it. Estimated cost includes its currency, per-component
breakdown, rates, and pricing source; it is omitted when no catalog price is
known or the credential is subscription-based rather than metered API billing.
An unsuccessful request contains its duration, completion time, and error.
Authentication headers, credentials, and raw response bytes are not recorded.

The LLM files are also the canonical ordered execution trace. Socai does not
write a parallel event log or span log: LLM duration lives on the response,
tool duration lives on the tool manifest, and total duration lives on
`run.json`.

The run root is `SOCAI_RUNS_DIR`, then `socai config get runs.dir`, then
`~/.socai/runs`.

## L3: one tool invocation

An agent tool call is nested under its L2 run:

```text
tools/turn-001-call-01-search/
├── tool.json
├── output.json          # only when the invocation returned a value
├── artifacts/          # only when the tool saves full domain data
├── site_media/         # only when media is downloaded
├── media_manifest.json # only when media needs a registry
├── stats/
│   └── ocr.json        # only when OCR ran
└── snapshots/          # only with debug snapshots enabled
```

The directory name makes the LLM turn, call sequence within that turn, and tool
name visible. `tool.json` owns the tool name, effective input after
tool-specific defaults/forced options, lifecycle status, start time, duration,
and an error when present. The parent LLM response retains the input originally
requested by the model; when defaults are involved these two values are
intentionally different.

`output.json` is the raw `ToolResultBlock[]` returned to the agent. It is not
wrapped with tool names, ids, timestamps, durations, or another `output` field.

Artifacts contain full, untrimmed domain results when the model/CLI-facing
output is intentionally lean. Downloaded media, its manifest, tool-specific
statistics, and debug snapshots all stay inside the same tool directory.
There is no artifact index: a consumer can enumerate `artifacts/`, while the
tool output points to any artifact it expects a caller to use.

## Standalone CLI tool invocation

A site CLI command is already one tool invocation, so its run directory is
directly L3:

```text
~/.socai/runs/<timestamp>_<site>_<command>_<identifier>/
├── tool.json
├── output.json          # only when the invocation returned a value
├── artifacts/
├── site_media/
├── media_manifest.json
├── stats/
│   └── ocr.json
└── snapshots/
```

There is no parent LLM response, so standalone `tool.json` additionally owns
the public tool name and effective input. It includes `implementation` only when
the CLI command and internal tool names differ. `output.json` is the raw tool
value.

Optional directories and files are created lazily. A normal search without
media, OCR, or snapshots should not contain empty `stats/`, `site_media/`, or
`snapshots/` directories.

## Desktop task history

`~/.socai/app/tasks.json` is a lightweight UI index, not a fourth logging
layer. It stores task lookup/status fields plus run and session directories.
Run id, errors, answer text, turn count, and usage are hydrated from
`run.json` and `report.md`.

Live desktop events exist only in memory. After restart, the desktop rebuilds
its typed timeline from `run.json`, `llm/*.response.json`, and each referenced
tool's `tool.json` and `output.json`. It does not persist a duplicate
`timeline.jsonl`.

## Ownership table

| Fact | Single durable owner |
| --- | --- |
| Conversation order and user interactions | L1 `session.json` |
| Run task, model, status, totals | L2 `run.json` |
| Final answer | L2 `report.md` |
| Exact LLM request and response | L2 `llm/` pair |
| Agent tool input | L3 `tool.json` |
| Provider tool call id | Parent LLM response |
| Standalone tool input | L3 `tool.json` |
| Tool status, duration, error | L3 `tool.json` |
| Tool result | L3 `output.json` |
| Full domain artifact | L3 `artifacts/` |
| Downloaded media and registry | L3 `site_media/`, `media_manifest.json` |
| OCR details | L3 `stats/ocr.json` |
| Debug browser state | L3 `snapshots/` |

These local files currently have no external schema consumer or migration
reader, so they intentionally do not carry a `schema_version` field.
