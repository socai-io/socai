//! The current ReAct workflow: no planner, coverage gate, or prompt changes.

use crate::agent::workflow::WorkflowPlugin;

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

impl WorkflowPlugin for ReactivePlugin {
    fn system_instructions(&self) -> &str {
        &self.instructions
    }
}
