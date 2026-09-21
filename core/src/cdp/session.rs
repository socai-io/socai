use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde_json::{json, Value};

use super::connection::Cdp;
use super::raw_client::RawCdpClient;
use super::snapshot::SnapshotRecorder;

/// A tab-scoped session. Unlike the previous chromiumoxide-backed
/// implementation, this sends only the commands socai needs to one owned page
/// target. It does not enable Network/Page/Runtime event domains globally and
/// does not auto-attach to unrelated browser tabs.
///
/// `recorder` is an optional debug hook: when set (via `--debug-snapshot`),
/// every `evaluate_json` — the universal perception point for the tools —
/// first lets the recorder capture a DOM/a11y/screenshot bundle if the page
/// changed since the last capture. See [`SnapshotRecorder`].
pub struct PageSession {
    owner: Cdp,
    connection: tokio::sync::RwLock<PageConnection>,
    /// Whether the browser backing the current connection is a remote hosted
    /// one. Updated together with the connection during recovery: asking the
    /// shared `Cdp` later could report a different browser during a swap.
    remote_browser: AtomicBool,
    recorder: StdMutex<Option<Arc<SnapshotRecorder>>>,
    /// Background control tabs are owned by this handle and must be closed if
    /// a cancelled browser script drops the handle before explicit cleanup.
    close_on_drop: AtomicBool,
}

struct PageConnection {
    target_id: String,
    client: RawCdpClient,
    session_id: Option<String>,
}

/// One immutable page-session generation used by a browser-script evaluation.
/// Keeping the client/session pair together prevents cancellation cleanup from
/// targeting a replacement page after CDP recovery rebinds `PageSession`.
#[derive(Clone)]
pub(crate) struct PageJavascriptSession {
    client: RawCdpClient,
    session_id: Option<String>,
}

impl PageJavascriptSession {
    async fn execute(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        self.client
            .execute_for_session(self.session_id.as_deref(), method, params)
            .await
    }

    pub(crate) async fn evaluate_json_in_isolated_world_with_timeout(
        &self,
        expression: &str,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        let frame_tree = self.execute("Page.getFrameTree", json!({})).await?;
        let frame_id = frame_tree
            .pointer("/frameTree/frame/id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Page.getFrameTree missing main frame id"))?;
        let world = self
            .execute(
                "Page.createIsolatedWorld",
                json!({
                    "frameId": frame_id,
                    "worldName": "socai-browser-script",
                    "grantUniveralAccess": false,
                }),
            )
            .await?;
        let context_id = world
            .get("executionContextId")
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow!("Page.createIsolatedWorld missing executionContextId"))?;
        evaluate_json_on_session(
            &self.client,
            self.session_id.as_deref(),
            expression,
            timeout,
            Some(context_id),
        )
        .await
    }

    pub(crate) async fn terminate_javascript(&self) -> anyhow::Result<()> {
        self.execute("Runtime.terminateExecution", json!({}))
            .await?;
        Ok(())
    }

    /// Read the committed top-frame URL without depending on a JavaScript
    /// execution context. This remains usable while navigation is replacing
    /// the document and is therefore suitable for browser-script authority
    /// enforcement.
    pub(crate) async fn top_frame_url(&self) -> anyhow::Result<String> {
        let history = self.execute("Page.getNavigationHistory", json!({})).await?;
        let index = history
            .get("currentIndex")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("Page.getNavigationHistory missing currentIndex"))?;
        history
            .get("entries")
            .and_then(Value::as_array)
            .and_then(|entries| entries.get(index as usize))
            .and_then(|entry| entry.get("url"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow!("Page.getNavigationHistory missing current entry URL"))
    }

    pub(crate) async fn stop_loading(&self) -> anyhow::Result<()> {
        self.execute("Page.stopLoading", json!({})).await?;
        Ok(())
    }

    pub(crate) async fn navigate(&self, url: &str) -> anyhow::Result<()> {
        let response = self.execute("Page.navigate", json!({ "url": url })).await?;
        if let Some(error) = response.get("errorText").and_then(Value::as_str) {
            anyhow::bail!("navigation to {url} failed: {error}");
        }
        Ok(())
    }
}

const PAGE_INFO_JS: &str = r#"
return {
  url: location.href,
  title: document.title,
  w: innerWidth,
  h: innerHeight,
  sx: scrollX,
  sy: scrollY,
  pw: document.documentElement.scrollWidth,
  ph: document.documentElement.scrollHeight,
  readyState: document.readyState
};
"#;

impl PageSession {
    pub(crate) fn attached(
        target_id: String,
        client: RawCdpClient,
        session_id: String,
        owner: Cdp,
        remote_browser: bool,
        close_on_drop: bool,
    ) -> Self {
        Self {
            owner,
            connection: tokio::sync::RwLock::new(PageConnection {
                target_id,
                client,
                session_id: Some(session_id),
            }),
            remote_browser: AtomicBool::new(remote_browser),
            recorder: StdMutex::new(None),
            close_on_drop: AtomicBool::new(close_on_drop),
        }
    }

    pub async fn target_id(&self) -> String {
        self.connection.read().await.target_id.clone()
    }

    /// A separately owned tab on the same browser/profile. Dropping its guard
    /// closes only that target, including when a branch is cancelled.
    pub async fn new_sibling(&self) -> anyhow::Result<super::pages::OwnedPage> {
        let page = super::pages::OwnedPage::new(
            super::pages::PageSessionManager::new(self.owner.clone())
                .create_page("about:blank")
                .await?,
        );
        // Background tabs otherwise defer mouse input acknowledgements by ~5s.
        // Scoped to our temporary worker, without bringing user windows to front.
        let active = page.page();
        let result = active
            .execute(
                "Emulation.setFocusEmulationEnabled",
                json!({"enabled":true}),
            )
            .await;
        super::diagnostics::record(
            "worker_focus_emulation",
            json!({"page":active.transport_diagnostic().await,"enabled":result.is_ok()}),
        );
        if let Err(error) = result {
            tracing::warn!(%error,"worker focus emulation unavailable");
        }
        Ok(page)
    }

    /// True when this page was created in a remote hosted browser (socai pro
    /// `chrome.profile remote`) rather than any local chrome.
    pub fn is_remote_browser(&self) -> bool {
        self.remote_browser.load(Ordering::Acquire)
    }

    /// True once the websocket command loop backing this page has ended.
    /// Target closure keeps the browser websocket alive and therefore remains
    /// distinguishable from a recoverable whole-CDP transport loss.
    pub async fn transport_closed(&self) -> bool {
        let connection = self.connection.read().await;
        connection
            .client
            .is_session_closed(connection.session_id.as_deref())
    }

    pub async fn transport_diagnostic(&self) -> Value {
        let c = self.connection.read().await;
        json!({"target_id":c.target_id,"session_id":c.session_id,"health":c.client.health_diagnostic()})
    }

    /// Rebind this stable page handle to a freshly-created target. All tools
    /// for a running agent hold the outer `Arc<PageSession>`; swapping only
    /// the connection lets a retried tool transparently use the new browser
    /// without rebuilding the tool registry or losing the agent transcript.
    pub(crate) async fn replace_connection(&self, replacement: PageSession) {
        let replacement_remote = replacement.remote_browser.load(Ordering::Acquire);
        let replacement_connection = {
            let connection = replacement.connection.read().await;
            PageConnection {
                target_id: connection.target_id.clone(),
                client: connection.client.clone(),
                session_id: connection.session_id.clone(),
            }
        };
        replacement.close_on_drop.store(false, Ordering::Release);
        let old_target_id = {
            let mut connection = self.connection.write().await;
            let old_target_id = connection.target_id.clone();
            *connection = replacement_connection;
            old_target_id
        };
        self.remote_browser
            .store(replacement_remote, Ordering::Release);
        super::diagnostics::record(
            "page_rebound",
            json!({"old_target_id":old_target_id,"replacement":self.transport_diagnostic().await}),
        );
        self.owner.unregister_owned_target(&old_target_id).await;
    }

    /// Attach a debug snapshot recorder. Captures begin on the next
    /// `evaluate_json`. Replacing or clearing it is cheap and lock-guarded.
    pub fn set_recorder(&self, recorder: Arc<SnapshotRecorder>) {
        if let Ok(mut guard) = self.recorder.lock() {
            *guard = Some(recorder);
        }
    }

    pub fn clear_recorder(&self) {
        if let Ok(mut guard) = self.recorder.lock() {
            *guard = None;
        }
    }

    fn recorder(&self) -> Option<Arc<SnapshotRecorder>> {
        self.recorder.lock().ok().and_then(|guard| guard.clone())
    }

    async fn execute(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let (client, session_id) = {
            let connection = self.connection.read().await;
            (connection.client.clone(), connection.session_id.clone())
        };
        client
            .execute_for_session(session_id.as_deref(), method, params)
            .await
    }

    /// Let an attached recorder capture the page *before* an operation runs.
    async fn snapshot_before(&self) {
        if let Some(recorder) = self.recorder() {
            recorder.before_operation(self).await;
        }
    }

    /// Navigate to `url` and wait for DOM readiness.
    pub async fn navigate(&self, url: &str) -> anyhow::Result<()> {
        self.navigate_with_timeout(url, 15.0).await
    }

    pub async fn navigate_with_timeout(
        &self,
        url: &str,
        timeout_seconds: f64,
    ) -> anyhow::Result<()> {
        self.snapshot_before().await;
        let timeout = seconds(timeout_seconds);
        let resp = tokio::time::timeout(
            timeout,
            self.execute("Page.navigate", json!({ "url": url })),
        )
        .await
        .map_err(|_| anyhow!("Page.navigate timed out after {timeout_seconds}s"))??;
        // `Page.navigate` reports navigation failures (DNS, net::ERR_*, blocked)
        // in an `errorText` field rather than as a CDP error — chromiumoxide's
        // `goto` surfaced these, so check it to preserve that behavior.
        if let Some(err) = resp.get("errorText").and_then(Value::as_str) {
            anyhow::bail!("navigation to {url} failed: {err}");
        }
        self.wait_for_load_state("domcontentloaded", timeout_seconds)
            .await?;
        Ok(())
    }

    pub async fn wait_for_load_state(
        &self,
        state: &str,
        timeout_seconds: f64,
    ) -> anyhow::Result<bool> {
        let target = state.to_ascii_lowercase();
        let deadline = Instant::now() + seconds(timeout_seconds);
        while Instant::now() < deadline {
            let ready = self
                .evaluate_json("document.readyState")
                .await
                .ok()
                .and_then(|v| v.as_str().map(ToOwned::to_owned))
                .unwrap_or_default();
            if matches!(target.as_str(), "domcontentloaded" | "interactive")
                && matches!(ready.as_str(), "interactive" | "complete")
            {
                return Ok(true);
            }
            if matches!(target.as_str(), "load" | "complete") && ready == "complete" {
                return Ok(true);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Ok(false)
    }

    /// Evaluate a JS snippet and deserialize its return value as
    /// `serde_json::Value`. The expression is wrapped in an IIFE when it
    /// contains a top-level `return`, so callers can pass function-body style
    /// snippets.
    pub async fn evaluate_json(&self, expression: &str) -> anyhow::Result<Value> {
        self.snapshot_before().await;
        self.evaluate_json_raw(expression).await
    }

    /// Evaluate JavaScript with a caller-selected CDP response timeout. The
    /// default CDP command timeout remains appropriate for page probes; a
    /// browser-script control program can legitimately span several awaited
    /// navigation and extraction operations.
    pub async fn evaluate_json_with_timeout(
        &self,
        expression: &str,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        self.snapshot_before().await;
        self.evaluate_json_raw_with_timeout(expression, timeout)
            .await
    }

    /// Evaluate in a fresh isolated world for the main frame. DOM nodes remain
    /// visible, but page scripts cannot replace the JavaScript intrinsics used
    /// by the browser-script result envelope and size checks.
    pub async fn evaluate_json_in_isolated_world_with_timeout(
        &self,
        expression: &str,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        self.javascript_session()
            .await
            .evaluate_json_in_isolated_world_with_timeout(expression, timeout)
            .await
    }

    pub(crate) async fn javascript_session(&self) -> PageJavascriptSession {
        self.snapshot_before().await;
        let connection = self.connection.read().await;
        PageJavascriptSession {
            client: connection.client.clone(),
            session_id: connection.session_id.clone(),
        }
    }

    /// Stop JavaScript currently running in this target. Used by the local
    /// browser-script escape hatch when its own shorter timeout expires, so a
    /// runaway loop cannot leave the controlled tab permanently frozen.
    pub async fn terminate_javascript(&self) -> anyhow::Result<()> {
        self.execute("Runtime.terminateExecution", json!({}))
            .await?;
        Ok(())
    }

    /// Create a sibling tab on the same browser connection. Browser-script
    /// programs use a short-lived blank sibling as their stable JavaScript
    /// control context, so the program survives navigation in the site tab.
    pub async fn create_sibling(&self, start_url: &str) -> anyhow::Result<PageSession> {
        super::pages::PageSessionManager::new(self.owner.clone())
            .create_page(start_url)
            .await
    }

    /// Create a sibling target without focusing it.
    pub async fn create_background_sibling(&self, start_url: &str) -> anyhow::Result<PageSession> {
        super::pages::PageSessionManager::new(self.owner.clone())
            .create_background_page(start_url)
            .await
    }

    /// Uninstrumented `evaluate_json`. Used internally by the snapshot recorder
    /// so its own DOM reads don't recurse back into capture.
    pub(crate) async fn evaluate_json_raw(&self, expression: &str) -> anyhow::Result<Value> {
        self.evaluate_json_raw_with_timeout(expression, Duration::from_secs(30))
            .await
    }

    async fn evaluate_json_raw_with_timeout(
        &self,
        expression: &str,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        self.evaluate_json_raw_with_timeout_in_context(expression, timeout, None)
            .await
    }

    async fn evaluate_json_raw_with_timeout_in_context(
        &self,
        expression: &str,
        timeout: Duration,
        context_id: Option<i64>,
    ) -> anyhow::Result<Value> {
        let (client, session_id) = {
            let connection = self.connection.read().await;
            (connection.client.clone(), connection.session_id.clone())
        };
        evaluate_json_on_session(
            &client,
            session_id.as_deref(),
            expression,
            timeout,
            context_id,
        )
        .await
    }

    /// Turn on the CDP Accessibility domain. The recorder calls it once when
    /// debug snapshots are enabled. This is intentionally opt-in because AX tree
    /// collection can be expensive on large pages.
    pub(crate) async fn enable_accessibility(&self) -> anyhow::Result<()> {
        self.execute("Accessibility.enable", json!({})).await?;
        Ok(())
    }

    /// Full accessibility tree for the document as JSON (`{ "nodes": [...] }`).
    pub(crate) async fn ax_tree_json(&self) -> anyhow::Result<Value> {
        self.execute("Accessibility.getFullAXTree", json!({})).await
    }

    pub async fn page_info(&self) -> anyhow::Result<Value> {
        self.evaluate_json(PAGE_INFO_JS).await
    }

    /// Fetch a same-session resource through the attached page and stream it
    /// into `destination` in bounded chunks. This preserves browser cookies,
    /// Origin and other session state for CDNs that reject an independent HTTP
    /// client without holding the whole response in memory.
    pub async fn fetch_file_with_browser(
        &self,
        url: &str,
        max_bytes: usize,
        destination: &Path,
        allowed_https_host_suffixes: &[&str],
    ) -> anyhow::Result<(String, String)> {
        const CHUNK_BYTES: usize = 1024 * 1024;

        if max_bytes == 0 {
            anyhow::bail!("browser resource byte limit must be greater than zero");
        }
        if allowed_https_host_suffixes.is_empty()
            || !url_matches_https_host_suffixes(url, allowed_https_host_suffixes)
        {
            anyhow::bail!("browser resource URL is outside the allowed HTTPS host set");
        }
        let encoded_url = serde_json::to_string(url)?;
        let encoded_host_suffixes = serde_json::to_string(allowed_https_host_suffixes)?;
        let fetch_id = uuid::Uuid::new_v4().to_string();
        let encoded_id = serde_json::to_string(&fetch_id)?;
        let result = async {
            let start_expression = format!(
                r#"
return (async () => {{
  const key = {encoded_id};
  const registry = globalThis.__socaiResourceFetches ||= new Map();
  const allowedHostSuffixes = {encoded_host_suffixes};
  const allowedUrl = (raw) => {{
    try {{
      const candidate = new URL(raw);
      const host = candidate.hostname.toLowerCase();
      return candidate.protocol === "https:" && !candidate.username && !candidate.password &&
        !candidate.port && allowedHostSuffixes.some((suffix) =>
          host === suffix || host.endsWith(`.${{suffix}}`));
    }} catch (_) {{
      return false;
    }}
  }};
  const response = await fetch({encoded_url}, {{
    credentials: "include",
    signal: AbortSignal.timeout(115000),
  }});
  const finalUrl = response.url || "";
  if (!allowedUrl(finalUrl)) {{
    if (response.body) await response.body.cancel();
    return {{
      status: response.status,
      error: "redirect target is outside the allowed HTTPS host set",
      final_url: finalUrl,
    }};
  }}
  const contentLength = Number(response.headers.get("content-length") || 0);
  if (contentLength > {max_bytes}) {{
    if (response.body) await response.body.cancel();
    return {{ status: response.status, too_large: true, content_length: contentLength }};
  }}
  if (!response.body) {{
    return {{ status: response.status, error: "response body is not streamable" }};
  }}
  const state = {{ reader: response.body.getReader(), total: 0, expiry: 0 }};
  state.expiry = setTimeout(async () => {{
    if (registry.get(key) !== state) return;
    try {{ await state.reader.cancel(); }} catch (_) {{}}
    registry.delete(key);
  }}, 120000);
  registry.set(key, state);
  return {{
    status: response.status,
    content_type: response.headers.get("content-type") || "",
    final_url: finalUrl,
  }};
}})();
"#
            );
            let metadata = self
                .evaluate_json_raw_with_timeout(&start_expression, Duration::from_secs(120))
                .await?;
            let status = metadata.get("status").and_then(Value::as_u64).unwrap_or(0);
            if !matches!(status, 200 | 206) {
                anyhow::bail!("browser resource fetch returned HTTP {status}");
            }
            if metadata.get("too_large").and_then(Value::as_bool) == Some(true) {
                anyhow::bail!("browser resource exceeds the {max_bytes} byte limit");
            }
            if let Some(error) = metadata.get("error").and_then(Value::as_str) {
                anyhow::bail!("browser resource fetch failed: {error}");
            }
            let content_type = metadata
                .get("content_type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let final_url = metadata
                .get("final_url")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or(url)
                .to_string();
            if !url_matches_https_host_suffixes(&final_url, allowed_https_host_suffixes) {
                anyhow::bail!("browser resource redirect left the allowed HTTPS host set");
            }

            let mut file = tokio::fs::File::create(destination)
                .await
                .with_context(|| {
                    format!(
                        "failed to create browser resource file {}",
                        destination.display()
                    )
                })?;
            let mut written = 0usize;
            loop {
                let read_expression = format!(
                    r#"
return (async () => {{
  const key = {encoded_id};
  const registry = globalThis.__socaiResourceFetches;
  const state = registry && registry.get(key);
  if (!state) return {{ ok: false, error: "browser fetch state is unavailable" }};
  const chunks = [];
  let length = 0;
  let done = false;
  while (length < {CHUNK_BYTES}) {{
    const item = await state.reader.read();
    if (item.done) {{ done = true; break; }}
    const value = item.value || new Uint8Array();
    if (state.total + value.byteLength > {max_bytes}) {{
      try {{ await state.reader.cancel(); }} catch (_) {{}}
      clearTimeout(state.expiry);
      registry.delete(key);
      return {{ ok: false, too_large: true }};
    }}
    state.total += value.byteLength;
    length += value.byteLength;
    chunks.push(value);
  }}
  const payload = new Uint8Array(length);
  let cursor = 0;
  for (const chunk of chunks) {{ payload.set(chunk, cursor); cursor += chunk.byteLength; }}
  let binary = "";
  for (let offset = 0; offset < payload.length; offset += 0x8000) {{
    binary += String.fromCharCode(...payload.subarray(offset, offset + 0x8000));
  }}
  if (done) {{
    clearTimeout(state.expiry);
    registry.delete(key);
  }}
  return {{ ok: true, done, body: btoa(binary) }};
}})();
"#
                );
                let response = self
                    .evaluate_json_raw_with_timeout(&read_expression, Duration::from_secs(120))
                    .await?;
                if response.get("too_large").and_then(Value::as_bool) == Some(true) {
                    anyhow::bail!("browser resource exceeds the {max_bytes} byte limit");
                }
                if response.get("ok").and_then(Value::as_bool) != Some(true) {
                    let error = response
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("browser resource stream failed");
                    anyhow::bail!("browser resource fetch failed: {error}");
                }
                let body = response
                    .get("body")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("browser resource fetch omitted response bytes"))?;
                let chunk = BASE64
                    .decode(body)
                    .context("failed to decode browser resource bytes")?;
                if written.saturating_add(chunk.len()) > max_bytes {
                    anyhow::bail!("browser resource exceeds the {max_bytes} byte limit");
                }
                tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
                written += chunk.len();
                if response.get("done").and_then(Value::as_bool) == Some(true) {
                    break;
                }
            }
            if written == 0 {
                anyhow::bail!("browser resource fetch returned an empty body");
            }
            tokio::io::AsyncWriteExt::flush(&mut file).await?;
            drop(file);
            Ok((content_type, final_url))
        }
        .await;

        // Best-effort cleanup for HTTP/type/decoding errors. Successful reads
        // remove themselves when the stream reaches EOF.
        let cleanup_expression = format!(
            r#"
return (async () => {{
  const registry = globalThis.__socaiResourceFetches;
  const state = registry && registry.get({encoded_id});
  if (state) {{
    try {{ await state.reader.cancel(); }} catch (_) {{}}
    clearTimeout(state.expiry);
    registry.delete({encoded_id});
  }}
  return true;
}})();
"#
        );
        let _ = self
            .evaluate_json_raw_with_timeout(&cleanup_expression, Duration::from_secs(5))
            .await;
        result
    }

    pub async fn click(&self, x: f64, y: f64) -> anyhow::Result<()> {
        self.snapshot_before().await;
        self.dispatch_mouse("mouseMoved", x, y, "none", 0).await?;
        self.dispatch_mouse("mousePressed", x, y, "left", 1).await?;
        self.dispatch_mouse("mouseReleased", x, y, "left", 1)
            .await?;
        Ok(())
    }

    pub async fn mouse_move(&self, x: f64, y: f64) -> anyhow::Result<()> {
        self.snapshot_before().await;
        self.dispatch_mouse("mouseMoved", x, y, "none", 0).await
    }

    async fn dispatch_mouse(
        &self,
        event_type: &str,
        x: f64,
        y: f64,
        button: &str,
        click_count: i64,
    ) -> anyhow::Result<()> {
        self.execute(
            "Input.dispatchMouseEvent",
            json!({
                "type": event_type,
                "x": x,
                "y": y,
                "button": button,
                "clickCount": click_count,
            }),
        )
        .await?;
        Ok(())
    }

    pub async fn type_text(&self, text: &str) -> anyhow::Result<()> {
        self.snapshot_before().await;
        self.execute("Input.insertText", json!({ "text": text }))
            .await?;
        Ok(())
    }

    /// Type `text` as a stream of per-character key events (keyDown with the
    /// char's `text` payload, then keyUp), the way a human keyboard does.
    /// `type_text`'s single `Input.insertText` fires an `input` event with no
    /// keydown/keyup around it — XHS's search composer treats that signature
    /// as bot input and dead-ends the whole widget (submit click and Enter
    /// both stop responding). Per-char key events keep the page's key-driven
    /// behaviours (suggestion fetch, submit arming) working.
    pub async fn type_chars(&self, text: &str) -> anyhow::Result<()> {
        self.snapshot_before().await;
        for ch in text.chars() {
            let ch = ch.to_string();
            self.execute(
                "Input.dispatchKeyEvent",
                json!({ "type": "keyDown", "key": ch, "text": ch }),
            )
            .await?;
            self.execute(
                "Input.dispatchKeyEvent",
                json!({ "type": "keyUp", "key": ch }),
            )
            .await?;
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        Ok(())
    }

    pub async fn press_key(&self, key: &str) -> anyhow::Result<()> {
        self.snapshot_before().await;
        let (vk, code, text) = key_definition(key);
        let base = |event_type: &str| {
            let mut params = json!({
                "type": event_type,
                "key": key,
                "code": code,
                "windowsVirtualKeyCode": vk,
                "nativeVirtualKeyCode": vk,
            });
            if event_type == "keyDown" && !text.is_empty() {
                params["text"] = Value::String(text.to_string());
            }
            params
        };
        self.execute("Input.dispatchKeyEvent", base("keyDown"))
            .await?;
        self.execute("Input.dispatchKeyEvent", base("keyUp"))
            .await?;
        Ok(())
    }

    pub async fn scroll(&self, delta_y: i64) -> anyhow::Result<()> {
        let expr = format!(
            "window.scrollBy({{left: 0, top: {}, behavior: 'instant'}}); return {{x: scrollX, y: scrollY}};",
            delta_y
        );
        self.evaluate_json(&expr).await?;
        Ok(())
    }

    /// JPEG screenshot (quality 0-100). Web-page captures compress far
    /// better as JPEG than PNG at no practical loss for review purposes.
    pub async fn screenshot_jpeg(&self, full: bool, quality: u32) -> anyhow::Result<Vec<u8>> {
        self.capture_screenshot("jpeg", full, Some(quality)).await
    }

    pub async fn screenshot_png(&self, full: bool) -> anyhow::Result<Vec<u8>> {
        self.capture_screenshot("png", full, None).await
    }

    /// Raw `Page.captureScreenshot`. For full-page captures we clip to the
    /// document's content size (CDP otherwise only returns the viewport). The
    /// reply is base64 PNG/JPEG bytes regardless of format.
    async fn capture_screenshot(
        &self,
        format: &str,
        full: bool,
        quality: Option<u32>,
    ) -> anyhow::Result<Vec<u8>> {
        let mut params = json!({
            "format": format,
            "captureBeyondViewport": full,
            "fromSurface": true,
        });
        if let Some(quality) = quality {
            params["quality"] = json!(quality);
        }
        if full {
            let metrics = self.execute("Page.getLayoutMetrics", json!({})).await?;
            let content = metrics
                .get("contentSize")
                .ok_or_else(|| anyhow!("Page.getLayoutMetrics missing contentSize"))?;
            let width = content.get("width").and_then(Value::as_f64).unwrap_or(1.0);
            let height = content.get("height").and_then(Value::as_f64).unwrap_or(1.0);
            params["clip"] = json!({
                "x": content.get("x").and_then(Value::as_f64).unwrap_or(0.0),
                "y": content.get("y").and_then(Value::as_f64).unwrap_or(0.0),
                "width": width.max(1.0),
                "height": height.max(1.0),
                "scale": 1.0,
            });
        }
        let resp = self.execute("Page.captureScreenshot", params).await?;
        let data = resp
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Page.captureScreenshot missing data"))?;
        BASE64.decode(data).context("failed to decode screenshot")
    }

    pub async fn save_screenshot(&self, path: impl AsRef<Path>, full: bool) -> anyhow::Result<()> {
        let bytes = self.screenshot_png(full).await?;
        if let Some(parent) = path.as_ref().parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(path, bytes).await?;
        Ok(())
    }

    /// Close the underlying tab. Consumes the session.
    pub async fn close(self) -> anyhow::Result<()> {
        self.close_target().await
    }

    pub(crate) async fn close_target(&self) -> anyhow::Result<()> {
        let (target_id, session_id, client) = {
            let connection = self.connection.read().await;
            (
                connection.target_id.clone(),
                connection.session_id.clone(),
                connection.client.clone(),
            )
        };
        // Use the browser websocket that created/attached this PageSession, not
        // whatever browser client the runtime currently holds. The runtime may
        // have disconnected/reconnected or switched profile by the time a
        // session is dropped/cancelled; cleanup should still target the browser
        // that owns this target id.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.execute("Target.closeTarget", json!({ "targetId": target_id })),
        )
        .await;
        self.owner.unregister_owned_target(&target_id).await;
        client.forget_session(session_id.as_deref());
        self.close_on_drop.store(false, Ordering::Release);
        result.context("Closing target timed out")?.map(|_| ())
    }
}

impl Drop for PageSession {
    fn drop(&mut self) {
        if !self.close_on_drop.load(Ordering::Acquire) {
            return;
        }
        let Ok(connection) = self.connection.try_read() else {
            // The owner still tracks this target; an explicit disconnect can
            // sweep it if cancellation races a connection replacement.
            return;
        };
        let target_id = connection.target_id.clone();
        if target_id.is_empty() {
            return;
        }
        let client = connection.client.clone();
        let owner = self.owner.clone();
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            // The owner still tracks this target; an explicit disconnect can
            // sweep it even when no async runtime is available during drop.
            return;
        };
        handle.spawn(async move {
            match client
                .execute("Target.closeTarget", json!({ "targetId": &target_id }))
                .await
            {
                Ok(_) => owner.unregister_owned_target(&target_id).await,
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        target_id,
                        "failed to close dropped page session"
                    );
                }
            }
        });
    }
}

async fn evaluate_json_on_session(
    client: &RawCdpClient,
    session_id: Option<&str>,
    expression: &str,
    timeout: Duration,
    context_id: Option<i64>,
) -> anyhow::Result<Value> {
    let wrapped = wrap_expression(expression);
    let mut params = json!({
        "expression": wrapped,
        "awaitPromise": true,
        "returnByValue": true,
    });
    if let Some(context_id) = context_id {
        params["contextId"] = json!(context_id);
    }
    let resp = client
        .execute_for_session_with_timeout(session_id, "Runtime.evaluate", params, timeout)
        .await?;
    if let Some(exception) = resp.get("exceptionDetails") {
        anyhow::bail!("javascript exception: {}", summarize_exception(exception));
    }
    let result = resp
        .get("result")
        .ok_or_else(|| anyhow!("Runtime.evaluate missing result"))?;
    remote_object_value(result)
}

fn url_matches_https_host_suffixes(value: &str, suffixes: &[&str]) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return false;
    }
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
        return false;
    };
    suffixes.iter().any(|suffix| {
        let suffix = suffix.trim().trim_start_matches('.').to_ascii_lowercase();
        !suffix.is_empty() && (host == suffix || host.ends_with(&format!(".{suffix}")))
    })
}

fn remote_object_value(object: &Value) -> anyhow::Result<Value> {
    if let Some(value) = object.get("value") {
        return Ok(value.clone());
    }
    if object.get("subtype").and_then(Value::as_str) == Some("null") {
        return Ok(Value::Null);
    }
    if object.get("type").and_then(Value::as_str) == Some("undefined") {
        return Ok(Value::Null);
    }
    if let Some(unserializable) = object.get("unserializableValue").and_then(Value::as_str) {
        return Ok(Value::String(unserializable.to_string()));
    }
    Ok(Value::Null)
}

fn summarize_exception(exception: &Value) -> String {
    exception
        .get("exception")
        .and_then(|value| value.get("description").or_else(|| value.get("value")))
        .and_then(Value::as_str)
        .or_else(|| exception.get("text").and_then(Value::as_str))
        .unwrap_or("unknown exception")
        .to_string()
}

fn seconds(value: f64) -> Duration {
    Duration::from_secs_f64(value.max(0.1))
}

fn key_definition(key: &str) -> (i64, &str, &str) {
    match key {
        "Enter" => (13, "Enter", "\r"),
        "Tab" => (9, "Tab", "\t"),
        "Backspace" => (8, "Backspace", ""),
        "Escape" => (27, "Escape", ""),
        "Delete" => (46, "Delete", ""),
        " " => (32, "Space", " "),
        "ArrowLeft" => (37, "ArrowLeft", ""),
        "ArrowUp" => (38, "ArrowUp", ""),
        "ArrowRight" => (39, "ArrowRight", ""),
        "ArrowDown" => (40, "ArrowDown", ""),
        "Home" => (36, "Home", ""),
        "End" => (35, "End", ""),
        "PageUp" => (33, "PageUp", ""),
        "PageDown" => (34, "PageDown", ""),
        _ if key.len() == 1 => (key.as_bytes()[0] as i64, key, key),
        _ => (0, key, ""),
    }
}

fn wrap_expression(expression: &str) -> String {
    let trimmed = expression.trim();
    if has_top_level_return(trimmed) && !trimmed.starts_with('(') {
        format!("(function(){{{}}})()", expression)
    } else {
        expression.to_string()
    }
}

/// Detect a top-level `return` statement, skipping strings, line comments,
/// and block comments. Handles the common case where the user writes
/// multi-line JS with a `return` at the end and expects it to behave like a
/// function body.
///
/// Iterates by char index, not byte index, so multi-byte UTF-8 (e.g. the
/// non-breaking space '\u{a0}' that appears in real-world JS bundles)
/// doesn't trip char-boundary panics on slicing.
fn has_top_level_return(src: &str) -> bool {
    #[derive(Clone, Copy)]
    enum S {
        Code,
        Line,
        Block,
        Str(char),
    }
    let chars: Vec<(usize, char)> = src.char_indices().collect();
    let mut state = S::Code;
    let mut i = 0;
    while i < chars.len() {
        let (byte_idx, c) = chars[i];
        let n = chars.get(i + 1).map(|(_, ch)| *ch).unwrap_or('\0');
        match state {
            S::Code => {
                if c == '"' || c == '\'' || c == '`' {
                    state = S::Str(c);
                    i += 1;
                    continue;
                }
                if c == '/' && n == '/' {
                    state = S::Line;
                    i += 2;
                    continue;
                }
                if c == '/' && n == '*' {
                    state = S::Block;
                    i += 2;
                    continue;
                }
                if c == 'r' && src[byte_idx..].starts_with("return") {
                    let before = if i > 0 { chars[i - 1].1 } else { ' ' };
                    let after = chars.get(i + 6).map(|(_, ch)| *ch).unwrap_or(' ');
                    let before_ok = !(before.is_alphanumeric() || before == '_');
                    let after_ok = !(after.is_alphanumeric() || after == '_');
                    if before_ok && after_ok {
                        return true;
                    }
                }
                i += 1;
            }
            S::Line => {
                if c == '\n' {
                    state = S::Code;
                }
                i += 1;
            }
            S::Block => {
                if c == '*' && n == '/' {
                    state = S::Code;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            S::Str(q) => {
                if c == '\\' {
                    i += 2;
                    continue;
                }
                if c == q {
                    state = S::Code;
                }
                i += 1;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_detected_at_top_level() {
        assert!(has_top_level_return("return 1;"));
        assert!(has_top_level_return("const x = 1; return x;"));
    }

    #[test]
    fn return_inside_string_ignored() {
        assert!(!has_top_level_return("'return inside'"));
        assert!(!has_top_level_return("`return inside`"));
    }

    #[test]
    fn return_inside_comment_ignored() {
        assert!(!has_top_level_return("// return\n"));
        assert!(!has_top_level_return("/* return */"));
    }

    #[test]
    fn return_inside_word_ignored() {
        assert!(!has_top_level_return("noreturn"));
        assert!(!has_top_level_return("return_value"));
    }

    #[test]
    fn handles_non_ascii_chars() {
        // Regression: \u{a0} is non-breaking space (2 bytes in UTF-8). The
        // previous byte-indexing scanner panicked here when slicing through
        // its first byte. The real-world trigger was the XHS page_scripts.js
        // bundle, which uses \u{a0} in a string literal.
        assert!(!has_top_level_return("const s = '\u{a0}';"));
        assert!(has_top_level_return(
            "const s = '\u{a0}'; const x = 1; return x;"
        ));
        assert!(has_top_level_return("// 中文注释\nreturn 1;"));
    }

    #[test]
    fn wrap_preserves_expressions() {
        assert_eq!(wrap_expression("1 + 2"), "1 + 2");
        assert_eq!(wrap_expression("document.title"), "document.title");
    }

    #[test]
    fn wrap_with_return() {
        assert_eq!(
            wrap_expression("return document.title;"),
            "(function(){return document.title;})()"
        );
    }
}
