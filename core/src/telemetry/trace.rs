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
//! and tool output through the count-only `summarize_tool_result`.
//!
//! Chat content rides on the `chat` spans (gated by
//! `SOCAI_TELEMETRY_CHAT_TEXT`): `gen_ai.input.messages` carries only the
//! conversation messages *new since the previous LLM call* — every request
//! re-sends the whole history, so per-span deltas let one trace reconstruct
//! the conversation exactly once instead of O(n²) re-uploads — and
//! `gen_ai.output.messages` carries that call's full response (history keeps
//! a truncated copy; the trace keeps the real one). `gen_ai.system_instructions`
//! is emitted when the system prompt changes. Image bytes, thinking
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
    Block, LLMResponse, Message, MessageContent, MessageRole, ToolResultContent,
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
    /// Remaining chat-content bytes for this run's upload; once spent, later
    /// spans carry `socai.chat_text_dropped` instead of content.
    chat_bytes_left: usize,
    /// Hash of the last uploaded system prompt, so `gen_ai.system_instructions`
    /// is emitted only when it changes.
    last_system_hash: String,
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
        self.input_tokens += response.input_tokens;
        self.output_tokens += response.output_tokens;
        let mut attrs = vec![
            attr_str("gen_ai.operation.name", "chat"),
            attr_str("gen_ai.request.model", &self.model),
            attr_int("socai.step", step as i64),
            attr_str("socai.stop_reason", stop_reason_str(response)),
            attr_int("socai.tool_calls", response.tool_calls.len() as i64),
            attr_int("gen_ai.usage.input_tokens", response.input_tokens as i64),
            attr_int("gen_ai.usage.output_tokens", response.output_tokens as i64),
        ];
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
        attrs.push(attr_int("socai.new_messages", new_messages.len() as i64));
        let system_hash = md5_hex(system.as_bytes());
        if self.last_system_hash != system_hash {
            self.last_system_hash = system_hash;
            let rendered = capped_chat_json(
                |cap| json!([{ "type": "text", "content": truncate_chars(system, cap) }]),
            );
            if !self.push_budgeted(attrs, "gen_ai.system_instructions", rendered) {
                return;
            }
        }
        let input = capped_chat_json(|cap| input_messages_json(new_messages, cap));
        if !self.push_budgeted(attrs, "gen_ai.input.messages", input) {
            return;
        }
        if let Some(response) = response {
            let output = capped_chat_json(|cap| output_messages_json(response, cap));
            self.push_budgeted(attrs, "gen_ai.output.messages", output);
        }
    }

    /// Push one chat-content attribute if it fits the remaining budget;
    /// otherwise zero the budget and mark the span. Returns whether it fit.
    fn push_budgeted(&mut self, attrs: &mut Vec<Value>, key: &str, rendered: String) -> bool {
        if self.chat_bytes_left < rendered.len() {
            self.chat_bytes_left = 0;
            attrs.push(attr_bool("socai.chat_text_dropped", true));
            return false;
        }
        self.chat_bytes_left -= rendered.len();
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
                let rendered = Value::Array(notes).to_string();
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
fn input_messages_json(messages: &[Message], part_cap: usize) -> Value {
    Value::Array(
        messages
            .iter()
            .map(|message| {
                json!({
                    "role": message_role(message),
                    "parts": message_parts(&message.content.as_blocks(), part_cap),
                })
            })
            .collect(),
    )
}

/// OTel GenAI `gen_ai.output.messages` shape. Built from the raw response —
/// not conversation history, which truncates assistant text — so the trace
/// carries the full text, reasoning, and tool calls of this step.
fn output_messages_json(response: &LLMResponse, part_cap: usize) -> Value {
    let mut parts: Vec<Value> = Vec::new();
    for block in &response.thinking_blocks {
        parts.push(json!({
            "type": "reasoning",
            "content": truncate_chars(&block.thinking, part_cap),
        }));
    }
    if response.thinking_blocks.is_empty() && !response.reasoning_content.trim().is_empty() {
        parts.push(json!({
            "type": "reasoning",
            "content": truncate_chars(&response.reasoning_content, part_cap),
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
                "content": truncate_chars(thinking.trim(), part_cap),
            })),
            None => {
                parts.push(json!({ "type": "text", "content": truncate_chars(trimmed, part_cap) }))
            }
        }
    }
    for tool_call in &response.tool_calls {
        parts.push(json!({
            "type": "tool_call",
            "id": tool_call.id,
            "name": tool_call.name,
            "arguments": capped_arguments(&tool_call.input, part_cap),
        }));
    }
    json!([{
        "role": "assistant",
        "parts": parts,
        "finish_reason": stop_reason_str(response),
    }])
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
    let id = string_field(entity, "note_id");
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
        note.insert("title".into(), json!(title));
    }
    let caption = string_field(entity, "content");
    if !caption.is_empty() {
        note.insert(
            "caption".into(),
            json!(truncate_chars(&caption, NOTE_CAPTION_MAX_CHARS)),
        );
    }
    for (field, key) in [
        ("likes", "likes"),
        ("favorites", "favorites"),
        ("comments_count", "comments"),
    ] {
        let value = string_field(entity, field);
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

fn message_parts(blocks: &[Block], part_cap: usize) -> Vec<Value> {
    let mut parts: Vec<Value> = Vec::new();
    for block in blocks {
        match block {
            Block::Text { text } => {
                parts.push(json!({ "type": "text", "content": truncate_chars(text, part_cap) }));
            }
            Block::Image { media_type, .. } => {
                parts.push(
                    json!({ "type": "text", "content": format!("[image {media_type} omitted]") }),
                );
            }
            Block::ReasoningContent { text } => {
                parts.push(
                    json!({ "type": "reasoning", "content": truncate_chars(text, part_cap) }),
                );
            }
            // Signature is an opaque replay token — only the thinking text goes up.
            Block::Thinking { thinking, .. } => {
                parts.push(
                    json!({ "type": "reasoning", "content": truncate_chars(thinking, part_cap) }),
                );
            }
            // Encrypted replay blob; nothing human-readable to upload.
            Block::OpenAIReasoning { .. } => {}
            Block::ToolUse { id, name, input } => {
                parts.push(json!({
                    "type": "tool_call",
                    "id": id,
                    "name": name,
                    "arguments": capped_arguments(input, part_cap),
                }));
            }
            Block::ToolResult {
                tool_use_id,
                content,
            } => {
                parts.push(json!({
                    "type": "tool_call_response",
                    "id": tool_use_id,
                    "response": truncate_chars(&tool_result_text(content), part_cap),
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

/// Tool arguments stay structured JSON unless oversized, in which case they
/// degrade to a truncated string form.
fn capped_arguments(input: &Value, part_cap: usize) -> Value {
    let rendered = input.to_string();
    if rendered.chars().count() <= part_cap {
        input.clone()
    } else {
        Value::String(truncate_chars(&rendered, part_cap))
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
