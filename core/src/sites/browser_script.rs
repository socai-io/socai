//! Agent-driven browser JavaScript and durable local tool overrides.
//!
//! Site tools remain the preferred path. When a browser-backed tool stops
//! matching the live page, `browser_script` lets the agent inspect and repair
//! that exact tab with arbitrary JavaScript. A verified script
//! can be saved under `~/.socai/tool-overrides/<site>/<tool>/` and reused on
//! later calls. An override validated by an older socai build is retained as a
//! fallback, but the upgraded built-in tool gets one canary call first. JavaScript
//! runs in Chrome, never in a host shell, so it has the current page's authority
//! but no native file system or process APIs.

use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::agent::file_bash_tools::socai_home_dir;
use crate::agent::{SharedTool, Tool, ToolContext, ToolResult, ToolResultBlock};
use crate::cdp::PageSession;
use crate::sites::runner::json_result;

pub const BROWSER_SCRIPT_TOOL_NAME: &str = "browser_script";

const DEFAULT_TIMEOUT_MS: u64 = 15_000;
const MAX_TIMEOUT_MS: u64 = 120_000;
const MAX_SCRIPT_BYTES: usize = 512 * 1024;
const MAX_RESULT_BYTES: usize = 4 * 1024 * 1024;
const MAX_BRIDGE_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_SCRIPT_ERROR_CHARS: usize = 4096;
const OVERRIDE_MANIFEST_VERSION: u32 = 3;
const LEGACY_OVERRIDE_MANIFEST_VERSION: u32 = 2;
const LEGACY_OVERRIDE_CONTRACT_VERSION: u32 = 1;

static OVERRIDE_LIFECYCLE_LOCK: OnceLock<Arc<tokio::sync::Mutex<()>>> = OnceLock::new();

fn override_lifecycle_lock() -> Arc<tokio::sync::Mutex<()>> {
    OVERRIDE_LIFECYCLE_LOCK
        .get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

const BROWSER_SCRIPT_BRIDGE_JS: &str = r#"
(() => {
  let nextId = 1;
  const queue = [];
  const pending = new Map();
  const stringify = JSON.stringify.bind(JSON);
  const encode = TextEncoder.prototype.encode.bind(new TextEncoder());
  const functionToString = Function.prototype.toString;
  const normalizePageScript = (script, caller) => {
    if (typeof script === 'string' && script.trim()) return script;
    if (typeof script === 'function') {
      const source = functionToString.call(script);
      if (!source || source.includes('[native code]')) {
        throw new Error(`${caller} received a function that cannot run in the target page`);
      }
      return `return await (${source})(input);`;
    }
    throw new Error(`${caller} expects a non-empty JavaScript string or function`);
  };
  const request = (op, args = {}) => new Promise((resolve, reject) => {
    let serialized;
    try {
      serialized = stringify(args);
    } catch (_) {
      reject(new Error('browser-script bridge arguments must be JSON-serializable'));
      return;
    }
    if (encode(serialized).byteLength > 1048576) {
      reject(new Error('browser-script bridge arguments exceed 1048576 bytes'));
      return;
    }
    const id = nextId++;
    pending.set(id, { resolve, reject });
    queue.push({ id, op, args });
  });
  const api = {
    evaluate: (script, input = {}) => request('evaluate', {
      script: normalizePageScript(script, 'socai.evaluate'),
      input,
    }),
    pageInfo: () => request('page_info'),
    navigate: (url) => request('navigate', { url }),
    click: (selector) => request('click', { selector }),
    type: (selector, text) => request('type', { selector, text }),
    press: (key) => request('press', { key }),
    scroll: (deltaY) => request('scroll', { delta_y: deltaY }),
    wait: (milliseconds) => request('wait', { milliseconds }),
    waitFor: async (script, options = {}) => {
      const normalizedScript = normalizePageScript(script, 'socai.waitFor');
      const timeout = Math.max(100, Math.min(Number(options.timeout_ms || 10000), 30000));
      const interval = Math.max(40, Math.min(Number(options.interval_ms || 250), 2000));
      const started = Date.now();
      let lastValue = null;
      while (Date.now() - started < timeout) {
        lastValue = await request('evaluate', { script: normalizedScript, input: options.input || {} });
        if (lastValue) return lastValue;
        await request('wait', { milliseconds: interval });
      }
      throw new Error(`socai.waitFor timed out after ${timeout}ms; last value: ${JSON.stringify(lastValue)}`);
    },
  };
  window.__socaiBridge = {
    api: Object.freeze(api),
    take: () => queue.shift() || null,
    settle: (id, ok, value) => {
      const entry = pending.get(id);
      if (!entry) return false;
      pending.delete(id);
      if (ok) entry.resolve(value);
      else entry.reject(new Error(String(value).slice(0, 4096)));
      return true;
    },
  };
  return true;
})()
"#;

const NON_REPAIRABLE_REASONS: &[&str] = &[
    "login_required",
    "rate_limited",
    "security_verification",
    "captcha_required",
    "permission_denied",
    "user_action_required",
];

const NON_OVERRIDABLE_TOOLS: &[&str] = &[
    BROWSER_SCRIPT_TOOL_NAME,
    "wait_for_login",
    "wait_for_rate_limit",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ActiveOverride {
    version: u32,
    site: String,
    tool: String,
    script: String,
    sha256: String,
    source_version: String,
    created_at: String,
    source_url: String,
    timeout_ms: u64,
    #[serde(default)]
    validated_with_version: String,
    #[serde(default = "default_override_contract_version")]
    tool_contract_version: u32,
    #[serde(default)]
    tool_impl_revision: u32,
    #[serde(default)]
    last_validated_at: String,
    #[serde(default)]
    status_changed_at: String,
    #[serde(default)]
    status: OverrideStatus,
    #[serde(default)]
    status_reason: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OverrideStatus {
    #[default]
    Active,
    Stale,
    Quarantined,
}

#[derive(Clone)]
struct LocalToolOverrideRegistry {
    root: PathBuf,
    lifecycle_lock: Arc<tokio::sync::Mutex<()>>,
}

impl LocalToolOverrideRegistry {
    fn open_default() -> Self {
        Self {
            root: socai_home_dir().join("tool-overrides"),
            lifecycle_lock: override_lifecycle_lock(),
        }
    }

    fn tool_dir(&self, site: &str, tool: &str) -> Result<PathBuf> {
        validate_segment("site", site)?;
        validate_segment("tool", tool)?;
        Ok(self.root.join(site).join(tool))
    }

    fn ensure_tool_dir(&self, site: &str, tool: &str) -> Result<PathBuf> {
        let site_dir = self.root.join(site);
        let tool_dir = self.tool_dir(site, tool)?;
        for dir in [&self.root, &site_dir, &tool_dir] {
            create_private_dir_all(dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        Ok(tool_dir)
    }

    fn active_script(
        &self,
        site: &str,
        tool: &str,
    ) -> Result<Option<(ActiveOverride, PathBuf, PathBuf, String)>> {
        let dir = self.tool_dir(site, tool)?;
        let manifest_path = match newest_override_manifest(&dir)? {
            Some(path) => path,
            None => return Ok(None),
        };
        let text = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed to read {}", manifest_path.display()))?;
        let mut manifest: ActiveOverride = serde_json::from_str(&text)
            .with_context(|| format!("invalid local override {}", manifest_path.display()))?;
        if !matches!(
            manifest.version,
            LEGACY_OVERRIDE_MANIFEST_VERSION | OVERRIDE_MANIFEST_VERSION
        ) || manifest.site != site
            || manifest.tool != tool
        {
            anyhow::bail!(
                "local override metadata does not match {site}.{tool}: {}",
                manifest_path.display()
            );
        }
        if manifest.validated_with_version.is_empty() {
            manifest.validated_with_version = manifest.source_version.clone();
        }
        if manifest.last_validated_at.is_empty() {
            manifest.last_validated_at = manifest.created_at.clone();
        }
        if manifest.status_changed_at.is_empty() {
            manifest.status_changed_at = manifest.created_at.clone();
        }
        if manifest.status != OverrideStatus::Active {
            return Ok(None);
        }
        validate_script_path(&manifest.script)?;
        let script_path = dir.join(&manifest.script);
        if !script_path.is_file() {
            anyhow::bail!(
                "local override script is missing: {}",
                script_path.display()
            );
        }
        let script = std::fs::read(&script_path)
            .with_context(|| format!("failed to read {}", script_path.display()))?;
        let actual_hash = sha256_hex(&script);
        if actual_hash != manifest.sha256 {
            anyhow::bail!(
                "local override script hash mismatch: {}",
                script_path.display()
            );
        }
        let script = String::from_utf8(script)
            .with_context(|| format!("local override is not UTF-8: {}", script_path.display()))?;
        Ok(Some((manifest, manifest_path, script_path, script)))
    }

    fn write_candidate(&self, site: &str, tool: &str, script: &str) -> Result<PathBuf> {
        let dir = self.ensure_tool_dir(site, tool)?.join("versions");
        create_private_dir_all(&dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
        let timestamp = Utc::now().format("%Y%m%d-%H%M%S%.9f");
        let suffix = Uuid::new_v4().simple().to_string();
        let path = dir.join(format!("{tool}.{timestamp}.{}.js", &suffix[..8]));
        write_text_atomic(&path, script)?;
        Ok(path)
    }

    fn activate(
        &self,
        site: &str,
        tool: &str,
        candidate_path: &Path,
        source_url: &str,
        timeout_ms: u64,
    ) -> Result<(PathBuf, ActiveOverride)> {
        let dir = self.ensure_tool_dir(site, tool)?;
        let relative_script = candidate_path.strip_prefix(&dir).with_context(|| {
            format!(
                "override candidate {} is outside {}",
                candidate_path.display(),
                dir.display()
            )
        })?;
        let relative_script = relative_script
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("override script path is not UTF-8"))?;
        validate_script_path(relative_script)?;
        let script_bytes = std::fs::read(candidate_path)
            .with_context(|| format!("failed to read {}", candidate_path.display()))?;
        let now = Utc::now().to_rfc3339();
        let manifest = ActiveOverride {
            // Keep activation records readable by the previous binary. New
            // lifecycle-only fields are additive and ignored by serde there;
            // v3 state transitions use a separate `state-*` namespace.
            version: LEGACY_OVERRIDE_MANIFEST_VERSION,
            site: site.to_string(),
            tool: tool.to_string(),
            script: relative_script.to_string(),
            sha256: sha256_hex(&script_bytes),
            source_version: env!("CARGO_PKG_VERSION").to_string(),
            created_at: now.clone(),
            source_url: sanitize_source_url(source_url),
            timeout_ms,
            validated_with_version: env!("CARGO_PKG_VERSION").to_string(),
            tool_contract_version: override_contract_version(site, tool),
            tool_impl_revision: builtin_tool_revision(site, tool),
            last_validated_at: now.clone(),
            status_changed_at: now,
            status: OverrideStatus::Active,
            status_reason: "newly_activated".to_string(),
        };
        self.write_manifest_record(&manifest, "active")?;
        Ok((candidate_path.to_path_buf(), manifest))
    }

    fn transition(
        &self,
        manifest: &ActiveOverride,
        expected_manifest_path: &Path,
        status: OverrideStatus,
        reason: &str,
        revalidate: bool,
    ) -> Result<ActiveOverride> {
        let dir = self.tool_dir(&manifest.site, &manifest.tool)?;
        let current = newest_override_manifest(&dir)?
            .ok_or_else(|| anyhow::anyhow!("local override disappeared before state transition"))?;
        if current != expected_manifest_path {
            anyhow::bail!(
                "local override changed before state transition: expected {}, found {}",
                expected_manifest_path.display(),
                current.display()
            );
        }
        let mut next = manifest.clone();
        next.version = OVERRIDE_MANIFEST_VERSION;
        next.status = status;
        next.status_reason = reason.to_string();
        next.status_changed_at = Utc::now().to_rfc3339();
        if revalidate {
            next.validated_with_version = env!("CARGO_PKG_VERSION").to_string();
            next.tool_contract_version = override_contract_version(&next.site, &next.tool);
            next.tool_impl_revision = builtin_tool_revision(&next.site, &next.tool);
            next.last_validated_at = Utc::now().to_rfc3339();
        }
        self.write_manifest_record(&next, "state")?;
        Ok(next)
    }

    fn write_manifest_record(&self, manifest: &ActiveOverride, prefix: &str) -> Result<PathBuf> {
        let dir = self.ensure_tool_dir(&manifest.site, &manifest.tool)?;
        debug_assert!(matches!(prefix, "active" | "state"));
        // Each activation or state transition gets an immutable record. New
        // readers select the lexicographically newest complete active/state
        // file, so publication is one atomic rename and never exposes a
        // manifest/script mismatch.
        let timestamp = Utc::now().format("%Y%m%d-%H%M%S%.9f");
        let suffix = Uuid::new_v4().simple().to_string();
        let manifest_path = dir.join(format!("{prefix}-{timestamp}-{}.json", &suffix[..8]));
        write_json_atomic(&manifest_path, manifest)?;
        Ok(manifest_path)
    }
}

const fn default_override_contract_version() -> u32 {
    LEGACY_OVERRIDE_CONTRACT_VERSION
}

#[derive(Clone)]
struct BrowserScriptRuntime {
    site: String,
    page: Arc<PageSession>,
    registry: LocalToolOverrideRegistry,
}

/// If an agent task is cancelled while arbitrary JavaScript is still running
/// in the shared site tab, terminate that evaluation in the background. Normal
/// completion disarms the guard and leaves the site's own scripts untouched.
struct TargetEvaluationGuard {
    page: Arc<PageSession>,
    armed: bool,
}

impl TargetEvaluationGuard {
    fn new(page: Arc<PageSession>) -> Self {
        Self { page, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TargetEvaluationGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let page = self.page.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ =
                    tokio::time::timeout(Duration::from_secs(1), page.terminate_javascript()).await;
            });
        }
    }
}

impl BrowserScriptRuntime {
    fn new(site: &str, page: Arc<PageSession>) -> Result<Self> {
        validate_segment("site", site)?;
        Ok(Self {
            site: site.to_string(),
            page,
            registry: LocalToolOverrideRegistry::open_default(),
        })
    }

    async fn execute(&self, script: &str, input: &Value, timeout_ms: u64) -> Result<Value> {
        if script.trim().is_empty() {
            anyhow::bail!("browser_script requires non-empty JavaScript");
        }
        if script.len() > MAX_SCRIPT_BYTES {
            anyhow::bail!(
                "browser script is too large: {} bytes (maximum {MAX_SCRIPT_BYTES})",
                script.len()
            );
        }
        let timeout_ms = timeout_ms.clamp(100, MAX_TIMEOUT_MS);
        let runner = self
            .page
            .create_background_sibling("about:blank")
            .await
            .context("failed to create browser-script control tab")?;
        let value = self
            .execute_in_runner(&runner, script, input, timeout_ms)
            .await;
        if value.is_err() {
            let _ = runner.terminate_javascript().await;
        }
        let close = runner.close().await;
        match value {
            Ok(value) => {
                close
                    .context("browser script completed but its control tab could not be closed")?;
                Ok(value)
            }
            Err(error) => {
                if let Err(close_error) = close {
                    tracing::warn!(
                        error = %close_error,
                        "failed to close browser-script control tab after script failure"
                    );
                }
                Err(error)
            }
        }
    }

    async fn execute_in_runner(
        &self,
        runner: &PageSession,
        script: &str,
        input: &Value,
        timeout_ms: u64,
    ) -> Result<Value> {
        runner
            .evaluate_json(BROWSER_SCRIPT_BRIDGE_JS)
            .await
            .context("failed to initialize browser-script bridge")?;
        let expression = bounded_program_expression(script, input)?;
        let evaluation = runner.evaluate_json_with_timeout(
            &expression,
            Duration::from_millis(timeout_ms.saturating_add(5_000)),
        );
        tokio::pin!(evaluation);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        let mut poll = tokio::time::interval(Duration::from_millis(40));
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                value = &mut evaluation => return decode_bounded_result(value?),
                _ = tokio::time::sleep_until(deadline) => {
                    anyhow::bail!("browser script timed out after {timeout_ms}ms")
                },
                _ = poll.tick() => {
                    let request = tokio::time::timeout_at(
                        deadline,
                        runner.evaluate_json("window.__socaiBridge.take()"),
                    )
                        .await
                        .map_err(|_| anyhow::anyhow!("browser script timed out after {timeout_ms}ms"))?
                        .context("failed to poll browser-script bridge")?;
                    if request.is_null() {
                        continue;
                    }
                    let request_bytes = serde_json::to_vec(&request)?.len();
                    if request_bytes > MAX_BRIDGE_REQUEST_BYTES {
                        anyhow::bail!(
                            "browser-script bridge request is too large: {request_bytes} bytes"
                        );
                    }
                    let id = request
                        .get("id")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| anyhow::anyhow!("browser-script bridge request has no id"))?;
                    let response = match tokio::time::timeout_at(
                        deadline,
                        self.handle_bridge_request(&request, deadline),
                    )
                    .await
                    {
                        Ok(response) => response,
                        Err(_) => anyhow::bail!("browser script timed out after {timeout_ms}ms"),
                    };
                    tokio::time::timeout_at(
                        deadline,
                        settle_bridge_request(runner, id, response),
                    )
                    .await
                    .map_err(|_| anyhow::anyhow!("browser script timed out after {timeout_ms}ms"))??;
                }
            }
        }
    }

    async fn handle_bridge_request(
        &self,
        request: &Value,
        deadline: tokio::time::Instant,
    ) -> Result<Value> {
        let operation = request
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("browser-script bridge request has no operation"))?;
        let args = request.get("args").cloned().unwrap_or_else(|| json!({}));
        match operation {
            "evaluate" => {
                let script = required_arg(&args, "script")?;
                let input = args.get("input").cloned().unwrap_or_else(|| json!({}));
                let remaining = deadline
                    .saturating_duration_since(tokio::time::Instant::now())
                    .saturating_add(Duration::from_secs(1));
                self.evaluate_target(script, &input, remaining).await
            }
            "page_info" => self.page.page_info().await,
            "navigate" => {
                let url = required_arg(&args, "url")?;
                self.page.navigate(url).await?;
                self.page.page_info().await
            }
            "click" => {
                let selector = required_arg(&args, "selector")?;
                let target = self.locate_target(selector, true, None).await?;
                let x = target
                    .get("x")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| anyhow::anyhow!("click target has no x coordinate"))?;
                let y = target
                    .get("y")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| anyhow::anyhow!("click target has no y coordinate"))?;
                self.page.click(x, y).await?;
                Ok(target)
            }
            "type" => {
                let selector = required_arg(&args, "selector")?;
                let text = args.get("text").and_then(Value::as_str).unwrap_or_default();
                let marker = Uuid::new_v4().simple().to_string();
                let target = self.locate_target(selector, true, Some(&marker)).await?;
                let x = target
                    .get("x")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| anyhow::anyhow!("type target has no x coordinate"))?;
                let y = target
                    .get("y")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| anyhow::anyhow!("type target has no y coordinate"))?;
                self.page.click(x, y).await?;
                self.clear_marked_target(&marker).await?;
                self.page.type_chars(text).await?;
                Ok(json!({ "ok": true, "selector": selector, "text_length": text.chars().count() }))
            }
            "press" => {
                let key = required_arg(&args, "key")?;
                self.page.press_key(key).await?;
                Ok(json!({ "ok": true, "key": key }))
            }
            "scroll" => {
                let delta_y = args.get("delta_y").and_then(Value::as_i64).unwrap_or(0);
                self.page.scroll(delta_y).await?;
                self.page.page_info().await
            }
            "wait" => {
                let milliseconds = args
                    .get("milliseconds")
                    .and_then(Value::as_u64)
                    .unwrap_or(250)
                    .min(30_000);
                tokio::time::sleep(Duration::from_millis(milliseconds)).await;
                Ok(json!({ "ok": true, "waited_ms": milliseconds }))
            }
            other => anyhow::bail!("unsupported browser-script operation: {other}"),
        }
    }

    async fn evaluate_target(
        &self,
        script: &str,
        input: &Value,
        timeout: Duration,
    ) -> Result<Value> {
        if script.len() > MAX_SCRIPT_BYTES {
            anyhow::bail!("nested page script is too large");
        }
        let expression = bounded_page_expression(script, input)?;
        let mut cancellation = TargetEvaluationGuard::new(self.page.clone());
        let value = self
            .page
            .evaluate_json_in_isolated_world_with_timeout(&expression, timeout)
            .await;
        let timed_out = value.as_ref().err().is_some_and(|error| {
            format!("{error:#}")
                .to_ascii_lowercase()
                .contains("timed out")
        });
        if !timed_out {
            cancellation.disarm();
        }
        let value = value?;
        decode_bounded_result(value)
    }

    async fn locate_target(
        &self,
        selector: &str,
        scroll: bool,
        marker: Option<&str>,
    ) -> Result<Value> {
        let selector_json = serde_json::to_string(selector)?;
        let marker_json = serde_json::to_string(&marker)?;
        let script = format!(
            r#"
const selector = {selector_json};
const marker = {marker_json};
const elements = Array.from(document.querySelectorAll(selector));
const element = elements.find((candidate) => {{
  const rect = candidate.getBoundingClientRect();
  const style = getComputedStyle(candidate);
  return rect.width > 0 && rect.height > 0 && style.visibility !== 'hidden' && style.display !== 'none';
}});
if (!element) return {{ ok: false, error: `selector matched no visible element: ${{selector}}`, count: elements.length }};
if ({scroll}) element.scrollIntoView({{ block: 'center', inline: 'center' }});
if (marker) element.setAttribute('data-socai-type-target', marker);
const rect = element.getBoundingClientRect();
return {{ ok: true, selector, marker, count: elements.length, tag: element.tagName, x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 }};
"#
        );
        let value = self.page.evaluate_json(&script).await?;
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            anyhow::bail!(
                "{}",
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("failed to locate browser-script target")
            );
        }
        Ok(value)
    }

    async fn clear_marked_target(&self, marker: &str) -> Result<()> {
        let marker_json = serde_json::to_string(marker)?;
        let script = format!(
            r#"
const marker = {marker_json};
const element = Array.from(document.querySelectorAll('[data-socai-type-target]'))
  .find((candidate) => candidate.getAttribute('data-socai-type-target') === marker);
if (!element) return {{ ok: false, error: 'located type target disappeared before typing' }};
element.removeAttribute('data-socai-type-target');
if (document.activeElement !== element) {{
  return {{ ok: false, error: 'focus moved away from the located type target' }};
}}
if (element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement) {{
  const prototype = element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  const descriptor = Object.getOwnPropertyDescriptor(prototype, 'value');
  if (descriptor && descriptor.set) descriptor.set.call(element, '');
  else element.value = '';
}} else if (element.isContentEditable) {{
  element.textContent = '';
}} else {{
  return {{ ok: false, error: `located type target is not editable: ${{element.tagName}}` }};
}}
element.dispatchEvent(new InputEvent('input', {{ bubbles: true, inputType: 'deleteContentBackward', data: null }}));
element.dispatchEvent(new Event('change', {{ bubbles: true }}));
element.focus();
if (document.activeElement !== element) return {{ ok: false, error: 'type target lost focus while being cleared' }};
return {{ ok: true, tag: element.tagName }};
"#
        );
        let value = self.page.evaluate_json(&script).await?;
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            anyhow::bail!(
                "{}",
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("failed to clear browser-script target")
            );
        }
        Ok(())
    }

    async fn page_info(&self) -> Value {
        self.page.evaluate_json("return { url: location.href, title: document.title, ready_state: document.readyState };")
            .await
            .unwrap_or_else(|error| json!({ "error": format!("{error:#}") }))
    }

    async fn override_blocker(&self) -> Option<String> {
        match self.site.as_str() {
            "xhs" => {
                use crate::sites::xhs::page::XhsPageRuntime;

                match XhsPageRuntime::new(&self.page).is_logged_in().await {
                    Ok(true) => {}
                    Ok(false) => return Some("login_state_unconfirmed".to_string()),
                    Err(error) => return Some(format!("page_access_failed: {error:#}")),
                }
                match crate::sites::xhs::page_diagnostics::browser_override_blocker(&self.page)
                    .await
                {
                    Some(blocker) => Some(blocker),
                    None => self.page_security_blocker().await,
                }
            }
            "dy" => {
                let state = match crate::sites::dy::DouyinPageRuntime::new(&self.page)
                    .detect_state()
                    .await
                {
                    Ok(state) => state,
                    Err(error) => return Some(format!("page_access_failed: {error:#}")),
                };
                if state.get("login_required").and_then(Value::as_bool) == Some(true) {
                    Some("login_required".to_string())
                } else if state.get("blank_or_throttled").and_then(Value::as_bool) == Some(true) {
                    Some("blank_or_throttled".to_string())
                } else {
                    self.page_security_blocker().await
                }
            }
            _ => Some("unsupported_override_site".to_string()),
        }
    }

    async fn page_security_blocker(&self) -> Option<String> {
        let state = self
            .page
            .evaluate_json(
                r#"
const body = (document.body?.innerText || '').slice(0, 50000);
const visible = (element) => {
  if (!element) return false;
  const style = getComputedStyle(element);
  const rect = element.getBoundingClientRect();
  return style.display !== 'none' && style.visibility !== 'hidden' && Number(style.opacity || 1) > 0 && rect.width > 0 && rect.height > 0;
};
const captchaNode = Array.from(document.querySelectorAll(
  'iframe[src*="captcha" i], iframe[title*="captcha" i], [class*="captcha" i], [id*="captcha" i]'
)).find(visible);
return {
  captcha_required: Boolean(captchaNode) || /请完成.{0,12}验证|拖动.{0,12}滑块|点击.{0,12}验证|verify you are human|\bcaptcha\b/i.test(body),
  rate_limited: /访问过于频繁|操作过于频繁|操作频繁|请求频繁|稍后再试|too many requests|rate[ -]?limit|frequent requests/i.test(body),
  security_verification: /安全验证|风险验证|异常访问|unusual traffic|security verification/i.test(body),
};
"#,
            )
            .await
            .ok()?;
        if state.get("captcha_required").and_then(Value::as_bool) == Some(true) {
            Some("captcha_required".to_string())
        } else if state.get("rate_limited").and_then(Value::as_bool) == Some(true) {
            Some("rate_limited".to_string())
        } else if state.get("security_verification").and_then(Value::as_bool) == Some(true) {
            Some("security_verification".to_string())
        } else {
            None
        }
    }
}

pub fn with_browser_script(
    site: &str,
    page: Arc<PageSession>,
    tools: Vec<SharedTool>,
) -> Result<Vec<SharedTool>> {
    let runtime = BrowserScriptRuntime::new(site, page)?;
    let allowed_tools = tools
        .iter()
        .map(|tool| tool.name().to_string())
        .filter(|name| !NON_OVERRIDABLE_TOOLS.contains(&name.as_str()))
        .filter(|name| supports_local_override(site, name))
        .collect::<BTreeSet<_>>();
    let allowed_tools = Arc::new(allowed_tools);

    let mut wrapped = tools
        .into_iter()
        .map(|inner| {
            Arc::new(LocalOverrideTool {
                inner,
                runtime: runtime.clone(),
            }) as SharedTool
        })
        .collect::<Vec<_>>();
    wrapped.push(Arc::new(BrowserScriptTool::new(runtime, allowed_tools)) as SharedTool);
    Ok(wrapped)
}

struct BrowserScriptTool {
    runtime: BrowserScriptRuntime,
    allowed_tools: Arc<BTreeSet<String>>,
    description: String,
}

impl BrowserScriptTool {
    fn new(runtime: BrowserScriptRuntime, allowed_tools: Arc<BTreeSet<String>>) -> Self {
        let names = allowed_tools.iter().cloned().collect::<Vec<_>>().join(", ");
        let contract = override_contract_hint(&runtime.site);
        let description = format!(
            "Execute arbitrary JavaScript that controls the current {} browser tab. The script is an async function body with `input` and `socai` available and must return JSON-serializable data. Use `await socai.evaluate(pageScript, input)` for arbitrary DOM JavaScript in the live page's isolated world; pageScript may be a JavaScript function such as `() => ({{ title: document.title }})` or a function-body string. Function closures and page-owned JavaScript globals are not shared, but the live DOM is available. The same forms work with `waitFor(pageScript, options)`. Other helpers are `pageInfo()`, `click(selector)`, `type(selector, text)`, `press(key)`, `navigate(url)`, `scroll(deltaY)`, and `wait(ms)`. The control program survives target-page navigation, and click/type/press use trusted CDP input. Use small probes to diagnose a failed browser-backed tool. Optional `save_as.tool` writes and activates a verified local override for one of: {}. {} On a socai upgrade, the built-in tool receives one canary call before an older override is reused; a working built-in retires the override, while a still-failing built-in lets a contract-valid override be re-certified. JavaScript has the logged-in page's authority but no host shell or file-system APIs. Do not use it to bypass login, captcha, security verification, rate limits, permissions, or a confirmed valid empty result.",
            runtime.site, names, contract
        );
        Self {
            runtime,
            allowed_tools,
            description,
        }
    }
}

#[async_trait]
impl Tool for BrowserScriptTool {
    fn name(&self) -> &str {
        BROWSER_SCRIPT_TOOL_NAME
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "script": {
                    "type": "string",
                    "description": "JavaScript async-function body. Use `input` and the async `socai` browser API; return JSON-serializable data."
                },
                "input": {
                    "description": "JSON value exposed to the script as `input`."
                },
                "timeout_ms": {
                    "type": "integer",
                    "minimum": 100,
                    "maximum": MAX_TIMEOUT_MS,
                    "default": DEFAULT_TIMEOUT_MS
                },
                "save_as": {
                    "type": "object",
                    "description": "After a successful disk-backed execution, activate this script as a local override for the named site tool.",
                    "properties": {
                        "tool": {
                            "type": "string",
                            "enum": self.allowed_tools.iter().cloned().collect::<Vec<_>>()
                        }
                    },
                    "required": ["tool"],
                    "additionalProperties": false
                }
            },
            "required": ["script"],
            "additionalProperties": false
        })
    }

    fn always_available(&self) -> bool {
        true
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        let _lifecycle_guard = self.runtime.registry.lifecycle_lock.lock().await;
        let script = input
            .get("script")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("browser_script requires `script`"))?;
        let script_input = input.get("input").cloned().unwrap_or_else(|| json!({}));
        let timeout_ms = input
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .clamp(100, MAX_TIMEOUT_MS);
        let page_before = self.runtime.page_info().await;

        let save_tool = input
            .get("save_as")
            .and_then(|value| value.get("tool"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());

        if save_tool.is_some() {
            if let Some(blocker) = self.runtime.override_blocker().await {
                return Ok(json_result(&json!({
                    "ok": false,
                    "reason": "override_activation_blocked",
                    "error": "a local override cannot be validated or activated on the current blocked page",
                    "blocker": blocker,
                    "page": self.runtime.page_info().await,
                })));
            }
        }

        let candidate_path = if let Some(tool) = save_tool {
            if !self.allowed_tools.contains(tool) {
                return Ok(json_result(&json!({
                    "ok": false,
                    "reason": "override_not_allowed",
                    "error": format!("{tool} is not an overridable {} tool", self.runtime.site),
                    "allowed_tools": self.allowed_tools,
                })));
            }
            match self
                .runtime
                .registry
                .write_candidate(&self.runtime.site, tool, script)
            {
                Ok(path) => Some(path),
                Err(error) => {
                    return Ok(json_result(&json!({
                        "ok": false,
                        "reason": "script_save_failed",
                        "error": format!("{error:#}"),
                    })))
                }
            }
        } else {
            None
        };

        // When persisting, execute the exact bytes read back from disk. This
        // proves that the reusable file — not only the model's in-memory call
        // argument — parses and returns the expected result.
        let executable = match candidate_path.as_ref() {
            Some(path) => match std::fs::read_to_string(path) {
                Ok(source) => source,
                Err(error) => {
                    return Ok(json_result(&json!({
                        "ok": false,
                        "reason": "script_readback_failed",
                        "error": format!("failed to read {}: {error}", path.display()),
                        "candidate_path": path,
                    })))
                }
            },
            None => script.to_string(),
        };

        let value = match self
            .runtime
            .execute(&executable, &script_input, timeout_ms)
            .await
        {
            Ok(value) => value,
            Err(error) => {
                return Ok(json_result(&json!({
                    "ok": false,
                    "reason": "browser_script_failed",
                    "error": format!("{error:#}"),
                    "candidate_path": candidate_path,
                    "page": self.runtime.page_info().await,
                })))
            }
        };

        if save_tool.is_some() {
            if let Some(blocker) = self.runtime.override_blocker().await {
                return Ok(json_result(&json!({
                    "ok": false,
                    "reason": "override_activation_blocked",
                    "error": "the script reached a blocked page, so its result cannot be validated or activated",
                    "blocker": blocker,
                    "result": value,
                    "candidate_path": candidate_path,
                    "page": self.runtime.page_info().await,
                })));
            }
        }

        if let Some(tool) = save_tool {
            if let Err(error) =
                validate_override_result(&self.runtime.site, tool, &script_input, &value, true)
            {
                return Ok(json_result(&json!({
                    "ok": false,
                    "reason": "script_validation_failed",
                    "error": format!("{error:#}"),
                    "result": value,
                    "candidate_path": candidate_path,
                    "page": self.runtime.page_info().await,
                })));
            }
        }

        let saved = if let Some(tool) = save_tool {
            let source_url = page_before
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match self.runtime.registry.activate(
                &self.runtime.site,
                tool,
                candidate_path
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("override candidate path is missing"))?,
                source_url,
                timeout_ms,
            ) {
                Ok((path, manifest)) => Some(json!({
                    "site": manifest.site,
                    "tool": manifest.tool,
                    "path": path,
                    "source_version": manifest.source_version,
                    "validated_with_version": manifest.validated_with_version,
                    "tool_contract_version": manifest.tool_contract_version,
                    "tool_impl_revision": manifest.tool_impl_revision,
                    "created_at": manifest.created_at,
                    "last_validated_at": manifest.last_validated_at,
                    "status_changed_at": manifest.status_changed_at,
                    "status": manifest.status,
                    "active": true,
                })),
                Err(error) => {
                    return Ok(json_result(&json!({
                        "ok": false,
                        "reason": "script_activation_failed",
                        "error": format!("{error:#}"),
                        "result": value,
                        "candidate_path": candidate_path,
                    })))
                }
            }
        } else {
            None
        };

        Ok(json_result(&json!({
            "ok": true,
            "result": value,
            "saved": saved,
            "page_before": page_before,
            "page_after": self.runtime.page_info().await,
            "next_action": saved.as_ref().map(|_| "retry_the_original_tool_once"),
        })))
    }
}

struct LocalOverrideTool {
    inner: SharedTool,
    runtime: BrowserScriptRuntime,
}

struct OverrideExecutionFailure {
    reason: &'static str,
    error: String,
    blocker: Option<String>,
}

impl LocalOverrideTool {
    async fn execute_override(
        &self,
        input: &Value,
        manifest: &ActiveOverride,
        script: &str,
    ) -> std::result::Result<Value, OverrideExecutionFailure> {
        let value = self
            .runtime
            .execute(script, input, manifest.timeout_ms)
            .await
            .map_err(|error| OverrideExecutionFailure {
                reason: "local_override_failed",
                error: format!("{error:#}"),
                blocker: None,
            })?;
        if let Some(blocker) = self.runtime.override_blocker().await {
            return Err(OverrideExecutionFailure {
                reason: "local_override_page_blocked",
                error: format!("local override reached a blocked page: {blocker}"),
                blocker: Some(blocker),
            });
        }
        validate_override_result(&self.runtime.site, self.inner.name(), input, &value, false)
            .map_err(|error| OverrideExecutionFailure {
                reason: "local_override_result_failed",
                error: format!("{error:#}"),
                blocker: None,
            })?;
        Ok(value)
    }

    fn transition_override(
        &self,
        manifest: &ActiveOverride,
        manifest_path: &Path,
        status: OverrideStatus,
        reason: &str,
        revalidate: bool,
    ) -> ActiveOverride {
        match self
            .runtime
            .registry
            .transition(manifest, manifest_path, status, reason, revalidate)
        {
            Ok(next) => next,
            Err(error) => {
                tracing::warn!(
                    site = %self.runtime.site,
                    tool = %self.inner.name(),
                    %error,
                    "failed to persist local override state transition"
                );
                manifest.clone()
            }
        }
    }

    async fn call_builtin(&self, input: Value, ctx: &ToolContext) -> Result<ToolResult> {
        match self.inner.call(input.clone(), ctx).await {
            Ok(result) => {
                Ok(
                    annotate_repairable_result(&self.runtime, self.inner.name(), &input, result)
                        .await,
                )
            }
            Err(error) => {
                let rendered = format!("{error:#}");
                if is_repairable_browser_error(&rendered) {
                    Ok(repair_failure_result(
                        &self.runtime,
                        self.inner.name(),
                        &input,
                        "tool_execution_failed",
                        &rendered,
                        None,
                    )
                    .await)
                } else {
                    Err(error)
                }
            }
        }
    }

    async fn call_builtin_after_override_failure(
        &self,
        input: Value,
        ctx: &ToolContext,
        manifest: Option<&ActiveOverride>,
        manifest_path: Option<&Path>,
        path: Option<&Path>,
        failure: OverrideExecutionFailure,
    ) -> Result<ToolResult> {
        if let Some(blocker) = failure.blocker.as_deref() {
            return Ok(override_blocked_result(
                &self.runtime,
                self.inner.name(),
                "local_override_execution",
                blocker,
            )
            .await);
        }
        match self.inner.call(input.clone(), ctx).await {
            Ok(result)
                if tool_result_succeeded(
                    &self.runtime.site,
                    self.inner.name(),
                    &input,
                    &result,
                ) =>
            {
                if let Some(blocker) = self.runtime.override_blocker().await {
                    return Ok(override_blocked_result(
                        &self.runtime,
                        self.inner.name(),
                        "built_in_fallback",
                        &blocker,
                    )
                    .await);
                }
                if let (Some(manifest), Some(manifest_path)) = (manifest, manifest_path) {
                    self.transition_override(
                        manifest,
                        manifest_path,
                        OverrideStatus::Stale,
                        "override_failed_builtin_succeeded",
                        false,
                    );
                }
                Ok(result)
            }
            Ok(result) if tool_result_is_repairable_failure(&result) => {
                let error = format!(
                    "{}; built-in fallback also failed: {}",
                    failure.error,
                    tool_result_failure_summary(&result)
                );
                Ok(repair_failure_result(
                    &self.runtime,
                    self.inner.name(),
                    &input,
                    failure.reason,
                    &error,
                    path,
                )
                .await)
            }
            Ok(result) => Ok(result),
            Err(error) => {
                let rendered = format!("{error:#}");
                if is_repairable_browser_error(&rendered) {
                    let error = format!(
                        "{}; built-in fallback also failed: {rendered}",
                        failure.error
                    );
                    Ok(repair_failure_result(
                        &self.runtime,
                        self.inner.name(),
                        &input,
                        failure.reason,
                        &error,
                        path,
                    )
                    .await)
                } else {
                    Err(error)
                }
            }
        }
    }

    async fn revalidate_override(
        &self,
        input: Value,
        ctx: &ToolContext,
        manifest: ActiveOverride,
        manifest_path: PathBuf,
        script_path: PathBuf,
        script: String,
    ) -> Result<ToolResult> {
        if let Some(blocker) = self.runtime.override_blocker().await {
            tracing::warn!(
                site = %self.runtime.site,
                tool = %self.inner.name(),
                %blocker,
                "older local override skipped because the live page is blocked"
            );
            return self.call_builtin(input, ctx).await;
        }

        let builtin_failure = match self.inner.call(input.clone(), ctx).await {
            Ok(result)
                if tool_result_succeeded(
                    &self.runtime.site,
                    self.inner.name(),
                    &input,
                    &result,
                ) =>
            {
                if let Some(blocker) = self.runtime.override_blocker().await {
                    return Ok(override_blocked_result(
                        &self.runtime,
                        self.inner.name(),
                        "built_in_version_canary",
                        &blocker,
                    )
                    .await);
                }
                self.transition_override(
                    &manifest,
                    &manifest_path,
                    OverrideStatus::Stale,
                    "builtin_succeeded_after_version_change",
                    false,
                );
                return Ok(result);
            }
            Ok(result) if tool_result_is_repairable_failure(&result) => {
                tool_result_failure_summary(&result)
            }
            Ok(result) => return Ok(result),
            Err(error) => {
                let rendered = format!("{error:#}");
                if !is_repairable_browser_error(&rendered) {
                    return Err(error);
                }
                rendered
            }
        };

        if let Some(blocker) = self.runtime.override_blocker().await {
            return Ok(override_blocked_result(
                &self.runtime,
                self.inner.name(),
                "built_in_version_canary",
                &blocker,
            )
            .await);
        }

        match self.execute_override(&input, &manifest, &script).await {
            Ok(value) => {
                let manifest = self.transition_override(
                    &manifest,
                    &manifest_path,
                    OverrideStatus::Active,
                    "override_revalidated_after_builtin_failure",
                    true,
                );
                Ok(local_override_result(
                    ctx,
                    value,
                    &manifest,
                    &script_path,
                    Some(&builtin_failure),
                ))
            }
            Err(failure) => {
                if let Some(blocker) = failure.blocker.as_deref() {
                    return Ok(override_blocked_result(
                        &self.runtime,
                        self.inner.name(),
                        "local_override_revalidation",
                        blocker,
                    )
                    .await);
                }
                let error = format!(
                    "built-in canary failed: {builtin_failure}; previous override also failed: {}",
                    failure.error
                );
                Ok(repair_failure_result(
                    &self.runtime,
                    self.inner.name(),
                    &input,
                    failure.reason,
                    &error,
                    Some(script_path.as_path()),
                )
                .await)
            }
        }
    }
}

#[async_trait]
impl Tool for LocalOverrideTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn input_schema(&self) -> Value {
        self.inner.input_schema()
    }

    fn always_available(&self) -> bool {
        self.inner.always_available()
    }

    fn defer_until_site(&self) -> &str {
        self.inner.defer_until_site()
    }

    fn effective_input(&self, input: &Value) -> Value {
        self.inner.effective_input(input)
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let _lifecycle_guard = self.runtime.registry.lifecycle_lock.lock().await;
        let local_override = match self
            .runtime
            .registry
            .active_script(&self.runtime.site, self.inner.name())
        {
            Ok(value) => value,
            Err(error) => {
                return self
                    .call_builtin_after_override_failure(
                        input,
                        ctx,
                        None,
                        None,
                        None,
                        OverrideExecutionFailure {
                            reason: "local_override_load_failed",
                            error: format!("{error:#}"),
                            blocker: None,
                        },
                    )
                    .await
            }
        };
        let Some((manifest, manifest_path, script_path, script)) = local_override else {
            return self.call_builtin(input, ctx).await;
        };

        let current_contract = override_contract_version(&self.runtime.site, self.inner.name());
        if manifest.tool_contract_version != current_contract {
            let reason = format!(
                "tool_contract_changed_from_{}_to_{}",
                manifest.tool_contract_version, current_contract
            );
            self.transition_override(
                &manifest,
                &manifest_path,
                OverrideStatus::Quarantined,
                &reason,
                false,
            );
            return self.call_builtin(input, ctx).await;
        }

        let current_revision = builtin_tool_revision(&self.runtime.site, self.inner.name());
        if manifest.validated_with_version != env!("CARGO_PKG_VERSION")
            || manifest.tool_impl_revision != current_revision
        {
            return self
                .revalidate_override(input, ctx, manifest, manifest_path, script_path, script)
                .await;
        }

        if let Some(blocker) = self.runtime.override_blocker().await {
            tracing::warn!(
                site = %self.runtime.site,
                tool = %self.inner.name(),
                %blocker,
                "local override skipped because the live page is blocked"
            );
            return self.call_builtin(input, ctx).await;
        }

        match self.execute_override(&input, &manifest, &script).await {
            Ok(value) => Ok(local_override_result(
                ctx,
                value,
                &manifest,
                &script_path,
                None,
            )),
            Err(failure) => {
                self.call_builtin_after_override_failure(
                    input,
                    ctx,
                    Some(&manifest),
                    Some(manifest_path.as_path()),
                    Some(script_path.as_path()),
                    failure,
                )
                .await
            }
        }
    }
}

fn tool_result_value(result: &ToolResult) -> Option<Value> {
    let ToolResultBlock::Text { text } = result.blocks.first()? else {
        return None;
    };
    serde_json::from_str(text).ok()
}

fn tool_result_succeeded(site: &str, tool: &str, input: &Value, result: &ToolResult) -> bool {
    let Some(mut value) = tool_result_value(result) else {
        return false;
    };

    // The XHS preview intentionally removes its redundant `ok:true` before
    // returning the lean payload. Restore it only for contract validation;
    // every other built-in must explicitly report success.
    if site == "xhs"
        && tool == "search"
        && input.get("preview").and_then(Value::as_bool) == Some(true)
        && value.get("ok").is_none()
    {
        if let Some(object) = value.as_object_mut() {
            object.insert("ok".to_string(), Value::Bool(true));
        }
    }

    validate_override_result(site, tool, input, &value, false).is_ok()
}

fn tool_result_is_repairable_failure(result: &ToolResult) -> bool {
    let Some(value) = tool_result_value(result) else {
        return false;
    };
    value.get("ok").and_then(Value::as_bool) == Some(false)
        && !contains_non_repairable_failure(&value)
        && is_structured_repairable_failure(&value)
}

fn tool_result_failure_summary(result: &ToolResult) -> String {
    tool_result_value(result)
        .as_ref()
        .map(failure_reason)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| result.flat_text().chars().take(1024).collect())
}

async fn annotate_repairable_result(
    runtime: &BrowserScriptRuntime,
    tool: &str,
    input: &Value,
    mut result: ToolResult,
) -> ToolResult {
    let Some(ToolResultBlock::Text { text }) = result.blocks.first_mut() else {
        return result;
    };
    let Ok(mut value) = serde_json::from_str::<Value>(text) else {
        return result;
    };
    if value.get("ok").and_then(Value::as_bool) != Some(false) {
        return result;
    }
    let reason = failure_reason(&value).to_string();
    if contains_non_repairable_failure(&value) || !is_structured_repairable_failure(&value) {
        return result;
    }
    let page = runtime.page_info().await;
    if let Some(object) = value.as_object_mut() {
        object.entry("failure".to_string()).or_insert_with(|| {
            json!({
                "kind": "browser_tool_failed",
                "summary": reason,
                "observed": page,
            })
        });
        object.insert(
            "recovery".to_string(),
            recovery_directive(&runtime.site, tool, input, None),
        );
    }
    *text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    result
}

async fn repair_failure_result(
    runtime: &BrowserScriptRuntime,
    tool: &str,
    input: &Value,
    reason: &str,
    error: &str,
    active_script: Option<&Path>,
) -> ToolResult {
    json_result(&json!({
        "ok": false,
        "reason": reason,
        "error": error,
        "failure": {
            "kind": "browser_tool_failed",
            "observed": runtime.page_info().await,
            "active_script": active_script,
        },
        "recovery": recovery_directive(&runtime.site, tool, input, active_script),
    }))
}

async fn override_blocked_result(
    runtime: &BrowserScriptRuntime,
    tool: &str,
    phase: &str,
    blocker: &str,
) -> ToolResult {
    json_result(&json!({
        "ok": false,
        "reason": blocker,
        "error": "local override processing stopped because the live page requires user or server recovery",
        "failure": {
            "kind": "browser_page_blocked",
            "phase": phase,
            "tool": tool,
            "observed": runtime.page_info().await,
        },
    }))
}

fn recovery_directive(
    site: &str,
    tool: &str,
    input: &Value,
    active_script: Option<&Path>,
) -> Value {
    json!({
        "action": BROWSER_SCRIPT_TOOL_NAME,
        "same_page": true,
        "original_tool": tool,
        "original_input": input,
        "active_script": active_script,
        "save_as": {
            "site": site,
            "tool": tool,
            "mode": "override",
        },
        "instruction": "Inspect the live page with small browser_script probes, save a verified replacement for this exact tool, then retry the original tool once and continue the user's task. Do not repeat the original call before repair.",
    })
}

fn failure_reason(value: &Value) -> &str {
    value
        .get("reason")
        .and_then(Value::as_str)
        .or_else(|| value.get("error").and_then(Value::as_str))
        .unwrap_or("unknown_browser_tool_failure")
}

fn supports_local_override(site: &str, tool: &str) -> bool {
    match site {
        "xhs" => matches!(tool, "search" | "get_notes" | "author_scan"),
        "dy" => tool == "search",
        _ => false,
    }
}

fn override_contract_version(site: &str, tool: &str) -> u32 {
    match (site, tool) {
        ("xhs", "search" | "get_notes" | "author_scan") | ("dy", "search") => 1,
        _ => 0,
    }
}

fn builtin_tool_revision(site: &str, tool: &str) -> u32 {
    match (site, tool) {
        // Increment the matching revision when a built-in implementation
        // changes without changing the override input/output contract. That
        // forces one native canary even in a locally rebuilt package carrying
        // the same Cargo version.
        ("xhs", "search") => 2,
        ("xhs", "get_notes" | "author_scan") | ("dy", "search") => 1,
        _ => 0,
    }
}

fn override_contract_hint(site: &str) -> &'static str {
    match site {
        "xhs" => {
            "Persistent results must explicitly return ok:true and are contract-checked per item: search must preserve query and return usable notes (or cards when preview=true); get_notes must return every requested note_id; author_scan must preserve author_id and return an identified profile plus extracted notes/cards. Activation requires non-empty extraction evidence."
        }
        "dy" => {
            "Persistent search results must explicitly return ok:true, preserve query, and return non-empty, usable video cards before activation."
        }
        _ => "Persistent local overrides are unavailable for this site.",
    }
}

fn validate_override_result(
    site: &str,
    tool: &str,
    input: &Value,
    value: &Value,
    require_extraction_evidence: bool,
) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("{site}.{tool} override must return a JSON object"))?;
    if object.get("ok").and_then(Value::as_bool) != Some(true) {
        anyhow::bail!("{site}.{tool} override must explicitly return ok:true");
    }

    match (site, tool) {
        ("xhs", "search") => {
            require_matching_field(input, value, "query")?;
            let expected = if input.get("preview").and_then(Value::as_bool) == Some(true) {
                "cards"
            } else {
                "notes"
            };
            validate_collection(
                value,
                expected,
                require_extraction_evidence,
                |entry, index| {
                    if expected == "cards" {
                        validate_xhs_card(entry, &format!("cards[{index}]"))
                    } else {
                        validate_xhs_note_entry(entry, &format!("notes[{index}]"))
                    }
                },
            )?;
        }
        ("xhs", "get_notes") => {
            validate_get_notes_result(input, value, require_extraction_evidence)?
        }
        ("xhs", "author_scan") => {
            require_matching_field(input, value, "author_id")?;
            let profile = value
                .get("profile")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "xhs.author_scan override result requires object field 'profile'"
                    )
                })?;
            require_nonempty_string(profile, "display_name", "profile")?;
            require_any_nonempty_string(profile, &["xhs_id", "url"], "profile")?;
            validate_collection(value, "notes", false, |entry, index| {
                validate_xhs_note_entry(entry, &format!("notes[{index}]"))
            })?;
            if let Some(cards) = profile.get("note_cards").and_then(Value::as_array) {
                for (index, card) in cards.iter().enumerate() {
                    validate_xhs_card(card, &format!("profile.note_cards[{index}]"))?;
                }
            }
            if require_extraction_evidence {
                let notes_have_evidence = value
                    .get("notes")
                    .and_then(Value::as_array)
                    .is_some_and(|notes| !notes.is_empty());
                let cards_have_evidence = profile
                    .get("note_cards")
                    .and_then(Value::as_array)
                    .is_some_and(|cards| !cards.is_empty());
                if !notes_have_evidence && !cards_have_evidence {
                    anyhow::bail!(
                        "xhs.author_scan override needs at least one extracted note or profile card before activation"
                    );
                }
            }
        }
        ("dy", "search") => {
            require_matching_field(input, value, "query")?;
            validate_collection(
                value,
                "cards",
                require_extraction_evidence,
                |entry, index| validate_dy_card(entry, &format!("cards[{index}]")),
            )?;
        }
        _ => anyhow::bail!("{site}.{tool} does not support persistent local overrides"),
    }
    Ok(())
}

fn validate_get_notes_result(
    input: &Value,
    value: &Value,
    require_extraction_evidence: bool,
) -> Result<()> {
    let returned = require_array_field(value, "notes")?;
    let requested = input
        .get("notes")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("xhs.get_notes input requires array field 'notes'"))?;
    if require_extraction_evidence && requested.is_empty() {
        anyhow::bail!(
            "xhs.get_notes override requires at least one requested note before activation"
        );
    }
    if returned.len() != requested.len() {
        anyhow::bail!(
            "xhs.get_notes override returned {} notes for {} requested notes",
            returned.len(),
            requested.len()
        );
    }
    for (index, entry) in returned.iter().enumerate() {
        validate_xhs_note_entry(entry, &format!("notes[{index}]"))?;
    }
    let returned_ids = returned
        .iter()
        .filter_map(note_id_from_entry)
        .collect::<BTreeSet<_>>();
    for requested_note in requested {
        let note_id = requested_note
            .get("note_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("xhs.get_notes input contains an empty note_id"))?;
        if !returned_ids.contains(note_id) {
            anyhow::bail!("xhs.get_notes override result is missing requested note {note_id}");
        }
    }
    Ok(())
}

fn validate_collection<F>(
    value: &Value,
    key: &str,
    require_evidence: bool,
    mut validate: F,
) -> Result<()>
where
    F: FnMut(&Value, usize) -> Result<()>,
{
    let entries = require_array_field(value, key)?;
    if require_evidence && entries.is_empty() {
        anyhow::bail!("override result needs a non-empty '{key}' array before activation");
    }
    for (index, entry) in entries.iter().enumerate() {
        validate(entry, index)?;
    }
    Ok(())
}

fn validate_xhs_card(value: &Value, label: &str) -> Result<()> {
    let card = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("{label} must be an object"))?;
    require_nonempty_string(card, "note_id", label)?;
    require_any_nonempty_string(card, &["link", "url", "xsec_token"], label)?;
    require_any_nonempty_string(card, &["title", "author", "cover_url"], label)
}

fn validate_dy_card(value: &Value, label: &str) -> Result<()> {
    let card = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("{label} must be an object"))?;
    require_nonempty_string(card, "video_id", label)?;
    require_any_nonempty_string(card, &["url", "link"], label)?;
    require_any_nonempty_string(card, &["title", "author"], label)
}

fn validate_xhs_note_entry(value: &Value, label: &str) -> Result<()> {
    let entry = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("{label} must be an object"))?;
    if entry.get("ok").and_then(Value::as_bool) != Some(true) {
        anyhow::bail!("{label} must explicitly return ok:true");
    }
    let entity = entry
        .get("entity")
        .and_then(Value::as_object)
        .unwrap_or(entry);
    let entity_label = format!("{label}.entity");
    require_nonempty_string(entity, "note_id", &entity_label)?;
    require_nonempty_string(entity, "url", &entity_label)?;
    require_any_nonempty_string(
        entity,
        &["title", "content", "author", "author_id", "cover_url"],
        &entity_label,
    )
}

fn require_nonempty_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
    label: &str,
) -> Result<()> {
    if object
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Ok(());
    }
    anyhow::bail!("{label} requires non-empty string field '{key}'")
}

fn require_any_nonempty_string(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
    label: &str,
) -> Result<()> {
    if keys.iter().any(|key| {
        object
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    }) {
        return Ok(());
    }
    anyhow::bail!(
        "{label} requires a non-empty string in one of: {}",
        keys.join(", ")
    )
}

fn require_matching_field(input: &Value, output: &Value, key: &str) -> Result<()> {
    let expected = input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("override input requires string field '{key}'"))?;
    let actual = output
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if actual != expected {
        anyhow::bail!("override result '{key}' does not match the original input");
    }
    Ok(())
}

fn require_array_field<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("override result requires array field '{key}'"))
}

fn note_id_from_entry(value: &Value) -> Option<&str> {
    value
        .get("note_id")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("entity")
                .and_then(|entity| entity.get("note_id"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn local_override_result(
    ctx: &ToolContext,
    mut value: Value,
    manifest: &ActiveOverride,
    path: &Path,
    builtin_attempt_failure: Option<&str>,
) -> ToolResult {
    let note_ids = value
        .get("notes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(note_id_from_entry)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if !note_ids.is_empty() {
        ctx.add_search_note_ids(&note_ids);
    }

    let artifact_path = ctx
        .write_json_artifact(
            &format!("{}_local_override", manifest.tool),
            &value,
            "artifacts",
            &manifest.tool,
            "json",
            &format!("{} result from a local browser override", manifest.tool),
            json!({
                "site": manifest.site,
                "tool": manifest.tool,
                "local_override": true,
            }),
        )
        .ok()
        .map(|relative| ctx.run_dir.join(relative).to_string_lossy().into_owned());

    if let Some(object) = value.as_object_mut() {
        object.insert(
            "_socai_local_override".to_string(),
            json!({
                "active": true,
                "site": manifest.site,
                "tool": manifest.tool,
                "path": path,
                "source_version": manifest.source_version,
                "validated_with_version": manifest.validated_with_version,
                "tool_contract_version": manifest.tool_contract_version,
                "tool_impl_revision": manifest.tool_impl_revision,
                "created_at": manifest.created_at,
                "last_validated_at": manifest.last_validated_at,
                "status_changed_at": manifest.status_changed_at,
                "status": manifest.status,
                "status_reason": manifest.status_reason,
                "artifact_path": artifact_path,
                "builtin_attempt": builtin_attempt_failure.map(|failure| json!({
                    "status": "repairable_failure",
                    "summary": failure,
                    "side_effects": "The failed built-in attempt may already have changed the tab, written partial artifacts/history, or started requested host enrichment.",
                })),
                "host_enrichment": if builtin_attempt_failure.is_some() {
                    "built_in_attempt_may_have_partially_run"
                } else {
                    "not_run"
                },
                "note": if builtin_attempt_failure.is_some() {
                    "The override result is authoritative for this call, but a preceding failed built-in revalidation attempt may have produced partial local side effects."
                } else {
                    "The browser override preserves JSON evidence and a run artifact, but built-in media download, OCR, ASR, and cross-run history hooks are not available."
                },
            }),
        );
    }
    json_result(&value)
}

fn contains_non_repairable_failure(value: &Value) -> bool {
    failure_strings(value).any(|text| {
        let text = text.to_ascii_lowercase();
        NON_REPAIRABLE_REASONS
            .iter()
            .any(|reason| text.contains(reason))
            || [
                "login",
                "captcha",
                "security verification",
                "rate limit",
                "rate-limit",
                "permission",
                "user action",
                "blank_or_throttled",
                "no_results",
                "empty_result",
            ]
            .iter()
            .any(|marker| text.contains(marker))
    })
}

fn failure_strings(value: &Value) -> Box<dyn Iterator<Item = &str> + '_> {
    match value {
        Value::Object(object) => Box::new(object.iter().flat_map(|(key, value)| {
            let own = matches!(key.as_str(), "reason" | "error" | "code")
                .then(|| value.as_str())
                .flatten()
                .into_iter();
            own.chain(failure_strings(value))
        })),
        Value::Array(values) => Box::new(values.iter().flat_map(failure_strings)),
        _ => Box::new(std::iter::empty()),
    }
}

fn is_structured_repairable_failure(value: &Value) -> bool {
    let reason = failure_reason(value).to_ascii_lowercase();
    let known_browser_reason = matches!(
        reason.as_str(),
        "search_failed"
            | "search_submit_failed"
            | "search_transition_failed"
            | "page_access_failed"
            | "navigation_failed"
            | "extract_profile_failed"
            | "not_profile_page"
            | "note_content_unavailable"
            | "stale_note"
            | "direct_open_failed"
            | "open_profile_failed"
    ) || reason.ends_with("_selector_failed")
        || reason.ends_with("_transition_failed")
        || reason.ends_with("_extraction_failed");
    known_browser_reason || failure_strings(value).any(is_repairable_browser_error)
}

fn is_repairable_browser_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    if [
        "login",
        "captcha",
        "security verification",
        "security_verification",
        "rate limit",
        "rate_limit",
        "permission",
        "user action",
        "blank_or_throttled",
        "no_results",
        "empty_result",
        "query is required",
        "missing ",
        "invalid input",
        "must be ",
        "unsupported ",
        "artifact",
        "file system",
        "filesystem",
        "media processor",
        "ocr ",
        "provider",
        "api key",
        "configuration",
    ]
    .iter()
    .any(|marker| error.contains(marker))
    {
        return false;
    }
    let direct_browser_evidence = [
        "selector",
        "dom",
        "javascript exception",
        "runtime.evaluate",
        "cdp command",
        "target closed",
        "page script",
        "page transition",
        "search transition",
        "search did not transition",
        "not a profile page",
        "not a search result page",
        "modal",
        "element not found",
        "element is not",
        "click target",
        "search input",
        "navigation to ",
    ]
    .iter()
    .any(|marker| error.contains(marker));
    let browser_timeout = (error.contains("timed out") || error.contains("timeout"))
        && [
            "cdp",
            "runtime.evaluate",
            "navigation",
            "page transition",
            "search transition",
            "selector",
            "dom",
        ]
        .iter()
        .any(|marker| error.contains(marker));
    direct_browser_evidence || browser_timeout
}

fn bounded_program_expression(script: &str, input: &Value) -> Result<String> {
    let input_json = serde_json::to_string(input)?;
    Ok(format!(
        r#"(async () => {{
  const __socaiStringify = JSON.stringify.bind(JSON);
  const __socaiEncode = TextEncoder.prototype.encode.bind(new TextEncoder());
  const __socaiError = (error) => {{
    let message = 'browser script failed';
    try {{
      if (typeof error === 'string') message = error.slice(0, {MAX_SCRIPT_ERROR_CHARS});
      else if (error && typeof error.message === 'string') message = error.message.slice(0, {MAX_SCRIPT_ERROR_CHARS});
    }} catch (_) {{}}
    return __socaiStringify({{ __socai_browser_error__: message }});
  }};
  try {{
    const __socaiValue = await (async function(input, socai) {{
{script}
    }})({input_json}, window.__socaiBridge.api);
    const __socaiJson = __socaiStringify(__socaiValue);
    if (typeof __socaiJson !== 'string') throw new Error('browser script must return a JSON-serializable value');
    const __socaiBytes = __socaiEncode(__socaiJson).byteLength;
    if (__socaiBytes > {MAX_RESULT_BYTES}) throw new Error('browser script result exceeds {MAX_RESULT_BYTES} bytes');
    return __socaiJson;
  }} catch (error) {{
    return __socaiError(error);
  }}
}})()
//# sourceURL=socai-browser-script.js"#
    ))
}

fn bounded_page_expression(script: &str, input: &Value) -> Result<String> {
    let input_json = serde_json::to_string(input)?;
    Ok(format!(
        r#"(async () => {{
  const __socaiStringify = JSON.stringify.bind(JSON);
  const __socaiEncode = TextEncoder.prototype.encode.bind(new TextEncoder());
  const __socaiError = (error) => {{
    let message = 'page script failed';
    try {{
      if (typeof error === 'string') message = error.slice(0, {MAX_SCRIPT_ERROR_CHARS});
      else if (error && typeof error.message === 'string') message = error.message.slice(0, {MAX_SCRIPT_ERROR_CHARS});
    }} catch (_) {{}}
    return __socaiStringify({{ __socai_browser_error__: message }});
  }};
  try {{
    const __socaiValue = await (async function(input) {{
{script}
    }})({input_json});
    const __socaiJson = __socaiStringify(__socaiValue);
    if (typeof __socaiJson !== 'string') throw new Error('page script must return a JSON-serializable value');
    const __socaiBytes = __socaiEncode(__socaiJson).byteLength;
    if (__socaiBytes > {MAX_RESULT_BYTES}) throw new Error('page script result exceeds {MAX_RESULT_BYTES} bytes');
    return __socaiJson;
  }} catch (error) {{
    return __socaiError(error);
  }}
}})()
//# sourceURL=socai-page-evaluate.js"#
    ))
}

fn decode_bounded_result(value: Value) -> Result<Value> {
    let serialized = value
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("bounded browser script returned a non-string payload"))?;
    if serialized.len() > MAX_RESULT_BYTES {
        anyhow::bail!(
            "browser script result is too large: {} bytes (maximum {MAX_RESULT_BYTES})",
            serialized.len()
        );
    }
    let value: Value = serde_json::from_str(serialized)
        .context("browser script returned invalid serialized JSON")?;
    if let Some(error) = value.get("__socai_browser_error__").and_then(Value::as_str) {
        anyhow::bail!("{error}");
    }
    Ok(value)
}

fn required_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("browser-script operation requires `{key}`"))
}

async fn settle_bridge_request(
    runner: &PageSession,
    id: u64,
    response: Result<Value>,
) -> Result<()> {
    let (ok, payload) = match response {
        Ok(value) => (true, value),
        Err(error) => (false, Value::String(format!("{error:#}"))),
    };
    let payload = serde_json::to_string(&payload)?;
    let expression = format!(
        "window.__socaiBridge.settle({id}, {}, {payload})",
        if ok { "true" } else { "false" }
    );
    runner
        .evaluate_json(&expression)
        .await
        .context("failed to settle browser-script bridge request")?;
    Ok(())
}

fn validate_segment(kind: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        anyhow::bail!("invalid {kind}: {value}");
    }
    Ok(())
}

fn validate_script_path(value: &str) -> Result<()> {
    use std::path::Component;

    let mut components = Path::new(value).components();
    let valid = matches!(components.next(), Some(Component::Normal(value)) if value == "versions")
        && matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && Path::new(value)
            .extension()
            .and_then(|value| value.to_str())
            == Some("js");
    if !valid {
        anyhow::bail!("invalid local override script path: {value}");
    }
    Ok(())
}

fn newest_override_manifest(dir: &Path) -> Result<Option<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", dir.display()))
        }
    };
    let mut newest: Option<PathBuf> = None;
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(order_key) = override_manifest_order_key(&name) else {
            continue;
        };
        let path = entry.path();
        let replace = newest
            .as_ref()
            .and_then(|current| current.file_name())
            .is_none_or(|current| {
                let current = current.to_string_lossy();
                override_manifest_order_key(&current).is_none_or(|current| order_key > current)
            });
        if replace {
            newest = Some(path);
        }
    }
    Ok(newest)
}

fn override_manifest_order_key(name: &str) -> Option<&str> {
    let key = name
        .strip_prefix("active-")
        .or_else(|| name.strip_prefix("state-"))?;
    key.ends_with(".json").then_some(key)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sanitize_source_url(value: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(value) else {
        return String::new();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn create_private_dir_all(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_text_atomic(path: &Path, text: &str) -> Result<()> {
    write_bytes_atomic(path, text.as_bytes())
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    write_bytes_atomic(path, &serde_json::to_vec_pretty(value)?)
}

fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir_all(parent)?;
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("record");
    let temporary = path.with_file_name(format!(".{name}.{}.tmp", Uuid::new_v4()));
    let write_result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    write_result
}
