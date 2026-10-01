//! Workflow hooks around the single core-owned agent loop.
//!
//! Hooks return no control directives until another workflow needs them.

use async_trait::async_trait;

use crate::agent::llm::{LLMResponse, ToolResultContent, ToolSchema};

pub(crate) enum TerminalCause {
    ModelError,
    Truncated,
    MaxSteps,
}

#[async_trait]
pub(crate) trait WorkflowPlugin: Send {
    async fn prepare(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn system_instructions(&self) -> &str;

    fn before_step(&mut self, _schemas: &mut Vec<ToolSchema>) {}

    async fn inspect_response(&mut self, _response: &LLMResponse) -> anyhow::Result<()> {
        Ok(())
    }

    fn decorate_tool_result(
        &mut self,
        _step: u32,
        _sequence: u32,
        _is_last_in_step: bool,
        _tool_name: &str,
        _error: Option<&str>,
        _history_content: &mut Vec<ToolResultContent>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn on_terminal(&mut self, _cause: TerminalCause) -> anyhow::Result<()> {
        Ok(())
    }
}
