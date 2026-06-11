//! Site registry — the single wiring point for site capabilities.
//!
//! Each site module exposes one `pub static <ID>_SITE: SiteSpec` and gets
//! listed in [`all_sites`]. Everything downstream (CLI subcommands, daemon
//! dispatch, TUI/desktop agent setup) is derived from the spec, so adding a
//! site never touches the CLI, daemon, or app shells.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;

use crate::agent::{Backend as LlmProvider, Tool};
use crate::cdp::PageSession;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = anyhow::Result<T>> + Send>>;

/// Async factory: build the site's agent tools against a shared page.
pub type AgentToolsFn = fn(Arc<PageSession>, Arc<dyn LlmProvider>) -> BoxFuture<Vec<Arc<dyn Tool>>>;

/// One-shot CLI/daemon command: `(page, JSON args, debug_snapshot)` → JSON.
pub type CommandRunFn = fn(Arc<PageSession>, Value, bool) -> BoxFuture<Value>;

pub struct SiteSpec {
    /// Short site id — doubles as the CLI subcommand (`socai <id> <tool>`),
    /// the daemon `site` field, and the `enabled_sites` gate value.
    pub id: &'static str,
    pub about: &'static str,
    pub home_url: &'static str,
    pub agent_tools: AgentToolsFn,
    /// Agent playbook (knowledge.md) with host-specific preamble prepended.
    pub agent_instructions: fn(&str) -> String,
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

impl SiteSpec {
    pub fn command(&self, name: &str) -> Option<&'static SiteCommand> {
        self.commands.iter().find(|cmd| cmd.name == name)
    }
}

/// Every registered site. Site order is also CLI help order.
static SITES: &[&SiteSpec] = &[&crate::sites::xhs::XHS_SITE];

pub fn all_sites() -> &'static [&'static SiteSpec] {
    SITES
}

pub fn find_site(id: &str) -> Option<&'static SiteSpec> {
    all_sites().iter().copied().find(|site| site.id == id)
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
