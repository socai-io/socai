//! Optional page-snapshot recorder for debugging (`--debug-snapshot`).
//!
//! Rather than sampling on a fixed timer, the recorder hooks *before* every
//! tool operation (each `evaluate_json`, plus `navigate`/`click`/`type_text`/
//! `press_key`). The result is a timeline aligned to what the tool actually
//! did: every search, scroll, note-open, and the intermediate frames of
//! multi-second waits.
//!
//! Three gates keep the timeline lean, cheapest first:
//!
//! 1. A page-injected `MutationObserver` version counter — if the DOM hasn't
//!    mutated at all since the last poll, skip without reading anything.
//! 2. A **DOM hash** — if the serialized DOM is identical to the last frame we
//!    examined, skip before taking a screenshot or reading the a11y tree
//!    (e.g. the byte-identical skeleton frames a feed cycles through while
//!    hydrating).
//! 3. A **screenshot hash** — the DOM changed, but this framework simulates a
//!    human driving the browser, so the real question is "would a human see
//!    anything different?" If the viewport renders byte-identically to the last
//!    written node, skip the frame anyway (e.g. a navigation whose new page
//!    hasn't painted yet — different DOM/URL, same blank screen).
//!
//! Each capture node bundles three views taken back-to-back under one shared
//! sequence number + timestamp, so DOM / a11y / screenshot line up for
//! side-by-side debugging:
//!
//! ```text
//! <run_dir>/snapshots/
//!   index.jsonl                     # one line per node
//!   00001_153012_482/
//!     dom.html                      # document.documentElement.outerHTML
//!     a11y.json                     # slimmed getFullAXTree (screen-reader view)
//!     screenshot.png                # viewport PNG
//!   00002_153013_771/
//!     ...
//! ```
//!
//! Every perception ultimately reads the live DOM (selectors,
//! `getBoundingClientRect`, `innerText`), so this is a faithful record of what
//! the tools "saw" at each step.

use std::future::Future;
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::Mutex;

use super::session::PageSession;

/// Run `fut` with a [`SnapshotRecorder`] attached to `page` for its duration,
/// writing into `<run_dir>/snapshots`. A no-op when `enabled` is false.
///
/// This is the entire per-command surface for snapshot recording: any site's
/// command runner can wrap its body in this call. The recorder hooks
/// `evaluate_json` at the generic CDP layer, so the captured timeline needs no
/// site-specific knowledge. The recorder is detached and a terminal frame
/// flushed even if `fut` returns an error.
pub async fn with_snapshot_recording<F, T>(
    page: &PageSession,
    run_dir: &Path,
    enabled: bool,
    fut: F,
) -> T
where
    F: Future<Output = T>,
{
    let recorder = enabled.then(|| {
        let recorder = Arc::new(SnapshotRecorder::new(run_dir.join("snapshots")));
        page.set_recorder(recorder.clone());
        recorder
    });

    let out = fut.await;

    if let Some(recorder) = recorder {
        recorder.finish(page).await;
        page.clear_recorder();
    }
    out
}

/// Installs a `MutationObserver` (once per page world) that bumps a version
/// counter on any subtree/attribute/text mutation, and returns the current
/// version plus URL. After a navigation the page world resets, so `installed`
/// is false again and the observer is re-installed; the version restarts from
/// 1, which the recorder detects as a change (via the URL or version delta).
/// Written as an IIFE so `evaluate_json_raw` does not wrap it.
const WATCH_JS: &str = r#"
(function () {
  var w = window.__socaiDomWatch;
  if (!w) {
    w = window.__socaiDomWatch = { v: 1, installed: false };
  }
  if (!w.installed) {
    try {
      var root = document.documentElement || document;
      var obs = new MutationObserver(function () { w.v++; });
      obs.observe(root, { subtree: true, childList: true, attributes: true, characterData: true });
      w.installed = true;
    } catch (e) {}
  }
  return { v: w.v, url: location.href };
})()
"#;

/// Returns the full serialized DOM plus URL.
const SNAPSHOT_JS: &str = r#"
return {
  html: document.documentElement ? document.documentElement.outerHTML : '',
  url: location.href
};
"#;

/// Records DOM/a11y/screenshot bundles whenever the page changes between tool
/// operations. Attach to a [`PageSession`] via
/// [`PageSession::set_recorder`](super::session::PageSession::set_recorder).
pub struct SnapshotRecorder {
    dir: PathBuf,
    state: Mutex<RecorderState>,
}

struct RecorderState {
    initialized: bool,
    a11y_enabled: bool,
    seq: u64,
    last_version: i64,
    last_url: String,
    /// Hash of the last DOM we examined. Lets consecutive identical DOMs skip
    /// before the (more expensive) screenshot and a11y reads.
    last_dom_hash: Option<u64>,
    /// Hash of the last written node's screenshot. Frames whose DOM changed but
    /// render to a pixel-identical viewport are skipped — a human wouldn't
    /// perceive them as a new state.
    last_shot_hash: Option<u64>,
}

impl SnapshotRecorder {
    /// `dir` is the snapshots root (e.g. `<run_dir>/snapshots`); created lazily
    /// on the first captured node.
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            state: Mutex::new(RecorderState {
                initialized: false,
                a11y_enabled: false,
                seq: 0,
                last_version: -1,
                last_url: String::new(),
                last_dom_hash: None,
                last_shot_hash: None,
            }),
        }
    }

    /// Capture the current page if it changed since the last node. Called
    /// before each tool operation.
    pub async fn before_operation(&self, page: &PageSession) {
        let mut state = self.state.lock().await;
        self.capture(&mut state, page, false).await;
    }

    /// Capture the terminal page state once, after the command's last
    /// operation and before the page is closed/reused. By this point the data
    /// the command produced is already in the DOM (the tool just read it), so a
    /// single capture suffices; the `force` flag bypasses the version gate so
    /// this post-operation state is examined even if no further mutation fired.
    pub async fn finish(&self, page: &PageSession) {
        let mut state = self.state.lock().await;
        self.capture(&mut state, page, true).await;
    }

    async fn capture(&self, state: &mut RecorderState, page: &PageSession, force: bool) {
        // Cheap version probe (raw eval → no recursion into the recorder).
        let probe = page.evaluate_json_raw(WATCH_JS).await.ok();
        let version = probe
            .as_ref()
            .and_then(|v| v.get("v"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let url = probe
            .as_ref()
            .and_then(|v| v.get("url"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        // First gate: nothing mutated since the last poll → skip without even
        // taking a screenshot. Mark version/url as seen so an identical next
        // poll also early-outs here.
        let version_changed =
            !(state.initialized && version == state.last_version && url == state.last_url);
        if !version_changed && !force {
            return;
        }
        state.initialized = true;
        state.last_version = version;
        state.last_url = url.clone();

        // Second gate (cheap): the DOM mutated, but is the serialized DOM
        // actually different from the last frame we examined? If not, skip
        // before paying for a screenshot or the a11y tree.
        let dom = page
            .evaluate_json_raw(SNAPSHOT_JS)
            .await
            .ok()
            .and_then(|v| v.get("html").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_default();
        let dom_hash = hash_bytes(dom.as_bytes());
        if state.last_dom_hash == Some(dom_hash) {
            return;
        }
        state.last_dom_hash = Some(dom_hash);

        // Third gate: the DOM differs, but does the page *look* any different?
        // Take the viewport screenshot and skip the node if it's pixel-identical
        // to the last one written. A failed screenshot can't be deduped, so fall
        // through and capture the node anyway.
        let shot = page.screenshot_png(false).await.ok();
        if let Some(bytes) = &shot {
            let shot_hash = hash_bytes(bytes);
            if state.last_shot_hash == Some(shot_hash) {
                return;
            }
            state.last_shot_hash = Some(shot_hash);
        }

        // Enable the Accessibility domain once so AXNodeIds stay stable across
        // nodes (not required for getFullAXTree to return data).
        if !state.a11y_enabled && page.enable_accessibility().await.is_ok() {
            state.a11y_enabled = true;
        }

        state.seq += 1;
        let seq = state.seq;

        if let Err(err) = self
            .write_node(page, seq, &url, &dom, shot.as_deref())
            .await
        {
            tracing::warn!("debug-snapshot: node {seq} failed: {err:#}");
        }
    }

    async fn write_node(
        &self,
        page: &PageSession,
        seq: u64,
        url: &str,
        dom: &str,
        shot: Option<&[u8]>,
    ) -> anyhow::Result<()> {
        // One timestamp + one folder for all three views, so they share a
        // synchronized timepoint label. DOM and screenshot were captured moments
        // ago (for the dedup gates); the a11y tree is read here.
        let now = chrono::Local::now();
        let node_name = format!("{:05}_{}", seq, now.format("%H%M%S_%3f"));
        let node_dir = self.dir.join(&node_name);
        tokio::fs::create_dir_all(&node_dir).await?;

        tokio::fs::write(node_dir.join("dom.html"), dom).await?;

        let (a11y_ok, a11y_nodes) = match page.ax_tree_json().await {
            Ok(ax) => {
                // getFullAXTree is a raw debug dump: every node (incl. ignored)
                // plus protocol metadata (name.sources, chromeRole, frameId, …).
                // Slim it to the screen-reader view — role + name + states +
                // structure — which is ~5x smaller.
                let tree = compact_ax_tree(&ax);
                let nodes = tree
                    .get("nodes")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0);
                let pretty = serde_json::to_vec_pretty(&tree)?;
                tokio::fs::write(node_dir.join("a11y.json"), pretty).await?;
                (true, nodes)
            }
            Err(err) => {
                tracing::warn!("debug-snapshot: a11y for node {seq} failed: {err:#}");
                (false, 0)
            }
        };

        if let Some(bytes) = shot {
            tokio::fs::write(node_dir.join("screenshot.png"), bytes).await?;
        }

        let entry = serde_json::json!({
            "seq": seq,
            "ts": now.to_rfc3339(),
            "url": url,
            "dir": node_name,
            "dom_bytes": dom.len(),
            "a11y_nodes": a11y_nodes,
            "has_a11y": a11y_ok,
            "has_screenshot": shot.is_some(),
        });
        append_jsonl(&self.dir.join("index.jsonl"), &entry).await
    }
}

/// Reduce a raw `getFullAXTree` reply to the semantic tree a screen reader
/// actually navigates: drop `ignored` nodes and keep, per node, only the role,
/// accessible name, value, meaningful states, and child links. Structure is
/// preserved via `children` (the original `AXNodeId`s).
fn compact_ax_tree(ax: &Value) -> Value {
    let nodes = ax
        .get("nodes")
        .and_then(Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .filter(|n| !n.get("ignored").and_then(Value::as_bool).unwrap_or(false))
                .map(compact_ax_node)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    serde_json::json!({ "nodes": nodes })
}

fn compact_ax_node(node: &Value) -> Value {
    let mut out = serde_json::Map::new();

    if let Some(id) = node.get("nodeId").cloned() {
        out.insert("id".into(), id);
    }
    if let Some(role) = node.get("role").and_then(|r| r.get("value")).cloned() {
        out.insert("role".into(), role);
    }
    if let Some(name) = node.get("name").and_then(|n| n.get("value")) {
        if name.as_str() != Some("") {
            out.insert("name".into(), name.clone());
        }
    }
    if let Some(value) = node.get("value").and_then(|v| v.get("value")) {
        if value.as_str() != Some("") {
            out.insert("value".into(), value.clone());
        }
    }

    // States/properties, flattened to `{ name: value }`, dropping the noisy
    // false/empty defaults that dominate the raw tree.
    let mut states = serde_json::Map::new();
    if let Some(props) = node.get("properties").and_then(Value::as_array) {
        for prop in props {
            let Some(name) = prop.get("name").and_then(Value::as_str) else {
                continue;
            };
            let Some(value) = prop.get("value").and_then(|v| v.get("value")) else {
                continue;
            };
            if value.as_bool() == Some(false) || value.as_str() == Some("") {
                continue;
            }
            states.insert(name.to_string(), value.clone());
        }
    }
    if !states.is_empty() {
        out.insert("states".into(), Value::Object(states));
    }

    if let Some(children) = node.get("childIds").and_then(Value::as_array) {
        if !children.is_empty() {
            out.insert("children".into(), Value::Array(children.clone()));
        }
    }

    Value::Object(out)
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(bytes);
    hasher.finish()
}

async fn append_jsonl(path: &Path, entry: &Value) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut line = serde_json::to_string(entry)?;
    line.push('\n');
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.write_all(line.as_bytes()).await?;
    Ok(())
}
