//! Agent loop — the heart of the agent runtime.
//!
//! ```text
//!   while normal or wrap-up steps remain:
//!     response = backend.send(system, messages, tool_schemas)
//!     append assistant
//!     if no tool calls: break
//!     dispatch tool_calls concurrently only across distinct opt-in scopes
//!     append tool_results in the model's original order
//! ```
//!
//! Cross-cutting concerns are split out:
//! - `signature.rs` — md5 fingerprint for repeated-call detection
//! - `memory.rs`    — windowing the message history once it's long
//! - `local_memory.rs` — cross-task summary injected into later runs
//! - `report.rs`    — final report enrichment with artifact links
//! - `compaction.rs` — truncating tool_result bodies for the history budget
//! - `run_logging.rs` — canonical agent-run / LLM-step / tool-call records
//! - `run_state.rs` — in-memory context compaction state

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::join_all;
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::agent::api_errors::is_transient_api_error;
use crate::agent::compaction::{compress_text_maybe_json, TOOL_RESULT_TEXT_MAX_CHARS};
use crate::agent::extensions::{FinishDecision, RequestPhase, RunExtensions};
use crate::agent::llm::{
    Backend, Block, LLMResponse, Message, StopReason, TokenUsage, ToolCall, ToolResultContent,
    ToolSchema,
};
use crate::agent::local_memory::{self, memory_prompt};
use crate::agent::memory::{
    compact_messages_for_context, DEFAULT_COMPACT_AFTER_MESSAGES, DEFAULT_KEEP_RECENT_MESSAGES,
};
use crate::agent::report::report_with_artifacts;
use crate::agent::run_logging::{make_run_dir, AgentRunRecorder, ToolCallRecorder};
use crate::agent::run_state::RunState;
use crate::agent::signature::tool_call_signature;
use crate::agent::system_prompt::build_system_prompt;
use crate::agent::tool::{
    SharedTool, SharedToolFailureRecovery, Tool, ToolContext, ToolProgressEvent,
    ToolRecoveryOutcome, ToolResult, ToolResultBlock,
};
use crate::media::RemoteOcrConfig;
use crate::telemetry::trace::RunTraceBuilder;
use crate::util::execution_clock::{ExecutionClock, ExecutionDeadline};

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
    /// Transient generation preview. Empty strings clear the current attempt.
    ModelPreview {
        text: String,
        reasoning: String,
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
    ToolProgress {
        id: String,
        step: u32,
        sequence: u32,
        name: String,
        progress: ToolProgressEvent,
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
    ModelConnection {
        step: u32,
        attempt: usize,
        provider: String,
        state: &'static str,
        reason: &'static str,
        timing: Option<crate::util::suspend_clock::ClockInterval>,
    },
    ApiError {
        step: u32,
        message: String,
    },
    Done {
        run_id: String,
        steps: u32,
        final_text: String,
        partial: bool,
        degraded_reason: Option<String>,
    },
}

pub const DEFAULT_MAX_STEPS: u32 = 50;
const WRAP_UP_STEPS: u32 = 5;

#[derive(Debug, Clone)]
pub struct AgentOptions {
    /// Optional caller-owned run identifier. Entrypoints with a durable run
    /// record use this to keep interactive signals and Core tool contexts on
    /// the same identity; local entrypoints keep the generated default.
    pub run_id: Option<String>,
    /// Normal tool-using budget. A nonzero budget also allows five wrap-up steps.
    pub max_steps: u32,
    pub max_tokens: u32,
    pub extra_instructions: String,
    pub run_dir: Option<PathBuf>,
    /// Site names to pre-enable in ToolContext (gates `defer_until_site` tools).
    pub enabled_sites: Vec<String>,
    /// Entrypoint-selected skills loaded before the first model request.
    pub initial_skills: Vec<String>,
    /// Full-message count that triggers deterministic transcript compaction.
    pub compact_after_messages: usize,
    /// Recent full-message window kept verbatim after compaction.
    pub keep_recent_messages: usize,
    /// Prior chat-level messages to seed the conversation with, so a reply can
    /// continue an ongoing conversation. The current task is appended as
    /// the final user message. Empty = a fresh, single-shot run.
    pub seed_messages: Vec<Message>,
    /// Optional parent conversation identifier.
    pub session_id: Option<String>,
    /// Optional user-turn generation for cancelable background media work.
    pub background_media_generation: Option<u64>,
    /// Optional client task id for aggregating paid cloud-tool usage.
    pub billing_task_id: Option<String>,
    /// Optional host-owned OCR endpoint. Local entrypoints leave this unset.
    pub remote_ocr: Option<RemoteOcrConfig>,
    /// Optional entrypoint-owned recovery hook for transient dependencies used
    /// by tools. A recovered call is retried once; failed recovery switches
    /// the loop to bounded local delivery followed by a partial summary.
    pub tool_failure_recovery: Option<SharedToolFailureRecovery>,
    pub desktop_network_recovery: bool,
    pub model_previews: bool,
    /// Read and update `~/.socai/memories` for this run. Hosted workers turn
    /// this off; their session home is not the durable local task store.
    pub local_memory: bool,
}

impl Default for AgentOptions {
    fn default() -> Self {
        Self {
            run_id: None,
            max_steps: DEFAULT_MAX_STEPS,
            max_tokens: 16000,
            extra_instructions: String::new(),
            run_dir: None,
            enabled_sites: Vec::new(),
            initial_skills: Vec::new(),
            compact_after_messages: DEFAULT_COMPACT_AFTER_MESSAGES,
            keep_recent_messages: DEFAULT_KEEP_RECENT_MESSAGES,
            seed_messages: Vec::new(),
            session_id: None,
            background_media_generation: None,
            billing_task_id: None,
            remote_ocr: None,
            tool_failure_recovery: None,
            desktop_network_recovery: false,
            model_previews: false,
            local_memory: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentOutcome {
    pub run_id: String,
    pub run_dir: PathBuf,
    pub steps: u32,
    pub final_text: String,
    pub usage: TokenUsage,
    /// Terminal error that ended the run early: an unretryable LLM API
    /// error or a failed forced summary.
    /// When set, run.json and the trace already carry status "failed", and
    /// `final_text` is best-effort — an error placeholder, or partial output
    /// from earlier steps — so callers must not report the run as completed.
    pub error: Option<String>,
    /// Why the run completed with a best-effort partial answer, for example
    /// after tools became unavailable, repeated output truncation, or the
    /// execution limit was reached before a requested deliverable was published.
    /// This is not a terminal error: the final LLM summary completed successfully
    /// and `final_text` is visible.
    pub degraded_reason: Option<String>,
}

struct PendingParallelToolCall {
    id: String,
    name: String,
    effective_input: Value,
    repeat_count: u32,
    sequence: u32,
    recorder: ToolCallRecorder,
    tool_ctx: ToolContext,
    progress_rx: tokio::sync::mpsc::UnboundedReceiver<ToolProgressEvent>,
}

struct CompletedParallelToolCall {
    pending: PendingParallelToolCall,
    result: ToolResult,
    error: Option<String>,
    duration_ms: u64,
}

const STRUCTURED_FAILURE_LIMIT: u32 = 3;

#[derive(Default)]
struct ToolFailureCircuit {
    counts: BTreeMap<(String, String), u32>,
}

impl ToolFailureCircuit {
    fn record(&mut self, name: &str, result: &ToolResult) -> Option<String> {
        if !result.failed() {
            self.counts.retain(|(tool, _), _| tool != name);
            return None;
        }
        let reason = structured_dependency_failure_reason(result)?;
        let count = self
            .counts
            .entry((name.to_string(), reason.clone()))
            .or_default();
        *count += 1;
        (*count >= STRUCTURED_FAILURE_LIMIT).then(|| {
            format!(
                "{name} repeatedly returned dependency failure {reason} ({count} times); acquisition stopped to avoid an unbounded retry loop"
            )
        })
    }
}

fn structured_dependency_failure_reason(result: &ToolResult) -> Option<String> {
    let value: Value = serde_json::from_str(&result.flat_text()).ok()?;
    let reason = value
        .get("reason")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("failure")
                .and_then(Value::as_object)
                .and_then(|failure| failure.get("reason"))
                .and_then(Value::as_str)
        })?
        .trim()
        .to_ascii_lowercase();
    matches!(
        reason.as_str(),
        "unexpected_search_redirect"
            | "search_http_error"
            | "search_request_failed"
            | "challenge_required"
            | "rate_limited"
            | "retry_after"
            | "service_unavailable"
            | "upstream_unavailable"
    )
    .then_some(reason)
}

async fn execute_parallel_tool_call(
    tools: &[SharedTool],
    events: &broadcast::Sender<AgentEvent>,
    mut pending: PendingParallelToolCall,
    step: u32,
) -> CompletedParallelToolCall {
    let started = Instant::now();
    let (result, error) = {
        let dispatch = dispatch_tool(
            tools,
            &pending.name,
            &pending.effective_input,
            &pending.tool_ctx,
        );
        tokio::pin!(dispatch);
        let mut progress_open = true;
        let mut watchdog = ReadToolWatchdog::new(tools, &pending.name);
        loop {
            tokio::select! {
                outcome = &mut dispatch => break outcome,
                _ = watchdog.deadline().wait(), if watchdog.enabled => {
                    pending.tool_ctx.record_tool_outcome(&pending.name, &pending.effective_input, false);
                    break watchdog.failure(&pending.name);
                }
                progress = pending.progress_rx.recv(), if progress_open => {
                    match progress {
                        Some(progress) => {
                            watchdog.progress();
                            emit(
                            events,
                            AgentEvent::ToolProgress {
                                id: pending.id.clone(),
                                step,
                                sequence: pending.sequence,
                                name: pending.name.clone(),
                                progress,
                            },
                            );
                        },
                        None => progress_open = false,
                    }
                }
            }
        }
    };
    while let Ok(progress) = pending.progress_rx.try_recv() {
        emit(
            events,
            AgentEvent::ToolProgress {
                id: pending.id.clone(),
                step,
                sequence: pending.sequence,
                name: pending.name.clone(),
                progress,
            },
        );
    }
    CompletedParallelToolCall {
        pending,
        result,
        error,
        duration_ms: started.elapsed().as_millis() as u64,
    }
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

fn resolve_run_id(run_id: Option<String>) -> String {
    run_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(new_run_id)
}

pub async fn run_agent_with_events(
    task: &str,
    backend: Arc<dyn Backend>,
    tools: Vec<SharedTool>,
    options: AgentOptions,
    events: broadcast::Sender<AgentEvent>,
) -> anyhow::Result<AgentOutcome> {
    let run_id = resolve_run_id(options.run_id);
    let run_dir = options.run_dir.unwrap_or_else(|| make_run_dir(task));
    ensure_dir(&run_dir)?;
    let run_state = Arc::new(RunState::new(task));
    let run_recorder = Arc::new(AgentRunRecorder::start(
        &run_dir,
        &run_id,
        options.session_id.as_deref(),
        task,
        backend.provider(),
        backend.model(),
    )?);
    let mut run_trace = RunTraceBuilder::new(
        &run_dir,
        &run_id,
        task,
        backend.provider(),
        backend.model(),
        options.session_id.as_deref(),
        options.seed_messages.len(),
    );

    let mut ctx = ToolContext::new(&run_id, &run_dir)
        .with_usage_recorder(Arc::clone(&run_recorder))
        .with_run_state(Arc::clone(&run_state))
        .with_background_media_generation(options.background_media_generation)
        .with_billing_task_id(options.billing_task_id)
        .with_remote_ocr(options.remote_ocr);
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
    let mut initial_skills = options.initial_skills.clone();
    if find_tool(&tools, "read_skill").is_some() {
        initial_skills.extend(super::skills::initial_social_skills(
            task,
            &options.enabled_sites,
        ));
    }
    let mut seen = BTreeSet::new();
    initial_skills.retain(|name| seen.insert(name.clone()));
    let initial_tool_count = initial_skills.len() as u32;
    if initial_tool_count > 0 {
        // Preparation belongs to the first step, before its model request.
        emit(&events, AgentEvent::Step { step: 1 });
    }
    // Policies are instructions, not fabricated assistant tool calls. Execute
    // the real reader and retain canonical logs/events before the first model.
    let initial_instructions = load_initial_skills(
        &initial_skills,
        &ctx,
        &run_recorder,
        &mut run_trace,
        &events,
    )
    .await?
    .join("\n\n");
    let memory_instructions = options.local_memory.then(memory_prompt).unwrap_or_default();
    let extra_instructions = [
        options.extra_instructions.trim(),
        initial_instructions.trim(),
        memory_instructions.trim(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n");
    let mut anchor_user_index = options.seed_messages.len();
    let is_follow_up = !options.seed_messages.is_empty();
    // Everything before this index is already in the trace: seed messages were
    // uploaded by the earlier turns that share this conversation's trace id,
    // and within the run the marker advances so each `chat` span carries only
    // the messages new since the previous LLM call (see RunTraceBuilder).
    //
    // Known divergence, accepted: follow-up seeds are rebuilt from persisted
    // turn outputs (`Conversation::chat_messages` reads the artifact-enriched
    // report.md), while the earlier turn's span recorded the raw LLMResponse.
    // The joined trace is the per-turn transcript, not a byte-exact replay of
    // the next request — the same class of local-only divergence as
    // compaction rewrites. Tracking a persisted cross-run cursor to close the
    // gap isn't worth the state it would add.
    let mut traced_len = options.seed_messages.len();

    let mut step = 0u32;
    let mut final_text = String::new();
    let mut usage = TokenUsage::default();
    let mut tool_call_history: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut tool_failure_circuit = ToolFailureCircuit::default();
    let mut completed = false;
    let mut terminal_error: Option<String> = None;
    let mut degraded_reason: Option<String> = None;
    let mut extension_stop_reason: Option<String> = None;
    let mut extensions = RunExtensions::default();
    let mut truncation_retries = 0u32;
    let mut invalid_tool_retries = 0u32;
    let mut delivery_steps_left: Option<u32> = None;
    let mut last_system: String = build_system_prompt(&[], &extra_instructions);
    // Pixels are needed for the next model request, not durable chat history.
    let mut pending_tool_images: BTreeMap<String, Vec<ToolResultContent>> = BTreeMap::new();

    loop {
        let max_steps = extensions.execution_ceiling(&ctx, options.max_steps);
        let wrap_up_limit = max_steps.saturating_add(WRAP_UP_STEPS);
        let local_delivery = delivery_steps_left.is_some();
        if let Some(left) = delivery_steps_left.as_mut() {
            if *left == 0 {
                break;
            }
            *left -= 1;
        } else if max_steps == 0 || step >= wrap_up_limit {
            break;
        }
        let wrapping_up = !local_delivery && step >= max_steps;
        step += 1;
        ctx.step = step;
        if step != 1 || initial_tool_count == 0 {
            emit(&events, AgentEvent::Step { step });
        }
        debug!(step, "agent step start");

        let mut schemas = tool_schemas(&tools, &ctx);
        if local_delivery {
            schemas.retain(|schema| {
                find_tool(&tools, &schema.name)
                    .is_some_and(|tool| tool.available_in_local_delivery())
            });
        }
        let tool_names: Vec<&str> = schemas.iter().map(|s| s.name.as_str()).collect();
        let system = build_system_prompt(&tool_names, &extra_instructions);
        last_system = system.clone();
        if compact_messages_for_context(
            &mut messages,
            options.compact_after_messages,
            options.keep_recent_messages,
            &mut anchor_user_index,
            is_follow_up,
        ) {
            // A loaded skill's full instruction may have been compacted out.
            // Require another read before any persistent learning write.
            ctx.clear_loaded_skills();
            // The trace is an append-only diagnostic record. The rewritten
            // transcript is local context management, so restart its cursor
            // rather than attempting to represent the synthetic summary as a
            // normal user message in the trace.
            traced_len = messages.len();
        }
        let mut request_messages = messages.clone();
        let mut image_bytes = 0usize;
        for message in &mut request_messages {
            if let crate::agent::llm::MessageContent::Blocks(blocks) = &mut message.content {
                for block in blocks {
                    if let Block::ToolResult { tool_use_id, content } = block {
                        if let Some(images) = pending_tool_images.get(tool_use_id) {
                            for image in images {
                                if let ToolResultContent::Image { data, .. } = image {
                                    if image_bytes + data.len() <= 8 * 1024 * 1024 {
                                        image_bytes += data.len();
                                        content.push(image.clone());
                                    } else {
                                        content.push(ToolResultContent::Text { text: "[Image not included in this request: visual payload budget reached. Read fewer images at a time or use OCR; do not claim to have viewed this image.]".into() });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        for context in extensions.request_context(
            &ctx,
            if local_delivery || wrapping_up {
                RequestPhase::LocalDelivery
            } else {
                RequestPhase::Action { step, max_steps }
            },
        ) {
            request_messages.push(Message::user(context));
        }
        if wrapping_up {
            let remaining = wrap_up_limit - step + 1;
            request_messages.push(Message::user(format!(
                "Wrap up now. Tool-using steps remaining (including this one): {remaining}. \
                 Stop searching and collecting new material. Use the available tools to finish \
                 the requested work and publish any requested files. Answer as soon as you are ready."
            )));
        }
        let request_payload =
            backend.request_payload(&system, &request_messages, &schemas, options.max_tokens)?;
        run_recorder.record_llm_request(step, &request_payload)?;

        let llm_started = Instant::now();
        let response: LLMResponse = match send_chat(
            &backend,
            &system,
            &request_messages,
            &schemas,
            options.max_tokens,
            step,
            options.model_previews.then_some(&events),
            options
                .desktop_network_recovery
                .then_some((&events, &mut run_trace)),
        )
        .await
        {
            Ok(response) => {
                let duration_ms = llm_started.elapsed().as_millis() as u64;
                run_recorder.record_llm_response(step, &response, duration_ms)?;
                run_trace.record_llm(
                    step,
                    duration_ms,
                    &system,
                    &messages[traced_len..],
                    &response,
                );
                traced_len = messages.len();
                response
            }
            Err(e) => {
                let msg = format!("{e:#}");
                let duration_ms = llm_started.elapsed().as_millis() as u64;
                run_recorder.record_llm_error(step, &msg, duration_ms)?;
                run_trace.record_llm_error(
                    step,
                    duration_ms,
                    &system,
                    &messages[traced_len..],
                    &msg,
                );
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

        pending_tool_images.clear();
        usage += &response.usage;

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
            run_recorder.mark_llm_response_hidden(step)?;
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
                warn!(step, error = %msg, "forcing a concise summary after repeated truncation");
                degraded_reason = Some(msg);
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

        if response.tool_calls.is_empty() && contains_pseudo_tool_call(&visible_texts.join("\n\n")) {
            run_recorder.mark_llm_response_hidden(step)?;
            invalid_tool_retries += 1;
            if invalid_tool_retries > 2 {
                extension_stop_reason = Some("Repeated invalid tool-call text; summarize available progress without claiming unexecuted actions.".into());
                break;
            }
            messages.push(Message::user("Use the actual exposed tool API instead of XML/DSML markup, or answer directly using the available results."));
            continue;
        }

        // Build the assistant block list manually instead of using
        // LLMResponse::to_assistant_blocks() so we can:
        // - drop [Thinking]-prefixed text from history
        // - truncate visible text to ASSISTANT_TEXT_MAX_CHARS, matching
        //   ASSISTANT_TEXT_MAX_CHARS (320 chars)
        let assistant_blocks = build_assistant_blocks(&response, &visible_texts);
        messages.push(Message::assistant_blocks(assistant_blocks));
        // The assistant turn is already on the trace as the previous span's
        // gen_ai.output.messages; don't repeat it in the next input delta.
        traced_len = messages.len();

        if response.tool_calls.is_empty() && !local_delivery && !wrapping_up {
            match extensions.before_finish(&ctx, wrap_up_limit.saturating_sub(step)) {
                FinishDecision::Allow => {}
                FinishDecision::Continue(instruction) => {
                    run_recorder.mark_llm_response_hidden(step)?;
                    messages.push(Message::user(instruction));
                    continue;
                }
                FinishDecision::Partial(reason) => {
                    extension_stop_reason = Some(reason);
                    break;
                }
            }
        }

        for text in &visible_texts {
            emit(
                &events,
                AgentEvent::AssistantText {
                    step,
                    text: text.clone(),
                },
            );
        }
        if !visible_texts.is_empty() {
            final_text = visible_texts.join("\n\n");
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
        let parallel_platform_batch = !local_delivery
            && extension_stop_reason.is_none()
            && degraded_reason.is_none()
            && options.tool_failure_recovery.is_none()
            && tool_calls_have_distinct_parallel_scopes(&tools, &response.tool_calls);
        if parallel_platform_batch {
            info!(
                step,
                tool_calls = response.tool_calls.len(),
                "dispatching independent platform tools concurrently"
            );
            let mut pending_calls = Vec::with_capacity(response.tool_calls.len());
            for (idx, tc) in response.tool_calls.iter().enumerate() {
                let ToolCall { id, name, input } = tc;
                if !local_delivery {
                    extensions.before_tool(&ctx, name);
                }
                let sig = tool_call_signature(name, input);
                let history = tool_call_history.entry(sig).or_default();
                history.push(step);
                let repeat_count = history.len() as u32;
                let sequence = (idx + 1) as u32 + if step == 1 { initial_tool_count } else { 0 };
                let effective_input = if input.is_object() {
                    find_tool(&tools, name)
                        .map(|tool| tool.effective_input(input))
                        .unwrap_or_else(|| input.clone())
                } else {
                    input.clone()
                };
                let recorder =
                    run_recorder.start_tool_call(step, sequence, name, &effective_input)?;
                let (progress_tx, progress_rx) = tokio::sync::mpsc::unbounded_channel();
                let mut tool_ctx = ctx
                    .clone()
                    .with_tool_dir(recorder.dir())
                    .with_progress_sender(Some(progress_tx));
                tool_ctx.active_tool_name = name.clone();
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
                pending_calls.push(PendingParallelToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    effective_input,
                    repeat_count,
                    sequence,
                    recorder,
                    tool_ctx,
                    progress_rx,
                });
            }
            let completed_calls = join_all(
                pending_calls
                    .into_iter()
                    .map(|pending| execute_parallel_tool_call(&tools, &events, pending, step)),
            )
            .await;
            for completed in completed_calls {
                let CompletedParallelToolCall {
                    pending,
                    result,
                    error,
                    duration_ms,
                } = completed;
                let PendingParallelToolCall {
                    id,
                    name,
                    effective_input,
                    repeat_count,
                    sequence,
                    recorder,
                    tool_ctx,
                    ..
                } = pending;
                let duration_s = (duration_ms as f64) / 1000.0;
                if error
                    .as_deref()
                    .is_some_and(|message| message.starts_with(READ_TOOL_DEADLINE_ERROR))
                {
                    degraded_reason = error.clone();
                }
                recorder.finish_blocks(&result.blocks, duration_ms, error.as_deref())?;
                let result_content = tool_result_to_content(&result);
                let content = content_for_log(&result_content);
                run_trace.record_tool(
                    step,
                    sequence,
                    &name,
                    duration_ms,
                    &effective_input,
                    &content,
                    error.as_deref(),
                );
                let flat = result.flat_text();
                let summary = truncate_summary(&flat, 240);
                let failure_circuit_reason = (!local_delivery)
                    .then(|| tool_failure_circuit.record(&name, &result))
                    .flatten();
                if let Some(reason) = &failure_circuit_reason {
                    degraded_reason.get_or_insert_with(|| reason.clone());
                }
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
                run_state.note_tool_result(step, &name, &effective_input, &summary, duration_s);
                let mut history_content = content_for_tool_history(&name, &result_content);
                pending_tool_images.insert(id.clone(), live_tool_images(&result_content));
                let feedback = if local_delivery {
                    Default::default()
                } else {
                    extensions.after_tool(&tool_ctx, &name, &result, error.as_deref())
                };
                for text in feedback.notes.into_iter().rev() {
                    history_content.insert(0, ToolResultContent::Text { text });
                }
                if let Some(reason) = failure_circuit_reason {
                    history_content.insert(
                        0,
                        ToolResultContent::Text {
                            text: format!(
                                "[Tool circuit opened: {reason}. Do not call this tool again.]"
                            ),
                        },
                    );
                }
                extension_stop_reason = extension_stop_reason.or(feedback.stop_reason);
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
                    tool_use_id: id,
                    content: history_content,
                });
            }
        } else {
            for (idx, tc) in response.tool_calls.iter().enumerate() {
                let ToolCall { id, name, input } = tc;
                if !local_delivery {
                    extensions.before_tool(&ctx, name);
                }
                ctx.active_tool_name = name.clone();

                let sig = tool_call_signature(name, input);
                let history = tool_call_history.entry(sig).or_default();
                history.push(step);
                let repeat_count = history.len() as u32;

                let sequence = (idx + 1) as u32 + if step == 1 { initial_tool_count } else { 0 };
                let effective_input = if input.is_object() {
                    find_tool(&tools, name)
                        .map(|tool| tool.effective_input(input))
                        .unwrap_or_else(|| input.clone())
                } else {
                    input.clone()
                };
                let tool_recorder =
                    run_recorder.start_tool_call(step, sequence, name, &effective_input)?;
                let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
                let tool_ctx = ctx
                    .clone()
                    .with_call_recovery(
                        options
                            .tool_failure_recovery
                            .clone()
                            .filter(|_| degraded_reason.is_none()),
                    )
                    .with_tool_dir(tool_recorder.dir())
                    .with_progress_sender(Some(progress_tx));
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
                let unavailable_reason = extension_stop_reason.clone().or_else(|| {
                    degraded_reason.clone().filter(|_| {
                        !find_tool(&tools, name)
                            .is_some_and(|tool| tool.available_in_local_delivery())
                    })
                });
                let (result, error) = if let Some(reason) = unavailable_reason {
                    let error = format!("tool skipped because recovery already failed: {reason}");
                    (ToolResult::text(format!("Error: {error}")), Some(error))
                } else {
                    loop {
                        let dispatch = dispatch_tool(&tools, name, &effective_input, &tool_ctx);
                        tokio::pin!(dispatch);
                        let mut progress_open = true;
                        let mut watchdog = ReadToolWatchdog::new(&tools, name);
                        let outcome = loop {
                            tokio::select! {
                                outcome = &mut dispatch => break outcome,
                                _ = watchdog.deadline().wait(), if watchdog.enabled => {
                                    tool_ctx.record_tool_outcome(name, &effective_input, false);
                                    break watchdog.failure(name);
                                }
                                progress = progress_rx.recv(), if progress_open => {
                                    match progress {
                                        Some(progress) => {
                                            watchdog.progress();
                                            emit(
                                            &events,
                                            AgentEvent::ToolProgress {
                                                id: id.clone(),
                                                step,
                                                sequence,
                                                name: name.clone(),
                                                progress,
                                            },
                                            );
                                        },
                                        None => progress_open = false,
                                    }
                                }
                            }
                        };
                        if outcome
                            .1
                            .as_deref()
                            .is_some_and(|message| message.starts_with(READ_TOOL_DEADLINE_ERROR))
                        {
                            degraded_reason = outcome.1.clone();
                            break outcome;
                        }
                        match tool_ctx.recover_dependency().await {
                            ToolRecoveryOutcome::NotNeeded => break outcome,
                            ToolRecoveryOutcome::Recovered => {
                                info!(
                                    step,
                                    sequence,
                                    tool = name,
                                    "tool dependency recovered; retrying"
                                );
                            }
                            ToolRecoveryOutcome::Degraded { reason } => {
                                degraded_reason = Some(reason.clone());
                                let (mut result, _) = outcome;
                                result.blocks.insert(
                                0,
                                ToolResultBlock::text(format!(
                                    "[Browser tools are unavailable: {reason}. Use prior results for the final answer.]"
                                )),
                            );
                                break (result, Some(reason));
                            }
                        }
                    }
                };
                while let Ok(progress) = progress_rx.try_recv() {
                    emit(
                        &events,
                        AgentEvent::ToolProgress {
                            id: id.clone(),
                            step,
                            sequence,
                            name: name.clone(),
                            progress,
                        },
                    );
                }
                let duration_ms = started.elapsed().as_millis() as u64;
                let duration_s = (duration_ms as f64) / 1000.0;
                tool_recorder.finish_blocks(&result.blocks, duration_ms, error.as_deref())?;

                let result_content = tool_result_to_content(&result);
                let content = content_for_log(&result_content);
                run_trace.record_tool(
                    step,
                    sequence,
                    name,
                    duration_ms,
                    &effective_input,
                    &content,
                    error.as_deref(),
                );
                let flat = result.flat_text();
                let summary = truncate_summary(&flat, 240);
                let failure_circuit_reason = (!local_delivery)
                    .then(|| tool_failure_circuit.record(name, &result))
                    .flatten();
                if let Some(reason) = &failure_circuit_reason {
                    degraded_reason.get_or_insert_with(|| reason.clone());
                }
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
                let mut history_content = content_for_tool_history(name, &result_content);
                pending_tool_images.insert(id.clone(), live_tool_images(&result_content));
                let feedback = if local_delivery {
                    Default::default()
                } else {
                    extensions.after_tool(&tool_ctx, &name, &result, error.as_deref())
                };
                for text in feedback.notes.into_iter().rev() {
                    history_content.insert(0, ToolResultContent::Text { text });
                }
                if let Some(reason) = failure_circuit_reason {
                    history_content.insert(
                        0,
                        ToolResultContent::Text {
                            text: format!(
                                "[Tool circuit opened: {reason}. Do not call this tool again.]"
                            ),
                        },
                    );
                }
                extension_stop_reason = extension_stop_reason.or(feedback.stop_reason);
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

                ctx.active_tool_name.clear();
            }
        }
        usage += &ctx.take_auxiliary_usage();
        messages.push(Message::user_blocks(tool_result_blocks));
        if extension_stop_reason.is_some() {
            break;
        }
        if degraded_reason.is_some() && delivery_steps_left.is_none() {
            if !tools
                .iter()
                .any(|tool| tool.available_in_local_delivery() && tool.is_available(&ctx))
            {
                break;
            }
            let remaining = if step >= max_steps {
                wrap_up_limit - step
            } else {
                8
            };
            delivery_steps_left = Some(remaining);
            messages.push(Message::user(format!(
                "External dependency recovery failed. Acquisition is over; the result remains partial. You now have up to {remaining} local-only steps to read existing material, create requested deliverables with write_file, and publish them. Only exposed local tools may be called; no shell, browser, new searches or external services. Use real tool calls, not XML/DSML markup. Do not claim a file is attached before publish_artifact succeeds. Then give a useful partial answer explaining the interruption."
            )));
        }
    }

    let max_steps = extensions.execution_ceiling(&ctx, options.max_steps);
    if !completed && terminal_error.is_none() && step >= max_steps {
        extension_stop_reason =
            extension_stop_reason.or_else(|| extensions.execution_limit_reason(&ctx));
    }
    let forced_summary_prompt = extension_stop_reason
        .as_ref()
        .map(|reason| format!(
            "{reason} Do not call more tools. Produce a complete user-visible partial answer \
             in the task's language using collected evidence only. State the completed scope, \
             missing information and useful next steps. Do not claim the original task is complete."
        ))
        .or_else(|| degraded_reason
        .as_ref()
        .map(|reason| {
            info!(step, reason, "degraded execution; forcing partial summary");
            format!(
                "The task could not continue normally: {reason}. \
                 Do not call any more tools. Produce the best possible final answer now in the same \
                 language as the original task, using only evidence already present in the tool \
                 results and conversation. Clearly label the answer as partial, distinguish verified \
                 facts from missing information, and never guess or invent values."
            )
        }))
        .or_else(|| {
            (step >= max_steps).then(|| {
                info!(step, "reached max_steps, forcing final summary");
                format!(
                    "You have reached the maximum of {} tool-using steps. Do not call any \
                     more tools. Based on the evidence already gathered, produce the best \
                     possible final answer for the user now in the same language as the \
                     original task. If information is incomplete, state what is known, \
                     what is missing, and give your best-effort conclusion.",
                    step
                )
            })
        });
    if !completed && terminal_error.is_none() {
        let Some(mut forced_summary_prompt) = forced_summary_prompt else {
            unreachable!("an incomplete run must have a forced-summary reason")
        };
        for summary_attempt in 0..2u32 {
            let summary_step = step + 1 + summary_attempt;
            messages.push(Message::user(forced_summary_prompt));
            if compact_messages_for_context(
                &mut messages,
                options.compact_after_messages,
                options.keep_recent_messages,
                &mut anchor_user_index,
                is_follow_up,
            ) {
                traced_len = messages.len();
            }
            let mut request_messages = messages.clone();
            for context in extensions.request_context(&ctx, RequestPhase::Summary) {
                request_messages.push(Message::user(context));
            }
            let request_payload = backend.request_payload(
                &last_system,
                &request_messages,
                &[],
                options.max_tokens,
            )?;
            run_recorder.record_llm_request(summary_step, &request_payload)?;
            let llm_started = Instant::now();
            match send_chat(
                &backend,
                &last_system,
                &request_messages,
                &[],
                options.max_tokens,
                summary_step,
                options.model_previews.then_some(&events),
                options
                    .desktop_network_recovery
                    .then_some((&events, &mut run_trace)),
            )
            .await
            {
                Ok(response) => {
                    let duration_ms = llm_started.elapsed().as_millis() as u64;
                    // Incomplete summary attempts stay hidden from timeline replay.
                    run_recorder.record_llm_response_with_visibility(
                        summary_step,
                        &response,
                        duration_ms,
                        false,
                    )?;
                    run_trace.record_llm(
                        summary_step,
                        duration_ms,
                        &last_system,
                        &messages[traced_len..],
                        &response,
                    );
                    traced_len = messages.len();
                    usage += &response.usage;
                    let (visible_texts, _) = split_thinking(&response.text_blocks);
                    let visible_text = visible_texts.join("\n\n");
                    let invalid_summary = response.stop_reason == StopReason::MaxTokens
                        || visible_texts.is_empty()
                        || !response.tool_calls.is_empty()
                        || contains_pseudo_tool_call(&visible_text);
                    if invalid_summary {
                        warn!(
                            step = summary_step,
                            summary_attempt,
                            stop_reason = ?response.stop_reason,
                            visible_blocks = visible_texts.len(),
                            tool_calls = response.tool_calls.len(),
                            "forced summary was incomplete"
                        );
                        if summary_attempt == 0 {
                            forced_summary_prompt =
                                "Your previous tool-free summary was incomplete or truncated. Do not call tools, emit XML/DSML tool markup, promise to write files, or include reasoning-only output. Only claim published files when a successful tool result exists. Respond once with a concise, complete user-visible answer based only on the gathered evidence, clearly marking any missing information."
                                    .to_string();
                            continue;
                        }
                        let msg = format!(
                            "forced summary did not produce complete visible text after two attempts (stop reason: {:?})",
                            response.stop_reason
                        );
                        emit(
                            &events,
                            AgentEvent::ApiError {
                                step: summary_step,
                                message: msg.clone(),
                            },
                        );
                        terminal_error = Some(msg);
                        break;
                    }
                    run_recorder.mark_llm_response_visible(summary_step)?;
                    for text in &visible_texts {
                        emit(
                            &events,
                            AgentEvent::AssistantText {
                                step: summary_step,
                                text: text.clone(),
                            },
                        );
                    }
                    final_text = visible_text;
                    break;
                }
                Err(e) => {
                    let msg = format!("{e:#}");
                    let duration_ms = llm_started.elapsed().as_millis() as u64;
                    run_recorder.record_llm_error(summary_step, &msg, duration_ms)?;
                    run_trace.record_llm_error(
                        summary_step,
                        duration_ms,
                        &last_system,
                        &messages[traced_len..],
                        &msg,
                    );
                    warn!(step = summary_step, error = %msg, "forced summary error");
                    emit(
                        &events,
                        AgentEvent::ApiError {
                            step: summary_step,
                            message: msg.clone(),
                        },
                    );
                    terminal_error = Some(msg);
                    break;
                }
            }
        }
    }

    // A degraded reason represents a successfully delivered partial answer,
    // whether recovery failed or the execution budget ended before delivery.
    // If the summary itself failed, persist and return only the terminal error.
    let completed_degraded_reason = terminal_error
        .is_none()
        .then(|| {
            degraded_reason
                .clone()
                .or(extension_stop_reason)
                .or_else(|| extensions.partial_reason(&ctx))
        })
        .flatten();

    // Failed runs already signalled ApiError; a Done event on top would give
    // subscribers contradictory success ("✓ done") and failure signals.
    if terminal_error.is_none() {
        emit(
            &events,
            AgentEvent::Done {
                run_id: run_id.clone(),
                steps: step,
                final_text: final_text.clone(),
                partial: completed_degraded_reason.is_some(),
                degraded_reason: completed_degraded_reason.clone(),
            },
        );
    }

    let enriched_report = report_with_artifacts(&final_text, Some(&run_state));
    let _ = std::fs::write(run_dir.join("report.md"), &enriched_report);
    if options.local_memory && terminal_error.is_none() {
        local_memory::record_completed_run(
            backend.as_ref(),
            &run_id,
            &run_dir,
            options.session_id.as_deref(),
            task,
            &enriched_report,
        )
        .await;
    }

    let status = if terminal_error.is_some() {
        "failed"
    } else {
        "completed"
    };
    run_recorder.finish(
        status,
        step,
        &usage,
        terminal_error.as_deref(),
        completed_degraded_reason.as_deref(),
    )?;
    run_trace.finish(
        status,
        step,
        &usage,
        terminal_error.as_deref(),
        completed_degraded_reason.as_deref(),
    );

    Ok(AgentOutcome {
        run_id,
        run_dir,
        steps: step,
        final_text,
        usage,
        error: terminal_error,
        degraded_reason: completed_degraded_reason,
    })
}

// ---------- small private helpers (not core logic, kept here for locality) ----------

/// Two ordinary resubmissions. A detected suspend on the last desktop
/// transport attempt grants one further attempt after the route responds.
const CHAT_RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(6)];

/// `backend.send` with retries for transient failures (network transport
/// errors, 408/429/5xx) — a multi-minute run should not die on one dropped
/// packet. Permanent errors (auth, billing, bad request) surface on the
/// first attempt.
pub(crate) async fn send_with_retry(
    backend: &Arc<dyn Backend>,
    system: &str,
    messages: &[Message],
    schemas: &[ToolSchema],
    max_tokens: u32,
    step: u32,
) -> anyhow::Result<LLMResponse> {
    send_chat(backend, system, messages, schemas, max_tokens, step, None, None).await
}

async fn send_chat(
    backend: &Arc<dyn Backend>,
    system: &str,
    messages: &[Message],
    schemas: &[ToolSchema],
    max_tokens: u32,
    step: u32,
    previews: Option<&broadcast::Sender<AgentEvent>>,
    mut recovery: Option<(&broadcast::Sender<AgentEvent>, &mut RunTraceBuilder)>,
) -> anyhow::Result<LLMResponse> {
    use crate::util::suspend_clock::SuspendClock;
    let mut attempt = 0;
    loop {
        attempt += 1;
        let start = recovery.as_ref().and_then(|_| SuspendClock::sample());
        let result = if let Some(events) = previews {
            let preview = |text: &str, reasoning: &str| {
                emit(
                    events,
                    AgentEvent::ModelPreview {
                        text: text.to_owned(),
                        reasoning: reasoning.to_owned(),
                    },
                );
            };
            let result = backend
                .send_with_preview(system, messages, schemas, max_tokens, &preview)
                .await;
            preview("", "");
            result
        } else {
            backend.send(system, messages, schemas, max_tokens).await
        };
        let timing = start.and_then(SuspendClock::elapsed);
        let monitor_connection = recovery.is_some();
        let mut notify = |state, reason| {
            if let Some((events, trace)) = recovery.as_mut() {
                trace.record_model_connection(step, attempt, state, reason, timing);
                emit(
                    events,
                    AgentEvent::ModelConnection {
                        step,
                        attempt,
                        provider: backend.provider().to_owned(),
                        state,
                        reason,
                        timing,
                    },
                );
            }
        };
        let error = match result {
            Ok(response) => {
                if attempt > 1 || timing.is_some_and(|t| t.slept()) {
                    notify("completed", "response_received");
                }
                return Ok(response);
            }
            Err(error) => error,
        };
        if !is_transient_api_error(&error) {
            return Err(error);
        }
        let transport = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<reqwest::Error>());
        let reason = match transport {
            Some(e) if e.is_connect() => "connect_failed",
            Some(e) if e.is_timeout() => "timeout",
            Some(_) => "transport_error",
            None => "provider_error",
        };
        let delay = if let Some(delay) = CHAT_RETRY_DELAYS.get(attempt - 1) {
            *delay
        } else if attempt == CHAT_RETRY_DELAYS.len() + 1
            && transport.is_some()
            && timing.is_some_and(|t| t.slept())
        {
            // Even the final normal attempt can be interrupted by closing the lid.
            Duration::from_secs(2)
        } else {
            notify("exhausted", reason);
            return Err(error);
        };
        notify("retry_scheduled", reason);
        warn!(step, attempt, delay_secs = delay.as_secs(), error = %format!("{error:#}"), "transient LLM error; retrying");
        tokio::time::sleep(delay).await;
        if monitor_connection && transport.is_some() {
            let mut wait = Duration::from_secs(2);
            while let Some(probe) = backend.connectivity_probe() {
                if probe.timeout(Duration::from_secs(5)).send().await.is_ok() {
                    break;
                }
                if wait == Duration::from_secs(2) {
                    notify("waiting_for_network", reason);
                }
                // The task remains cancellable. Waiting does not spend a model attempt.
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(Duration::from_secs(30));
            }
        }
        notify("retrying", reason);
    }
}

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

fn tool_calls_have_distinct_parallel_scopes(tools: &[SharedTool], calls: &[ToolCall]) -> bool {
    if calls.len() < 2 {
        return false;
    }
    let mut scopes = BTreeSet::new();
    calls.iter().all(|call| {
        find_tool(tools, &call.name)
            .and_then(|tool| tool.parallel_scope())
            .is_some_and(|scope| !scope.is_empty() && scopes.insert(scope.to_string()))
    })
}

/// Runtime preparation uses the same real skill reader as model tool calls.
/// Keep ctx.step at zero so activation precedes the first model step, while
/// recording these calls as the first step's preparation (never step zero).
async fn load_initial_skills(
    names: &[String],
    ctx: &ToolContext,
    run_recorder: &AgentRunRecorder,
    run_trace: &mut RunTraceBuilder,
    events: &broadcast::Sender<AgentEvent>,
) -> anyhow::Result<Vec<String>> {
    let tool = super::skills::ReadSkillTool::new(super::skills::default_skills_root());
    let mut instructions = Vec::new();
    for (idx, name) in names.iter().enumerate() {
        let sequence = (idx + 1) as u32;
        let input = json!({"name": name});
        let id = format!("initial-skill-{}-{sequence}", ctx.run_id);
        let recorder = run_recorder.start_tool_call(1, sequence, "read_skill", &input)?;
        let mut tool_ctx = ctx.clone().with_tool_dir(recorder.dir());
        tool_ctx.active_tool_name = "read_skill".to_string();
        emit(
            events,
            AgentEvent::ToolCall {
                id: id.clone(),
                step: 1,
                sequence,
                name: "read_skill".to_string(),
                input: input.clone(),
                repeat_count: 1,
            },
        );
        let started = Instant::now();
        let (result, error) = match tool.call(input.clone(), &tool_ctx).await {
            Ok(result) => {
                let error = result.failed().then(|| result.flat_text());
                (result, error)
            }
            Err(error) => {
                let message = format!("{error:#}");
                (ToolResult::failure(message.clone()), Some(message))
            }
        };
        let duration_ms = started.elapsed().as_millis() as u64;
        recorder.finish_blocks(&result.blocks, duration_ms, error.as_deref())?;
        let content = content_for_log(&tool_result_to_content(&result));
        run_trace.record_tool(
            1,
            sequence,
            "read_skill",
            duration_ms,
            &input,
            &content,
            error.as_deref(),
        );
        let flat = result.flat_text();
        emit(
            events,
            AgentEvent::ToolResult {
                id,
                step: 1,
                sequence,
                name: "read_skill".to_string(),
                input,
                content,
                summary: truncate_summary(&flat, 240),
                duration_ms,
                error: error.clone(),
            },
        );
        if let Some(error) = error {
            anyhow::bail!("Initial skill `{name}` failed: {error}");
        }
        instructions.push(format!(
            "Request method, already loaded by runtime preparation:\n{flat}"
        ));
    }
    Ok(instructions)
}

fn emit(events: &broadcast::Sender<AgentEvent>, event: AgentEvent) {
    let _ = events.send(event);
}

/// Hosted read-only research must return control to the model before the
/// backend's outer inactivity watchdog kills the entire run. Only genuine
/// tool progress renews the idle deadline; a separate active-time limit bounds
/// pathological tools that keep emitting progress forever. Opt-in keeps CLI
/// and Desktop behavior unchanged. Never cancel external writes this way.
const READ_TOOL_DEADLINE_ERROR: &str = "[socai:read-deadline]";

struct ReadToolWatchdog {
    enabled: bool,
    idle: Duration,
    clock: ExecutionClock,
    idle_until: Duration,
    hard_until: Duration,
}

impl ReadToolWatchdog {
    fn new(tools: &[SharedTool], name: &str) -> Self {
        let idle_seconds = std::env::var("SOCAI_READ_TOOL_IDLE_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| (1..=300).contains(value));
        let wall_seconds = std::env::var("SOCAI_READ_TOOL_WALL_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| (1..=3600).contains(value))
            .unwrap_or(900);
        let read_name = name.rsplit("__").next().unwrap_or(name);
        let read_only = matches!(
            read_name,
            "search" | "get_notes" | "author_scan" | "web_read" | "web_navigate"
        ) && find_tool(tools, name).is_some_and(|tool| !tool.external_write());
        let clock = ExecutionClock::current();
        let now = clock.elapsed();
        let idle = Duration::from_secs(idle_seconds.unwrap_or(90));
        Self {
            enabled: read_only && idle_seconds.is_some(),
            idle,
            clock,
            idle_until: now + idle,
            hard_until: now + Duration::from_secs(wall_seconds),
        }
    }

    fn progress(&mut self) {
        self.idle_until = self.clock.elapsed() + self.idle;
    }

    fn deadline(&self) -> ExecutionDeadline {
        self.clock.deadline_at(self.idle_until.min(self.hard_until))
    }

    fn failure(&self, name: &str) -> (ToolResult, Option<String>) {
        let message = format!(
            "{READ_TOOL_DEADLINE_ERROR} Read-only tool {name} exceeded its progress or active-execution deadline. \
            Do not repeat the same bulk request. Review any saved evidence and files, \
            then provide an explicitly partial answer with sources. If nothing was \
            saved, explain that limitation instead of claiming the research completed."
        );
        warn!(
            tool = name,
            "read-only research tool timed out; returning control to the model"
        );
        (ToolResult::failure(message.clone()), Some(message))
    }
}

async fn dispatch_tool(
    tools: &[SharedTool],
    name: &str,
    input: &Value,
    ctx: &ToolContext,
) -> (ToolResult, Option<String>) {
    if !input.is_object() {
        let error = "Tool arguments were not a valid JSON object. No action was executed. Resend this call with well-formed JSON arguments.".to_string();
        ctx.record_tool_outcome(name, input, false);
        return (ToolResult::failure(&error), Some(error));
    }
    let outcome = match find_tool(tools, name) {
        Some(tool) if tool.is_available(ctx) => match tool.call(input.clone(), ctx).await {
            Ok(r) => {
                // A tool can report a trusted failure without throwing. Keep
                // that outcome in events/traces as well as recovery evidence;
                // never infer success from an Ok transport or from page text.
                let error = r.failed().then(|| truncate_summary(&r.flat_text(), 240));
                (r, error)
            }
            Err(e) => {
                let msg = format!("{e:#}");
                (
                    ToolResult::failure(format!("Error executing {name}: {msg}")),
                    Some(msg),
                )
            }
        },
        Some(_) => {
            let msg = format!("Tool '{name}' is not currently available");
            (ToolResult::failure(format!("Error: {msg}")), Some(msg))
        }
        None => {
            let msg = format!("Unknown tool '{name}'");
            (ToolResult::failure(format!("Error: {msg}")), Some(msg))
        }
    };
    let succeeded = outcome.1.is_none() && !outcome.0.failed();
    ctx.record_tool_outcome(name, input, succeeded);
    outcome
}

fn tool_result_to_content(result: &ToolResult) -> Vec<ToolResultContent> {
    tool_result_blocks_to_content(&result.blocks)
}

pub(crate) fn live_tool_images(content: &[ToolResultContent]) -> Vec<ToolResultContent> {
    content.iter().filter_map(|block| match block {
        ToolResultContent::Image { data, .. } if data.len() <= 6 * 1024 * 1024 => Some(block.clone()),
        _ => None,
    }).collect()
}

fn tool_result_blocks_to_content(blocks: &[ToolResultBlock]) -> Vec<ToolResultContent> {
    blocks
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

/// Rebuild the model-facing form of a persisted tool result. This is
/// used when a later run resumes an interrupted turn: raw `output.json` stays
/// the source of truth, while the replayed context matches the same bounds as
/// the live agent loop, including intact Browser Script DOM evidence.
pub(crate) fn tool_result_blocks_for_history(
    name: &str,
    blocks: &[ToolResultBlock],
) -> Vec<ToolResultContent> {
    content_for_tool_history(name, &tool_result_blocks_to_content(blocks))
}

fn content_for_tool_history(name: &str, content: &[ToolResultContent]) -> Vec<ToolResultContent> {
    // DOM evidence must reach the model intact. The browser runtime already
    // rejects oversized results; generic JSON compaction would reduce the
    // entire slim DOM string to 320 characters and hide the relevant controls.
    if name == "browser_script" {
        return content.to_vec();
    }
    bound_content_for_history(content)
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

/// Prompt-driven thinking convention: models without a native thinking
/// channel are asked to prefix reasoning text with this marker. Shared with
/// the run-trace builder so those blocks upload as reasoning, not answer text.
pub(crate) const THINKING_TEXT_PREFIX: &str = "[Thinking] ";

/// Split assistant text blocks into (visible, thinking) by the `[Thinking] `
/// prefix. Whitespace-only blocks are dropped from both buckets.
fn split_thinking(text_blocks: &[String]) -> (Vec<String>, Vec<String>) {
    let mut visible: Vec<String> = Vec::new();
    let mut thinking: Vec<String> = Vec::new();
    for block in text_blocks {
        let trimmed = block.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix(THINKING_TEXT_PREFIX) {
            thinking.push(rest.trim().to_string());
        } else {
            visible.push(trimmed.to_string());
        }
    }
    (visible, thinking)
}

fn contains_pseudo_tool_call(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "<｜｜dsml｜｜tool_calls",
        "<｜dsml｜tool_calls",
        "&lt;｜｜dsml｜｜tool_calls",
        "<tool_calls",
        "<function_calls",
        "<invoke",
        "<function name=",
        "&lt;tool_calls",
        "&lt;function_calls",
        "&lt;invoke",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
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
    // Preserve every reasoning_content response, including text-only turns.
    // DeepSeek thinking mode requires all prior reasoning to be replayed when
    // a later request carries tools, not only turns that called a tool.
    if response.thinking_blocks.is_empty()
        && response.reasoning_items.is_empty()
        && !response.reasoning_content.trim().is_empty()
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

/// Rebuild the exact bounded assistant history form from a persisted response.
/// Interrupted-run replay uses this instead of exposing reasoning-only text or
/// growing history beyond the limits applied during the original live run.
pub(crate) fn assistant_blocks_for_history(response: &LLMResponse) -> Vec<Block> {
    let (visible_texts, _) = split_thinking(&response.text_blocks);
    build_assistant_blocks(response, &visible_texts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::file_bash_tools::ShellTool;
    use tempfile::tempdir;

    #[test]
    fn caller_owned_run_id_is_preserved_for_interactive_signals() {
        assert_eq!(
            resolve_run_id(Some("orchestrator-run-1".into())),
            "orchestrator-run-1"
        );
        assert!(!resolve_run_id(Some("  ".into())).trim().is_empty());
    }

    #[test]
    fn pseudo_tool_markup_is_not_a_valid_summary() {
        assert!(contains_pseudo_tool_call(
            "<｜｜DSML｜｜tool_calls><｜｜DSML｜｜invoke name=\"shell\">"
        ));
        assert!(contains_pseudo_tool_call(
            "<function_calls><invoke name=\"publish_artifact\">"
        ));
    }

    #[test]
    fn repeated_structured_dependency_failures_trip_across_changed_arguments() {
        let mut circuit = ToolFailureCircuit::default();
        let failed = ToolResult::failure(
            json!({"ok": false, "reason": "unexpected_search_redirect"}).to_string(),
        );

        assert!(circuit.record("weixin_search", &failed).is_none());
        assert!(circuit.record("weixin_search", &failed).is_none());
        let reason = circuit.record("weixin_search", &failed).unwrap();

        assert!(reason.contains("weixin_search"));
        assert!(reason.contains("unexpected_search_redirect"));
    }

    #[test]
    fn structured_failure_circuit_resets_after_success_and_ignores_user_action() {
        let mut circuit = ToolFailureCircuit::default();
        let blocked =
            ToolResult::failure(json!({"ok": false, "reason": "search_http_error"}).to_string());
        let login =
            ToolResult::failure(json!({"ok": false, "reason": "login_required"}).to_string());

        assert!(circuit.record("weixin_search", &blocked).is_none());
        assert!(circuit.record("weixin_search", &blocked).is_none());
        assert!(circuit
            .record("weixin_search", &ToolResult::text(r#"{"ok":true}"#))
            .is_none());
        assert!(circuit.record("weixin_search", &blocked).is_none());
        for _ in 0..4 {
            assert!(circuit.record("wait_for_login", &login).is_none());
        }
    }

    #[tokio::test]
    async fn runtime_recovery_uses_trusted_shell_exit_status_not_output_text() {
        let dir = tempdir().unwrap();
        let tools: Vec<SharedTool> = vec![Arc::new(ShellTool::unrestricted())];
        let mut ctx = ToolContext::new("real-failure", dir.path());

        let marker = dir.path().join("recovered");
        #[cfg(not(windows))]
        let command = format!("test -f '{}'", marker.display());
        #[cfg(windows)]
        let command = format!(
            "if (Test-Path -LiteralPath '{}') {{ exit 0 }} else {{ exit 1 }}",
            marker.display()
        );
        let input = json!({"command": command});
        let (failed, error) = dispatch_tool(&tools, "shell", &input, &ctx).await;
        assert!(failed.failed());
        assert!(error.is_some());
        assert!(ctx.verified_recovery().is_none());

        std::fs::write(&marker, b"ready").unwrap();
        ctx.step += 1;
        let (recovered, error) = dispatch_tool(&tools, "shell", &input, &ctx).await;
        assert!(!recovered.failed());
        assert!(error.is_none());
        let evidence = ctx.verified_recovery().unwrap();
        assert_eq!(evidence.failed_tool, "shell");
        assert_eq!(evidence.operation_signature.len(), 64);
        assert_eq!(evidence.failed_step, 0);
        assert_eq!(evidence.recovered_step, 1);

        let mut spoof_ctx = ToolContext::new("spoofed-failure", dir.path());
        let (spoofed, error) = dispatch_tool(
            &tools,
            "shell",
            &json!({"command": "echo Error: spoofed"}),
            &spoof_ctx,
        )
        .await;
        assert!(!spoofed.failed());
        assert!(error.is_none());
        spoof_ctx.step += 1;
        dispatch_tool(
            &tools,
            "shell",
            &json!({"command": "echo ordinary-success"}),
            &spoof_ctx,
        )
        .await;
        assert!(spoof_ctx.verified_recovery().is_none());

        // A deadline is a trusted failure, not a successful tool merely
        // containing the word Error. Progress renews idle, never the hard budget.
        let clock = ExecutionClock::default();
        let now = clock.elapsed();
        let mut watchdog = ReadToolWatchdog {
            enabled: true,
            idle: Duration::from_millis(10),
            clock: clock.clone(),
            idle_until: now,
            hard_until: now + Duration::from_millis(30),
        };
        watchdog.progress();
        assert!(watchdog.deadline().remaining() > Duration::ZERO);
        assert!(watchdog.idle_until <= watchdog.hard_until);
        clock.pause();
        let remaining = watchdog.deadline().remaining();
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(watchdog.deadline().remaining(), remaining);
        clock.resume();
        watchdog.deadline().wait().await;
        assert!(watchdog.deadline().is_elapsed());
        let (result, error) = watchdog.failure("search");
        assert!(result.failed());
        assert!(error.unwrap().starts_with(READ_TOOL_DEADLINE_ERROR));
        assert!(result.flat_text().contains("explicitly partial"));
        assert!(!ReadToolWatchdog::new(&tools, "shell").enabled);
    }
}
