//! Agent-callable tool wrappers around [`XhsPageRuntime`].
//!
//! Each wrapper owns an `Arc<PageSession>` — the same tab is reused across
//! tool calls so the agent's actions accumulate state (search results
//! visible, note modal open, etc.). The caller is responsible for creating
//! the page and closing it after `run_agent` returns.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::agent::compaction::truncate;
use crate::agent::tool::{
    ToolProgressEvent, ToolProgressPhase, ToolProgressSender, ToolProgressStatus,
};
use crate::agent::{Backend as LlmProvider, Tool, ToolContext, ToolResult};
use crate::cdp::PageSession;
use crate::media::{ocr_diagnostics, ocr_warm_up, timing_delta, MediaProcessor, TimingSnapshot};
use async_trait::async_trait;
use serde_json::{json, Map, Value};

use crate::sites::registry::{
    required_string, ArgKind, BoxFuture, CommandArg, SiteCommand, SiteSpec, SlowWhen,
};
use crate::sites::runner::{
    get_bool, get_f64, get_i64, get_str, json_result, run_tool_command, trimmed_required, PageHook,
    ToolCommand,
};
use crate::sites::xhs::media_manifest::{
    ensure_entity_note_id, search_media_manifest, write_media_manifest_file,
};
use crate::sites::xhs::page::XHS_SEARCH_FILTERS;
use crate::sites::xhs::{
    ReadNoteOptions, XhsAuthorProfile, XhsHistoryStore, XhsNoteCard, XhsPageRuntime, XHS_HOME_URL,
};

/// Default number of notes `search` reads when the caller doesn't specify.
const DEFAULT_NUM_NOTES: i64 = 10;

/// Seconds to wait for search results to render before reading cards. Internal
/// (not a user/agent knob), shared by the full scan and the preview path.
const SEARCH_WAIT_SECONDS: f64 = 2.0;

/// Top comments attached to every note read (read_note, extract_note,
/// search, author_scan). Comments are read from the already-open note's DOM
/// (one extra JS read, no extra navigation), so every note read includes them.
const TOP_COMMENTS_PER_NOTE: i64 = 12;

/// XHS macro-agent playbook for the single app/TUI agent interface. Embedded
/// at compile time so the agent prompt always carries the latest copy.
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
        Arc::new(ExtractSearchCardsTool {
            page: page.clone(),
            history: history.clone(),
        }) as Arc<dyn Tool>,
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
        Arc::new(SearchTool {
            page: page.clone(),
            llm_provider,
            history: history.clone(),
            always_download_media: false,
            always_ocr: false,
        }),
        Arc::new(AuthorScanTool {
            page: page.clone(),
            history,
            always_download_media: false,
            always_ocr: false,
        }),
        Arc::new(PageStateTool { page }),
    ]
}

pub fn xhs_macro_tools_with_llm_provider(
    page: Arc<PageSession>,
    llm_provider: Option<Arc<dyn LlmProvider>>,
) -> Vec<Arc<dyn Tool>> {
    let history = Arc::new(XhsHistoryStore::open_default());
    // The app/TUI agent interface always downloads note media so the offline
    // files are on hand for deeper analysis, and always OCRs every image; the
    // CLI keeps its --download-media / --ocr opt-ins via the full tool set above.
    vec![
        Arc::new(SearchTool {
            page: page.clone(),
            llm_provider,
            history: history.clone(),
            always_download_media: true,
            always_ocr: true,
        }) as Arc<dyn Tool>,
        Arc::new(AuthorScanTool {
            page,
            history,
            always_download_media: true,
            always_ocr: true,
        }),
    ]
}

pub async fn xhs_agent_tools(
    page: Arc<PageSession>,
    llm_provider: Arc<dyn LlmProvider>,
) -> anyhow::Result<Vec<Arc<dyn Tool>>> {
    XhsPageRuntime::new(&page).ensure_xhs(false).await.ok();
    Ok(xhs_tools_with_llm_provider(page, Some(llm_provider)))
}

pub async fn xhs_default_agent_tools(
    page: Arc<PageSession>,
    llm_provider: Arc<dyn LlmProvider>,
) -> anyhow::Result<Vec<Arc<dyn Tool>>> {
    // Macro tools are self-contained and perform their own navigation/typed
    // failure handling, so the default agent factory should not require the
    // current tab to already be on XHS.
    Ok(xhs_macro_tools_with_llm_provider(page, Some(llm_provider)))
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

/// Registry entry for Xiaohongshu — the only wiring a site needs beyond its
/// module declaration in `sites/mod.rs`.
pub static XHS_SITE: SiteSpec = SiteSpec {
    id: "xhs",
    about: "Xiaohongshu (xiaohongshu.com)",
    home_url: XHS_HOME_URL,
    agent_tools: |page, llm| Box::pin(xhs_agent_tools(page, llm)),
    default_agent_tools: Some(|page, llm| Box::pin(xhs_default_agent_tools(page, llm))),
    agent_instructions: xhs_agent_instructions,
    default_agent_instructions: Some(xhs_agent_instructions),
    commands: &[
        SiteCommand {
            name: "search",
            tool_name: "search",
            about: "Search Xiaohongshu. By default opens each result and returns note bodies \
                    + top comments (a topic scan). With --preview, returns only the result \
                    cards (titles/likes/covers) without opening any note.",
            args: &[
                CommandArg {
                    key: "query",
                    long: None,
                    value_name: "QUERY",
                    help: "Search query",
                    required: true,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "filters",
                    long: Some("filter"),
                    value_name: "GROUP=OPTION",
                    help: "Search-result filter as `group=option` (repeatable), e.g. \
                           `--filter publish_time=一天内 --filter note_type=图文`. Groups: \
                           sort, note_type, publish_time, search_scope, distance.",
                    required: false,
                    kind: ArgKind::KeyValueMap,
                },
                CommandArg {
                    key: "num_notes",
                    long: Some("num-notes"),
                    value_name: "N",
                    help: "How many notes/cards to collect, scrolling the feed to reach it. \
                           Omit for the first page only.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "download_media",
                    long: Some("download-media"),
                    value_name: "DOWNLOAD_MEDIA",
                    help: "Download note images/videos into the run_dir and include \
                           local_path fields. Ignored with --preview.",
                    required: false,
                    kind: ArgKind::Flag,
                },
                CommandArg {
                    key: "ocr",
                    long: Some("ocr"),
                    value_name: "OCR",
                    help: "OCR every downloaded note image (PP-OCRv6 small, local) and \
                           attach per-image ocr_text plus a per-note ocr summary. \
                           Implies --download-media. Ignored with --preview.",
                    required: false,
                    kind: ArgKind::Flag,
                },
                CommandArg {
                    key: "preview",
                    long: Some("preview"),
                    value_name: "PREVIEW",
                    help: "Preview mode: return result cards only (titles/likes/covers), \
                           without opening notes or reading bodies/comments.",
                    required: false,
                    kind: ArgKind::Flag,
                },
            ],
            slow: SlowWhen::Always,
            run: run_search,
        },
        SiteCommand {
            name: "author",
            tool_name: "author_scan",
            about: "Open a Xiaohongshu author's profile and print their header (bio, xhs id, \
                    IP location, follower/following/like counts) plus their notes. By default \
                    opens each note for its body + top comments; with --preview, returns only \
                    the note cards.",
            args: &[
                CommandArg {
                    key: "author_id",
                    long: None,
                    value_name: "AUTHOR_ID",
                    help: "Author id — the trailing segment of /user/profile/<id>.",
                    required: true,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "num_notes",
                    long: Some("num-notes"),
                    value_name: "N",
                    help: "Scroll the profile grid to collect this many notes (default 10).",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "download_media",
                    long: Some("download-media"),
                    value_name: "DOWNLOAD_MEDIA",
                    help: "Download each note's images/videos into the run_dir and include \
                           local_path fields. Only applies when notes are opened (not --preview).",
                    required: false,
                    kind: ArgKind::Flag,
                },
                CommandArg {
                    key: "ocr",
                    long: Some("ocr"),
                    value_name: "OCR",
                    help: "OCR every downloaded note image (PP-OCRv6 small, local) and \
                           attach per-image ocr_text plus a per-note ocr summary. \
                           Implies --download-media. Only applies when notes are opened.",
                    required: false,
                    kind: ArgKind::Flag,
                },
                CommandArg {
                    key: "preview",
                    long: Some("preview"),
                    value_name: "PREVIEW",
                    help: "Preview mode: return note cards only (titles/likes/covers), without \
                           opening each note for its body + comments.",
                    required: false,
                    kind: ArgKind::Flag,
                },
            ],
            // Scrolling for a large num_notes or opening each note for details
            // can take a while; give it the longer budget.
            slow: SlowWhen::Always,
            run: run_author_scan,
        },
    ],
};

/// `search` dispatches on `--preview`: default opens each result (full scan —
/// body + top comments); `--preview` returns result cards only (titles/likes/
/// covers) without opening any note.
fn run_search(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    Box::pin(async move {
        let query = required_string(&args, "query")?;
        let filters = args.get("filters").cloned();
        // Default to DEFAULT_NUM_NOTES so omitting --num-notes collects a fixed
        // batch (scrolling as needed), not just whatever the first page renders.
        let num_notes = args
            .get("num_notes")
            .and_then(Value::as_i64)
            .or(Some(DEFAULT_NUM_NOTES));
        let preview = args
            .get("preview")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // OCR applies in both modes: a full scan OCRs every downloaded image, a
        // --preview pass OCRs each result card's cover (like a human scanning the
        // results page). So `ocr` is not gated on !preview.
        let ocr = args.get("ocr").and_then(Value::as_bool).unwrap_or(false);
        // download_media (full media into the run dir) doesn't apply to a
        // card-only (--preview) read; in a full scan, OCR implies it.
        let download_media = !preview
            && (ocr
                || args
                    .get("download_media")
                    .and_then(Value::as_bool)
                    .unwrap_or(false));
        search_command(
            page,
            "search",
            &query,
            filters.as_ref(),
            num_notes,
            download_media,
            ocr,
            preview,
            debug_snapshot,
            progress,
        )
        .await
    })
}

fn run_author_scan(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    Box::pin(async move {
        let author_id = required_string(&args, "author_id")?;
        // Default to DEFAULT_NUM_NOTES so omitting --num-notes collects a fixed
        // batch (scrolling as needed), not just whatever the first page renders.
        let num_notes = args
            .get("num_notes")
            .and_then(Value::as_i64)
            .or(Some(DEFAULT_NUM_NOTES));
        // Default opens each note; --preview returns cards only.
        let preview = args
            .get("preview")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // OCR applies in both modes: a full scan OCRs every downloaded image, a
        // --preview pass OCRs each note card's cover. download_media (full media
        // into the run dir) only applies when notes are opened (not preview);
        // there a full scan's OCR implies it.
        let ocr = args.get("ocr").and_then(Value::as_bool).unwrap_or(false);
        let download_media = !preview
            && (ocr
                || args
                    .get("download_media")
                    .and_then(Value::as_bool)
                    .unwrap_or(false));
        author_scan_command(
            page,
            &author_id,
            num_notes,
            preview,
            download_media,
            ocr,
            debug_snapshot,
            progress,
        )
        .await
    })
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
    include_run_metadata: bool,
}

const SEARCH_COMMAND: XhsCommandSpec = XhsCommandSpec {
    command_name: "search",
    tool_name: "search",
    before: CommandPageAction::SearchReady,
    after: CommandPageAction::None,
    include_run_metadata: true,
};

const AUTHOR_SCAN_COMMAND: XhsCommandSpec = XhsCommandSpec {
    command_name: "author",
    tool_name: "author_scan",
    // The tool navigates to the profile URL itself; just make sure no stale
    // note modal is left open before/after.
    before: CommandPageAction::CloseOpenNote,
    after: CommandPageAction::CloseOpenNote,
    include_run_metadata: false,
};

#[allow(clippy::too_many_arguments)]
pub async fn search_command(
    page: Arc<PageSession>,
    command_name: &'static str,
    query: &str,
    filters: Option<&Value>,
    num_notes: Option<i64>,
    download_media: bool,
    ocr: bool,
    preview: bool,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> anyhow::Result<Value> {
    // `command_name` is passed through so the run-dir label and envelope reflect
    // the actual command the caller invoked. `preview` selects the cards-only
    // fast path inside the `search` tool; the default opens each result.
    let spec = XhsCommandSpec {
        command_name,
        ..SEARCH_COMMAND
    };
    run_xhs_tool_command(
        page,
        spec,
        search_input(query, filters, num_notes, download_media, ocr, preview)?,
        debug_snapshot,
        progress,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn author_scan_command(
    page: Arc<PageSession>,
    author_id: &str,
    num_notes: Option<i64>,
    preview: bool,
    download_media: bool,
    ocr: bool,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> anyhow::Result<Value> {
    run_xhs_tool_command(
        page,
        AUTHOR_SCAN_COMMAND,
        author_scan_input(author_id, num_notes, preview, download_media, ocr)?,
        debug_snapshot,
        progress,
    )
    .await
}

fn search_input(
    query: &str,
    filters: Option<&Value>,
    num_notes: Option<i64>,
    download_media: bool,
    ocr: bool,
    preview: bool,
) -> anyhow::Result<Value> {
    let mut input = json!({
        "query": trimmed_required(query, "query")?,
    });
    if let Some(filters) = filters {
        input["filters"] = filters.clone();
    }
    if let Some(n) = num_notes {
        input["num_notes"] = json!(n.max(1));
    }
    if download_media {
        input["download_media"] = json!(true);
    }
    if ocr {
        input["ocr"] = json!(true);
    }
    if preview {
        input["preview"] = json!(true);
    }
    Ok(input)
}

fn author_scan_input(
    author_id: &str,
    num_notes: Option<i64>,
    preview: bool,
    download_media: bool,
    ocr: bool,
) -> anyhow::Result<Value> {
    let mut input = json!({
        "author_id": trimmed_required(author_id, "author_id")?,
    });
    if let Some(n) = num_notes {
        input["num_notes"] = json!(n.max(1));
    }
    if preview {
        input["preview"] = json!(true);
    }
    if download_media {
        input["download_media"] = json!(true);
    }
    if ocr {
        input["ocr"] = json!(true);
    }
    Ok(input)
}

async fn run_xhs_tool_command(
    page: Arc<PageSession>,
    spec: XhsCommandSpec,
    input: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> anyhow::Result<Value> {
    let tools = xhs_tools(page.clone());
    run_tool_command(
        ToolCommand {
            site_id: "xhs",
            command_name: spec.command_name,
            tool_name: spec.tool_name,
            before: page_action_hook(spec.before),
            after: page_action_hook(spec.after),
            include_run_metadata: spec.include_run_metadata,
        },
        page,
        &tools,
        input,
        debug_snapshot,
        progress,
    )
    .await
}

fn page_action_hook(action: CommandPageAction) -> Option<PageHook> {
    match action {
        CommandPageAction::None => None,
        CommandPageAction::SearchReady => Some(Box::new(|page| {
            Box::pin(async move { ensure_search_ready(&page).await })
        })),
        CommandPageAction::CloseOpenNote => Some(Box::new(|page| {
            Box::pin(async move {
                close_open_note(&page).await;
                Ok(())
            })
        })),
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

fn read_note_options(input: &Value) -> ReadNoteOptions {
    ReadNoteOptions {
        // `level` is no longer a user-facing knob (body content is identical
        // across tiers; media is gated by include_media/download_media). It now
        // only feeds the cross-run history dedup key.
        level: "lite".to_string(),
        include_media: get_bool(input, "include_media", false),
        download_media: get_bool(input, "download_media", false),
        ocr: get_bool(input, "ocr", false),
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
            ctx.output_dir(),
            llm_provider,
        )?))
    } else {
        Ok(None)
    }
}

/// Read up to `TOP_COMMENTS_PER_NOTE` top comments from the currently open note
/// and insert them under `note["top_comments"]`. Best-effort: on failure the
/// note is left without the field. Shared by `read_note` and `extract_note`.
async fn attach_top_comments(xhs: &XhsPageRuntime<'_>, note: &mut Value) {
    let Ok(payload) = xhs
        .extract_comments_with_wait(TOP_COMMENTS_PER_NOTE, 5.0)
        .await
    else {
        return;
    };
    let comments = payload
        .get("comments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if let Some(map) = note.as_object_mut() {
        map.insert("top_comments".into(), Value::Array(comments));
    }
}

/// Build a skipped-note entry. We return the full entity cached in history
/// (body + comments + images + location) so a reused note keeps its data
/// instead of degrading to the bare search card; the `skipped` block carries a
/// compact provenance summary (no title/author/url repeat — those are in
/// `entity`). Falls back to the card when history has no cached entity (e.g. a
/// pre-upgrade entry, or one only ever seen as a card).
fn skipped_note_entry(card: &XhsNoteCard, reason: &str, history: &XhsHistoryStore) -> Value {
    let entry = history.get(&card.note_id);
    let entity = entry
        .as_ref()
        .and_then(|e| e.entity.clone())
        .unwrap_or_else(|| serde_json::to_value(card).unwrap_or(Value::Null));
    let mut skipped = json!({ "reason": reason });
    if let (Some(entry), Some(map)) = (entry.as_ref(), skipped.as_object_mut()) {
        map.insert("level".into(), json!(entry.level));
        map.insert("analysis_count".into(), json!(entry.analysis_count));
        map.insert("first_seen_at".into(), json!(entry.first_seen_at));
        map.insert("last_seen_at".into(), json!(entry.last_seen_at));
    }
    json!({
        "source_position": card.position,
        "skipped": skipped,
        "entity": entity,
    })
}

fn progress_title(card: &XhsNoteCard) -> Option<String> {
    let title = card.title.trim();
    (!title.is_empty()).then(|| title.to_string())
}

fn report_item_progress(
    ctx: &ToolContext,
    phase: ToolProgressPhase,
    status: ToolProgressStatus,
    current: usize,
    total: usize,
    item_index: Option<usize>,
    title: Option<String>,
) {
    ctx.report_progress(ToolProgressEvent {
        phase,
        status,
        current: current as u64,
        total: total as u64,
        item_index: item_index.map(|value| value as u64),
        title,
    });
}

#[derive(Clone)]
struct ScanProgress {
    ctx: ToolContext,
    total: usize,
    ocr_completed: Arc<AtomicU64>,
}

impl ScanProgress {
    fn new(ctx: &ToolContext, total: usize) -> Self {
        Self {
            ctx: ctx.clone(),
            total,
            ocr_completed: Arc::new(AtomicU64::new(0)),
        }
    }

    fn reading_started(&self, item_index: usize, title: Option<String>) {
        report_item_progress(
            &self.ctx,
            ToolProgressPhase::Reading,
            ToolProgressStatus::ItemStarted,
            item_index.saturating_sub(1),
            self.total,
            Some(item_index),
            title,
        );
    }

    fn reading_completed(&self, item_index: usize, title: Option<String>) {
        report_item_progress(
            &self.ctx,
            ToolProgressPhase::Reading,
            ToolProgressStatus::ItemCompleted,
            item_index,
            self.total,
            Some(item_index),
            title,
        );
    }

    fn ocr_started(&self, item_index: usize, title: Option<String>) {
        report_item_progress(
            &self.ctx,
            ToolProgressPhase::Ocr,
            ToolProgressStatus::ItemStarted,
            self.ocr_completed.load(Ordering::Relaxed) as usize,
            self.total,
            Some(item_index),
            title,
        );
    }

    fn ocr_completed(&self, item_index: usize, title: Option<String>) {
        let current = self.ocr_completed.fetch_add(1, Ordering::Relaxed) + 1;
        report_item_progress(
            &self.ctx,
            ToolProgressPhase::Ocr,
            ToolProgressStatus::ItemCompleted,
            current as usize,
            self.total,
            Some(item_index),
            title,
        );
    }

    fn finish_reading(&self, actual: usize) {
        report_item_progress(
            &self.ctx,
            ToolProgressPhase::Reading,
            ToolProgressStatus::Finished,
            actual,
            actual,
            None,
            None,
        );
    }

    fn finish_ocr(&self, actual: usize) {
        report_item_progress(
            &self.ctx,
            ToolProgressPhase::Ocr,
            ToolProgressStatus::Finished,
            actual,
            actual,
            None,
            None,
        );
    }
}

/// Open one already-selected card, read its body at `level`, attach top
/// comments, and record it in run + cross-run history. Shared by `search`
/// (cards from search) and `author_scan` (cards from a profile page) — the only
/// difference between those macros is where the cards come from, not how each
/// note is read. Returns the per-note entry; the caller pushes it and closes
/// the modal.
#[allow(clippy::too_many_arguments)]
async fn scan_card_note(
    xhs: &XhsPageRuntime<'_>,
    history: &XhsHistoryStore,
    ctx: &ToolContext,
    card: &XhsNoteCard,
    level: &str,
    comment_count: i64,
    include_media: bool,
    download_media: bool,
    ocr: bool,
    // When true, images are downloaded inline but OCR is left to the caller (run
    // in a background task so it overlaps the next note's read+download). The
    // dedup check still uses the real `ocr` flag, so a cache hit returns the
    // already-OCR'd entity and needs no background work.
    defer_ocr: bool,
) -> Value {
    let requested_media = include_media;

    // Dedup applies to every read, including download/OCR runs: if a prior run
    // already covers this note at the requested level + enrichments (vision,
    // downloaded media, OCR), reuse the cached entity instead of re-opening,
    // re-downloading, and re-OCR'ing it. The cached entity carries its prior
    // ocr_text / local_path, so the reuse is complete.
    if !card.note_id.is_empty() && ctx.has_processed_note(&card.note_id, level, requested_media) {
        return skipped_note_entry(card, "already_processed", history);
    }
    if !card.note_id.is_empty()
        && history.is_satisfied_by(&card.note_id, level, requested_media, download_media, ocr)
        // Only short-circuit when we actually have the cached entity to return;
        // a pre-upgrade entry without one is re-read so it backfills the cache
        // instead of degrading to a bare card.
        && history.has_cached_entity(&card.note_id)
    {
        ctx.mark_processed_note(&card.note_id, level, requested_media);
        return skipped_note_entry(card, "already_analyzed", history);
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
                // Inline OCR only when not deferred; otherwise just download and
                // let the caller OCR in the background.
                ocr: ocr && !defer_ocr,
                // Pure downloads are cheap compared with OCR/vision, so allow
                // full XHS carousels instead of the enrichment-oriented default.
                max_images: if download_media { 100 } else { 12 },
                max_video_frames: 4,
                poster_url_fallback: card.cover_url.clone(),
                note_id_fallback: card.note_id.clone(),
            },
        )
        .await;
    let mut entry = match read_result {
        Ok(payload) => {
            let ok = payload.get("ok").and_then(Value::as_bool).unwrap_or(false);
            let mut entity = payload.get("entity").cloned().unwrap_or(Value::Null);
            ensure_entity_note_id(&mut entity, card);
            // When the read failed (modal never opened, stale note, …), fall
            // back to the card so the entry still carries note_id/title, and
            // surface the failure reason for debugging.
            if entity.is_null() {
                entity = serde_json::to_value(card).unwrap_or(Value::Null);
            }
            let mut entry = json!({
                "source_position": card.position,
                "ok": ok,
                "entity": entity,
            });
            if !ok {
                if let Some(map) = entry.as_object_mut() {
                    if let Some(err) = payload.get("error") {
                        map.insert("error".into(), err.clone());
                    }
                    if let Some(open) = payload.get("open") {
                        map.insert("open".into(), open.clone());
                    }
                }
            }
            entry
        }
        Err(e) => json!({
            "source_position": card.position,
            "ok": false,
            "entity": card,
            "error": format!("{e:#}"),
        }),
    };

    // Pull comments separately after waiting for the slower comment list to
    // hydrate. Body content often appears before comments. Scans always include
    // comments — there is no longer a level gate.
    if comment_count > 0 {
        if let Ok(comments_payload) = xhs.extract_comments_with_wait(comment_count, 5.0).await {
            let comments = comments_payload
                .get("comments")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(map) = entry.get_mut("entity").and_then(|v| v.as_object_mut()) {
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

    // The note-level `ocr_text` array is derived at lean-trim time from each
    // image's ocr_text (see lean_scan_note), so nothing to attach here.

    // Mark processed in-run + record in cross-run history.
    if !card.note_id.is_empty() {
        ctx.mark_processed_note(&card.note_id, level, requested_media);
    }
    if entry.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        if let Some(entity) = entry.get("entity") {
            history.record(entity, level, requested_media);
        }
    }

    entry
}

/// Max note OCR tasks in flight at once. The OCR engine serializes inference
/// behind its own mutex, so this mainly bounds decoded-image memory and blocking
/// threads while still letting OCR overlap the browse loop.
const OCR_PIPELINE_CONCURRENCY: usize = 4;

/// Spawn a background task that OCRs a freshly-read note's already-downloaded
/// images, returning the enriched image array. `None` when there's nothing to
/// OCR (no media processor, or no image has a `local_path` yet). Runs
/// concurrently with the browse loop so OCR of note N overlaps the read +
/// download of note N+1.
fn spawn_note_ocr(
    media: &Option<MediaProcessor>,
    sem: &Arc<tokio::sync::Semaphore>,
    entry: &Value,
    epoch: std::time::Instant,
    progress: ScanProgress,
    item_index: usize,
    title: Option<String>,
) -> Option<tokio::task::JoinHandle<NoteOcrResult>> {
    // Only fresh successful reads (they carry `ok`); cache hits carry `skipped`
    // and already have their ocr_text.
    if entry.get("ok").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let images = entry
        .get("entity")
        .and_then(|entity| entity.get("images"))
        .and_then(Value::as_array)
        .cloned()?;
    let has_local = images.iter().any(|image| {
        image
            .get("local_path")
            .and_then(Value::as_str)
            .is_some_and(|path| !path.is_empty())
    });
    if !has_local {
        return None;
    }
    let media = media.clone()?;
    let sem = sem.clone();
    let mut images = images;
    Some(tokio::spawn(async move {
        let _permit = sem.acquire_owned().await;
        progress.ocr_started(item_index, title.clone());
        // Measure wall start→end relative to the shared scan epoch so the perf
        // file can show each note's OCR span on the same timeline as the browse
        // loop (started_ms < browse_ms ⇒ that OCR ran while browsing continued).
        let started_ms = epoch.elapsed().as_millis() as u64;
        let predict = media.ocr_downloaded_images(&mut images).await;
        let finished_ms = epoch.elapsed().as_millis() as u64;
        progress.ocr_completed(item_index, title);
        NoteOcrResult {
            images,
            started_ms,
            finished_ms,
            predict_ms: predict.as_millis() as u64,
        }
    }))
}

/// A note's background OCR result plus its measured timing (ms since the scan
/// epoch). `finished_ms - started_ms` is the note's real OCR wall (including any
/// time blocked on the engine mutex behind other notes); `predict_ms` is the
/// batched inference time alone.
struct NoteOcrResult {
    images: Vec<Value>,
    started_ms: u64,
    finished_ms: u64,
    predict_ms: u64,
}

/// Per-note OCR timing collected by [`join_note_ocr`], keyed by note index.
struct NoteOcrTiming {
    idx: usize,
    started_ms: u64,
    finished_ms: u64,
    predict_ms: u64,
}

/// Await the background OCR tasks and merge each result back into its note:
/// replace the note's images with the OCR'd ones and re-record the now-OCR'd
/// entity in history (so a repeat run finds the OCR in cache). Returns each
/// note's measured OCR wall span for the perf file. Order-independent — each
/// task is keyed by note index.
async fn join_note_ocr(
    notes: &mut [Value],
    pending: Vec<(usize, tokio::task::JoinHandle<NoteOcrResult>)>,
    history: &XhsHistoryStore,
    level: &str,
    include_media: bool,
) -> Vec<NoteOcrTiming> {
    let mut timings = Vec::new();
    for (idx, handle) in pending {
        let Ok(result) = handle.await else {
            continue;
        };
        timings.push(NoteOcrTiming {
            idx,
            started_ms: result.started_ms,
            finished_ms: result.finished_ms,
            predict_ms: result.predict_ms,
        });
        let Some(note) = notes.get_mut(idx) else {
            continue;
        };
        if let Some(entity) = note.get_mut("entity").and_then(Value::as_object_mut) {
            entity.insert("image_count".into(), json!(result.images.len()));
            entity.insert("images".into(), Value::Array(result.images));
        }
        // The note-level `ocr_text` array is derived at lean-trim time from the
        // per-image ocr_text (see lean_scan_note); the recorded entity keeps just
        // the per-image OCR.
        if note.get("ok").and_then(Value::as_bool) == Some(true) {
            if let Some(entity) = note.get("entity") {
                history.record(entity, level, include_media);
            }
        }
    }
    timings
}

/// Trim a search / author_scan bundle to the shape we hand back to the
/// agent/CLI so it doesn't dominate an LLM context window. The full payload is
/// still written to the run artifact (`<run_dir>/artifacts/…json`), which is the
/// place to look for everything dropped here. Diagnostic blocks (`sampling`,
/// `timing`) and the `search` result block are dropped; the opened `notes`
/// (each trimmed by [`lean_scan_note`]) carry the per-note listing, so
/// `author_scan`'s `profile.note_cards` — a duplicate of that listing — is
/// dropped too once notes were read.
fn lean_scan_payload(payload: &mut Value) {
    let Some(obj) = payload.as_object_mut() else {
        return;
    };
    obj.remove("search");
    obj.remove("sampling");
    obj.remove("timing");
    // `ok: true` is pure noise — a successful bundle is self-evident from its
    // notes. Keep `ok: false` (with its `reason`): there it's the failure signal.
    if obj.get("ok").and_then(Value::as_bool) == Some(true) {
        obj.remove("ok");
    }
    let notes_read = obj
        .get("notes")
        .and_then(Value::as_array)
        .is_some_and(|notes| !notes.is_empty());
    // author_scan only: `profile.note_cards` repeats the note listing now
    // carried by `notes`. Keep it only when no notes were opened (preview),
    // where it's the sole listing.
    if notes_read {
        if let Some(profile) = obj.get_mut("profile").and_then(Value::as_object_mut) {
            profile.remove("note_cards");
        }
    }
    if let Some(notes) = obj.get_mut("notes").and_then(Value::as_array_mut) {
        for note in notes.iter_mut() {
            lean_scan_note(note);
        }
    }
}

/// Per-note entity fields kept in the lean output handed to the LLM: the basic
/// human-readable post (title/author/content/date), the engagement counts, the
/// comment texts, and the two locators (`note_id`, `url`). Everything else —
/// hashtags, images, image_count, author ids/urls, location, type, video,
/// content_source, and the wait diagnostics — stays only in the run artifact,
/// which the LLM can re-read by `note_id` when it needs the dropped detail.
const LEAN_NOTE_FIELDS: &[&str] = &[
    "note_id",
    "url",
    "title",
    "author",
    "content",
    "date",
    "likes",
    "favorites",
    "comments_count",
    "top_comments",
    // Per-note OCR summary (joined from each image's ocr_text). Only present
    // when the scan ran with `ocr`; the per-image texts stay in the artifact.
    "ocr_text",
];

/// Trim one scanned-note entry to the lean shape: drop the per-entry provenance
/// wrappers (`source_position`, `ok`, `skipped`, plus failure detail), collapse
/// `top_comments` to a plain array of comment texts, and whitelist the entity to
/// [`LEAN_NOTE_FIELDS`]. Applies to both freshly-read and reused (cached)
/// entities; the full objects stay in the run artifact.
fn lean_scan_note(note: &mut Value) {
    let Some(entry) = note.as_object_mut() else {
        return;
    };
    // The entity is the only thing handed back; the provenance wrappers around
    // it (history dedup status, source ordinal, read status) are diagnostics.
    entry.remove("source_position");
    entry.remove("ok");
    entry.remove("skipped");
    entry.remove("error");
    entry.remove("open");
    let Some(entity) = entry.get_mut("entity") else {
        return;
    };
    // Derive the lean note-level `ocr_text` array from the per-image ocr_text
    // here — while the images array is still present — so the artifact keeps only
    // the per-image OCR (no duplicated note-level copy) and the lean return still
    // carries the index-aligned, cover-first OCR view after images are dropped.
    attach_note_ocr_summary(entity);
    let Some(entity) = entity.as_object_mut() else {
        return;
    };
    // Collapse comment objects to their text before the whitelist runs (the
    // whitelist keeps `top_comments`, but we want the plain-string form).
    if let Some(comments) = entity.get_mut("top_comments").and_then(Value::as_array_mut) {
        let texts: Vec<Value> = comments
            .iter()
            .filter_map(|comment| comment.get("text").cloned())
            .filter(|text| text.as_str().is_some_and(|s| !s.is_empty()))
            .collect();
        entity.insert("top_comments".into(), Value::Array(texts));
    }
    entity.retain(|key, _| LEAN_NOTE_FIELDS.contains(&key.as_str()));
}

/// Surface OCR text on the entity as `ocr_text`: an array of one string per
/// note image, in image order (so the cover — image 0 for an XHS image note —
/// is first). Each entry is that image's recognized text ("" when an image has
/// none). No-op when no image produced any text. Called only during lean
/// trimming (see [`lean_scan_note`]) so the artifact keeps only the per-image
/// `ocr_text`; this is the lean, index-aligned view that survives images being
/// dropped from the returned notes.
fn attach_note_ocr_summary(entity: &mut Value) {
    let Some(images) = entity.get("images").and_then(Value::as_array) else {
        return;
    };
    let texts: Vec<Value> = images
        .iter()
        .map(|image| {
            let text = image
                .get("ocr_text")
                .and_then(Value::as_str)
                .map(|text| truncate(text, 1200))
                .unwrap_or_default();
            Value::String(text)
        })
        .collect();
    let any = texts
        .iter()
        .any(|value| value.as_str().is_some_and(|s| !s.is_empty()));
    if !any {
        return;
    }
    if let Some(map) = entity.as_object_mut() {
        map.insert("ocr_text".into(), Value::Array(texts));
    }
}

/// OCR performance/debug record, written to a separate file
/// (`stats/ocr.json` in the tool-call dir) rather than the LLM-facing JSON
/// artifact. OCR runs as one batched `predict` per note (fast, multi-core), so
/// timing is per note/batch, not per image. The recognized `ocr_text` stays on
/// the images for the LLM; only timing lives here.
///
/// `summary` is the key to reading the pipeline. All `*_ms` are wall time:
///   - `ocr_predict_total_ms` — summed per-note batch inference (total OCR CPU
///     cost).
///   - `ocr_wall_ms` — **measured** first-OCR-start → last-OCR-end. OCR tasks
///     share one engine (serialized), so this ≈ `ocr_predict_total_ms`; the
///     pipeline overlaps OCR with the *browse loop*, not with other OCR.
///   - `browse_loop_ms` — the open→read→download→close loop OCR runs behind.
///   - `ocr_overhang_ms` — OCR still running after the browse loop ended (the
///     part the pipeline could NOT hide).
///   - `scan_total_ms` — whole scan wall.
///
/// Each note carries `predict_ms` (its batch inference), plus
/// `ocr_started_ms`/`ocr_finished_ms`/`ocr_wall_ms` (relative to scan start) so
/// you can see its OCR span on the same timeline as the browse loop.
fn write_note_ocr_perf(
    ctx: &ToolContext,
    notes: &[Value],
    browse_loop_ms: u64,
    timings: &[NoteOcrTiming],
) {
    if timings.is_empty() {
        return;
    }
    let mut note_reports: Vec<Value> = Vec::new();
    let mut total_images: u64 = 0;
    let mut predict_total_ms: u64 = 0;
    for t in timings {
        let Some(note) = notes.get(t.idx) else {
            continue;
        };
        let entity = note.get("entity");
        let note_id = entity
            .and_then(|e| e.get("note_id"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let images = entity
            .and_then(|e| e.get("images"))
            .and_then(Value::as_array)
            .map(|a| a.len())
            .unwrap_or(0) as u64;
        total_images += images;
        predict_total_ms += t.predict_ms;
        note_reports.push(json!({
            "note_id": note_id,
            "images": images,
            "predict_ms": t.predict_ms,
            "ocr_started_ms": t.started_ms,
            "ocr_finished_ms": t.finished_ms,
            "ocr_wall_ms": t.finished_ms.saturating_sub(t.started_ms),
        }));
    }
    if note_reports.is_empty() {
        return;
    }

    // Overall OCR wall = first start → last finish, measured across all notes.
    let ocr_wall_ms = match (
        timings.iter().map(|t| t.started_ms).min(),
        timings.iter().map(|t| t.finished_ms).max(),
    ) {
        (Some(start), Some(end)) => end.saturating_sub(start),
        _ => 0,
    };
    let last_finish = timings.iter().map(|t| t.finished_ms).max().unwrap_or(0);
    let scan_total_ms = browse_loop_ms.max(last_finish);
    let ocr_overhang_ms = last_finish.saturating_sub(browse_loop_ms);

    let perf = json!({
        "ocr": ocr_diagnostics(),
        "summary": {
            "images": total_images,
            "ocr_predict_total_ms": predict_total_ms,
            "ocr_wall_ms": ocr_wall_ms,
            "browse_loop_ms": browse_loop_ms,
            "ocr_overhang_ms": ocr_overhang_ms,
            "scan_total_ms": scan_total_ms,
        },
        "notes": note_reports,
    });
    write_run_perf_file(ctx, &perf);
}

/// OCR performance record for the preview path: all card covers are OCR'd in one
/// batch, so this records the cover count and the batch inference + wall time.
fn write_cover_ocr_perf(
    ctx: &ToolContext,
    covers: usize,
    predict: std::time::Duration,
    wall: std::time::Duration,
) {
    if covers == 0 {
        return;
    }
    let perf = json!({
        "ocr": ocr_diagnostics(),
        "summary": {
            "covers": covers,
            "ocr_predict_ms": predict.as_millis() as u64,
            "ocr_wall_ms": wall.as_millis() as u64,
        },
    });
    write_run_perf_file(ctx, &perf);
}

/// Write the tool-specific OCR record under the current tool call's `stats/`.
fn write_run_perf_file(ctx: &ToolContext, perf: &Value) {
    let stats_dir = ctx.output_dir().join("stats");
    if std::fs::create_dir_all(&stats_dir).is_err() {
        return;
    }
    if let Ok(rendered) = serde_json::to_string_pretty(perf) {
        let _ = std::fs::write(stats_dir.join("ocr.json"), rendered);
    }
}

/// Remove OCR timing keys from the media-timing summary so the JSON artifact's
/// `timing.media` carries only non-OCR (download) timings; OCR timing lives in
/// the dedicated `stats/ocr.json`.
fn strip_ocr_timing(media_timing: &mut Value) {
    if let Some(map) = media_timing.as_object_mut() {
        map.retain(|key, _| !key.starts_with("ocr"));
    }
}

/// Per-note properties that live only in the full scan artifact (dropped from
/// the lean notes by [`lean_scan_note`]). Surfaced in the artifact pointer so
/// the LLM knows what a `note_id` lookup can recover.
const ARTIFACT_EXTRA_NOTE_PROPERTIES: &[&str] = &[
    "hashtags",
    "images (index, url, ocr_text, ocr_ms)",
    "image_count",
    "video",
    "type",
    "author_id",
    "author_url",
    "location",
    "content_source",
    "top_comments (full objects: text, author, likes, time)",
];

/// Per-card properties that live only in the `search --preview` artifact
/// (dropped from the lean cards by [`lean_preview_cards`]).
const ARTIFACT_EXTRA_PREVIEW_CARD_PROPERTIES: &[&str] = &[
    "author_id",
    "author_url",
    "cover_url",
    "xsec_token",
    "position",
    "already_analyzed",
    "history_level",
    "history_include_media",
];

/// Append a pointer to the full run artifact so the LLM knows the lean output is
/// abridged and can re-read the artifact (keyed by each note's `note_id`) for
/// the dropped detail. `extra_properties` lists what the artifact carries beyond
/// the lean fields, so the LLM can decide whether a lookup is worth it without
/// opening the file blind.
fn attach_artifact_pointer(
    payload: &mut Value,
    artifact_path: Option<String>,
    extra_properties: &[&str],
) {
    let Some(path) = artifact_path else {
        return;
    };
    let Some(obj) = payload.as_object_mut() else {
        return;
    };
    obj.insert(
        "artifact".into(),
        json!({
            "path": path,
            "note": "Full untrimmed results. Look up a note by note_id for the fields trimmed below.",
            "extra_note_properties": extra_properties,
        }),
    );
}

/// Per-card fields kept in the lean `search --preview` output. Card-only mode
/// can't fill the body fields (content/date/comments), so it keeps the subset
/// that overlaps the full-scan note shape — note_id, url, title, author, likes —
/// plus `type` (image/video). The card's note link is exposed as `url` to match
/// the note shape (the raw card calls it `link`).
const LEAN_PREVIEW_CARD_FIELDS: &[&str] = &[
    "note_id", "url", "title", "author", "likes", "type",
    // Cover-image OCR text, present only when the preview ran with `ocr`.
    "ocr_text",
];

/// Trim each `search --preview` card to [`LEAN_PREVIEW_CARD_FIELDS`], renaming
/// `link` → `url` for parity with the full-scan note shape. The full cards stay
/// in the run artifact.
fn lean_preview_cards(payload: &mut Value) {
    let Some(cards) = payload.get_mut("cards").and_then(Value::as_array_mut) else {
        return;
    };
    for card in cards.iter_mut() {
        let Some(obj) = card.as_object_mut() else {
            continue;
        };
        if let Some(text) = obj.get("ocr_text").and_then(Value::as_str) {
            obj.insert("ocr_text".into(), json!(truncate(text, 1200)));
        }
        if let Some(link) = obj.remove("link") {
            obj.insert("url".into(), link);
        }
        obj.retain(|key, _| LEAN_PREVIEW_CARD_FIELDS.contains(&key.as_str()));
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
         Specify either `note_id` (from a card returned by `search --preview`) or \
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
        // this note at the requested level + enrichments (vision, downloaded
        // media, OCR). Only fires when note_id is known up front; the cached
        // entity already carries any prior local_path / ocr_text.
        if let Some(id) = note_id.as_deref().filter(|s| !s.trim().is_empty()) {
            if self.history.is_satisfied_by(
                id,
                &options.level,
                options.include_media,
                options.download_media,
                options.ocr,
            ) {
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
        let mut value = xhs
            .read_note_with_options(
                note_id.as_deref().unwrap_or(""),
                index,
                wait_seconds,
                options.clone(),
            )
            .await?;
        if value.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            // The note modal is still open here (the command's `after` hook
            // closes it), so always attach top comments before recording.
            if let Some(entity) = value.get_mut("entity") {
                attach_top_comments(&xhs, entity).await;
            }
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
        "Extract the currently visible note from the page (body + top comments). \
         Assumes the user already navigated to a note URL or has the detail \
         modal open."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "wait_seconds": { "type": "number", "default": 8.0 },
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
        let mut value = serde_json::to_value(&note)?;
        // Always attach top comments — the note is already open, so reading them
        // is one extra DOM read with no extra navigation.
        attach_top_comments(&xhs, &mut value).await;
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
         (without re-running the search). Useful after applying filters to \
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

/// search(query, filters?, num_notes?, download_media?, preview?) -> aggregated bundle
///
/// The single Xiaohongshu search tool. Composite macro: search → optional
/// search filters → collect up to `num_notes` cards in page order (scrolling
/// the feed only when the first page is too small) → open each note and extract
/// its body + top comments → bundle into one artifact. Prefer this for any
/// "research a topic on XHS" task — it returns search results plus the note
/// bodies plus comments in one tool call, so the agent doesn't have to chain
/// 10+ tools by hand.
///
/// With `preview = true` it returns result cards only (titles/likes/covers)
/// without opening any note — the fast cards-only path exposed on the CLI as
/// `search --preview`.
///
/// Defaults to `DEFAULT_NUM_NOTES` notes; pass a larger `num_notes` to scan
/// more (each note is opened, so latency grows roughly linearly).
pub struct SearchTool {
    page: Arc<PageSession>,
    llm_provider: Option<Arc<dyn LlmProvider>>,
    history: Arc<XhsHistoryStore>,
    /// Force media download regardless of the `download_media` input. Set by the
    /// app/TUI macro factory; the CLI/full set leaves the input in control.
    always_download_media: bool,
    /// Force OCR (and therefore download) of every note image regardless of the
    /// `ocr` input. Set by the app/TUI macro factory so the desktop agent always
    /// gets OCR; the CLI/full set leaves it to the `--ocr` flag.
    always_ocr: bool,
}

fn effective_macro_input(
    input: &Value,
    default_num_notes: Option<i64>,
    always_download_media: bool,
    always_ocr: bool,
) -> Value {
    let mut effective = input.clone();
    let preview = get_bool(&effective, "preview", false);
    if effective.get("num_notes").is_none() {
        if let Some(default) = default_num_notes {
            effective["num_notes"] = json!(default);
        }
    }
    let ocr = always_ocr || get_bool(&effective, "ocr", false);
    if ocr {
        effective["ocr"] = json!(true);
    }
    if preview {
        if let Some(object) = effective.as_object_mut() {
            object.remove("download_media");
        }
    } else if always_download_media || ocr || get_bool(&effective, "download_media", false) {
        effective["download_media"] = json!(true);
    }
    effective
}

#[async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }

    fn description(&self) -> &str {
        "Xiaohongshu search — the single XHS search tool. Default (full scan): \
         search → optional search filters → collect up to `num_notes` cards in \
         page order (scrolling only if the first page is too small) → open each \
         note and read its body + top comments → return one compact bundle \
         (search results + selected cards + note bodies + comments). Pass \
         `download_media=true` to download note images/videos into the run dir, \
         include local paths, and emit a stable media_manifest_path. Pass \
         `ocr=true` to also OCR every downloaded image locally (PP-OCRv6 small) \
         and attach a per-note ocr_text (implies download_media). Pass \
         `preview=true` for a fast cards-only pass that returns result cards \
         (titles/likes/covers) without opening any note. Defaults to 10 notes; \
         pass a larger `num_notes` to scan more (each note is opened, so latency \
         scales with it). Prefer this for XHS topic/keyword research. Do not \
         repeat the same search unless the previous one was clearly insufficient."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "filters": search_filters_schema(),
                "num_notes": {
                    "type": "integer",
                    "description": "Number of notes to read (body + top comments each). The first results page is used directly; only if it holds fewer than this does the feed scroll for more. Each note is opened, so latency scales with this. In preview mode, the number of cards to collect by scrolling.",
                    "default": DEFAULT_NUM_NOTES,
                    "minimum": 1
                },
                "download_media": {
                    "type": "boolean",
                    "description": "Download note images/videos into the command run_dir, include local_path fields in returned notes, and write a stable media_manifest.json surfaced by media_manifest_path. Ignored in preview mode.",
                    "default": false
                },
                "ocr": {
                    "type": "boolean",
                    "description": "Run local OCR (PP-OCRv6 small). Full scan: OCR every downloaded note image (implies download_media); each returned note gets ocr_text as an array of per-image strings (image order, cover first). Preview: OCR each card's cover image and attach its ocr_text. Per-image ocr_text/ocr_ms and OCR diagnostics are kept in the artifact.",
                    "default": false
                },
                "preview": {
                    "type": "boolean",
                    "description": "Fast cards-only mode: return result cards (titles/likes/covers) without opening notes or reading bodies/comments. Off by default (full scan).",
                    "default": false
                }
            },
            "required": ["query"]
        })
    }

    fn effective_input(&self, input: &Value) -> Value {
        effective_macro_input(
            input,
            Some(DEFAULT_NUM_NOTES),
            self.always_download_media,
            self.always_ocr,
        )
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let query = get_str(&input, "query")
            .ok_or_else(|| anyhow::anyhow!("missing query"))?
            .to_string();
        let filters = input
            .get("filters")
            .filter(|value| !value.is_null())
            .cloned();

        // Preview mode: return result cards only (titles/likes/covers) without
        // opening notes. This is the cards-only fast path surfaced on the CLI as
        // `search --preview` (formerly the standalone `search_notes` tool).
        if get_bool(&input, "preview", false) {
            // num_notes defaults to DEFAULT_NUM_NOTES to match the schema and
            // the full-scan path: collect at least that many cards, scrolling
            // only if the first page holds fewer.
            let num_notes = Some(get_i64(&input, "num_notes", DEFAULT_NUM_NOTES).max(1) as usize);
            // Warm the OCR engine off the critical path so cover OCR doesn't pay
            // model-load latency; overlaps the search + card collection below.
            let ocr = self.always_ocr || get_bool(&input, "ocr", false);
            if ocr {
                tokio::task::spawn_blocking(ocr_warm_up);
            }
            let xhs = XhsPageRuntime::new(&self.page);
            let mut value = xhs
                .search_notes(&query, filters.as_ref(), SEARCH_WAIT_SECONDS, num_notes)
                .await?;
            if let Some(cards) = value.get_mut("cards") {
                self.history.annotate_cards(cards);
            }
            // Preview OCR: read each result card's cover image (like a human
            // glancing at the results page). Covers are fetched to memory and
            // OCR'd without being saved to the run dir.
            if ocr {
                if let Some(cards) = value.get_mut("cards").and_then(Value::as_array_mut) {
                    report_item_progress(
                        ctx,
                        ToolProgressPhase::Ocr,
                        ToolProgressStatus::ItemStarted,
                        0,
                        cards.len(),
                        None,
                        None,
                    );
                    let media = MediaProcessor::for_run_dir(ctx.output_dir(), None)?;
                    let cover_t0 = std::time::Instant::now();
                    let predict = media.ocr_cover_images(cards, XHS_HOME_URL).await;
                    let wall = cover_t0.elapsed();
                    // Cover OCR timing goes to the perf file; cards keep only ocr_text.
                    write_cover_ocr_perf(ctx, cards.len(), predict, wall);
                    report_item_progress(
                        ctx,
                        ToolProgressPhase::Ocr,
                        ToolProgressStatus::Finished,
                        cards.len(),
                        cards.len(),
                        None,
                        None,
                    );
                }
            }
            let failed = value.get("ok").and_then(Value::as_bool) == Some(false);
            // On success, persist the full card bundle as an artifact (same as
            // the full scan) so the trimmed return can point back at it. A failed
            // preview has no cards worth keeping — its `submit`/`reason` are the
            // only useful detail — so it isn't persisted or trimmed.
            if !failed {
                let artifact_path = ctx
                    .write_json_artifact(
                        &format!("xhs_search_preview_{}", sanitize_for_filename(&query)),
                        &value,
                        "artifacts",
                        "search",
                        "json",
                        &format!(
                            "Search preview: {query} ({} cards)",
                            value
                                .get("cards")
                                .and_then(Value::as_array)
                                .map(Vec::len)
                                .unwrap_or(0)
                        ),
                        json!({"site": "xhs", "category": "search_preview"}),
                    )
                    .ok()
                    .map(|rel| ctx.run_dir.join(rel).to_string_lossy().into_owned());
                if let Some(obj) = value.as_object_mut() {
                    // `submit` is the search-submission diagnostic (strategy +
                    // page-state echo), `reason` is empty on success, and `ok`
                    // is self-evident — drop them to match the full-scan output.
                    obj.remove("submit");
                    obj.remove("reason");
                    obj.remove("ok");
                }
                lean_preview_cards(&mut value);
                attach_artifact_pointer(
                    &mut value,
                    artifact_path,
                    ARTIFACT_EXTRA_PREVIEW_CARD_PROPERTIES,
                );
            }
            return Ok(json_result(&value));
        }

        let num_notes = get_i64(&input, "num_notes", DEFAULT_NUM_NOTES).max(1);
        // Every scanned note is read the same way: open it, extract the body,
        // and pull top comments. Per-note image vision is off (it's the one
        // genuinely expensive enrichment and not needed for topic research).
        let include_media = false;
        // OCR implies download (it reads the saved files).
        let ocr = self.always_ocr || get_bool(&input, "ocr", false);
        let download_media =
            ocr || self.always_download_media || get_bool(&input, "download_media", false);
        // Warm the OCR engine off the critical path (model load + session init +
        // graph compile) so the first note's OCR doesn't pay it; overlaps the
        // search submit + first note open below.
        if ocr {
            tokio::task::spawn_blocking(ocr_warm_up);
        }

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

        // Filters are applied after the initial search below, so don't pass
        // them here.
        let search = xhs
            .search_notes(&query, None, SEARCH_WAIT_SECONDS, None)
            .await?;

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
            let mut payload = json!({
                "ok": false,
                "query": query,
                "search": search,
                "notes": [],
                "reason": reason,
                "sampling": {
                    "num_notes": num_notes,
                    "selected": 0,
                    "comments_per_note": TOP_COMMENTS_PER_NOTE,
                    "include_media": include_media,
                    "download_media": download_media,
                    "ocr": ocr,
                },
            });
            // `reason` already summarizes the failure; keep the failed scan
            // compact too.
            lean_scan_payload(&mut payload);
            return Ok(json_result(&payload));
        }

        // Optional filter application.
        let mut filter_result = Value::Object(serde_json::Map::new());
        if let Some(filters) = filters {
            filter_result = xhs.apply_search_filters(&filters, 1.5).await?;
        }

        // Every sampled note is read with the same extraction level (body +
        // top comments).
        let level = "deep";
        let comment_count = TOP_COMMENTS_PER_NOTE;
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
        // OCR runs in the background so it overlaps the next note's read +
        // download; tasks are joined after the browse loop.
        let ocr_sem = Arc::new(tokio::sync::Semaphore::new(OCR_PIPELINE_CONCURRENCY));
        let mut pending_ocr: Vec<(usize, tokio::task::JoinHandle<NoteOcrResult>)> = Vec::new();
        let scan_progress = ScanProgress::new(ctx, want);
        // Wall-clock markers so the perf file can show how much OCR overlapped
        // the browse loop (the pipeline benefit) vs. spilled past it.
        let browse_t0 = std::time::Instant::now();

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
                ctx.add_search_note_ids(std::slice::from_ref(&card.note_id));
            }
            selected.push(card.clone());
            let item_index = notes.len() + 1;
            let title = progress_title(&card);
            scan_progress.reading_started(item_index, title.clone());

            let entry = scan_card_note(
                &xhs,
                &self.history,
                ctx,
                &card,
                level,
                comment_count,
                include_media,
                download_media,
                ocr,
                // Defer OCR to a background task when OCR is on, so the loop can
                // move on to the next note's read + download immediately.
                ocr,
            )
            .await;
            notes.push(entry);
            let idx = notes.len() - 1;
            if ocr {
                if let Some(handle) = spawn_note_ocr(
                    &media,
                    &ocr_sem,
                    &notes[idx],
                    browse_t0,
                    scan_progress.clone(),
                    item_index,
                    title.clone(),
                ) {
                    pending_ocr.push((idx, handle));
                } else {
                    scan_progress.ocr_completed(item_index, title.clone());
                }
            }
            let _ = xhs.close_note(0.6).await;
            scan_progress.reading_completed(item_index, title);
        }

        scan_progress.finish_reading(notes.len());
        let browse_ms = browse_t0.elapsed().as_millis() as u64;
        // Join background OCR (epoch = browse start, so the timings line up with
        // browse_ms) and merge results back into the notes in place.
        let ocr_timings =
            join_note_ocr(&mut notes, pending_ocr, &self.history, level, include_media).await;
        // Write OCR perf to a separate debug file and strip per-image ocr_ms from
        // the notes so the JSON artifact stays LLM-facing (ocr_text only).
        if ocr {
            write_note_ocr_perf(ctx, &notes, browse_ms, &ocr_timings);
            scan_progress.finish_ocr(notes.len());
        }

        let mut media_timing = match (&media, &media_baseline) {
            (Some(media), Some(before)) => timing_delta(before, &media.timing().snapshot()),
            _ => json!({}),
        };
        strip_ocr_timing(&mut media_timing);

        // Annotate cards in the search payload against the pre-call snapshot so
        // flags reflect "known before this scan" rather than "known after this
        // scan's own writes". (Only kept in the artifact; the opened notes are
        // the returned listing.)
        let mut search = search;
        if let Some(cards) = search.get_mut("cards") {
            history_snapshot.annotate_cards(cards);
        }

        let media_manifest_metadata = if download_media {
            let media_manifest = search_media_manifest(&notes, ctx.output_dir());
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
            "filters": filter_result,
            "search": search,
            "notes": notes,
            "sampling": {
                "num_notes": num_notes,
                "selected": selected.len(),
                "comments_per_note": TOP_COMMENTS_PER_NOTE,
                "include_media": include_media,
                "download_media": download_media,
                "ocr": ocr,
            },
            "timing": {
                "media": media_timing,
            }
        });
        // OCR perf (model / EP / machine / per-image timing) is written to
        // `stats/ocr.json` (see write_note_ocr_perf above), not the JSON artifact.
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
        // `write_json_artifact` returns the run-dir-relative path; the pointer we
        // hand the LLM uses the absolute path so it's directly openable.
        let artifact_path = ctx
            .write_json_artifact(
                &format!("xhs_search_{}", sanitize_for_filename(&query)),
                &payload,
                "artifacts",
                "search",
                "json",
                &format!(
                    "Search: {query} ({} notes)",
                    payload
                        .get("notes")
                        .and_then(Value::as_array)
                        .map(Vec::len)
                        .unwrap_or(0)
                ),
                json!({"site": "xhs", "category": "search"}),
            )
            .ok()
            .map(|rel| ctx.run_dir.join(rel).to_string_lossy().into_owned());

        // Artifact above keeps the full bundle; trim what we return so the
        // agent/CLI output stays small, then point at the artifact for the rest.
        lean_scan_payload(&mut payload);
        attach_artifact_pointer(&mut payload, artifact_path, ARTIFACT_EXTRA_NOTE_PROPERTIES);
        Ok(json_result(&payload))
    }
}

/// author_scan(author_id, num_notes?, preview?, download_media?) -> author profile bundle
///
/// Composite macro mirroring `search`, but entered from an author's profile
/// page instead of a search query: open `…/user/profile/<id>` → read the author
/// header (bio, xhs id, IP location, follower/following/like counts) → collect
/// note summary cards in page order (scrolling to reach `num_notes`) → by
/// default open each note and read its body + top comments. With `preview =
/// true` it returns the note cards only, without opening any note — the fast
/// cards-only path exposed on the CLI as `author --preview`.
pub struct AuthorScanTool {
    page: Arc<PageSession>,
    history: Arc<XhsHistoryStore>,
    /// Force media download regardless of the `download_media` input. Set by the
    /// app/TUI macro factory; the CLI/full set leaves the input in control.
    always_download_media: bool,
    /// Force OCR (and therefore download) of every note image regardless of the
    /// `ocr` input. Set by the app/TUI macro factory; the CLI/full set leaves it
    /// to the `--ocr` flag.
    always_ocr: bool,
}

#[async_trait]
impl Tool for AuthorScanTool {
    fn name(&self) -> &str {
        "author_scan"
    }

    fn description(&self) -> &str {
        "Xiaohongshu author/creator scan: open an author's profile page by id → \
         read the author header (display name, xhs id, bio, IP location, \
         follower/following/liked-&-collected counts) → collect their note \
         summary cards in page order (pass `num_notes` to scroll the grid for \
         more, omit for just the first screen) → open each collected note and \
         read its body + top comments (like `search`; latency scales with the \
         card count). Pass `ocr=true` to also OCR every downloaded image locally \
         (PP-OCRv6 small) and attach a per-note ocr_text (implies download_media). \
         Pass `preview=true` for a fast cards-only pass that \
         returns the note cards (titles/likes/covers) without opening any note. \
         Use this for creator research — it's like `search` but scoped to one \
         author instead of a query."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "author_id": {
                    "type": "string",
                    "description": "Xiaohongshu user id — the trailing segment of /user/profile/<id>."
                },
                "num_notes": {
                    "type": "integer",
                    "description": "Collect at least this many note cards, scrolling the profile grid (lazy-loaded). Omit for the first screen only.",
                    "minimum": 1
                },
                "preview": {
                    "type": "boolean",
                    "description": "Fast cards-only mode: return the note cards (titles/likes/covers) without opening notes or reading bodies/comments. Off by default (full scan: each note opened for its body + top comments).",
                    "default": false
                },
                "download_media": {
                    "type": "boolean",
                    "description": "Download each note's images/videos into the run dir, include local_path fields, and emit a stable media_manifest_path. Ignored in preview mode.",
                    "default": false
                },
                "ocr": {
                    "type": "boolean",
                    "description": "Run local OCR (PP-OCRv6 small) on every downloaded note image, attaching per-image ocr_text in the artifact and a joined per-note ocr_text in the returned notes. Implies download_media. Ignored in preview mode.",
                    "default": false
                }
            },
            "required": ["author_id"]
        })
    }

    fn effective_input(&self, input: &Value) -> Value {
        effective_macro_input(input, None, self.always_download_media, self.always_ocr)
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let author_id = get_str(&input, "author_id")
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow::anyhow!("missing author_id"))?
            .to_string();
        let preview = get_bool(&input, "preview", false);
        // OCR applies in both modes: a full scan OCRs every downloaded image, a
        // preview OCRs each note card's cover. download_media only applies when
        // notes are opened, so gate it on a full scan — without this, a
        // `preview=true, download_media=true` call would still spin up the media
        // pipeline and emit media metadata even though no note is opened.
        let ocr = self.always_ocr || get_bool(&input, "ocr", false);
        let download_media = !preview
            && (ocr || self.always_download_media || get_bool(&input, "download_media", false));
        // Warm the OCR engine off the critical path; overlaps opening the profile
        // and collecting note cards below.
        if ocr {
            tokio::task::spawn_blocking(ocr_warm_up);
        }
        let num_notes = input
            .get("num_notes")
            .and_then(Value::as_i64)
            .filter(|n| *n > 0)
            .map(|n| n as usize);

        // Media processor only needed when downloading note media (no vision,
        // so no LLM provider required).
        let media = media_for(ctx, None, download_media)?;
        let media_baseline: Option<TimingSnapshot> = media.as_ref().map(|m| m.timing().snapshot());
        let xhs = XhsPageRuntime::new_with_media(&self.page, media.clone());
        // Snapshot history before reading so card annotations reflect "known
        // before this scan" rather than this scan's own writes.
        let history_snapshot = self.history.snapshot();

        let open = xhs.open_profile(&author_id, 8.0).await?;
        if !open.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            let reason = open
                .get("reason")
                .and_then(Value::as_str)
                .filter(|reason| !reason.is_empty())
                .unwrap_or("open_profile_failed")
                .to_string();
            return Ok(json_result(&json!({
                "ok": false,
                "author_id": author_id,
                "open": open,
                "profile": Value::Null,
                "notes": [],
                "reason": reason,
            })));
        }

        let info = xhs.profile_info().await?;
        let cards = match num_notes {
            Some(target) => xhs.collect_profile_cards(target).await?,
            None => xhs.extract_profile_cards_once().await?,
        };

        let profile_url = {
            let candidate = get_str(&info, "profile_url").unwrap_or("");
            if candidate.is_empty() {
                open.get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            } else {
                candidate.to_string()
            }
        };
        let display_name = get_str(&info, "display_name").unwrap_or("").to_string();
        let profile = XhsAuthorProfile {
            display_name: display_name.clone(),
            xhs_id: get_str(&info, "xhs_id").unwrap_or("").to_string(),
            profile_url,
            bio: get_str(&info, "bio").unwrap_or("").to_string(),
            ip_location: get_str(&info, "ip_location").unwrap_or("").to_string(),
            followers: get_str(&info, "followers").unwrap_or("").to_string(),
            following: get_str(&info, "following").unwrap_or("").to_string(),
            likes_and_collections: get_str(&info, "likes_and_collections")
                .unwrap_or("")
                .to_string(),
            note_cards: cards.clone(),
        };
        let mut profile_value = profile.to_value();
        if let Some(cards_value) = profile_value.get_mut("note_cards") {
            history_snapshot.annotate_cards(cards_value);
        }

        // By default open each collected note and read body + top comments;
        // `preview` returns the cards only and skips this.
        let mut notes: Vec<Value> = Vec::new();
        if !preview {
            // OCR runs in the background so it overlaps the next note's read +
            // download; tasks are joined after the loop.
            let ocr_sem = Arc::new(tokio::sync::Semaphore::new(OCR_PIPELINE_CONCURRENCY));
            let mut pending_ocr: Vec<(usize, tokio::task::JoinHandle<NoteOcrResult>)> = Vec::new();
            let scan_progress = ScanProgress::new(ctx, cards.len());
            let browse_t0 = std::time::Instant::now();
            for card in &cards {
                if !card.note_id.is_empty() {
                    ctx.add_search_note_ids(std::slice::from_ref(&card.note_id));
                }
                let item_index = notes.len() + 1;
                let title = progress_title(card);
                scan_progress.reading_started(item_index, title.clone());
                let entry = scan_card_note(
                    &xhs,
                    &self.history,
                    ctx,
                    card,
                    "deep",
                    TOP_COMMENTS_PER_NOTE,
                    false,
                    download_media,
                    ocr,
                    ocr,
                )
                .await;
                notes.push(entry);
                let idx = notes.len() - 1;
                if ocr {
                    if let Some(handle) = spawn_note_ocr(
                        &media,
                        &ocr_sem,
                        &notes[idx],
                        browse_t0,
                        scan_progress.clone(),
                        item_index,
                        title.clone(),
                    ) {
                        pending_ocr.push((idx, handle));
                    } else {
                        scan_progress.ocr_completed(item_index, title.clone());
                    }
                }
                let _ = xhs.close_note(0.6).await;
                scan_progress.reading_completed(item_index, title);
            }
            scan_progress.finish_reading(notes.len());
            let browse_ms = browse_t0.elapsed().as_millis() as u64;
            let ocr_timings =
                join_note_ocr(&mut notes, pending_ocr, &self.history, "deep", false).await;
            if ocr {
                write_note_ocr_perf(ctx, &notes, browse_ms, &ocr_timings);
                scan_progress.finish_ocr(notes.len());
            }
        } else if ocr {
            // Preview + OCR: read each note card's cover image (fetched to
            // memory, not saved), mirroring the search preview path.
            if let Some(cards_value) = profile_value
                .get_mut("note_cards")
                .and_then(Value::as_array_mut)
            {
                report_item_progress(
                    ctx,
                    ToolProgressPhase::Ocr,
                    ToolProgressStatus::ItemStarted,
                    0,
                    cards_value.len(),
                    None,
                    None,
                );
                let cover_media = MediaProcessor::for_run_dir(ctx.output_dir(), None)?;
                let covers = cards_value.len();
                let cover_t0 = std::time::Instant::now();
                let predict = cover_media
                    .ocr_cover_images(cards_value, XHS_HOME_URL)
                    .await;
                write_cover_ocr_perf(ctx, covers, predict, cover_t0.elapsed());
                report_item_progress(
                    ctx,
                    ToolProgressPhase::Ocr,
                    ToolProgressStatus::Finished,
                    covers,
                    covers,
                    None,
                    None,
                );
            }
        }

        let mut media_timing = match (&media, &media_baseline) {
            (Some(media), Some(before)) => timing_delta(before, &media.timing().snapshot()),
            _ => json!({}),
        };
        strip_ocr_timing(&mut media_timing);

        // Build the media manifest from `notes` before they move into payload.
        let media_manifest_metadata = if download_media {
            let media_manifest = search_media_manifest(&notes, ctx.output_dir());
            let media_manifest_count = media_manifest.as_array().map(Vec::len).unwrap_or_default();
            let (path, error) = match write_media_manifest_file(ctx, &media_manifest) {
                Ok(path) => (Some(path), None),
                Err(err) => (None, Some(format!("{err:#}"))),
            };
            Some((media_manifest_count, path, error))
        } else {
            None
        };

        let mut payload = json!({
            "ok": true,
            "author_id": author_id,
            "profile": profile_value,
            "notes": notes,
            "sampling": {
                "num_notes": num_notes,
                "collected": cards.len(),
                "preview": preview,
                "comments_per_note": if preview { 0 } else { TOP_COMMENTS_PER_NOTE },
                "download_media": download_media,
                "ocr": ocr,
            },
            "timing": { "media": media_timing },
        });

        // OCR perf is written to `stats/ocr.json` (see above), not the artifact.

        if let Some((count, path, error)) = media_manifest_metadata {
            if let Some(map) = payload.as_object_mut() {
                map.insert("media_manifest_count".into(), json!(count));
                if let Some(path) = path {
                    map.insert("media_manifest_path".into(), json!(path));
                }
                if let Some(error) = error {
                    map.insert("media_manifest_error".into(), json!(error));
                }
            }
        }

        // Persist as artifact so it shows up in the run dir + working memory.
        let label = if display_name.is_empty() {
            author_id.clone()
        } else {
            display_name
        };
        let artifact_path = ctx
            .write_json_artifact(
                &format!("xhs_author_scan_{}", sanitize_for_filename(&author_id)),
                &payload,
                "artifacts",
                "author_scan",
                "json",
                &format!("Author scan: {label} ({} notes)", cards.len()),
                json!({"site": "xhs", "category": "author_scan"}),
            )
            .ok()
            .map(|rel| ctx.run_dir.join(rel).to_string_lossy().into_owned());

        // Artifact above keeps the full bundle; trim what we return so the
        // agent/CLI output stays small, then point at the artifact for the rest.
        lean_scan_payload(&mut payload);
        attach_artifact_pointer(&mut payload, artifact_path, ARTIFACT_EXTRA_NOTE_PROPERTIES);
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

    #[test]
    fn run_metadata_includes_media_dir_only_when_requested() {
        let ctx = ToolContext::new("run-1", "/tmp/socai-run");

        let without_media = crate::sites::runner::run_metadata(&ctx, false);
        assert_eq!(without_media["id"], json!("run-1"));
        assert_eq!(without_media["dir"], json!("/tmp/socai-run"));
        assert!(without_media.get("media_dir").is_none());

        let with_media = crate::sites::runner::run_metadata(&ctx, true);
        assert_eq!(with_media["media_dir"], json!("/tmp/socai-run/site_media"));
    }

    #[test]
    fn attach_run_metadata_preserves_existing_run_object() {
        let run = json!({"id": "outer", "dir": "/tmp/outer"});
        let data = crate::sites::runner::attach_run_metadata(json!({"run": {"id": "inner"}}), &run);
        assert_eq!(data["run"]["id"], json!("inner"));
    }

    #[test]
    fn search_command_includes_run_metadata() {
        assert!(SEARCH_COMMAND.include_run_metadata);
        assert!(!AUTHOR_SCAN_COMMAND.include_run_metadata);
    }
}
