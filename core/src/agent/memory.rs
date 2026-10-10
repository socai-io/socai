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
const WEB_SOURCE_EXCERPT_MIN_CHARS: usize = 1_200;
const WEB_SOURCE_EXCERPT_MAX_CHARS: usize = 2_000;
const WEB_SOURCE_TITLE_MAX_CHARS: usize = 240;
const MAX_WEB_SOURCES: usize = 24;
const WEB_SOURCE_TOTAL_EXCERPT_MAX_CHARS: usize = WEB_SOURCE_EXCERPT_MIN_CHARS * MAX_WEB_SOURCES;
const COMPACT_CONTEXT_HEADING: &str = "# Earlier compacted context";
const LEGACY_EVIDENCE_HEADING: &str = "# Earlier tool evidence";
const WEB_SOURCE_SECTION_HEADING: &str = "## Earlier web source evidence";
const WEB_SOURCE_SECTION_MARKER: &str = "\n\n## Earlier web source evidence\n";

/// Rewrite the transcript only when it has grown beyond `compact_after` full
/// messages. The message at `anchor_user_index` — the current run's user task,
/// index `0` for a fresh run and `seed_messages.len()` for a follow-up — stays
/// verbatim at the front; the last `keep_recent` messages remain verbatim
/// (widened backward when the window would open on a tool_result message, so
/// tool_use/tool_result pairs never split); older tool outputs become artifact
/// locators. Follow-up runs (`is_follow_up`) also get a short task
/// reminder adjacent to the recent tail. `anchor_user_index` is updated to the
/// anchor's new position after a rewrite so a later compaction in the same run
/// cannot accidentally pin a tool message; `is_follow_up` remains stable so
/// every later rewrite can recreate the reminder. Mutating the transcript,
/// rather than rebuilding a summary for every request, leaves the request
/// prefix stable until the next sawtooth compaction point and therefore
/// friendly to provider prompt caches.
pub fn compact_messages_for_context(
    messages: &mut Vec<Message>,
    compact_after: usize,
    keep_recent: usize,
    anchor_user_index: &mut usize,
    is_follow_up: bool,
) -> bool {
    if compact_after == 0
        || keep_recent == 0
        || keep_recent >= compact_after
        || messages.len() <= compact_after
    {
        return false;
    }

    let mut recent_start = messages.len() - keep_recent;
    // Tool results live in a user message appended immediately after the
    // assistant message carrying the matching tool_use blocks, and providers
    // reject a request that keeps one side of that pair without the other
    // (OpenAI-compat: tool message without tool_calls; Anthropic: tool_result
    // without its tool_use). A count-based boundary can land between the two —
    // an extra lone user message (max-tokens discard note, forced-summary
    // prompt) shifts the window onto the tool_result — so widen the window
    // until it no longer starts mid-pair.
    while recent_start > 1 && contains_tool_result(&messages[recent_start]) {
        recent_start -= 1;
    }
    let anchor_idx = (*anchor_user_index).min(messages.len().saturating_sub(1));
    let anchor = messages[anchor_idx].clone();
    let older: Vec<Message> = (0..recent_start)
        .filter(|&index| index != anchor_idx)
        .map(|index| messages[index].clone())
        .collect();
    let recent: Vec<Message> = messages[recent_start..]
        .iter()
        .enumerate()
        .filter(|(offset, _)| recent_start + offset != anchor_idx)
        .map(|(_, message)| message.clone())
        .collect();
    let evidence = compact_older_messages(&older);

    let task_reminder = is_follow_up
        .then(|| user_text(&anchor))
        .flatten()
        .map(|task| current_task_reminder(&task));
    let recent_ends_with_assistant = recent
        .last()
        .is_some_and(|message| matches!(message.role, MessageRole::Assistant));

    let mut compacted = Vec::with_capacity(3 + recent.len());
    compacted.push(anchor);
    if !evidence.is_empty() {
        compacted.push(Message::user(evidence));
    }
    if !recent_ends_with_assistant {
        compacted.extend(task_reminder.clone());
    }
    compacted.extend(recent);
    if recent_ends_with_assistant {
        compacted.extend(task_reminder);
    }
    *messages = compacted;
    *anchor_user_index = 0;
    true
}

fn user_text(message: &Message) -> Option<String> {
    if !matches!(message.role, MessageRole::User) {
        return None;
    }
    match &message.content {
        MessageContent::Text(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        MessageContent::Blocks(_) => None,
    }
}

fn current_task_reminder(task: &str) -> Message {
    Message::user(format!(
        "Current task (do not confuse with earlier turns): {task}"
    ))
}

fn contains_tool_result(message: &Message) -> bool {
    match &message.content {
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .any(|block| matches!(block, Block::ToolResult { .. })),
        MessageContent::Text(_) => false,
    }
}

fn compact_older_messages(messages: &[Message]) -> String {
    let mut inherited = Vec::new();
    let mut artifacts: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
    let mut web_read_calls = BTreeSet::new();
    let mut web_sources: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut turns = Vec::new();
    let mut pending_user: Option<String> = None;

    for message in messages {
        match (&message.role, &message.content) {
            (MessageRole::User, MessageContent::Text(text)) => {
                if text.starts_with(COMPACT_CONTEXT_HEADING)
                    || text.starts_with(LEGACY_EVIDENCE_HEADING)
                {
                    let inherited_without_web_sources =
                        collect_inherited_web_source_evidence(text, &mut web_sources);
                    if !inherited_without_web_sources.is_empty() {
                        inherited.push(inherited_without_web_sources);
                    }
                } else {
                    pending_user = Some(text.trim().to_string());
                }
                continue;
            }
            (MessageRole::Assistant, MessageContent::Blocks(blocks)) => {
                for block in blocks {
                    if let Block::ToolUse { id, name, .. } = block {
                        if name == "web_read" {
                            web_read_calls.insert(id.clone());
                        }
                    }
                }
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
            let Block::ToolResult {
                tool_use_id,
                content,
            } = block
            else {
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
                if web_read_calls.contains(tool_use_id) {
                    collect_web_source_evidence(&value, &mut web_sources);
                }
            }
        }
    }

    if let Some(user) = pending_user.take().filter(|text| !text.trim().is_empty()) {
        turns.push(compact_turn_markdown(
            Some(user.as_str()),
            "(no assistant report was recorded before compaction)",
        ));
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
    if !web_sources.is_empty() {
        let source_count = web_sources.len().min(MAX_WEB_SOURCES);
        let excerpt_max_chars = web_source_excerpt_max_chars(source_count);
        rendered.push_str(&format!("\n\n{WEB_SOURCE_SECTION_HEADING}\n"));
        rendered.push_str(
            "These pages were opened and read directly. Preserve their URLs and excerpts when producing the final report.\n",
        );
        for (url, (title, excerpt)) in web_sources.into_iter().take(MAX_WEB_SOURCES) {
            rendered.push_str(&format!("\n- URL: {url}\n"));
            if !title.is_empty() {
                rendered.push_str(&format!("  Title: {title}\n"));
            }
            if !excerpt.is_empty() {
                rendered.push_str(&format!(
                    "  Evidence excerpt: {}\n",
                    truncate_plain(&excerpt, excerpt_max_chars)
                ));
            }
        }
    }
    rendered
}

fn collect_inherited_web_source_evidence(
    text: &str,
    sources: &mut BTreeMap<String, (String, String)>,
) -> String {
    let mut remaining = text;
    let mut preserved = String::new();
    while let Some(section_start) = remaining.find(WEB_SOURCE_SECTION_MARKER) {
        preserved.push_str(&remaining[..section_start]);
        let section_tail = &remaining[section_start + WEB_SOURCE_SECTION_MARKER.len()..];
        let section_end = section_tail.find("\n\n## ").unwrap_or(section_tail.len());
        let section = &section_tail[..section_end];
        let mut current_url = String::new();
        let mut current_title = String::new();
        let mut current_excerpt = String::new();

        let flush = |url: &mut String,
                     title: &mut String,
                     excerpt: &mut String,
                     sources: &mut BTreeMap<String, (String, String)>| {
            if !url.is_empty() {
                insert_web_source_evidence(
                    sources,
                    std::mem::take(url),
                    std::mem::take(title),
                    std::mem::take(excerpt),
                );
            }
        };
        for line in section.lines() {
            if let Some(url) = line.strip_prefix("- URL: ") {
                flush(
                    &mut current_url,
                    &mut current_title,
                    &mut current_excerpt,
                    sources,
                );
                current_url = url.trim().to_string();
            } else if let Some(title) = line.strip_prefix("  Title: ") {
                current_title = title.trim().to_string();
            } else if let Some(excerpt) = line.strip_prefix("  Evidence excerpt: ") {
                current_excerpt = excerpt.trim().to_string();
            }
        }
        flush(
            &mut current_url,
            &mut current_title,
            &mut current_excerpt,
            sources,
        );
        remaining = &section_tail[section_end..];
    }
    preserved.push_str(remaining);
    preserved.trim().to_string()
}

fn web_source_excerpt_max_chars(source_count: usize) -> usize {
    if source_count == 0 {
        return 0;
    }
    (WEB_SOURCE_TOTAL_EXCERPT_MAX_CHARS / source_count.min(MAX_WEB_SOURCES))
        .clamp(WEB_SOURCE_EXCERPT_MIN_CHARS, WEB_SOURCE_EXCERPT_MAX_CHARS)
}

fn collect_web_source_evidence(value: &Value, sources: &mut BTreeMap<String, (String, String)>) {
    let Some(url) = value
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
    else {
        return;
    };
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .map(normalize_web_evidence)
        .map(|text| truncate_plain(&text, WEB_SOURCE_TITLE_MAX_CHARS))
        .unwrap_or_default();
    let excerpt = value
        .get("text")
        .and_then(Value::as_str)
        .map(normalize_web_evidence)
        .map(|text| truncate_plain(&text, WEB_SOURCE_EXCERPT_MAX_CHARS))
        .unwrap_or_default();
    if title.is_empty() && excerpt.is_empty() {
        return;
    }

    insert_web_source_evidence(sources, url.to_string(), title, excerpt);
}

fn insert_web_source_evidence(
    sources: &mut BTreeMap<String, (String, String)>,
    url: String,
    title: String,
    excerpt: String,
) {
    let candidate = (title, excerpt);
    match sources.get(&url) {
        Some(existing) if evidence_size(existing) >= evidence_size(&candidate) => {}
        _ => {
            sources.insert(url, candidate);
        }
    }
}

fn normalize_web_evidence(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_plain(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push('…');
    out
}

fn evidence_size(value: &(String, String)) -> usize {
    value.0.chars().count() + value.1.chars().count()
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool_exchange(id: &str, name: &str, result: Value) -> Vec<Message> {
        vec![
            Message::assistant_blocks(vec![Block::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input: json!({}),
            }]),
            Message::user_blocks(vec![Block::ToolResult {
                tool_use_id: id.to_string(),
                content: vec![ToolResultContent::Text {
                    text: serde_json::to_string(&result).unwrap(),
                }],
            }]),
        ]
    }

    #[test]
    fn compacted_context_preserves_opened_web_source_evidence() {
        let messages = tool_exchange(
            "read-1",
            "web_read",
            json!({
                "url": "https://arxiv.org/abs/2604.08516",
                "title": "MolmoWeb:  Open Visual Web Agent",
                "text": "Submitted on 9 Apr 2026\nMolmoWeb uses screenshot actions.\nPass@4 is 94.7%."
            }),
        );

        let compacted = compact_older_messages(&messages);

        assert!(compacted.contains("## Earlier web source evidence"));
        assert!(compacted.contains("https://arxiv.org/abs/2604.08516"));
        assert!(compacted.contains("MolmoWeb: Open Visual Web Agent"));
        assert!(compacted.contains("Pass@4 is 94.7%."));
    }

    #[test]
    fn compacted_context_preserves_late_web_results_for_small_source_sets() {
        let messages = tool_exchange(
            "read-1",
            "web_read",
            json!({
                "url": "https://arxiv.org/abs/2511.12997",
                "title": "WebCoach",
                "text": format!(
                    "{}Evaluations on WebVoyager increase task success from 47% to 61%.",
                    "method and architecture context ".repeat(50)
                )
            }),
        );

        let compacted = compact_older_messages(&messages);

        assert!(compacted.contains("Evaluations on WebVoyager"));
        assert!(compacted.contains("47% to 61%"));
    }

    #[test]
    fn web_source_excerpt_budget_stays_bounded_as_source_count_grows() {
        assert_eq!(web_source_excerpt_max_chars(0), 0);
        assert_eq!(web_source_excerpt_max_chars(1), 2_000);
        assert_eq!(web_source_excerpt_max_chars(3), 2_000);
        assert_eq!(web_source_excerpt_max_chars(15), 1_920);
        assert_eq!(web_source_excerpt_max_chars(24), 1_200);
        assert_eq!(web_source_excerpt_max_chars(100), 1_200);
    }

    #[test]
    fn compacted_context_does_not_treat_navigation_metadata_as_read_evidence() {
        let messages = tool_exchange(
            "navigate-1",
            "web_navigate",
            json!({
                "url": "https://arxiv.org/abs/2604.08516",
                "title": "MolmoWeb"
            }),
        );

        let compacted = compact_older_messages(&messages);

        assert!(!compacted.contains("Earlier web source evidence"));
        assert!(!compacted.contains("https://arxiv.org/abs/2604.08516"));
    }

    #[test]
    fn compacted_context_keeps_the_richer_read_for_a_repeated_url() {
        let mut messages = tool_exchange(
            "read-1",
            "web_read",
            json!({
                "url": "https://example.com/report",
                "title": "Report",
                "text": "short"
            }),
        );
        messages.extend(tool_exchange(
            "read-2",
            "web_read",
            json!({
                "url": "https://example.com/report",
                "title": "Report",
                "text": "longer evidence with the benchmark result"
            }),
        ));

        let compacted = compact_older_messages(&messages);

        assert_eq!(
            compacted
                .matches("- URL: https://example.com/report")
                .count(),
            1
        );
        assert!(compacted.contains("longer evidence with the benchmark result"));
        assert!(!compacted.contains("Evidence excerpt: short\n"));
    }
}
