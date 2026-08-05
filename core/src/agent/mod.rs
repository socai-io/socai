//! Agent runtime: LLM clients, the Tool trait, the agent loop, and
//! run-state persistence.
//!
//! Browser/CDP is intentionally out of scope — site tools live in the
//! `sites` module and call into `cdp` themselves.

pub mod api_errors;
pub mod compaction;
pub mod conversation;
pub mod file_bash_tools;
pub mod llm;
pub mod r#loop;
pub mod memory;
pub mod note_store;
pub mod provider;
pub mod report;
pub mod run_logging;
pub mod run_state;
pub mod signature;
pub mod system_prompt;
pub mod tool;

pub use self::file_bash_tools::{
    desktop_agent_tools, local_agent_tools, BashTool, ReadFileTool, ShellTool,
};
pub use self::llm::{
    AnthropicBackend, Backend, Block, LLMResponse, Message, MessageContent, MessageRole,
    OpenAICompatBackend, StopReason, TokenUsage, ToolCall, ToolResultContent, ToolSchema, UsageCost,
};
pub use self::provider::{
    catalog_model_display_name, catalog_models_for, config_for, configured_default_model_for,
    configured_default_provider, default_model_for, list_available_providers, load_api_key,
    load_openai_credential, load_provider_credential, provider_credential_kind, resolve_provider,
    save_api_key, save_default_model, Credential, CredentialKind, ModelCatalogEntry, ModelPricing,
    ModelPricingTier, Provider, ProviderConfig, PROVIDERS,
};
pub use self::conversation::{default_sessions_root, Conversation, Run};
pub use self::r#loop::{run_agent, run_agent_with_events, AgentEvent, AgentOptions, AgentOutcome};
pub use self::run_logging::{
    default_runs_root, make_run_dir, mark_agent_run_status, AgentRunRecorder, ToolCallRecorder,
};
pub use self::run_state::{ArtifactRecord, RunState};
pub use self::tool::{
    EchoTool, ProcessedNote, SharedTool, Tool, ToolContext, ToolResult, ToolResultBlock,
};
