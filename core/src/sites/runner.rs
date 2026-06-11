//! Shared one-shot command runner for site commands.
//!
//! Every site command (CLI `socai <site> <tool>` / daemon dispatch) needs the
//! same scaffolding: allocate a run dir, persist the invocation, wrap the
//! whole command in optional snapshot recording, invoke one agent tool by
//! name, and return the `{command, run_dir, input, data}` envelope. Site
//! modules supply only the tool set and optional page-state hooks — do not
//! copy this scaffolding into a site folder.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{make_run_dir, Tool, ToolContext, ToolResult};
use crate::cdp::{with_snapshot_recording, PageSession};
use crate::sites::registry::BoxFuture;

/// Page-state fixup run inside the snapshot recording, before or after the
/// tool call (e.g. close a leftover modal, make sure search is reachable).
pub type PageHook = Box<dyn FnOnce(Arc<PageSession>) -> BoxFuture<()> + Send>;

/// One site command invocation, described by names plus optional hooks.
pub struct ToolCommand<'a> {
    pub site_id: &'a str,
    /// CLI/daemon command name; doubles as the run-dir label.
    pub command_name: &'a str,
    /// Agent tool to invoke (may differ from the command name).
    pub tool_name: &'a str,
    pub before: Option<PageHook>,
    pub after: Option<PageHook>,
}

pub async fn run_tool_command(
    cmd: ToolCommand<'_>,
    page: Arc<PageSession>,
    tools: &[Arc<dyn Tool>],
    input: Value,
    debug_snapshot: bool,
) -> anyhow::Result<Value> {
    // Run-dir label: site + command, plus the query when the command has one,
    // so runs are tellable apart at a glance (…_xhs_search_notes_咖啡).
    let mut label = format!("{}_{}", cmd.site_id, cmd.command_name);
    if let Some(query) = input.get("query").and_then(Value::as_str) {
        let query = query.trim();
        if !query.is_empty() {
            label.push('_');
            label.push_str(query);
        }
    }
    let (run_dir, ctx) = command_context_for_label(cmd.site_id, &label);
    // Persist the full command input up front (best-effort) so a run is
    // debuggable from its dir alone — including the exact args — even when the
    // tool errors out partway.
    let invocation = json!({
        "command": cmd.command_name,
        "tool": cmd.tool_name,
        "input": input.clone(),
    });
    let _ = std::fs::create_dir_all(&ctx.run_dir);
    if let Ok(bytes) = serde_json::to_vec_pretty(&invocation) {
        let _ = std::fs::write(ctx.run_dir.join("command_input.json"), bytes);
    }
    // Snapshot recording (when `--debug-snapshot` is on) wraps the whole
    // command — setup navigation, the tool's clicks/scrolls, and teardown. All
    // recorder machinery lives in the generic CDP layer; this is the only hook
    // a site command runner needs.
    let data = with_snapshot_recording(&page, &ctx.run_dir, debug_snapshot, async {
        if let Some(hook) = cmd.before {
            hook(page.clone()).await?;
        }
        let data = call_site_tool(tools, cmd.tool_name, input, &ctx).await?;
        if let Some(hook) = cmd.after {
            hook(page.clone()).await?;
        }
        Ok::<Value, anyhow::Error>(data)
    })
    .await?;

    Ok(json!({
        "command": cmd.command_name,
        "run_dir": run_dir,
        "input": invocation.get("input").cloned().unwrap_or(Value::Null),
        "data": data,
    }))
}

/// Invoke one tool by name and parse its text reply as JSON (falling back to
/// `{"raw_reply": …}` for non-JSON output).
pub async fn call_site_tool(
    tools: &[Arc<dyn Tool>],
    tool_name: &str,
    input: Value,
    ctx: &ToolContext,
) -> anyhow::Result<Value> {
    let tool = tools
        .iter()
        .find(|tool| tool.name() == tool_name)
        .ok_or_else(|| anyhow::anyhow!("site tool not found: {tool_name}"))?;
    let result = tool.call(input, ctx).await?;
    let text = result.flat_text();
    serde_json::from_str(text.trim()).or_else(|_| Ok(json!({ "raw_reply": text })))
}

/// Allocate a run dir for a one-shot command and build its ToolContext with
/// the site enabled (so site-gated tools resolve). The site id is part of the
/// run-dir name (`<ts>_<site>_<command>[_<query>]`).
pub fn command_context(site_id: &str, label: &str) -> (String, ToolContext) {
    command_context_for_label(site_id, &format!("{site_id}_{label}"))
}

fn command_context_for_label(site_id: &str, label: &str) -> (String, ToolContext) {
    let run_dir = make_run_dir(label);
    let run_id = run_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(label)
        .to_string();
    let ctx = ToolContext::new(run_id, run_dir.clone());
    ctx.enable_site(site_id);
    (run_dir.display().to_string(), ctx)
}

// ── Tool-input helpers shared by site tool implementations ─────────────────

pub fn trimmed_required(value: &str, label: &str) -> anyhow::Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("{label} is empty");
    }
    Ok(trimmed.to_string())
}

pub fn insert_optional_str(input: &mut Value, key: &str, value: Option<&str>) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    if let Some(obj) = input.as_object_mut() {
        obj.insert(key.to_string(), Value::String(value.to_string()));
    }
}

pub fn json_result(value: &Value) -> ToolResult {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    ToolResult::text(text)
}

pub fn get_f64(input: &Value, key: &str, default: f64) -> f64 {
    input.get(key).and_then(Value::as_f64).unwrap_or(default)
}

pub fn get_i64(input: &Value, key: &str, default: i64) -> i64 {
    input.get(key).and_then(Value::as_i64).unwrap_or(default)
}

pub fn get_str<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str)
}

pub fn get_bool(input: &Value, key: &str, default: bool) -> bool {
    input.get(key).and_then(Value::as_bool).unwrap_or(default)
}
