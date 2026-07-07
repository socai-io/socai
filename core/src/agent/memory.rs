//! Context-window management for long agent runs.
//!
//! When the history is short, send it as-is. When it gets long, replace
//! the older half with a single user-role message containing:
//!   - the run-state context block (working memory render, capped)
//!   - a condensed list of recent tool events
//!
//! This is deliberately deterministic and local — no extra summarizer call.

use std::sync::Arc;

use crate::agent::llm::{Block, Message, MessageContent, MessageRole};
use crate::agent::run_state::RunState;

pub fn prepare_messages_for_context(
    messages: &[Message],
    run_state: Option<&Arc<RunState>>,
    memory_entries: &[String],
    keep_recent: usize,
    memory_max_chars: usize,
) -> Vec<Message> {
    if messages.len() <= keep_recent + 2 {
        return messages.to_vec();
    }
    let mut sections: Vec<String> = Vec::new();
    if let Some(state) = run_state {
        let block = state.context_block(memory_max_chars.max(1200) / 2);
        if !block.trim().is_empty() {
            sections.push(format!(
                "Structured run state from earlier steps:\n\n{block}"
            ));
        }
    }
    let memory = compact_memory_entries(memory_entries, memory_max_chars);
    if !memory.is_empty() {
        sections.push(format!(
            "Condensed event memory from earlier steps:\n\n{memory}"
        ));
    }
    if sections.is_empty() {
        return messages.to_vec();
    }

    let mut recent: Vec<Message> = messages.iter().rev().take(keep_recent).cloned().collect();
    recent.reverse();
    while recent.first().map(is_tool_result_message).unwrap_or(false) {
        recent.remove(0);
    }
    let mut out = Vec::with_capacity(2 + recent.len());
    out.push(messages[0].clone());
    out.push(Message::user(sections.join("\n\n")));
    out.extend(recent);
    out
}

/// Detect whether a message looks like a tool-result wrapper. The windowing
/// routine uses this to avoid stripping the user-side half of a
/// tool_call/tool_result pair (the assistant step would then refer to a
/// tool_call_id the model has lost context for).
pub fn is_tool_result_message(message: &Message) -> bool {
    if !matches!(message.role, MessageRole::User) {
        return false;
    }
    match &message.content {
        MessageContent::Blocks(blocks) => {
            blocks.iter().any(|b| matches!(b, Block::ToolResult { .. }))
        }
        _ => false,
    }
}

pub fn compact_memory_entries(entries: &[String], max_chars: usize) -> String {
    if entries.is_empty() || max_chars == 0 {
        return String::new();
    }
    let mut selected: Vec<&String> = Vec::new();
    let mut total = 0usize;
    for entry in entries.iter().rev() {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        let projected = total + trimmed.len() + 1;
        if !selected.is_empty() && projected > max_chars {
            break;
        }
        selected.push(entry);
        total = projected.min(max_chars);
        if total >= max_chars {
            break;
        }
    }
    selected.reverse();
    let joined = selected
        .into_iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if joined.chars().count() > max_chars {
        joined.chars().take(max_chars).collect()
    } else {
        joined
    }
}
