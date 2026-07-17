# socai telemetry maintainer runbook

This is development/maintainer documentation for operating socai CLI telemetry.
It intentionally lives outside the README: the README stays focused on what
users need to run socai.

The final implementation sends one sanitized trace per top-level CLI tool
command through the first-party endpoint at `https://socai.io/v1/events`.
For the exact field contract, see [`../telemetry-schema.md`](../telemetry-schema.md).

## Product behavior summary

socai uses telemetry to understand whether CLI commands work reliably, how long
they take, which tools are used, and what result sizes look like. This helps us
prioritize fixes for search, note extraction, and topic scans.

Telemetry is enabled by default. Search query text is included by default because
it is the main signal for understanding user intent and result quality.

Each supported daemon command emits one trace:

- `search`
- `author`

The trace includes safe operational context such as command name, tool name,
duration, success/failure, result counts, app version, platform, OS details,
approximate device capacity, terminal app, and explicitly provided optional CLI
parameters under `metadata`.

Over the events pipeline, socai does not send note bodies, comments, images,
browser cookies, raw tool output bodies, or Axiom credentials; the free-text
fields it does send (task prompt, query text) are scrubbed client-side for
secret-shaped values (api keys, tokens) before capture. Run
traces (desktop only) are the deliberate exception: they carry conversation
content and note summaries by default — see the desktop section below and
[`../telemetry-schema.md`](../telemetry-schema.md) → Run traces.

### Desktop app

The desktop app (`source: "desktop"`) emits agent-task lifecycle events —
`socai_agent_task_start` / `_end`, one `socai_tool_call` per tool invocation,
plus `socai_browser_connect` when the user connects Chrome. Setup/config actions
(API-key save, model pick, Codex login, app open) are not tracked; the provider
and model in use are captured on `socai_agent_task_start`. Unlike the CLI, the
desktop sends the full agent prompt as
`task_text` with **no per-field opt-out**; `SOCAI_TELEMETRY=off` disables the
whole desktop pipeline. Desktop **events** never carry agent results
(`report.md` / `final_text`), assistant/reasoning text, or raw tool
arguments/output. Desktop **run traces** do: each task also uploads one OTLP
trace to `https://socai.io/v1/traces` (dataset `socai-traces-prod`) carrying
the conversation — per-step message deltas, full model output including
reasoning content, the system prompt, and note title/caption/stat
summaries — under client-side caps, secret scrubbing, and a whole-payload
size gate. The controls are separate: `SOCAI_TELEMETRY_CHAT_TEXT=off` strips
conversation content and note summaries; query text (`socai.query_text` on
tool spans, and the `query` argument inside chat tool calls) follows
`SOCAI_TELEMETRY_QUERY_TEXT=off`; the task prompt (`socai.task_text`) is only
suppressed by `SOCAI_TELEMETRY=off`, which disables trace upload entirely.
Field-level contract: [`../telemetry-schema.md`](../telemetry-schema.md) →
Run traces. Desktop
identity and the local buffer live under `~/.socai/app/telemetry/`
(`$SOCAI_HOME/app/telemetry/` when set). Desktop events route to the same
`socai-cli-prod` dataset; filter by `source == "desktop"`.

Delivery is best-effort: `capture()` is fire-and-forget over a bounded in-process
queue, so under a heavy burst of `socai_tool_call` events a few may be
dropped without backpressure. Lifecycle start/end events are very unlikely to be
lost given the single-concurrent-task limit. Treat tool-call counts as
near-complete, not exact.

## User controls

These environment variables are the user-facing controls. They are documented
here and in [`../telemetry-schema.md`](../telemetry-schema.md) — the README
deliberately stays product-focused and does not cover telemetry.

Disable telemetry for a single command:

```bash
SOCAI_TELEMETRY=off socai xhs search "运营爆款思路"
```

Redact query text while keeping the rest of the telemetry trace:

```bash
SOCAI_TELEMETRY_QUERY_TEXT=off socai xhs search "运营爆款思路"
```

For agent runs, the query gate removes the dedicated query attributes and
redacts tool-call arguments, but tool results can still echo the query into
chat content — when the query must not leave the machine at all, set **both**
`SOCAI_TELEMETRY_QUERY_TEXT=off` and `SOCAI_TELEMETRY_CHAT_TEXT=off`.

Keep telemetry but omit conversation content and note summaries from run
traces. Run traces come from **agent runs** — plain CLI tool commands like
`socai xhs search` emit events only, so this control doesn't apply to them.
For the TUI (writes `trace.json` locally, no upload):

```bash
SOCAI_TELEMETRY_CHAT_TEXT=off socai
```

For the desktop app — the only surface that uploads run traces — the variable
must reach the app process itself. A plain `VAR=... open -a socai` does NOT
work (LaunchServices drops the client environment); quit socai first, then
launch with `open --env`:

```bash
open --env SOCAI_TELEMETRY_CHAT_TEXT=off -a socai
```

Accepted off values are:

```text
0
false
off
disabled
no
```

These controls are evaluated by the CLI request, so they also work when reusing
an existing daemon process.

## Local telemetry buffer

The daemon writes a local JSONL buffer for debugging and replay:

```text
~/.socai/telemetry/events.jsonl
```

If `SOCAI_HOME` is set, the path is:

```text
$SOCAI_HOME/telemetry/events.jsonl
```

The desktop app writes its own buffer under the app data dir:

```text
~/.socai/app/telemetry/events.jsonl
```

Example inspection:

```bash
tail -n 5 ~/.socai/telemetry/events.jsonl
tail -n 5 ~/.socai/app/telemetry/events.jsonl
```

The local buffer is a debugging aid. It can contain a local creation timestamp
(`created_at_ms`) that the client removes before sending to the proxy. The proxy
itself no longer filters fields — it forwards everything the client sends,
sanitizing values only.

## Upgrade note: restart old daemons

A previously running daemon keeps using the code from the old installed binary.
After upgrading socai, stop the old daemon before validating telemetry behavior:

```bash
socai stop
```

The next CLI command will start a fresh daemon from the new binary.

## Maintainer architecture

```text
socai CLI daemon  ┐  (source: cli_daemon)
socai desktop app ┘  (source: desktop)
  -> local JSONL buffer (under each surface's data dir)
  -> https://socai.io/v1/events
  -> Vercel serverless proxy
  -> Axiom dataset (socai-cli-prod)
```

Important files:

- Shared telemetry client: `core/src/telemetry/mod.rs`
- CLI daemon instrumentation: `cli/src/daemon.rs`
- Desktop instrumentation: `app/src-tauri/src/telemetry.rs`,
  `app/src-tauri/src/commands.rs`, `app/src-tauri/src/lib.rs`
- Vercel proxy: `site/api/telemetry.js`
- Vercel rewrite/runtime config: `site/vercel.json`

The public CLI must never embed an Axiom token. The CLI sends unauthenticated
telemetry to the first-party socai endpoint, and the server-side proxy adds the
Axiom authorization from Vercel environment variables.

## Vercel configuration

Production project:

- Vercel team/scope: `socai-d83824c8`
- Vercel project: `socai-site`
- Production domain: `https://socai.io`
- Telemetry route: `https://socai.io/v1/events`

Server-side environment variable names:

- `AXIOM_TOKEN`
- `AXIOM_DATASET`
- `AXIOM_URL`
- `AXIOM_ORG_ID`

Do not put environment variable values in the repo, in docs, in PR comments, or
in public build logs.

Deployment details for the site project live in
[`../website-deployment.md`](../website-deployment.md).

## Axiom datasets

Current datasets:

- production: `socai-cli-prod`
- development/testing: `socai-cli-dev`

Older rows in the production dataset may have created columns that the current
proxy no longer forwards, such as `event`, `arch`, or custom timestamp fields.
Axiom can still show those fields as `null` on new rows because the dataset has
historical schema state.

## Production smoke test

Use this only when validating the proxy/deploy path. It sends a synthetic trace
to the production dataset through `https://socai.io/v1/events`.

```bash
request_id="runbook-smoke-$(date +%s)"

curl -sS -X POST https://socai.io/v1/events \
  -H 'Content-Type: application/json' \
  --data "{\"events\":[{\"event\":\"socai_runbook_smoke_test\",\"install_id\":\"00000000-0000-4000-8000-000000000061\",\"session_id\":\"00000000-0000-4000-8000-000000000062\",\"request_id\":\"${request_id}\",\"source\":\"runbook_smoke_test\",\"command\":\"search\",\"tool_name\":\"search\",\"query_text_enabled\":false,\"metadata\":{\"num_notes\":1},\"duration_ms\":1,\"ok\":true}]}"
```

Expected response:

```json
{"ok":true,"accepted":1}
```

If you have Axiom CLI access, verify ingestion:

```bash
axiom query "['socai-cli-prod'] | where request_id == '${request_id}' | limit 1" \
  --start-time=-15m \
  --format=json \
  --no-spinner
```

The resulting row should include `event` (`socai_runbook_smoke_test`),
`request_id`, `command`, `tool_name`, `ok`, and `metadata.num_notes`. It should
not include non-null custom `arch`, `created_at_ms`, `client_created_at_ms`, or
`received_at_ms` values.

## CLI smoke checks

When validating a release candidate or local build, restart the daemon first:

```bash
socai stop || true
```

Then run one command that should emit one trace:

```bash
socai xhs search "运营爆款思路" --num-notes 1
```

Validate query redaction:

```bash
SOCAI_TELEMETRY_QUERY_TEXT=off socai xhs search "运营爆款思路" --num-notes 1
```

Validate full telemetry disable:

```bash
SOCAI_TELEMETRY=off socai xhs search "运营爆款思路" --num-notes 1
```

Use Axiom or the local JSONL buffer to confirm the expected behavior.

## Troubleshooting missing events

1. Stop the old daemon and rerun the command:

   ```bash
   socai stop || true
   ```

2. Confirm the CLI request did not disable telemetry with `SOCAI_TELEMETRY=off`.
3. Check the local JSONL buffer. If the local trace is missing, inspect daemon
   logs and command errors first.
4. Confirm the proxy responds:

   ```bash
   curl -i -X OPTIONS https://socai.io/v1/events
   ```

5. Confirm Vercel project env vars are present for production and that the
   deployment includes `site/api/telemetry.js` and `site/vercel.json`.
6. Check Vercel function logs for `telemetry forward failed`.
7. Check Axiom query time range, dataset selection, and field filters. Remember
   that old schema columns can appear as `null` on new rows.
8. If the endpoint works but Axiom has no row, verify the server-side Axiom token
   and dataset configuration in Vercel without exposing the values.

## Security and privacy rules

- Never commit Axiom token values.
- Never commit local `.socai` telemetry files.
- Never add a public CLI telemetry endpoint override.
- Keep the Axiom token server-side in Vercel only.
- Use synthetic IDs and no real query text for manual production smoke tests.
- Treat query text as user data; use `SOCAI_TELEMETRY_QUERY_TEXT=off` in demos or
  tests where the query should not leave the machine.
- Treat LLM chat content the same way; use `SOCAI_TELEMETRY_CHAT_TEXT=off` in
  demos or tests where conversation content should not leave the machine.
