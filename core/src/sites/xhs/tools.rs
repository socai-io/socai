//! Agent-callable tool wrappers around [`XhsPageRuntime`].
//!
//! Each wrapper owns an `Arc<PageSession>` — the same tab is reused across
//! tool calls so the agent's actions accumulate state (search results
//! visible, note modal open, etc.). The caller is responsible for creating
//! the page and closing it after `run_agent` returns.

use std::sync::Arc;

use crate::agent::{make_run_dir, Backend as LlmProvider, Tool, ToolContext, ToolResult};
use crate::cdp::{with_snapshot_recording, PageSession};
use crate::media::{timing_delta, MediaProcessor, TimingSnapshot};
use async_trait::async_trait;
use serde_json::{json, Map, Value};

use crate::sites::xhs::media_manifest::{
    ensure_entity_note_id, topic_scan_media_manifest, write_media_manifest_file,
};
use crate::sites::xhs::page::XHS_SEARCH_FILTERS;
use crate::sites::xhs::{ReadNoteOptions, XhsHistoryStore, XhsNoteCard, XhsPageRuntime};

/// Default number of notes `topic_scan` reads when the caller doesn't specify.
const DEFAULT_NUM_NOTES: i64 = 10;

/// XHS agent playbook: browser-lock rule, tool inventory, anti-bot rules,
/// page states, entity fields, workflows, reading levels, evidence rules,
/// and Chinese UI hints. Embedded at compile time so the agent prompt always
/// carries the latest copy.
pub const XHS_KNOWLEDGE: &str = include_str!("knowledge.md");

/// All XHS tools constructed against the same page. Convenience helper for
/// the CLI / agent host — just register everything.
pub fn xhs_tools(page: Arc<PageSession>) -> Vec<Arc<dyn Tool>> {
    xhs_tools_with_llm_provider(page, None)
}

pub fn xhs_tools_with_llm_provider(
    page: Arc<PageSession>,
    llm_provider: Option<Arc<dyn LlmProvider>>,
) -> Vec<Arc<dyn Tool>> {
    let history = Arc::new(XhsHistoryStore::open_default());
    vec![
        Arc::new(SearchNotesTool {
            page: page.clone(),
            history: history.clone(),
        }) as Arc<dyn Tool>,
        Arc::new(ExtractSearchCardsTool {
            page: page.clone(),
            history: history.clone(),
        }),
        Arc::new(ListSearchTabsTool { page: page.clone() }),
        Arc::new(ClickSearchTabTool { page: page.clone() }),
        Arc::new(ResetSearchFiltersTool { page: page.clone() }),
        Arc::new(ApplySearchFiltersTool { page: page.clone() }),
        Arc::new(OpenNoteTool { page: page.clone() }),
        Arc::new(CloseNoteTool { page: page.clone() }),
        Arc::new(ReadNoteTool {
            page: page.clone(),
            llm_provider: llm_provider.clone(),
            history: history.clone(),
        }),
        Arc::new(ExtractNoteTool {
            page: page.clone(),
            llm_provider: llm_provider.clone(),
            history: history.clone(),
        }),
        Arc::new(ExtractCommentsTool { page: page.clone() }),
        Arc::new(ScrollInNoteTool { page: page.clone() }),
        Arc::new(CollectCarouselImagesTool { page: page.clone() }),
        Arc::new(ExtractProfileTool { page: page.clone() }),
        Arc::new(TopicScanTool {
            page: page.clone(),
            llm_provider,
            history,
        }),
        Arc::new(PageStateTool { page }),
    ]
}

pub async fn xhs_agent_tools(
    page: Arc<PageSession>,
    llm_provider: Arc<dyn LlmProvider>,
) -> anyhow::Result<Vec<Arc<dyn Tool>>> {
    XhsPageRuntime::new(&page).ensure_xhs(false).await.ok();
    Ok(xhs_tools_with_llm_provider(page, Some(llm_provider)))
}

pub fn xhs_agent_instructions(extra: &str) -> String {
    let base = XHS_KNOWLEDGE.trim().to_string();
    let extra = extra.trim();
    if extra.is_empty() {
        base
    } else {
        format!("{extra}\n\n{base}")
    }
}

#[derive(Clone, Copy)]
enum CommandPageAction {
    None,
    SearchReady,
    CloseOpenNote,
}

#[derive(Clone, Copy)]
struct XhsCommandSpec {
    command_name: &'static str,
    tool_name: &'static str,
    before: CommandPageAction,
    after: CommandPageAction,
}

const SEARCH_NOTES_COMMAND: XhsCommandSpec = XhsCommandSpec {
    command_name: "search_notes",
    tool_name: "search_notes",
    before: CommandPageAction::SearchReady,
    after: CommandPageAction::None,
};

const TOPIC_SCAN_COMMAND: XhsCommandSpec = XhsCommandSpec {
    command_name: "topic_scan",
    tool_name: "topic_scan",
    before: CommandPageAction::SearchReady,
    after: CommandPageAction::None,
};

const EXTRACT_NOTE_COMMAND: XhsCommandSpec = XhsCommandSpec {
    command_name: "extract_note",
    tool_name: "read_note",
    before: CommandPageAction::CloseOpenNote,
    after: CommandPageAction::CloseOpenNote,
};

pub async fn search_notes_command(
    page: Arc<PageSession>,
    query: &str,
    filters: Option<&Value>,
    num_notes: Option<i64>,
    debug_snapshot: bool,
) -> anyhow::Result<Value> {
    run_xhs_tool_command(
        page,
        SEARCH_NOTES_COMMAND,
        search_notes_input(query, filters, num_notes)?,
        debug_snapshot,
    )
    .await
}

pub async fn topic_scan_command(
    page: Arc<PageSession>,
    query: &str,
    tab_label: Option<&str>,
    filters: Option<&Value>,
    num_notes: Option<i64>,
    download_media: bool,
    debug_snapshot: bool,
) -> anyhow::Result<Value> {
    run_xhs_tool_command(
        page,
        TOPIC_SCAN_COMMAND,
        topic_scan_input(query, tab_label, filters, num_notes, download_media)?,
        debug_snapshot,
    )
    .await
}

pub async fn extract_note_command(
    page: Arc<PageSession>,
    note_id: &str,
    debug_snapshot: bool,
) -> anyhow::Result<Value> {
    run_xhs_tool_command(
        page,
        EXTRACT_NOTE_COMMAND,
        extract_note_input(note_id)?,
        debug_snapshot,
    )
    .await
}

fn search_notes_input(
    query: &str,
    filters: Option<&Value>,
    num_notes: Option<i64>,
) -> anyhow::Result<Value> {
    let mut input = json!({
        "query": trimmed_required(query, "query")?,
        "wait_seconds": 2.0,
    });
    if let Some(filters) = filters {
        input["filters"] = filters.clone();
    }
    if let Some(n) = num_notes {
        input["num_notes"] = json!(n.max(1));
    }
    Ok(input)
}

fn topic_scan_input(
    query: &str,
    tab_label: Option<&str>,
    filters: Option<&Value>,
    num_notes: Option<i64>,
    download_media: bool,
) -> anyhow::Result<Value> {
    let mut input = json!({
        "query": trimmed_required(query, "query")?,
    });
    insert_optional_str(&mut input, "tab_label", tab_label);
    if let Some(filters) = filters {
        input["filters"] = filters.clone();
    }
    if let Some(n) = num_notes {
        input["num_notes"] = json!(n.max(1));
    }
    if download_media {
        input["download_media"] = json!(true);
    }
    Ok(input)
}

fn extract_note_input(note_id: &str) -> anyhow::Result<Value> {
    Ok(json!({
        "note_id": trimmed_required(note_id, "note_id")?,
        "wait_seconds": 6.0,
    }))
}

async fn run_xhs_tool_command(
    page: Arc<PageSession>,
    spec: XhsCommandSpec,
    input: Value,
    debug_snapshot: bool,
) -> anyhow::Result<Value> {
    let (run_dir, ctx) = command_context(spec.command_name);
    // Persist the full command input up front (best-effort) so a run is
    // debuggable from its dir alone — including the exact args — even when the
    // tool errors out partway.
    let invocation = json!({
        "command": spec.command_name,
        "tool": spec.tool_name,
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
        apply_command_page_action(spec.before, &page).await?;
        let data = call_xhs_tool(page.clone(), spec.tool_name, input, &ctx).await?;
        apply_command_page_action(spec.after, &page).await?;
        Ok::<Value, anyhow::Error>(data)
    })
    .await?;

    Ok(json!({
        "command": spec.command_name,
        "run_dir": run_dir,
        "input": invocation.get("input").cloned().unwrap_or(Value::Null),
        "data": data,
    }))
}

async fn apply_command_page_action(
    action: CommandPageAction,
    page: &PageSession,
) -> anyhow::Result<()> {
    match action {
        CommandPageAction::None => Ok(()),
        CommandPageAction::SearchReady => ensure_search_ready(page).await,
        CommandPageAction::CloseOpenNote => {
            close_open_note(page).await;
            Ok(())
        }
    }
}

pub async fn ensure_search_ready(page: &PageSession) -> anyhow::Result<()> {
    close_open_note(page).await;
    let runtime = XhsPageRuntime::new(page);
    let state = runtime.detect_state().await.ok();
    let state_name = state
        .as_ref()
        .and_then(|state| state.get("state"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let current_url = runtime.current_url().await.unwrap_or_default();
    if !current_url.contains("xiaohongshu.com") || state_name == "note_detail" {
        page.navigate_with_timeout(crate::sites::xhs::XHS_HOME_URL, 60.0)
            .await?;
    }
    Ok(())
}

pub async fn close_open_note(page: &PageSession) {
    let runtime = XhsPageRuntime::new(page);
    let state = runtime.detect_state().await.ok();
    let note_open = state
        .as_ref()
        .and_then(|state| state.get("note_open"))
        .and_then(|open| open.get("open"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let state_name = state
        .as_ref()
        .and_then(|state| state.get("state"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if note_open || state_name == "note_detail" {
        let _ = runtime.close_note(0.8).await;
    }
}

async fn call_xhs_tool(
    page: Arc<PageSession>,
    tool_name: &str,
    input: Value,
    ctx: &ToolContext,
) -> anyhow::Result<Value> {
    let tool = xhs_tools(page)
        .into_iter()
        .find(|tool| tool.name() == tool_name)
        .ok_or_else(|| anyhow::anyhow!("xhs tool not found: {tool_name}"))?;
    let result = tool.call(input, ctx).await?;
    let text = result.flat_text();
    serde_json::from_str(text.trim()).or_else(|_| Ok(json!({ "raw_reply": text })))
}

fn command_context(label: &str) -> (String, ToolContext) {
    let run_dir = make_run_dir(label);
    let run_id = run_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(label)
        .to_string();
    let ctx = ToolContext::new(run_id, run_dir.clone());
    ctx.enable_site("xhs");
    (run_dir.display().to_string(), ctx)
}

fn trimmed_required(value: &str, label: &str) -> anyhow::Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("{label} is empty");
    }
    Ok(trimmed.to_string())
}

fn insert_optional_str(input: &mut Value, key: &str, value: Option<&str>) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    if let Some(obj) = input.as_object_mut() {
        obj.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn json_result(value: &Value) -> ToolResult {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    ToolResult::text(text)
}

fn get_f64(input: &Value, key: &str, default: f64) -> f64 {
    input.get(key).and_then(Value::as_f64).unwrap_or(default)
}

fn get_i64(input: &Value, key: &str, default: i64) -> i64 {
    input.get(key).and_then(Value::as_i64).unwrap_or(default)
}

fn get_str<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str)
}

fn get_bool(input: &Value, key: &str, default: bool) -> bool {
    input.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn read_note_options(input: &Value) -> ReadNoteOptions {
    ReadNoteOptions {
        level: get_str(input, "level").unwrap_or("lite").to_string(),
        include_media: get_bool(input, "include_media", false),
        download_media: get_bool(input, "download_media", false),
        max_images: get_i64(input, "max_images", 12).max(1) as usize,
        max_video_frames: get_i64(input, "max_video_frames", 4).max(1) as usize,
        poster_url_fallback: get_str(input, "poster_url_fallback")
            .unwrap_or("")
            .to_string(),
        note_id_fallback: get_str(input, "note_id_fallback").unwrap_or("").to_string(),
    }
}

fn media_for(
    ctx: &ToolContext,
    llm_provider: Option<Arc<dyn LlmProvider>>,
    include_media: bool,
) -> anyhow::Result<Option<MediaProcessor>> {
    if include_media {
        Ok(Some(MediaProcessor::for_run_dir(
            &ctx.run_dir,
            llm_provider,
        )?))
    } else {
        Ok(None)
    }
}

/// search_notes(query, wait_seconds) -> {query, cards: [...]}
pub struct SearchNotesTool {
    page: Arc<PageSession>,
    history: Arc<XhsHistoryStore>,
}

#[async_trait]
impl Tool for SearchNotesTool {
    fn name(&self) -> &str {
        "search_notes"
    }

    fn description(&self) -> &str {
        "Search Xiaohongshu for notes matching `query` and return result cards \
         (id, title, author, likes, cover image). By default reads only the \
         first results page (~19 cards, no scrolling). Pass `num_notes` to \
         auto-scroll the feed, lazy-loading more cards until that many are \
         collected (titles/likes/covers only — note bodies are NOT opened, so \
         it stays fast). Optionally applies search-result `filters` (omitted \
         groups reset to defaults); each group is single-select. Use before \
         `open_note` to pick a note; to read note bodies + comments in one call \
         use `topic_scan`."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query (Chinese works fine)" },
                "filters": search_filters_schema(),
                "num_notes": {
                    "type": "integer",
                    "description": "Scroll to collect at least this many cards (lazy-loaded). Omit for the first page only.",
                    "minimum": 1
                },
                "wait_seconds": {
                    "type": "number",
                    "description": "Extra seconds to wait for cards to load",
                    "default": 2.0
                }
            },
            "required": ["query"]
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let query = get_str(&input, "query")
            .ok_or_else(|| anyhow::anyhow!("missing query"))?
            .to_string();
        let filters = input
            .get("filters")
            .filter(|value| !value.is_null())
            .cloned();
        let wait_seconds = get_f64(&input, "wait_seconds", 2.0);
        let num_notes = input
            .get("num_notes")
            .and_then(Value::as_i64)
            .filter(|n| *n > 0)
            .map(|n| n as usize);
        let xhs = XhsPageRuntime::new(&self.page);
        let mut value = xhs
            .search_notes(&query, filters.as_ref(), wait_seconds, num_notes)
            .await?;
        if let Some(cards) = value.get_mut("cards") {
            self.history.annotate_cards(cards);
        }
        Ok(json_result(&value))
    }
}

/// open_note(note_id?, index?, wait_seconds?) -> {ok, ...}
pub struct OpenNoteTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for OpenNoteTool {
    fn name(&self) -> &str {
        "open_note"
    }

    fn description(&self) -> &str {
        "Open a note's detail modal on the current search results page. \
         Specify either `note_id` (from a card returned by search_notes) or \
         a 0-based `index` into the visible card list."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "note_id": { "type": "string", "description": "Note id from a search card" },
                "index": { "type": "integer", "description": "0-based index into the search results", "minimum": 0 },
                "wait_seconds": { "type": "number", "default": 4.0 }
            }
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let note_id = get_str(&input, "note_id").map(str::to_string);
        let index = input
            .get("index")
            .and_then(Value::as_i64)
            .and_then(|i| usize::try_from(i).ok());
        let wait_seconds = get_f64(&input, "wait_seconds", 4.0);
        let xhs = XhsPageRuntime::new(&self.page);
        let value = xhs
            .open_note(note_id.as_deref().unwrap_or(""), index, wait_seconds)
            .await?;
        Ok(json_result(&value))
    }
}

/// close_note(wait_seconds?) -> {ok}
pub struct CloseNoteTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for CloseNoteTool {
    fn name(&self) -> &str {
        "close_note"
    }

    fn description(&self) -> &str {
        "Close the currently open note detail modal so search results are \
         visible again."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "wait_seconds": { "type": "number", "default": 1.0 }
            }
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let wait_seconds = get_f64(&input, "wait_seconds", 1.0);
        let xhs = XhsPageRuntime::new(&self.page);
        let value = xhs.close_note(wait_seconds).await?;
        Ok(json_result(&value))
    }
}

/// read_note(note_id?, index?, wait_seconds?, include_media?) -> full XhsNote
pub struct ReadNoteTool {
    page: Arc<PageSession>,
    llm_provider: Option<Arc<dyn LlmProvider>>,
    history: Arc<XhsHistoryStore>,
}

#[async_trait]
impl Tool for ReadNoteTool {
    fn name(&self) -> &str {
        "read_note"
    }

    fn description(&self) -> &str {
        "Open a note from the current search results and return its full \
         body (title, author, content, images, location, like/collect/comment \
         counts). Closes the modal when done. Prefer this over open_note + \
         extract_note when you only need the body."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "note_id": { "type": "string" },
                "index": { "type": "integer", "minimum": 0 },
                "wait_seconds": { "type": "number", "default": 6.0 },
                "level": { "type": "string", "enum": ["card", "lite", "deep"], "default": "lite" },
                "include_media": { "type": "boolean", "default": false },
                "download_media": { "type": "boolean", "default": false },
                "max_images": { "type": "integer", "default": 12, "minimum": 1 },
                "max_video_frames": { "type": "integer", "default": 4, "minimum": 1 }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let note_id = get_str(&input, "note_id").map(str::to_string);
        let index = input
            .get("index")
            .and_then(Value::as_i64)
            .and_then(|i| usize::try_from(i).ok());
        let wait_seconds = get_f64(&input, "wait_seconds", 6.0);
        let options = read_note_options(&input);

        // Cross-run dedup: short-circuit when a previous run already covers
        // this note at the requested level + media. Only fires when note_id
        // is known up front. Downloads are intentionally never skipped because
        // the caller expects fresh local files in the current run dir.
        if let Some(id) = note_id.as_deref().filter(|s| !s.trim().is_empty()) {
            if !options.download_media
                && self
                    .history
                    .is_satisfied_by(id, &options.level, options.include_media)
            {
                let entry = self.history.get(id).unwrap_or_default();
                return Ok(json_result(&json!({
                    "ok": true,
                    "skipped": true,
                    "reason": "already_analyzed",
                    "note_id": id,
                    "requested_level": options.level,
                    "requested_include_media": options.include_media,
                    "requested_download_media": options.download_media,
                    "history": entry,
                })));
            }
        }

        let xhs = XhsPageRuntime::new_with_media(
            &self.page,
            media_for(
                ctx,
                self.llm_provider.clone(),
                options.include_media || options.download_media,
            )?,
        );
        let value = xhs
            .read_note_with_options(
                note_id.as_deref().unwrap_or(""),
                index,
                wait_seconds,
                options.clone(),
            )
            .await?;
        if value.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            if let Some(entity) = value.get("entity") {
                self.history
                    .record(entity, &options.level, options.include_media);
            }
        }
        Ok(json_result(&value))
    }
}

/// extract_note(wait_seconds?) -> XhsNote
pub struct ExtractNoteTool {
    page: Arc<PageSession>,
    llm_provider: Option<Arc<dyn LlmProvider>>,
    history: Arc<XhsHistoryStore>,
}

#[async_trait]
impl Tool for ExtractNoteTool {
    fn name(&self) -> &str {
        "extract_note"
    }

    fn description(&self) -> &str {
        "Extract the currently visible note body from the page. Assumes the \
         user already navigated to a note URL or has the detail modal open."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "wait_seconds": { "type": "number", "default": 8.0 },
                "level": { "type": "string", "enum": ["card", "lite", "deep"], "default": "lite" },
                "include_media": { "type": "boolean", "default": false },
                "download_media": { "type": "boolean", "default": false },
                "max_images": { "type": "integer", "default": 12, "minimum": 1 },
                "max_video_frames": { "type": "integer", "default": 4, "minimum": 1 }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let wait_seconds = get_f64(&input, "wait_seconds", 8.0);
        let options = read_note_options(&input);
        let xhs = XhsPageRuntime::new_with_media(
            &self.page,
            media_for(
                ctx,
                self.llm_provider.clone(),
                options.include_media || options.download_media,
            )?,
        );
        let note = xhs
            .extract_note_with_options(wait_seconds, options.clone())
            .await?;
        let value = serde_json::to_value(&note)?;
        self.history
            .record(&value, &options.level, options.include_media);
        Ok(json_result(&value))
    }
}

/// extract_comments(max_comments?) -> [comment]
pub struct ExtractCommentsTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for ExtractCommentsTool {
    fn name(&self) -> &str {
        "extract_comments"
    }

    fn description(&self) -> &str {
        "Extract visible comments on the currently open note. Requires a \
         note detail modal to be open (use open_note first)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "max_comments": { "type": "integer", "default": 20, "minimum": 1 }
            }
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let max = get_i64(&input, "max_comments", 20);
        let xhs = XhsPageRuntime::new(&self.page);
        let value = xhs.extract_comments(max).await?;
        Ok(json_result(&Value::Array(value)))
    }
}

/// page_state() -> {site, location, signed_in, modal_open, ...}
pub struct PageStateTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for PageStateTool {
    fn name(&self) -> &str {
        "page_state"
    }

    fn description(&self) -> &str {
        "Read a quick snapshot of the current page (site, signed-in state, \
         whether a note modal is open, current URL). Use this to verify what \
         step the agent is on."
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let xhs = XhsPageRuntime::new(&self.page);
        // ensure we're on XHS first, but don't navigate if we're not — just
        // report whatever the current page is.
        let value = xhs.detect_state().await?;
        Ok(json_result(&value))
    }
}

/// extract_search_cards() -> [card] — read-only; just returns the cards
/// currently visible in the search results without re-running the search.
pub struct ExtractSearchCardsTool {
    page: Arc<PageSession>,
    history: Arc<XhsHistoryStore>,
}

#[async_trait]
impl Tool for ExtractSearchCardsTool {
    fn name(&self) -> &str {
        "extract_search_cards"
    }

    fn description(&self) -> &str {
        "Return the note cards currently visible on the search results page \
         (without re-running the search). Useful after `click_search_tab` to \
         re-read the filtered card list."
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let xhs = XhsPageRuntime::new(&self.page);
        let cards = xhs.extract_search_cards().await?;
        let mut value = serde_json::to_value(&cards)?;
        self.history.annotate_cards(&mut value);
        Ok(json_result(&value))
    }
}

/// list_search_tabs() -> [tab] — list filter tabs on a search results page.
pub struct ListSearchTabsTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for ListSearchTabsTool {
    fn name(&self) -> &str {
        "list_search_tabs"
    }

    fn description(&self) -> &str {
        "List the search-filter tabs visible on the current search page \
         (e.g. 全部 / 图文 / 视频 / 用户). Each entry has a label and an \
         `active` flag."
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let xhs = XhsPageRuntime::new(&self.page);
        let tabs = xhs.list_search_tabs().await?;
        Ok(json_result(&Value::Array(tabs)))
    }
}

/// click_search_tab(label, wait_seconds?) -> {ok, label, active_filter, tabs}
pub struct ClickSearchTabTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for ClickSearchTabTool {
    fn name(&self) -> &str {
        "click_search_tab"
    }

    fn description(&self) -> &str {
        "Click the named search-filter tab to narrow results. Tab labels: \
         全部 / 图文 / 视频 / 用户 (or any active tab returned by \
         list_search_tabs). After clicking, follow up with \
         extract_search_cards to see the filtered cards."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "label": { "type": "string", "description": "Tab label (e.g. 全部, 图文, 视频, 用户)" },
                "wait_seconds": { "type": "number", "default": 1.5 }
            },
            "required": ["label"]
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let label = get_str(&input, "label")
            .ok_or_else(|| anyhow::anyhow!("missing label"))?
            .to_string();
        let wait_seconds = get_f64(&input, "wait_seconds", 1.5);
        let xhs = XhsPageRuntime::new(&self.page);
        let value = xhs.click_search_tab(&label, wait_seconds).await?;
        Ok(json_result(&value))
    }
}

/// reset_search_filters() -> {ok, reset}
pub struct ResetSearchFiltersTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for ResetSearchFiltersTool {
    fn name(&self) -> &str {
        "reset_search_filters"
    }

    fn description(&self) -> &str {
        "Hover the Xiaohongshu search page's `筛选` control, reset active \
         search filters to their defaults."
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let xhs = XhsPageRuntime::new(&self.page);
        let value = xhs.reset_search_filters(1.0).await?;
        Ok(json_result(&value))
    }
}

/// apply_search_filters(filters) -> {ok, changed, filters}
pub struct ApplySearchFiltersTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for ApplySearchFiltersTool {
    fn name(&self) -> &str {
        "apply_search_filters"
    }

    fn description(&self) -> &str {
        "Hover the Xiaohongshu search page's `筛选` control and select filter \
        options from the current panel. Omitted groups are reset to defaults, \
        preventing filters from previous searches from leaking into the results. \
        Each group is single-select, but multiple groups can be combined. Use \
        `extract_search_cards` after applying filters to read the current cards."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "filters": search_filters_schema()
            },
            "required": ["filters"]
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let filters = input
            .get("filters")
            .ok_or_else(|| anyhow::anyhow!("missing filters"))?;
        let xhs = XhsPageRuntime::new(&self.page);
        let value = xhs.apply_search_filters(filters, 1.0).await?;
        Ok(json_result(&value))
    }
}

/// scroll_in_note(pixels?) -> {ok, scroll_top, ...}
pub struct ScrollInNoteTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for ScrollInNoteTool {
    fn name(&self) -> &str {
        "scroll_in_note"
    }

    fn description(&self) -> &str {
        "Scroll the currently open note's detail panel by `pixels` (positive \
         = down). Use this to bring more comments or note body into view \
         before re-extracting."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pixels": { "type": "integer", "default": 400 }
            }
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let pixels = get_i64(&input, "pixels", 400);
        let xhs = XhsPageRuntime::new(&self.page);
        let value = xhs.scroll_in_note(pixels).await?;
        Ok(json_result(&value))
    }
}

/// collect_carousel_images(max_images?) -> [url]
pub struct CollectCarouselImagesTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for CollectCarouselImagesTool {
    fn name(&self) -> &str {
        "collect_carousel_images"
    }

    fn description(&self) -> &str {
        "Collect image URLs from the carousel of the currently open note. \
         Requires the note detail modal to be open (use open_note first)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "max_images": { "type": "integer", "default": 12, "minimum": 1 }
            }
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let max_images = get_i64(&input, "max_images", 12);
        let xhs = XhsPageRuntime::new(&self.page);
        let urls = xhs.collect_carousel_images(max_images).await?;
        Ok(json_result(&serde_json::to_value(&urls)?))
    }
}

/// extract_profile(max_notes?, scroll_rounds?) -> profile entity
pub struct ExtractProfileTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for ExtractProfileTool {
    fn name(&self) -> &str {
        "extract_profile"
    }

    fn description(&self) -> &str {
        "Extract the currently visible Xiaohongshu profile page (author \
         display_name, xhs_id, bio, followers/following counts, and a paginated \
         list of note cards by scrolling the page). Caller must have navigated \
         to a profile URL first; this errors otherwise."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "max_notes": { "type": "integer", "default": 20, "minimum": 1 },
                "scroll_rounds": { "type": "integer", "default": 6, "minimum": 1 }
            }
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let max_notes = get_i64(&input, "max_notes", 20);
        let scroll_rounds = get_i64(&input, "scroll_rounds", 6);
        let xhs = XhsPageRuntime::new(&self.page);
        let profile = xhs.extract_profile(max_notes, scroll_rounds).await?;
        Ok(json_result(&profile.to_value()))
    }
}

/// topic_scan(query, tab_label?, filters?, num_notes?, download_media?) -> aggregated topic bundle
///
/// Composite macro: search → optional tab switch → optional search filters →
/// collect up to `num_notes` cards in page order (scrolling the feed only when
/// the first page is too small) → open each note and extract its body + top
/// comments → bundle into one artifact. Prefer this for any "research a topic
/// on XHS" task — it returns search results plus the note bodies plus comments
/// in one tool call, so the agent doesn't have to chain 10+ tools by hand.
///
/// Defaults to `DEFAULT_NUM_NOTES` notes; pass a larger `num_notes` to scan
/// more (each note is opened, so latency grows roughly linearly).
pub struct TopicScanTool {
    page: Arc<PageSession>,
    llm_provider: Option<Arc<dyn LlmProvider>>,
    history: Arc<XhsHistoryStore>,
}

/// Number of top comments pulled per scanned note. Comments are read for free
/// from the already-open note modal's DOM (one extra JS read, no extra
/// navigation), so every scan includes them.
const TOPIC_SCAN_COMMENTS: i64 = 12;

#[async_trait]
impl Tool for TopicScanTool {
    fn name(&self) -> &str {
        "topic_scan"
    }

    fn description(&self) -> &str {
        "Xiaohongshu topic research macro: search → optional tab switch → \
         optional search filters → \
         collect up to `num_notes` cards in page order (scrolling only if the \
         first page is too small) → open each note and read its body + top \
         comments → return one compact bundle (search results + selected cards \
         + note bodies + comments). Pass `download_media=true` to download \
         note images/videos into the run dir, include local paths, and emit a \
         stable media_manifest_path. Defaults \
         to 10 notes; pass a larger `num_notes` to scan more (each note is \
         opened, so latency scales with it). Prefer this for XHS topic \
         research. Do not repeat the same scan unless the previous one was \
         clearly insufficient."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "tab_label": {
                    "type": "string",
                    "enum": ["全部", "图文", "视频", "用户"]
                },
                "filters": search_filters_schema(),
                "num_notes": {
                    "type": "integer",
                    "description": "Number of notes to read (body + top comments each). The first results page is used directly; only if it holds fewer than this does the feed scroll for more. Each note is opened, so latency scales with this.",
                    "default": DEFAULT_NUM_NOTES,
                    "minimum": 1
                },
                "download_media": {
                    "type": "boolean",
                    "description": "Download note images/videos into the command run_dir, include local_path fields in returned notes, and write a stable media_manifest.json surfaced by media_manifest_path.",
                    "default": false
                }
            },
            "required": ["query"]
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let query = get_str(&input, "query")
            .ok_or_else(|| anyhow::anyhow!("missing query"))?
            .to_string();
        let num_notes = get_i64(&input, "num_notes", DEFAULT_NUM_NOTES).max(1);
        let tab_label = get_str(&input, "tab_label").unwrap_or("").to_string();
        let filters = input
            .get("filters")
            .filter(|value| !value.is_null())
            .cloned();
        // Every scanned note is read the same way: open it, extract the body,
        // and pull top comments. Per-note image vision is off (it's the one
        // genuinely expensive enrichment and not needed for topic research).
        let include_media = false;
        let download_media = get_bool(&input, "download_media", false);

        let media = media_for(
            ctx,
            self.llm_provider.clone(),
            include_media || download_media,
        )?;
        let media_baseline: Option<TimingSnapshot> = media.as_ref().map(|m| m.timing().snapshot());
        let xhs = XhsPageRuntime::new_with_media(&self.page, media.clone());

        // Snapshot history BEFORE we start reading. The loop below may
        // record notes into the live store, but final-payload annotations
        // should reflect the state going in — otherwise a first-time scan
        // labels its own freshly-read cards as `already_analyzed`.
        let history_snapshot = self.history.snapshot();

        // Filters are applied after the optional tab switch below (tab switch
        // re-runs the search and would drop them), so don't pass them here.
        let search = xhs.search_notes(&query, None, 2.0, None).await?;

        // If the search never landed on a results page (search box not found,
        // login required, …) bail before the browse loop. Otherwise
        // extract_search_cards would read whatever feed is on screen — the
        // /explore recommendations — and silently return notes unrelated to the
        // query. Surface the failure so callers see why instead of bad data.
        if !search.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            let reason = search
                .get("reason")
                .and_then(Value::as_str)
                .filter(|reason| !reason.is_empty())
                .unwrap_or("search_failed")
                .to_string();
            let payload = json!({
                "ok": false,
                "query": query,
                "search": search,
                "selected_cards": [],
                "notes": [],
                "reason": reason,
                "sampling": {
                    "num_notes": num_notes,
                    "selected": 0,
                    "comments_per_note": TOPIC_SCAN_COMMENTS,
                    "include_media": include_media,
                    "download_media": download_media,
                },
            });
            return Ok(json_result(&payload));
        }

        // Optional tab switch (re-runs the search under the chosen tab), then
        // optional filter application.
        let mut tab_result = Value::Object(serde_json::Map::new());
        if !tab_label.is_empty() {
            tab_result = xhs.click_search_tab(&tab_label, 1.5).await?;
        }

        let mut filter_result = Value::Object(serde_json::Map::new());
        if let Some(filters) = filters {
            filter_result = xhs.apply_search_filters(&filters, 1.5).await?;
        }

        // Every sampled note is read with the same extraction level (body +
        // top comments).
        let level = "deep";
        let comment_count = TOPIC_SCAN_COMMENTS;
        let requested_media = include_media;
        let can_use_cached_reads = !download_media;
        let want = num_notes.max(1) as usize;

        // Read top-to-bottom: pull cards from the results state (which only
        // grows) in feed order and open each. Opening a card scrolls it into
        // view, which pages the later cards in on its own — there's no
        // separate "scroll to the bottom and collect everything first" phase.
        // When we've consumed every loaded card, wait briefly for that async
        // paging to land; if nothing more loads after a few tries, stop.
        let mut notes: Vec<Value> = Vec::new();
        let mut selected: Vec<XhsNoteCard> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut cursor = 0usize;
        let mut stalls = 0usize;

        while notes.len() < want {
            let cards = xhs.extract_search_cards().await?;
            if cursor >= cards.len() {
                if stalls >= 3 {
                    break;
                }
                stalls += 1;
                tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                continue;
            }
            stalls = 0;
            let card = cards[cursor].clone();
            cursor += 1;
            let dedup = if !card.note_id.is_empty() {
                card.note_id.clone()
            } else if !card.link.is_empty() {
                card.link.clone()
            } else {
                format!("pos:{}", card.position)
            };
            if !seen.insert(dedup) {
                continue;
            }
            if !card.note_id.is_empty() {
                ctx.add_topic_scan_note_ids(std::slice::from_ref(&card.note_id));
            }
            selected.push(card.clone());

            // Dedup: skip notes already processed at this level or deeper
            // within the same run OR in a previous run (cross-run history).
            // Media downloads are never cache-skipped: the caller expects
            // fresh files under this command's run_dir.
            if can_use_cached_reads
                && !card.note_id.is_empty()
                && ctx.has_processed_note(&card.note_id, level, requested_media)
            {
                notes.push(json!({
                    "scan_level": level,
                    "source_position": card.position,
                    "skipped": {"reason": "already_processed"},
                    "entity": &card,
                }));
                continue;
            }
            if can_use_cached_reads
                && !card.note_id.is_empty()
                && self
                    .history
                    .is_satisfied_by(&card.note_id, level, requested_media)
            {
                let entry = self.history.get(&card.note_id).unwrap_or_default();
                notes.push(json!({
                    "scan_level": level,
                    "source_position": card.position,
                    "skipped": {"reason": "already_analyzed", "history": entry},
                    "entity": &card,
                }));
                ctx.mark_processed_note(&card.note_id, level, requested_media);
                continue;
            }
            let read_result = xhs
                .read_note_with_options(
                    &card.note_id,
                    None,
                    6.0,
                    ReadNoteOptions {
                        level: level.to_string(),
                        include_media,
                        download_media,
                        // Pure downloads are cheap compared with OCR/vision,
                        // so allow full XHS carousels instead of the smaller
                        // enrichment-oriented default.
                        max_images: if download_media { 100 } else { 12 },
                        max_video_frames: 4,
                        poster_url_fallback: card.cover_url.clone(),
                        note_id_fallback: card.note_id.clone(),
                    },
                )
                .await;
            let mut entry = match read_result {
                Ok(payload) => {
                    let mut entity = payload.get("entity").cloned().unwrap_or(Value::Null);
                    ensure_entity_note_id(&mut entity, &card);
                    json!({
                        "scan_level": level,
                        "source_position": card.position,
                        "ok": payload.get("ok").and_then(Value::as_bool).unwrap_or(false),
                        "entity": entity,
                    })
                }
                Err(e) => json!({
                    "scan_level": level,
                    "source_position": card.position,
                    "ok": false,
                    "entity": &card,
                    "error": format!("{e:#}"),
                }),
            };

            // Pull comments separately after waiting for the slower comment
            // list to hydrate. Body content often appears before comments.
            if level == "deep" && comment_count > 0 {
                if let Ok(comments_payload) =
                    xhs.extract_comments_with_wait(comment_count, 5.0).await
                {
                    let comments = comments_payload
                        .get("comments")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let entity_map = entry.get_mut("entity").and_then(|v| v.as_object_mut());
                    if let Some(map) = entity_map {
                        map.insert("top_comments".into(), Value::Array(comments));
                        map.insert(
                            "top_comments_wait".into(),
                            json!({
                                "ready": comments_payload.get("ready").and_then(Value::as_bool).unwrap_or(false),
                                "reason": comments_payload.get("reason").and_then(Value::as_str).unwrap_or(""),
                                "waited_ms": comments_payload.get("waited_ms").and_then(Value::as_i64).unwrap_or(0),
                                "attempts": comments_payload.get("attempts").and_then(Value::as_i64).unwrap_or(0),
                            }),
                        );
                    }
                }
            }

            // Mark processed in-run + record in cross-run history.
            if !card.note_id.is_empty() {
                ctx.mark_processed_note(&card.note_id, level, requested_media);
            }
            if entry.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                if let Some(entity) = entry.get("entity") {
                    self.history.record(entity, level, requested_media);
                }
            }

            notes.push(entry);
            let _ = xhs.close_note(0.6).await;
        }

        let media_timing = match (&media, &media_baseline) {
            (Some(media), Some(before)) => timing_delta(before, &media.timing().snapshot()),
            _ => json!({}),
        };

        // Annotate cards in the search payload and selected_cards against
        // the pre-call snapshot so flags reflect "known before this scan"
        // rather than "known after this scan's own writes".
        let mut search = search;
        if let Some(cards) = search.get_mut("cards") {
            history_snapshot.annotate_cards(cards);
        }
        let mut selected_cards = serde_json::to_value(&selected)?;
        history_snapshot.annotate_cards(&mut selected_cards);

        let media_manifest_metadata = if download_media {
            let media_manifest = topic_scan_media_manifest(&notes, &ctx.run_dir);
            let media_manifest_count = media_manifest.as_array().map(Vec::len).unwrap_or_default();
            let (media_manifest_path, media_manifest_error) =
                match write_media_manifest_file(ctx, &media_manifest) {
                    Ok(path) => (Some(path), None),
                    Err(err) => (None, Some(format!("{err:#}"))),
                };
            Some((
                media_manifest_count,
                media_manifest_path,
                media_manifest_error,
            ))
        } else {
            None
        };

        let mut payload = json!({
            "ok": search.get("ok").and_then(Value::as_bool).unwrap_or(false),
            "query": query,
            "tab": tab_result,
            "filters": filter_result,
            "search": search,
            "selected_cards": selected_cards,
            "notes": notes,
            "sampling": {
                "num_notes": num_notes,
                "selected": selected.len(),
                "comments_per_note": TOPIC_SCAN_COMMENTS,
                "include_media": include_media,
                "download_media": download_media,
            },
            "timing": {
                "media": media_timing,
            }
        });
        if let Some((media_manifest_count, media_manifest_path, media_manifest_error)) =
            media_manifest_metadata
        {
            if let Some(map) = payload.as_object_mut() {
                map.insert("media_manifest_count".into(), json!(media_manifest_count));
                if let Some(path) = media_manifest_path {
                    map.insert("media_manifest_path".into(), json!(path));
                }
                if let Some(error) = media_manifest_error {
                    map.insert("media_manifest_error".into(), json!(error));
                }
            }
        }

        // Persist as artifact so it shows up in the run dir + working memory.
        let _ = ctx.write_json_artifact(
            &format!("xhs_topic_scan_{}", sanitize_for_filename(&query)),
            &payload,
            "artifacts",
            "topic_scan",
            "json",
            &format!(
                "Topic scan: {query} ({} notes)",
                payload
                    .get("notes")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0)
            ),
            json!({"site": "xhs", "category": "topic_scan"}),
        );

        Ok(json_result(&payload))
    }
}

fn search_filters_schema() -> Value {
    let properties: Map<String, Value> = XHS_SEARCH_FILTERS
        .iter()
        .map(|(key, _title, options)| {
            (
                key.to_string(),
                json!({
                    "type": "string",
                    "enum": options,
                }),
            )
        })
        .collect();

    json!({
        "type": "object",
        "description": "Search filter selections by group key.",
        "properties": properties,
        "minProperties": 1,
        "additionalProperties": false
    })
}

fn sanitize_for_filename(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .chars()
        .take(48)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_media_does_not_imply_include_media() {
        let options = read_note_options(&json!({ "download_media": true }));

        assert!(options.download_media);
        assert!(!options.include_media);
    }

    #[test]
    fn include_media_remains_independent_from_download_media() {
        let options = read_note_options(&json!({
            "include_media": true,
            "download_media": false,
        }));

        assert!(options.include_media);
        assert!(!options.download_media);
    }
}
