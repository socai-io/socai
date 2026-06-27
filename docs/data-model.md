# socai data model

This document describes the persisted data that backs agent runs and the desktop
app task history. Keep this current when changing run artifacts, task state, or
history replay.

## Guiding rule

`~/.socai/runs/...` (or the root configured by `runs.dir` / `SOCAI_RUNS_DIR`)
is the source of truth for task results.

The desktop app may keep an index in `~/.socai/app/tasks.json`, but it should not
persist duplicate task result payloads such as final answers or event timelines.
The app can hydrate those fields from the run directory when serving the UI.

## Locations

| Data | Default path | Override | Owner |
| --- | --- | --- | --- |
| Core run artifacts | `~/.socai/runs/<timestamp>_<task-slug>/` | `SOCAI_RUNS_DIR`, else `socai config set runs.dir <path>` | `socai-core` |
| Desktop task index | `~/.socai/app/tasks.json` | `SOCAI_HOME` (`$SOCAI_HOME/app/tasks.json`) | Tauri app |

Do not add a second persistent task-result store under `~/.socai/app/`. If the
UI needs result data after restart, derive it from the task's `run_dir`.

## Identifiers

### `task_id`

Created by the desktop app task registry before the agent starts.

Example:

```txt
task-1790000000000-1
```

Used for desktop UI state: task list rows, selected task, cancellation, and
mapping an app task to a core run.

### `run_id`

Created inside `socai-core` when the agent loop starts.

Example:

```txt
20260518-142233-123456
```

Used by the core agent run and persisted in run artifacts / task index once the
run exists.

### `run_dir`

Created before starting the core run and passed into `socai-core`.

Example:

```txt
~/.socai/runs/20260518_142233_find_xhs_coffee_notes
```

This directory is the source of truth for final answer and timeline replay.

`task_id` and `run_id` are intentionally different. `task_id` is an app/workflow
identifier; `run_id` is a core agent-run identifier.

## Core run directory

Each agent run writes files under `run_dir`.

| Path | Purpose | Notes |
| --- | --- | --- |
| `report.md` | Final answer shown by the desktop app | Source of truth for completed task output. May include appended artifact markdown. |
| `timeline.jsonl` | Canonical typed task timeline | Source of truth for live and historical desktop timeline rows for new runs. |
| `reasoning_log.jsonl` | Append-only debug/event log | Debug log and legacy fallback source for historical timeline replay. |
| `conversation.json` | Final system prompt and message history snapshot | Useful for debugging/model replay. |
| `agent_log.json` | Run summary | Includes task, model, turns, token counts, paths to run files. |
| `tool_results/<turn>_<seq>_<tool>.json` | Full tool result bodies | `reasoning_log.jsonl` only stores compact summaries. |
| `run_state/events.jsonl` | Compact run-state timeline | Fallback source for historical timeline replay. |
| `run_state/working_memory.md` | Rendered working memory | Used by the agent context/memory path. |
| `run_state/artifacts.json` | Artifact registry | Screenshots/media/other artifacts saved by tools. |
| `run_state/evidence.json` | Extracted evidence records | Populated from entity-like tool artifacts. |
| `run_state/plan.json` | Current plan state | Updated by plan tools when used. |

### `reasoning_log.jsonl`

One JSON object per line. Current important event types include:

- `task_start`
- `llm_response`
- `reasoning`
- `tool_call_start`
- `tool_result`
- `api_error`
- `turn_end`
- `task_end`

For new runs, the desktop app replays timeline rows from `timeline.jsonl` first.
If `timeline.jsonl` is absent or empty, the app falls back to this file and then
to `run_state/events.jsonl` for legacy runs.

## Desktop task index: `tasks.json`

`tasks.json` is an app index, not a task result store. It is an array of task
snapshots with fields like:

```json
{
  "task_id": "task-1790000000000-1",
  "task": "find popular xhs coffee notes",
  "model": "claude-sonnet-4-5-20250929",
  "status": "completed",
  "created_at": 1790000000000,
  "started_at": 1790000000200,
  "finished_at": 1790000060000,
  "run_id": "20260518-142233-123456",
  "run_dir": "/Users/alice/.socai/runs/20260518_142233_find_popular_xhs_coffee_notes",
  "target_id": null,
  "error": null,
  "turns": 4,
  "input_tokens": 12345,
  "output_tokens": 2345
}
```

Rules:

- `final_text` is not persisted in `tasks.json`.
- If old `tasks.json` files contain `final_text`, the app ignores it on load and
  omits it the next time the index is written.
- API responses may still include `final_text`, but it is hydrated from
  `<run_dir>/report.md`.
- `error` is status metadata for failed/interrupted tasks; it is not a final
  answer.
- Queued/running tasks are marked `interrupted` on app startup because their
  in-process runner and browser tab no longer exist.

## Desktop timeline state

New task timeline rows are appended to `<run_dir>/timeline.jsonl` before they are
emitted to the webview. The Tauri `agent_task:event` stream is a live delivery
mechanism for already-persisted rows; frontend memory is only a view cache.

Timeline rows use a typed discriminated shape with `kind` at the top level. For
example:

```json
{
  "task_id": "task-1790000000000-1",
  "kind": "tool_call",
  "id": "toolu_123",
  "turn": 1,
  "sequence_in_turn": 1,
  "name": "search",
  "label": "searched xiaohongshu",
  "args": { "query": "..." },
  "repeat_count": 1,
  "text": "search({\"query\":\"...\"})",
  "sequence": 4,
  "created_at": 1790000005000
}
```

Tool results can additionally carry normalized inline entities:

```json
{
  "task_id": "task-1790000000000-1",
  "kind": "tool_result",
  "id": "toolu_123",
  "name": "search",
  "label": "searched xiaohongshu",
  "ok": true,
  "duration_ms": 1400,
  "entities": [{ "type": "xhs_note_card_grid", "data": [] }],
  "text": "search (1400ms): ...",
  "sequence": 5,
  "created_at": 1790000006500
}
```

After restart or webview reload, the app does not read a separate app event
cache. Instead, `agent_task_events(task_id)` reads rows from the task's
`run_dir`:

1. `<run_dir>/timeline.jsonl`
2. legacy fallback: `<run_dir>/reasoning_log.jsonl` plus `tool_results/*.json`
3. legacy fallback: `<run_dir>/run_state/events.jsonl`
4. terminal status metadata from `tasks.json` when applicable

This keeps timeline and final answer data rooted in the run directory.

## Realtime event stream vs `timeline.jsonl` / `reasoning_log.jsonl`

For new runs, `timeline.jsonl` is the canonical source for both realtime and
historical timeline rows. The live UI receives rows only after they have been
written to that file.

The live UI receives two classes of events:

1. App lifecycle/status events emitted by Tauri: `queued`, `running`, `tab`,
   `completed`, `failed`, `cancelled`, `interrupted`.
2. Core agent events emitted by `socai-core`: `started`, `turn`, `assistant`,
   `reasoning`, `tool_call`, `tool_result`, `tool_error`, `api_error`, `done`.

`reasoning_log.jsonl` remains a persistent core debug log and legacy replay
fallback. It is not an exact byte-for-byte copy of what the user sees in
realtime.

| UI row | New-run source | Legacy fallback source | Replay behavior |
| --- | --- | --- | --- |
| `queued` | `timeline.jsonl` | Not logged | Replayed for new runs. |
| `running` | `timeline.jsonl` | Not logged | Replayed for new runs. |
| `tab` | `timeline.jsonl` | Not logged | Replayed for new runs. |
| `started` | `timeline.jsonl` | `task_start` | Replayed. |
| `turn` | `timeline.jsonl` | inferred from `turn` fields | Replayed/reconstructed. |
| `assistant` | `timeline.jsonl` | `llm_response.text` | Replayed. |
| `reasoning` | `timeline.jsonl` | `reasoning` when present | Replayed for new runs. |
| `tool_call` | `timeline.jsonl` | `tool_call_start` | Replayed. |
| `tool_result` / `tool_error` | `timeline.jsonl` | `tool_result` + `tool_results/*.json` | Replayed with normalized entities when possible. |
| `api_error` | `timeline.jsonl` | `api_error` | Replayed. |
| `done` | `timeline.jsonl` | `task_end` | Replayed. |
| terminal app status | `timeline.jsonl` | `tasks.json` status metadata | Replayed/reconstructed. |

New runs persist the typed desktop timeline directly in `timeline.jsonl` under
`run_dir`. `tasks.json` remains an index and must not grow a duplicate timeline
cache.
