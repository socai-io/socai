mod engine;

pub use self::engine::{
    create_llm_provider, ensure_llm_provider_configured, resolve_llm_model, run_agent_task,
    wait_browser_connected, wait_browser_connected_with_options, AgentRunConfig, SocaiRuntime,
};
pub use crate::cdp::{
    BrowserEvent as RuntimeBrowserEvent, ChromeConnectOptions, ChromeProfile,
    PageSession as RuntimePageSession, StatusPayload as BrowserStatus,
    TargetInfo as BrowserTargetInfo,
};
