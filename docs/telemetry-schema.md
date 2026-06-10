# CLI telemetry schema

This document is the current schema, privacy, and configuration contract for
socai CLI daemon telemetry. It describes the implementation merged in PR #63.

The finalized model is **one sanitized trace per top-level CLI tool command**.
Telemetry is not a stream of lifecycle events, and the public CLI does not talk
to PostHog or Axiom directly.

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

- CLI enrichment, identity, endpoint, and local JSONL:
  `cli/src/tracking.rs`
- Command trace shape and safe result metrics: `cli/src/daemon.rs`
- Proxy allowlist, sanitization, and Axiom forwarding: `site/api/telemetry.js`

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
| `search_notes` | `search_notes` | `search_notes` |
| `topic_scan` | `topic_scan` | `topic_scan` |
| `extract_note` | `extract_note` | `read_note` |

The internal client-to-proxy payload currently includes an internal event name
for proxy validation. The proxy strips that before forwarding to Axiom. Axiom
rows for the current schema should not contain a custom `event` value.

## Forwarded Axiom fields

The proxy forwards only allowlisted fields. Fields not listed here should be
assumed unavailable in Axiom.

### Identity and correlation

| Field | Type | Description |
| --- | --- | --- |
| `install_id` | string UUID | Stable anonymous install identity stored in `telemetry/identity.json`. |
| `session_id` | string UUID | One daemon process lifetime. |
| `request_id` | string | One CLI request/daemon command invocation. Treat as opaque. |
| `schema_version` | number | Telemetry schema version. Current value: `1`. |

### App, source, and device context

| Field | Type | Description |
| --- | --- | --- |
| `app` | string | Always `socai`. |
| `source` | string | Current source is `cli_daemon`. |
| `app_version` | string | CLI crate version. |
| `platform` | string | Rust target OS, such as `macos` or `linux`. |
| `os_version` | string | OS version, for example macOS product version or Linux `PRETTY_NAME`. |
| `os_kernel_version` | string | Kernel version when available. |
| `memory_total_mb` | number | Total device memory in MiB when available. |
| `cpu_count` | number | Available CPU parallelism when available. |
| `terminal_app` | string | Best-effort terminal/app detection, such as Terminal, Ghostty, WezTerm, kitty, VS Code, Codex-related parent process, or `$TERM`. |
| `parent_process` | string | Best-effort parent process command name on Unix. |

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
| `metadata.tab` | string | `topic_scan --tab <value>` | `--tab` is not passed or is empty. |
| `metadata.num_notes` | number | `topic_scan` / `search_notes` `--num-notes <n>` | `--num-notes` is not passed. |
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

## Fields intentionally not forwarded to Axiom

Current Axiom rows should not include these custom fields:

- `event`
- `arch`
- `created_at_ms`
- `client_created_at_ms`
- `received_at_ms`
- `daemon_session_id`
- `query_redacted`
- raw `tab_label` or raw top-level `num_notes` outside `metadata`
- `note_id_present`

Axiom still has native time columns:

- `_time`
- `_sysTime`

Those are Axiom-managed fields, not custom CLI telemetry fields. Historical rows
in the existing dataset created older columns, so Axiom may still display fields
such as `event`, `arch`, or `created_at_ms` as `null` on new rows. `null` in
those old columns does not mean the current proxy forwarded those values.

## Local JSONL caveat

The local JSONL buffer is a debug/replay aid, not the forwarded Axiom schema. It
may contain local-only fields such as:

- `event`
- `created_at_ms`
- `properties.created_at_ms`
- `properties.note_id_present`

The CLI strips the local millisecond timestamp before sending to the proxy, and
the proxy strips/ignores non-allowlisted fields before forwarding to Axiom.

## Privacy boundaries

The telemetry contract must never send:

- note body text
- comments
- image data, screenshots, or media contents
- browser cookies or session storage
- API keys, bearer tokens, Axiom tokens, or other secrets
- raw tool output bodies
- raw note ids or note-id presence flags in forwarded Axiom rows

Approved content-bearing telemetry is limited to query text, which is included by
default and can be omitted with `SOCAI_TELEMETRY_QUERY_TEXT=off`.

## Sanitization and limits

Proxy behavior in `site/api/telemetry.js`:

- Accepts only JSON `POST` requests.
- Enforces a maximum request body size of 128 KiB.
- Accepts at most 100 events/traces per request envelope.
- Requires the internal validation event name to start with `socai_`.
- Uses an allowlist for forwarded fields.
- Removes ASCII control characters from strings, trims whitespace, and truncates
  strings longer than 2,000 characters with an ellipsis.
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

## Example: normal `topic_scan` trace

A user runs:

```bash
socai xhs topic_scan "运营爆款思路" --num-notes 12 --tab latest
```

Representative Axiom row after proxy sanitization:

```json
{
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
  "command": "topic_scan",
  "tool_name": "topic_scan",
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

## Example: query-redacted `topic_scan` trace

A user runs:

```bash
SOCAI_TELEMETRY_QUERY_TEXT=off socai xhs topic_scan "运营爆款思路" --num-notes 12
```

Representative Axiom row:

```json
{
  "install_id": "11111111-1111-4111-8111-111111111111",
  "session_id": "22222222-2222-4222-8222-222222222222",
  "request_id": "12345-1780616790456",
  "schema_version": 1,
  "app": "socai",
  "source": "cli_daemon",
  "app_version": "0.1.0",
  "platform": "macos",
  "command": "topic_scan",
  "tool_name": "topic_scan",
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
- Additive fields may be introduced through the proxy allowlist and documented
  here.
- Removing or renaming fields should update this document and any dashboard or
  release-smoke-test queries that depend on the old names.
