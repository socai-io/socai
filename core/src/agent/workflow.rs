//! The small lifecycle contract shared by agent workflows.
//!
//! The core loop still owns model calls, tools, messages, and persistence.
//! Workflow-specific decisions are made at these boundaries.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::warn;

use crate::agent::llm::{Backend, LLMResponse, Message, TokenUsage, ToolResultContent, ToolSchema};
use crate::agent::r#loop::AgentEvent;
use crate::agent::reactive_workflow::ReactivePlugin;
use crate::agent::research_workflow::ResearchWorkflow;
use crate::agent::run_logging::AgentRunRecorder;
use crate::agent::run_state::RunState;
use crate::agent::workflow_router::{route, WorkflowSelection};
use crate::telemetry::trace::RunTraceBuilder;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowPreference {
    Auto,
    Reactive,
    Research,
}

impl std::str::FromStr for WorkflowPreference {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "reactive" => Ok(Self::Reactive),
            "research" => Ok(Self::Research),
            _ => anyhow::bail!("workflow must be auto, reactive or research"),
        }
    }
}

const EVIDENCE_INSTRUCTIONS: &str = "## Evidence and delivery
Separate observed facts, source claims and your inferences. Bind evidence to
the same entity, variant and time; similar names do not establish identity.
Advertising, hearsay and platform AI summaries are not independent verification
or population consensus. Failed-page diagnostics are not the target note.
Preserve uncertainty in headings, tables, summaries and recommendations; an
unknown decisive criterion cannot become 'all requirements met'. Summarizing
an earlier answer must not increase its certainty. A disclaimer does not repair
an unsupported main conclusion. Do not guess missing years, units or eligibility
conditions; distinguish posting, editing and event dates, and past from current
states. Link decisive external claims to obtained sources, never invented URLs
or internal E-IDs. State missing evidence while still giving the useful supported
answer. Do not claim manual or exhaustive verification.
Tool results may be abridged: respect read failures, content sources and OCR
coverage. Check reported active filters against the requested filters before
inferring scarcity; a mismatch or absent readback does not prove an empty market.
Read omitted material from the current run artifact when needed, not by default
before every new search. Do not treat an omitted field as a negative finding.";

const FOLLOWUP_INSTRUCTIONS: &str = "## Current turn
Resolve this request using the relevant earlier user messages and assistant
answer, including any proposal the user is accepting. Apply the user's new
goals, scope and preferences over conflicting earlier requirements; retain only
relevant unchanged constraints. Do not reopen unaffected completed work.
Prior assistant claims are context, not primary evidence. Verify challenged
external claims rather than automatically defending or rejecting the old answer.
For rewrites or delivery repairs, do not invent new research. For fresh facts or
new samples, obtain the necessary evidence; never present old data as newly read.
Use relevant historical raw material only when its source, scope and freshness
fit this request; there is no requirement to check local files before searching.
If prior work failed or delivery is incomplete, establish the actual remaining
work rather than trusting a completed label. Clarify only if multiple plausible
referents materially change the task. Deliver this turn's result; explain
corrections or remaining uncertainty when relevant.";

pub(crate) fn research_available(enabled_sites: &[String]) -> bool {
    enabled_sites.len() == 1 && enabled_sites[0] == "xhs"
}

pub(crate) struct WorkflowPrepareContext<'a> {
    pub preference: Option<WorkflowPreference>,
    pub first_step: u32,
    pub max_steps: u32,
    pub task: &'a str,
    pub backend: &'a Arc<dyn Backend>,
    pub seed_messages: &'a [Message],
    pub enabled_sites: &'a [String],
    pub max_tokens: u32,
    pub compact_after_messages: usize,
    pub keep_recent_messages: usize,
    pub run_dir: &'a Path,
    pub extra_instructions: &'a str,
    pub recorder: &'a AgentRunRecorder,
    pub trace: &'a mut RunTraceBuilder,
    pub events: &'a broadcast::Sender<AgentEvent>,
}

pub(crate) struct PreparedWorkflow<W> {
    pub workflow: W,
    pub usage: TokenUsage,
    pub immediate_text: Option<String>,
    pub error: Option<String>,
    /// All preflight attempts, including routing, counted in the run budget.
    pub planning_steps: u32,
}

/// Reactive stays the default; Research is selected explicitly or through Auto.
/// Failed preparation retains its usage and attempt count without a second loop.
pub(crate) async fn prepare_selected_workflow(
    mut ctx: WorkflowPrepareContext<'_>,
) -> anyhow::Result<PreparedWorkflow<Box<dyn WorkflowPlugin>>> {
    let (preference, source) = match ctx.preference {
        Some(preference) => (preference, "request"),
        None => match std::env::var("SOCAI_WORKFLOW") {
            Ok(value) if !value.is_empty() => match value.parse() {
                Ok(preference) => (preference, "environment"),
                Err(_) => {
                    warn!(%value, "unknown workflow; using reactive");
                    (WorkflowPreference::Reactive, "invalid_environment")
                }
            },
            _ => (WorkflowPreference::Reactive, "default"),
        },
    };
    let mut selection = WorkflowSelection::new(preference, source);
    let available = research_available(ctx.enabled_sites);
    match preference {
        WorkflowPreference::Auto if !available => selection.fallback("capability_restriction"),
        WorkflowPreference::Auto if ctx.max_steps < 2 => selection.fallback("step_budget_exceeded"),
        WorkflowPreference::Auto => route(&mut ctx, &mut selection).await?,
        WorkflowPreference::Research if !available => {
            selection.error = Some("Research requires exactly the Xiaohongshu site; select Reactive or Auto for this configuration.".into());
        }
        other => selection.selected_mode = Some(other),
    }
    let mut instructions = ctx.extra_instructions.to_string();
    if ctx.enabled_sites.iter().any(|site| site == "xhs") {
        instructions.push_str("\n\n");
        instructions.push_str(EVIDENCE_INSTRUCTIONS);
    }
    if !ctx.seed_messages.is_empty() {
        instructions.push_str("\n\n");
        instructions.push_str(FOLLOWUP_INSTRUCTIONS);
    }
    let run_dir = ctx.run_dir;
    let events = ctx.events;
    let preflight_steps = selection.router_attempts;
    if let Some(error) = selection.error.clone() {
        selection.prepare_outcome = "failed";
        selection.persist(run_dir)?;
        return Ok(PreparedWorkflow {
            workflow: Box::new(ReactivePlugin::new(instructions)),
            usage: selection.router_usage,
            immediate_text: None,
            error: Some(error),
            planning_steps: preflight_steps,
        });
    }
    // Reborrow the owned instruction suffix only for preparation. Plugins copy
    // what they need; none holds a long-lived borrow of loop state.
    let ctx = WorkflowPrepareContext {
        extra_instructions: &instructions,
        first_step: ctx.first_step + preflight_steps,
        ..ctx
    };
    let mut prepared = if selection.selected_mode == Some(WorkflowPreference::Research) {
        let prepared = <ResearchWorkflow as WorkflowPlugin>::prepare(ctx).await?;
        selection.prepare_outcome = if prepared.immediate_text.is_some() {
            "clarify"
        } else if prepared.workflow.is_active() {
            "ready"
        } else {
            "planner_fallback"
        };
        selection.effective_mode = Some(
            if prepared.workflow.is_active() || prepared.immediate_text.is_some() {
                WorkflowPreference::Research
            } else {
                selection.fallback_reason = Some("planner_failure");
                WorkflowPreference::Reactive
            },
        );
        let workflow: Box<dyn WorkflowPlugin> = if prepared.workflow.is_active() {
            Box::new(prepared.workflow)
        } else {
            Box::new(ReactivePlugin::new(instructions))
        };
        PreparedWorkflow {
            workflow,
            usage: prepared.usage,
            immediate_text: prepared.immediate_text,
            error: prepared.error,
            planning_steps: prepared.planning_steps,
        }
    } else {
        let prepared = <ReactivePlugin as WorkflowPlugin>::prepare(ctx).await?;
        selection.effective_mode = Some(WorkflowPreference::Reactive);
        selection.prepare_outcome = "ready";
        PreparedWorkflow {
            workflow: Box::new(prepared.workflow) as Box<dyn WorkflowPlugin>,
            usage: prepared.usage,
            immediate_text: prepared.immediate_text,
            error: prepared.error,
            planning_steps: prepared.planning_steps,
        }
    };
    prepared.usage += &selection.router_usage;
    prepared.planning_steps += preflight_steps;
    selection.persist(run_dir)?;
    if let Some(effective) = selection.effective_mode {
        let mut text = format!("Workflow: {preference:?} → {effective:?}");
        if let Some(reason) = selection.fallback_reason {
            text.push_str(&format!(" (fallback: {reason})"));
        }
        let _ = events.send(AgentEvent::WorkflowSelected { text });
    }
    Ok(prepared)
}

pub(crate) enum ResponseDirective {
    UseDefaultLoop,
    Continue,
    Complete(String),
    Fail { final_text: String, error: String },
}

pub(crate) enum WorkflowOutcome {
    Complete(String),
    Fail { final_text: String, error: String },
}

impl WorkflowOutcome {
    pub fn apply(
        self,
        final_text: &mut String,
        completed: &mut bool,
        terminal_error: &mut Option<String>,
    ) {
        match self {
            Self::Complete(text) => {
                *final_text = text;
                *completed = true;
            }
            Self::Fail {
                final_text: text,
                error,
            } => {
                *final_text = text;
                *terminal_error = Some(error);
            }
        }
    }
}

pub(crate) enum TerminalCause<'a> {
    Error(&'a str),
    MaxSteps,
}

/// Temporary short-lived host context. A later workflow migration can narrow
/// this to explicit host methods without changing the loop entry points.
pub(crate) struct WorkflowIo<'a> {
    pub backend: &'a Arc<dyn Backend>,
    pub messages: &'a mut Vec<Message>,
    pub run_dir: &'a Path,
    pub run_state: &'a Arc<RunState>,
    pub recorder: &'a AgentRunRecorder,
    pub trace: &'a mut RunTraceBuilder,
    pub usage: &'a mut TokenUsage,
    pub events: &'a broadcast::Sender<AgentEvent>,
    pub max_tokens: u32,
    pub max_steps: u32,
    pub compact_after_messages: usize,
    pub keep_recent_messages: usize,
    pub anchor_user_index: &'a mut usize,
    pub is_follow_up: bool,
    pub traced_len: &'a mut usize,
}

#[async_trait]
pub(crate) trait WorkflowPlugin: Send {
    async fn prepare(ctx: WorkflowPrepareContext<'_>) -> anyhow::Result<PreparedWorkflow<Self>>
    where
        Self: Sized;

    fn is_active(&self) -> bool;
    fn system_instructions(&self) -> String;
    fn before_step(&mut self, schemas: &mut Vec<ToolSchema>);

    async fn inspect_response(
        &mut self,
        response: &LLMResponse,
        visible_texts: &[String],
        step: u32,
        io: &mut WorkflowIo<'_>,
    ) -> anyhow::Result<ResponseDirective>;

    fn decorate_tool_result(
        &mut self,
        step: u32,
        sequence: u32,
        is_last_in_step: bool,
        tool_name: &str,
        error: Option<&str>,
        history_content: &mut Vec<ToolResultContent>,
    );

    async fn on_terminal(
        &mut self,
        cause: TerminalCause<'_>,
        prior_final_text: &str,
        step: u32,
        io: &mut WorkflowIo<'_>,
    ) -> anyhow::Result<Option<WorkflowOutcome>>;
}
