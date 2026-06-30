//! Persistent multi-turn conversation index.
//!
//! A session owns user-message order and references to L2 agent runs. Assistant
//! output is read from each run's `report.md`; an inline fallback is stored
//! only when a run did not produce a report.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent::llm::{Block, Message};

#[derive(Serialize, Deserialize, Clone)]
pub struct Turn {
    pub user: String,
    pub run_dir: String,
    pub status: String,
    pub timestamp_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_fallback: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Session {
    #[serde(skip)]
    pub id: String,
    #[serde(skip)]
    pub dir: PathBuf,
    pub model: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub turns: Vec<Turn>,
}

impl Session {
    pub fn new(model: Option<String>) -> std::io::Result<Self> {
        let now = now_ms();
        let id = format!(
            "session-{now}-{}",
            &Uuid::new_v4().simple().to_string()[..8]
        );
        let dir = default_sessions_root().join(&id);
        std::fs::create_dir_all(&dir)?;
        let session = Self {
            id,
            dir,
            model,
            created_at_ms: now,
            updated_at_ms: now,
            turns: Vec::new(),
        };
        session.persist();
        Ok(session)
    }

    pub fn load(dir: impl AsRef<Path>) -> std::io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let mut session: Self =
            serde_json::from_str(&std::fs::read_to_string(dir.join("session.json"))?)
                .map_err(std::io::Error::other)?;
        session.id = dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("session")
            .to_string();
        session.dir = dir;
        Ok(session)
    }

    pub fn record_turn(&mut self, user: &str, assistant: &str, run_dir: &Path) {
        self.record_run(user, assistant, run_dir, "completed");
    }

    pub fn record_run(&mut self, user: &str, assistant: &str, run_dir: &Path, status: &str) {
        let assistant_fallback = (!run_dir.join("report.md").is_file())
            .then(|| assistant.to_string())
            .filter(|value| !value.is_empty());
        self.turns.push(Turn {
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
        let mut messages = Vec::with_capacity(self.turns.len() * 2);
        for turn in &self.turns {
            messages.push(Message::user(turn.user.clone()));
            let report = std::fs::read_to_string(Path::new(&turn.run_dir).join("report.md"))
                .ok()
                .or_else(|| turn.assistant_fallback.clone())
                .unwrap_or_default();
            messages.push(Message::assistant_blocks(vec![Block::Text {
                text: report,
            }]));
        }
        messages
    }

    pub fn context_note(&self) -> String {
        let mut lines = vec![format!(
            "This is an ongoing chat session (session dir: {}). Earlier turns have \
             their full records in these run dirs:",
            self.dir.display()
        )];
        for (index, turn) in self.turns.iter().enumerate() {
            lines.push(format!(
                "  {}. {} — {}",
                index + 1,
                turn.run_dir,
                preview(&turn.user)
            ));
        }
        lines.join("\n")
    }

    fn persist(&self) {
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(self.dir.join("session.json"), text);
        }
    }
}

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
