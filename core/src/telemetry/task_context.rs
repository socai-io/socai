//! Explicit task context supplied by external agents. No host transcript discovery.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::redact_secrets;

pub const MAX_INPUT_BYTES: u64 = 128 * 1024;

fn unknown_host() -> String {
    "unknown".into()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskContextInput {
    /// Null when the agent no longer has the user's actual words.
    pub user_prompt: Option<String>,
    #[serde(default = "unknown_host")]
    pub agent_host: String,
}

/// Bounded, scrubbed payload, created by the short-lived CLI before IPC.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRegistration {
    pub task_id: String,
    pub agent_host: String,
    pub user_prompt: Option<String>,
    pub task_text_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskLink {
    pub task_id: String,
    pub agent_host: String,
    pub task_context_status: String,
}

pub fn validate_task_id(id: &str) -> Result<String> {
    let parsed =
        uuid::Uuid::parse_str(id).context("task ID must be a UUID returned by socai task begin")?;
    Ok(parsed.to_string())
}

fn scrub(text: Option<String>, limit: usize) -> (Option<String>, bool) {
    let Some(text) = text else {
        return (None, false);
    };
    let text = redact_secrets(&text);
    let text = text.trim();
    if text.is_empty() {
        return (None, false);
    }
    (
        Some(text.chars().take(limit).collect()),
        text.chars().count() > limit,
    )
}

impl TaskRegistration {
    pub fn new(input: TaskContextInput, include_text: bool) -> Result<Self> {
        let mut registration = Self {
            task_id: uuid::Uuid::new_v4().to_string(),
            agent_host: input.agent_host,
            user_prompt: input.user_prompt,
            task_text_truncated: false,
        };
        registration.sanitize(include_text)?;
        Ok(registration)
    }

    /// Apply request-level controls again at the daemon boundary.
    pub fn sanitize(&mut self, include_text: bool) -> Result<()> {
        self.task_id = validate_task_id(&self.task_id)?;
        let host = self.agent_host.trim();
        if host.is_empty()
            || host.len() > 80
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        {
            bail!(
                "agent_host must be a short identifier, such as workbuddy, claude-code, or unknown"
            );
        }
        self.agent_host = host.to_ascii_lowercase();
        if !include_text {
            self.user_prompt = None;
            self.task_text_truncated = false;
        }
        let (prompt, truncated) = scrub(self.user_prompt.take(), 8_000);
        self.user_prompt = prompt;
        self.task_text_truncated |= truncated;
        Ok(())
    }

    pub fn link(&self, include_text: bool) -> TaskLink {
        TaskLink {
            task_id: self.task_id.clone(),
            agent_host: self.agent_host.clone(),
            task_context_status: if !include_text {
                "disabled"
            } else if self.user_prompt.is_some() {
                "provided"
            } else {
                "missing"
            }
            .into(),
        }
    }

    pub fn event_properties(&self, include_text: bool) -> Value {
        let mut properties = self.link(include_text).properties(include_text);
        properties.insert("task_context_schema_version".into(), json!(1));
        if include_text {
            if let Some(prompt) = &self.user_prompt {
                properties.insert("task_text".into(), json!(prompt));
                properties.insert(
                    "task_text_truncated".into(),
                    json!(self.task_text_truncated),
                );
            }
        }
        Value::Object(properties)
    }
}

impl TaskLink {
    pub fn properties(&self, include_text: bool) -> Map<String, Value> {
        let mut props = Map::new();
        props.insert("task_id".into(), json!(self.task_id));
        props.insert("agent_host".into(), json!(self.agent_host));
        props.insert("capture_method".into(), json!("agent_reported"));
        props.insert(
            "task_context_status".into(),
            json!(if include_text {
                self.task_context_status.as_str()
            } else {
                "disabled"
            }),
        );
        props
    }
}

/// Snapshot the daemon's current task when a site command is admitted.
/// No task text is retained in daemon state or repeated in tool events.
pub fn correlation_properties(
    current_task: Option<&TaskLink>,
    include_text: bool,
) -> Map<String, Value> {
    match current_task {
        Some(task) => task.properties(include_text),
        None => Map::from_iter([(
            "task_context_status".into(),
            json!(if include_text {
                "not_provided"
            } else {
                "disabled"
            }),
        )]),
    }
}
