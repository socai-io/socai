//! In-memory working state used for context compaction and report enrichment.
//!
//! This is deliberately not persisted: exact LLM and tool records already own
//! the durable execution history.

#![allow(clippy::expect_used)]
#![allow(clippy::too_many_arguments)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::agent::compaction::{compact_value, truncate};

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactRecord {
    pub key: String,
    pub path: String,
    pub label: String,
    pub kind: String,
    pub source_tool: String,
    pub step: Option<u32>,
    pub summary: String,
    pub metadata: Value,
}

#[derive(Debug)]
struct Inner {
    task: String,
    plan: Vec<Value>,
    artifacts: BTreeMap<String, ArtifactRecord>,
    evidence: BTreeMap<String, Value>,
    recent_events: Vec<Value>,
}

#[derive(Debug)]
pub struct RunState {
    inner: Mutex<Inner>,
}

impl RunState {
    pub fn new(task: impl Into<String>) -> Self {
        Self {
            inner: Mutex::new(Inner {
                task: task.into().trim().to_string(),
                plan: Vec::new(),
                artifacts: BTreeMap::new(),
                evidence: BTreeMap::new(),
                recent_events: Vec::new(),
            }),
        }
    }

    fn append_event(&self, event: Value) {
        let mut guard = self.inner.lock().expect("poisoned");
        guard.recent_events.push(event);
        let overflow = guard.recent_events.len().saturating_sub(80);
        guard.recent_events.drain(0..overflow);
    }

    pub fn note_assistant_step(&self, step: u32, text: &str, tool_calls: &[Value]) {
        self.append_event(json!({
            "type": "assistant_step",
            "step": step,
            "text": truncate(text, 800),
            "tool_calls": tool_calls,
        }));
    }

    pub fn note_tool_call(&self, step: u32, tool_name: &str, tool_input: &Value) {
        self.append_event(json!({
            "type": "tool_call",
            "step": step,
            "tool": tool_name,
            "input": compact_value(tool_input),
        }));
    }

    pub fn note_tool_result(
        &self,
        step: u32,
        tool_name: &str,
        tool_input: &Value,
        result_summary: &str,
        duration_s: f64,
    ) {
        self.append_event(json!({
            "type": "tool_result",
            "step": step,
            "tool": tool_name,
            "input": compact_value(tool_input),
            "result_summary": truncate(result_summary, 800),
            "duration_s": duration_s,
        }));
    }

    pub fn record_artifact(
        &self,
        relative_path: &str,
        label: &str,
        kind: &str,
        source_tool: &str,
        step: Option<u32>,
        summary: &str,
        metadata: Value,
        payload: Option<&Value>,
    ) {
        {
            let mut guard = self.inner.lock().expect("poisoned");
            let key = format!("{:03}_{label}", guard.artifacts.len() + 1);
            guard.artifacts.insert(
                key.clone(),
                ArtifactRecord {
                    key,
                    path: relative_path.to_string(),
                    label: label.to_string(),
                    kind: kind.to_string(),
                    source_tool: source_tool.to_string(),
                    step,
                    summary: summary.to_string(),
                    metadata,
                },
            );
        }
        if let Some(payload) = payload {
            self.ingest_evidence(payload, relative_path, step);
        }
        self.append_event(json!({
            "type": "artifact_recorded",
            "step": step,
            "path": relative_path,
        }));
    }

    fn ingest_evidence(&self, payload: &Value, artifact_path: &str, step: Option<u32>) {
        let Value::Object(map) = payload else { return };
        let signal_keys = [
            "id",
            "entity_id",
            "note_id",
            "url",
            "title",
            "author",
            "content",
            "summary",
        ];
        if !signal_keys.iter().any(|key| {
            map.get(*key)
                .is_some_and(|value| !value.is_null() && value.as_str() != Some(""))
        }) {
            return;
        }
        let key = map
            .get("entity_id")
            .or_else(|| map.get("note_id"))
            .or_else(|| map.get("id"))
            .or_else(|| map.get("url"))
            .and_then(Value::as_str)
            .unwrap_or(artifact_path)
            .to_string();
        let mut compact = match compact_value(&Value::Object(map.clone())) {
            Value::Object(value) => value,
            _ => Map::new(),
        };
        compact.insert("key".into(), json!(key));
        compact.insert("artifact_path".into(), json!(artifact_path));
        if let Some(step) = step {
            compact.insert("step".into(), json!(step));
        }
        self.inner
            .lock()
            .expect("poisoned")
            .evidence
            .insert(key, Value::Object(compact));
    }

    pub fn update_plan_steps(&self, steps: Vec<Value>) {
        self.inner.lock().expect("poisoned").plan = steps;
        self.append_event(json!({"type": "plan_update"}));
    }

    pub fn render_working_memory(&self, max_recent_events: usize, max_evidence: usize) -> String {
        let guard = self.inner.lock().expect("poisoned");
        let mut out = format!("# Task\n{}\n\n# Plan\n", guard.task);
        if guard.plan.is_empty() {
            out.push_str("- No explicit plan has been recorded yet.\n");
        }
        for step in &guard.plan {
            let status = step
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending");
            let title = step.get("title").and_then(Value::as_str).unwrap_or("");
            out.push_str(&format!("- [{status}] {title}\n"));
        }
        out.push_str(&format!(
            "\n# Current State\n- Saved artifacts: {}\n- Evidence records: {}\n",
            guard.artifacts.len(),
            guard.evidence.len()
        ));
        out.push_str("\n# Recent Activity\n");
        for event in guard
            .recent_events
            .iter()
            .rev()
            .take(max_recent_events)
            .rev()
        {
            let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
            let step = event.get("step").and_then(Value::as_u64).unwrap_or(0);
            match kind {
                "assistant_step" => out.push_str(&format!(
                    "- step {step} assistant: {}\n",
                    truncate(event.get("text").and_then(Value::as_str).unwrap_or(""), 160)
                )),
                "tool_call" => out.push_str(&format!(
                    "- step {step} tool_call {}\n",
                    event.get("tool").and_then(Value::as_str).unwrap_or("")
                )),
                "tool_result" => out.push_str(&format!(
                    "- step {step} tool_result {}: {}\n",
                    event.get("tool").and_then(Value::as_str).unwrap_or(""),
                    truncate(
                        event
                            .get("result_summary")
                            .and_then(Value::as_str)
                            .unwrap_or(""),
                        160
                    )
                )),
                "artifact_recorded" => out.push_str(&format!(
                    "- step {step} saved {}\n",
                    event.get("path").and_then(Value::as_str).unwrap_or("")
                )),
                _ => {}
            }
        }
        out.push_str("\n# Key Evidence\n");
        for item in guard.evidence.values().rev().take(max_evidence).rev() {
            let title = item
                .get("title")
                .or_else(|| item.get("key"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let summary = item.get("summary").and_then(Value::as_str).unwrap_or("");
            out.push_str(&format!("- {title}: {}\n", truncate(summary, 180)));
        }
        out
    }

    pub fn context_block(&self, max_chars: usize) -> String {
        truncate(&self.render_working_memory(10, 8), max_chars)
    }

    pub fn has_structured_state(&self) -> bool {
        let guard = self.inner.lock().expect("poisoned");
        !guard.plan.is_empty() || !guard.evidence.is_empty() || !guard.artifacts.is_empty()
    }

    pub fn artifact_records(&self) -> Vec<ArtifactRecord> {
        self.inner
            .lock()
            .expect("poisoned")
            .artifacts
            .values()
            .cloned()
            .collect()
    }
}
