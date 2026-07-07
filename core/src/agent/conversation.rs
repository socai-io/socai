//! Persistent conversation index of agent runs.
//!
//! A conversation owns user-message order and references to L2 agent runs.
//! Assistant output is read from each run's `report.md`; an inline fallback
//! is stored only when a run did not produce a report.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::agent::llm::{Block, Message};

#[derive(Serialize, Deserialize, Clone)]
pub struct Run {
    pub user: String,
    pub run_dir: String,
    pub status: String,
    pub timestamp_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_fallback: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Conversation {
    #[serde(skip)]
    pub id: String,
    #[serde(skip)]
    pub dir: PathBuf,
    pub model: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub runs: Vec<Run>,
}

impl Conversation {
    pub fn new(model: Option<String>) -> std::io::Result<Self> {
        let now = now_ms();
        let id = format!(
            "session-{now}-{}",
            &Uuid::new_v4().simple().to_string()[..8]
        );
        Self::create_at(default_sessions_root().join(&id), model)
    }

    /// Create a conversation at an explicit directory — the desktop app puts
    /// it under the runs root (named after the first task) and nests each
    /// turn's run dir inside, so one conversation is one folder on disk.
    pub fn create_at(dir: impl Into<PathBuf>, model: Option<String>) -> std::io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        let now = now_ms();
        let conversation = Self {
            id: dir_id(&dir),
            dir,
            model,
            created_at_ms: now,
            updated_at_ms: now,
            runs: Vec::new(),
        };
        conversation.persist();
        Ok(conversation)
    }

    pub fn load(dir: impl AsRef<Path>) -> std::io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        // Conversations recorded before the conversation-run-step naming
        // unification persisted as session.json; the next persist migrates.
        let text = std::fs::read_to_string(dir.join("conversation.json"))
            .or_else(|_| std::fs::read_to_string(dir.join("session.json")))?;
        let mut conversation: Self =
            serde_json::from_str(&text).map_err(std::io::Error::other)?;
        conversation.id = dir_id(&dir);
        conversation.dir = dir;
        Ok(conversation)
    }

    /// Directory for this conversation's next run, nested inside the
    /// conversation dir: `turn-NN_<message slug>`, uniquified on collision
    /// (a turn that crashed before recording leaves its number unused).
    pub fn next_turn_dir(&self, message: &str) -> PathBuf {
        let turn = self.runs.len() + 1;
        let slug = slug_component(message, 40);
        let base = self.dir.join(format!("turn-{turn:02}_{slug}"));
        if !base.exists() {
            return base;
        }
        for suffix in 2u32.. {
            let candidate = self.dir.join(format!("turn-{turn:02}_{slug}_{suffix}"));
            if !candidate.exists() {
                return candidate;
            }
        }
        base
    }

    pub fn record_run(&mut self, user: &str, assistant: &str, run_dir: &Path, status: &str) {
        let assistant_fallback = (!run_dir.join("report.md").is_file())
            .then(|| assistant.to_string())
            .filter(|value| !value.is_empty());
        self.runs.push(Run {
            user: user.to_string(),
            run_dir: run_dir.to_string_lossy().to_string(),
            status: status.to_string(),
            timestamp_ms: now_ms(),
            assistant_fallback,
        });
        self.updated_at_ms = now_ms();
        self.persist();
    }

    pub fn chat_messages(&self) -> Vec<Message> {
        let mut messages = Vec::with_capacity(self.runs.len() * 2);
        for run in &self.runs {
            messages.push(Message::user(run.user.clone()));
            let report = std::fs::read_to_string(Path::new(&run.run_dir).join("report.md"))
                .ok()
                .or_else(|| run.assistant_fallback.clone())
                .unwrap_or_default();
            // Providers reject empty text blocks, so a run that produced
            // neither a report nor a fallback seeds a placeholder instead.
            let report = if report.trim().is_empty() {
                "(no answer was recorded for this run)".to_string()
            } else {
                report
            };
            messages.push(Message::assistant_blocks(vec![Block::Text {
                text: report,
            }]));
        }
        messages
    }

    pub fn context_note(&self) -> String {
        let mut lines = vec![format!(
            "This is an ongoing chat conversation (conversation dir: {}). Earlier runs \
             have their full records in these run dirs:",
            self.dir.display()
        )];
        for (index, run) in self.runs.iter().enumerate() {
            lines.push(format!(
                "  {}. {} — {}",
                index + 1,
                run.run_dir,
                preview(&run.user)
            ));
            let notes = notes_line(Path::new(&run.run_dir));
            if !notes.is_empty() {
                lines.push(format!("     notes read: {notes}"));
            }
        }
        if !self.runs.is_empty() {
            lines.push(
                "Each run dir contains report.md (that run's final answer), notes.json \
                 (full data for every note listed above), and tools/*/ raw tool outputs \
                 (full note text, comments, and per-image OCR text where captured). \
                 These local files can often answer follow-ups about already-gathered \
                 content without re-fetching from the site."
                    .to_string(),
            );
        }
        lines.join("\n")
    }

    fn persist(&self) {
        if let Ok(text) = serde_json::to_string_pretty(self) {
            match std::fs::write(self.dir.join("conversation.json"), text) {
                // Drop a pre-unification session.json so the dir keeps a
                // single source of truth once the new file exists.
                Ok(()) => {
                    let _ = std::fs::remove_file(self.dir.join("session.json"));
                }
                Err(error) => {
                    tracing::warn!(%error, dir = %self.dir.display(), "failed to persist conversation.json");
                }
            }
        }
    }
}

fn dir_id(dir: &Path) -> String {
    dir.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("session")
        .to_string()
}

// The directory root and env var keep their historical "session" naming —
// renaming them would orphan every conversation already saved under
// `~/.socai/sessions` on users' machines. The per-conversation file is
// `conversation.json` (with a `session.json` read fallback for old dirs).
pub fn default_sessions_root() -> PathBuf {
    if let Ok(path) = std::env::var("SOCAI_SESSIONS_DIR") {
        return PathBuf::from(path);
    }
    if let Ok(home) = std::env::var("SOCAI_HOME") {
        return PathBuf::from(home).join("sessions");
    }
    dirs::home_dir()
        .map(|home| home.join(".socai/sessions"))
        .unwrap_or_else(|| PathBuf::from(".socai/sessions"))
}

/// `note_id «title»` pairs from a run's notes.json, capped so the context
/// note stays bounded even for large scans.
fn notes_line(run_dir: &Path) -> String {
    const MAX_LISTED: usize = 12;
    let notes = crate::agent::note_store::load_notes(run_dir);
    if notes.is_empty() {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::new();
    for note in notes.iter().take(MAX_LISTED) {
        let id = note.get("note_id").and_then(Value::as_str).unwrap_or("");
        if id.is_empty() {
            continue;
        }
        match note.get("title").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
            Some(title) => parts.push(format!("{id} «{}»", preview(title))),
            None => parts.push(id.to_string()),
        }
    }
    if notes.len() > MAX_LISTED {
        parts.push(format!("… and {} more in notes.json", notes.len() - MAX_LISTED));
    }
    parts.join("; ")
}

fn slug_component(value: &str, max_chars: usize) -> String {
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
        "message".to_string()
    } else {
        trimmed.chars().take(max_chars).collect()
    }
}

fn preview(text: &str) -> String {
    let line = text.trim().lines().next().unwrap_or("").trim();
    if line.chars().count() > 60 {
        format!("{}…", line.chars().take(60).collect::<String>())
    } else {
        line.to_string()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}
