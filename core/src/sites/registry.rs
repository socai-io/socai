//! Native site-adapter registry.
//!
//! An adapter binds already-compiled Rust tools to CLI, daemon, and app hosts.
//! It is deliberately not a capability manifest: discovery, domains, notes,
//! and browser-tool schemas belong to runtime-loaded site learning packages in
//! [`crate::sites::learning`].

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;

use crate::agent::tool::ToolProgressSender;
use crate::agent::{Backend as LlmProvider, Tool};
use crate::cdp::PageSession;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = anyhow::Result<T>> + Send>>;

/// Async factory: build the site's agent tools against a shared page.
pub type AgentToolsFn = fn(Arc<PageSession>, Arc<dyn LlmProvider>) -> BoxFuture<Vec<Arc<dyn Tool>>>;
pub type AgentInstructionsFn = fn(&str) -> String;

/// One-shot CLI/daemon command: `(page, JSON args, debug_snapshot, progress)` → JSON.
pub type CommandRunFn =
    fn(Arc<PageSession>, Value, bool, Option<ToolProgressSender>) -> BoxFuture<Value>;

pub type RuntimeCommandRunFn = fn(
    crate::runtime::SocaiRuntime,
    &'static str,
    Value,
    bool,
    Option<ToolProgressSender>,
) -> BoxFuture<Value>;

pub struct NativeSiteAdapter {
    /// Optional lazy-browser command dispatch; HTTP-only work need not open Chrome.
    pub runtime_command: Option<RuntimeCommandRunFn>,
    /// Short site id — doubles as the CLI subcommand (`socai <id> <tool>`),
    /// the daemon `site` field, and the `enabled_sites` gate value.
    pub id: &'static str,
    pub about: &'static str,
    pub home_url: &'static str,
    pub agent_tools: AgentToolsFn,
    /// Optional default tool surface for normal app/TUI agents. Sites can keep
    /// a broader command/debug surface in `agent_tools` while exposing a
    /// smaller, product-safe macro surface to interactive users.
    pub default_agent_tools: Option<AgentToolsFn>,
    /// Compose the package's optional notes with the host-specific preamble.
    /// The note resource remains manifest-declared even when it is empty.
    pub agent_instructions: AgentInstructionsFn,
    /// Optional default playbook that matches `default_agent_tools`.
    pub default_agent_instructions: Option<AgentInstructionsFn>,
    pub commands: &'static [SiteCommand],
}

/// A one-shot command exposed both as a CLI subcommand and a daemon command.
pub struct SiteCommand {
    pub name: &'static str,
    /// Underlying tool name, reported in telemetry (may differ from `name`).
    pub tool_name: &'static str,
    pub about: &'static str,
    pub args: &'static [CommandArg],
    /// Whether the client should budget the long command timeout.
    pub slow: SlowWhen,
    pub run: CommandRunFn,
}

/// Declarative CLI argument. The CLI builds clap args from these and collects
/// matches into the JSON `args` object sent to the daemon, keyed by `key`.
pub struct CommandArg {
    /// JSON key in the command args object.
    pub key: &'static str,
    /// CLI flag name (`--<long>`); `None` makes this a positional argument.
    pub long: Option<&'static str>,
    pub value_name: &'static str,
    pub help: &'static str,
    pub required: bool,
    pub kind: ArgKind,
}

pub enum ArgKind {
    Str,
    /// Repeatable string flag collected into a JSON array, preserving order.
    StrList,
    Int,
    /// Boolean `--flag`; sent as `true` only when set.
    Flag,
    /// Repeatable `key=value` flag collected into a JSON object.
    KeyValueMap,
}

pub enum SlowWhen {
    Never,
    Always,
    /// Slow only when the named arg was provided (e.g. deep scrolling).
    ArgPresent(&'static str),
}

impl SlowWhen {
    pub fn applies(&self, args: &Value) -> bool {
        match self {
            SlowWhen::Never => false,
            SlowWhen::Always => true,
            SlowWhen::ArgPresent(key) => args.get(key).is_some_and(|value| !value.is_null()),
        }
    }
}

impl NativeSiteAdapter {
    pub fn command(&self, name: &str) -> Option<&'static SiteCommand> {
        self.commands.iter().find(|cmd| cmd.name == name)
    }
}

/// Compatibility name used by external workers built before the native
/// adapter registry was renamed.
pub type SiteSpec = NativeSiteAdapter;

/// Every registered site. Site order is also CLI help order.
static NATIVE_SITE_ADAPTERS: &[&NativeSiteAdapter] = &[
    &crate::sites::xhs::XHS_NATIVE_ADAPTER,
    &crate::sites::dy::DY_NATIVE_ADAPTER,
    &crate::sites::tiktok::TIKTOK_NATIVE_ADAPTER,
    &crate::sites::instagram::INSTAGRAM_NATIVE_ADAPTER,
    &crate::sites::linkedin::LINKEDIN_NATIVE_ADAPTER,
    &crate::sites::x::X_NATIVE_ADAPTER,
    &crate::sites::weixin::WEIXIN_NATIVE_ADAPTER,
    &crate::sites::podcast::PODCAST_NATIVE_ADAPTER,
    &crate::sites::web::WEB_SITE,
];

pub fn all_native_site_adapters() -> &'static [&'static NativeSiteAdapter] {
    NATIVE_SITE_ADAPTERS
}

pub fn find_native_site_adapter(id: &str) -> Option<&'static NativeSiteAdapter> {
    all_native_site_adapters()
        .iter()
        .copied()
        .find(|site| site.id == id)
}

/// Backward-compatible lookup for independently deployed agent workers.
pub fn find_site(id: &str) -> Option<&'static SiteSpec> {
    find_native_site_adapter(id)
}

/// Extract a required non-empty string arg from a command args object.
pub fn required_string(args: &Value, key: &str) -> anyhow::Result<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("missing required argument: {key}"))
}
