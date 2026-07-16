//! Deterministic, artifact-first context compaction for long agent runs.
//!
//! Keep a growing tail of full messages so the provider can reuse its prompt
//! cache. Once that tail reaches its limit, replace only the older tool
//! results with durable evidence locators (post/author id, title, artifact
//! path), then start growing the tail again.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::agent::llm::{Block, Message, MessageContent, ToolResultContent};

pub const DEFAULT_COMPACT_AFTER_MESSAGES: usize = 20;
pub const DEFAULT_KEEP_RECENT_MESSAGES: usize = 10;
const EVIDENCE_HEADING: &str = "# Earlier tool evidence";

/// Rewrite the transcript only when it has grown beyond `compact_after` full
/// messages. The original first message remains, the last `keep_recent`
/// messages remain verbatim, and the older tool outputs become artifact
/// locators. Mutating the transcript, rather than rebuilding a summary for
/// every request, leaves the request prefix stable until the next sawtooth
/// compaction point and therefore friendly to provider prompt caches.
pub fn compact_messages_for_context(
    messages: &mut Vec<Message>,
    compact_after: usize,
    keep_recent: usize,
) -> bool {
    if compact_after == 0
        || keep_recent == 0
        || keep_recent >= compact_after
        || messages.len() <= compact_after
    {
        return false;
    }

    let recent_start = messages.len() - keep_recent;
    let original = messages[0].clone();
    let older = &messages[1..recent_start];
    let recent = messages[recent_start..].to_vec();
    let evidence = compact_older_messages(older);

    let mut compacted = Vec::with_capacity(2 + recent.len());
    compacted.push(original);
    if !evidence.is_empty() {
        compacted.push(Message::user(evidence));
    }
    compacted.extend(recent);
    *messages = compacted;
    true
}

fn compact_older_messages(messages: &[Message]) -> String {
    let mut inherited = Vec::new();
    let mut artifacts: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();

    for message in messages {
        if let MessageContent::Text(text) = &message.content {
            if text.starts_with(EVIDENCE_HEADING) {
                inherited.push(text.trim().to_string());
            }
            continue;
        }
        let MessageContent::Blocks(blocks) = &message.content else {
            continue;
        };
        for block in blocks {
            let Block::ToolResult { content, .. } = block else {
                continue;
            };
            for item in content {
                let ToolResultContent::Text { text } = item else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<Value>(text) else {
                    continue;
                };
                collect_artifact_evidence(&value, &mut artifacts);
            }
        }
    }

    let mut sections = inherited;
    if !artifacts.is_empty() {
        let mut rendered = String::from(EVIDENCE_HEADING);
        rendered.push_str("\nFull data is available in the listed artifacts.\n");
        for (path, entities) in artifacts {
            rendered.push_str(&format!("\n## Artifact: {path}\n"));
            for (id, title) in entities {
                if title.is_empty() {
                    rendered.push_str(&format!("- {id}\n"));
                } else {
                    rendered.push_str(&format!("- {id} — {title}\n"));
                }
            }
        }
        sections.push(rendered);
    }
    sections.join("\n\n")
}

fn collect_artifact_evidence(
    value: &Value,
    artifacts: &mut BTreeMap<String, BTreeSet<(String, String)>>,
) {
    let Some(path) = value
        .pointer("/artifact/path")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
    else {
        return;
    };
    let entities = artifacts.entry(path.to_string()).or_default();

    if let Some(author_id) = value.get("author_id").and_then(Value::as_str) {
        let title = value
            .pointer("/profile/nickname")
            .or_else(|| value.pointer("/profile/display_name"))
            .or_else(|| value.pointer("/profile/name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        entities.insert((format!("author:{author_id}"), title.to_string()));
    }

    for key in ["notes", "cards"] {
        let Some(items) = value.get(key).and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            let entity = item.get("entity").unwrap_or(item);
            let id = entity
                .get("note_id")
                .or_else(|| entity.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let title = entity.get("title").and_then(Value::as_str).unwrap_or("");
            if !id.is_empty() || !title.is_empty() {
                entities.insert((id.to_string(), title.to_string()));
            }
        }
    }
}
