//! Lossless archive of the tool results that were present on provider request
//! wires. The source is the existing `llm/NNN.request.json` run artifact, not
//! raw tool output and not the size-bounded OTLP chat representation.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::trace::redact_secrets_in_value;
use crate::agent::llm::LLMResponse;
use crate::agent::run_logging::write_json_atomic;

pub(crate) const SCHEMA_VERSION: &str = "socai.model-visible-evidence.v1";
const ARCHIVE_RELATIVE_PATH: &str = "evidence/model-visible-v1.json";
const CHUNK_MAX_BYTES: usize = 24 * 1024;

#[derive(Debug)]
struct TraceIdentity {
    trace_id: String,
    root_span_id: String,
}

#[derive(Debug)]
struct RunMetadata {
    run_id: String,
    provider: String,
    model: String,
    steps: u64,
}

#[derive(Debug)]
struct ExtractedToolResult {
    tool_call_id: String,
    tool_name: Option<String>,
    message_index: usize,
    result_index: usize,
    wire_format: &'static str,
    content: Value,
}

/// Build a model-visible evidence archive, keep a shareable copy in the run,
/// patch the trace with a small local summary, and durably stage the upload.
pub(crate) fn stage_run_archive(
    run_dir: &Path,
    pending_dir: &Path,
    content_enabled: bool,
) -> io::Result<PathBuf> {
    let archive = build_archive(run_dir, content_enabled)?;
    let evaluation_id = archive
        .get("evaluation_id")
        .and_then(Value::as_str)
        .ok_or_else(|| io::Error::other("evidence archive has no evaluation_id"))?;

    let local_path = run_dir.join(ARCHIVE_RELATIVE_PATH);
    write_json_atomic(&local_path, &archive)?;
    patch_trace_summary(run_dir, &archive)?;

    fs::create_dir_all(pending_dir)?;
    let destination = pending_dir.join(format!("{}.json", evaluation_id.replace(':', "-")));
    write_json_atomic(&destination, &archive)?;
    Ok(destination)
}

fn build_archive(run_dir: &Path, content_enabled: bool) -> io::Result<Value> {
    let trace = read_json(&run_dir.join("trace.json"))?;
    let identity = trace_identity(&trace)
        .ok_or_else(|| io::Error::other("trace.json has no root trace/span id"))?;
    let run = read_json(&run_dir.join("run.json"))?;
    let metadata = run_metadata(&run);
    let evaluation_id = format!("{}:{}", identity.trace_id, identity.root_span_id);
    let created_at = Utc::now().to_rfc3339();

    let mut records = Vec::new();
    let mut index_entries = Vec::new();
    let mut seen_evidence = HashSet::new();
    let mut model_view_cache: HashMap<String, (Value, String)> = HashMap::new();
    let mut accepted_request_count = 0usize;
    let mut evidence_object_count = 0usize;
    let mut evidence_chunk_count = 0usize;
    let mut total_content_bytes = 0usize;
    let mut partial = false;

    if content_enabled {
        let requests = request_files(run_dir)?;
        if requests.is_empty() && metadata.steps > 0 {
            partial = true;
        }
        for (step, request_path) in requests {
            let response_path = run_dir.join("llm").join(format!("{step:03}.response.json"));
            let (request_status, wire_format) = request_observation(run_dir, step, &response_path);
            let mut evidence_ids = Vec::new();
            let mut extraction_error = None;

            if request_status == "accepted" {
                accepted_request_count += 1;
                match read_json(&request_path).and_then(|payload| {
                    extract_model_visible_tool_results(
                        &metadata.provider,
                        wire_format.as_deref(),
                        &payload,
                    )
                }) {
                    Ok(results) => {
                        for result in results {
                            let mut content = result.content;
                            if let Some((cached_content, cached_id)) =
                                model_view_cache.get(&result.tool_call_id)
                            {
                                if cached_content == &content {
                                    evidence_ids.push(cached_id.clone());
                                    continue;
                                }
                            }
                            let original = content.clone();
                            redact_secrets_in_value(&mut content);
                            let redaction_count = changed_leaf_count(&original, &content);
                            let content_bytes =
                                serde_json::to_vec(&content).map_err(io::Error::other)?;
                            let content_sha256 = sha256_hex(&content_bytes);
                            let evidence_id =
                                evidence_id(&evaluation_id, &result.tool_call_id, &content_sha256);
                            evidence_ids.push(evidence_id.clone());
                            model_view_cache.insert(
                                result.tool_call_id.clone(),
                                (original, evidence_id.clone()),
                            );

                            if !seen_evidence.insert(evidence_id.clone()) {
                                continue;
                            }

                            let content_text =
                                String::from_utf8(content_bytes).map_err(io::Error::other)?;
                            let chunks = split_utf8(&content_text, CHUNK_MAX_BYTES);
                            let chunk_count = chunks.len();
                            for (chunk_index, chunk_text) in chunks.into_iter().enumerate() {
                                let chunk_bytes = chunk_text.as_bytes();
                                records.push(common_record(
                                    &identity,
                                    &metadata,
                                    &evaluation_id,
                                    &created_at,
                                    json!({
                                        "record_type": "evidence_chunk",
                                        "evidence_id": evidence_id,
                                        "tool_call_id": result.tool_call_id,
                                        "tool_name": result.tool_name,
                                        "wire_format": result.wire_format,
                                        "first_observed_step": step,
                                        "message_index": result.message_index,
                                        "result_index": result.result_index,
                                        "content_encoding": "canonical-json-utf8",
                                        "content_sha256": content_sha256,
                                        "content_bytes": content_text.len(),
                                        "chunk_index": chunk_index,
                                        "chunk_count": chunk_count,
                                        "chunk_sha256": sha256_hex(chunk_bytes),
                                        "chunk_bytes": chunk_bytes.len(),
                                        "chunk_text": chunk_text,
                                        "redaction_version": "socai-secret-redactor-v1",
                                        "redaction_count": redaction_count,
                                        "semantic_redaction": false,
                                    }),
                                ));
                            }
                            evidence_object_count += 1;
                            evidence_chunk_count += chunk_count;
                            total_content_bytes += content_text.len();
                            index_entries.push(json!({
                                "evidence_id": evidence_id,
                                "content_sha256": content_sha256,
                                "chunk_count": chunk_count,
                            }));
                        }
                    }
                    Err(error) => {
                        partial = true;
                        extraction_error = Some(error.to_string());
                    }
                }
            } else if request_status == "unknown" {
                partial = true;
            }

            dedupe_preserving_order(&mut evidence_ids);
            let manifest_status = if extraction_error.is_some() {
                "unsupported"
            } else {
                request_status.as_str()
            };
            let manifest_body = json!({
                "step": step,
                "request_status": manifest_status,
                "evidence_ids": evidence_ids.clone(),
            });
            let manifest_sha256 = sha256_json(&manifest_body)?;
            index_entries.push(json!({
                "step": step,
                "manifest_sha256": manifest_sha256,
            }));
            records.push(common_record(
                &identity,
                &metadata,
                &evaluation_id,
                &created_at,
                json!({
                    "record_type": "request_manifest",
                    "step": step,
                    "request_status": manifest_status,
                    "evidence_count": evidence_ids.len(),
                    "evidence_ids": evidence_ids,
                    "manifest_sha256": manifest_sha256,
                    "error": extraction_error,
                }),
            ));
        }
    }

    let archive_status = if !content_enabled {
        "disabled"
    } else if partial {
        "partial"
    } else {
        "complete"
    };
    let evidence_index_sha256 = sha256_json(&Value::Array(index_entries))?;
    records.push(common_record(
        &identity,
        &metadata,
        &evaluation_id,
        &created_at,
        json!({
            "record_type": "turn_commit",
            "archive_status": archive_status,
            "accepted_request_count": accepted_request_count,
            "evidence_object_count": evidence_object_count,
            "evidence_chunk_count": evidence_chunk_count,
            "total_content_bytes": total_content_bytes,
            "evidence_index_sha256": evidence_index_sha256,
            "telemetry_policy": if content_enabled { "chat_and_evidence_enabled" } else { "evidence_disabled" },
            "committed_at": created_at,
        }),
    ));

    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "evaluation_id": evaluation_id,
        "trace_id": identity.trace_id,
        "root_span_id": identity.root_span_id,
        "run_id": metadata.run_id,
        "archive_status": archive_status,
        "accepted_request_count": accepted_request_count,
        "evidence_object_count": evidence_object_count,
        "evidence_chunk_count": evidence_chunk_count,
        "total_content_bytes": total_content_bytes,
        "records": records,
    }))
}

fn extract_model_visible_tool_results(
    provider: &str,
    wire_format: Option<&str>,
    payload: &Value,
) -> io::Result<Vec<ExtractedToolResult>> {
    match wire_format {
        Some("openai_responses") => return extract_openai_responses(payload),
        Some("anthropic_messages") => return extract_anthropic_messages(payload),
        Some("openai_chat") => return extract_openai_chat(payload),
        Some(other) => {
            return Err(io::Error::other(format!(
                "unsupported recorded provider wire format: {other}"
            )))
        }
        None => {}
    }
    if payload.get("input").and_then(Value::as_array).is_some() {
        return extract_openai_responses(payload);
    }
    if provider.eq_ignore_ascii_case("anthropic") || has_anthropic_tool_result(payload) {
        return extract_anthropic_messages(payload);
    }
    if payload.get("messages").and_then(Value::as_array).is_some() {
        return extract_openai_chat(payload);
    }
    Err(io::Error::other(format!(
        "unsupported provider request shape: {provider}"
    )))
}

fn extract_openai_chat(payload: &Value) -> io::Result<Vec<ExtractedToolResult>> {
    let messages = payload
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("OpenAI chat request has no messages"))?;
    let mut names = HashMap::new();
    for message in messages {
        let Some(calls) = message.get("tool_calls").and_then(Value::as_array) else {
            continue;
        };
        for call in calls {
            if let (Some(id), Some(name)) = (
                call.get("id").and_then(Value::as_str),
                call.pointer("/function/name").and_then(Value::as_str),
            ) {
                names.insert(id.to_string(), name.to_string());
            }
        }
    }

    let mut results = Vec::new();
    for (message_index, message) in messages.iter().enumerate() {
        if message.get("role").and_then(Value::as_str) != Some("tool") {
            continue;
        }
        let tool_call_id = message
            .get("tool_call_id")
            .and_then(Value::as_str)
            .ok_or_else(|| io::Error::other("OpenAI tool message has no tool_call_id"))?;
        let content = message
            .get("content")
            .ok_or_else(|| io::Error::other("OpenAI tool message has no content"))?;
        results.push(ExtractedToolResult {
            tool_call_id: tool_call_id.to_string(),
            tool_name: names.get(tool_call_id).cloned(),
            message_index,
            result_index: results.len(),
            wire_format: "openai_chat_tool_message",
            content: content.clone(),
        });
    }
    Ok(results)
}

fn extract_openai_responses(payload: &Value) -> io::Result<Vec<ExtractedToolResult>> {
    let input = payload
        .get("input")
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("OpenAI Responses request has no input"))?;
    let mut names = HashMap::new();
    for item in input {
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            continue;
        }
        if let (Some(id), Some(name)) = (
            item.get("call_id").and_then(Value::as_str),
            item.get("name").and_then(Value::as_str),
        ) {
            names.insert(id.to_string(), name.to_string());
        }
    }

    let mut results = Vec::new();
    for (message_index, item) in input.iter().enumerate() {
        if item.get("type").and_then(Value::as_str) != Some("function_call_output") {
            continue;
        }
        let tool_call_id = item
            .get("call_id")
            .and_then(Value::as_str)
            .ok_or_else(|| io::Error::other("function_call_output has no call_id"))?;
        let content = item
            .get("output")
            .ok_or_else(|| io::Error::other("function_call_output has no output"))?;
        results.push(ExtractedToolResult {
            tool_call_id: tool_call_id.to_string(),
            tool_name: names.get(tool_call_id).cloned(),
            message_index,
            result_index: results.len(),
            wire_format: "openai_responses_function_call_output",
            content: content.clone(),
        });
    }
    Ok(results)
}

fn extract_anthropic_messages(payload: &Value) -> io::Result<Vec<ExtractedToolResult>> {
    let messages = payload
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("Anthropic request has no messages"))?;
    let mut names = HashMap::new();
    for message in messages {
        let Some(blocks) = message.get("content").and_then(Value::as_array) else {
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            if let (Some(id), Some(name)) = (
                block.get("id").and_then(Value::as_str),
                block.get("name").and_then(Value::as_str),
            ) {
                names.insert(id.to_string(), name.to_string());
            }
        }
    }

    let mut results = Vec::new();
    for (message_index, message) in messages.iter().enumerate() {
        let Some(blocks) = message.get("content").and_then(Value::as_array) else {
            continue;
        };
        for (block_index, block) in blocks.iter().enumerate() {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let tool_call_id = block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .ok_or_else(|| io::Error::other("Anthropic tool_result has no tool_use_id"))?;
            let content = block
                .get("content")
                .ok_or_else(|| io::Error::other("Anthropic tool_result has no content"))?;
            results.push(ExtractedToolResult {
                tool_call_id: tool_call_id.to_string(),
                tool_name: names.get(tool_call_id).cloned(),
                message_index,
                result_index: block_index,
                wire_format: "anthropic_tool_result_block",
                content: content.clone(),
            });
        }
    }
    Ok(results)
}

fn has_anthropic_tool_result(payload: &Value) -> bool {
    payload
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
        .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
}

fn request_files(run_dir: &Path) -> io::Result<Vec<(u32, PathBuf)>> {
    let llm_dir = run_dir.join("llm");
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(llm_dir) else {
        return Ok(files);
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(step) = name
            .strip_suffix(".request.json")
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        files.push((step, entry.path()));
    }
    files.sort_by_key(|(step, _)| *step);
    Ok(files)
}

fn response_status(path: &Path) -> &'static str {
    let Ok(response) = read_json(path) else {
        return "unknown";
    };
    if response.get("error").is_some_and(|error| !error.is_null()) {
        "failed"
    } else if serde_json::from_value::<LLMResponse>(response).is_ok() {
        "accepted"
    } else {
        "unknown"
    }
}

fn request_observation(
    run_dir: &Path,
    step: u32,
    response_path: &Path,
) -> (String, Option<String>) {
    let metadata_path = run_dir
        .join("llm")
        .join(format!("{step:03}.request.meta.json"));
    if let Ok(metadata) = read_json(&metadata_path) {
        let status = match metadata.get("status").and_then(Value::as_str) {
            Some("accepted") => "accepted",
            Some("failed") => "failed",
            _ => "unknown",
        };
        let wire_format = metadata
            .get("wire_format")
            .and_then(Value::as_str)
            .map(str::to_string);
        return (status.to_string(), wire_format);
    }
    (response_status(response_path).to_string(), None)
}

fn trace_identity(trace: &Value) -> Option<TraceIdentity> {
    let root = trace
        .pointer("/resourceSpans/0/scopeSpans/0/spans")
        .and_then(Value::as_array)?
        .iter()
        .find(|span| span.get("parentSpanId").is_none())?;
    Some(TraceIdentity {
        trace_id: root.get("traceId")?.as_str()?.to_string(),
        root_span_id: root.get("spanId")?.as_str()?.to_string(),
    })
}

fn run_metadata(run: &Value) -> RunMetadata {
    RunMetadata {
        run_id: value_string(run, "id"),
        provider: value_string(run, "provider"),
        model: value_string(run, "model"),
        steps: run.get("steps").and_then(Value::as_u64).unwrap_or_default(),
    }
}

fn value_string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn common_record(
    identity: &TraceIdentity,
    metadata: &RunMetadata,
    evaluation_id: &str,
    created_at: &str,
    fields: Value,
) -> Value {
    let mut record = match fields {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    record.insert("schema_version".into(), json!(SCHEMA_VERSION));
    record.insert("evaluation_id".into(), json!(evaluation_id));
    record.insert("trace_id".into(), json!(identity.trace_id));
    record.insert("root_span_id".into(), json!(identity.root_span_id));
    record.insert("run_id".into(), json!(metadata.run_id));
    record.insert("provider".into(), json!(metadata.provider));
    record.insert("model".into(), json!(metadata.model));
    record.insert("created_at".into(), json!(created_at));
    record.insert("client_version".into(), json!(env!("CARGO_PKG_VERSION")));
    record.insert("platform".into(), json!(std::env::consts::OS));
    Value::Object(record)
}

fn patch_trace_summary(run_dir: &Path, archive: &Value) -> io::Result<()> {
    let trace_path = run_dir.join("trace.json");
    let mut trace = read_json(&trace_path)?;
    let root = trace
        .pointer_mut("/resourceSpans/0/scopeSpans/0/spans")
        .and_then(Value::as_array_mut)
        .and_then(|spans| {
            spans
                .iter_mut()
                .find(|span| span.get("parentSpanId").is_none())
        })
        .ok_or_else(|| io::Error::other("trace.json has no root span"))?;
    let attributes = root
        .get_mut("attributes")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| io::Error::other("trace root has no attributes"))?;
    const KEYS: [&str; 6] = [
        "socai.evidence.schema_version",
        "socai.evidence.local_status",
        "socai.evidence.upload_status_at_trace_build",
        "socai.evidence.accepted_request_count",
        "socai.evidence.object_count",
        "socai.evidence.total_bytes",
    ];
    attributes.retain(|attribute| {
        let key = attribute
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or_default();
        !KEYS.contains(&key)
    });
    for (key, value) in [
        ("socai.evidence.schema_version", json!(SCHEMA_VERSION)),
        (
            "socai.evidence.local_status",
            archive
                .get("archive_status")
                .cloned()
                .unwrap_or(Value::Null),
        ),
        (
            "socai.evidence.upload_status_at_trace_build",
            json!("queued"),
        ),
    ] {
        attributes.push(
            json!({ "key": key, "value": { "stringValue": value.as_str().unwrap_or_default() } }),
        );
    }
    for (key, field) in [
        (
            "socai.evidence.accepted_request_count",
            "accepted_request_count",
        ),
        ("socai.evidence.object_count", "evidence_object_count"),
        ("socai.evidence.total_bytes", "total_content_bytes"),
    ] {
        let value = archive
            .get(field)
            .and_then(Value::as_u64)
            .unwrap_or_default();
        attributes.push(json!({ "key": key, "value": { "intValue": value.to_string() } }));
    }
    write_json_atomic(&trace_path, &trace)
}

fn read_json(path: &Path) -> io::Result<Value> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn changed_leaf_count(before: &Value, after: &Value) -> usize {
    match (before, after) {
        (Value::Array(left), Value::Array(right)) => left
            .iter()
            .zip(right)
            .map(|(a, b)| changed_leaf_count(a, b))
            .sum(),
        (Value::Object(left), Value::Object(right)) => left
            .iter()
            .map(|(key, value)| {
                right
                    .get(key)
                    .map_or(1, |other| changed_leaf_count(value, other))
            })
            .sum(),
        _ => usize::from(before != after),
    }
}

fn split_utf8(text: &str, max_bytes: usize) -> Vec<String> {
    assert!(
        max_bytes >= 4,
        "UTF-8 chunks require a budget of at least 4 bytes"
    );
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + max_bytes).min(text.len());
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        debug_assert!(end > start);
        chunks.push(text[start..end].to_string());
        start = end;
    }
    chunks
}

fn dedupe_preserving_order(values: &mut Vec<String>) {
    let mut seen = HashSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

fn evidence_id(evaluation_id: &str, tool_call_id: &str, content_sha256: &str) -> String {
    let input = format!("{evaluation_id}\0{tool_call_id}\0{content_sha256}");
    format!("ev_{}", sha256_hex(input.as_bytes()))
}

fn sha256_json(value: &Value) -> io::Result<String> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    Ok(sha256_hex(&bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}
