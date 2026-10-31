//! Isolated, session-affine Agent worker built on the public `socai-core` loop.

#![forbid(unsafe_code)]

pub mod runner;
mod input_files;
mod runtime;

pub use runtime::{run_stdio, run_stdio_with_build_info};

pub const WORKER_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const SOCAI_CORE_VERSION: &str = "0.6.7";
pub const SOCAI_CORE_REVISION: &str = "workspace";
pub const WORKER_CAPABILITIES: &[&str] = &[
    "agent_events.v1",
    "bounded_frames.v1",
    "browser_session_recovery.v1",
    "evidence_records.v1",
    "external_write_grants.v1",
    "interactive_login_resume.v1",
    "kernel_cdp_binding.v1",
    "host_tools.v1",
    "input_files.v1",
    "remote_ocr.v1",
    "multi_site_namespaces.v1",
    "parallel_platform_tools.v1",
    "proactive_browser_takeover.v1",
    "research_experts.v1",
    "account_memory.v1",
    "persistent_conversation.v1",
    "run_sequence_resume.v1",
];

#[derive(Debug, Clone)]
pub struct WorkerBuildInfo {
    pub worker_version: String,
    pub socai_core_version: String,
    pub socai_core_revision: String,
    pub capabilities: Vec<String>,
}

impl Default for WorkerBuildInfo {
    fn default() -> Self {
        Self {
            worker_version: WORKER_VERSION.into(),
            socai_core_version: SOCAI_CORE_VERSION.into(),
            socai_core_revision: SOCAI_CORE_REVISION.into(),
            capabilities: WORKER_CAPABILITIES
                .iter()
                .map(|value| (*value).into())
                .collect(),
        }
    }
}
