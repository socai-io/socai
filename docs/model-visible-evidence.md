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
<socai-home>/telemetry/pending-evidence/<trace-id>-<root-span-id>.state
<socai-home>/telemetry/pending-evidence/dead/
```

Pending files survive proxy or Axiom outages. The `.state` sidecar checkpoints
the next batch and retry time. The worker retries eligible oldest files with
exponential backoff, honors numeric `Retry-After`, and removes an archive only
after all content/manifests and the final commit receive verified proxy
acknowledgements. Deterministically invalid local payloads and proxy 400/413/422
responses move to `dead/` with a content-free reason file instead of retrying
forever or being deleted.

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
the single `turn_commit`, batching at no more than 32 records and 512 KiB. Each
batch has a stable index and SHA-256; the client verifies the proxy echo and
checkpoints every accepted batch. Duplicate records can still occur after a lost HTTP
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

New runs record wire format and request outcome explicitly in
`llm/NNN.request.meta.json`. Legacy runs without the sidecar use the shape
detector and response parser as a compatibility fallback. An accepted request
with an unsupported shape produces an `unsupported` manifest and a `partial`
turn commit. It never falls back to raw tool output.

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
all sensitive JSON field names, including `xsec_token`, `access_token`,
`refresh_token`, `client_secret`, `password`, and `api_key`. Query-value
scanning accepts token-safe characters only, so commas, JSON escapes, brackets,
and neighboring fields remain intact. The proxy validates chunk and batch
hashes and forwards content without trimming or rewriting it.

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

It limits request bodies to 1 MiB, batches to 32 records, and individual chunks
to 32 KiB. It checks wire `Content-Length` before expensive validation and uses
both IP and install buckets. Because Vercel's in-memory buckets are
instance-local and install IDs are client-asserted, production must also enable
Vercel Firewall/WAF rate limits. Proxy errors return non-2xx so clients retain,
back off, or quarantine pending files according to the status class.

## Local proxy overrides

For local integration work:

```bash
SOCAI_EVIDENCE_ENDPOINT=http://localhost:3000/v1/evidence
SOCAI_TRACES_ENDPOINT=http://localhost:3000/v1/traces
```

The client sends at most 32 records per batch below 512 KiB and sends the
terminal commit only after every preceding batch succeeds. A restart resumes
from the checkpointed batch rather than batch zero.

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
