//! Tool trait + ToolContext.
//!
//! Each tool advertises a name, description, and JSON Schema for its input,
//! plus an async `call` that returns either text or mixed content
//! (text + images) back to the model. Tools own their own state — the
//! `ToolContext` is for *shared* per-run state (counters, run-state handle,
//! enabled sites for gating, …).

// Same rationale as run_state.rs: lock-poisoned panics are fatal.
#![allow(clippy::expect_used)]
// write_json_artifact takes label + payload + 4 metadata fields — see also
// the matching helper in `run_state.rs`.
#![allow(clippy::too_many_arguments)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::agent::run_state::RunState;

/// Machine-readable progress emitted by long-running tools. Core code reports
/// state only; entrypoints decide whether and how to render it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolProgressPhase {
    Reading,
    Ocr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolProgressStatus {
    ItemStarted,
    ItemCompleted,
    Finished,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolProgressEvent {
    pub phase: ToolProgressPhase,
    pub status: ToolProgressStatus,
    /// Completed items in this phase.
    pub current: u64,
    /// Current target. A final event may lower this when a feed ends early.
    pub total: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

pub type ToolProgressSender = mpsc::UnboundedSender<ToolProgressEvent>;

/// Content block returned by a tool. Mirrors the subset of Anthropic's
/// content blocks we actually use.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultBlock {
    Text {
        text: String,
    },
    Image {
        /// Base64-encoded image bytes.
        data: String,
        /// IANA media type, e.g. "image/png".
        media_type: String,
    },
}

impl ToolResultBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    pub fn image_png(data: impl Into<String>) -> Self {
        Self::Image {
            data: data.into(),
            media_type: "image/png".into(),
        }
    }

    pub fn as_text(&self) -> String {
        match self {
            Self::Text { text } => text.clone(),
            Self::Image { .. } => "[image omitted]".into(),
        }
    }
}

/// What a tool's `call` returns.
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub blocks: Vec<ToolResultBlock>,
}

impl ToolResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            blocks: vec![ToolResultBlock::text(text)],
        }
    }

    pub fn blocks(blocks: Vec<ToolResultBlock>) -> Self {
        Self { blocks }
    }

    pub fn flat_text(&self) -> String {
        self.blocks
            .iter()
            .map(|b| b.as_text())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    pub fn has_image(&self) -> bool {
        self.blocks
            .iter()
            .any(|b| matches!(b, ToolResultBlock::Image { .. }))
    }
}

impl From<String> for ToolResult {
    fn from(value: String) -> Self {
        ToolResult::text(value)
    }
}

impl From<&str> for ToolResult {
    fn from(value: &str) -> Self {
        ToolResult::text(value.to_string())
    }
}

/// Per-run shared context. Counters, dedup tables, and the run-state handle
/// live in `Arc<Mutex>` so tools can clone the context and still cooperate.
#[derive(Clone)]
pub struct ToolContext {
    pub run_id: String,
    pub run_dir: PathBuf,
    pub step: u32,
    pub active_tool_name: String,
    pub run_state: Option<Arc<RunState>>,
    tool_dir: Option<PathBuf>,
    pub enabled_sites: Arc<Mutex<BTreeSet<String>>>,
    progress: Option<ToolProgressSender>,
    counters: Arc<Mutex<Counters>>,
    /// Notes the agent has already processed at a given level. Used by
    /// macros like `search` to short-circuit repeated reads of the
    /// same note. Keyed by note id; value is the processed level
    /// ("deep" / "lite") and whether media was included.
    processed_notes: Arc<Mutex<BTreeMap<String, ProcessedNote>>>,
    /// Note ids the agent has sampled via `search` in this run — useful
    /// for "show me what I've already covered" tools.
    search_note_ids: Arc<Mutex<Vec<String>>>,
    /// Notes the agent fully read this run, in the order they were first
    /// recorded (the order the tool processed them — for `search`, result
    /// order). Archived to `<run_dir>/notes.json` so the desktop app can
    /// render them as rich, locally-served cards without re-fetching, in the
    /// same order everywhere. The record shape is built by the site tools;
    /// see [`crate::agent::note_store`].
    notes_seen: Arc<Mutex<Vec<(String, Value)>>>,
}

#[derive(Default)]
struct Counters {
    screenshot: u32,
    artifact: u32,
}

#[derive(Debug, Clone)]
pub struct ProcessedNote {
    pub level: String,
    pub include_media: bool,
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("run_id", &self.run_id)
            .field("run_dir", &self.run_dir)
            .field("step", &self.step)
            .field("active_tool_name", &self.active_tool_name)
            .field("has_run_state", &self.run_state.is_some())
            .finish()
    }
}

impl ToolContext {
    pub fn new(run_id: impl Into<String>, run_dir: impl AsRef<Path>) -> Self {
        Self {
            run_id: run_id.into(),
            run_dir: run_dir.as_ref().to_path_buf(),
            step: 0,
            active_tool_name: String::new(),
            run_state: None,
            tool_dir: None,
            enabled_sites: Arc::new(Mutex::new(BTreeSet::new())),
            progress: None,
            counters: Arc::new(Mutex::new(Counters::default())),
            processed_notes: Arc::new(Mutex::new(BTreeMap::new())),
            search_note_ids: Arc::new(Mutex::new(Vec::new())),
            notes_seen: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn with_progress_sender(mut self, progress: Option<ToolProgressSender>) -> Self {
        self.progress = progress;
        self
    }

    /// Best-effort progress delivery. A closed receiver must never fail the
    /// underlying tool call.
    pub fn report_progress(&self, event: ToolProgressEvent) {
        if let Some(progress) = &self.progress {
            let _ = progress.send(event);
        }
    }

    /// Mark a note as processed. Subsequent calls to `has_processed_note_at_level`
    /// for the same note id at the same-or-lower depth will short-circuit.
    pub fn mark_processed_note(&self, note_id: &str, level: &str, include_media: bool) {
        if note_id.is_empty() {
            return;
        }
        if let Ok(mut guard) = self.processed_notes.lock() {
            let new_rank = depth_rank(level);
            match guard.get_mut(note_id) {
                Some(prev) if depth_rank(&prev.level) > new_rank => {}
                Some(prev) if depth_rank(&prev.level) == new_rank => {
                    prev.include_media |= include_media;
                }
                Some(prev) => {
                    prev.level = level.to_string();
                    prev.include_media = include_media;
                }
                None => {
                    guard.insert(
                        note_id.to_string(),
                        ProcessedNote {
                            level: level.to_string(),
                            include_media,
                        },
                    );
                }
            }
        }
    }

    /// `true` when the note has already been processed at `requested_level`
    /// or deeper. "deep" is considered strictly deeper than "lite".
    pub fn has_processed_note_at_level(&self, note_id: &str, requested_level: &str) -> bool {
        self.has_processed_note(note_id, requested_level, false)
    }

    /// `true` when the note has already been processed at `requested_level`
    /// or deeper, and media requirements have been satisfied.
    pub fn has_processed_note(
        &self,
        note_id: &str,
        requested_level: &str,
        requested_include_media: bool,
    ) -> bool {
        if note_id.is_empty() {
            return false;
        }
        let Ok(guard) = self.processed_notes.lock() else {
            return false;
        };
        let Some(prev) = guard.get(note_id) else {
            return false;
        };
        depth_rank(&prev.level) >= depth_rank(requested_level)
            && (!requested_include_media || prev.include_media)
    }

    /// Append note ids to the search history (de-duped, preserving order).
    pub fn add_search_note_ids(&self, ids: &[String]) {
        let Ok(mut guard) = self.search_note_ids.lock() else {
            return;
        };
        for id in ids {
            if id.is_empty() {
                continue;
            }
            if !guard.iter().any(|existing| existing == id) {
                guard.push(id.clone());
            }
        }
    }

    pub fn search_note_ids(&self) -> Vec<String> {
        self.search_note_ids
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// Record a fully-read note (full content + resolved local media) into the
    /// run's note archive (`<run_dir>/notes.json`). Re-recording the same
    /// `note_id` overwrites the prior entry (latest read wins) but keeps its
    /// original position, so the archive stays in first-recorded order. No-op
    /// on empty id. The record shape is built by the caller (site tools).
    pub fn record_note(&self, note_id: &str, record: Value) {
        if note_id.is_empty() {
            return;
        }
        let Ok(mut guard) = self.notes_seen.lock() else {
            return;
        };
        match guard.iter_mut().find(|(id, _)| id == note_id) {
            Some((_, existing)) => *existing = record,
            None => guard.push((note_id.to_string(), record)),
        }
        // Deliberately write while holding the lock: it keeps on-disk snapshots
        // in insertion order (clone-then-write lets an older snapshot land after
        // a newer one, dropping notes from disk until the next write). Tool
        // calls run sequentially within a run and nothing else locks the
        // archive, so there is no contention to relieve.
        if let Err(err) = crate::agent::note_store::write_notes(&self.run_dir, &guard) {
            // Non-fatal: the in-memory copy still serves this process; only the
            // on-disk archive the desktop app reads is affected.
            tracing::warn!(error = %err, note_id, "failed to persist note archive");
        }
    }

    pub fn with_run_state(mut self, run_state: Arc<RunState>) -> Self {
        self.run_state = Some(run_state);
        self
    }

    pub fn with_tool_dir(mut self, tool_dir: impl AsRef<Path>) -> Self {
        self.tool_dir = Some(tool_dir.as_ref().to_path_buf());
        self.counters = Arc::new(Mutex::new(Counters::default()));
        self
    }

    /// Root owned by the current tool invocation. For standalone CLI calls it
    /// is the run directory itself; for agent calls it is
    /// `<run_dir>/tools/<tool-call>/`.
    pub fn output_dir(&self) -> &Path {
        self.tool_dir.as_deref().unwrap_or(&self.run_dir)
    }

    pub fn enable_site(&self, site: impl Into<String>) {
        if let Ok(mut guard) = self.enabled_sites.lock() {
            guard.insert(site.into());
        }
    }

    pub fn site_enabled(&self, site: &str) -> bool {
        self.enabled_sites
            .lock()
            .map(|g| g.contains(site))
            .unwrap_or(false)
    }

    /// Next screenshot path: `<run_dir>/NNN_<label>.png`.
    pub fn next_screenshot_path(&self, label: &str) -> PathBuf {
        let mut guard = self.counters.lock().expect("poisoned");
        guard.screenshot += 1;
        let label = sanitize_label(label, "screenshot");
        let path = self
            .output_dir()
            .join(format!("{:03}_{label}.png", guard.screenshot));
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        path
    }

    /// Next artifact path under `<run_dir>/<subdir>/NNN_<label><suffix>`.
    pub fn next_artifact_path(&self, label: &str, suffix: &str, subdir: &str) -> PathBuf {
        let mut guard = self.counters.lock().expect("poisoned");
        guard.artifact += 1;
        let label = sanitize_label(label, "artifact");
        let dir = self.output_dir().join(subdir);
        let _ = std::fs::create_dir_all(&dir);
        dir.join(format!("{:03}_{label}{suffix}", guard.artifact))
    }

    /// Register an existing on-disk artifact with the run-state registry.
    /// Returns the path *relative to* the run directory.
    pub fn register_artifact(
        &self,
        path: &Path,
        label: &str,
        kind: &str,
        summary: &str,
        metadata: Value,
        payload: Option<&Value>,
        source_tool: &str,
    ) -> String {
        let rel = path
            .strip_prefix(&self.run_dir)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string_lossy().to_string());
        if let Some(state) = &self.run_state {
            let source = if source_tool.is_empty() {
                self.active_tool_name.as_str()
            } else {
                source_tool
            };
            state.record_artifact(
                &rel,
                label,
                kind,
                source,
                Some(self.step),
                summary,
                metadata,
                payload,
            );
        }
        rel
    }

    /// Convenience: write a JSON payload to a new artifact path, then register it.
    pub fn write_json_artifact(
        &self,
        label: &str,
        payload: &Value,
        subdir: &str,
        source_tool: &str,
        artifact_kind: &str,
        summary: &str,
        metadata: Value,
    ) -> std::io::Result<String> {
        let path = self.next_artifact_path(label, ".json", subdir);
        let rendered = serde_json::to_string_pretty(payload).map_err(std::io::Error::other)?;
        std::fs::write(&path, rendered)?;
        Ok(self.register_artifact(
            &path,
            label,
            artifact_kind,
            summary,
            metadata,
            Some(payload),
            source_tool,
        ))
    }
}

fn depth_rank(level: &str) -> u8 {
    match level.to_ascii_lowercase().as_str() {
        "deep" => 2,
        "lite" => 1,
        _ => 0,
    }
}

fn sanitize_label(label: &str, fallback: &str) -> String {
    let cleaned: String = label
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let trimmed: String = cleaned.trim_matches('_').to_string();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed
    }
}

/// A tool the agent can call.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;

    /// Tools that should always be exposed regardless of `enabled_sites`.
    fn always_available(&self) -> bool {
        false
    }

    /// Empty string = no gating; otherwise the tool only appears when the
    /// matching site name has been added to `ctx.enabled_sites`.
    fn defer_until_site(&self) -> &str {
        ""
    }

    fn is_available(&self, ctx: &ToolContext) -> bool {
        if self.always_available() {
            return true;
        }
        let site = self.defer_until_site();
        if site.is_empty() {
            return true;
        }
        ctx.site_enabled(site)
    }

    /// Input after tool-specific defaults or forced runtime options are
    /// applied. This is what gets executed and persisted in `tool.json`.
    fn effective_input(&self, input: &Value) -> Value {
        input.clone()
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult>;
}

pub type SharedTool = Arc<dyn Tool>;

/// A trivial echo tool — used for testing and as a documentation example.
pub struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echo the input text back verbatim. Useful for verifying tool dispatch."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "Text to echo back" }
            },
            "required": ["text"]
        })
    }

    fn always_available(&self) -> bool {
        true
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let text = input
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        Ok(ToolResult::text(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processed_notes_require_media_when_requested() {
        let ctx = ToolContext::new("run", std::env::temp_dir());
        ctx.mark_processed_note("n1", "deep", false);

        assert!(ctx.has_processed_note("n1", "lite", false));
        assert!(ctx.has_processed_note("n1", "deep", false));
        assert!(!ctx.has_processed_note("n1", "deep", true));

        ctx.mark_processed_note("n1", "deep", true);
        assert!(ctx.has_processed_note("n1", "deep", true));
    }

    #[test]
    fn processed_notes_keep_deeper_level() {
        let ctx = ToolContext::new("run", std::env::temp_dir());
        ctx.mark_processed_note("n1", "deep", true);
        ctx.mark_processed_note("n1", "lite", false);

        assert!(ctx.has_processed_note("n1", "deep", true));
        assert!(ctx.has_processed_note_at_level("n1", "lite"));
    }
}
