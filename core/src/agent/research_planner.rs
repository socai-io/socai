//! Planner execution for the research workflow.

use std::sync::Arc;
use std::time::Instant;

use crate::agent::llm::{Backend, Message, TokenUsage};
use crate::agent::memory::compact_messages_for_context;
use crate::agent::r#loop::send_with_retry;
use crate::agent::research::{
    planner_correction_prompt, planner_system_prompt, research_brief_tool_schema,
    ResearchBriefEnvelope, DEFAULT_RESEARCH_PLAN_MAX_TOKENS, SUBMIT_RESEARCH_BRIEF_TOOL,
};
use crate::agent::run_logging::AgentRunRecorder;
use crate::telemetry::trace::RunTraceBuilder;

pub(crate) struct ResearchPlanningResult {
    pub envelope: Option<ResearchBriefEnvelope>,
    pub usage: TokenUsage,
    pub error: Option<String>,
    pub steps: u32,
}

pub(crate) async fn prepare_research_brief(
    task: &str,
    backend: &Arc<dyn Backend>,
    seed_messages: &[Message],
    max_tokens: u32,
    first_step: u32,
    max_steps: u32,
    compact_after_messages: usize,
    keep_recent_messages: usize,
    recorder: &AgentRunRecorder,
    trace: &mut RunTraceBuilder,
) -> anyhow::Result<ResearchPlanningResult> {
    let system = planner_system_prompt(!seed_messages.is_empty());
    let schemas = vec![research_brief_tool_schema()];
    let max_tokens = DEFAULT_RESEARCH_PLAN_MAX_TOKENS.min(max_tokens).max(1);
    let mut messages = seed_messages.to_vec();
    messages.push(Message::user(task.to_string()));
    let mut anchor = seed_messages.len();
    compact_messages_for_context(
        &mut messages,
        compact_after_messages,
        keep_recent_messages,
        &mut anchor,
        !seed_messages.is_empty(),
    );
    let mut usage = TokenUsage::default();
    let mut last_error = None;
    let mut steps = 0u32;

    // Leave at least one step for the actual task, keeping two planner attempts
    // as a ceiling rather than expanding the overall run budget.
    for attempt in 1..=max_steps.saturating_sub(first_step).min(2) {
        steps = attempt;
        let step = first_step + attempt - 1;
        if attempt > 1 {
            messages.push(Message::user(planner_correction_prompt(
                last_error
                    .as_deref()
                    .unwrap_or("the response did not match the planner schema"),
            )));
        }
        let delta_start = if attempt == 1 {
            anchor
        } else {
            messages.len().saturating_sub(1)
        };
        let request_payload = backend.request_payload(&system, &messages, &schemas, max_tokens)?;
        recorder.record_llm_request(step, &request_payload)?;
        let started = Instant::now();
        let response =
            match send_with_retry(backend, &system, &messages, &schemas, max_tokens, step).await {
                Ok(response) => {
                    let duration_ms = started.elapsed().as_millis() as u64;
                    recorder.record_llm_response(step, &response, duration_ms)?;
                    trace.record_llm(
                        step,
                        duration_ms,
                        &system,
                        &messages[delta_start..],
                        &response,
                    );
                    response
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    let duration_ms = started.elapsed().as_millis() as u64;
                    recorder.record_llm_error(step, &message, duration_ms)?;
                    trace.record_llm_error(
                        step,
                        duration_ms,
                        &system,
                        &messages[delta_start..],
                        &message,
                    );
                    last_error = Some(message);
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
                    steps: attempt,
                });
            }
            Err(error) => last_error = Some(format!("{error:#}")),
        }
    }
    Ok(ResearchPlanningResult {
        envelope: None,
        usage,
        error: last_error,
        steps,
    })
}
