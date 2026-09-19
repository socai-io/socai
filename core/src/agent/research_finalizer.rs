//! No-tool final writer used after research protocol or transport failure.

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::broadcast;

use crate::agent::api_errors::is_transient_api_error;
use crate::agent::llm::{Backend, Message, StopReason, TokenUsage, ToolSchema};
use crate::agent::r#loop::{emit, send_with_retry, split_thinking, AgentEvent};
use crate::agent::research::ResearchBrief;
use crate::agent::research_coverage::{
    forced_final_writer_prompt, forced_final_writer_system_prompt, is_usable_final_answer,
    DEFAULT_MAX_FORCED_WRITER_ATTEMPTS,
};
use crate::agent::run_logging::AgentRunRecorder;
use crate::telemetry::trace::RunTraceBuilder;

const FINAL_ANSWER_TERMINATOR: &str = "<!-- SOCAI_FINAL_COMPLETE -->";

pub(crate) struct ForcedWriterOutcome {
    pub final_text: Option<String>,
    pub failure_reasons: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_forced_final_writer(
    brief: &ResearchBrief,
    extra_instructions: &str,
    mut failure_reasons: Vec<String>,
    first_request_step: u32,
    backend: &Arc<dyn Backend>,
    messages: &[Message],
    max_tokens: u32,
    recorder: &AgentRunRecorder,
    trace: &mut RunTraceBuilder,
    usage: &mut TokenUsage,
    events: &broadcast::Sender<AgentEvent>,
) -> anyhow::Result<ForcedWriterOutcome> {
    let system = format!(
        "{}\n\nFinalization completion contract:\n\
         - End the complete answer with {FINAL_ANSWER_TERMINATOR}.\n\
         - Do not write anything after that marker.",
        forced_final_writer_system_prompt(brief, extra_instructions)?
    );
    let schemas: Vec<ToolSchema> = Vec::new();

    for attempt in 1..=DEFAULT_MAX_FORCED_WRITER_ATTEMPTS {
        let request_step = first_request_step + attempt - 1;
        let mut request_messages = messages.to_vec();
        request_messages.push(Message::user(format!(
            "{}\n\nEnd the complete answer with {FINAL_ANSWER_TERMINATOR}.",
            forced_final_writer_prompt(attempt)
        )));
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
            if attempt < DEFAULT_MAX_FORCED_WRITER_ATTEMPTS {
                continue;
            }
            break;
        }
        if let Some(final_text) = complete_final_answer(&visible) {
            return Ok(ForcedWriterOutcome {
                final_text: Some(final_text),
                failure_reasons,
            });
        }
        let reason = if is_usable_final_answer(&visible) {
            format!("forced writer attempt {attempt} omitted the completion terminator")
        } else {
            format!("forced writer attempt {attempt} returned no usable visible text")
        };
        failure_reasons.push(reason);
    }

    Ok(ForcedWriterOutcome {
        final_text: None,
        failure_reasons,
    })
}

fn complete_final_answer(visible: &str) -> Option<String> {
    let final_text = visible
        .trim()
        .strip_suffix(FINAL_ANSWER_TERMINATOR)?
        .trim_end();
    is_usable_final_answer(final_text).then(|| final_text.to_string())
}
