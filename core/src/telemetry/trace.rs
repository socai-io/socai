//! OTLP-JSON trace assembly for one agent run.
//!
//! The agent loop records each LLM call and tool call as it completes;
//! `finish` closes the root span and writes an OTLP/HTTP
//! `ExportTraceServiceRequest` JSON value to `run_dir/trace.json`. If the run
//! future is dropped before `finish` — an entrypoint aborted the task, e.g.
//! desktop cancel — a drop guard writes the same file with
//! `socai.status=interrupted` and the spans recorded so far; the aborting
//! entrypoint then patches the precise status via [`mark_run_trace_status`]
//! (the trace analog of `mark_agent_run_status` for `run.json`). Entrypoints
//! upload the file fire-and-forget via
//! [`super::Telemetry::upload_run_trace`], which appends identity resource
//! attributes (source, install id) so the on-disk file stays shareable.
//!
//! Span attributes follow the OpenTelemetry GenAI semantic conventions
//! (`gen_ai.*`) — Axiom promotes those to first-class trace columns — with
//! socai-specific context under `socai.*`. Tool args go through
//! `summarize_tool_args` (query text gated by `SOCAI_TELEMETRY_QUERY_TEXT`)
//! and tool output through `summarize_tool_result` (counts plus bounded
//! unexpected-page OCR diagnostics).
//!
//! Chat content rides on the `chat` spans (gated by
//! `SOCAI_TELEMETRY_CHAT_TEXT`): `gen_ai.input.messages` carries only the
//! conversation messages *new since the previous LLM call* — every request
//! re-sends the whole history, so per-span deltas let one trace reconstruct
//! the conversation exactly once instead of O(n²) re-uploads — and
//! `gen_ai.output.messages` carries that call's full response (history keeps
//! a truncated copy; the trace keeps the real one). `gen_ai.system_instructions`
//! is emitted once per run and again when it changes. Image bytes, thinking
//! signatures, and encrypted reasoning items never leave the machine, and a
//! per-run byte budget keeps the upload inside the traces proxy's body cap.
//! Under the same gate and budget, `execute_tool` spans carry `socai.notes` —
//! compact id/title/caption/stats summaries of the notes a site tool returned.

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use super::tool_call::{summarize_tool_args, summarize_tool_result};
use super::{chat_text_enabled, query_text_enabled};
use crate::agent::llm::{
    Block, LLMResponse, Message, MessageContent, MessageRole, TokenUsage, ToolResultContent,
};
use crate::agent::r#loop::THINKING_TEXT_PREFIX;
use crate::agent::signature::md5_hex;

/// Safety net for pathological runs; a default run (30 steps) stays far below.
const MAX_CHILD_SPANS: usize = 400;
/// Matches the proxy's `task_text` cap on the events route.
const TASK_TEXT_MAX_CHARS: usize = 8000;
const ERROR_MAX_CHARS: usize = 240;
const VALUE_JSON_MAX_CHARS: usize = 500;
/// Per-part chat content cap; big tool-result dumps get the tail cut, the
/// conversation structure stays intact.
const CHAT_PART_MAX_CHARS: usize = 20_000;
/// Retry cap when a span's serialized chat content overshoots the attribute cap.
const CHAT_PART_TIGHT_MAX_CHARS: usize = 2_000;
/// Cap on one serialized chat attribute (Axiom ingests spans as single events).
const CHAT_ATTR_MAX_BYTES: usize = 150_000;
/// Cumulative chat-content budget per run upload. The traces proxy rejects
/// bodies over 512 KiB outright, so content must leave room for the span
/// envelope and tool-arg summaries.
const CHAT_BUDGET_BYTES: usize = 300_000;
/// Max note summaries carried on one `execute_tool` span (`socai.notes`).
const NOTES_MAX_PER_SPAN: usize = 25;
/// Caption (note body) cap inside a note summary.
const NOTE_CAPTION_MAX_CHARS: usize = 200;
/// Remaining note-summary field caps; with all fields bounded, 25 notes stay
/// far below the per-attribute byte cap even in CJK.
const NOTE_TITLE_MAX_CHARS: usize = 300;
const NOTE_ID_MAX_CHARS: usize = 100;
const NOTE_STAT_MAX_CHARS: usize = 50;
/// Ceiling for the assembled payload: the traces proxy rejects bodies over
/// 512 KiB, and the uploader appends identity resource attributes. The chat
/// budget bounds content, but span envelopes and non-content attributes ride
/// on top of it — `enforce_payload_cap` strips content from oldest spans
/// when the serialized whole would overshoot.
const PAYLOAD_MAX_BYTES: usize = 460_000;
/// What `enforce_payload_cap` may strip, in order: chat/note content first,
/// then the unbounded summarizer strings (query text, arg metadata). Every
/// attribute not listed here is small and bounded, so a payload with both
/// tiers stripped always fits under [`PAYLOAD_MAX_BYTES`] at
/// [`MAX_CHILD_SPANS`].
const STRIP_TIERS: [&[&str]; 2] = [
    &[
        "gen_ai.system_instructions",
        "gen_ai.input.messages",
        "gen_ai.output.messages",
        "socai.notes",
    ],
    &["socai.query_text", "socai.metadata"],
];

const SPAN_KIND_INTERNAL: u32 = 1;
const SPAN_KIND_CLIENT: u32 = 3;
const STATUS_CODE_ERROR: u32 = 2;

pub struct RunTraceBuilder {
    run_dir: PathBuf,
    trace_id: String,
    root_span_id: String,
    run_id: String,
    task_text: String,
    provider: String,
    model: String,
    session_id: Option<String>,
    /// Prior chat messages seeding this run — non-zero marks a conversation
    /// follow-up rather than a fresh task.
    seed_messages: usize,
    started_ns: u64,
    spans: Vec<Value>,
    dropped_spans: u64,
    /// Running figures for the drop guard; `finish` overwrites them with the
    /// loop's authoritative totals.
    steps_seen: u32,
    usage: TokenUsage,
    /// Remaining chat-content bytes for this run's upload; once spent, later
    /// spans carry `socai.chat_text_dropped` instead of content.
    chat_bytes_left: usize,
    /// Hash of the last uploaded system prompt, so `gen_ai.system_instructions`
    /// is emitted once and then only when it changes — **per run**. Follow-up
    /// runs of a conversation share the trace id but each uploads its own
    /// trace.json, and deliberately re-emit the (unchanged) system prompt so
    /// every turn's upload is self-contained: readable even if another turn's
    /// upload is lost or arrives out of order.
    last_system_hash: String,
    finalized: bool,
}

impl RunTraceBuilder {
    pub fn new(
        run_dir: &Path,
        run_id: &str,
        task: &str,
        provider: &str,
        model: &str,
        session_id: Option<&str>,
        seed_messages: usize,
    ) -> Self {
        // One conversation = one trace: every run of a conversation derives
        // the same trace id from its session id, so follow-up turns join the
        // first turn's trace as additional root-level spans. Runs without a
        // session get their own single-run trace.
        let trace_id = match session_id.map(str::trim).filter(|s| !s.is_empty()) {
            Some(session) => md5_hex(format!("socai-conversation:{session}").as_bytes()),
            None => Uuid::new_v4().simple().to_string(),
        };
        Self {
            run_dir: run_dir.to_path_buf(),
            trace_id,
            root_span_id: new_span_id(),
            run_id: run_id.to_string(),
            task_text: truncate_chars(&redact_secrets(task), TASK_TEXT_MAX_CHARS),
            provider: provider.to_string(),
            model: model.to_string(),
            session_id: session_id.map(ToOwned::to_owned),
            seed_messages,
            started_ns: now_ns(),
            spans: Vec::new(),
            dropped_spans: 0,
            steps_seen: 0,
            usage: TokenUsage::default(),
            chat_bytes_left: CHAT_BUDGET_BYTES,
            last_system_hash: String::new(),
            finalized: false,
        }
    }

    /// `new_messages` is the transcript delta: conversation messages appended
    /// since the previous LLM call (compaction rewrites of older context are
    /// visible only in the local run log, not here).
    pub fn record_llm(
        &mut self,
        step: u32,
        duration_ms: u64,
        system: &str,
        new_messages: &[Message],
        response: &LLMResponse,
    ) {
        self.steps_seen = self.steps_seen.max(step);
        self.usage += &response.usage;
        let mut attrs = vec![
            attr_str("gen_ai.operation.name", "chat"),
            attr_str("gen_ai.provider.name", &self.provider),
            attr_str("gen_ai.request.model", &self.model),
            attr_int("socai.step", step as i64),
            attr_str("socai.stop_reason", stop_reason_str(response)),
            attr_int("socai.tool_calls", response.tool_calls.len() as i64),
        ];
        push_usage_attrs(&mut attrs, &response.usage);
        self.push_chat_attrs(&mut attrs, system, new_messages, Some(response));
        self.push_span(
            format!("chat {}", self.model),
            SPAN_KIND_CLIENT,
            duration_ms,
            attrs,
            None,
        );
    }

    pub fn record_llm_error(
        &mut self,
        step: u32,
        duration_ms: u64,
        system: &str,
        new_messages: &[Message],
        error: &str,
    ) {
        self.steps_seen = self.steps_seen.max(step);
        let mut attrs = vec![
            attr_str("gen_ai.operation.name", "chat"),
            attr_str("gen_ai.provider.name", &self.provider),
            attr_str("gen_ai.request.model", &self.model),
            attr_int("socai.step", step as i64),
        ];
        self.push_chat_attrs(&mut attrs, system, new_messages, None);
        self.push_span(
            format!("chat {}", self.model),
            SPAN_KIND_CLIENT,
            duration_ms,
            attrs,
            Some(error),
        );
    }

    /// Attach chat content to a `chat` span: system prompt (when changed),
    /// input delta, and full response, each budgeted against the run's
    /// remaining chat bytes. Stops at the first attribute that would overrun
    /// the budget and marks the span `socai.chat_text_dropped` instead.
    fn push_chat_attrs(
        &mut self,
        attrs: &mut Vec<Value>,
        system: &str,
        new_messages: &[Message],
        response: Option<&LLMResponse>,
    ) {
        if !chat_text_enabled() {
            return;
        }
        // Query text has its own gate: when it's off, tool-call arguments in
        // chat content redact their `query` field too, matching the events
        // pipeline. Tool RESULTS may still echo the query — full removal of
        // conversation content is the chat gate's job.
        let include_query = query_text_enabled();
        attrs.push(attr_int("socai.new_messages", new_messages.len() as i64));
        let system_hash = md5_hex(system.as_bytes());
        if self.last_system_hash != system_hash {
            self.last_system_hash = system_hash;
            let system = redact_secrets(system);
            let rendered = capped_chat_json(
                |cap| json!([{ "type": "text", "content": truncate_chars(&system, cap) }]),
            );
            if !self.push_budgeted(attrs, "gen_ai.system_instructions", rendered) {
                return;
            }
        }
        let input = capped_chat_json(|cap| input_messages_json(new_messages, cap, include_query));
        if !self.push_budgeted(attrs, "gen_ai.input.messages", input) {
            return;
        }
        if let Some(response) = response {
            let output = capped_chat_json(|cap| output_messages_json(response, cap, include_query));
            self.push_budgeted(attrs, "gen_ai.output.messages", output);
        }
    }

    /// Push one chat-content attribute if it fits the remaining budget;
    /// otherwise zero the budget and mark the span. Returns whether it fit.
    /// The budget is charged at the attribute's wire cost — the JSON-escaped
    /// form the upload serializes (quotes/backslashes/newlines in the content
    /// escape a second time there), not the raw string length, so admitted
    /// content can't push the POST past the proxy's body cap.
    fn push_budgeted(&mut self, attrs: &mut Vec<Value>, key: &str, rendered: String) -> bool {
        let wire_len = serde_json::to_string(&rendered)
            .map(|escaped| escaped.len())
            .unwrap_or(rendered.len());
        if self.chat_bytes_left < wire_len {
            self.chat_bytes_left = 0;
            attrs.push(attr_bool("socai.chat_text_dropped", true));
            return false;
        }
        self.chat_bytes_left -= wire_len;
        attrs.push(attr_str(key, &rendered));
        true
    }

    pub fn record_tool(
        &mut self,
        step: u32,
        sequence: u32,
        name: &str,
        duration_ms: u64,
        input: &Value,
        output: &Value,
        error: Option<&str>,
    ) {
        self.steps_seen = self.steps_seen.max(step);
        let mut attrs = vec![
            attr_str("gen_ai.operation.name", "execute_tool"),
            attr_str("gen_ai.tool.name", name),
            attr_int("socai.step", step as i64),
            attr_int("socai.sequence", sequence as i64),
            attr_bool("socai.ok", error.is_none()),
        ];
        extend_prefixed(&mut attrs, summarize_tool_args(input, query_text_enabled()));
        extend_prefixed(&mut attrs, summarize_tool_result(output));
        if chat_text_enabled() {
            let notes = note_summaries(output);
            if !notes.is_empty() {
                let mut rendered = Value::Array(notes).to_string();
                // Field caps bound this far below CHAT_ATTR_MAX_BYTES; the
                // marker is a belt against unforeseen shapes.
                if rendered.len() > CHAT_ATTR_MAX_BYTES {
                    rendered = "[notes omitted: over attribute cap]".to_string();
                }
                self.push_budgeted(&mut attrs, "socai.notes", rendered);
            }
        }
        self.push_span(
            format!("execute_tool {name}"),
            SPAN_KIND_INTERNAL,
            duration_ms,
            attrs,
            error,
        );
    }

    /// Close the root span and write the OTLP `ExportTraceServiceRequest` to
    /// `run_dir/trace.json` (best-effort, like every other telemetry write).
    pub fn finish(&mut self, status: &str, steps: u32, usage: &TokenUsage, error: Option<&str>) {
        self.finalized = true;
        let payload = self.build_payload(status, steps, usage, error);
        self.write_trace_file(&payload);
    }

    fn build_payload(
        &mut self,
        status: &str,
        steps: u32,
        usage: &TokenUsage,
        error: Option<&str>,
    ) -> Value {
        let end_ns = now_ns();
        let mut attrs = vec![
            attr_str("gen_ai.operation.name", "invoke_agent"),
            attr_str("gen_ai.agent.name", "socai"),
            attr_str("gen_ai.provider.name", &self.provider),
            attr_str("gen_ai.request.model", &self.model),
            attr_str("socai.run_id", &self.run_id),
            attr_str("socai.task_text", &self.task_text),
            attr_str("socai.status", status),
            attr_int("socai.steps", steps as i64),
        ];
        push_usage_attrs(&mut attrs, usage);
        if let Some(session_id) = &self.session_id {
            // Desktop session ids embed the first task's slug (conversation
            // dir name), so scrub the uploaded attribute; the trace id was
            // already derived from the original, keeping follow-up turns
            // joined.
            attrs.push(attr_str("socai.session_id", &redact_secrets(session_id)));
        }
        attrs.push(attr_bool("socai.follow_up", self.seed_messages > 0));
        attrs.push(attr_int("socai.seed_messages", self.seed_messages as i64));
        if self.dropped_spans > 0 {
            attrs.push(attr_int("socai.spans_dropped", self.dropped_spans as i64));
        }

        // Keep the span name low-cardinality (OTel GenAI convention:
        // `invoke_agent {agent}`): Axiom's operations panel groups by span
        // name, so putting the task in the name mints one "operation" per
        // task. The task lives in socai.task_text; turns within a
        // conversation trace are told apart by that attribute.
        let mut root = json!({
            "traceId": self.trace_id,
            "spanId": self.root_span_id,
            "name": "invoke_agent socai",
            "kind": SPAN_KIND_INTERNAL,
            "startTimeUnixNano": self.started_ns.to_string(),
            "endTimeUnixNano": end_ns.to_string(),
            "attributes": attrs,
        });
        if let Some(error) = error {
            root["status"] = json!({
                "code": STATUS_CODE_ERROR,
                "message": truncate_chars(error, ERROR_MAX_CHARS),
            });
        }

        let mut spans = std::mem::take(&mut self.spans);
        spans.insert(0, root);

        let mut payload = json!({
            "resourceSpans": [{
                "resource": {
                    "attributes": [
                        attr_str("service.name", "socai"),
                        attr_str("service.version", env!("CARGO_PKG_VERSION")),
                        attr_str("os.type", std::env::consts::OS),
                    ],
                },
                "scopeSpans": [{
                    "scope": {
                        "name": "socai-core",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "spans": spans,
                }],
            }],
        });
        enforce_payload_cap(&mut payload);
        payload
    }

    /// Append a completed child span. Start time is derived from the duration
    /// so call sites only need the `Instant`-measured `duration_ms` they
    /// already have for the run recorder.
    fn push_span(
        &mut self,
        name: String,
        kind: u32,
        duration_ms: u64,
        attributes: Vec<Value>,
        error: Option<&str>,
    ) {
        if self.spans.len() >= MAX_CHILD_SPANS {
            self.dropped_spans += 1;
            return;
        }
        let end_ns = now_ns();
        let start_ns = end_ns.saturating_sub(duration_ms.saturating_mul(1_000_000));
        let mut span = json!({
            "traceId": self.trace_id,
            "spanId": new_span_id(),
            "parentSpanId": self.root_span_id,
            "name": name,
            "kind": kind,
            "startTimeUnixNano": start_ns.to_string(),
            "endTimeUnixNano": end_ns.to_string(),
            "attributes": attributes,
        });
        if let Some(error) = error {
            span["status"] = json!({
                "code": STATUS_CODE_ERROR,
                "message": truncate_chars(error, ERROR_MAX_CHARS),
            });
        }
        self.spans.push(span);
    }

    fn write_trace_file(&self, payload: &Value) {
        if let Ok(bytes) = serde_json::to_vec(payload) {
            let _ = std::fs::write(self.run_dir.join("trace.json"), bytes);
        }
    }
}

/// Abort safety net, mirroring `AgentRunRecorder`'s drop guard for `run.json`:
/// when the run future is dropped before `finish` (an entrypoint aborted the
/// task), persist the spans recorded so far under `socai.status=interrupted`.
/// The guard can't know why it was dropped, so the aborting entrypoint patches
/// the precise status afterwards via [`mark_run_trace_status`].
impl Drop for RunTraceBuilder {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        let usage = self.usage.clone();
        let payload = self.build_payload("interrupted", self.steps_seen, &usage, None);
        self.write_trace_file(&payload);
    }
}

/// Best-effort terminal patch when an entrypoint cancels the agent future:
/// rewrite the root span's `socai.status` (left as "interrupted" by the drop
/// guard) to the precise terminal status. The trace analog of
/// `mark_agent_run_status`.
pub fn mark_run_trace_status(run_dir: impl AsRef<Path>, status: &str) -> std::io::Result<()> {
    let path = run_dir.as_ref().join("trace.json");
    let mut payload: Value =
        serde_json::from_str(&std::fs::read_to_string(&path)?).map_err(std::io::Error::other)?;
    let spans = payload
        .pointer_mut("/resourceSpans/0/scopeSpans/0/spans")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| std::io::Error::other("trace.json has no spans"))?;
    // The root span is the only one without a parent.
    let root = spans
        .iter_mut()
        .find(|span| span.get("parentSpanId").is_none())
        .ok_or_else(|| std::io::Error::other("trace.json has no root span"))?;
    let attributes = root
        .get_mut("attributes")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| std::io::Error::other("root span has no attributes"))?;
    for attribute in attributes {
        if attribute.get("key").and_then(Value::as_str) == Some("socai.status") {
            attribute["value"] = json!({ "stringValue": status });
        }
    }
    std::fs::write(
        &path,
        serde_json::to_vec(&payload).map_err(std::io::Error::other)?,
    )
}

/// Flatten a summarizer map (`summarize_tool_args` / `summarize_tool_result`)
/// into `socai.`-prefixed OTLP attributes. Nested objects (the args `metadata`)
/// are carried as a capped JSON string.
fn extend_prefixed(attrs: &mut Vec<Value>, props: Map<String, Value>) {
    for (key, value) in props {
        if let Some(attr) = otlp_attr(&format!("socai.{key}"), &value) {
            attrs.push(attr);
        }
    }
}

fn otlp_attr(key: &str, value: &Value) -> Option<Value> {
    let encoded = match value {
        Value::Null => return None,
        Value::Bool(flag) => json!({ "boolValue": flag }),
        Value::Number(number) if number.is_i64() || number.is_u64() => {
            json!({ "intValue": number.to_string() })
        }
        Value::Number(number) => json!({ "doubleValue": number.as_f64() }),
        Value::String(text) => json!({ "stringValue": text }),
        other => json!({ "stringValue": truncate_chars(&other.to_string(), VALUE_JSON_MAX_CHARS) }),
    };
    Some(json!({ "key": key, "value": encoded }))
}

fn attr_str(key: &str, value: &str) -> Value {
    json!({ "key": key, "value": { "stringValue": value } })
}

fn attr_int(key: &str, value: i64) -> Value {
    json!({ "key": key, "value": { "intValue": value.to_string() } })
}

fn attr_bool(key: &str, value: bool) -> Value {
    json!({ "key": key, "value": { "boolValue": value } })
}

fn attr_double(key: &str, value: f64) -> Value {
    json!({ "key": key, "value": { "doubleValue": value } })
}

/// Attach the same normalized usage contract to both per-call chat spans and
/// the aggregate agent-run span. Raw provider usage stays in local run files;
/// telemetry only carries bounded numeric totals and pricing provenance.
fn push_usage_attrs(attrs: &mut Vec<Value>, usage: &TokenUsage) {
    attrs.extend([
        attr_int("gen_ai.usage.input_tokens", usage.input_tokens as i64),
        attr_int(
            "gen_ai.usage.uncached_input_tokens",
            usage.uncached_input_tokens as i64,
        ),
        attr_int("gen_ai.usage.output_tokens", usage.output_tokens as i64),
        attr_int(
            "gen_ai.usage.cache_read_input_tokens",
            usage.cache_read_input_tokens as i64,
        ),
        attr_int(
            "gen_ai.usage.cache_creation_input_tokens",
            usage.cache_creation_input_tokens as i64,
        ),
    ]);
    if let Some(tokens) = usage.reasoning_output_tokens {
        attrs.push(attr_int(
            "gen_ai.usage.reasoning_output_tokens",
            tokens as i64,
        ));
    }
    if let Some(cost) = &usage.cost {
        attrs.extend([
            attr_double("gen_ai.usage.estimated_input_cost", cost.input),
            attr_double("gen_ai.usage.estimated_output_cost", cost.output),
            attr_double("gen_ai.usage.estimated_cache_read_cost", cost.cache_read),
            attr_double(
                "gen_ai.usage.estimated_cache_creation_cost",
                cost.cache_creation,
            ),
            attr_double("gen_ai.usage.estimated_cost", cost.total),
            attr_bool("gen_ai.usage.cost_estimated", cost.estimated),
            attr_str("gen_ai.usage.cost_currency", &cost.currency),
            attr_str("gen_ai.usage.cost_pricing_source", &cost.pricing_source),
        ]);
    }
}

/// Last-line size gate on the assembled payload: the chat budget bounds
/// content bytes, but span envelopes and non-content attributes (tool arg
/// summaries, query text, timings) ride on top, so a span-heavy run could
/// still overshoot the proxy's body cap and lose the whole trace. When the
/// serialized payload exceeds [`PAYLOAD_MAX_BYTES`], strip attributes tier by
/// tier ([`STRIP_TIERS`]) and, within a tier, span by span — oldest first, so
/// the most recent steps stay readable — marking each stripped span with
/// `socai.content_dropped`. Freed bytes are counted from each removed
/// attribute's own serialized size (exact modulo separators). With both tiers
/// stripped only small bounded attributes remain, so the result always fits.
fn enforce_payload_cap(payload: &mut Value) {
    let Ok(serialized) = serde_json::to_vec(payload) else {
        return;
    };
    let mut total = serialized.len();
    if total <= PAYLOAD_MAX_BYTES {
        return;
    }
    let Some(spans) = payload
        .pointer_mut("/resourceSpans/0/scopeSpans/0/spans")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    'tiers: for keys in STRIP_TIERS {
        for span in spans.iter_mut() {
            if total <= PAYLOAD_MAX_BYTES {
                break 'tiers;
            }
            let Some(attrs) = span.get_mut("attributes").and_then(Value::as_array_mut) else {
                continue;
            };
            let mut freed = 0usize;
            attrs.retain(|attr| {
                let strip = attr
                    .get("key")
                    .and_then(Value::as_str)
                    .is_some_and(|key| keys.contains(&key));
                if strip {
                    freed += serde_json::to_string(attr).map(|s| s.len()).unwrap_or(0);
                }
                !strip
            });
            if freed == 0 {
                continue;
            }
            let marker_key = "socai.content_dropped";
            if !attrs
                .iter()
                .any(|attr| attr.get("key").and_then(Value::as_str) == Some(marker_key))
            {
                let marker = attr_bool(marker_key, true);
                freed = freed.saturating_sub(marker.to_string().len());
                attrs.push(marker);
            }
            total = total.saturating_sub(freed);
        }
    }
}

/// Serialize chat content, retrying with a tighter per-part cap when the
/// first pass overshoots `CHAT_ATTR_MAX_BYTES`; pathological content degrades
/// to a marker string rather than risking the whole trace upload.
fn capped_chat_json(build: impl Fn(usize) -> Value) -> String {
    let full = build(CHAT_PART_MAX_CHARS).to_string();
    if full.len() <= CHAT_ATTR_MAX_BYTES {
        return full;
    }
    let tight = build(CHAT_PART_TIGHT_MAX_CHARS).to_string();
    if tight.len() <= CHAT_ATTR_MAX_BYTES {
        return tight;
    }
    "[chat content omitted: exceeds trace attribute cap]".to_string()
}

/// OTel GenAI `gen_ai.input.messages` shape: `[{role, parts: [...]}]`.
fn input_messages_json(messages: &[Message], part_cap: usize, include_query: bool) -> Value {
    Value::Array(
        messages
            .iter()
            .map(|message| {
                json!({
                    "role": message_role(message),
                    "parts": message_parts(&message.content.as_blocks(), part_cap, include_query),
                })
            })
            .collect(),
    )
}

/// OTel GenAI `gen_ai.output.messages` shape. Built from the raw response —
/// not conversation history, which truncates assistant text — so the trace
/// carries the full text, reasoning, and tool calls of this step.
fn output_messages_json(response: &LLMResponse, part_cap: usize, include_query: bool) -> Value {
    let mut parts: Vec<Value> = Vec::new();
    for block in &response.thinking_blocks {
        parts.push(json!({
            "type": "reasoning",
            "content": chat_text(&block.thinking, part_cap),
        }));
    }
    if response.thinking_blocks.is_empty() && !response.reasoning_content.trim().is_empty() {
        parts.push(json!({
            "type": "reasoning",
            "content": chat_text(&response.reasoning_content, part_cap),
        }));
    }
    for text in &response.text_blocks {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Prompt-driven thinking (models without a native channel) arrives as
        // prefixed text; label it reasoning like the other channels.
        match trimmed.strip_prefix(THINKING_TEXT_PREFIX) {
            Some(thinking) => parts.push(json!({
                "type": "reasoning",
                "content": chat_text(thinking.trim(), part_cap),
            })),
            None => parts.push(json!({ "type": "text", "content": chat_text(trimmed, part_cap) })),
        }
    }
    for tool_call in &response.tool_calls {
        parts.push(json!({
            "type": "tool_call",
            "id": tool_call.id,
            "name": tool_call.name,
            "arguments": tool_call_arguments(&tool_call.input, part_cap, include_query),
        }));
    }
    json!([{
        "role": "assistant",
        "parts": parts,
        "finish_reason": stop_reason_str(response),
    }])
}

/// Redact-then-truncate for every chat content string. Redaction runs on the
/// full text first so a cap can't split a secret and leak its prefix.
fn chat_text(text: &str, part_cap: usize) -> String {
    truncate_chars(&redact_secrets(text), part_cap)
}

/// Compact note summaries (id/title/caption/stats) pulled from a site tool's
/// output text blocks, so a trace shows which notes the agent saw without
/// replaying the run. Matches the opened-note shape (`notes[].entity`) and the
/// preview/profile card arrays — the same paths `summarize_tool_result`
/// counts. Stats stay the raw displayed strings ("1.2万", "评论").
fn note_summaries(output: &Value) -> Vec<Value> {
    let mut notes: Vec<Value> = Vec::new();
    let mut seen_ids: Vec<String> = Vec::new();
    let Some(blocks) = output.as_array() else {
        return notes;
    };
    for block in blocks {
        let Some(text) = block.get("text").and_then(Value::as_str) else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<Value>(text) else {
            continue;
        };
        let data = parsed.get("data").unwrap_or(&parsed);
        if let Some(items) = data.get("notes").and_then(Value::as_array) {
            for item in items {
                let entity = item.get("entity").unwrap_or(item);
                push_note_summary(entity, &mut notes, &mut seen_ids);
            }
        }
        let card_lists = [
            data.get("cards"),
            data.get("search").and_then(|search| search.get("cards")),
            data.get("selected_cards"),
            // author_scan preview: `profile.note_cards` is the sole listing
            // (it's stripped once notes are opened — see xhs tools).
            data.get("profile")
                .and_then(|profile| profile.get("note_cards")),
        ];
        for items in card_lists.into_iter().flatten() {
            if let Some(items) = items.as_array() {
                for item in items {
                    push_note_summary(item, &mut notes, &mut seen_ids);
                }
            }
        }
    }
    notes
}

fn push_note_summary(entity: &Value, notes: &mut Vec<Value>, seen_ids: &mut Vec<String>) {
    if notes.len() >= NOTES_MAX_PER_SPAN {
        return;
    }
    let id = truncate_chars(&string_field(entity, "note_id"), NOTE_ID_MAX_CHARS);
    let title = string_field(entity, "title");
    if id.is_empty() && title.is_empty() {
        return;
    }
    if !id.is_empty() {
        if seen_ids.iter().any(|seen| seen == &id) {
            return;
        }
        seen_ids.push(id.clone());
    }
    let mut note = Map::new();
    if !id.is_empty() {
        note.insert("id".into(), json!(id));
    }
    if !title.is_empty() {
        note.insert(
            "title".into(),
            json!(chat_text(&title, NOTE_TITLE_MAX_CHARS)),
        );
    }
    let caption = string_field(entity, "content");
    if !caption.is_empty() {
        note.insert(
            "caption".into(),
            json!(chat_text(&caption, NOTE_CAPTION_MAX_CHARS)),
        );
    }
    for (field, key) in [
        ("likes", "likes"),
        ("favorites", "favorites"),
        ("comments_count", "comments"),
    ] {
        let value = truncate_chars(&string_field(entity, field), NOTE_STAT_MAX_CHARS);
        if !value.is_empty() {
            note.insert(key.into(), json!(value));
        }
    }
    notes.push(Value::Object(note));
}

/// Field as trimmed text; numbers are rendered so card shapes that carry
/// numeric counts still report.
fn string_field(entity: &Value, key: &str) -> String {
    match entity.get(key) {
        Some(Value::String(text)) => text.trim().to_string(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

/// JSON field names whose string values are scrubbed by [`redact_secrets`].
/// Covers `~/.socai/auth.json` (every provider key lives under `api_key`)
/// and common token envelopes a `bash`/`read_file` result could echo.
const SECRET_JSON_FIELDS: [&str; 11] = [
    "api_key",
    "apikey",
    "access_token",
    "refresh_token",
    "id_token",
    "client_secret",
    "device_token",
    "authorization",
    "password",
    "secret",
    "xsec_token",
];

/// Client-side secret scrub applied to every chat-content string before it
/// enters a trace attribute. Desktop `read_file`/`bash` are confined to
/// `~/.socai` — which is exactly where `auth.json` stores provider api keys —
/// so tool results can legitimately contain live secrets. Targeted patterns,
/// chosen for near-zero false positives on prose:
/// - values of sensitive JSON fields (`"api_key": "…"` → `"api_key":"[redacted]"`)
/// - `sk-`-prefixed token runs (every configured provider's key shape)
/// - JWT-shaped `eyJ…` runs (oauth access/id tokens)
/// - `Bearer <token>` header values
pub fn redact_secrets(text: &str) -> String {
    let text = redact_json_secret_fields(text);
    redact_token_runs(&text)
}

/// Recursively scrub every string inside a JSON value — used for structured
/// payloads (tool-arg `metadata`) where secrets can sit in any nested string,
/// e.g. a desktop `bash` command carrying an `Authorization: Bearer …` header.
pub fn redact_secrets_in_value(value: &mut Value) {
    match value {
        Value::String(text) => {
            let scrubbed = redact_secrets(text);
            if scrubbed != *text {
                *text = scrubbed;
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_secrets_in_value(item);
            }
        }
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                if SECRET_JSON_FIELDS
                    .iter()
                    .any(|field| key.eq_ignore_ascii_case(field))
                {
                    *item = Value::String("[redacted]".to_string());
                } else {
                    redact_secrets_in_value(item);
                }
            }
        }
        _ => {}
    }
}

fn redact_json_secret_fields(text: &str) -> String {
    let bytes = text.as_bytes();
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            if let Some(key_end) = match_secret_field(&lower, i) {
                let mut j = key_end;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b':' {
                    let mut k = j + 1;
                    while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                        k += 1;
                    }
                    if k < bytes.len() && bytes[k] == b'"' {
                        let mut v = k + 1;
                        while v < bytes.len() && !(bytes[v] == b'"' && bytes[v - 1] != b'\\') {
                            v += 1;
                        }
                        if v < bytes.len() {
                            out.push_str(&text[i..=k]);
                            out.push_str("[redacted]");
                            out.push('"');
                            i = v + 1;
                            continue;
                        }
                    }
                }
            }
        }
        let step = utf8_len(bytes[i]);
        out.push_str(&text[i..i + step]);
        i += step;
    }
    out
}

/// If `lower[start..]` opens a quoted sensitive field name, return the byte
/// index just past its closing quote.
fn match_secret_field(lower: &str, start: usize) -> Option<usize> {
    let rest = &lower.as_bytes()[start + 1..];
    for field in SECRET_JSON_FIELDS {
        let f = field.as_bytes();
        if rest.len() > f.len() && rest.starts_with(f) && rest[f.len()] == b'"' {
            return Some(start + 1 + f.len() + 1);
        }
    }
    None
}

fn redact_token_runs(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let rest = &text[i..];
        if rest.starts_with("sk-") {
            let run = token_run_len(&bytes[i + 3..], |b| {
                b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
            });
            if run >= 12 {
                out.push_str("sk-[redacted]");
                i += 3 + run;
                continue;
            }
        }
        if rest.starts_with("eyJ") {
            let run = token_run_len(&bytes[i..], |b| {
                b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.'
            });
            if run >= 20 && bytes[i..i + run].contains(&b'.') {
                out.push_str("[redacted-jwt]");
                i += run;
                continue;
            }
        }
        if bytes.len() - i > 7 && bytes[i..i + 7].eq_ignore_ascii_case(b"bearer ") {
            let run = token_run_len(&bytes[i + 7..], |b| {
                b.is_ascii_alphanumeric()
                    || matches!(b, b'.' | b'_' | b'~' | b'+' | b'/' | b'-' | b'=')
            });
            if run >= 16 {
                out.push_str(&text[i..i + 7]);
                out.push_str("[redacted]");
                i += 7 + run;
                continue;
            }
        }
        let step = utf8_len(bytes[i]);
        out.push_str(&text[i..i + step]);
        i += step;
    }
    out
}

fn token_run_len(bytes: &[u8], accept: impl Fn(u8) -> bool) -> usize {
    bytes.iter().take_while(|&&b| accept(b)).count()
}

fn utf8_len(lead: u8) -> usize {
    match lead {
        b if b < 0x80 => 1,
        b if b < 0xE0 => 2,
        b if b < 0xF0 => 3,
        _ => 4,
    }
}

/// Tool results travel as user messages internally; semconv wants them under
/// role `tool` so GenAI-aware views render them as tool output.
fn message_role(message: &Message) -> &'static str {
    match message.role {
        MessageRole::Assistant => "assistant",
        MessageRole::User => match &message.content {
            MessageContent::Blocks(blocks)
                if blocks
                    .iter()
                    .any(|block| matches!(block, Block::ToolResult { .. })) =>
            {
                "tool"
            }
            _ => "user",
        },
    }
}

fn message_parts(blocks: &[Block], part_cap: usize, include_query: bool) -> Vec<Value> {
    let mut parts: Vec<Value> = Vec::new();
    for block in blocks {
        match block {
            Block::Text { text } => {
                parts.push(json!({ "type": "text", "content": chat_text(text, part_cap) }));
            }
            Block::Image { media_type, .. } => {
                parts.push(
                    json!({ "type": "text", "content": format!("[image {media_type} omitted]") }),
                );
            }
            Block::ReasoningContent { text } => {
                parts.push(json!({ "type": "reasoning", "content": chat_text(text, part_cap) }));
            }
            // Signature is an opaque replay token — only the thinking text goes up.
            Block::Thinking { thinking, .. } => {
                parts
                    .push(json!({ "type": "reasoning", "content": chat_text(thinking, part_cap) }));
            }
            // Encrypted replay blob; nothing human-readable to upload.
            Block::OpenAIReasoning { .. } => {}
            Block::ToolUse { id, name, input } => {
                parts.push(json!({
                    "type": "tool_call",
                    "id": id,
                    "name": name,
                    "arguments": tool_call_arguments(input, part_cap, include_query),
                }));
            }
            Block::ToolResult {
                tool_use_id,
                content,
            } => {
                parts.push(json!({
                    "type": "tool_call_response",
                    "id": tool_use_id,
                    "response": chat_text(&tool_result_text(content), part_cap),
                }));
            }
        }
    }
    parts
}

fn tool_result_text(content: &[ToolResultContent]) -> String {
    content
        .iter()
        .map(|item| match item {
            ToolResultContent::Text { text } => text.clone(),
            ToolResultContent::Image { media_type, .. } => format!("[image {media_type} omitted]"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Tool arguments stay structured JSON unless a secret was redacted or the
/// serialized form is oversized, in which case they degrade to a (redacted,
/// truncated) string form. `include_query` mirrors the events pipeline's
/// query gate: when off, a top-level `query` string argument is redacted.
fn tool_call_arguments(input: &Value, part_cap: usize, include_query: bool) -> Value {
    let mut input = input.clone();
    if !include_query {
        if let Some(object) = input.as_object_mut() {
            // Any type: a malformed tool call ({"query": {…}}) must not
            // sidestep the gate.
            if object.contains_key("query") {
                object.insert("query".into(), json!("[redacted]"));
            }
        }
    }
    let rendered = input.to_string();
    let redacted = redact_secrets(&rendered);
    if redacted == rendered && rendered.chars().count() <= part_cap {
        input
    } else {
        Value::String(truncate_chars(&redacted, part_cap))
    }
}

fn stop_reason_str(response: &LLMResponse) -> &'static str {
    use crate::agent::llm::StopReason;
    match response.stop_reason {
        StopReason::EndTurn => "end_turn",
        StopReason::ToolUse => "tool_use",
        StopReason::MaxTokens => "max_tokens",
        StopReason::Other => "other",
    }
}

fn new_span_id() -> String {
    Uuid::new_v4().simple().to_string()[..16].to_string()
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or_default()
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{truncated}…")
}
