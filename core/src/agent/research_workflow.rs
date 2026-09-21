//! Research-specific orchestration layered onto the generic agent loop.
//!
//! The core loop owns transport, message history, tool dispatch, events, and
//! run persistence. This module owns the Planner -> Coverage -> Finalize state
//! machine and exposes narrow lifecycle hooks back to that loop.

use std::collections::BTreeMap;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::agent::llm::{
    Block, LLMResponse, Message, TokenUsage, ToolCall, ToolResultContent, ToolSchema,
};
use crate::agent::memory::compact_messages_for_context;
use crate::agent::r#loop::{
    build_assistant_blocks, emit, send_with_retry, split_thinking, AgentEvent,
};
use crate::agent::research::{ResearchBrief, FOLLOWUP_EVIDENCE_INSTRUCTIONS};
use crate::agent::research_coverage::{
    answer_with_missing_limitations, budget_exhausted_completion_prompt, completion_limitations,
    completion_protocol_correction_prompt, completion_tool_result_content, evaluate_completion,
    initial_coverage_state, research_completion_tool_schema, salvage_completion_text,
    CompletionGateDecision, EvidenceLocatorCatalog, ResearchCompletionSubmission,
    DEFAULT_MAX_COMPLETION_ATTEMPTS, DEFAULT_MAX_COVERAGE_PROTOCOL_RETRIES,
    DEFAULT_MAX_RESEARCH_RECOVERY_ROUNDS, SUBMIT_RESEARCH_COMPLETION_TOOL,
};
use crate::agent::research_finalizer::{run_forced_final_writer, ForcedWriterOutcome};
use crate::agent::research_planner::prepare_research_brief;
use crate::agent::system_prompt::build_system_prompt;
use crate::agent::workflow::{
    research_available, PreparedWorkflow, ResponseDirective, TerminalCause, WorkflowIo,
    WorkflowOutcome, WorkflowPlugin, WorkflowPrepareContext,
};

pub(crate) struct ResearchWorkflow {
    runtime: Option<CoverageRuntime>,
    base_instructions: String,
    execution_instructions: String,
}

impl ResearchWorkflow {
    pub async fn prepare(
        ctx: WorkflowPrepareContext<'_>,
    ) -> anyhow::Result<PreparedWorkflow<Self>> {
        if !research_available(ctx.enabled_sites) {
            return Ok(PreparedWorkflow {
                workflow: Self {
                    runtime: None,
                    base_instructions: ctx.extra_instructions.to_string(),
                    execution_instructions: ctx.extra_instructions.to_string(),
                },
                usage: TokenUsage::default(),
                immediate_text: None,
                error: None,
                planning_steps: 0,
            });
        }

        let planning = prepare_research_brief(
            ctx.task,
            ctx.backend,
            ctx.seed_messages,
            ctx.max_tokens,
            ctx.first_step,
            ctx.max_steps,
            ctx.compact_after_messages,
            ctx.keep_recent_messages,
            ctx.recorder,
            ctx.trace,
        )
        .await?;
        let mut instructions = ctx.extra_instructions.to_string();
        if !ctx.seed_messages.is_empty() {
            instructions.push_str("\n\n");
            instructions.push_str(FOLLOWUP_EVIDENCE_INSTRUCTIONS);
        }
        let mut workflow = Self {
            runtime: None,
            base_instructions: instructions.clone(),
            execution_instructions: instructions,
        };
        let mut immediate_text = None;
        let mut planning_error = planning.error.clone();

        if let Some(envelope) = planning.envelope {
            match envelope.persist(ctx.run_dir) {
                Ok(()) => {
                    if let Some(question) = envelope.clarification() {
                        immediate_text = Some(question.to_string());
                        planning_error = None;
                    } else if let Some(brief) = envelope.brief() {
                        match brief.execution_prompt() {
                            Ok(prompt) => {
                                if !workflow.execution_instructions.trim().is_empty() {
                                    workflow.execution_instructions.push_str("\n\n");
                                }
                                workflow.execution_instructions.push_str(&prompt);
                                workflow.runtime = Some(CoverageRuntime::new(
                                    brief.clone(),
                                    initial_coverage_state(brief),
                                ));
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

        if workflow.runtime.is_none() && immediate_text.is_none() {
            warn!(
                error = planning_error
                    .as_deref()
                    .unwrap_or("unknown planning failure"),
                "research brief planning failed; falling back to reactive execution"
            );
        }

        Ok(PreparedWorkflow {
            workflow,
            usage: planning.usage,
            immediate_text,
            error: None,
            planning_steps: planning.steps,
        })
    }

    pub fn is_active(&self) -> bool {
        self.runtime.is_some()
    }

    /// Hook 2: adjust the next main-loop request without exposing coverage
    /// internals to the generic loop.
    pub fn before_step(&self, schemas: &mut Vec<ToolSchema>) {
        let Some(runtime) = &self.runtime else {
            return;
        };
        if runtime.revise_only {
            schemas.clear();
        }
        schemas.push(research_completion_tool_schema());
    }

    pub fn system_instructions(&self) -> String {
        let Some(runtime) = &self.runtime else {
            return self.base_instructions.clone();
        };
        runtime.system_instructions(&self.execution_instructions)
    }

    /// Hook 3: inspect a model response before the generic loop decides that
    /// prose is final or dispatches tools.
    pub async fn inspect_response(
        &mut self,
        response: &LLMResponse,
        visible_texts: &[String],
        step: u32,
        io: &mut WorkflowIo<'_>,
    ) -> anyhow::Result<ResponseDirective> {
        let finalizer_instructions = self.base_instructions.as_str();
        let Some(runtime) = self.runtime.as_mut() else {
            return Ok(ResponseDirective::UseDefaultLoop);
        };
        let completion_calls: Vec<&ToolCall> = response
            .tool_calls
            .iter()
            .filter(|call| call.name == SUBMIT_RESEARCH_COMPLETION_TOOL)
            .collect();

        if !completion_calls.is_empty() {
            push_assistant(response, visible_texts, io);
            runtime.attempts = runtime.attempts.saturating_add(1);
            let attempt = runtime.attempts;

            if response.tool_calls.len() != 1 || completion_calls.len() != 1 {
                let reason = format!(
                    "{SUBMIT_RESEARCH_COMPLETION_TOOL} must be the only tool call in its response"
                );
                runtime.protocol_retries = runtime.protocol_retries.saturating_add(1);
                runtime.state = json!({
                    "tool_calls": response.tool_calls.iter().map(|call| json!({
                        "name": call.name,
                        "input": call.input,
                    })).collect::<Vec<_>>(),
                });
                io.messages
                    .push(Message::user_blocks(protocol_error_results(
                        &response.tool_calls,
                        &reason,
                    )));
                if coverage_protocol_can_retry(runtime, step, io.max_steps) {
                    return Ok(ResponseDirective::Continue);
                }
                if let Some((answer, _)) = salvage_completion_text(
                    completion_calls.first().map(|call| &call.input),
                    visible_texts,
                ) {
                    emit_final_text(io.events, step, &answer);
                    return Ok(ResponseDirective::Complete(answer));
                }
                let brief = runtime.brief.clone();
                let outcome = force_write_final_answer(
                    &brief,
                    finalizer_instructions,
                    vec![reason],
                    step + 1,
                    io,
                )
                .await?;
                return Ok(response_directive(outcome, io.events, step + 1));
            }

            let call = completion_calls[0];
            let evidence =
                EvidenceLocatorCatalog::from_run(io.run_dir, io.run_state, &runtime.evidence_ids);
            let submission = match ResearchCompletionSubmission::from_tool_input(
                call.input.clone(),
                &runtime.brief,
                &evidence,
            ) {
                Ok(submission) => submission,
                Err(error) => {
                    let reason = format!("{error:#}");
                    runtime.protocol_retries = runtime.protocol_retries.saturating_add(1);
                    runtime.state = call.input.clone();
                    let instruction = if coverage_protocol_can_retry(runtime, step, io.max_steps) {
                        completion_protocol_correction_prompt()
                    } else {
                        "Research has ended. A final-answer writer will produce the user-facing answer without tools."
                    };
                    io.messages
                        .push(Message::user_blocks(vec![Block::ToolResult {
                            tool_use_id: call.id.clone(),
                            content: completion_tool_result_content(&json!({
                                "accepted": false,
                                "action": "protocol_error",
                                "error": reason,
                                "instruction": instruction,
                            })),
                        }]));
                    if coverage_protocol_can_retry(runtime, step, io.max_steps) {
                        return Ok(ResponseDirective::Continue);
                    }
                    if let Some((answer, _)) =
                        salvage_completion_text(Some(&call.input), visible_texts)
                    {
                        emit_final_text(io.events, step, &answer);
                        return Ok(ResponseDirective::Complete(answer));
                    }
                    let brief = runtime.brief.clone();
                    let outcome = force_write_final_answer(
                        &brief,
                        finalizer_instructions,
                        vec![reason],
                        step + 1,
                        io,
                    )
                    .await?;
                    return Ok(response_directive(outcome, io.events, step + 1));
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
                CompletionGateDecision::Accept | CompletionGateDecision::FinishWithLimitations => {}
            }
            runtime.state = serde_json::to_value(&submission).map_err(anyhow::Error::from)?;

            match decision {
                CompletionGateDecision::Accept | CompletionGateDecision::FinishWithLimitations => {
                    let limitations = completion_limitations(&submission, decision);
                    let answer =
                        answer_with_missing_limitations(&submission.final_answer, &limitations);
                    emit_final_text(io.events, step, &answer);
                    Ok(ResponseDirective::Complete(answer))
                }
                CompletionGateDecision::ResearchMore
                | CompletionGateDecision::LastMileResearch
                | CompletionGateDecision::ReviseOnly => {
                    let value = gate.tool_result_value(&runtime.brief, &submission);
                    io.messages
                        .push(Message::user_blocks(vec![Block::ToolResult {
                            tool_use_id: call.id.clone(),
                            content: completion_tool_result_content(&value),
                        }]));
                    Ok(ResponseDirective::Continue)
                }
            }
        } else if response.tool_calls.is_empty() {
            push_assistant(response, visible_texts, io);
            runtime.protocol_retries = runtime.protocol_retries.saturating_add(1);
            runtime.state = json!({ "ordinary_prose_attempt": visible_texts });
            if coverage_protocol_can_retry(runtime, step, io.max_steps) {
                io.messages
                    .push(Message::user(completion_protocol_correction_prompt()));
                return Ok(ResponseDirective::Continue);
            }
            let reason =
                "coverage completion protocol retries exhausted after ordinary prose".to_string();
            if let Some((answer, _)) = salvage_completion_text(None, visible_texts) {
                emit_final_text(io.events, step, &answer);
                return Ok(ResponseDirective::Complete(answer));
            }
            let brief = runtime.brief.clone();
            let outcome = force_write_final_answer(
                &brief,
                finalizer_instructions,
                vec![reason],
                step + 1,
                io,
            )
            .await?;
            Ok(response_directive(outcome, io.events, step + 1))
        } else if runtime.last_mile_active && response.tool_calls.len() > 2 {
            let reason = format!(
                "last-mile recovery permits at most two tool calls (got {})",
                response.tool_calls.len()
            );
            push_assistant(response, visible_texts, io);
            io.messages
                .push(Message::user_blocks(protocol_error_results(
                    &response.tool_calls,
                    &reason,
                )));
            runtime.last_mile_active = false;
            runtime.revise_only = true;
            runtime.state = json!({
                "last_mile_protocol_error": reason,
                "requested_tool_calls": response.tool_calls.len(),
            });
            Ok(ResponseDirective::Continue)
        } else {
            Ok(ResponseDirective::UseDefaultLoop)
        }
    }

    /// Hook 4: attach the short evidence identifier before the tool result is
    /// appended to model history.
    pub fn decorate_tool_result(
        &mut self,
        step: u32,
        sequence: u32,
        tool_name: &str,
        error: Option<&str>,
        history_content: &mut Vec<ToolResultContent>,
    ) {
        let Some(runtime) = self.runtime.as_mut() else {
            return;
        };
        if error.is_some() {
            return;
        }
        let (evidence_id, canonical_locator) =
            runtime.register_tool_evidence(step, sequence, tool_name);
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

    pub fn finish_tool_step(&mut self) {
        if let Some(runtime) = self.runtime.as_mut() {
            if runtime.last_mile_active {
                runtime.last_mile_active = false;
                runtime.revise_only = true;
            }
        }
    }

    /// Hook 5a: salvage an interrupted research run when evidence exists.
    pub async fn on_terminal_error(
        &mut self,
        reason: &str,
        prior_final_text: &str,
        step: u32,
        io: &mut WorkflowIo<'_>,
    ) -> anyhow::Result<Option<WorkflowOutcome>> {
        let finalizer_instructions = self.base_instructions.as_str();
        let Some(runtime) = self.runtime.as_ref() else {
            return Ok(None);
        };
        if runtime.evidence_ids.is_empty()
            && !io.run_state.has_structured_state()
            && !crate::agent::research_coverage::is_usable_final_answer(prior_final_text)
        {
            return Ok(None);
        }
        info!(step, error = %reason, "terminal research error; invoking final writer");
        let brief = runtime.brief.clone();
        let outcome = force_write_final_answer(
            &brief,
            finalizer_instructions,
            vec![format!(
                "main research loop ended with API/protocol error: {reason}"
            )],
            step + 1,
            io,
        )
        .await?;
        Ok(Some(workflow_outcome(outcome, io.events, step + 1)))
    }

    /// Hook 5b: structured max-step completion followed by the same bounded
    /// salvage/forced-writer chain used by PR #296.
    pub async fn on_max_steps(
        &mut self,
        step: u32,
        prior_final_text: &str,
        io: &mut WorkflowIo<'_>,
    ) -> anyhow::Result<Option<WorkflowOutcome>> {
        let finalizer_instructions = self.base_instructions.as_str();
        let Some(runtime) = self.runtime.as_mut() else {
            return Ok(None);
        };
        io.messages
            .push(Message::user(budget_exhausted_completion_prompt(
                io.max_steps,
            )));
        if compact_messages_for_context(
            io.messages,
            io.compact_after_messages,
            io.keep_recent_messages,
            io.anchor_user_index,
            io.is_follow_up,
        ) {
            *io.traced_len = io.messages.len();
        }
        let request_messages = io.messages.clone();
        let forced_schemas = vec![research_completion_tool_schema()];
        let forced_system = build_system_prompt(
            &[SUBMIT_RESEARCH_COMPLETION_TOOL],
            &runtime.system_instructions(&self.execution_instructions),
        );
        let request_payload = io.backend.request_payload(
            &forced_system,
            &request_messages,
            &forced_schemas,
            io.max_tokens,
        )?;
        io.recorder.record_llm_request(step + 1, &request_payload)?;
        let started = Instant::now();
        let response = match send_with_retry(
            io.backend,
            &forced_system,
            &request_messages,
            &forced_schemas,
            io.max_tokens,
            step + 1,
        )
        .await
        {
            Ok(response) => {
                let duration_ms = started.elapsed().as_millis() as u64;
                io.recorder
                    .record_llm_response(step + 1, &response, duration_ms)?;
                io.trace.record_llm(
                    step + 1,
                    duration_ms,
                    &forced_system,
                    &io.messages[*io.traced_len..],
                    &response,
                );
                response
            }
            Err(error) => {
                let message = format!("{error:#}");
                let duration_ms = started.elapsed().as_millis() as u64;
                io.recorder
                    .record_llm_error(step + 1, &message, duration_ms)?;
                io.trace.record_llm_error(
                    step + 1,
                    duration_ms,
                    &forced_system,
                    &io.messages[*io.traced_len..],
                    &message,
                );
                warn!(step = step + 1, error = %message, "forced summary error");
                emit(
                    io.events,
                    AgentEvent::ApiError {
                        step: step + 1,
                        message: message.clone(),
                    },
                );
                return Ok(Some(WorkflowOutcome::Fail {
                    final_text: prior_final_text.to_string(),
                    error: message,
                }));
            }
        };
        *io.usage += &response.usage;
        let (visible_texts, _) = split_thinking(&response.text_blocks);
        runtime.attempts = runtime.attempts.saturating_add(1);
        let completion_call = (response.tool_calls.len() == 1
            && response.tool_calls[0].name == SUBMIT_RESEARCH_COMPLETION_TOOL)
            .then(|| &response.tool_calls[0]);
        let mut failure_reasons = Vec::new();
        let salvage;

        if let Some(call) = completion_call {
            let evidence =
                EvidenceLocatorCatalog::from_run(io.run_dir, io.run_state, &runtime.evidence_ids);
            match ResearchCompletionSubmission::from_tool_input(
                call.input.clone(),
                &runtime.brief,
                &evidence,
            ) {
                Ok(submission) => {
                    let gate = evaluate_completion(&runtime.brief, &submission, true, false);
                    let decision = match gate.decision {
                        CompletionGateDecision::ResearchMore
                        | CompletionGateDecision::LastMileResearch
                        | CompletionGateDecision::ReviseOnly => {
                            CompletionGateDecision::FinishWithLimitations
                        }
                        decision => decision,
                    };
                    runtime.state =
                        serde_json::to_value(&submission).map_err(anyhow::Error::from)?;
                    let limitations = completion_limitations(&submission, decision);
                    let answer =
                        answer_with_missing_limitations(&submission.final_answer, &limitations);
                    emit_final_text(io.events, step + 1, &answer);
                    return Ok(Some(WorkflowOutcome::Complete(answer)));
                }
                Err(error) => {
                    runtime.state = call.input.clone();
                    salvage = salvage_completion_text(Some(&call.input), &visible_texts);
                    failure_reasons.push(format!("{error:#}"));
                }
            }
        } else {
            runtime.state = json!({
                "tool_calls": response.tool_calls.iter().map(|call| json!({
                    "name": call.name,
                    "input": call.input,
                })).collect::<Vec<_>>(),
                "visible_text": visible_texts,
            });
            salvage = salvage_completion_text(None, &visible_texts);
            failure_reasons.push(format!(
                "budget finalization requires exactly one {SUBMIT_RESEARCH_COMPLETION_TOOL} call"
            ));
        }

        if let Some((answer, _)) = salvage {
            emit_final_text(io.events, step + 1, &answer);
            return Ok(Some(WorkflowOutcome::Complete(answer)));
        }
        let brief = runtime.brief.clone();
        let outcome = force_write_final_answer(
            &brief,
            finalizer_instructions,
            failure_reasons,
            step + 2,
            io,
        )
        .await?;
        Ok(Some(workflow_outcome(outcome, io.events, step + 2)))
    }
}

#[async_trait]
impl WorkflowPlugin for ResearchWorkflow {
    async fn prepare(ctx: WorkflowPrepareContext<'_>) -> anyhow::Result<PreparedWorkflow<Self>> {
        ResearchWorkflow::prepare(ctx).await
    }

    fn is_active(&self) -> bool {
        ResearchWorkflow::is_active(self)
    }

    fn system_instructions(&self) -> String {
        ResearchWorkflow::system_instructions(self)
    }

    fn before_step(&mut self, schemas: &mut Vec<ToolSchema>) {
        ResearchWorkflow::before_step(self, schemas);
    }

    async fn inspect_response(
        &mut self,
        response: &LLMResponse,
        visible_texts: &[String],
        step: u32,
        io: &mut WorkflowIo<'_>,
    ) -> anyhow::Result<ResponseDirective> {
        ResearchWorkflow::inspect_response(self, response, visible_texts, step, io).await
    }

    fn decorate_tool_result(
        &mut self,
        step: u32,
        sequence: u32,
        is_last_in_step: bool,
        tool_name: &str,
        error: Option<&str>,
        history_content: &mut Vec<ToolResultContent>,
    ) {
        ResearchWorkflow::decorate_tool_result(
            self,
            step,
            sequence,
            tool_name,
            error,
            history_content,
        );
        if is_last_in_step {
            ResearchWorkflow::finish_tool_step(self);
        }
    }

    async fn on_terminal(
        &mut self,
        cause: TerminalCause<'_>,
        prior_final_text: &str,
        step: u32,
        io: &mut WorkflowIo<'_>,
    ) -> anyhow::Result<Option<WorkflowOutcome>> {
        match cause {
            TerminalCause::Error(reason) => {
                ResearchWorkflow::on_terminal_error(self, reason, prior_final_text, step, io).await
            }
            TerminalCause::MaxSteps => {
                ResearchWorkflow::on_max_steps(self, step, prior_final_text, io).await
            }
        }
    }
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
        if self.last_mile_active {
            instructions.push_str(
                "\n# Last-mile recovery\n\
                 Target only the accepted search-scarcity gaps with at most two precise, \
                 non-duplicate tool calls; then submit completion. Do not broaden.\n",
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

fn push_assistant(response: &LLMResponse, visible_texts: &[String], io: &mut WorkflowIo<'_>) {
    io.messages
        .push(Message::assistant_blocks(build_assistant_blocks(
            response,
            visible_texts,
        )));
    *io.traced_len = io.messages.len();
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

async fn force_write_final_answer(
    brief: &ResearchBrief,
    extra_instructions: &str,
    failure_reasons: Vec<String>,
    first_request_step: u32,
    io: &mut WorkflowIo<'_>,
) -> anyhow::Result<ForcedWriterOutcome> {
    run_forced_final_writer(
        brief,
        extra_instructions,
        failure_reasons,
        first_request_step,
        io.backend,
        io.messages,
        io.max_tokens,
        io.recorder,
        io.trace,
        io.usage,
        io.events,
    )
    .await
}

fn response_directive(
    outcome: ForcedWriterOutcome,
    events: &broadcast::Sender<AgentEvent>,
    step: u32,
) -> ResponseDirective {
    match workflow_outcome(outcome, events, step) {
        WorkflowOutcome::Complete(text) => ResponseDirective::Complete(text),
        WorkflowOutcome::Fail { final_text, error } => {
            ResponseDirective::Fail { final_text, error }
        }
    }
}

fn workflow_outcome(
    outcome: ForcedWriterOutcome,
    events: &broadcast::Sender<AgentEvent>,
    step: u32,
) -> WorkflowOutcome {
    if let Some(text) = outcome.final_text {
        emit_final_text(events, step, &text);
        return WorkflowOutcome::Complete(text);
    }
    let detail = if outcome.failure_reasons.is_empty() {
        "the model returned no usable final answer".to_string()
    } else {
        outcome.failure_reasons.join("; ")
    };
    let error = format!("forced finalization failed: {detail}");
    emit(
        events,
        AgentEvent::ApiError {
            step,
            message: error.clone(),
        },
    );
    WorkflowOutcome::Fail {
        final_text: format!("Error: {error}"),
        error,
    }
}
