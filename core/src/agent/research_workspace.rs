//! Durable, compact index for coverage-guided research runs.
//!
//! Exact tool inputs and outputs remain owned by `tools/**`. This module only
//! writes a small navigation layer under `research/` so an interrupted run,
//! the desktop app, and a human reviewer can find the brief, coverage state,
//! evidence calls, saved notes, artifacts, and final report without replaying
//! the whole transcript.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent::note_store::load_notes;
use crate::agent::run_state::RunState;

const WORKSPACE_SCHEMA_VERSION: u32 = 1;
const MAX_SUMMARY_CHARS: usize = 600;

#[derive(Debug, Clone, Copy)]
pub(crate) enum ResearchWorkspaceStatus {
    Running,
    Completed,
    Partial,
    Failed,
}

impl ResearchWorkspaceStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Partial => "partial",
            Self::Failed => "failed",
        }
    }
}

/// Converts an in-process early return or panic into a truthful workspace
/// status. Process termination can still leave `running`, which accurately
/// represents an interrupted run and can be reconciled from `run.json`.
pub(crate) struct ResearchWorkspaceStatusGuard {
    workspace_path: Option<PathBuf>,
}

impl ResearchWorkspaceStatusGuard {
    pub(crate) fn new(run_dir: &Path, active: bool) -> Self {
        Self {
            workspace_path: active.then(|| run_dir.join("research/workspace.json")),
        }
    }

    pub(crate) fn disarm(&mut self) {
        self.workspace_path = None;
    }
}

impl Drop for ResearchWorkspaceStatusGuard {
    fn drop(&mut self) {
        let Some(path) = &self.workspace_path else {
            return;
        };
        if let Err(error) = mark_workspace_failed(path) {
            tracing::warn!(%error, path = %path.display(), "failed to mark research workspace after early exit");
        }
    }
}

#[derive(Debug, Serialize)]
struct EvidenceRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    evidence_id: Option<String>,
    kind: &'static str,
    locator: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    platform: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    content_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    author: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    canonical_url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    summary: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    tool: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    status: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    source_path: String,
}

pub(crate) fn persist_research_workspace(
    run_dir: &Path,
    evidence_ids: &BTreeMap<String, String>,
    coverage_state: &Value,
    run_state: &RunState,
    status: ResearchWorkspaceStatus,
) -> std::io::Result<()> {
    let research_dir = run_dir.join("research");
    std::fs::create_dir_all(&research_dir)?;

    write_json_atomic(&research_dir.join("coverage.json"), coverage_state)?;

    let mut evidence = tool_evidence(run_dir, evidence_ids);
    evidence.extend(note_evidence(run_dir));
    evidence.extend(artifact_evidence(run_state));

    let mut jsonl = String::new();
    for record in &evidence {
        jsonl.push_str(&serde_json::to_string(record).map_err(std::io::Error::other)?);
        jsonl.push('\n');
    }
    write_text_atomic(&research_dir.join("evidence.jsonl"), &jsonl)?;
    write_text_atomic(
        &research_dir.join("sources.csv"),
        &render_sources_csv(&evidence),
    )?;

    let workspace = json!({
        "schema_version": WORKSPACE_SCHEMA_VERSION,
        "status": status.as_str(),
        "brief": "brief.json",
        "coverage": "coverage.json",
        "evidence": "evidence.jsonl",
        "sources": "sources.csv",
        "report": "../report.md",
        "report_ready": run_dir.join("report.md").is_file(),
        "notes": "../notes.json",
        "evidence_count": evidence.len(),
        "tool_evidence_count": evidence_ids.len(),
        "note_count": load_notes(run_dir).len(),
        "artifact_count": run_state.artifact_records().len(),
    });
    write_json_atomic(&research_dir.join("workspace.json"), &workspace)
}

fn tool_evidence(run_dir: &Path, evidence_ids: &BTreeMap<String, String>) -> Vec<EvidenceRecord> {
    evidence_ids
        .iter()
        .map(|(evidence_id, locator)| {
            let tool_dir = tool_dir_for_locator(run_dir, locator);
            let manifest = tool_dir
                .as_ref()
                .and_then(|dir| read_json(&dir.join("tool.json")))
                .unwrap_or(Value::Null);
            let output_path = tool_dir
                .as_ref()
                .map(|dir| dir.join("output.json"))
                .filter(|path| path.is_file());
            let source_path = output_path
                .as_ref()
                .and_then(|path| path.strip_prefix(run_dir).ok())
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            EvidenceRecord {
                evidence_id: Some(evidence_id.clone()),
                kind: "tool_result",
                locator: locator.clone(),
                platform: String::new(),
                content_id: String::new(),
                title: String::new(),
                author: String::new(),
                canonical_url: String::new(),
                summary: String::new(),
                tool: string_field(&manifest, &["tool"]),
                status: string_field(&manifest, &["status"]),
                source_path,
            }
        })
        .collect()
}

fn note_evidence(run_dir: &Path) -> Vec<EvidenceRecord> {
    load_notes(run_dir)
        .into_iter()
        .filter_map(|note| {
            let content_id = string_field(&note, &["note_id", "content_id", "id"]);
            let canonical_url =
                string_field(&note, &["canonical_url", "note_url", "url", "share_url"]);
            if content_id.is_empty() && canonical_url.is_empty() {
                return None;
            }
            let locator = if content_id.is_empty() {
                canonical_url.clone()
            } else {
                format!("note:{content_id}")
            };
            Some(EvidenceRecord {
                evidence_id: None,
                kind: "note",
                locator,
                platform: string_field(&note, &["platform", "site"]),
                content_id,
                title: string_field(&note, &["title"]),
                author: author_field(&note),
                canonical_url,
                summary: truncate(
                    &string_field(&note, &["summary", "content", "description", "text"]),
                    MAX_SUMMARY_CHARS,
                ),
                tool: String::new(),
                status: "recorded".to_string(),
                source_path: "notes.json".to_string(),
            })
        })
        .collect()
}

fn artifact_evidence(run_state: &RunState) -> Vec<EvidenceRecord> {
    run_state
        .artifact_records()
        .into_iter()
        .map(|artifact| EvidenceRecord {
            evidence_id: None,
            kind: "artifact",
            locator: format!("artifact:{}", artifact.path),
            platform: String::new(),
            content_id: String::new(),
            title: artifact.label,
            author: String::new(),
            canonical_url: String::new(),
            summary: truncate(&artifact.summary, MAX_SUMMARY_CHARS),
            tool: artifact.source_tool,
            status: "recorded".to_string(),
            source_path: artifact.path,
        })
        .collect()
}

fn tool_dir_for_locator(run_dir: &Path, locator: &str) -> Option<PathBuf> {
    let mut parts = locator.split(':');
    if parts.next()? != "tool" {
        return None;
    }
    let step = parts.next()?.parse::<u32>().ok()?;
    let sequence = parts.next()?.parse::<u32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let prefix = format!("step-{step:03}-call-{sequence:02}-");
    std::fs::read_dir(run_dir.join("tools"))
        .ok()?
        .filter_map(Result::ok)
        .find(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .map(|entry| entry.path())
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn string_field(value: &Value, keys: &[&str]) -> String {
    let Value::Object(map) = value else {
        return String::new();
    };
    keys.iter()
        .find_map(|key| map.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string()
}

fn author_field(value: &Value) -> String {
    let Value::Object(map) = value else {
        return String::new();
    };
    match map.get("author") {
        Some(Value::String(author)) => author.trim().to_string(),
        Some(Value::Object(author)) => ["name", "nickname", "display_name", "username"]
            .iter()
            .find_map(|key| author.get(*key).and_then(Value::as_str))
            .unwrap_or_default()
            .trim()
            .to_string(),
        _ => string_field(value, &["nickname", "username"]),
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut output = value.chars().take(max_chars).collect::<String>();
    output.push('…');
    output
}

fn render_sources_csv(records: &[EvidenceRecord]) -> String {
    let mut output = "evidence_id,kind,locator,platform,content_id,title,author,canonical_url,tool,status,source_path\n".to_string();
    for record in records {
        let fields = [
            record.evidence_id.as_deref().unwrap_or_default(),
            record.kind,
            &record.locator,
            &record.platform,
            &record.content_id,
            &record.title,
            &record.author,
            &record.canonical_url,
            &record.tool,
            &record.status,
            &record.source_path,
        ];
        output.push_str(&fields.map(csv_field).join(","));
        output.push('\n');
    }
    output
}

fn csv_field(value: &str) -> String {
    let value = if matches!(
        value.trim_start().chars().next(),
        Some('=' | '+' | '-' | '@')
    ) {
        format!("'{value}")
    } else {
        value.to_string()
    };
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

fn write_json_atomic(path: &Path, value: &Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    write_bytes_atomic(path, &bytes)
}

fn write_text_atomic(path: &Path, value: &str) -> std::io::Result<()> {
    write_bytes_atomic(path, value.as_bytes())
}

fn mark_workspace_failed(path: &Path) -> std::io::Result<()> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut workspace: Value = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
    let object = workspace.as_object_mut().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "research workspace manifest is not a JSON object",
        )
    })?;
    object.insert("status".to_string(), Value::String("failed".to_string()));
    write_json_atomic(path, &workspace)
}

fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("research-index");
    let temporary = path.with_file_name(format!(".{name}.{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        replace_file(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(temporary, destination)
}

#[cfg(windows)]
fn replace_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    use std::iter::once;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = temporary
        .as_os_str()
        .encode_wide()
        .chain(once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
