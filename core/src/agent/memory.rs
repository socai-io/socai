//! Deterministic, artifact-first context compaction for long agent runs.
//!
//! Keep a growing tail of full messages so the provider can reuse its prompt
//! cache. Once that tail reaches its limit, replace only the older tool
//! results with durable evidence locators (post/author id, title, artifact
//! path), then start growing the tail again.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::agent::llm::{Block, Message, MessageContent, MessageRole, ToolResultContent};

pub const DEFAULT_COMPACT_AFTER_MESSAGES: usize = 20;
pub const DEFAULT_KEEP_RECENT_MESSAGES: usize = 10;
const TURN_MARKDOWN_MAX_CHARS: usize = 2_000;
const USER_REQUEST_MAX_CHARS: usize = 500;
const COMPACT_CONTEXT_HEADING: &str = "# Earlier compacted context";
const LEGACY_EVIDENCE_HEADING: &str = "# Earlier tool evidence";

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
    let mut turns = Vec::new();
    let mut pending_user: Option<String> = None;

    for message in messages {
        match (&message.role, &message.content) {
            (MessageRole::User, MessageContent::Text(text)) => {
                if text.starts_with(COMPACT_CONTEXT_HEADING)
                    || text.starts_with(LEGACY_EVIDENCE_HEADING)
                {
                    inherited.push(text.trim().to_string());
                } else {
                    pending_user = Some(text.trim().to_string());
                }
                continue;
            }
            (MessageRole::Assistant, _) => {
                if let Some(markdown) = assistant_report_markdown(message) {
                    turns.push(compact_turn_markdown(
                        pending_user.take().as_deref(),
                        &markdown,
                    ));
                    continue;
                }
            }
            _ => {}
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

    let mut rendered = if inherited.is_empty() {
        COMPACT_CONTEXT_HEADING.to_string()
    } else {
        inherited.join("\n\n")
    };
    if !turns.is_empty() {
        rendered.push_str("\n\n## Earlier conversation turns\n");
        for (index, turn) in turns.iter().enumerate() {
            rendered.push_str(&format!("\n### Turn {}\n{}", index + 1, turn));
        }
    }
    if !artifacts.is_empty() {
        rendered.push_str("\n\n## Earlier tool evidence\n");
        rendered.push_str("Full data is available in the listed artifacts.\n");
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
    }
    rendered
}

fn assistant_report_markdown(message: &Message) -> Option<String> {
    if !matches!(message.role, MessageRole::Assistant) {
        return None;
    }
    match &message.content {
        MessageContent::Text(text) => (!text.trim().is_empty()).then(|| text.trim().to_string()),
        MessageContent::Blocks(blocks) => {
            if blocks
                .iter()
                .any(|block| !matches!(block, Block::Text { .. }))
            {
                return None;
            }
            let markdown = blocks
                .iter()
                .filter_map(|block| match block {
                    Block::Text { text } => Some(text.trim()),
                    _ => None,
                })
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            (!markdown.is_empty()).then_some(markdown)
        }
    }
}

fn compact_turn_markdown(user: Option<&str>, markdown: &str) -> String {
    let mut rendered = String::new();
    if let Some(user) = user.filter(|text| !text.trim().is_empty()) {
        rendered.push_str("User request:\n");
        rendered.push_str(&truncate_chars(user, USER_REQUEST_MAX_CHARS));
        rendered.push_str("\n\n");
    }
    rendered.push_str("Assistant report excerpt:\n");
    rendered.push_str(&truncate_chars(markdown, TURN_MARKDOWN_MAX_CHARS));

    let (notes, artifacts) = extract_markdown_evidence(markdown);
    if !notes.is_empty() || !artifacts.is_empty() {
        rendered.push_str("\n\nExtracted evidence:\n");
        for (id, title) in notes {
            if title.is_empty() {
                rendered.push_str(&format!("- note_id: {id}\n"));
            } else {
                rendered.push_str(&format!("- note_id: {id}; title: {title}\n"));
            }
        }
        for path in artifacts {
            rendered.push_str(&format!("- artifact: {path}\n"));
        }
    }
    rendered
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max_chars).collect();
    out.push_str("\n\n[truncated; full report remains in the run artifact]");
    out
}

fn extract_markdown_evidence(markdown: &str) -> (BTreeSet<(String, String)>, BTreeSet<String>) {
    let mut notes = BTreeSet::new();
    let mut artifacts = BTreeSet::new();
    let mut cursor = 0;

    while let Some(open_offset) = markdown[cursor..].find('[') {
        let open = cursor + open_offset;
        let Some(close_offset) = markdown[open + 1..].find(']') else {
            break;
        };
        let close = open + 1 + close_offset;
        if markdown.as_bytes().get(close + 1) != Some(&b'(') {
            cursor = close + 1;
            continue;
        }
        let target_start = close + 2;
        let Some(target_offset) = markdown[target_start..].find(')') else {
            break;
        };
        let target_end = target_start + target_offset;
        let title = markdown[open + 1..close].trim();
        let target = markdown[target_start..target_end].trim();

        if let Some(note_id) = target.strip_prefix("note:") {
            let note_id = note_id.trim();
            if !note_id.is_empty() {
                notes.insert((note_id.to_string(), title.to_string()));
            }
        } else if is_artifact_link(target) {
            artifacts.insert(target.to_string());
        }
        cursor = target_end + 1;
    }

    (notes, artifacts)
}

fn is_artifact_link(target: &str) -> bool {
    let normalized = target.replace('\\', "/");
    normalized.contains("/.socai/runs/")
        || normalized.starts_with("artifacts/")
        || normalized.contains("/artifacts/")
        || normalized.starts_with("tools/")
        || normalized.contains("/tools/")
        || normalized.starts_with("snapshots/")
        || normalized.contains("/snapshots/")
        || normalized.starts_with("site_media/")
        || normalized.contains("/site_media/")
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
