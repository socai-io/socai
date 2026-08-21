# Model-visible evidence archive

socai archives the ToolResult values that were present on successful provider
request wires. This is a separate path from the bounded OTLP chat transcript:
the archive preserves what the agent could observe after normal context
management, without uploading raw tool data the agent never received.

The archive does not add an LLM call, change the provider request, or increase
input tokens. It post-processes the exact request artifacts already written to
`<run-dir>/llm/NNN.request.json` after a run terminates.

## Data flow

```text
tool output
  -> normal bound_content_for_history()
  -> provider request + llm/NNN.request.json
  -> provider-specific ToolResult extraction
  -> secret redaction
  -> SHA-256 content addressing and UTF-8-safe chunks
  -> telemetry/pending-evidence
  -> https://socai.io/v1/evidence
  -> socai-evidence-prod
```

Raw `tools/*/output.json` files remain local and are not evidence archive
sources. If context management omitted comments before a provider request, the
archive contains the same omission marker rather than restoring those comments
from raw output.

## Local files

Each terminal run gets a shareable, identity-free archive:

```text
<run-dir>/evidence/model-visible-v1.json
```

Uploads are staged before the telemetry worker returns:

```text
<socai-home>/telemetry/pending-evidence/<trace-id>-<root-span-id>.json
```

Pending files survive proxy or Axiom outages. The worker retries up to five
files every 30 seconds and removes a file only after all content/manifests and
the final commit receive successful proxy acknowledgements.

## Schema

Schema version: `socai.model-visible-evidence.v1`.

The dataset contains three record types:

| Record | Purpose |
| --- | --- |
| `evidence_chunk` | One UTF-8-safe piece of a canonical JSON ToolResult value. |
| `request_manifest` | Evidence IDs present in one logical provider request. |
| `turn_commit` | Terminal object/chunk counts and archive integrity hash. |

Content is addressed by:

```text
content_sha256 = SHA256(canonical_json(redacted_tool_result_value))
evidence_id = SHA256(evaluation_id + tool_call_id + content_sha256)
```

Repeated history across provider requests references the same evidence object.
If later context management changes a ToolResult value, its content hash and
evidence ID change, preserving the model-visible version for each request.

The client uploads all `evidence_chunk` and `request_manifest` records before
the single `turn_commit`. Duplicate records can occur after a lost HTTP
acknowledgement; readers must deduplicate by `(evidence_id, chunk_index)` and
reject conflicting values for the same stable key.

## Provider request formats

The extractor recognizes explicit wire shapes rather than recursively looking
for generic `content` fields:

| Protocol | Tool result shape |
| --- | --- |
| OpenAI-compatible Chat Completions | `messages[]` with `role=tool`. |
| OpenAI Responses | `input[]` with `type=function_call_output`. |
| Anthropic Messages | `messages[].content[]` with `type=tool_result`. |

An accepted request with an unsupported shape produces an `unsupported`
manifest and a `partial` turn commit. It never falls back to raw tool output.

## Privacy controls

The master `SOCAI_TELEMETRY=off` switch disables the telemetry object entirely.
Evidence content additionally follows both gates:

```bash
SOCAI_TELEMETRY_CHAT_TEXT=off
SOCAI_TELEMETRY_EVIDENCE=off
```

If either content gate is off, socai uploads a manifest-only `disabled` commit
and no ToolResult body. The local exact provider request artifact remains part
of the user's run record.

Before staging, the existing trace secret scrubber removes sensitive JSON
fields, API keys, JWTs, Bearer values, and URL query credentials including
`xsec_token`, `access_token`, and `api_key`. The proxy validates chunk hashes
and forwards content without trimming or rewriting it.

## Proxy deployment

The Vercel function is `site/api/evidence.js`, exposed as `/v1/evidence`.
Configure:

```text
AXIOM_EVIDENCE_TOKEN=<dataset-scoped ingest API token>
AXIOM_EVIDENCE_DATASET=socai-evidence-prod
AXIOM_EVIDENCE_URL=https://api.axiom.co
AXIOM_ORG_ID=<only when the organization requires it>
```

Create `socai-evidence-dev` and `socai-evidence-prod` before enabling uploads.
Use an API token limited to ingesting the evidence dataset. Axiom's ingest
endpoint does not accept personal access tokens; see the
[official ingest documentation](https://axiom.co/docs/restapi/ingest).

The proxy forwards validated arrays to:

```text
POST /v1/datasets/<dataset>/ingest
```

It limits request bodies to 1 MiB, batches to 32 records, individual chunks to
32 KiB, and each install to 240 requests/32 MiB per minute. Proxy errors return
non-2xx so clients retain pending files.

## Local proxy overrides

For local integration work:

```bash
SOCAI_EVIDENCE_ENDPOINT=http://localhost:3000/v1/evidence
SOCAI_TRACES_ENDPOINT=http://localhost:3000/v1/traces
```

The client sends records in batches below 512 KiB and sends the terminal commit
only after every preceding batch succeeds.

## Trace summary

The root trace span carries no evidence body. It only reports local build state:

```text
socai.evidence.schema_version
socai.evidence.local_status
socai.evidence.upload_status_at_trace_build
socai.evidence.accepted_request_count
socai.evidence.object_count
socai.evidence.total_bytes
```

`upload_status_at_trace_build=queued` does not prove Axiom completeness. A
reader must find and validate the evidence dataset's `turn_commit`, all request
manifests, every referenced chunk, and all SHA-256 values.

## Current integration boundary

This implementation produces and reliably uploads the socai-side archive. It
does not change evaluator. Until evaluator gains an evidence repository and
integrity verifier, existing evaluations continue to use the bounded trace
packet.
