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
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::agent::api_errors::is_transient_api_error;
use crate::agent::compaction::{compress_text_maybe_json, TOOL_RESULT_TEXT_MAX_CHARS};
use crate::agent::llm::{
    Backend, Block, LLMResponse, Message, StopReason, TokenUsage, ToolCall, ToolResultContent,
    ToolSchema,
};
use crate::agent::memory::{
    compact_messages_for_context, DEFAULT_COMPACT_AFTER_MESSAGES, DEFAULT_KEEP_RECENT_MESSAGES,
};
use crate::agent::report::report_with_artifacts;
use crate::agent::research_workflow::{
    ResearchPrepareContext, ResearchWorkflow, ResponseDirective, WorkflowIo,
};
use crate::agent::research_workspace::{ResearchWorkspaceStatus, ResearchWorkspaceStatusGuard};
use crate::agent::run_logging::{make_run_dir, AgentRunRecorder};
use crate::agent::run_state::RunState;
use crate::agent::signature::tool_call_signature;
use crate::agent::system_prompt::build_system_prompt;
use crate::agent::tool::{
    SharedTool, SharedToolFailureRecovery, ToolContext, ToolProgressEvent, ToolRecoveryOutcome,
    ToolResult, ToolResultBlock,
};
use crate::telemetry::trace::RunTraceBuilder;

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

#[derive(Debug, Clone)]
pub struct AgentOptions {
    pub max_steps: u32,
    pub max_tokens: u32,
    pub extra_instructions: String,
    pub run_dir: Option<PathBuf>,
    /// Site names to pre-enable in ToolContext (gates `defer_until_site` tools).
    pub enabled_sites: Vec<String>,
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
    /// Optional entrypoint-owned recovery hook for transient dependencies used
    /// by tools. A recovered call is retried once; failed recovery switches
    /// the loop to a tool-free best-effort summary.
    pub tool_failure_recovery: Option<SharedToolFailureRecovery>,
}

impl Default for AgentOptions {
    fn default() -> Self {
        Self {
            max_steps: 30,
            max_tokens: 16000,
            extra_instructions: String::new(),
            run_dir: None,
            enabled_sites: Vec::new(),
            compact_after_messages: DEFAULT_COMPACT_AFTER_MESSAGES,
            keep_recent_messages: DEFAULT_KEEP_RECENT_MESSAGES,
            seed_messages: Vec::new(),
            session_id: None,
            background_media_generation: None,
            billing_task_id: None,
            tool_failure_recovery: None,
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
    /// error, repeated max-token truncation, or a failed forced summary.
    /// When set, run.json and the trace already carry status "failed", and
    /// `final_text` is best-effort — an error placeholder, or partial output
    /// from earlier steps — so callers must not report the run as completed.
    pub error: Option<String>,
    /// Why the run completed with a best-effort partial answer after tools
    /// became unavailable. This is not a terminal error: the final LLM summary
    /// completed successfully and `final_text` is user-visible.
    pub degraded_reason: Option<String>,
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
        backend.provider(),
        backend.model(),
    )?;
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
        .with_run_state(Arc::clone(&run_state))
        .with_background_media_generation(options.background_media_generation)
        .with_billing_task_id(options.billing_task_id);
    for site in &options.enabled_sites {
        ctx.enable_site(site.clone());
    }

    let mut messages: Vec<Message> = options.seed_messages.clone();
    messages.push(Message::user(task.to_string()));
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

    emit(
        &events,
        AgentEvent::Started {
            run_id: run_id.clone(),
            task: task.to_string(),
            model: backend.label(),
        },
    );

    let prepared = ResearchWorkflow::prepare(ResearchPrepareContext {
        task,
        backend: &backend,
        seed_messages: &options.seed_messages,
        enabled_sites: &options.enabled_sites,
        max_tokens: options.max_tokens,
        compact_after_messages: options.compact_after_messages,
        keep_recent_messages: options.keep_recent_messages,
        run_dir: &run_dir,
        extra_instructions: &options.extra_instructions,
        recorder: &run_recorder,
        trace: &mut run_trace,
    })
    .await?;
    let mut workflow = prepared.workflow;
    let mut step = prepared.planning_steps;
    let mut completed = prepared.immediate_text.is_some();
    let mut final_text = prepared.immediate_text.unwrap_or_default();
    let mut usage = prepared.usage;
    let mut tool_call_history: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut terminal_error: Option<String> = None;
    let mut degraded_reason: Option<String> = None;
    let mut terminal_finalize_reason: Option<String> = None;
    let mut truncation_retries = 0u32;
    let mut last_system: String = build_system_prompt(&[], &workflow.system_instructions());
    if let Err(error) =
        workflow.persist_workspace(&run_dir, &run_state, ResearchWorkspaceStatus::Running)
    {
        warn!(%error, "failed to persist initial research workspace");
    }
    let mut workspace_status_guard =
        ResearchWorkspaceStatusGuard::new(&run_dir, workflow.is_active());

    macro_rules! workflow_io {
        () => {
            WorkflowIo {
                backend: &backend,
                messages: &mut messages,
                run_dir: &run_dir,
                run_state: &run_state,
                recorder: &run_recorder,
                trace: &mut run_trace,
                usage: &mut usage,
                events: &events,
                max_tokens: options.max_tokens,
                max_steps: options.max_steps,
                compact_after_messages: options.compact_after_messages,
                keep_recent_messages: options.keep_recent_messages,
                anchor_user_index: &mut anchor_user_index,
                is_follow_up,
                traced_len: &mut traced_len,
            }
        };
    }

    while !completed && step < options.max_steps {
        step += 1;
        ctx.step = step;
        emit(&events, AgentEvent::Step { step });
        debug!(step, "agent step start");

        let mut schemas = tool_schemas(&tools, &ctx);
        workflow.before_step(&mut schemas);
        let tool_names: Vec<&str> = schemas.iter().map(|s| s.name.as_str()).collect();
        let system = build_system_prompt(&tool_names, &workflow.system_instructions());
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
        let request_messages = messages.clone();
        let request_payload =
            backend.request_payload(&system, &request_messages, &schemas, options.max_tokens)?;
        run_recorder.record_llm_request(step, &request_payload)?;

        let llm_started = Instant::now();
        let response: LLMResponse = match send_with_retry(
            &backend,
            &system,
            &request_messages,
            &schemas,
            options.max_tokens,
            step,
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
                if workflow.is_active() {
                    terminal_finalize_reason = Some(msg);
                } else {
                    emit(
                        &events,
                        AgentEvent::ApiError {
                            step,
                            message: msg.clone(),
                        },
                    );
                    final_text = format!("API error: {msg}");
                    terminal_error = Some(msg);
                }
                break;
            }
        };

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
                if workflow.is_active() {
                    terminal_finalize_reason = Some(msg);
                } else {
                    emit(
                        &events,
                        AgentEvent::ApiError {
                            step,
                            message: msg.clone(),
                        },
                    );
                    final_text = format!("Error: {msg}");
                    terminal_error = Some(msg);
                }
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

        let tool_call_summary: Vec<Value> = response
            .tool_calls
            .iter()
            .map(|tc| json!({"name": tc.name, "input": tc.input}))
            .collect();
        run_state.note_assistant_step(step, &visible_texts.join("\n"), &tool_call_summary);
        let directive = {
            let mut io = workflow_io!();
            workflow
                .inspect_response(&response, &visible_texts, step, &mut io)
                .await?
        };
        match directive {
            ResponseDirective::UseDefaultLoop => {}
            ResponseDirective::Continue => continue,
            ResponseDirective::Complete(text) => {
                final_text = text;
                completed = true;
                break;
            }
            ResponseDirective::Fail {
                final_text: text,
                error,
            } => {
                final_text = text;
                terminal_error = Some(error);
                break;
            }
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
            let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
            let tool_ctx = ctx
                .clone()
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
            let (result, error) = if let Some(reason) = degraded_reason.clone() {
                let error = format!("tool skipped because recovery already failed: {reason}");
                (ToolResult::text(format!("Error: {error}")), Some(error))
            } else {
                let mut retry_number = 0u8;
                loop {
                    let dispatch = dispatch_tool(&tools, name, &effective_input, &tool_ctx);
                    tokio::pin!(dispatch);
                    let mut progress_open = true;
                    let outcome = loop {
                        tokio::select! {
                            outcome = &mut dispatch => break outcome,
                            progress = progress_rx.recv(), if progress_open => {
                                match progress {
                                    Some(progress) => emit(
                                        &events,
                                        AgentEvent::ToolProgress {
                                            id: id.clone(),
                                            step,
                                            sequence,
                                            name: name.clone(),
                                            progress,
                                        },
                                    ),
                                    None => progress_open = false,
                                }
                            }
                        }
                    };
                    let recovery = match &options.tool_failure_recovery {
                        Some(recovery) => recovery.recover_after_tool(name, retry_number).await,
                        None => ToolRecoveryOutcome::NotNeeded,
                    };
                    match recovery {
                        ToolRecoveryOutcome::NotNeeded => break outcome,
                        ToolRecoveryOutcome::Recovered if retry_number == 0 => {
                            retry_number = 1;
                            info!(
                                step,
                                sequence,
                                tool = name,
                                "tool dependency recovered; retrying"
                            );
                        }
                        ToolRecoveryOutcome::Recovered => {
                            let reason =
                                format!("{name} still requires recovery after its single retry");
                            degraded_reason = Some(reason.clone());
                            let (mut result, _) = outcome;
                            result.blocks.insert(
                                0,
                                ToolResultBlock::text(format!(
                                    "[Tool execution stopped: {reason}. Use prior results for the final answer.]"
                                )),
                            );
                            break (result, Some(reason));
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
            workflow.decorate_tool_result(
                step,
                sequence,
                name,
                error.as_deref(),
                &mut history_content,
            );
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
        messages.push(Message::user_blocks(tool_result_blocks));
        workflow.finish_tool_step();
        if let Err(error) =
            workflow.persist_workspace(&run_dir, &run_state, ResearchWorkspaceStatus::Running)
        {
            warn!(%error, "failed to persist research workspace checkpoint");
        }
        if degraded_reason.is_some() {
            break;
        }
    }

    if !completed {
        if let Some(reason) = terminal_finalize_reason.take() {
            let outcome = {
                let mut io = workflow_io!();
                workflow
                    .on_terminal_error(&reason, &final_text, step, &mut io)
                    .await?
            };
            if let Some(outcome) = outcome {
                outcome.apply(&mut final_text, &mut completed, &mut terminal_error);
            } else {
                if final_text.trim().is_empty() {
                    final_text = format!("Error: {reason}");
                }
                terminal_error = Some(reason.clone());
                emit(
                    &events,
                    AgentEvent::ApiError {
                        step,
                        message: reason,
                    },
                );
            }
        }
    }

    if !completed && terminal_error.is_none() && step >= options.max_steps {
        let outcome = {
            let mut io = workflow_io!();
            workflow.on_max_steps(step, &final_text, &mut io).await?
        };
        if let Some(outcome) = outcome {
            outcome.apply(&mut final_text, &mut completed, &mut terminal_error);
        }
    }

    let forced_summary_prompt = degraded_reason
        .as_ref()
        .map(|reason| {
            info!(
                step,
                reason, "tool recovery failed; forcing partial summary"
            );
            format!(
                "The browser connection was lost and automatic recovery did not succeed: {reason}. \
                 Do not call any more tools. Produce the best possible final answer now in the same \
                 language as the original task, using only evidence already present in the tool \
                 results and conversation. Clearly label the answer as partial, distinguish verified \
                 facts from missing information, and never guess or invent values."
            )
        })
        .or_else(|| {
            (step >= options.max_steps).then(|| {
                info!(step, "reached max_steps, forcing final summary");
                format!(
                    "You have reached the maximum of {} tool-using steps. Do not call any \
                     more tools. Based on the evidence already gathered, produce the best \
                     possible final answer for the user now in the same language as the \
                     original task. If information is incomplete, state what is known, \
                     what is missing, and give your best-effort conclusion.",
                    options.max_steps
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
            let request_messages = messages.clone();
            let request_payload = backend.request_payload(
                &last_system,
                &request_messages,
                &[],
                options.max_tokens,
            )?;
            run_recorder.record_llm_request(summary_step, &request_payload)?;
            let llm_started = Instant::now();
            match send_with_retry(
                &backend,
                &last_system,
                &request_messages,
                &[],
                options.max_tokens,
                summary_step,
            )
            .await
            {
                Ok(response) => {
                    let duration_ms = llm_started.elapsed().as_millis() as u64;
                    run_recorder.record_llm_response(summary_step, &response, duration_ms)?;
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
                                "Your previous tool-free summary was incomplete or truncated. Do not call tools or include reasoning-only output. Respond once with a concise, complete user-visible answer based only on the gathered evidence, clearly marking any missing information."
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
                    if let Some(message) = forced_summary_failure(
                        task,
                        &response,
                        &visible_text,
                        run_state.as_ref(),
                        tools.iter().any(|tool| tool.name() == "publish_artifact"),
                        if degraded_reason.is_some() {
                            ForcedSummaryKind::RecoveryPartial
                        } else {
                            ForcedSummaryKind::ExecutionLimit
                        },
                    ) {
                        warn!(
                            step = summary_step,
                            "forced summary did not finish the task"
                        );
                        final_text = message.clone();
                        emit(
                            &events,
                            AgentEvent::ApiError {
                                step: summary_step,
                                message: message.clone(),
                            },
                        );
                        terminal_error = Some(message);
                        break;
                    }
                    for text in &visible_texts {
                        emit(
                            &events,
                            AgentEvent::AssistantText {
                                step: summary_step,
                                text: text.clone(),
                            },
                        );
                        final_text = text.clone();
                    }
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

    let enriched_report = report_with_artifacts(&final_text, Some(&run_state));
    if let Err(error) = std::fs::write(run_dir.join("report.md"), &enriched_report) {
        let message = format!("failed to persist final report: {error}");
        warn!(%error, "failed to persist final report");
        if terminal_error.is_none() {
            emit(
                &events,
                AgentEvent::ApiError {
                    step,
                    message: message.clone(),
                },
            );
            terminal_error = Some(message);
        }
    }

    // Stage the complete indexes while retaining a non-terminal status. The
    // final workspace status and Done event are committed only after the
    // canonical run manifest has finalized successfully.
    if let Err(error) =
        workflow.persist_workspace(&run_dir, &run_state, ResearchWorkspaceStatus::Running)
    {
        let message = format!("failed to persist final research workspace: {error}");
        warn!(%error, "failed to persist final research workspace");
        if terminal_error.is_none() {
            emit(
                &events,
                AgentEvent::ApiError {
                    step,
                    message: message.clone(),
                },
            );
            terminal_error = Some(message);
        }
    }

    // A degraded reason represents a successfully delivered partial answer,
    // not merely the browser failure that caused us to attempt one. If the
    // summary or required final persistence failed, persist and return only
    // the terminal error.
    let mut completed_degraded_reason = terminal_error
        .is_none()
        .then(|| degraded_reason.clone())
        .flatten();
    let mut status = if terminal_error.is_some() {
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

    let workspace_status = if terminal_error.is_some() {
        ResearchWorkspaceStatus::Failed
    } else if completed_degraded_reason.is_some() {
        ResearchWorkspaceStatus::Partial
    } else {
        ResearchWorkspaceStatus::Completed
    };
    match workflow.persist_workspace(&run_dir, &run_state, workspace_status) {
        Ok(()) => workspace_status_guard.disarm(),
        Err(error) => {
            let message = format!("failed to commit final research workspace: {error}");
            warn!(%error, "failed to commit final research workspace");
            emit(
                &events,
                AgentEvent::ApiError {
                    step,
                    message: message.clone(),
                },
            );
            if terminal_error.is_none() {
                terminal_error = Some(message);
            }
            completed_degraded_reason = None;
            status = "failed";
            run_recorder.finish(
                status,
                step,
                &usage,
                terminal_error.as_deref(),
                completed_degraded_reason.as_deref(),
            )?;
        }
    }

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

/// Backoff schedule for transient chat failures. Two retries keeps the worst
/// case bounded: a fully dead network adds at most two extra request
/// timeouts before the run fails.
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
    let mut attempt = 0usize;
    loop {
        match backend.send(system, messages, schemas, max_tokens).await {
            Ok(response) => return Ok(response),
            Err(error) => {
                let Some(delay) = CHAT_RETRY_DELAYS.get(attempt).copied() else {
                    return Err(error);
                };
                if !is_transient_api_error(&error) {
                    return Err(error);
                }
                attempt += 1;
                warn!(
                    step,
                    attempt,
                    delay_secs = delay.as_secs(),
                    error = %format!("{error:#}"),
                    "transient LLM error; retrying"
                );
                tokio::time::sleep(delay).await;
            }
        }
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

pub(crate) fn emit(events: &broadcast::Sender<AgentEvent>, event: AgentEvent) {
    let _ = events.send(event);
}

async fn dispatch_tool(
    tools: &[SharedTool],
    name: &str,
    input: &Value,
    ctx: &ToolContext,
) -> (ToolResult, Option<String>) {
    let outcome = match find_tool(tools, name) {
        Some(tool) if tool.is_available(ctx) => match tool.call(input.clone(), ctx).await {
            Ok(r) => (r, None),
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

/// Rebuild the bounded model-facing form of a persisted tool result. This is
/// used when a later run resumes an interrupted turn: raw `output.json` stays
/// the source of truth, while the replayed context matches the same bounds as
/// the live agent loop.
pub(crate) fn tool_result_blocks_for_history(blocks: &[ToolResultBlock]) -> Vec<ToolResultContent> {
    bound_content_for_history(&tool_result_blocks_to_content(blocks))
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
pub(crate) fn split_thinking(text_blocks: &[String]) -> (Vec<String>, Vec<String>) {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForcedSummaryKind {
    ExecutionLimit,
    RecoveryPartial,
}

fn forced_summary_failure(
    task: &str,
    response: &LLMResponse,
    visible_text: &str,
    run_state: &RunState,
    publish_required: bool,
    summary_kind: ForcedSummaryKind,
) -> Option<String> {
    let tried_unavailable_tool =
        !response.tool_calls.is_empty() || contains_pseudo_tool_call(visible_text);
    let published = run_state
        .artifact_records()
        .into_iter()
        .filter(|artifact| {
            artifact.source_tool == "publish_artifact"
                && artifact.metadata.get("category").and_then(Value::as_str) == Some("deliverable")
        })
        .collect::<Vec<_>>();
    // A max-step summary must still honour the original deliverable request.
    // A recovery summary is explicitly allowed to report that the requested
    // file is missing; only deliverables it claims were created must exist.
    let mut required_kinds = if summary_kind == ForcedSummaryKind::ExecutionLimit {
        requested_artifact_kinds(task)
    } else {
        Vec::new()
    };
    for kind in claimed_artifact_kinds(visible_text) {
        if !required_kinds.contains(&kind) {
            required_kinds.push(kind);
        }
    }
    required_kinds.sort_by_key(|kind| *kind == "generic");
    let mut matched_artifacts = vec![false; published.len()];
    let missing_required_deliverable = publish_required
        && required_kinds.iter().any(|kind| {
            if *kind == "generic" {
                return published.is_empty();
            }
            let matching_index = published.iter().enumerate().position(|(index, artifact)| {
                !matched_artifacts[index] && artifact_matches_kind(&artifact.path, kind)
            });
            if let Some(index) = matching_index {
                matched_artifacts[index] = true;
                false
            } else {
                true
            }
        });
    if !tried_unavailable_tool && !missing_required_deliverable {
        return None;
    }

    let chinese = task
        .chars()
        .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch));
    Some(if chinese {
        if missing_required_deliverable {
            if summary_kind == ForcedSummaryKind::RecoveryPartial {
                "浏览器连接中断后的部分总结声称已生成交付文件，但没有找到经过验证并成功发布的对应文件。已有过程记录已保留，请重试。".to_string()
            } else {
                "任务在达到最大执行步数时仍未完成，且没有经过验证并成功发布的交付文件。已有过程记录已保留，请重试或缩小任务范围。".to_string()
            }
        } else {
            "任务在达到最大执行步数后仍试图调用工具，但该操作没有实际执行。已有过程记录已保留，请重试。".to_string()
        }
    } else if missing_required_deliverable {
        if summary_kind == ForcedSummaryKind::RecoveryPartial {
            "The partial recovery summary claimed a deliverable that was not verified and published. Progress was preserved; please retry.".to_string()
        } else {
            "The task reached its execution limit without a verified, published deliverable. Progress was preserved; retry or narrow the task scope.".to_string()
        }
    } else {
        "The task reached its execution limit while still attempting a tool call, so that operation was not executed. Progress was preserved; please retry.".to_string()
    })
}

fn contains_pseudo_tool_call(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
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

fn requested_artifact_kinds(task: &str) -> Vec<&'static str> {
    let lower = task.to_ascii_lowercase();
    let creation_markers = [
        "create",
        "generate",
        "export",
        "produce",
        "write",
        "save",
        "build",
        "make",
        "downloadable",
        "生成",
        "创建",
        "制作",
        "导出",
        "保存",
        "写入",
        "做成",
        "整理成",
        "放到",
    ];
    let source_markers = [
        " from ",
        " using ",
        " based on ",
        " source ",
        " input ",
        " after ",
        " for ",
        " of ",
        " about ",
        "根据",
        "基于",
        "关于",
        "输入",
        "从",
    ];
    let mut kinds = Vec::new();
    for creation_marker in &creation_markers {
        for (offset, _) in lower.match_indices(*creation_marker) {
            let target_start = offset + creation_marker.len();
            let remaining = &lower[target_start..];
            let target_end = source_markers
                .iter()
                .chain(creation_markers.iter())
                .filter_map(|marker| remaining.find(marker))
                .min()
                .unwrap_or(remaining.len());
            let target = &remaining[..target_end];
            let target_kinds = artifact_kinds(target);
            let has_typed_artifact = !target_kinds.is_empty();
            for kind in target_kinds {
                kinds.push(kind);
            }
            if !has_typed_artifact
                && ["file", "document", "文件", "文档"]
                    .iter()
                    .any(|marker| target.contains(marker))
                && !kinds.contains(&"generic")
            {
                kinds.push("generic");
            }
        }
    }
    kinds
}

fn claimed_artifact_kinds(text: &str) -> Vec<&'static str> {
    let lower = text.to_ascii_lowercase();
    let completion_markers = [
        "generated",
        "created",
        "exported",
        "saved",
        "published",
        "ready for download",
        "已生成",
        "已创建",
        "已导出",
        "已保存",
        "已发布",
        "可下载",
    ];
    let claim_boundaries = [
        " source ",
        " input ",
        " based on ",
        " from ",
        ". ",
        "; ",
        "\n",
        "来源",
        "输入",
        "基于",
        "根据",
        "。",
        "；",
    ];
    let mut kinds = Vec::new();
    for completion_marker in &completion_markers {
        for (offset, _) in lower.match_indices(*completion_marker) {
            let target_start = offset + completion_marker.len();
            let remaining = &lower[target_start..];
            let target_end = claim_boundaries
                .iter()
                .chain(completion_markers.iter())
                .filter_map(|marker| remaining.find(marker))
                .min()
                .unwrap_or(remaining.len());
            let target = &remaining[..target_end];
            for kind in artifact_kinds(target) {
                if !kinds.contains(&kind) {
                    kinds.push(kind);
                }
            }
            if kinds.is_empty()
                && ["file", "document", "文件", "文档", "下载"]
                    .iter()
                    .any(|marker| target.contains(marker))
            {
                kinds.push("generic");
            }
        }
    }
    kinds
}

fn artifact_kinds(text: &str) -> Vec<&'static str> {
    let groups: [(&str, &[&str]); 8] = [
        (
            "excel",
            &[
                ".xlsx",
                ".xlsm",
                "excel",
                "excel file",
                "excel report",
                "excel workbook",
                "excel spreadsheet",
                "excel 文件",
                "excel 报告",
                "excel 表格",
                "电子表格",
            ],
        ),
        ("csv", &[".csv", "csv", "csv 文件", "csv file", "csv 格式"]),
        (
            "spreadsheet",
            &[
                "spreadsheet",
                "spreadsheet file",
                "spreadsheet document",
                "表格文件",
            ],
        ),
        (
            "word",
            &[
                ".docx",
                ".docm",
                "word document",
                "word file",
                "word 文档",
                "word 文件",
            ],
        ),
        (
            "powerpoint",
            &[
                ".pptx",
                ".pptm",
                "powerpoint",
                "ppt",
                "powerpoint presentation",
                "powerpoint file",
                "ppt 文件",
                "ppt 文档",
                "幻灯片文件",
            ],
        ),
        (
            "pdf",
            &[
                ".pdf",
                "pdf",
                "pdf 文件",
                "pdf file",
                "pdf 文档",
                "pdf document",
                "pdf 报告",
                "pdf report",
            ],
        ),
        ("archive", &[".zip", "zip", "zip 文件", "zip file"]),
        (
            "subtitle",
            &[".srt", ".vtt", "srt", "vtt", "字幕文件", "subtitle file"],
        ),
    ];
    let mut kinds = Vec::new();
    for (kind, markers) in groups {
        let occurrences = markers
            .iter()
            .map(|marker| text.match_indices(marker).count())
            .max()
            .unwrap_or_default();
        kinds.extend(std::iter::repeat_n(kind, occurrences));
    }
    if kinds.contains(&"excel") {
        kinds.retain(|kind| *kind != "spreadsheet");
    }
    kinds
}

fn artifact_matches_kind(path: &str, kind: &str) -> bool {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match kind {
        "excel" => matches!(extension.as_str(), "xlsx" | "xlsm"),
        "csv" => extension == "csv",
        "spreadsheet" => matches!(extension.as_str(), "xlsx" | "xlsm" | "csv"),
        "word" => matches!(extension.as_str(), "docx" | "docm"),
        "powerpoint" => matches!(extension.as_str(), "pptx" | "pptm"),
        "pdf" => extension == "pdf",
        "archive" => extension == "zip",
        "subtitle" => matches!(extension.as_str(), "srt" | "vtt"),
        "generic" => true,
        _ => false,
    }
}

/// Build the assistant message blocks for history. Truncates visible text to
/// `ASSISTANT_TEXT_MAX_CHARS` to keep history bounded over many steps. Drops
/// `[Thinking]`-prefixed text since that's already surfaced as a Reasoning
/// event.
pub(crate) fn build_assistant_blocks(
    response: &LLMResponse,
    visible_texts: &[String],
) -> Vec<Block> {
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

    fn summary_response(text: &str) -> LLMResponse {
        LLMResponse {
            text_blocks: vec![text.to_string()],
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: TokenUsage::default(),
            provider_usage: None,
            reasoning_content: String::new(),
            thinking_blocks: Vec::new(),
            reasoning_items: Vec::new(),
        }
    }

    #[test]
    fn recovery_partial_can_report_a_requested_deliverable_as_missing() {
        let response = summary_response("浏览器连接中断；已整理数据如下，PDF 尚未生成。");
        let state = RunState::new("请根据搜索结果生成 PDF 报告");

        assert!(forced_summary_failure(
            "请根据搜索结果生成 PDF 报告",
            &response,
            &response.text_blocks.join("\n\n"),
            &state,
            true,
            ForcedSummaryKind::RecoveryPartial,
        )
        .is_none());
    }

    #[test]
    fn recovery_partial_rejects_an_unpublished_deliverable_claim() {
        let response = summary_response("已生成 PDF 报告，可供下载。");
        let state = RunState::new("请根据搜索结果生成 PDF 报告");

        assert!(forced_summary_failure(
            "请根据搜索结果生成 PDF 报告",
            &response,
            &response.text_blocks.join("\n\n"),
            &state,
            true,
            ForcedSummaryKind::RecoveryPartial,
        )
        .is_some());
    }

    #[test]
    fn execution_limit_still_requires_the_requested_deliverable() {
        let response = summary_response("目前只完成了数据整理。");
        let state = RunState::new("请根据搜索结果生成 PDF 报告");

        assert!(forced_summary_failure(
            "请根据搜索结果生成 PDF 报告",
            &response,
            &response.text_blocks.join("\n\n"),
            &state,
            true,
            ForcedSummaryKind::ExecutionLimit,
        )
        .is_some());
    }

    #[test]
    fn pseudo_tool_markup_is_not_a_valid_summary() {
        assert!(contains_pseudo_tool_call(
            "<function_calls><invoke name=\"publish_artifact\">"
        ));
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
        assert!(error.is_none());
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
    }
}
