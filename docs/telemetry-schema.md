# socai telemetry schema

This document is the current schema, privacy, and configuration contract for
socai telemetry across both surfaces that emit events:

- the **CLI daemon** (`source: "cli_daemon"`) — one sanitized trace per
  top-level CLI tool command (introduced in PR #63), and
- the **desktop app** (`source: "desktop"`) — agent-task lifecycle and per-tool
  events for tasks the user runs in the Tauri app.

Both surfaces share the same `Telemetry` client in `core/src/telemetry/mod.rs`,
the same first-party proxy at `https://socai.io/v1/events`, and the same Axiom
dataset; the `source` field distinguishes them. The public clients never talk to
PostHog or Axiom directly.

## Transport and ownership

```text
socai CLI daemon
  -> first-party socai proxy: https://socai.io/v1/events
      -> Axiom dataset
```

- The CLI endpoint is fixed at `https://socai.io/v1/events`.
- The public CLI must not include an Axiom token, Axiom dataset secret, or user
  configurable telemetry endpoint.
- The Axiom token and dataset configuration live only in Vercel environment
  variables for the proxy.
- Telemetry send failures are best-effort and must not fail user commands.
- The daemon also writes a local JSONL debug buffer under
  `~/.socai/telemetry/events.jsonl`, or `$SOCAI_HOME/telemetry/events.jsonl`
  when `SOCAI_HOME` is set.

Source references:

- Shared telemetry client (enrichment, identity, endpoint, local JSONL):
  `core/src/telemetry/mod.rs`
- CLI command trace shape and safe result metrics: `cli/src/daemon.rs`
- Desktop agent-task instrumentation: `app/src-tauri/src/telemetry.rs`,
  `app/src-tauri/src/commands.rs`, `app/src-tauri/src/lib.rs`
- Proxy validation, value sanitization, and Axiom forwarding: `site/api/telemetry.js`

## User controls

Telemetry is enabled by default, and query text is included by default.

| Control | Effect |
| --- | --- |
| `SOCAI_TELEMETRY=off` | Disables telemetry for that CLI command request. |
| `SOCAI_TELEMETRY_QUERY_TEXT=off` | Keeps telemetry enabled but omits `query_text`. |

The off values accepted by the CLI are:

```text
0, false, off, disabled, no
```

These controls are evaluated by the short-lived CLI process and included in the
request to the long-running daemon, so they apply per command even when an
existing daemon process is reused.

## Trace model

Each successful or failed top-level daemon command emits one trace after the
command finishes. The command result can fail while telemetry still records the
attempt with `ok=false` and an error summary.

Supported command/tool mapping:

| CLI daemon command | `command` | `tool_name` |
| --- | --- | --- |
| `search` | `search` | `search` |
| `author` | `author` | `author_scan` |

Every event carries a top-level `event` field naming its type — the CLI tool
trace is `socai_tool_call`. The proxy validates that it starts with `socai_` and
forwards it to Axiom as the type discriminator. The emitting surface is carried
separately in `source`, so the same `event` value (for example `socai_tool_call`)
spans both the CLI daemon and the desktop app.

## Forwarded Axiom fields

The proxy forwards **every field the client sends** — there is no field
allowlist; it only sanitizes values. The fields below are what the clients
currently emit, so use this as the reference for what to expect in Axiom (not as
a filter the proxy enforces). Any new client field reaches Axiom automatically.

### Identity and correlation

| Field | Type | Description |
| --- | --- | --- |
| `event` | string | Event type, for example `socai_tool_call`, `socai_agent_task_start`, or `socai_agent_task_end`. The surface lives in `source`, not the event name. |
| `install_id` | string UUID | Stable anonymous install identity stored in `telemetry/identity.json`. |
| `session_id` | string UUID | One daemon/app process lifetime. |
| `request_id` | string | One CLI request/daemon command invocation. Treat as opaque. |
| `schema_version` | number | Telemetry schema version. Current value: `1`. |

### App, source, and device context

| Field | Type | Description |
| --- | --- | --- |
| `app` | string | Always `socai`. |
| `source` | string | Emitting surface: `cli_daemon` or `desktop`. |
| `app_version` | string | socai-core (workspace) version, shared by the CLI daemon and desktop. |
| `platform` | string | Rust target OS, such as `macos` or `linux`. |
| `os_version` | string | OS version, for example macOS product version or Linux `PRETTY_NAME`. |
| `os_kernel_version` | string | Kernel version when available. |
| `memory_total_mb` | number | Total device memory in MiB when available. |
| `cpu_count` | number | Available CPU parallelism when available. |
| `terminal_app` | string | Best-effort terminal/app detection, such as Terminal, Ghostty, WezTerm, kitty, VS Code, Codex-related parent process, or `$TERM`. CLI daemon only. |
| `parent_process` | string | Best-effort parent process command name on Unix. CLI daemon only. |

### Command, query, and explicit parameters

| Field | Type | Description |
| --- | --- | --- |
| `command` | string | Top-level daemon command name. |
| `tool_name` | string | Tool label used for usage analysis. |
| `site` | string | Current site integration, `xhs`. |
| `query_text_enabled` | boolean | Whether query text was included for this command. |
| `query_text` | string | Search query text when enabled. Omitted when redacted. |
| `query_len` | number | Query length in Unicode scalar values. Kept even when text is redacted. |
| `metadata` | object | Explicit optional CLI parameters only. Defaults are omitted. |

Current metadata keys:

| Metadata key | Type | Source CLI flag | Omitted when |
| --- | --- | --- | --- |
| `metadata.tab` | string | `search --tab <value>` | `--tab` is not passed or is empty. |
| `metadata.num_notes` | number | `search --num-notes <n>` | `--num-notes` is not passed. |
| `metadata.debug_snapshot` | boolean | `--debug-snapshot` | `--debug-snapshot` is not passed / false. |

### Duration, status, and safe result metrics

| Field | Type | Description |
| --- | --- | --- |
| `duration_ms` | number | Command runtime in milliseconds. |
| `ok` | boolean | Whether the command returned successfully. |
| `error` | string | First-line error summary when `ok=false`. |
| `result_ok` | boolean | Safe `data.ok` result flag when present. |
| `cards_count` | number | Count of top-level `cards` result entries when present. |
| `search_cards_count` | number | Count of `search.cards` entries when present. |
| `selected_cards_count` | number | Count of selected cards when present. |
| `notes_count` | number | Count of note result entries when present. |
| `notes_skipped_count` | number | Count of notes marked skipped when present. |
| `has_run_dir` | boolean | Whether the command returned a run directory. |
| `proxy_version` | number | Added by the proxy. Current value: `1`. |

## Desktop events

The desktop app emits agent-task lifecycle events rather than a single
per-command trace. Each event carries the shared identity and context fields
above with `source: "desktop"`; terminal/parent-process fields are omitted
because a GUI has no meaningful terminal. Setup/config actions (API-key save,
model pick, Codex login, app open) are not tracked on their own — the provider
and model in use are captured on `socai_agent_task_start`.

| Event | Emitted when | Event-specific fields |
| --- | --- | --- |
| `socai_browser_connect` | User connects Chrome | — |
| `socai_agent_task_start` | A task begins running | `task_id`, `provider`, `model`, `task_len`, `task_text` |
| `socai_agent_task_end` | A task reaches a terminal state | `task_id`, `run_id`, `provider`, `model`, `outcome`, `turns`, `input_tokens`, `output_tokens`, `duration_ms`, `error` |
| `socai_tool_call` | Each tool call completes | `task_id`, `run_id`, `tool_name`, `turn`, `sequence`, `duration_ms`, `ok`, `error`, `query_text`, `query_len`, `metadata`, `note_id_present` |

Desktop field semantics:

| Field | Type | Description |
| --- | --- | --- |
| `task_id` | string | Stable desktop task identifier (`task-<ms>-<seq>`). Primary correlation key. |
| `run_id` | string | Core agent run id, attached once the run starts. |
| `provider` | string | LLM provider requested for the task. |
| `model` | string | Model id requested for the task. |
| `outcome` | string | Terminal state: `completed`, `failed`, `cancelled`, or `interrupted`. |
| `turns` | number | Agent loop turns when known. |
| `input_tokens` / `output_tokens` | number | Token usage for the run when known. |
| `task_len` | number | Agent prompt length in Unicode scalar values. |
| `task_text` | string | Full agent prompt. Always sent on desktop; see privacy boundaries. |
| `turn` / `sequence` | number | Position of a tool call within the run. |

`socai_tool_call` mirrors the CLI tool trace's argument summary: the tool's
`query` argument is lifted to `query_text` + `query_len`, a `note_id` argument
collapses to a `note_id_present` boolean (the raw id is not sent), and any other
scalar arguments go under `metadata` — with the `tab_label` arg renamed to `tab`
and empty strings dropped, matching the CLI. The tool's **output** (note bodies,
comments, scraped content) is never included — only the arguments.

Unlike the CLI's `query_text`, **the desktop has no opt-out for `task_text`**: it
is sent whenever desktop telemetry is enabled. `SOCAI_TELEMETRY=off` is the only
switch and disables the entire desktop pipeline. The proxy caps `task_text` at
8,000 characters (other strings stay capped at 2,000).

## Value-level handling

There is **no field allowlist** — every key the client sends is forwarded. The
proxy only sanitizes values:

- Non-scalar values (objects / arrays) other than `metadata` are dropped.
- Control characters are stripped from strings; strings are trimmed and truncated.
- `metadata` is coerced to a shallow primitive object (see limits below).
- `daemon_session_id` → `session_id` and `distinct_id` → `install_id` aliases are
  applied; a nested `properties` object is flattened up to the top level.

Axiom also has native time columns (`_time`, `_sysTime`) that it manages itself.
The client removes `created_at_ms` before sending, so it normally won't appear —
but because the proxy no longer filters fields, **anything a client sends now
reaches Axiom**, which is why the privacy boundaries below are enforced
client-side.

## Local JSONL caveat

The local JSONL buffer is a debug/replay aid, not the forwarded Axiom schema. It
may contain local-only fields such as:

- `created_at_ms`
- `properties.created_at_ms`
- `properties.note_id_present`

The CLI/desktop client strips the local millisecond timestamp before sending to
the proxy; the proxy forwards every remaining field, sanitizing values only.

## Privacy boundaries

These boundaries are enforced **entirely by the clients** — the proxy no longer
filters fields, so anything a client sends reaches Axiom. The clients must never
send:

- note body text
- comments
- image data, screenshots, or media contents
- browser cookies or session storage
- API keys, bearer tokens, Axiom tokens, or other secrets
- raw tool output bodies
- raw note ids or note-id presence flags in forwarded Axiom rows
- desktop agent results or model output: `report.md` / `final_text`, assistant or
  reasoning text, and raw tool arguments/results

Approved content-bearing telemetry is limited to:

- the CLI search `query_text` — included by default, omit with
  `SOCAI_TELEMETRY_QUERY_TEXT=off`; and
- the desktop agent `task_text` (the prompt the user submits) — always sent when
  desktop telemetry is enabled, with no per-field opt-out. Only
  `SOCAI_TELEMETRY=off` suppresses it.

Desktop tool telemetry is limited to tool name, timing, success, and a truncated
error string — never tool arguments or output bodies.

## Sanitization and limits

Proxy behavior in `site/api/telemetry.js`:

- Accepts only JSON `POST` requests.
- Enforces a maximum request body size of 128 KiB.
- Accepts at most 100 events/traces per request envelope.
- Requires each event's `event` name to start with `socai_` (the routing gate);
  `event` is forwarded as the type discriminator.
- Forwards every other field the client sends — there is no field allowlist.
- Drops non-scalar values (objects / arrays) other than `metadata`.
- Removes ASCII control characters from strings, trims whitespace, and truncates
  strings longer than 2,000 characters with an ellipsis. `task_text` uses a higher
  cap of 8,000 characters.
- Accepts `metadata` as a shallow object only.
- Limits `metadata` to 20 entries.
- Allows metadata keys up to 80 characters matching `[A-Za-z0-9_.-]+`.
- Allows only primitive metadata values: string, finite number, boolean, or null.
- Applies an in-memory rate limit keyed by install id when available, otherwise
  by client IP.

CLI behavior in `cli/src/daemon.rs`:

- Error summaries are first-line strings capped to 240 characters before proxy
  sanitization.
- Safe result metrics are counts/booleans only, not raw XHS content.

## Example: normal `search` trace

A user runs:

```bash
socai xhs search "运营爆款思路" --num-notes 12 --tab latest
```

Representative Axiom row after proxy sanitization:

```json
{
  "event": "socai_tool_call",
  "install_id": "11111111-1111-4111-8111-111111111111",
  "session_id": "22222222-2222-4222-8222-222222222222",
  "request_id": "12345-1780616790123",
  "schema_version": 1,
  "app": "socai",
  "source": "cli_daemon",
  "app_version": "0.1.0",
  "platform": "macos",
  "os_version": "15.5",
  "os_kernel_version": "24.5.0",
  "memory_total_mb": 65536,
  "cpu_count": 14,
  "terminal_app": "Ghostty",
  "parent_process": "zsh",
  "command": "search",
  "tool_name": "search",
  "site": "xhs",
  "query_text_enabled": true,
  "query_text": "运营爆款思路",
  "query_len": 6,
  "metadata": {
    "num_notes": 12,
    "tab": "latest"
  },
  "duration_ms": 42130,
  "ok": true,
  "result_ok": true,
  "search_cards_count": 20,
  "selected_cards_count": 12,
  "notes_count": 12,
  "notes_skipped_count": 1,
  "has_run_dir": true,
  "proxy_version": 1
}
```

Axiom will also show native `_time` and `_sysTime` columns for the row.

## Example: query-redacted `search` trace

A user runs:

```bash
SOCAI_TELEMETRY_QUERY_TEXT=off socai xhs search "运营爆款思路" --num-notes 12
```

Representative Axiom row:

```json
{
  "event": "socai_tool_call",
  "install_id": "11111111-1111-4111-8111-111111111111",
  "session_id": "22222222-2222-4222-8222-222222222222",
  "request_id": "12345-1780616790456",
  "schema_version": 1,
  "app": "socai",
  "source": "cli_daemon",
  "app_version": "0.1.0",
  "platform": "macos",
  "command": "search",
  "tool_name": "search",
  "site": "xhs",
  "query_text_enabled": false,
  "query_len": 6,
  "metadata": {
    "num_notes": 12
  },
  "duration_ms": 42130,
  "ok": true,
  "notes_count": 12,
  "proxy_version": 1
}
```

`query_text` is omitted. `query_len` remains available for aggregate product
analysis without storing the query string.

## Versioning notes

- `schema_version=1` covers the one-trace-per-tool-command schema described in
  this document.
- Additive fields flow through automatically (no allowlist); document them here
  so Axiom consumers know to expect them.
- Removing or renaming fields should update this document and any dashboard or
  release-smoke-test queries that depend on the old names.
