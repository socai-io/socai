use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use chromiumoxide::cdp::browser_protocol::accessibility::EnableParams;
use chromiumoxide::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, InsertTextParams,
};
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::layout::Point;
use chromiumoxide::page::ScreenshotParams;
use chromiumoxide::types::{Command, Method, MethodId};
use chromiumoxide::Page;
use serde::Serialize;
use serde_json::Value;

/// Raw `Accessibility.getFullAXTree` command whose response is left as
/// untyped JSON. chromiumoxide's generated `GetFullAxTreeParams` deserializes
/// the reply into its own `AxNode` structs, which lag the live Chrome AX
/// schema and fail (the historical `uninteresting` serde error). We only want
/// the JSON for the debug snapshot, so we skip the typed round-trip entirely.
#[derive(Debug, Serialize)]
struct GetFullAxTreeRaw {}

impl Method for GetFullAxTreeRaw {
    fn identifier(&self) -> MethodId {
        "Accessibility.getFullAXTree".into()
    }
}

impl Command for GetFullAxTreeRaw {
    type Response = Value;
}

use super::snapshot::SnapshotRecorder;

/// A tab-scoped session. Wraps a chromiumoxide `Page` with the small set of
/// primitives the agent layer needs: `evaluate_json` (the JS-extractor entry
/// point), `navigate`, and `page_info`. Higher-level tools (selector waits,
/// click-by-selector, fill, etc.) live in the sites module.
///
/// `recorder` is an optional debug hook: when set (via `--debug-snapshot`),
/// every `evaluate_json` — the universal perception point for the tools —
/// first lets the recorder capture a DOM/a11y/screenshot bundle if the page
/// changed since the last capture. See [`SnapshotRecorder`].
pub struct PageSession {
    page: Page,
    recorder: StdMutex<Option<Arc<SnapshotRecorder>>>,
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
    pub(crate) fn new(page: Page) -> Self {
        Self {
            page,
            recorder: StdMutex::new(None),
        }
    }

    pub fn target_id(&self) -> &str {
        self.page.target_id().inner()
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

    /// Let an attached recorder capture the page *before* an operation runs.
    /// Called at the top of every action (`navigate`, `click`, `type_text`,
    /// `press_key`) and every `evaluate_json`, so the snapshot timeline has a
    /// frame for each tool operation, showing the state it acted on. Cheap and
    /// a no-op when no recorder is attached; content-deduped downstream.
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
        tokio::time::timeout(timeout, self.page.goto(url)).await??;
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
    ///
    /// This is the tools' universal perception point. Before running the
    /// caller's snippet, it lets an attached [`SnapshotRecorder`] capture the
    /// *current* DOM — i.e. the state the tool is about to read or act on —
    /// gated on whether the page changed since the last capture. Capturing
    /// *before* (not after) means the snapshot reflects exactly what the tool
    /// saw going into this operation; any change the operation causes is
    /// captured before the next one.
    pub async fn evaluate_json(&self, expression: &str) -> anyhow::Result<Value> {
        self.snapshot_before().await;
        self.evaluate_json_raw(expression).await
    }

    /// Uninstrumented `evaluate_json`. Used internally by the snapshot recorder
    /// so its own DOM reads don't recurse back into capture.
    pub(crate) async fn evaluate_json_raw(&self, expression: &str) -> anyhow::Result<Value> {
        let wrapped = wrap_expression(expression);
        let result = self.page.evaluate(wrapped.as_str()).await?;
        let value: Value = result.into_value()?;
        Ok(value)
    }

    /// Turn on the CDP Accessibility domain. Not required for `getFullAXTree`
    /// to return data, but it keeps `AXNodeId`s stable across calls, which makes
    /// the per-node a11y dumps comparable between snapshots. The recorder calls
    /// it once. Idempotent; persists across navigations within the target.
    pub(crate) async fn enable_accessibility(&self) -> anyhow::Result<()> {
        self.page.execute(EnableParams::default()).await?;
        Ok(())
    }

    /// Full accessibility tree for the document as JSON (`{ "nodes": [...] }`).
    /// Used by the snapshot recorder; backed by CDP `Accessibility.getFullAXTree`
    /// with an untyped response (see [`GetFullAxTreeRaw`]).
    pub(crate) async fn ax_tree_json(&self) -> anyhow::Result<Value> {
        let resp = self.page.execute(GetFullAxTreeRaw {}).await?;
        Ok(resp.result)
    }

    pub async fn page_info(&self) -> anyhow::Result<Value> {
        self.evaluate_json(PAGE_INFO_JS).await
    }

    pub async fn click(&self, x: f64, y: f64) -> anyhow::Result<()> {
        self.snapshot_before().await;
        self.page.click(Point::new(x, y)).await?;
        Ok(())
    }

    pub async fn mouse_move(&self, x: f64, y: f64) -> anyhow::Result<()> {
        self.snapshot_before().await;
        self.page.move_mouse(Point::new(x, y)).await?;
        Ok(())
    }

    pub async fn type_text(&self, text: &str) -> anyhow::Result<()> {
        self.snapshot_before().await;
        self.page.execute(InsertTextParams::new(text)).await?;
        Ok(())
    }

    pub async fn press_key(&self, key: &str) -> anyhow::Result<()> {
        self.snapshot_before().await;
        let (vk, code, text) = key_definition(key);
        let base = |event_type| {
            let mut builder = DispatchKeyEventParams::builder()
                .r#type(event_type)
                .key(key)
                .code(code)
                .windows_virtual_key_code(vk)
                .native_virtual_key_code(vk);
            if !text.is_empty() {
                builder = builder.text(text);
            }
            builder.build().map_err(anyhow::Error::msg)
        };
        self.page
            .execute(base(DispatchKeyEventType::KeyDown)?)
            .await?;
        self.page
            .execute(base(DispatchKeyEventType::KeyUp)?)
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
        let params = ScreenshotParams::builder()
            .format(CaptureScreenshotFormat::Jpeg)
            .quality(i64::from(quality))
            .full_page(full)
            .capture_beyond_viewport(full)
            .build();
        Ok(self.page.screenshot(params).await?)
    }

    pub async fn screenshot_png(&self, full: bool) -> anyhow::Result<Vec<u8>> {
        let params = ScreenshotParams::builder()
            .format(CaptureScreenshotFormat::Png)
            .full_page(full)
            .capture_beyond_viewport(full)
            .build();
        Ok(self.page.screenshot(params).await?)
    }

    pub async fn save_screenshot(&self, path: impl AsRef<Path>, full: bool) -> anyhow::Result<()> {
        let bytes = self.screenshot_png(full).await?;
        if let Some(parent) = path.as_ref().parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(path, bytes).await?;
        Ok(())
    }

    /// Close the underlying tab. Consumes the session — the chromiumoxide
    /// page handle is dropped on success.
    pub async fn close(self) -> anyhow::Result<()> {
        self.page.close().await?;
        Ok(())
    }
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
