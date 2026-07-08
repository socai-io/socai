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
//! socai-specific context under `socai.*`. Payload policy matches the
//! `socai_tool_call` event: tool args go through `summarize_tool_args` (query
//! text gated by `SOCAI_TELEMETRY_QUERY_TEXT`), tool output through the
//! count-only `summarize_tool_result`, and LLM text/reasoning never leaves the
//! machine — only token counts, stop reasons, and timings.

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use super::query_text_enabled;
use super::tool_call::{summarize_tool_args, summarize_tool_result};
use crate::agent::llm::LLMResponse;
use crate::agent::signature::md5_hex;

/// Safety net for pathological runs; a default run (30 steps) stays far below.
const MAX_CHILD_SPANS: usize = 400;
/// Matches the proxy's `task_text` cap on the events route.
const TASK_TEXT_MAX_CHARS: usize = 8000;
const ERROR_MAX_CHARS: usize = 240;
const VALUE_JSON_MAX_CHARS: usize = 500;

const SPAN_KIND_INTERNAL: u32 = 1;
const SPAN_KIND_CLIENT: u32 = 3;
const STATUS_CODE_ERROR: u32 = 2;

pub struct RunTraceBuilder {
    run_dir: PathBuf,
    trace_id: String,
    root_span_id: String,
    run_id: String,
    task_text: String,
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
    input_tokens: u64,
    output_tokens: u64,
    finalized: bool,
}

impl RunTraceBuilder {
    pub fn new(
        run_dir: &Path,
        run_id: &str,
        task: &str,
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
            task_text: truncate_chars(task, TASK_TEXT_MAX_CHARS),
            model: model.to_string(),
            session_id: session_id.map(ToOwned::to_owned),
            seed_messages,
            started_ns: now_ns(),
            spans: Vec::new(),
            dropped_spans: 0,
            steps_seen: 0,
            input_tokens: 0,
            output_tokens: 0,
            finalized: false,
        }
    }

    pub fn record_llm(&mut self, step: u32, duration_ms: u64, response: &LLMResponse) {
        self.steps_seen = self.steps_seen.max(step);
        self.input_tokens += response.input_tokens;
        self.output_tokens += response.output_tokens;
        let attrs = vec![
            attr_str("gen_ai.operation.name", "chat"),
            attr_str("gen_ai.request.model", &self.model),
            attr_int("socai.step", step as i64),
            attr_str("socai.stop_reason", stop_reason_str(response)),
            attr_int("socai.tool_calls", response.tool_calls.len() as i64),
            attr_int("gen_ai.usage.input_tokens", response.input_tokens as i64),
            attr_int("gen_ai.usage.output_tokens", response.output_tokens as i64),
        ];
        self.push_span(
            format!("chat {}", self.model),
            SPAN_KIND_CLIENT,
            duration_ms,
            attrs,
            None,
        );
    }

    pub fn record_llm_error(&mut self, step: u32, duration_ms: u64, error: &str) {
        self.steps_seen = self.steps_seen.max(step);
        let attrs = vec![
            attr_str("gen_ai.operation.name", "chat"),
            attr_str("gen_ai.request.model", &self.model),
            attr_int("socai.step", step as i64),
        ];
        self.push_span(
            format!("chat {}", self.model),
            SPAN_KIND_CLIENT,
            duration_ms,
            attrs,
            Some(error),
        );
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
    pub fn finish(
        &mut self,
        status: &str,
        steps: u32,
        input_tokens: u64,
        output_tokens: u64,
        error: Option<&str>,
    ) {
        self.finalized = true;
        let payload = self.build_payload(status, steps, input_tokens, output_tokens, error);
        self.write_trace_file(&payload);
    }

    fn build_payload(
        &mut self,
        status: &str,
        steps: u32,
        input_tokens: u64,
        output_tokens: u64,
        error: Option<&str>,
    ) -> Value {
        let end_ns = now_ns();
        let mut attrs = vec![
            attr_str("gen_ai.operation.name", "invoke_agent"),
            attr_str("gen_ai.agent.name", "socai"),
            attr_str("gen_ai.request.model", &self.model),
            attr_str("socai.run_id", &self.run_id),
            attr_str("socai.task_text", &self.task_text),
            attr_str("socai.status", status),
            attr_int("socai.steps", steps as i64),
            attr_int("gen_ai.usage.input_tokens", input_tokens as i64),
            attr_int("gen_ai.usage.output_tokens", output_tokens as i64),
        ];
        if let Some(session_id) = &self.session_id {
            attrs.push(attr_str("socai.session_id", session_id));
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

        json!({
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
        })
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
        let payload = self.build_payload(
            "interrupted",
            self.steps_seen,
            self.input_tokens,
            self.output_tokens,
            None,
        );
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
