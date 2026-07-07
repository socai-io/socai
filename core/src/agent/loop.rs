//! Agent loop — the heart of the agent runtime.
//!
//! ```text
//!   while step < max_steps:
//!     response = backend.send(system, messages, tool_schemas)
//!     append assistant
//!     if no tool calls: break
//!     for tc in tool_calls:
//!       result = dispatcher.call(tc)
//!       append tool_result
//! ```
//!
//! Cross-cutting concerns are split out:
//! - `signature.rs` — md5 fingerprint for repeated-call detection
//! - `memory.rs`    — windowing the message history once it's long
//! - `report.rs`    — final report enrichment with artifact links
//! - `compaction.rs` — truncating tool_result bodies for the history budget
//! - `run_logging.rs` — canonical agent-run / LLM-step / tool-call records
//! - `run_state.rs` — in-memory context compaction state

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::agent::compaction::{compress_text_maybe_json, TOOL_RESULT_TEXT_MAX_CHARS};
use crate::agent::llm::{
    Backend, Block, LLMResponse, Message, StopReason, ToolCall, ToolResultContent, ToolSchema,
};
use crate::agent::memory::prepare_messages_for_context;
use crate::agent::report::report_with_artifacts;
use crate::agent::run_logging::{make_run_dir, AgentRunRecorder};
use crate::agent::run_state::RunState;
use crate::agent::signature::tool_call_signature;
use crate::agent::system_prompt::build_system_prompt;
use crate::agent::tool::{SharedTool, ToolContext, ToolResult, ToolResultBlock};

/// Events streamed to subscribers while the agent is running.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    Started {
        run_id: String,
        task: String,
        model: String,
    },
    Step {
        step: u32,
    },
    AssistantText {
        step: u32,
        text: String,
    },
    Reasoning {
        step: u32,
        text: String,
    },
    ToolCall {
        id: String,
        step: u32,
        sequence: u32,
        name: String,
        input: Value,
        repeat_count: u32,
    },
    ToolResult {
        id: String,
        step: u32,
        sequence: u32,
        name: String,
        input: Value,
        content: Value,
        summary: String,
        duration_ms: u64,
        error: Option<String>,
    },
    ApiError {
        step: u32,
        message: String,
    },
    Done {
        run_id: String,
        steps: u32,
        final_text: String,
    },
}

#[derive(Debug, Clone)]
pub struct AgentOptions {
    pub max_steps: u32,
    pub max_tokens: u32,
    pub extra_instructions: String,
    pub run_dir: Option<PathBuf>,
    /// Site names to pre-enable in ToolContext (gates `defer_until_site` tools).
    pub enabled_sites: Vec<String>,
    /// Recent-message window kept verbatim when history is condensed.
    pub keep_recent_messages: usize,
    /// Memory budget for the condensed context_block.
    pub memory_max_chars: usize,
    /// Prior chat-level messages to seed the conversation with, so a reply can
    /// continue an ongoing conversation. The current task is appended as
    /// the final user message. Empty = a fresh, single-shot run.
    pub seed_messages: Vec<Message>,
    /// Optional parent conversation identifier.
    pub session_id: Option<String>,
}

impl Default for AgentOptions {
    fn default() -> Self {
        Self {
            max_steps: 30,
            max_tokens: 16000,
            extra_instructions: String::new(),
            run_dir: None,
            enabled_sites: Vec::new(),
            keep_recent_messages: 12,
            memory_max_chars: 6000,
            seed_messages: Vec::new(),
            session_id: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentOutcome {
    pub run_id: String,
    pub run_dir: PathBuf,
    pub steps: u32,
    pub final_text: String,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
}

pub async fn run_agent(
    task: &str,
    backend: Arc<dyn Backend>,
    tools: Vec<SharedTool>,
    options: AgentOptions,
) -> anyhow::Result<AgentOutcome> {
    let (tx, _rx) = broadcast::channel(256);
    run_agent_with_events(task, backend, tools, options, tx).await
}

pub async fn run_agent_with_events(
    task: &str,
    backend: Arc<dyn Backend>,
    tools: Vec<SharedTool>,
    options: AgentOptions,
    events: broadcast::Sender<AgentEvent>,
) -> anyhow::Result<AgentOutcome> {
    let run_id = new_run_id();
    let run_dir = options.run_dir.unwrap_or_else(|| make_run_dir(task));
    ensure_dir(&run_dir)?;
    let run_state = Arc::new(RunState::new(task));
    let run_recorder = AgentRunRecorder::start(
        &run_dir,
        &run_id,
        options.session_id.as_deref(),
        task,
        backend.model(),
    )?;

    let mut ctx = ToolContext::new(&run_id, &run_dir).with_run_state(Arc::clone(&run_state));
    for site in &options.enabled_sites {
        ctx.enable_site(site.clone());
    }

    let mut messages: Vec<Message> = options.seed_messages.clone();
    messages.push(Message::user(task.to_string()));

    emit(
        &events,
        AgentEvent::Started {
            run_id: run_id.clone(),
            task: task.to_string(),
            model: backend.label(),
        },
    );

    let mut step = 0u32;
    let mut final_text = String::new();
    let mut total_input_tokens = 0u64;
    let mut total_output_tokens = 0u64;
    let mut context_memory: Vec<String> = Vec::new();
    let mut tool_call_history: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut completed = false;
    let mut terminal_error: Option<String> = None;
    let mut truncation_retries = 0u32;
    let mut last_system: String = build_system_prompt(&[], &options.extra_instructions);

    while step < options.max_steps {
        step += 1;
        ctx.step = step;
        emit(&events, AgentEvent::Step { step });
        debug!(step, "agent step start");

        let schemas = tool_schemas(&tools, &ctx);
        let tool_names: Vec<&str> = schemas.iter().map(|s| s.name.as_str()).collect();
        let system = build_system_prompt(&tool_names, &options.extra_instructions);
        last_system = system.clone();
        let request_messages = prepare_messages_for_context(
            &messages,
            Some(&run_state),
            &context_memory,
            options.keep_recent_messages,
            options.memory_max_chars,
        );
        let request_payload =
            backend.request_payload(&system, &request_messages, &schemas, options.max_tokens)?;
        run_recorder.record_llm_request(step, &request_payload)?;

        let llm_started = Instant::now();
        let response: LLMResponse = match backend
            .send(&system, &request_messages, &schemas, options.max_tokens)
            .await
        {
            Ok(response) => {
                run_recorder.record_llm_response(
                    step,
                    &response,
                    llm_started.elapsed().as_millis() as u64,
                )?;
                response
            }
            Err(e) => {
                let msg = format!("{e:#}");
                run_recorder.record_llm_error(
                    step,
                    &msg,
                    llm_started.elapsed().as_millis() as u64,
                )?;
                warn!(step, error = %msg, "backend error");
                emit(
                    &events,
                    AgentEvent::ApiError {
                        step,
                        message: msg.clone(),
                    },
                );
                final_text = format!("API error: {msg}");
                terminal_error = Some(msg);
                break;
            }
        };

        total_input_tokens += response.input_tokens;
        total_output_tokens += response.output_tokens;

        // Split text_blocks into visible vs "[Thinking] "-prefixed thinking.
        // Some hosts (Anthropic without extended-thinking enabled) ask the
        // model to prefix its reasoning so we can keep it out of final_text
        // while still emitting it on the event stream for UIs that want to
        // show it.
        let (visible_texts, thinking_texts) = split_thinking(&response.text_blocks);

        // Surface reasoning to subscribers — both the structured
        // reasoning_content (Kimi/Qwen) and the [Thinking]-prefixed text.
        if !response.reasoning_content.trim().is_empty() {
            emit(
                &events,
                AgentEvent::Reasoning {
                    step,
                    text: response.reasoning_content.clone(),
                },
            );
        }
        if !thinking_texts.is_empty() {
            let thinking_text = thinking_texts.join("\n");
            emit(
                &events,
                AgentEvent::Reasoning {
                    step,
                    text: thinking_text.clone(),
                },
            );
        }

        // A response cut off by max_tokens with no tool calls must not be
        // mistaken for completion: with thinking models the entire budget can
        // go to (possibly empty-text) thinking blocks, leaving no visible
        // output at all. Discard the truncated step — a partial step can't be
        // replayed reliably — and ask the model to redo it, bounded so a
        // pathological loop still terminates.
        if response.stop_reason == StopReason::MaxTokens && response.tool_calls.is_empty() {
            truncation_retries += 1;
            warn!(
                step,
                truncation_retries, "response truncated by max_tokens with no tool calls"
            );
            if truncation_retries > 2 {
                let msg = format!(
                    "model output was truncated by the max_tokens limit ({}) {} times in a row",
                    options.max_tokens, truncation_retries
                );
                emit(
                    &events,
                    AgentEvent::ApiError {
                        step,
                        message: msg.clone(),
                    },
                );
                final_text = format!("Error: {msg}");
                terminal_error = Some(msg);
                break;
            }
            messages.push(Message::user(
                "[Note: your previous response was cut off by the output token limit and \
                 has been discarded. Respond again, more concisely. If you were producing \
                 the final answer, write the complete final answer now — prioritize \
                 covering the full structure over verbose detail.]"
                    .to_string(),
            ));
            continue;
        }
        truncation_retries = 0;

        // Build the assistant block list manually instead of using
        // LLMResponse::to_assistant_blocks() so we can:
        // - drop [Thinking]-prefixed text from history
        // - truncate visible text to ASSISTANT_TEXT_MAX_CHARS, matching
        //   ASSISTANT_TEXT_MAX_CHARS (320 chars)
        let assistant_blocks = build_assistant_blocks(&response, &visible_texts);
        messages.push(Message::assistant_blocks(assistant_blocks));

        for text in &visible_texts {
            emit(
                &events,
                AgentEvent::AssistantText {
                    step,
                    text: text.clone(),
                },
            );
            final_text = text.clone();
        }

        let tool_call_summary: Vec<Value> = response
            .tool_calls
            .iter()
            .map(|tc| json!({"name": tc.name, "input": tc.input}))
            .collect();
        run_state.note_assistant_step(step, &visible_texts.join("\n"), &tool_call_summary);
        if response.tool_calls.is_empty() {
            completed = true;
            break;
        }

        let mut tool_result_blocks: Vec<Block> = Vec::new();
        for (idx, tc) in response.tool_calls.iter().enumerate() {
            let ToolCall { id, name, input } = tc;
            ctx.active_tool_name = name.clone();

            let sig = tool_call_signature(name, input);
            let history = tool_call_history.entry(sig).or_default();
            history.push(step);
            let repeat_count = history.len() as u32;

            let sequence = (idx + 1) as u32;
            let effective_input = find_tool(&tools, name)
                .map(|tool| tool.effective_input(input))
                .unwrap_or_else(|| input.clone());
            let tool_recorder =
                run_recorder.start_tool_call(step, sequence, name, &effective_input)?;
            let tool_ctx = ctx.clone().with_tool_dir(tool_recorder.dir());
            emit(
                &events,
                AgentEvent::ToolCall {
                    id: id.clone(),
                    step,
                    sequence,
                    name: name.clone(),
                    input: effective_input.clone(),
                    repeat_count,
                },
            );
            run_state.note_tool_call(step, name, &effective_input);
            let started = Instant::now();
            let (result, error) = dispatch_tool(&tools, name, &effective_input, &tool_ctx).await;
            let duration_ms = started.elapsed().as_millis() as u64;
            let duration_s = (duration_ms as f64) / 1000.0;
            tool_recorder.finish_blocks(&result.blocks, duration_ms, error.as_deref())?;

            let result_content = tool_result_to_content(&result);
            let content = content_for_log(&result_content);
            let flat = result.flat_text();
            let summary = truncate_summary(&flat, 240);
            emit(
                &events,
                AgentEvent::ToolResult {
                    id: id.clone(),
                    step,
                    sequence,
                    name: name.clone(),
                    input: effective_input.clone(),
                    content: content.clone(),
                    summary: summary.clone(),
                    duration_ms,
                    error: error.clone(),
                },
            );
            run_state.note_tool_result(step, name, &effective_input, &summary, duration_s);
            let mut history_content = bound_content_for_history(&result_content);
            // Break tight loops: when the model fires the *same* call with the
            // same args repeatedly, the bare result won't change its mind. Tell
            // it explicitly to stop and work with what it already has.
            if repeat_count >= 3 {
                history_content.insert(
                    0,
                    ToolResultContent::Text {
                        text: format!(
                            "[Note: you have called {name} with these exact arguments \
                             {repeat_count} times and the result is not changing. Stop \
                             repeating this call. Proceed with the information you already \
                             have — if something cannot be found, say so and complete the \
                             task with what is available.]"
                        ),
                    },
                );
            }
            tool_result_blocks.push(Block::ToolResult {
                tool_use_id: id.clone(),
                content: history_content,
            });

            context_memory.push(format!(
                "- step {step} {name}({}): {summary}",
                truncate_summary(
                    &serde_json::to_string(&effective_input).unwrap_or_default(),
                    160,
                )
            ));
            if context_memory.len() > 80 {
                let overflow = context_memory.len() - 80;
                context_memory.drain(0..overflow);
            }
            ctx.active_tool_name.clear();
        }
        messages.push(Message::user_blocks(tool_result_blocks));
    }

    if !completed && step >= options.max_steps {
        info!(step, "reached max_steps, forcing final summary");
        messages.push(Message::user(format!(
            "You have reached the maximum of {} tool-using steps. Do not call any \
             more tools. Based on the evidence already gathered, produce the best \
             possible final answer for the user now in the same language as the \
             original task. If information is incomplete, state what is known, \
             what is missing, and give your best-effort conclusion.",
            options.max_steps
        )));
        let request_messages = prepare_messages_for_context(
            &messages,
            Some(&run_state),
            &context_memory,
            options.keep_recent_messages,
            options.memory_max_chars,
        );
        let request_payload =
            backend.request_payload(&last_system, &request_messages, &[], options.max_tokens)?;
        run_recorder.record_llm_request(step + 1, &request_payload)?;
        let llm_started = Instant::now();
        match backend
            .send(&last_system, &request_messages, &[], options.max_tokens)
            .await
        {
            Ok(response) => {
                run_recorder.record_llm_response(
                    step + 1,
                    &response,
                    llm_started.elapsed().as_millis() as u64,
                )?;
                total_input_tokens += response.input_tokens;
                total_output_tokens += response.output_tokens;
                let (visible_texts, _) = split_thinking(&response.text_blocks);
                for text in &visible_texts {
                    emit(
                        &events,
                        AgentEvent::AssistantText {
                            step: step + 1,
                            text: text.clone(),
                        },
                    );
                    final_text = text.clone();
                }
            }
            Err(e) => {
                let msg = format!("{e:#}");
                run_recorder.record_llm_error(
                    step + 1,
                    &msg,
                    llm_started.elapsed().as_millis() as u64,
                )?;
                warn!(step = step + 1, error = %msg, "forced summary error");
                emit(
                    &events,
                    AgentEvent::ApiError {
                        step: step + 1,
                        message: msg.clone(),
                    },
                );
                terminal_error = Some(msg);
            }
        }
    }

    emit(
        &events,
        AgentEvent::Done {
            run_id: run_id.clone(),
            steps: step,
            final_text: final_text.clone(),
        },
    );

    let enriched_report = report_with_artifacts(&final_text, Some(&run_state));
    let _ = std::fs::write(run_dir.join("report.md"), &enriched_report);

    run_recorder.finish(
        if terminal_error.is_some() {
            "failed"
        } else {
            "completed"
        },
        step,
        total_input_tokens,
        total_output_tokens,
        terminal_error.as_deref(),
    )?;

    Ok(AgentOutcome {
        run_id,
        run_dir,
        steps: step,
        final_text,
        total_input_tokens,
        total_output_tokens,
    })
}

// ---------- small private helpers (not core logic, kept here for locality) ----------

fn new_run_id() -> String {
    use chrono::Utc;
    use std::time::{SystemTime, UNIX_EPOCH};
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() % 1_000_000)
        .unwrap_or(0);
    format!("{}-{:06}", Utc::now().format("%Y%m%d-%H%M%S"), suffix)
}

fn ensure_dir(path: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(path)?;
    Ok(())
}

fn tool_schemas(tools: &[SharedTool], ctx: &ToolContext) -> Vec<ToolSchema> {
    tools
        .iter()
        .filter(|t| t.is_available(ctx))
        .map(|t| ToolSchema {
            name: t.name().to_string(),
            description: t.description().to_string(),
            input_schema: t.input_schema(),
        })
        .collect()
}

fn find_tool<'a>(tools: &'a [SharedTool], name: &str) -> Option<&'a SharedTool> {
    tools.iter().find(|t| t.name() == name)
}

fn emit(events: &broadcast::Sender<AgentEvent>, event: AgentEvent) {
    let _ = events.send(event);
}

async fn dispatch_tool(
    tools: &[SharedTool],
    name: &str,
    input: &Value,
    ctx: &ToolContext,
) -> (ToolResult, Option<String>) {
    match find_tool(tools, name) {
        Some(tool) if tool.is_available(ctx) => match tool.call(input.clone(), ctx).await {
            Ok(r) => (r, None),
            Err(e) => {
                let msg = format!("{e:#}");
                (
                    ToolResult::text(format!("Error executing {name}: {msg}")),
                    Some(msg),
                )
            }
        },
        Some(_) => {
            let msg = format!("Tool '{name}' is not currently available");
            (ToolResult::text(format!("Error: {msg}")), Some(msg))
        }
        None => {
            let msg = format!("Unknown tool '{name}'");
            (ToolResult::text(format!("Error: {msg}")), Some(msg))
        }
    }
}

fn tool_result_to_content(result: &ToolResult) -> Vec<ToolResultContent> {
    result
        .blocks
        .iter()
        .map(|b| match b {
            ToolResultBlock::Text { text } => ToolResultContent::Text { text: text.clone() },
            ToolResultBlock::Image { data, media_type } => ToolResultContent::Image {
                data: data.clone(),
                media_type: media_type.clone(),
            },
        })
        .collect()
}

/// Squash a tool_result for the chat history:
/// - text blocks → compressed JSON-aware truncation
/// - image blocks → text placeholder. If a preceding text block contained
///   "Screenshot saved to <path>", the placeholder names that path so the
///   model can still cite it in the final report.
///
/// Returns a single Text block (or `(empty result)` when nothing usable
/// remained). The raw bodies are preserved by the tool-call recorder, so the
/// chat-history budget can stay bounded without losing debug data.
fn bound_content_for_history(content: &[ToolResultContent]) -> Vec<ToolResultContent> {
    let mut screenshot_path: Option<String> = None;
    let mut parts: Vec<String> = Vec::new();
    for block in content {
        match block {
            ToolResultContent::Text { text } => {
                if screenshot_path.is_none() {
                    screenshot_path = extract_screenshot_hint(text);
                }
                let compressed = compress_text_maybe_json(text, TOOL_RESULT_TEXT_MAX_CHARS);
                if !compressed.trim().is_empty() {
                    parts.push(compressed);
                }
            }
            ToolResultContent::Image { .. } => {
                parts.push(match &screenshot_path {
                    Some(path) => format!("[Image omitted from history. Screenshot file: {path}.]"),
                    None => "[Image omitted from history.]".to_string(),
                });
            }
        }
    }
    let mut combined = parts.join("\n\n").trim().to_string();
    if combined.chars().count() > TOOL_RESULT_TEXT_MAX_CHARS {
        combined = compress_text_maybe_json(&combined, TOOL_RESULT_TEXT_MAX_CHARS);
    }
    if combined.is_empty() {
        combined = "(empty result)".to_string();
    }
    vec![ToolResultContent::Text { text: combined }]
}

/// `"Screenshot saved to /tmp/x.png"` → `Some("/tmp/x.png")`.
fn extract_screenshot_hint(text: &str) -> Option<String> {
    let marker = "Screenshot saved to ";
    let idx = text.find(marker)?;
    let after = &text[idx + marker.len()..];
    let end = after
        .find(|c: char| c.is_whitespace())
        .unwrap_or(after.len());
    let path = after[..end].trim();
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

/// Render tool-result content for the live UI event stream. Image bodies stay
/// in the canonical tool output and are omitted from the event payload.
fn content_for_log(content: &[ToolResultContent]) -> Value {
    let array: Vec<Value> = content
        .iter()
        .map(|c| match c {
            ToolResultContent::Text { text } => json!({"type": "text", "text": text}),
            ToolResultContent::Image { media_type, .. } => json!({
                "type": "image",
                "media_type": media_type,
            }),
        })
        .collect();
    Value::Array(array)
}

fn truncate_summary(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut s: String = text.chars().take(max_chars).collect();
    s.push('…');
    s
}

/// Split assistant text blocks into (visible, thinking) by the `[Thinking] `
/// prefix. Whitespace-only blocks are dropped from both buckets.
fn split_thinking(text_blocks: &[String]) -> (Vec<String>, Vec<String>) {
    const PREFIX: &str = "[Thinking] ";
    let mut visible: Vec<String> = Vec::new();
    let mut thinking: Vec<String> = Vec::new();
    for block in text_blocks {
        let trimmed = block.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix(PREFIX) {
            thinking.push(rest.trim().to_string());
        } else {
            visible.push(trimmed.to_string());
        }
    }
    (visible, thinking)
}

/// Build the assistant message blocks for history. Truncates visible text to
/// `ASSISTANT_TEXT_MAX_CHARS` to keep history bounded over many steps. Drops
/// `[Thinking]`-prefixed text since that's already surfaced as a Reasoning
/// event.
fn build_assistant_blocks(response: &LLMResponse, visible_texts: &[String]) -> Vec<Block> {
    use crate::agent::compaction::{truncate, ASSISTANT_TEXT_MAX_CHARS};
    let mut blocks: Vec<Block> = Vec::new();
    // Provider-native reasoning goes first, verbatim (never truncated):
    // OpenAI Responses reasoning items and Anthropic thinking blocks must be
    // replayed unmodified. When present they already carry the reasoning
    // text, so the ReasoningContent mirror is skipped.
    for item in &response.reasoning_items {
        blocks.push(Block::OpenAIReasoning { item: item.clone() });
    }
    for tb in &response.thinking_blocks {
        blocks.push(Block::Thinking {
            thinking: tb.thinking.clone(),
            signature: tb.signature.clone(),
        });
    }
    // Preserve reasoning_content alongside tool_calls so providers that
    // require it round-tripped (Kimi/Qwen) get it on the next step.
    if response.thinking_blocks.is_empty()
        && response.reasoning_items.is_empty()
        && !response.reasoning_content.trim().is_empty()
        && !response.tool_calls.is_empty()
    {
        blocks.push(Block::ReasoningContent {
            text: response.reasoning_content.clone(),
        });
    }
    for text in visible_texts {
        let bounded = truncate(text, ASSISTANT_TEXT_MAX_CHARS);
        if !bounded.is_empty() {
            blocks.push(Block::Text { text: bounded });
        }
    }
    for tc in &response.tool_calls {
        blocks.push(Block::ToolUse {
            id: tc.id.clone(),
            name: tc.name.clone(),
            input: tc.input.clone(),
        });
    }
    blocks
}
