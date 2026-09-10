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
use crate::agent::research::{
    planner_correction_prompt, planner_system_prompt, research_brief_tool_schema, ResearchBrief,
    ResearchBriefEnvelope, DEFAULT_RESEARCH_PLAN_MAX_TOKENS, SUBMIT_RESEARCH_BRIEF_TOOL,
};
use crate::agent::research_coverage::{
    answer_with_missing_limitations, budget_exhausted_completion_prompt,
    completion_protocol_correction_prompt, completion_tool_result_content, evaluate_completion,
    forced_final_writer_prompt, forced_final_writer_system_prompt, initial_coverage_state,
    research_completion_tool_schema, salvage_completion_text, CompletionGateDecision,
    EvidenceLocatorCatalog, FinalAnswerSource, ResearchCompletionSubmission,
    DEFAULT_MAX_COMPLETION_ATTEMPTS, DEFAULT_MAX_COVERAGE_PROTOCOL_RETRIES,
    DEFAULT_MAX_FORCED_WRITER_ATTEMPTS, DEFAULT_MAX_RESEARCH_RECOVERY_ROUNDS,
    SUBMIT_RESEARCH_COMPLETION_TOOL,
};
use crate::agent::run_logging::{make_run_dir, AgentRunRecorder};
use crate::agent::run_state::RunState;
use crate::agent::signature::tool_call_signature;
use crate::agent::system_prompt::build_system_prompt;
use crate::agent::tool::{SharedTool, ToolContext, ToolProgressEvent, ToolResult, ToolResultBlock};
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
    },
}

#[derive(Debug, Clone)]
pub struct AgentOptions {
    pub max_steps: u32,
    pub max_tokens: u32,
    pub research_plan_max_tokens: u32,
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
}

impl Default for AgentOptions {
    fn default() -> Self {
        Self {
            max_steps: 30,
            max_tokens: 16000,
            research_plan_max_tokens: DEFAULT_RESEARCH_PLAN_MAX_TOKENS,
            extra_instructions: String::new(),
            run_dir: None,
            enabled_sites: Vec::new(),
            compact_after_messages: DEFAULT_COMPACT_AFTER_MESSAGES,
            keep_recent_messages: DEFAULT_KEEP_RECENT_MESSAGES,
            seed_messages: Vec::new(),
            session_id: None,
            background_media_generation: None,
            billing_task_id: None,
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
    let run_dir = options
        .run_dir
        .clone()
        .unwrap_or_else(|| make_run_dir(task));
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
        .with_billing_task_id(options.billing_task_id.clone());
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

    let mut usage = TokenUsage::default();
    let mut effective_extra_instructions = options.extra_instructions.clone();
    let mut validated_research_brief: Option<ResearchBrief> = None;

    let planning = prepare_research_brief(task, &backend, &options).await?;
    usage += &planning.usage;

    let mut status = "failed_fallback";
    let mut planning_error = planning.error.clone();
    if let Some(envelope) = planning.envelope {
        match envelope.persist(&run_dir) {
            Ok(()) => {
                if let Some(question) = envelope.clarification() {
                    let final_text = question.to_string();
                    emit(
                        &events,
                        AgentEvent::Done {
                            run_id: run_id.clone(),
                            steps: 0,
                            final_text: final_text.clone(),
                        },
                    );
                    let report = report_with_artifacts(&final_text, Some(&run_state));
                    let _ = std::fs::write(run_dir.join("report.md"), report);
                    run_recorder.finish("completed", 0, &usage, None)?;
                    run_trace.finish("completed", 0, &usage, None);
                    return Ok(AgentOutcome {
                        run_id,
                        run_dir,
                        steps: 0,
                        final_text,
                        usage,
                        error: None,
                    });
                }
                if let Some(brief) = envelope.brief() {
                    let rendered = brief.execution_prompt();
                    match rendered {
                        Ok(prompt) => {
                            if !effective_extra_instructions.trim().is_empty() {
                                effective_extra_instructions.push_str("\n\n");
                            }
                            effective_extra_instructions.push_str(&prompt);
                            validated_research_brief = Some(brief.clone());
                            status = "ready";
                            planning_error = None;
                        }
                        Err(error) => {
                            planning_error = Some(format!(
                                "could not render validated research brief: {error:#}"
                            ));
                        }
                    }
                }
            }
            Err(error) => {
                planning_error = Some(format!("could not persist research brief: {error}"));
            }
        }
    }
    if status == "failed_fallback" {
        warn!(
            error = planning_error
                .as_deref()
                .unwrap_or("unknown planning failure"),
            "research brief planning failed; falling back to reactive execution"
        );
    }

    let mut coverage_runtime = validated_research_brief.map(|brief| {
        let state = initial_coverage_state(&brief);
        CoverageRuntime::new(brief, state)
    });

    let mut step = 0u32;
    let mut final_text = String::new();
    let mut tool_call_history: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut completed = false;
    let mut terminal_error: Option<String> = None;
    let mut terminal_finalize_reason: Option<String> = None;
    let mut truncation_retries = 0u32;
    let mut last_system: String = build_system_prompt(&[], &effective_extra_instructions);

    while step < options.max_steps {
        step += 1;
        ctx.step = step;
        emit(&events, AgentEvent::Step { step });
        debug!(step, "agent step start");

        let mut schemas = if coverage_runtime
            .as_ref()
            .is_some_and(|runtime| runtime.revise_only)
        {
            Vec::new()
        } else {
            tool_schemas(&tools, &ctx)
        };
        if coverage_runtime.is_some() {
            schemas.push(research_completion_tool_schema());
        }
        let tool_names: Vec<&str> = schemas.iter().map(|s| s.name.as_str()).collect();
        let step_instructions = coverage_runtime
            .as_ref()
            .map(|runtime| runtime.system_instructions(&effective_extra_instructions))
            .unwrap_or_else(|| effective_extra_instructions.clone());
        let system = build_system_prompt(&tool_names, &step_instructions);
        last_system = system.clone();
        if compact_messages_for_context(
            &mut messages,
            options.compact_after_messages,
            options.keep_recent_messages,
            &mut anchor_user_index,
            is_follow_up,
        ) {
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
                if coverage_runtime.is_some() {
                    terminal_finalize_reason = Some(msg.clone());
                } else {
                    final_text = format!("API error: {msg}");
                    emit(
                        &events,
                        AgentEvent::ApiError {
                            step,
                            message: msg.clone(),
                        },
                    );
                }
                terminal_error = Some(msg);
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
                if coverage_runtime.is_some() {
                    terminal_finalize_reason = Some(msg.clone());
                } else {
                    emit(
                        &events,
                        AgentEvent::ApiError {
                            step,
                            message: msg.clone(),
                        },
                    );
                    final_text = format!("Error: {msg}");
                }
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

        let tool_call_summary: Vec<Value> = response
            .tool_calls
            .iter()
            .map(|tc| json!({"name": tc.name, "input": tc.input}))
            .collect();
        run_state.note_assistant_step(step, &visible_texts.join("\n"), &tool_call_summary);

        if let Some(runtime) = coverage_runtime.as_mut() {
            let completion_calls: Vec<&ToolCall> = response
                .tool_calls
                .iter()
                .filter(|call| call.name == SUBMIT_RESEARCH_COMPLETION_TOOL)
                .collect();

            if !completion_calls.is_empty() {
                let assistant_blocks = build_assistant_blocks(&response, &visible_texts);
                messages.push(Message::assistant_blocks(assistant_blocks));
                traced_len = messages.len();
                runtime.attempts = runtime.attempts.saturating_add(1);
                let attempt = runtime.attempts;

                if response.tool_calls.len() != 1 || completion_calls.len() != 1 {
                    let reason = format!(
                        "{SUBMIT_RESEARCH_COMPLETION_TOOL} must be the only tool call in its response"
                    );
                    let payload = json!({
                        "tool_calls": response.tool_calls.iter().map(|call| json!({
                            "name": call.name,
                            "input": call.input,
                        })).collect::<Vec<_>>(),
                    });
                    runtime.protocol_retries = runtime.protocol_retries.saturating_add(1);
                    runtime.state = payload;
                    if coverage_protocol_can_retry(runtime, step, options.max_steps) {
                        messages.push(Message::user_blocks(protocol_error_results(
                            &response.tool_calls,
                            &reason,
                        )));
                        continue;
                    }
                    messages.push(Message::user_blocks(protocol_error_results(
                        &response.tool_calls,
                        &reason,
                    )));
                    if let Some((answer, _)) = salvage_completion_text(
                        completion_calls.first().map(|call| &call.input),
                        &visible_texts,
                    ) {
                        final_text = answer;
                        emit_final_text(&events, step, &final_text);
                        completed = true;
                    } else {
                        let outcome = force_finalize_coverage_with_writer(
                            &backend,
                            &messages,
                            runtime,
                            vec![reason],
                            &options.extra_instructions,
                            options.max_tokens,
                            step + 1,
                            &run_recorder,
                            &mut run_trace,
                            &mut usage,
                            &events,
                        )
                        .await?;
                        finish_forced_writer_outcome(
                            outcome,
                            &events,
                            step + 1,
                            &mut final_text,
                            &mut completed,
                            &mut terminal_error,
                        );
                    }
                    break;
                }

                let call = completion_calls[0];
                let evidence =
                    EvidenceLocatorCatalog::from_run(&run_dir, &run_state, &runtime.evidence_ids);
                let submission = ResearchCompletionSubmission::from_tool_input(
                    call.input.clone(),
                    &runtime.brief,
                    &evidence,
                );
                let submission = match submission {
                    Ok(submission) => submission,
                    Err(error) => {
                        let reason = format!("{error:#}");
                        runtime.protocol_retries = runtime.protocol_retries.saturating_add(1);
                        runtime.state = call.input.clone();
                        if coverage_protocol_can_retry(runtime, step, options.max_steps) {
                            messages.push(Message::user_blocks(vec![Block::ToolResult {
                                tool_use_id: call.id.clone(),
                                content: completion_tool_result_content(&json!({
                                    "accepted": false,
                                    "action": "protocol_error",
                                    "error": reason,
                                    "instruction": completion_protocol_correction_prompt(),
                                })),
                            }]));
                            continue;
                        }
                        messages.push(Message::user_blocks(vec![Block::ToolResult {
                            tool_use_id: call.id.clone(),
                            content: completion_tool_result_content(&json!({
                                "accepted": false,
                                "action": "protocol_error",
                                "error": reason,
                                "instruction": "Research has ended. A final-answer writer will produce the user-facing answer without tools.",
                            })),
                        }]));
                        if let Some((answer, _)) =
                            salvage_completion_text(Some(&call.input), &visible_texts)
                        {
                            final_text = answer;
                            emit_final_text(&events, step, &final_text);
                            completed = true;
                        } else {
                            let outcome = force_finalize_coverage_with_writer(
                                &backend,
                                &messages,
                                runtime,
                                vec![reason],
                                &options.extra_instructions,
                                options.max_tokens,
                                step + 1,
                                &run_recorder,
                                &mut run_trace,
                                &mut usage,
                                &events,
                            )
                            .await?;
                            finish_forced_writer_outcome(
                                outcome,
                                &events,
                                step + 1,
                                &mut final_text,
                                &mut completed,
                                &mut terminal_error,
                            );
                        }
                        break;
                    }
                };

                let force_finish = attempt >= DEFAULT_MAX_COMPLETION_ATTEMPTS
                    || runtime.recovery_rounds >= DEFAULT_MAX_RESEARCH_RECOVERY_ROUNDS
                    || runtime.last_mile_used;
                let gate = evaluate_completion(
                    &runtime.brief,
                    &submission,
                    force_finish,
                    !runtime.last_mile_used,
                );
                let mut decision = gate.decision;
                if force_finish
                    && matches!(
                        decision,
                        CompletionGateDecision::ResearchMore
                            | CompletionGateDecision::LastMileResearch
                            | CompletionGateDecision::ReviseOnly
                    )
                {
                    decision = CompletionGateDecision::FinishWithLimitations;
                }
                match decision {
                    CompletionGateDecision::ResearchMore => {
                        runtime.recovery_rounds = runtime.recovery_rounds.saturating_add(1);
                        runtime.revise_only = false;
                    }
                    CompletionGateDecision::LastMileResearch => {
                        runtime.last_mile_used = true;
                        runtime.last_mile_active = true;
                        runtime.revise_only = false;
                    }
                    CompletionGateDecision::ReviseOnly => runtime.revise_only = true,
                    CompletionGateDecision::Accept
                    | CompletionGateDecision::FinishWithLimitations => {}
                }
                let state = serde_json::to_value(&submission).map_err(anyhow::Error::from)?;
                runtime.state = state.clone();

                match decision {
                    CompletionGateDecision::Accept
                    | CompletionGateDecision::FinishWithLimitations => {
                        let limitations = completion_limitations(&submission, decision);
                        final_text =
                            answer_with_missing_limitations(&submission.final_answer, &limitations);
                        emit_final_text(&events, step, &final_text);
                        completed = true;
                        break;
                    }
                    CompletionGateDecision::ResearchMore
                    | CompletionGateDecision::LastMileResearch => {
                        let value = gate.tool_result_value(&runtime.brief, &submission);
                        messages.push(Message::user_blocks(vec![Block::ToolResult {
                            tool_use_id: call.id.clone(),
                            content: completion_tool_result_content(&value),
                        }]));
                        continue;
                    }
                    CompletionGateDecision::ReviseOnly => {
                        let value = gate.tool_result_value(&runtime.brief, &submission);
                        messages.push(Message::user_blocks(vec![Block::ToolResult {
                            tool_use_id: call.id.clone(),
                            content: completion_tool_result_content(&value),
                        }]));
                        continue;
                    }
                }
            }

            if response.tool_calls.is_empty() {
                let assistant_blocks = build_assistant_blocks(&response, &visible_texts);
                messages.push(Message::assistant_blocks(assistant_blocks));
                traced_len = messages.len();
                runtime.protocol_retries = runtime.protocol_retries.saturating_add(1);
                runtime.state = json!({
                    "ordinary_prose_attempt": visible_texts,
                });
                if coverage_protocol_can_retry(runtime, step, options.max_steps) {
                    messages.push(Message::user(completion_protocol_correction_prompt()));
                    continue;
                }
                let reason = "coverage completion protocol retries exhausted after ordinary prose"
                    .to_string();
                if let Some((answer, _)) = salvage_completion_text(None, &visible_texts) {
                    final_text = answer;
                    emit_final_text(&events, step, &final_text);
                    completed = true;
                } else {
                    let outcome = force_finalize_coverage_with_writer(
                        &backend,
                        &messages,
                        runtime,
                        vec![reason],
                        &options.extra_instructions,
                        options.max_tokens,
                        step + 1,
                        &run_recorder,
                        &mut run_trace,
                        &mut usage,
                        &events,
                    )
                    .await?;
                    finish_forced_writer_outcome(
                        outcome,
                        &events,
                        step + 1,
                        &mut final_text,
                        &mut completed,
                        &mut terminal_error,
                    );
                }
                break;
            }
        }

        if coverage_runtime
            .as_ref()
            .is_some_and(|runtime| runtime.last_mile_active)
            && response.tool_calls.len() > 2
        {
            let reason = format!(
                "last-mile recovery permits at most two tool calls (got {})",
                response.tool_calls.len()
            );
            let assistant_blocks = build_assistant_blocks(&response, &visible_texts);
            messages.push(Message::assistant_blocks(assistant_blocks));
            traced_len = messages.len();
            messages.push(Message::user_blocks(protocol_error_results(
                &response.tool_calls,
                &reason,
            )));
            if let Some(runtime) = coverage_runtime.as_mut() {
                runtime.last_mile_active = false;
                runtime.revise_only = true;
                runtime.state = json!({
                    "last_mile_protocol_error": reason,
                    "requested_tool_calls": response.tool_calls.len(),
                });
            }
            continue;
        }

        // Build the assistant block list manually instead of using
        // LLMResponse::to_assistant_blocks() so we can drop synthetic thinking
        // text and keep long-running histories bounded.
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
            let dispatch = dispatch_tool(&tools, name, &effective_input, &tool_ctx);
            tokio::pin!(dispatch);
            let mut progress_open = true;
            let (result, error) = loop {
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
            let evidence_reference = if error.is_none() {
                coverage_runtime
                    .as_mut()
                    .map(|runtime| runtime.register_tool_evidence(step, sequence, name))
            } else {
                None
            };
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
            if let Some((evidence_id, canonical_locator)) = evidence_reference {
                history_content.insert(
                    0,
                    ToolResultContent::Text {
                        text: json!({
                            "evidence_id": evidence_id,
                            "canonical_locator": canonical_locator,
                            "instruction": "Cite the evidence_id in submit_research_completion.evidence_refs."
                        })
                        .to_string(),
                    },
                );
            }
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
        if let Some(runtime) = coverage_runtime.as_mut() {
            if runtime.last_mile_active {
                runtime.last_mile_active = false;
                runtime.revise_only = true;
            }
        }
    }

    if !completed {
        if let (Some(reason), Some(runtime)) =
            (terminal_finalize_reason.take(), coverage_runtime.as_mut())
        {
            if !runtime.evidence_ids.is_empty()
                || run_state.has_structured_state()
                || crate::agent::research_coverage::is_usable_final_answer(&final_text)
            {
                info!(step, error = %reason, "terminal research error; invoking final writer");
                terminal_error = None;
                let outcome = force_finalize_coverage_with_writer(
                    &backend,
                    &messages,
                    runtime,
                    vec![format!(
                        "main research loop ended with API/protocol error: {reason}"
                    )],
                    &options.extra_instructions,
                    options.max_tokens,
                    step + 1,
                    &run_recorder,
                    &mut run_trace,
                    &mut usage,
                    &events,
                )
                .await?;
                finish_forced_writer_outcome(
                    outcome,
                    &events,
                    step + 1,
                    &mut final_text,
                    &mut completed,
                    &mut terminal_error,
                );
            } else {
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
        info!(step, "reached max_steps, forcing final summary");
        let forced_schemas = if coverage_runtime.is_some() {
            messages.push(Message::user(budget_exhausted_completion_prompt(
                options.max_steps,
            )));
            vec![research_completion_tool_schema()]
        } else {
            messages.push(Message::user(format!(
                "You have reached the maximum of {} tool-using steps. Do not call any \
                 more tools. Based on the evidence already gathered, produce the best \
                 possible final answer for the user now in the same language as the \
                 original task. If information is incomplete, state what is known, \
                 what is missing, and give your best-effort conclusion.",
                options.max_steps
            )));
            Vec::new()
        };
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
        let forced_system = if coverage_runtime.is_some() {
            let step_instructions = coverage_runtime
                .as_ref()
                .map(|runtime| runtime.system_instructions(&effective_extra_instructions))
                .unwrap_or_else(|| effective_extra_instructions.clone());
            build_system_prompt(&[SUBMIT_RESEARCH_COMPLETION_TOOL], &step_instructions)
        } else {
            last_system.clone()
        };
        let request_payload = backend.request_payload(
            &forced_system,
            &request_messages,
            &forced_schemas,
            options.max_tokens,
        )?;
        run_recorder.record_llm_request(step + 1, &request_payload)?;
        let llm_started = Instant::now();
        match send_with_retry(
            &backend,
            &forced_system,
            &request_messages,
            &forced_schemas,
            options.max_tokens,
            step + 1,
        )
        .await
        {
            Ok(response) => {
                let duration_ms = llm_started.elapsed().as_millis() as u64;
                run_recorder.record_llm_response(step + 1, &response, duration_ms)?;
                run_trace.record_llm(
                    step + 1,
                    duration_ms,
                    &forced_system,
                    &messages[traced_len..],
                    &response,
                );
                usage += &response.usage;
                let (visible_texts, _) = split_thinking(&response.text_blocks);
                if let Some(runtime) = coverage_runtime.as_mut() {
                    runtime.attempts = runtime.attempts.saturating_add(1);
                    let completion_call = (response.tool_calls.len() == 1
                        && response.tool_calls[0].name == SUBMIT_RESEARCH_COMPLETION_TOOL)
                        .then(|| &response.tool_calls[0]);
                    let mut forced_failure_reasons = Vec::new();
                    let mut salvage: Option<(String, FinalAnswerSource)> = None;

                    if let Some(call) = completion_call {
                        let evidence = EvidenceLocatorCatalog::from_run(
                            &run_dir,
                            &run_state,
                            &runtime.evidence_ids,
                        );
                        match ResearchCompletionSubmission::from_tool_input(
                            call.input.clone(),
                            &runtime.brief,
                            &evidence,
                        ) {
                            Ok(submission) => {
                                let gate =
                                    evaluate_completion(&runtime.brief, &submission, true, false);
                                let mut decision = gate.decision;
                                if matches!(
                                    decision,
                                    CompletionGateDecision::ResearchMore
                                        | CompletionGateDecision::LastMileResearch
                                        | CompletionGateDecision::ReviseOnly
                                ) {
                                    decision = CompletionGateDecision::FinishWithLimitations;
                                }
                                let state = serde_json::to_value(&submission)
                                    .map_err(anyhow::Error::from)?;
                                runtime.state = state.clone();
                                let limitations = completion_limitations(&submission, decision);
                                final_text = answer_with_missing_limitations(
                                    &submission.final_answer,
                                    &limitations,
                                );
                                emit_final_text(&events, step + 1, &final_text);
                                completed = true;
                            }
                            Err(error) => {
                                let reason = format!("{error:#}");
                                runtime.state = call.input.clone();
                                salvage =
                                    salvage_completion_text(Some(&call.input), &visible_texts);
                                forced_failure_reasons.push(reason);
                            }
                        }
                    } else {
                        let reason = format!(
                            "budget finalization requires exactly one {SUBMIT_RESEARCH_COMPLETION_TOOL} call"
                        );
                        let payload = json!({
                            "tool_calls": response.tool_calls.iter().map(|call| json!({
                                "name": call.name,
                                "input": call.input,
                            })).collect::<Vec<_>>(),
                            "visible_text": visible_texts,
                        });
                        runtime.state = payload.clone();
                        salvage = salvage_completion_text(None, &visible_texts);
                        forced_failure_reasons.push(reason);
                    }

                    if !completed {
                        if let Some((answer, _)) = salvage {
                            final_text = answer;
                            emit_final_text(&events, step + 1, &final_text);
                        } else {
                            let outcome = force_finalize_coverage_with_writer(
                                &backend,
                                &messages,
                                runtime,
                                forced_failure_reasons,
                                &options.extra_instructions,
                                options.max_tokens,
                                step + 2,
                                &run_recorder,
                                &mut run_trace,
                                &mut usage,
                                &events,
                            )
                            .await?;
                            finish_forced_writer_outcome(
                                outcome,
                                &events,
                                step + 2,
                                &mut final_text,
                                &mut completed,
                                &mut terminal_error,
                            );
                        }
                    }
                } else {
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
            }
            Err(e) => {
                let msg = format!("{e:#}");
                let duration_ms = llm_started.elapsed().as_millis() as u64;
                run_recorder.record_llm_error(step + 1, &msg, duration_ms)?;
                run_trace.record_llm_error(
                    step + 1,
                    duration_ms,
                    &forced_system,
                    &messages[traced_len..],
                    &msg,
                );
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

    // Failed runs already signalled ApiError; a Done event on top would give
    // subscribers contradictory success ("✓ done") and failure signals.
    if terminal_error.is_none() {
        emit(
            &events,
            AgentEvent::Done {
                run_id: run_id.clone(),
                steps: step,
                final_text: final_text.clone(),
            },
        );
    }

    let enriched_report = report_with_artifacts(&final_text, Some(&run_state));
    let _ = std::fs::write(run_dir.join("report.md"), &enriched_report);

    let status = if terminal_error.is_some() {
        "failed"
    } else {
        "completed"
    };
    run_recorder.finish(status, step, &usage, terminal_error.as_deref())?;
    run_trace.finish(status, step, &usage, terminal_error.as_deref());

    Ok(AgentOutcome {
        run_id,
        run_dir,
        steps: step,
        final_text,
        usage,
        error: terminal_error,
    })
}

struct ForcedWriterOutcome {
    final_text: Option<String>,
    failure_reasons: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
async fn force_write_final_answer(
    backend: &Arc<dyn Backend>,
    messages: &[Message],
    brief: &ResearchBrief,
    extra_instructions: &str,
    max_tokens: u32,
    first_request_step: u32,
    recorder: &AgentRunRecorder,
    trace: &mut RunTraceBuilder,
    usage: &mut TokenUsage,
    events: &broadcast::Sender<AgentEvent>,
) -> anyhow::Result<ForcedWriterOutcome> {
    let system = forced_final_writer_system_prompt(brief, extra_instructions)?;
    let schemas: Vec<ToolSchema> = Vec::new();
    let mut failure_reasons = Vec::new();
    let mut partial_text: Option<String> = None;
    for attempt in 1..=DEFAULT_MAX_FORCED_WRITER_ATTEMPTS {
        let request_step = first_request_step + attempt - 1;
        let mut request_messages = messages.to_vec();
        request_messages.push(Message::user(forced_final_writer_prompt(attempt)));
        let delta_start = request_messages.len().saturating_sub(1);
        let request_payload =
            backend.request_payload(&system, &request_messages, &schemas, max_tokens)?;
        recorder.record_llm_request(request_step, &request_payload)?;

        let started = Instant::now();
        let response = match send_with_retry(
            backend,
            &system,
            &request_messages,
            &schemas,
            max_tokens,
            request_step,
        )
        .await
        {
            Ok(response) => {
                let duration_ms = started.elapsed().as_millis() as u64;
                recorder.record_llm_response(request_step, &response, duration_ms)?;
                trace.record_llm(
                    request_step,
                    duration_ms,
                    &system,
                    &request_messages[delta_start..],
                    &response,
                );
                response
            }
            Err(error) => {
                let retryable = is_transient_api_error(&error);
                let reason = format!("forced final writer API error: {error:#}");
                let duration_ms = started.elapsed().as_millis() as u64;
                recorder.record_llm_error(request_step, &reason, duration_ms)?;
                trace.record_llm_error(
                    request_step,
                    duration_ms,
                    &system,
                    &request_messages[delta_start..],
                    &reason,
                );
                failure_reasons.push(reason);
                if retryable && attempt < DEFAULT_MAX_FORCED_WRITER_ATTEMPTS {
                    continue;
                }
                break;
            }
        };
        *usage += &response.usage;

        if !response.reasoning_content.trim().is_empty() {
            emit(
                events,
                AgentEvent::Reasoning {
                    step: request_step,
                    text: response.reasoning_content.clone(),
                },
            );
        }
        let (visible_texts, thinking_texts) = split_thinking(&response.text_blocks);
        if !thinking_texts.is_empty() {
            emit(
                events,
                AgentEvent::Reasoning {
                    step: request_step,
                    text: thinking_texts.join("\n"),
                },
            );
        }
        let visible = visible_texts.join("\n").trim().to_string();

        if !response.tool_calls.is_empty() {
            failure_reasons.push(format!(
                "forced writer attempt {attempt} returned {} tool call(s) even though tools were disabled",
                response.tool_calls.len()
            ));
            continue;
        }
        if response.stop_reason == StopReason::MaxTokens {
            failure_reasons.push(format!(
                "forced writer attempt {attempt} was truncated by max_tokens"
            ));
            if crate::agent::research_coverage::is_usable_final_answer(&visible) {
                partial_text = Some(visible);
            }
            if attempt < DEFAULT_MAX_FORCED_WRITER_ATTEMPTS {
                continue;
            }
            break;
        }
        if crate::agent::research_coverage::is_usable_final_answer(&visible) {
            return Ok(ForcedWriterOutcome {
                final_text: Some(visible),
                failure_reasons,
            });
        }
        failure_reasons.push(format!(
            "forced writer attempt {attempt} returned no usable visible text"
        ));
    }

    if let Some(text) = partial_text {
        return Ok(ForcedWriterOutcome {
            final_text: Some(text),
            failure_reasons,
        });
    }
    Ok(ForcedWriterOutcome {
        final_text: None,
        failure_reasons,
    })
}

#[allow(clippy::too_many_arguments)]
async fn force_finalize_coverage_with_writer(
    backend: &Arc<dyn Backend>,
    messages: &[Message],
    runtime: &mut CoverageRuntime,
    mut failure_reasons: Vec<String>,
    extra_instructions: &str,
    max_tokens: u32,
    first_request_step: u32,
    recorder: &AgentRunRecorder,
    trace: &mut RunTraceBuilder,
    usage: &mut TokenUsage,
    events: &broadcast::Sender<AgentEvent>,
) -> anyhow::Result<ForcedWriterOutcome> {
    let mut outcome = force_write_final_answer(
        backend,
        messages,
        &runtime.brief,
        extra_instructions,
        max_tokens,
        first_request_step,
        recorder,
        trace,
        usage,
        events,
    )
    .await?;
    failure_reasons.append(&mut outcome.failure_reasons);
    outcome.failure_reasons = failure_reasons;
    Ok(outcome)
}

struct CoverageRuntime {
    brief: ResearchBrief,
    attempts: u32,
    protocol_retries: u32,
    recovery_rounds: u32,
    last_mile_used: bool,
    last_mile_active: bool,
    revise_only: bool,
    evidence_ids: BTreeMap<String, String>,
    state: Value,
}

impl CoverageRuntime {
    fn new(brief: ResearchBrief, state: Value) -> Self {
        Self {
            brief,
            attempts: 0,
            protocol_retries: 0,
            recovery_rounds: 0,
            last_mile_used: false,
            last_mile_active: false,
            revise_only: false,
            evidence_ids: BTreeMap::new(),
            state,
        }
    }

    fn register_tool_evidence(
        &mut self,
        step: u32,
        sequence: u32,
        _tool_name: &str,
    ) -> (String, String) {
        let canonical = format!("tool:{step}:{sequence}");
        if let Some((id, _)) = self
            .evidence_ids
            .iter()
            .find(|(_, locator)| locator.as_str() == canonical)
        {
            return (id.clone(), canonical);
        }
        let id = format!("E{}", self.evidence_ids.len() + 1);
        self.evidence_ids.insert(id.clone(), canonical.clone());
        (id, canonical)
    }

    fn system_instructions(&self, base: &str) -> String {
        let mut instructions = base.trim_end().to_string();
        instructions.push_str(
            "\n\n# System-generated evidence references\n\
             For submit_research_completion.evidence_refs, prefer the stable evidence IDs below. \
             Do not invent paths or reconstruct tool locators from directory names. The system \
             resolves these IDs to canonical current-run locators.\n",
        );
        if self.evidence_ids.is_empty() {
            instructions.push_str("- No evidence IDs have been issued yet.\n");
        } else {
            for (id, locator) in &self.evidence_ids {
                instructions.push_str(&format!("- {id} = {locator}\n"));
            }
        }
        if self.last_mile_active {
            instructions.push_str(
                "\n# Last-mile recovery is active\n\
                 This is the only scarcity recovery round. Make no more than two precise, \
                 non-duplicate tool calls targeting only the accepted search-scarcity gaps. \
                 Do not broaden the task. The next response after their results must call \
                 submit_research_completion; no further research round is available.\n",
            );
        }
        instructions
    }
}

fn coverage_protocol_can_retry(runtime: &CoverageRuntime, step: u32, max_steps: u32) -> bool {
    runtime.protocol_retries <= DEFAULT_MAX_COVERAGE_PROTOCOL_RETRIES
        && runtime.attempts < DEFAULT_MAX_COMPLETION_ATTEMPTS
        && step < max_steps
}

fn finish_forced_writer_outcome(
    outcome: ForcedWriterOutcome,
    events: &broadcast::Sender<AgentEvent>,
    step: u32,
    final_text: &mut String,
    completed: &mut bool,
    terminal_error: &mut Option<String>,
) {
    if let Some(text) = outcome.final_text {
        *final_text = text;
        emit_final_text(events, step, final_text);
        *completed = true;
        return;
    }
    let detail = if outcome.failure_reasons.is_empty() {
        "the model returned no usable final answer".to_string()
    } else {
        outcome.failure_reasons.join("; ")
    };
    let message = format!("forced finalization failed: {detail}");
    *final_text = format!("Error: {message}");
    *terminal_error = Some(message.clone());
    emit(events, AgentEvent::ApiError { step, message });
}

fn protocol_error_results(tool_calls: &[ToolCall], reason: &str) -> Vec<Block> {
    tool_calls
        .iter()
        .map(|call| Block::ToolResult {
            tool_use_id: call.id.clone(),
            content: completion_tool_result_content(&json!({
                "accepted": false,
                "action": "protocol_error",
                "error": reason,
                "instruction": completion_protocol_correction_prompt(),
            })),
        })
        .collect()
}

fn emit_final_text(events: &broadcast::Sender<AgentEvent>, step: u32, text: &str) {
    emit(
        events,
        AgentEvent::AssistantText {
            step,
            text: text.to_string(),
        },
    );
}

fn completion_limitations(
    submission: &ResearchCompletionSubmission,
    decision: CompletionGateDecision,
) -> Vec<String> {
    let mut limitations = submission.limitations.clone();
    if decision != CompletionGateDecision::FinishWithLimitations {
        return limitations;
    }
    for gap in &submission.gaps {
        if !limitations.contains(&gap.description) {
            limitations.push(gap.description.clone());
        }
        if matches!(
            gap.kind,
            crate::agent::research_coverage::ResearchGapKind::CapabilityBlocked
                | crate::agent::research_coverage::ResearchGapKind::SourceUnavailable
        ) {
            let disclosure = if submission.final_answer.chars().any(is_cjk_char) {
                format!(
                    "受阻证据影响：{}；解除条件：{}",
                    gap.impact_on_deliverable, gap.unblock_requirement
                )
            } else {
                format!(
                    "Blocked evidence impact: {}; unblock requirement: {}",
                    gap.impact_on_deliverable, gap.unblock_requirement
                )
            };
            if !limitations.contains(&disclosure) {
                limitations.push(disclosure);
            }
        }
    }
    for question in &submission.subquestions {
        if matches!(
            question.status,
            crate::agent::research_coverage::SubquestionCoverageStatus::Partial
                | crate::agent::research_coverage::SubquestionCoverageStatus::Missing
                | crate::agent::research_coverage::SubquestionCoverageStatus::Blocked
        ) && !question.material_gap.is_empty()
        {
            let limitation = format!("{}: {}", question.id, question.material_gap);
            if !limitations.contains(&limitation) {
                limitations.push(limitation);
            }
        }
    }
    if !submission.hard_constraints_satisfied
        && !limitations
            .iter()
            .any(|item| item.contains("constraint") || item.contains("约束"))
    {
        limitations.push(
            "One or more requested hard constraints could not be fully satisfied.".to_string(),
        );
    }
    if !submission.stop_conditions_satisfied
        && !limitations
            .iter()
            .any(|item| item.contains("stop condition") || item.contains("停止条件"))
    {
        limitations.push("The planned evidence stop conditions were not fully met.".to_string());
    }
    limitations
}

fn is_cjk_char(ch: char) -> bool {
    matches!(ch as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

struct ResearchPlanningResult {
    envelope: Option<ResearchBriefEnvelope>,
    usage: TokenUsage,
    error: Option<String>,
}

async fn prepare_research_brief(
    task: &str,
    backend: &Arc<dyn Backend>,
    options: &AgentOptions,
) -> anyhow::Result<ResearchPlanningResult> {
    let system = planner_system_prompt();
    let schemas = vec![research_brief_tool_schema()];
    let max_tokens = options
        .research_plan_max_tokens
        .min(options.max_tokens)
        .max(1);
    let mut planning_messages = options.seed_messages.clone();
    planning_messages.push(Message::user(task.to_string()));
    let mut planning_anchor = options.seed_messages.len();
    let is_follow_up = !options.seed_messages.is_empty();
    compact_messages_for_context(
        &mut planning_messages,
        options.compact_after_messages,
        options.keep_recent_messages,
        &mut planning_anchor,
        is_follow_up,
    );

    let mut usage = TokenUsage::default();
    let mut last_error = None;

    for attempt in 1..=2u32 {
        if attempt > 1 {
            planning_messages.push(Message::user(planner_correction_prompt(
                last_error
                    .as_deref()
                    .unwrap_or("the response did not match the planner schema"),
            )));
        }

        let response = match send_with_retry(
            backend,
            &system,
            &planning_messages,
            &schemas,
            max_tokens,
            0,
        )
        .await
        {
            Ok(response) => response,
            Err(error) => {
                last_error = Some(format!("{error:#}"));
                break;
            }
        };

        usage += &response.usage;

        let parsed = (|| -> anyhow::Result<ResearchBriefEnvelope> {
            if response.tool_calls.len() != 1 {
                anyhow::bail!(
                    "planner must make exactly one {SUBMIT_RESEARCH_BRIEF_TOOL} call (got {})",
                    response.tool_calls.len()
                );
            }
            let call = &response.tool_calls[0];
            if call.name != SUBMIT_RESEARCH_BRIEF_TOOL {
                anyhow::bail!(
                    "planner called '{}' instead of {SUBMIT_RESEARCH_BRIEF_TOOL}",
                    call.name
                );
            }
            ResearchBriefEnvelope::from_tool_input(call.input.clone())
        })();

        match parsed {
            Ok(envelope) => {
                return Ok(ResearchPlanningResult {
                    envelope: Some(envelope),
                    usage,
                    error: None,
                });
            }
            Err(error) => {
                let error = format!("{error:#}");
                last_error = Some(error);
            }
        }
    }

    Ok(ResearchPlanningResult {
        envelope: None,
        usage,
        error: last_error,
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
async fn send_with_retry(
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

/// Rebuild the exact bounded assistant history form from a persisted response.
/// Interrupted-run replay uses this instead of exposing reasoning-only text or
/// growing history beyond the limits applied during the original live run.
pub(crate) fn assistant_blocks_for_history(response: &LLMResponse) -> Vec<Block> {
    let (visible_texts, _) = split_thinking(&response.text_blocks);
    build_assistant_blocks(response, &visible_texts)
}
