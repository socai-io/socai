//! The unmodified ReAct behavior behind the workflow lifecycle contract.

use async_trait::async_trait;

use crate::agent::llm::{LLMResponse, ToolResultContent, ToolSchema};
use crate::agent::workflow::{
    PreparedWorkflow, ResponseDirective, TerminalCause, WorkflowIo, WorkflowOutcome,
    WorkflowPlugin, WorkflowPrepareContext,
};

pub(crate) struct ReactivePlugin {
    instructions: String,
}

impl ReactivePlugin {
    pub(crate) fn new(instructions: impl Into<String>) -> Self {
        Self {
            instructions: instructions.into(),
        }
    }
}

#[async_trait]
impl WorkflowPlugin for ReactivePlugin {
    async fn prepare(ctx: WorkflowPrepareContext<'_>) -> anyhow::Result<PreparedWorkflow<Self>> {
        Ok(PreparedWorkflow {
            workflow: Self::new(ctx.extra_instructions),
            usage: Default::default(),
            immediate_text: None,
            error: None,
            planning_steps: 0,
        })
    }

    fn is_active(&self) -> bool {
        false
    }

    fn system_instructions(&self) -> String {
        self.instructions.clone()
    }

    fn before_step(&mut self, _schemas: &mut Vec<ToolSchema>) {}

    async fn inspect_response(
        &mut self,
        _response: &LLMResponse,
        _visible_texts: &[String],
        _step: u32,
        _io: &mut WorkflowIo<'_>,
    ) -> anyhow::Result<ResponseDirective> {
        Ok(ResponseDirective::UseDefaultLoop)
    }

    fn decorate_tool_result(
        &mut self,
        _step: u32,
        _sequence: u32,
        _is_last_in_step: bool,
        _tool_name: &str,
        _error: Option<&str>,
        _history_content: &mut Vec<ToolResultContent>,
    ) {
    }

    async fn on_terminal(
        &mut self,
        _cause: TerminalCause<'_>,
        _prior_final_text: &str,
        _step: u32,
        _io: &mut WorkflowIo<'_>,
    ) -> anyhow::Result<Option<WorkflowOutcome>> {
        Ok(None)
    }
}
