# socai telemetry maintainer runbook

This is development/maintainer documentation for operating socai CLI telemetry.
It intentionally lives outside the README: the README should stay focused on
what CLI users need to run socai and control telemetry.

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

- `search_notes`
- `topic_scan`
- `extract_note`

The trace includes safe operational context such as command name, tool name,
duration, success/failure, result counts, app version, platform, OS details,
approximate device capacity, terminal app, and explicitly provided optional CLI
parameters under `metadata`.

socai does not intentionally send note bodies, comments, images, browser cookies,
API keys, raw tool output bodies, or Axiom credentials.

## User controls

The README should document only these user controls, not proxy/Axiom internals.

Disable telemetry for a single command:

```bash
SOCAI_TELEMETRY=off socai xhs topic_scan "运营爆款思路"
```

Redact query text while keeping the rest of the telemetry trace:

```bash
SOCAI_TELEMETRY_QUERY_TEXT=off socai xhs topic_scan "运营爆款思路"
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

Example inspection:

```bash
tail -n 5 ~/.socai/telemetry/events.jsonl
```

The local buffer is a debugging aid. It can contain internal fields, such as the
client-side validation event name and local creation timestamp, that the proxy
strips before forwarding to Axiom.

## Upgrade note: restart old daemons

A previously running daemon keeps using the code from the old installed binary.
After upgrading socai, stop the old daemon before validating telemetry behavior:

```bash
socai stop
```

The next CLI command will start a fresh daemon from the new binary.

## Maintainer architecture

```text
socai CLI daemon
  -> local JSONL buffer
  -> https://socai.io/v1/events
  -> Vercel serverless proxy
  -> Axiom dataset
```

Important files:

- CLI telemetry worker: `cli/src/tracking.rs`
- daemon instrumentation: `cli/src/daemon.rs`
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
  --data "{\"events\":[{\"event\":\"socai_runbook_smoke_test\",\"install_id\":\"00000000-0000-4000-8000-000000000061\",\"session_id\":\"00000000-0000-4000-8000-000000000062\",\"request_id\":\"${request_id}\",\"source\":\"runbook_smoke_test\",\"command\":\"topic_scan\",\"tool_name\":\"topic_scan\",\"query_text_enabled\":false,\"metadata\":{\"num_notes\":1},\"duration_ms\":1,\"ok\":true}]}"
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

The resulting row should include `request_id`, `command`, `tool_name`, `ok`, and
`metadata.num_notes`. It should not include non-null custom `event`, `arch`,
`created_at_ms`, `client_created_at_ms`, or `received_at_ms` values.

## CLI smoke checks

When validating a release candidate or local build, restart the daemon first:

```bash
socai stop || true
```

Then run one command that should emit one trace:

```bash
socai xhs topic_scan "运营爆款思路" --num-notes 1
```

Validate query redaction:

```bash
SOCAI_TELEMETRY_QUERY_TEXT=off socai xhs topic_scan "运营爆款思路" --num-notes 1
```

Validate full telemetry disable:

```bash
SOCAI_TELEMETRY=off socai xhs topic_scan "运营爆款思路" --num-notes 1
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
