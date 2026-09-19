//! The small lifecycle contract shared by agent workflows.
//!
//! The core loop still owns model calls, tools, messages, and persistence.
//! Workflow-specific decisions are made at these boundaries.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast;
use tracing::warn;

use crate::agent::llm::{Backend, LLMResponse, Message, TokenUsage, ToolResultContent, ToolSchema};
use crate::agent::r#loop::AgentEvent;
use crate::agent::reactive_workflow::ReactivePlugin;
use crate::agent::research_workflow::ResearchWorkflow;
use crate::agent::run_logging::AgentRunRecorder;
use crate::agent::run_state::RunState;
use crate::telemetry::trace::RunTraceBuilder;

pub(crate) struct WorkflowPrepareContext<'a> {
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
}

pub(crate) struct PreparedWorkflow<W> {
    pub workflow: W,
    pub usage: TokenUsage,
    pub immediate_text: Option<String>,
    pub planning_steps: u32,
}

/// Research is opt-in until routing is introduced; the existing ReAct path is
/// the default. A failed Research planner retains its usage and step count
/// while continuing with the no-op ReactivePlugin.
pub(crate) async fn prepare_selected_workflow(
    ctx: WorkflowPrepareContext<'_>,
) -> anyhow::Result<PreparedWorkflow<Box<dyn WorkflowPlugin>>> {
    let requested = std::env::var("SOCAI_WORKFLOW").unwrap_or_default();
    if requested == "research" {
        let instructions = ctx.extra_instructions.to_string();
        let prepared = <ResearchWorkflow as WorkflowPlugin>::prepare(ctx).await?;
        let workflow: Box<dyn WorkflowPlugin> = if prepared.workflow.is_active() {
            Box::new(prepared.workflow)
        } else {
            Box::new(ReactivePlugin::new(instructions))
        };
        return Ok(PreparedWorkflow {
            workflow,
            usage: prepared.usage,
            immediate_text: prepared.immediate_text,
            planning_steps: prepared.planning_steps,
        });
    }
    if !requested.is_empty() && requested != "reactive" {
        warn!(%requested, "unknown workflow; using reactive");
    }
    let prepared = <ReactivePlugin as WorkflowPlugin>::prepare(ctx).await?;
    Ok(PreparedWorkflow {
        workflow: Box::new(prepared.workflow),
        usage: prepared.usage,
        immediate_text: prepared.immediate_text,
        planning_steps: prepared.planning_steps,
    })
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
