//! Minimal persistence for agent runs and tool calls.
//!
//! Each fact has one owner:
//! - `run.json` owns agent-run metadata and aggregate usage.
//! - `llm/NNN.request.json` and `llm/NNN.response.json` own exact LLM I/O.
//! - `tools/step-NNN-call-NN-name/tool.json` owns tool input and lifecycle.
//! - tool `output.json` owns the raw tool result.
//! - standalone tool `tool.json` additionally owns its input because there is
//!   no parent LLM response.

#![allow(clippy::expect_used)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use chrono::{Local, Utc};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent::llm::LLMResponse;
use crate::agent::tool::ToolResultBlock;

fn timestamp() -> String {
    Utc::now().to_rfc3339()
}

fn safe_component(value: &str, fallback: &str) -> String {
    let cleaned: String = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.chars().take(48).collect()
    }
}

fn write_json_atomic(path: &Path, value: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("record.json");
    let temp = path.with_file_name(format!(".{name}.{}.tmp", Uuid::new_v4()));
    std::fs::write(&temp, bytes)?;
    if std::fs::rename(&temp, path).is_err() {
        let _ = std::fs::remove_file(path);
        std::fs::rename(temp, path)?;
    }
    Ok(())
}

fn env_runs_root(value: Option<OsString>) -> Option<PathBuf> {
    let value = value?;
    let value = value.to_string_lossy().trim().to_string();
    (!value.is_empty()).then(|| PathBuf::from(value))
}

fn default_runs_root_from_parts(
    env_root: Option<OsString>,
    config_root: Option<PathBuf>,
    home: Option<PathBuf>,
) -> PathBuf {
    env_runs_root(env_root)
        .or(config_root)
        .or_else(|| home.map(|path| path.join(".socai/runs")))
        .unwrap_or_else(|| PathBuf::from(".socai/runs"))
}

/// Root for generated run directories. Precedence: `SOCAI_RUNS_DIR`, then
/// `socai config set runs.dir <path>`, then `~/.socai/runs`.
pub fn default_runs_root() -> PathBuf {
    if let Some(root) = env_runs_root(std::env::var_os("SOCAI_RUNS_DIR")) {
        return root;
    }
    let config_root = match crate::config::load_config() {
        Ok(config) => config.runs_root(),
        Err(error) => {
            tracing::warn!(%error, "failed to load runs.dir; using the default");
            None
        }
    };
    default_runs_root_from_parts(None, config_root, dirs::home_dir())
}

/// Allocate `<timestamp>_<label>`, adding a numeric suffix on collision.
pub fn make_run_dir(label: &str) -> PathBuf {
    make_run_dir_in_root(default_runs_root(), label)
}

fn make_run_dir_in_root(root: impl AsRef<Path>, label: &str) -> PathBuf {
    let base = root.as_ref().join(format!(
        "{}_{}",
        Local::now().format("%Y%m%d_%H%M%S"),
        safe_component(label, "run")
    ));
    if !base.exists() {
        return base;
    }
    for suffix in 2u32.. {
        let candidate = base.with_file_name(format!(
            "{}_{suffix}",
            base.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("run")
        ));
        if !candidate.exists() {
            return candidate;
        }
    }
    base
}

pub struct AgentRunRecorder {
    run_dir: PathBuf,
    manifest_path: PathBuf,
    manifest: Mutex<Value>,
    started: Instant,
    finalized: AtomicBool,
}

impl AgentRunRecorder {
    pub fn start(
        run_dir: impl AsRef<Path>,
        run_id: &str,
        session_id: Option<&str>,
        task: &str,
        model: &str,
    ) -> std::io::Result<Self> {
        let run_dir = run_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(run_dir.join("llm"))?;
        let manifest_path = run_dir.join("run.json");
        let mut manifest = json!({
            "id": run_id,
            "task": task,
            "model": model,
            "status": "running",
            "started_at": timestamp(),
        });
        if let Some(session_id) = session_id {
            manifest["session_id"] = json!(session_id);
        }
        write_json_atomic(&manifest_path, &manifest)?;
        Ok(Self {
            run_dir,
            manifest_path,
            manifest: Mutex::new(manifest),
            started: Instant::now(),
            finalized: AtomicBool::new(false),
        })
    }

    pub fn record_llm_request(&self, step: u32, payload: &Value) -> std::io::Result<()> {
        write_json_atomic(
            &self.run_dir.join(format!("llm/{step:03}.request.json")),
            payload,
        )
    }

    pub fn record_llm_response(
        &self,
        step: u32,
        response: &LLMResponse,
        duration_ms: u64,
    ) -> std::io::Result<()> {
        let mut value = serde_json::to_value(response).map_err(std::io::Error::other)?;
        value["duration_ms"] = json!(duration_ms);
        value["completed_at"] = json!(timestamp());
        write_json_atomic(
            &self.run_dir.join(format!("llm/{step:03}.response.json")),
            &value,
        )?;
        // Keep running usage/step totals in run.json so the desktop app can
        // show them live mid-run; `finish` overwrites with the final figures.
        {
            let mut manifest = self.manifest.lock().expect("poisoned");
            let input = manifest
                .pointer("/usage/input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                + response.input_tokens;
            let output = manifest
                .pointer("/usage/output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                + response.output_tokens;
            manifest["usage"] = json!({
                "input_tokens": input,
                "output_tokens": output,
            });
            manifest["steps"] = json!(step);
            write_json_atomic(&self.manifest_path, &manifest)?;
        }
        Ok(())
    }

    pub fn record_llm_error(
        &self,
        step: u32,
        error: &str,
        duration_ms: u64,
    ) -> std::io::Result<()> {
        write_json_atomic(
            &self.run_dir.join(format!("llm/{step:03}.response.json")),
            &json!({
                "duration_ms": duration_ms,
                "completed_at": timestamp(),
                "error": error,
            }),
        )
    }

    pub fn start_tool_call(
        &self,
        step: u32,
        sequence: u32,
        tool_name: &str,
        input: &Value,
    ) -> std::io::Result<ToolCallRecorder> {
        let dir = self.run_dir.join("tools").join(format!(
            "step-{step:03}-call-{sequence:02}-{}",
            safe_component(tool_name, "tool")
        ));
        ToolCallRecorder::start_agent(dir, tool_name, input)
    }

    pub fn finish(
        &self,
        status: &str,
        steps: u32,
        input_tokens: u64,
        output_tokens: u64,
        error: Option<&str>,
    ) -> std::io::Result<()> {
        let mut manifest = self.manifest.lock().expect("poisoned");
        manifest["status"] = json!(status);
        manifest["duration_ms"] = json!(self.started.elapsed().as_millis() as u64);
        manifest["steps"] = json!(steps);
        manifest["usage"] = json!({
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
        });
        if let Some(error) = error {
            manifest["error"] = json!(error);
        }
        write_json_atomic(&self.manifest_path, &manifest)?;
        self.finalized.store(true, Ordering::Release);
        Ok(())
    }
}

impl Drop for AgentRunRecorder {
    fn drop(&mut self) {
        if self.finalized.load(Ordering::Acquire) {
            return;
        }
        if let Ok(mut manifest) = self.manifest.lock() {
            manifest["status"] = json!("interrupted");
            manifest["duration_ms"] = json!(self.started.elapsed().as_millis() as u64);
            let _ = write_json_atomic(&self.manifest_path, &manifest);
        }
    }
}

pub struct ToolCallRecorder {
    tool_dir: PathBuf,
    manifest_path: PathBuf,
    manifest: Mutex<Value>,
    started: Instant,
    finalized: AtomicBool,
}

impl ToolCallRecorder {
    fn start_agent(tool_dir: PathBuf, tool_name: &str, input: &Value) -> std::io::Result<Self> {
        Self::start(
            tool_dir,
            json!({
                "tool": tool_name,
                "input": input,
                "status": "running",
                "started_at": timestamp(),
            }),
        )
    }

    pub fn start_standalone(
        tool_dir: impl AsRef<Path>,
        site_id: &str,
        command_name: &str,
        implementation_name: &str,
        input: &Value,
    ) -> std::io::Result<Self> {
        let public_name = format!("{site_id}.{command_name}");
        let mut manifest = json!({
            "tool": public_name,
            "input": input,
            "status": "running",
            "started_at": timestamp(),
        });
        if implementation_name != command_name {
            manifest["implementation"] = json!(implementation_name);
        }
        Self::start(tool_dir.as_ref().to_path_buf(), manifest)
    }

    fn start(tool_dir: PathBuf, manifest: Value) -> std::io::Result<Self> {
        std::fs::create_dir_all(&tool_dir)?;
        let manifest_path = tool_dir.join("tool.json");
        write_json_atomic(&manifest_path, &manifest)?;
        Ok(Self {
            tool_dir,
            manifest_path,
            manifest: Mutex::new(manifest),
            started: Instant::now(),
            finalized: AtomicBool::new(false),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.tool_dir
    }

    pub fn finish_blocks(
        &self,
        blocks: &[ToolResultBlock],
        duration_ms: u64,
        error: Option<&str>,
    ) -> std::io::Result<()> {
        self.finish_output(
            &serde_json::to_value(blocks).map_err(std::io::Error::other)?,
            duration_ms,
            error,
        )
    }

    pub fn finish_value(
        &self,
        output: &Value,
        duration_ms: u64,
        error: Option<&str>,
    ) -> std::io::Result<()> {
        self.finish_output(output, duration_ms, error)
    }

    pub fn finish_error(&self, duration_ms: u64, error: &str) -> std::io::Result<()> {
        self.finish_manifest(duration_ms, Some(error))
    }

    fn finish_output(
        &self,
        output: &Value,
        duration_ms: u64,
        error: Option<&str>,
    ) -> std::io::Result<()> {
        write_json_atomic(&self.tool_dir.join("output.json"), output)?;
        self.finish_manifest(duration_ms, error)
    }

    fn finish_manifest(&self, duration_ms: u64, error: Option<&str>) -> std::io::Result<()> {
        let mut manifest = self.manifest.lock().expect("poisoned");
        manifest["status"] = json!(if error.is_some() {
            "failed"
        } else {
            "completed"
        });
        manifest["duration_ms"] = json!(duration_ms);
        if let Some(error) = error {
            manifest["error"] = json!(error);
        }
        write_json_atomic(&self.manifest_path, &manifest)?;
        self.finalized.store(true, Ordering::Release);
        Ok(())
    }
}

impl Drop for ToolCallRecorder {
    fn drop(&mut self) {
        if self.finalized.load(Ordering::Acquire) {
            return;
        }
        if let Ok(mut manifest) = self.manifest.lock() {
            manifest["status"] = json!("interrupted");
            manifest["duration_ms"] = json!(self.started.elapsed().as_millis() as u64);
            let _ = write_json_atomic(&self.manifest_path, &manifest);
        }
    }
}

/// Best-effort terminal update when an entrypoint cancels the agent future.
pub fn mark_agent_run_status(
    run_dir: impl AsRef<Path>,
    status: &str,
    error: Option<&str>,
) -> std::io::Result<()> {
    let path = run_dir.as_ref().join("run.json");
    let mut manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(&path)?).map_err(std::io::Error::other)?;
    let duration_ms = manifest
        .get("started_at")
        .and_then(Value::as_str)
        .and_then(|started| chrono::DateTime::parse_from_rfc3339(started).ok())
        .map(|started| {
            (Utc::now() - started.with_timezone(&Utc))
                .num_milliseconds()
                .max(0) as u64
        });
    manifest["status"] = json!(status);
    if let Some(duration_ms) = duration_ms {
        manifest["duration_ms"] = json!(duration_ms);
    }
    if let Some(error) = error {
        manifest["error"] = json!(error);
    }
    write_json_atomic(&path, &manifest)
}
