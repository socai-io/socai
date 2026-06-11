//! Persistent debug logging for one agent run.
//!
//! Writes (under the run dir):
//! - `reasoning_log.jsonl`       — append-only event timeline
//! - `conversation.json`         — final system + messages snapshot
//! - `agent_log.json`            — run summary (turns, run_dir, durations, …)
//! - `tool_results/<turn>_<seq>_<tool>.json` — full tool result body per call

// tool_result takes many fields (turn, sequence, tool_use_id, tool, input,
// content, duration_s, result_summary, repeat_count, error). Plain function
// args match the persisted debug payload directly.
#![allow(clippy::too_many_arguments)]

use std::path::{Path, PathBuf};

use chrono::Local;
use serde_json::{json, Map, Value};

fn timestamp() -> String {
    Local::now().to_rfc3339()
}

fn safe_slug(text: &str, max_chars: usize) -> String {
    let raw = text.trim().replace('/', " ");
    let mut acc: String = raw
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    while acc.contains("__") {
        acc = acc.replace("__", "_");
    }
    let trimmed: String = acc.trim_matches('_').to_string();
    let final_slug = if trimmed.is_empty() {
        "agent".to_string()
    } else {
        trimmed
    };
    final_slug.chars().take(max_chars).collect()
}

pub fn default_runs_root() -> PathBuf {
    if let Ok(env) = std::env::var("SOCAI_RUNS_DIR") {
        return PathBuf::from(env);
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".socai/runs");
    }
    PathBuf::from(".socai/runs")
}

/// Allocate a unique run directory for the given task:
/// `<YYYYMMDD_HHMMSS>_<slug>` with a numeric suffix if that name already
/// exists. The timestamp is local time, matching the per-frame snapshot
/// folder names.
pub fn make_run_dir(task: &str) -> PathBuf {
    let ts = Local::now().format("%Y%m%d_%H%M%S").to_string();
    let slug = safe_slug(task, 48);
    let base = default_runs_root().join(format!("{ts}_{slug}"));
    if !base.exists() {
        return base;
    }
    for suffix in 2u32.. {
        let candidate = base.with_file_name(format!(
            "{}_{}",
            base.file_name().and_then(|s| s.to_str()).unwrap_or("run"),
            suffix
        ));
        if !candidate.exists() {
            return candidate;
        }
    }
    base
}

/// Strip images / huge strings out of a value before logging it. Keeps
/// reasoning_log lean even when tool outputs are sprawling.
pub fn json_safe_for_log(value: &Value, max_string_chars: usize) -> Value {
    match value {
        Value::String(s) => {
            if s.chars().count() <= max_string_chars {
                Value::String(s.clone())
            } else {
                let kept: String = s.chars().take(max_string_chars).collect();
                Value::String(format!(
                    "{}\n... [truncated {} chars]",
                    kept,
                    s.chars().count() - max_string_chars
                ))
            }
        }
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("image") {
                return json!({
                    "type": "image",
                    "omitted": true,
                    "note": "Image data omitted from debug log; use the saved artifact path if present.",
                });
            }
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(k.clone(), json_safe_for_log(v, max_string_chars));
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(
            arr.iter()
                .map(|v| json_safe_for_log(v, max_string_chars))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn write_json(path: &Path, payload: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let safe = json_safe_for_log(payload, 100_000);
    let rendered = serde_json::to_string_pretty(&safe).map_err(std::io::Error::other)?;
    std::fs::write(path, rendered)
}

fn append_jsonl(path: &Path, entry: &Value) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let safe = json_safe_for_log(entry, 100_000);
    let line = serde_json::to_string(&safe).map_err(std::io::Error::other)?;
    let mut handle = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    handle.write_all(line.as_bytes())?;
    handle.write_all(b"\n")
}

#[derive(Debug, Clone)]
pub struct RunDebugLogger {
    pub run_dir: PathBuf,
    pub reasoning_log_path: PathBuf,
    pub tool_results_dir: PathBuf,
    pub conversation_path: PathBuf,
    pub agent_log_path: PathBuf,
}

impl RunDebugLogger {
    pub fn new(run_dir: impl AsRef<Path>) -> Self {
        let run_dir = run_dir.as_ref().to_path_buf();
        Self {
            reasoning_log_path: run_dir.join("reasoning_log.jsonl"),
            tool_results_dir: run_dir.join("tool_results"),
            conversation_path: run_dir.join("conversation.json"),
            agent_log_path: run_dir.join("agent_log.json"),
            run_dir,
        }
    }

    pub fn event(&self, event_type: &str, mut payload: Value) {
        if let Value::Object(map) = &mut payload {
            map.insert("type".into(), Value::String(event_type.to_string()));
            map.insert("timestamp".into(), Value::String(timestamp()));
        } else {
            payload = json!({"type": event_type, "timestamp": timestamp(), "value": payload});
        }
        let _ = append_jsonl(&self.reasoning_log_path, &payload);
    }

    pub fn api_error(&self, turn: u32, error: &str, forced_summary: bool) {
        let mut payload = json!({
            "turn": turn,
            "error": error,
        });
        if forced_summary {
            payload
                .as_object_mut()
                .map(|m| m.insert("forced_summary".into(), Value::Bool(true)));
        }
        self.event("api_error", payload);
    }

    pub fn tool_result(
        &self,
        turn: u32,
        sequence: u32,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &Value,
        content: &Value,
        duration_s: f64,
        result_summary: &str,
        repeat_count: u32,
        error: &str,
    ) -> String {
        let file_name = format!(
            "{:03}_{:02}_{}.json",
            turn,
            sequence,
            safe_slug(tool_name, 32)
        );
        let path = self.tool_results_dir.join(&file_name);
        let payload = json!({
            "type": "tool_result",
            "timestamp": timestamp(),
            "turn": turn,
            "sequence": sequence,
            "tool_use_id": tool_use_id,
            "tool": tool_name,
            "input": tool_input,
            "duration_s": duration_s,
            "error": error,
            "content": content,
        });
        let _ = write_json(&path, &payload);
        let relative = path
            .strip_prefix(&self.run_dir)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| file_name.clone());

        self.event(
            "tool_result",
            json!({
                "turn": turn,
                "sequence": sequence,
                "tool_use_id": tool_use_id,
                "tool": tool_name,
                "input": tool_input,
                "duration_s": duration_s,
                "result_summary": result_summary,
                "result_file": relative,
                "error": error,
                "repeat_count": repeat_count,
            }),
        );
        relative
    }

    pub fn write_conversation(&self, system_prompt: &str, messages: &Value) -> PathBuf {
        let _ = write_json(
            &self.conversation_path,
            &json!({"system": system_prompt, "messages": messages}),
        );
        self.conversation_path.clone()
    }

    pub fn write_agent_summary(&self, summary: &Value) {
        let _ = write_json(&self.agent_log_path, summary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_replaces_punctuation() {
        assert_eq!(safe_slug("hello, world!", 64), "hello_world");
    }

    #[test]
    fn slug_fallback_when_empty() {
        assert_eq!(safe_slug("!!!", 64), "agent");
    }

    #[test]
    fn json_safe_omits_images() {
        let v = json!({"type": "image", "source": {"data": "aGVsbG8="}});
        let safe = json_safe_for_log(&v, 1000);
        assert_eq!(safe.get("omitted"), Some(&Value::Bool(true)));
    }
}
