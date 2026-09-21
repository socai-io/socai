//! Task-independent lifecycle hooks. Policies own their state, prompts and
//! completion semantics; the loop only dispatches these lifecycle events.

use super::tool::{ToolContext, ToolResult};

#[derive(Clone, Copy)]
pub enum RequestPhase {
    Action { step: u32, max_steps: u32 },
    Summary,
    LocalDelivery,
}

pub enum FinishDecision {
    Allow,
    Continue(String),
    Partial(String),
}

#[derive(Default)]
pub struct ToolFeedback {
    pub notes: Vec<String>,
    pub stop_reason: Option<String>,
}

/// Hooks are instantiated per run. A dormant policy should return no context,
/// no feedback, and Allow, so unrelated tasks retain their existing behavior.
pub trait AgentExtension: Send + Sync {
    fn execution_ceiling(&self, _ctx: &ToolContext, configured: u32) -> u32 {
        configured
    }
    fn request_context(&self, _ctx: &ToolContext, _phase: RequestPhase) -> Option<String> {
        None
    }
    fn before_tool(&mut self, _ctx: &ToolContext, _name: &str) {}
    fn after_tool(
        &mut self,
        _ctx: &ToolContext,
        _name: &str,
        _result: &ToolResult,
        _error: Option<&str>,
    ) -> ToolFeedback {
        ToolFeedback::default()
    }
    fn before_finish(&mut self, _ctx: &ToolContext, _steps_remaining: u32) -> FinishDecision {
        FinishDecision::Allow
    }
    fn execution_limit_reason(&self, _ctx: &ToolContext) -> Option<String> {
        None
    }
    fn partial_reason(&self, _ctx: &ToolContext) -> Option<String> {
        None
    }
}

pub struct RunExtensions(Vec<Box<dyn AgentExtension>>);

impl Default for RunExtensions {
    fn default() -> Self {
        // Composition belongs here, never in the generic execution loop.
        Self(vec![
            Box::new(super::research::ResearchExtension::default()),
        ])
    }
}

impl RunExtensions {
    pub fn execution_ceiling(&self, ctx: &ToolContext, configured: u32) -> u32 {
        self.0.iter().fold(configured, |limit, extension| {
            extension.execution_ceiling(ctx, limit)
        })
    }
    pub fn request_context(&self, ctx: &ToolContext, phase: RequestPhase) -> Vec<String> {
        self.0
            .iter()
            .filter_map(|extension| extension.request_context(ctx, phase))
            .collect()
    }

    pub fn before_tool(&mut self, ctx: &ToolContext, name: &str) {
        for extension in &mut self.0 {
            extension.before_tool(ctx, name);
        }
    }

    pub fn after_tool(
        &mut self,
        ctx: &ToolContext,
        name: &str,
        result: &ToolResult,
        error: Option<&str>,
    ) -> ToolFeedback {
        let mut feedback = ToolFeedback::default();
        for extension in &mut self.0 {
            let next = extension.after_tool(ctx, name, result, error);
            feedback.notes.extend(next.notes);
            feedback.stop_reason = feedback.stop_reason.or(next.stop_reason);
        }
        feedback
    }

    pub fn before_finish(&mut self, ctx: &ToolContext, steps_remaining: u32) -> FinishDecision {
        for extension in &mut self.0 {
            match extension.before_finish(ctx, steps_remaining) {
                FinishDecision::Allow => {}
                decision => return decision,
            }
        }
        FinishDecision::Allow
    }

    pub fn execution_limit_reason(&self, ctx: &ToolContext) -> Option<String> {
        self.0
            .iter()
            .find_map(|extension| extension.execution_limit_reason(ctx))
    }

    pub fn partial_reason(&self, ctx: &ToolContext) -> Option<String> {
        self.0
            .iter()
            .find_map(|extension| extension.partial_reason(ctx))
    }
}
