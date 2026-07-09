//! Agent-callable tool wrappers around [`XhsPageRuntime`].
//!
//! Each wrapper owns an `Arc<PageSession>` — the same tab is reused across
//! tool calls so the agent's actions accumulate state (search results
//! visible, note modal open, etc.). The caller is responsible for creating
//! the page and closing it after `run_agent` returns.

use std::path::Path;
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
use crate::sites::xhs::entities::{parse_posted_at_ms, parse_stat_count};
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
const TOP_COMMENTS_PER_NOTE: i64 = 8;

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
    let pro = crate::cloud::pro_activated();
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
            pro,
        }),
        Arc::new(ExtractNoteTool {
            page: page.clone(),
            llm_provider: llm_provider.clone(),
            history: history.clone(),
            pro,
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
            pro,
        }),
        Arc::new(AuthorScanTool {
            page: page.clone(),
            history,
            always_download_media: false,
            always_ocr: false,
            pro,
        }),
        Arc::new(WaitForLoginTool { page: page.clone() }),
        Arc::new(PageStateTool { page }),
    ]
}

pub fn xhs_macro_tools_with_llm_provider(
    page: Arc<PageSession>,
    llm_provider: Option<Arc<dyn LlmProvider>>,
) -> Vec<Arc<dyn Tool>> {
    let history = Arc::new(XhsHistoryStore::open_default());
    let pro = crate::cloud::pro_activated();
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
            pro,
        }) as Arc<dyn Tool>,
        Arc::new(AuthorScanTool {
            page: page.clone(),
            history,
            always_download_media: true,
            always_ocr: true,
            pro,
        }),
        Arc::new(WaitForLoginTool { page }),
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
                    key: "num_comments",
                    long: Some("num-comments"),
                    value_name: "N",
                    help: "Comments to load per note, scrolling the comment area and expanding \
                           reply threads to reach it (replies count toward N). Default 8; \
                           ignored with --preview.",
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
                    help: "OCR each opened note (PP-OCRv6 small, local): every carousel \
                           image, or a video note's cover. Downloads what it reads on its \
                           own; video files still require --download-media. With --preview, \
                           OCRs each result card's cover instead.",
                    required: false,
                    kind: ArgKind::Flag,
                },
                CommandArg {
                    key: "transcribe_audio",
                    long: Some("transcribe-audio"),
                    value_name: "TRANSCRIBE_AUDIO",
                    help: "For opened video notes, download the video file and transcribe audio \
                           through socai pro. Ignored with --preview.",
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
                    key: "num_comments",
                    long: Some("num-comments"),
                    value_name: "N",
                    help: "Comments to load per note, scrolling the comment area and expanding \
                           reply threads to reach it (replies count toward N). Default 8; \
                           ignored with --preview.",
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
                    help: "OCR each opened note (PP-OCRv6 small, local): every carousel \
                           image, or a video note's cover. Downloads what it reads on its \
                           own; video files still require --download-media. With --preview, \
                           OCRs each note card's cover instead.",
                    required: false,
                    kind: ArgKind::Flag,
                },
                CommandArg {
                    key: "transcribe_audio",
                    long: Some("transcribe-audio"),
                    value_name: "TRANSCRIBE_AUDIO",
                    help: "For opened video notes, download the video file and transcribe audio \
                           through socai pro. Ignored with --preview.",
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
        let num_comments = args.get("num_comments").and_then(Value::as_i64);
        let preview = args
            .get("preview")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // OCR applies in both modes: a full scan OCRs every downloaded image, a
        // --preview pass OCRs each result card's cover (like a human scanning the
        // results page). So `ocr` is not gated on !preview.
        let ocr = args.get("ocr").and_then(Value::as_bool).unwrap_or(false);
        // download_media (full media into the run dir) doesn't apply to a
        // card-only (--preview) read. Passed through explicitly — OCR alone
        // downloads only what it reads (images / video poster), not video files.
        let download_media = !preview
            && args
                .get("download_media")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        let transcribe_audio = !preview
            && args
                .get("transcribe_audio")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        search_command(
            page,
            "search",
            &query,
            filters.as_ref(),
            num_notes,
            num_comments,
            download_media,
            ocr,
            transcribe_audio,
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
        let num_comments = args.get("num_comments").and_then(Value::as_i64);
        // Default opens each note; --preview returns cards only.
        let preview = args
            .get("preview")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // OCR applies in both modes: a full scan OCRs every downloaded image, a
        // --preview pass OCRs each note card's cover. download_media (full media
        // into the run dir) only applies when notes are opened (not preview).
        // Passed through explicitly — OCR alone downloads only what it reads
        // (images / video poster), not video files.
        let ocr = args.get("ocr").and_then(Value::as_bool).unwrap_or(false);
        let download_media = !preview
            && args
                .get("download_media")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        let transcribe_audio = !preview
            && args
                .get("transcribe_audio")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        author_scan_command(
            page,
            &author_id,
            num_notes,
            num_comments,
            preview,
            download_media,
            ocr,
            transcribe_audio,
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
#[allow(clippy::too_many_arguments)]
pub async fn search_command(
    page: Arc<PageSession>,
    command_name: &'static str,
    query: &str,
    filters: Option<&Value>,
    num_notes: Option<i64>,
    num_comments: Option<i64>,
    download_media: bool,
    ocr: bool,
    transcribe_audio: bool,
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
        search_input(
            query,
            filters,
            num_notes,
            num_comments,
            download_media,
            ocr,
            transcribe_audio,
            preview,
        )?,
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
    num_comments: Option<i64>,
    preview: bool,
    download_media: bool,
    ocr: bool,
    transcribe_audio: bool,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> anyhow::Result<Value> {
    run_xhs_tool_command(
        page,
        AUTHOR_SCAN_COMMAND,
        author_scan_input(
            author_id,
            num_notes,
            num_comments,
            preview,
            download_media,
            ocr,
            transcribe_audio,
        )?,
        debug_snapshot,
        progress,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
fn search_input(
    query: &str,
    filters: Option<&Value>,
    num_notes: Option<i64>,
    num_comments: Option<i64>,
    download_media: bool,
    ocr: bool,
    transcribe_audio: bool,
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
    if let Some(n) = num_comments {
        input["num_comments"] = json!(n.max(0));
    }
    if download_media {
        input["download_media"] = json!(true);
    }
    if ocr {
        input["ocr"] = json!(true);
    }
    if transcribe_audio {
        input["transcribe_audio"] = json!(true);
    }
    if preview {
        input["preview"] = json!(true);
    }
    Ok(input)
}

fn author_scan_input(
    author_id: &str,
    num_notes: Option<i64>,
    num_comments: Option<i64>,
    preview: bool,
    download_media: bool,
    ocr: bool,
    transcribe_audio: bool,
) -> anyhow::Result<Value> {
    let mut input = json!({
        "author_id": trimmed_required(author_id, "author_id")?,
    });
    if let Some(n) = num_notes {
        input["num_notes"] = json!(n.max(1));
    }
    if let Some(n) = num_comments {
        input["num_comments"] = json!(n.max(0));
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
    if transcribe_audio {
        input["transcribe_audio"] = json!(true);
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
    let transcribe_audio = get_bool(input, "transcribe_audio", false);
    let ocr = get_bool(input, "ocr", false);
    let download_media = get_bool(input, "download_media", false) || transcribe_audio || ocr;
    ReadNoteOptions {
        // `level` is no longer a user-facing knob (body content is identical
        // across tiers; media is gated by include_media/download_media). It now
        // only feeds the cross-run history dedup key.
        level: "lite".to_string(),
        include_media: get_bool(input, "include_media", false),
        download_media,
        download_video_file: true,
        ocr,
        transcribe_audio,
        max_images: get_i64(input, "max_images", 12).max(1) as usize,
        max_video_frames: get_i64(input, "max_video_frames", 4).max(1) as usize,
        poster_url_fallback: get_str(input, "poster_url_fallback")
            .unwrap_or("")
            .to_string(),
        note_id_fallback: get_str(input, "note_id_fallback").unwrap_or("").to_string(),
    }
}

/// Tool args that require an activated socai pro device. Today that's cloud
/// ASR (`transcribe_audio`); future pro-only args just get added here.
const PRO_ARG_KEYS: &[&str] = &["transcribe_audio"];

/// Attached to a result when a non-pro call asked for a pro-only arg, so the
/// agent can relay why instead of the whole call failing.
const PRO_SKIP_NOTE: &str = "transcribe_audio requires socai pro; the call ran without \
     transcription. Activate with `socai pro activate <invite_code>`.";

/// Remove pro-only properties from a tool input schema so a non-pro session
/// never shows the agent args it cannot use.
fn strip_pro_schema(schema: &mut Value) {
    if let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        for key in PRO_ARG_KEYS {
            props.remove(*key);
        }
    }
}

/// Drop pro-only args from a non-pro call's input (the schema hides them, but
/// a stale conversation or hallucinated arg can still carry one). Returns
/// whether any was actually requested, so the caller can attach
/// [`PRO_SKIP_NOTE`] to its result.
fn strip_pro_input(input: &mut Value) -> bool {
    let Some(obj) = input.as_object_mut() else {
        return false;
    };
    let mut requested = false;
    for key in PRO_ARG_KEYS {
        requested |= obj
            .remove(*key)
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
    }
    requested
}

fn attach_pro_skip_note(payload: &mut Value, skipped: bool) {
    if !skipped {
        return;
    }
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("transcribe_audio_skipped".into(), json!(PRO_SKIP_NOTE));
    }
}

fn media_for(
    ctx: &ToolContext,
    llm_provider: Option<Arc<dyn LlmProvider>>,
    include_media: bool,
    transcribe_audio: bool,
) -> anyhow::Result<Option<MediaProcessor>> {
    if include_media || transcribe_audio {
        let mut media = MediaProcessor::for_run_dir(ctx.output_dir(), llm_provider)?;
        media.set_cloud_asr(transcribe_audio);
        Ok(Some(media))
    } else {
        Ok(None)
    }
}

/// Wall-clock safety net for the comment load loop, per note. The loop's real
/// stop conditions are the budget being met, the thread being exhausted, or
/// growth stalling; this only exists so a pathological page (DOM that keeps
/// "growing" a sliver each round, or a hang) can't loop forever. It is a fixed
/// generous cap on purpose — scaling it with the requested count would truncate a
/// large request that is still legitimately loading.
const COMMENT_LOAD_TIMEOUT_S: f64 = 120.0;

/// Load up to `TOP_COMMENTS_PER_NOTE` top comments from the currently open note
/// and insert them under `note["top_comments"]`. Best-effort: on failure the
/// note is left without the field. Shared by `read_note` and `extract_note`.
async fn attach_top_comments(xhs: &XhsPageRuntime<'_>, note: &mut Value) {
    let Ok(payload) = xhs
        .load_comments(TOP_COMMENTS_PER_NOTE, COMMENT_LOAD_TIMEOUT_S)
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

/// A cache hit still lands in the run's note archive: the cached entity is
/// evidence the agent may cite, so the app needs a record to resolve the
/// citation against. Cached media paths point (absolute) into the run that
/// downloaded them; the app resolves absolute paths as-is, and the asset
/// scope covers the whole runs root.
fn recorded_skip_entry(
    card: &XhsNoteCard,
    reason: &str,
    history: &XhsHistoryStore,
    ctx: &ToolContext,
    level: &str,
) -> Value {
    let entry = skipped_note_entry(card, reason, history);
    if let Some(entity) = entry.get("entity").filter(|v| v.is_object()) {
        // note_data_record expects an ok-shaped entry (the media-manifest
        // builder ignores anything that isn't `ok: true`).
        let readable = json!({ "ok": true, "entity": entity });
        if let Some((note_id, record)) =
            note_data_record(&readable, ctx.output_dir(), &ctx.run_dir, ctx.step, level)
        {
            ctx.record_note(&note_id, record);
        }
    }
    entry
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
    // Whether the caller explicitly asked for media downloads (as opposed to
    // OCR forcing `download_media` on for its own reads). Controls fetching a
    // video note's video file — an OCR-only run downloads just the poster —
    // and is the download requirement checked against cross-run history.
    download_media_requested: bool,
    ocr: bool,
    transcribe_audio: bool,
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
        return recorded_skip_entry(card, "already_processed", history, ctx, level);
    }
    if !card.note_id.is_empty()
        && history.is_satisfied_by(
            &card.note_id,
            level,
            requested_media,
            download_media_requested,
            ocr,
            transcribe_audio,
            comment_count,
        )
        // Only short-circuit when we actually have the cached entity to return;
        // a pre-upgrade entry without one is re-read so it backfills the cache
        // instead of degrading to a bare card.
        && history.has_cached_entity(&card.note_id)
    {
        ctx.mark_processed_note(&card.note_id, level, requested_media);
        return recorded_skip_entry(card, "already_analyzed", history, ctx, level);
    }

    // Per-phase wall times for the scan perf record (stats/scan.json). Seeded
    // from the read payload's `perf` (open/extract/download), then extended
    // with the comment-loading time below. Attached to the entry under the
    // transient `scan_perf` key; the browse loop strips it before the entry
    // reaches the artifact or the LLM payload.
    let mut scan_perf = serde_json::Map::new();
    let t_read = std::time::Instant::now();
    let read_result = xhs
        .read_note_with_options(
            &card.note_id,
            None,
            6.0,
            ReadNoteOptions {
                level: level.to_string(),
                include_media,
                download_media,
                download_video_file: download_media_requested,
                // Scans never transcribe inline: the caller runs cloud ASR in a
                // background task (spawn_note_transcribe) so it overlaps the
                // next note's read + download. The dedup check above still uses
                // the real `transcribe_audio` flag, so a cache hit returns the
                // already-transcribed entity.
                transcribe_audio: false,
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
    scan_perf.insert("read_ms".into(), json!(t_read.elapsed().as_millis() as u64));
    let mut entry = match read_result {
        Ok(payload) => {
            if let Some(perf) = payload.get("perf").and_then(Value::as_object) {
                for (key, value) in perf {
                    scan_perf.insert(key.clone(), value.clone());
                }
            }
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

    // Pull comments separately after the body read: the list hydrates slower
    // than the body, and reaching `comment_count` may require scrolling the
    // comment area and expanding reply threads (replies count toward the total).
    // Scans always include comments — there is no longer a level gate.
    if comment_count > 0 {
        let t_comments = std::time::Instant::now();
        let comments_result = xhs
            .load_comments(comment_count, COMMENT_LOAD_TIMEOUT_S)
            .await;
        scan_perf.insert(
            "comments_ms".into(),
            json!(t_comments.elapsed().as_millis() as u64),
        );
        if let Ok(payload) = comments_result {
            let comments = payload
                .get("comments")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(map) = entry.get_mut("entity").and_then(|v| v.as_object_mut()) {
                map.insert("top_comments".into(), Value::Array(comments));
                map.insert(
                    "top_comments_wait".into(),
                    json!({
                        "ready": payload.get("ready").and_then(Value::as_bool).unwrap_or(false),
                        "loaded_total": payload.get("loaded_total").and_then(Value::as_i64).unwrap_or(0),
                        "returned_total": payload.get("returned_total").and_then(Value::as_i64).unwrap_or(0),
                        "rounds": payload.get("rounds").and_then(Value::as_i64).unwrap_or(0),
                        "stop_reason": payload.get("stop_reason").and_then(Value::as_str).unwrap_or(""),
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
        // Archive the fully-read note (full content + resolved local media) so
        // the desktop app can render it as a rich, locally-served card without
        // re-fetching. Recorded here — before the lean trim strips images/video
        // from the agent-facing payload — so the archive keeps the full note.
        if let Some((note_id, record)) = build_note_record(&entry, ctx, level) {
            ctx.record_note(&note_id, record);
        }
    }

    if let Some(map) = entry.as_object_mut() {
        map.insert("scan_perf".into(), Value::Object(scan_perf));
    }

    entry
}

/// Max note OCR tasks in flight at once. The OCR engine serializes inference
/// behind its own mutex, so this mainly bounds decoded-image memory and blocking
/// threads while still letting OCR overlap the browse loop.
const OCR_PIPELINE_CONCURRENCY: usize = 4;

/// Spawn a background task that OCRs a freshly-read note's already-downloaded
/// media — each carousel image for an image note, the poster (cover) for a
/// video note — returning the enriched image array / video object. `None` when
/// there's nothing to OCR (no media processor, or nothing has a local path
/// yet). Runs concurrently with the browse loop so OCR of note N overlaps the
/// read + download of note N+1.
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
    let entity = entry.get("entity")?;
    let images = entity
        .get("images")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let has_local = images.iter().any(|image| {
        image
            .get("local_path")
            .and_then(Value::as_str)
            .is_some_and(|path| !path.is_empty())
    });
    // A video note carries no images; its OCR surface is the downloaded poster.
    let video = entity
        .get("video")
        .filter(|video| {
            video.get("poster_ocr").is_none()
                && video
                    .get("poster_local_path")
                    .and_then(Value::as_str)
                    .is_some_and(|path| !path.is_empty())
        })
        .cloned();
    if !has_local && video.is_none() {
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
        let mut predict = media.ocr_downloaded_images(&mut images).await;
        let mut video = video;
        if let Some(video) = video.as_mut() {
            predict += media.ocr_downloaded_video_poster(video).await;
        }
        let finished_ms = epoch.elapsed().as_millis() as u64;
        progress.ocr_completed(item_index, title);
        NoteOcrResult {
            images,
            video,
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
    /// The note's video object with `poster_ocr` attached — only for video
    /// notes whose poster was OCR'd.
    video: Option<Value>,
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

/// Pop the transient `scan_perf` off a scanned entry (so it never reaches the
/// artifact or the LLM payload) and fold it into one per-note record for
/// stats/scan.json: the read's phase times (open/extract/download/comments)
/// plus the loop-level markers measured by the browse loop itself. Cache hits
/// carry no `scan_perf` and are marked `cached` instead.
fn harvest_note_perf(
    entry: &mut Value,
    idx: usize,
    started_ms: u64,
    close_ms: u64,
    total_ms: u64,
) -> Value {
    let mut record = match entry
        .as_object_mut()
        .and_then(|map| map.remove("scan_perf"))
    {
        Some(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    let note_id = entry
        .get("entity")
        .and_then(|e| e.get("note_id"))
        .and_then(Value::as_str)
        .unwrap_or("");
    record.insert("idx".into(), json!(idx));
    record.insert("note_id".into(), json!(note_id));
    if entry.get("skipped").is_some() {
        record.insert("cached".into(), json!(true));
    }
    if let Some(ok) = entry.get("ok").and_then(Value::as_bool) {
        record.insert("ok".into(), json!(ok));
    }
    record.insert("started_ms".into(), json!(started_ms));
    record.insert("close_ms".into(), json!(close_ms));
    record.insert("total_ms".into(), json!(total_ms));
    Value::Object(record)
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
            if let Some(video) = result.video {
                entity.insert("video".into(), video);
            }
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

/// Max cloud ASR tasks in flight at once. Transcription is network-bound
/// (upload + provider poll), so this bounds concurrent load on socai-server
/// while still letting note N's transcription overlap the read + download of
/// note N+1.
const ASR_PIPELINE_CONCURRENCY: usize = 2;

/// Spawn a background task that transcribes a freshly-read video note's
/// already-downloaded video file through cloud ASR. `None` when there's
/// nothing to transcribe (not a fresh successful read, no media processor, no
/// downloaded video, or the cached entity already carries a transcript).
fn spawn_note_transcribe(
    media: &Option<MediaProcessor>,
    sem: &Arc<tokio::sync::Semaphore>,
    entry: &Value,
    epoch: std::time::Instant,
) -> Option<tokio::task::JoinHandle<NoteTranscribeResult>> {
    // Only fresh successful reads (they carry `ok`); cache hits carry `skipped`
    // and already have their transcript.
    if entry.get("ok").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let video = entry.get("entity")?.get("video")?;
    let has_local = video
        .get("local_path")
        .and_then(Value::as_str)
        .is_some_and(|path| !path.trim().is_empty());
    let has_transcript = video
        .get("transcript")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty());
    if !has_local || has_transcript {
        return None;
    }
    let media = media.clone()?;
    let sem = sem.clone();
    let mut video = video.clone();
    Some(tokio::spawn(async move {
        let _permit = sem.acquire_owned().await;
        let started_ms = epoch.elapsed().as_millis() as u64;
        media.transcribe_downloaded_video(&mut video).await;
        let finished_ms = epoch.elapsed().as_millis() as u64;
        NoteTranscribeResult {
            video,
            started_ms,
            finished_ms,
        }
    }))
}

/// A note's background transcription result plus its wall span (ms since the
/// scan epoch). The video object carries `transcript` / `transcript_error` /
/// `transcript_ms` as attached by [`MediaProcessor::transcribe_downloaded_video`].
struct NoteTranscribeResult {
    video: Value,
    started_ms: u64,
    finished_ms: u64,
}

/// Await the background transcription tasks and merge each result back into
/// its note. Only the transcript fields are copied over (the OCR pipeline may
/// have merged `poster_ocr` into the live video object in the meantime, so
/// replacing the whole object would clobber it). Re-records history (marking
/// the note transcribed) and refreshes the app note archive record so the
/// desktop card carries the transcript. Timings land in the matching
/// `note_perfs` records for the scan perf file.
#[allow(clippy::too_many_arguments)]
async fn join_note_transcribe(
    notes: &mut [Value],
    pending: Vec<(usize, tokio::task::JoinHandle<NoteTranscribeResult>)>,
    note_perfs: &mut [Value],
    history: &XhsHistoryStore,
    ctx: &ToolContext,
    level: &str,
    include_media: bool,
) {
    for (idx, handle) in pending {
        let Ok(result) = handle.await else {
            continue;
        };
        let Some(note) = notes.get_mut(idx) else {
            continue;
        };
        if let Some(video) = note
            .get_mut("entity")
            .and_then(|entity| entity.get_mut("video"))
            .and_then(Value::as_object_mut)
        {
            for key in ["transcript", "transcript_error", "transcript_ms"] {
                if let Some(value) = result.video.get(key) {
                    video.insert(key.into(), value.clone());
                }
            }
        }
        if note.get("ok").and_then(Value::as_bool) == Some(true) {
            if let Some(entity) = note.get("entity") {
                history.record(entity, level, include_media);
            }
            // Refresh the archived record (built before the transcript existed)
            // so the desktop app's note card shows the transcript.
            if let Some((note_id, record)) = build_note_record(note, ctx, level) {
                ctx.record_note(&note_id, record);
            }
        }
        if let Some(record) = note_perfs
            .iter_mut()
            .find(|perf| perf.get("idx").and_then(Value::as_u64) == Some(idx as u64))
            .and_then(Value::as_object_mut)
        {
            record.insert(
                "transcribe_audio_ms".into(),
                json!(result.finished_ms.saturating_sub(result.started_ms)),
            );
            record.insert("transcribe_started_ms".into(), json!(result.started_ms));
            record.insert("transcribe_finished_ms".into(), json!(result.finished_ms));
        }
    }
}

/// Build the archived note record from a freshly-read scan entry
/// (`{ ok, entity }`) in the desktop app's `NoteData` shape (see
/// app/src/main.ts): identity + title, full `content` (plus the short
/// `excerpt` derived from it), author (name + profile url), `posted_at`
/// (epoch ms), `ip_location`, top `comments`, numeric `stats` (a key is
/// omitted when XHS hid the count; `shares` has no source at all), and a
/// `media` array of locally-downloaded assets —
/// cover first, a video before its carousel — with run-relative paths, plus
/// run provenance (`site`, `first_seen_step`, `level`). Media entries come
/// from the on-disk-validated media manifest, so only files that actually
/// downloaded are listed. Returns `(note_id, record)`, or `None` when the
/// entity carries no usable note id.
pub fn note_data_record(
    entry: &Value,
    media_base: &Path,
    run_dir: &Path,
    step: u32,
    level: &str,
) -> Option<(String, Value)> {
    let entity = entry.get("entity").filter(|v| v.is_object())?;
    let text = |key: &str| -> String {
        entity
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let note_id = text("note_id");
    if note_id.is_empty() {
        return None;
    }

    let manifest = search_media_manifest(std::slice::from_ref(entry), media_base);
    let media = note_media_from_manifest(&manifest, run_dir);

    let mut record = Map::new();
    record.insert("note_id".into(), Value::String(note_id.clone()));
    let url = text("url");
    if !url.is_empty() {
        record.insert("url".into(), Value::String(url));
    }
    record.insert("title".into(), Value::String(text("title")));
    let content = text("content");
    if !content.is_empty() {
        record.insert("content".into(), Value::String(content.clone()));
    }
    if let Some(excerpt) = note_excerpt(&content) {
        record.insert("excerpt".into(), Value::String(excerpt));
    }
    let author_name = text("author");
    if !author_name.is_empty() {
        // Name + profile url: XHS note pages expose no avatar, and the
        // internal author_id is not the user-facing handle (小红书号) — a
        // fabricated "@<id-prefix>" would masquerade as one. `handle` stays
        // absent until the extractor captures the real thing.
        let mut author = Map::new();
        author.insert("name".into(), Value::String(author_name));
        let author_url = text("author_url");
        if !author_url.is_empty() {
            author.insert("url".into(), Value::String(author_url));
        }
        record.insert("author".into(), Value::Object(author));
    }
    if let Some(posted_at) = parse_posted_at_ms(&text("date")) {
        record.insert("posted_at".into(), Value::from(posted_at));
    }
    let ip_location = text("ip_location");
    if !ip_location.is_empty() {
        record.insert("ip_location".into(), Value::String(ip_location));
    }
    // Top comments (full objects from the DOM read) in the app's shape:
    // {author, text, likes, time, replies[]}. Absent when none were read.
    if let Some(comments) = entity.get("top_comments").and_then(Value::as_array) {
        let list: Vec<Value> = comments.iter().filter_map(app_note_comment).collect();
        if !list.is_empty() {
            record.insert("comments".into(), Value::Array(list));
        }
    }
    let mut stats = Map::new();
    for (stat, field) in [
        ("likes", "likes"),
        ("collects", "favorites"),
        ("comments", "comments_count"),
    ] {
        if let Some(count) = parse_stat_count(&text(field)) {
            stats.insert(stat.into(), Value::from(count));
        }
    }
    record.insert("stats".into(), Value::Object(stats));
    // Video audio transcript (cloud ASR), so the app's note viewer can show
    // the spoken content alongside the media.
    if let Some(transcript) = entity
        .get("video")
        .and_then(|video| video.get("transcript"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        record.insert("transcript".into(), Value::String(transcript.to_string()));
    }
    record.insert("media".into(), Value::Array(media.clone()));
    record.insert("media_dir".into(), Value::String(String::new()));
    record.insert("saved".into(), Value::Bool(!media.is_empty()));
    record.insert("site".into(), Value::String("xhs".into()));
    record.insert("first_seen_step".into(), Value::from(step));
    record.insert("level".into(), Value::String(level.to_string()));
    Some((note_id, Value::Object(record)))
}

fn build_note_record(entry: &Value, ctx: &ToolContext, level: &str) -> Option<(String, Value)> {
    note_data_record(entry, ctx.output_dir(), &ctx.run_dir, ctx.step, level)
}

/// One archived comment in the app's `NoteComment` shape, from the full DOM
/// comment object (`username`/`text`/`like_count`/`time`/`sub_comments`).
/// `None` when the comment has no text. Replies flatten one level (XHS replies
/// don't nest further).
fn app_note_comment(comment: &Value) -> Option<Value> {
    let text = comment
        .get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())?;
    let mut map = Map::new();
    map.insert("text".into(), Value::String(text.to_string()));
    if let Some(author) = comment
        .get("username")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|a| !a.is_empty())
    {
        map.insert("author".into(), Value::String(author.to_string()));
    }
    if let Some(likes) = comment
        .get("like_count")
        .and_then(Value::as_i64)
        .filter(|&n| n > 0)
    {
        map.insert("likes".into(), Value::from(likes));
    }
    if let Some(time) = comment
        .get("time")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        map.insert("time".into(), Value::String(time.to_string()));
    }
    if comment
        .get("is_author_reply")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        map.insert("is_author".into(), Value::Bool(true));
    }
    let replies: Vec<Value> = comment
        .get("sub_comments")
        .and_then(Value::as_array)
        .map(|subs| subs.iter().filter_map(app_note_comment).collect())
        .unwrap_or_default();
    if !replies.is_empty() {
        map.insert("replies".into(), Value::Array(replies));
    }
    Some(Value::Object(map))
}

/// Map validated manifest rows to the app's `NoteMedia` entries: only assets
/// that downloaded, the video first as the cover (it renders even when only
/// its poster came down — blob-URL videos can't be fetched, and the app falls
/// back to poster + play glyph), then carousel images in index order.
fn note_media_from_manifest(manifest: &Value, run_dir: &Path) -> Vec<Value> {
    let rows: Vec<&Map<String, Value>> = manifest
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default();
    fn role(row: &Map<String, Value>) -> &str {
        row.get("role").and_then(Value::as_str).unwrap_or("")
    }
    let downloaded_path = |row: &Map<String, Value>| -> Option<String> {
        if row.get("download_status").and_then(Value::as_str) != Some("downloaded") {
            return None;
        }
        row.get("local_path")
            .and_then(Value::as_str)
            .filter(|path| !path.is_empty())
            .map(|path| run_relative_path(path, run_dir))
    };

    let mut media = Vec::new();
    let video_row = rows.iter().find(|row| role(row) == "video");
    let video_src = video_row.and_then(|row| downloaded_path(row));
    let poster = rows
        .iter()
        .find(|row| role(row) == "video_poster")
        .and_then(|row| downloaded_path(row));
    if video_src.is_some() || poster.is_some() {
        let mut item = Map::new();
        item.insert("kind".into(), Value::String("video".into()));
        item.insert("ratio".into(), Value::String("9:16".into()));
        if let Some(src) = video_src {
            item.insert("src".into(), Value::String(src));
        }
        if let Some(poster) = poster {
            item.insert("poster".into(), Value::String(poster));
        }
        if let Some(duration_s) = video_row.and_then(|row| {
            row.get("duration_s")
                .and_then(Value::as_f64)
                .filter(|s| *s > 0.0)
        }) {
            item.insert(
                "dur".into(),
                Value::String(format_media_duration(duration_s)),
            );
        }
        media.push(Value::Object(item));
    }

    let mut images: Vec<(i64, String)> = rows
        .iter()
        .filter(|row| role(row) == "image")
        .filter_map(|row| {
            downloaded_path(row).map(|path| {
                let index = row.get("index").and_then(Value::as_i64).unwrap_or(i64::MAX);
                (index, path)
            })
        })
        .collect();
    images.sort_by_key(|(index, _)| *index);
    for (_, src) in images {
        media.push(json!({ "kind": "image", "ratio": "3:4", "src": src }));
    }
    media
}

/// First ~90 code points of the note body, whitespace collapsed. Truncation
/// counts chars, not bytes — a byte slice could split a multi-byte char and
/// panic (the JS fixture generator had the UTF-16 twin of this bug) — and the
/// tail is trimmed so the cut can't end mid grapheme cluster (a ZWJ-joined
/// emoji family, a skin-tone modifier, or half a regional-indicator flag).
fn note_excerpt(content: &str) -> Option<String> {
    let collapsed = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    let mut chars: Vec<char> = collapsed.chars().collect();
    if chars.len() <= 90 {
        return Some(collapsed);
    }
    chars.truncate(88);
    let fragile = |c: char| {
        c == '\u{200D}' // zero-width joiner
            || ('\u{FE00}'..='\u{FE0F}').contains(&c) // variation selectors
            || ('\u{1F3FB}'..='\u{1F3FF}').contains(&c) // skin-tone modifiers
    };
    loop {
        match chars.last() {
            Some(&c) if fragile(c) => {
                chars.pop();
            }
            // A char right after a ZWJ is a fragment of a joined sequence —
            // drop it together with its joiner and re-check the new tail.
            Some(_) if chars.len() >= 2 && chars[chars.len() - 2] == '\u{200D}' => {
                chars.pop();
                chars.pop();
            }
            _ => break,
        }
    }
    let regional = |c: &&char| ('\u{1F1E6}'..='\u{1F1FF}').contains(*c);
    if chars.iter().rev().take_while(regional).count() % 2 == 1 {
        chars.pop();
    }
    let mut excerpt: String = chars.into_iter().collect();
    excerpt.push('…');
    Some(excerpt)
}

/// Media paths are emitted relative to the run dir so the archive survives the
/// run being moved or synced to another machine; paths outside the run dir
/// (custom output roots) stay absolute, which the app also resolves.
fn run_relative_path(path: &str, run_dir: &Path) -> String {
    Path::new(path)
        .strip_prefix(run_dir)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string())
}

/// Seconds → the app's display duration, "M:SS".
fn format_media_duration(seconds: f64) -> String {
    let total = seconds.round().max(0.0) as i64;
    format!("{}:{:02}", total / 60, total % 60)
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
    // The filter summary (which filters ended up active) is bookkeeping, not
    // analysis input — it stays in the artifact only.
    obj.remove("filters");
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

/// Collapse an `apply_search_filters` readback to its meaning: `changed` plus
/// each filter group's active value (and the failure signal when applying
/// failed). The raw readback — every panel option's label with its click
/// coordinates — is a UI-automation diagnostic; even the run artifact only
/// keeps this summary. `None` when no filters were requested (empty readback),
/// so the caller can drop the key entirely.
fn compact_filter_result(filters: &Value) -> Option<Value> {
    let obj = filters.as_object()?;
    if obj.is_empty() {
        return None;
    }
    let mut lean = serde_json::Map::new();
    if obj.get("ok").and_then(Value::as_bool) == Some(false) {
        lean.insert("ok".into(), json!(false));
        if let Some(error) = obj.get("error") {
            lean.insert("error".into(), error.clone());
        }
    }
    if let Some(changed) = obj.get("changed") {
        lean.insert("changed".into(), changed.clone());
    }
    let mut active = serde_json::Map::new();
    if let Some(groups) = obj
        .get("filters")
        .and_then(|panel| panel.get("groups"))
        .and_then(Value::as_array)
    {
        for group in groups {
            let (Some(key), Some(value)) = (
                group.get("key").and_then(Value::as_str),
                group.get("active").and_then(Value::as_str),
            ) else {
                continue;
            };
            active.insert(key.to_string(), json!(value));
        }
    }
    if !active.is_empty() {
        lean.insert("active".into(), Value::Object(active));
    }
    Some(Value::Object(lean))
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
    // Video audio transcript from cloud ASR. The full video object stays in the
    // artifact; this keeps the usable text in the compact result.
    "audio_transcript",
];

/// Collapse one full comment object to its lean form: `null` when it has no text,
/// a plain text string when it has no replies, or `{text, replies:[…]}` when it
/// does. Reply objects are flattened to their text the same way (one level deep —
/// XHS replies don't nest further).
fn lean_comment(comment: &Value) -> Option<Value> {
    let text = comment.get("text").and_then(Value::as_str).unwrap_or("");
    if text.is_empty() {
        return None;
    }
    let replies: Vec<Value> = comment
        .get("sub_comments")
        .and_then(Value::as_array)
        .map(|subs| {
            subs.iter()
                .filter_map(|s| s.get("text").and_then(Value::as_str))
                .filter(|s| !s.is_empty())
                .map(|s| Value::String(s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    if replies.is_empty() {
        Some(Value::String(text.to_string()))
    } else {
        Some(json!({ "text": text, "replies": replies }))
    }
}

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
    attach_note_audio_transcript(entity);
    let Some(entity) = entity.as_object_mut() else {
        return;
    };
    // Collapse comment objects before the whitelist runs (which keeps
    // `top_comments`): a comment with no replies becomes its plain text; one with
    // replies becomes `{text, replies:[reply text, …]}` so the lean view keeps
    // the thread structure. The full objects (usernames, likes, times) stay in
    // the run artifact.
    if let Some(comments) = entity.get_mut("top_comments").and_then(Value::as_array_mut) {
        let lean: Vec<Value> = comments.iter().filter_map(lean_comment).collect();
        entity.insert("top_comments".into(), Value::Array(lean));
    }
    entity.retain(|key, _| LEAN_NOTE_FIELDS.contains(&key.as_str()));
}

fn attach_note_audio_transcript(entity: &mut Value) {
    let Some(transcript) = entity
        .get("video")
        .and_then(|video| video.get("transcript"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| truncate(text, 6000))
    else {
        return;
    };
    if let Some(map) = entity.as_object_mut() {
        map.insert("audio_transcript".into(), Value::String(transcript));
    }
}

/// Surface OCR text on the entity as `ocr_text`: an array of one string per
/// OCR'd image, cover-first. For an image note that's the carousel in image
/// order (image 0 is the cover); for a video note it's the poster's OCR (the
/// poster is the cover, and the only OCR surface). Each entry is that image's
/// recognized text ("" when an image has none). No-op when nothing produced
/// any text. Called only during lean trimming (see [`lean_scan_note`]) so the
/// artifact keeps only the per-image / poster OCR; this is the lean,
/// index-aligned view that survives images and video being dropped from the
/// returned notes.
fn attach_note_ocr_summary(entity: &mut Value) {
    let mut texts: Vec<Value> = Vec::new();
    if let Some(poster) = entity
        .get("video")
        .and_then(|video| video.get("poster_ocr"))
        .and_then(Value::as_str)
    {
        texts.push(Value::String(truncate(poster, 1200)));
    }
    if let Some(images) = entity.get("images").and_then(Value::as_array) {
        texts.extend(images.iter().map(|image| {
            let text = image
                .get("ocr_text")
                .and_then(Value::as_str)
                .map(|text| truncate(text, 1200))
                .unwrap_or_default();
            Value::String(text)
        }));
    }
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

/// Per-note phase keys accumulated into `*_total_ms` summary fields by
/// [`write_scan_perf`]. `total_ms` is summarized as `notes_total_ms`.
const SCAN_PHASE_KEYS: &[&str] = &[
    "open_ms",
    "extract_ms",
    "enrich_ms",
    "download_ms",
    "ocr_inline_ms",
    "transcribe_audio_ms",
    "comments_ms",
    "close_ms",
];

/// Scan performance/debug record, written to a separate file
/// (`stats/scan.json` in the tool-call dir) rather than the LLM-facing JSON
/// artifact.
///
/// Each note carries the phases of its serial browse-loop cost —
/// `started_ms` (relative to scan start), `open_ms` + `open_strategy` (whether
/// the first card click opened the overlay or a retry was needed),
/// `extract_ms`, `download_ms`, `comments_ms`, `close_ms`, `read_ms`
/// (open+extract+download combined, as measured around the whole read), and
/// `total_ms` — plus, when video transcript ran for it,
/// `transcribe_audio_ms`, and when OCR ran for it, `predict_ms` and
/// `ocr_started_ms`/`ocr_finished_ms`/`ocr_wall_ms` on the same timeline.
/// Cache hits are marked `cached` and carry only the loop-level markers.
///
/// `summary` is the key to reading the pipeline. All `*_ms` are wall time:
///   - per-phase `*_total_ms` — summed serial cost of that phase across notes;
///     the biggest one is the scan's bottleneck.
///   - `open_retries` — notes whose overlay needed a retry click (each burns
///     the per-attempt open wait before the retry).
///   - `ocr_predict_total_ms` — summed per-note batch inference (total OCR CPU
///     cost).
///   - `ocr_wall_ms` — **measured** first-OCR-start → last-OCR-end. OCR tasks
///     share one engine (serialized), so this ≈ `ocr_predict_total_ms`; the
///     pipeline overlaps OCR with the *browse loop*, not with other OCR.
///   - `browse_loop_ms` — the open→read→download→close loop OCR runs behind.
///   - `ocr_overhang_ms` — OCR still running after the browse loop ended (the
///     part the pipeline could NOT hide).
///   - `scan_total_ms` — whole scan wall.
fn write_scan_perf(
    ctx: &ToolContext,
    notes: &[Value],
    browse_loop_ms: u64,
    ocr_timings: &[NoteOcrTiming],
    note_perfs: &[Value],
) {
    if note_perfs.is_empty() && ocr_timings.is_empty() {
        return;
    }
    let ocr_by_idx: std::collections::HashMap<usize, &NoteOcrTiming> =
        ocr_timings.iter().map(|t| (t.idx, t)).collect();

    let mut note_reports: Vec<Value> = Vec::new();
    let mut total_images: u64 = 0;
    let mut predict_total_ms: u64 = 0;
    let mut cached_count: u64 = 0;
    let mut open_retries: u64 = 0;
    let mut notes_total_ms: u64 = 0;
    let mut phase_totals: std::collections::BTreeMap<&str, u64> = std::collections::BTreeMap::new();
    for perf in note_perfs {
        let Some(record) = perf.as_object() else {
            continue;
        };
        let mut record = record.clone();
        let idx = record.get("idx").and_then(Value::as_u64).unwrap_or(0) as usize;

        if record.get("cached").and_then(Value::as_bool) == Some(true) {
            cached_count += 1;
        }
        if record
            .get("open_strategy")
            .and_then(Value::as_str)
            .is_some_and(|s| s.starts_with("retry"))
        {
            open_retries += 1;
        }
        notes_total_ms += record.get("total_ms").and_then(Value::as_u64).unwrap_or(0);
        for key in SCAN_PHASE_KEYS {
            if let Some(ms) = record.get(*key).and_then(Value::as_u64) {
                *phase_totals.entry(key).or_insert(0) += ms;
            }
        }

        let images = notes
            .get(idx)
            .and_then(|note| note.get("entity"))
            .and_then(|e| e.get("images"))
            .and_then(Value::as_array)
            .map(|a| a.len())
            .unwrap_or(0) as u64;
        total_images += images;
        record.insert("images".into(), json!(images));
        if let Some(t) = ocr_by_idx.get(&idx) {
            predict_total_ms += t.predict_ms;
            record.insert("predict_ms".into(), json!(t.predict_ms));
            record.insert("ocr_started_ms".into(), json!(t.started_ms));
            record.insert("ocr_finished_ms".into(), json!(t.finished_ms));
            record.insert(
                "ocr_wall_ms".into(),
                json!(t.finished_ms.saturating_sub(t.started_ms)),
            );
        }
        note_reports.push(Value::Object(record));
    }

    // Overall OCR wall = first start → last finish, measured across all notes.
    let ocr_wall_ms = match (
        ocr_timings.iter().map(|t| t.started_ms).min(),
        ocr_timings.iter().map(|t| t.finished_ms).max(),
    ) {
        (Some(start), Some(end)) => end.saturating_sub(start),
        _ => 0,
    };
    let last_finish = ocr_timings.iter().map(|t| t.finished_ms).max().unwrap_or(0);
    // Background transcription may outlive both the browse loop and OCR; its
    // per-note spans were merged into `note_perfs` by join_note_transcribe.
    let transcribe_last_finish = note_perfs
        .iter()
        .filter_map(|perf| perf.get("transcribe_finished_ms").and_then(Value::as_u64))
        .max()
        .unwrap_or(0);
    let scan_total_ms = browse_loop_ms.max(last_finish).max(transcribe_last_finish);
    let ocr_overhang_ms = last_finish.saturating_sub(browse_loop_ms);

    let mut summary = serde_json::Map::new();
    summary.insert("notes".into(), json!(note_perfs.len()));
    summary.insert("notes_cached".into(), json!(cached_count));
    summary.insert("open_retries".into(), json!(open_retries));
    summary.insert("images".into(), json!(total_images));
    summary.insert("notes_total_ms".into(), json!(notes_total_ms));
    for (key, total) in &phase_totals {
        let name = format!("{}_total_ms", key.trim_end_matches("_ms"));
        summary.insert(name, json!(total));
    }
    summary.insert("browse_loop_ms".into(), json!(browse_loop_ms));
    summary.insert("scan_total_ms".into(), json!(scan_total_ms));
    if !ocr_timings.is_empty() {
        summary.insert("ocr_predict_total_ms".into(), json!(predict_total_ms));
        summary.insert("ocr_wall_ms".into(), json!(ocr_wall_ms));
        summary.insert("ocr_overhang_ms".into(), json!(ocr_overhang_ms));
    }
    if transcribe_last_finish > 0 {
        summary.insert(
            "transcribe_overhang_ms".into(),
            json!(transcribe_last_finish.saturating_sub(browse_loop_ms)),
        );
    }

    let mut perf = serde_json::Map::new();
    if !ocr_timings.is_empty() {
        perf.insert("ocr".into(), ocr_diagnostics());
    }
    perf.insert("summary".into(), Value::Object(summary));
    perf.insert("notes".into(), Value::Array(note_reports));
    write_run_perf_file(ctx, "scan.json", &Value::Object(perf));
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
    write_run_perf_file(ctx, "ocr.json", &perf);
}

/// Write a tool-specific perf record under the current tool call's `stats/`.
fn write_run_perf_file(ctx: &ToolContext, name: &str, perf: &Value) {
    let stats_dir = ctx.output_dir().join("stats");
    if std::fs::create_dir_all(&stats_dir).is_err() {
        return;
    }
    if let Ok(rendered) = serde_json::to_string_pretty(perf) {
        let _ = std::fs::write(stats_dir.join(name), rendered);
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
    "top_comments (full objects: text, author, likes, time, sub_comments[])",
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
    /// socai pro activated on this device; when false, pro-only args
    /// ([`PRO_ARG_KEYS`]) are hidden from the schema and skipped at runtime.
    pro: bool,
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
        let mut schema = json!({
            "type": "object",
            "properties": {
                "note_id": { "type": "string" },
                "index": { "type": "integer", "minimum": 0 },
                "wait_seconds": { "type": "number", "default": 6.0 },
                "include_media": { "type": "boolean", "default": false },
                "download_media": { "type": "boolean", "default": false },
                "transcribe_audio": { "type": "boolean", "default": false },
                "max_images": { "type": "integer", "default": 12, "minimum": 1 },
                "max_video_frames": { "type": "integer", "default": 4, "minimum": 1 }
            }
        });
        if !self.pro {
            strip_pro_schema(&mut schema);
        }
        schema
    }

    async fn call(&self, mut input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let pro_skipped = !self.pro && strip_pro_input(&mut input);
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
                options.transcribe_audio,
                TOP_COMMENTS_PER_NOTE,
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
                options.transcribe_audio,
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
            let mut comments_ms = None;
            if let Some(entity) = value.get_mut("entity") {
                let t_comments = std::time::Instant::now();
                attach_top_comments(&xhs, entity).await;
                comments_ms = Some(t_comments.elapsed().as_millis() as u64);
            }
            if let (Some(ms), Some(perf)) = (
                comments_ms,
                value.get_mut("perf").and_then(Value::as_object_mut),
            ) {
                perf.insert("comments_ms".into(), json!(ms));
            }
            if let Some(entity) = value.get("entity") {
                self.history
                    .record(entity, &options.level, options.include_media);
            }
        }
        // Phase timing is a debug record, not analysis input: move it out of
        // the LLM-facing payload into stats/read.json.
        if let Some(perf) = value.as_object_mut().and_then(|map| map.remove("perf")) {
            write_run_perf_file(ctx, "read.json", &json!({ "read": perf }));
        }
        attach_pro_skip_note(&mut value, pro_skipped);
        Ok(json_result(&value))
    }
}

/// extract_note(wait_seconds?) -> XhsNote
pub struct ExtractNoteTool {
    page: Arc<PageSession>,
    llm_provider: Option<Arc<dyn LlmProvider>>,
    history: Arc<XhsHistoryStore>,
    /// See [`ReadNoteTool::pro`].
    pro: bool,
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
        let mut schema = json!({
            "type": "object",
            "properties": {
                "wait_seconds": { "type": "number", "default": 8.0 },
                "include_media": { "type": "boolean", "default": false },
                "download_media": { "type": "boolean", "default": false },
                "transcribe_audio": { "type": "boolean", "default": false },
                "max_images": { "type": "integer", "default": 12, "minimum": 1 },
                "max_video_frames": { "type": "integer", "default": 4, "minimum": 1 }
            }
        });
        if !self.pro {
            strip_pro_schema(&mut schema);
        }
        schema
    }

    async fn call(&self, mut input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let pro_skipped = !self.pro && strip_pro_input(&mut input);
        let wait_seconds = get_f64(&input, "wait_seconds", 8.0);
        let options = read_note_options(&input);
        let xhs = XhsPageRuntime::new_with_media(
            &self.page,
            media_for(
                ctx,
                self.llm_provider.clone(),
                options.include_media || options.download_media,
                options.transcribe_audio,
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
        attach_pro_skip_note(&mut value, pro_skipped);
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

/// wait_for_login — the login-recovery tool. When a macro returns
/// `reason: login_required`, the agent surfaces the login prompt to the user and
/// calls this instead of retrying; it brings up the XHS login page and blocks
/// (polling) until the user finishes scanning, then returns so the agent can
/// resume the original task.
pub struct WaitForLoginTool {
    page: Arc<PageSession>,
}

/// How long `wait_for_login` polls before giving up so the agent can re-prompt.
const WAIT_FOR_LOGIN_DEFAULT_SECS: i64 = 180;
const WAIT_FOR_LOGIN_MAX_SECS: i64 = 600;

#[async_trait]
impl Tool for WaitForLoginTool {
    fn name(&self) -> &str {
        "wait_for_login"
    }

    fn description(&self) -> &str {
        "Recover from a login wall: after a tool returns `reason:login_required`, \
         call this (don't retry) to open the login page and block until the user \
         signs in. Returns `logged_in:true` when done, else `logged_in:false` on \
         timeout. See the login protocol in the instructions."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "timeout_seconds": {
                    "type": "integer",
                    "description": "Seconds to wait before returning (default 180, max 600)."
                }
            }
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let xhs = XhsPageRuntime::new(&self.page);
        // Already logged in? Return right away — nothing to wait for.
        if xhs.is_logged_in().await.unwrap_or(false) {
            return Ok(json_result(&json!({
                "logged_in": true,
                "message": "Already logged in. Re-run the original tool.",
            })));
        }
        // Bring the XHS login wall on screen so the user has a QR to scan.
        xhs.ensure_xhs(true).await?;

        let timeout = get_i64(&input, "timeout_seconds", WAIT_FOR_LOGIN_DEFAULT_SECS)
            .clamp(10, WAIT_FOR_LOGIN_MAX_SECS);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout as u64);
        loop {
            if xhs.is_logged_in().await.unwrap_or(false) {
                return Ok(json_result(&json!({
                    "logged_in": true,
                    "message": "Login detected. Re-run the original tool to continue.",
                })));
            }
            if std::time::Instant::now() >= deadline {
                return Ok(json_result(&json!({
                    "logged_in": false,
                    "timed_out": true,
                    "message": format!(
                        "Still not logged in after {timeout}s. Ask the user to scan \
                         the QR / sign in on xiaohongshu.com in the browser, then \
                         call wait_for_login again."
                    ),
                })));
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
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
    /// See [`ReadNoteTool::pro`].
    pro: bool,
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
            object.remove("transcribe_audio");
        }
    } else if always_download_media
        || get_bool(&effective, "download_media", false)
        || get_bool(&effective, "transcribe_audio", false)
    {
        // Explicit downloads only — OCR alone downloads just what it reads
        // (images / video poster), so it must not force `download_media` here.
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
         `ocr=true` to OCR each opened note locally (PP-OCRv6 small) — every \
         carousel image, or a video note's cover — and attach a per-note \
         ocr_text; it downloads what it reads, but video files still require \
         download_media. Pass \
         `preview=true` for a fast cards-only pass that returns result cards \
         (titles/likes/covers) without opening any note. Defaults to 10 notes; \
         pass a larger `num_notes` to scan more (each note is opened, so latency \
         scales with it). Prefer this for XHS topic/keyword research. Do not \
         repeat the same search unless the previous one was clearly insufficient."
    }

    fn input_schema(&self) -> Value {
        let mut schema = json!({
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
                "num_comments": {
                    "type": "integer",
                    "description": "Comments to load per note. Scrolls the comment area and expands reply threads to reach this many; replies count toward the total. Higher values add latency per note. Ignored in preview mode.",
                    "default": TOP_COMMENTS_PER_NOTE,
                    "minimum": 0
                },
                "download_media": {
                    "type": "boolean",
                    "description": "Download note images/videos into the command run_dir, include local_path fields in returned notes, and write a stable media_manifest.json surfaced by media_manifest_path. Ignored in preview mode.",
                    "default": false
                },
                "ocr": {
                    "type": "boolean",
                    "description": "Run local OCR (PP-OCRv6 small). Full scan: OCR each opened note — every carousel image, or a video note's cover — downloading what it reads (video files still require download_media); each returned note gets ocr_text as an array of per-image strings (image order, cover first). Preview: OCR each card's cover image and attach its ocr_text. Per-image ocr_text/ocr_ms and OCR diagnostics are kept in the artifact.",
                    "default": false
                },
                "transcribe_audio": {
                    "type": "boolean",
                    "description": "For opened video notes, download the video file and transcribe audio through socai pro. Ignored in preview mode.",
                    "default": false
                },
                "preview": {
                    "type": "boolean",
                    "description": "Fast cards-only mode: return result cards (titles/likes/covers) without opening notes or reading bodies/comments. Off by default (full scan).",
                    "default": false
                }
            },
            "required": ["query"]
        });
        if !self.pro {
            strip_pro_schema(&mut schema);
        }
        schema
    }

    fn effective_input(&self, input: &Value) -> Value {
        effective_macro_input(
            input,
            Some(DEFAULT_NUM_NOTES),
            self.always_download_media,
            self.always_ocr,
        )
    }

    async fn call(&self, mut input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let pro_skipped = !self.pro && strip_pro_input(&mut input);
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
            // Collapse the filter readback before the artifact write so even
            // the artifact keeps only the compact summary, never the raw panel
            // dump with click coordinates.
            if let Some(obj) = value.as_object_mut() {
                match obj.get("filters").and_then(compact_filter_result) {
                    Some(summary) => {
                        obj.insert("filters".into(), summary);
                    }
                    None => {
                        obj.remove("filters");
                    }
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
                    // page-state echo), `reason` is empty on success, `ok` is
                    // self-evident, and the filter summary is bookkeeping that
                    // lives in the artifact — drop them all to match the
                    // full-scan output.
                    obj.remove("submit");
                    obj.remove("reason");
                    obj.remove("ok");
                    obj.remove("filters");
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
        let ocr = self.always_ocr || get_bool(&input, "ocr", false);
        let transcribe_audio = get_bool(&input, "transcribe_audio", false);
        // Explicit request: download everything, video files included.
        let download_media_requested = self.always_download_media
            || get_bool(&input, "download_media", false)
            || transcribe_audio;
        // OCR still implies downloading what it reads (carousel images, video
        // posters) — but only an explicit request fetches video files.
        let download_media = ocr || download_media_requested;
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
            transcribe_audio,
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
        let comment_count = get_i64(&input, "num_comments", TOP_COMMENTS_PER_NOTE).max(0);
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
        // OCR and cloud ASR run in the background so they overlap the next
        // note's read + download; tasks are joined after the browse loop.
        let ocr_sem = Arc::new(tokio::sync::Semaphore::new(OCR_PIPELINE_CONCURRENCY));
        let mut pending_ocr: Vec<(usize, tokio::task::JoinHandle<NoteOcrResult>)> = Vec::new();
        let asr_sem = Arc::new(tokio::sync::Semaphore::new(ASR_PIPELINE_CONCURRENCY));
        let mut pending_transcribe: Vec<(usize, tokio::task::JoinHandle<NoteTranscribeResult>)> =
            Vec::new();
        let mut note_perfs: Vec<Value> = Vec::new();
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
            let note_started_ms = browse_t0.elapsed().as_millis() as u64;

            let entry = scan_card_note(
                &xhs,
                &self.history,
                ctx,
                &card,
                level,
                comment_count,
                include_media,
                download_media,
                download_media_requested,
                ocr,
                transcribe_audio,
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
            if transcribe_audio {
                if let Some(handle) =
                    spawn_note_transcribe(&media, &asr_sem, &notes[idx], browse_t0)
                {
                    pending_transcribe.push((idx, handle));
                }
            }
            let t_close = std::time::Instant::now();
            let _ = xhs.close_note(0.6).await;
            let close_ms = t_close.elapsed().as_millis() as u64;
            scan_progress.reading_completed(item_index, title);
            let total_ms = (browse_t0.elapsed().as_millis() as u64).saturating_sub(note_started_ms);
            note_perfs.push(harvest_note_perf(
                &mut notes[idx],
                idx,
                note_started_ms,
                close_ms,
                total_ms,
            ));
        }

        scan_progress.finish_reading(notes.len());
        let browse_ms = browse_t0.elapsed().as_millis() as u64;
        // Join background OCR (epoch = browse start, so the timings line up with
        // browse_ms) and merge results back into the notes in place.
        let ocr_timings =
            join_note_ocr(&mut notes, pending_ocr, &self.history, level, include_media).await;
        // Join background ASR after OCR: both pipelines write into the video
        // object, and the transcribe join copies only its own fields.
        join_note_transcribe(
            &mut notes,
            pending_transcribe,
            &mut note_perfs,
            &self.history,
            ctx,
            level,
            include_media,
        )
        .await;
        // Write the scan perf record (per-note phase timing + OCR pipeline) to a
        // separate debug file; the JSON artifact stays LLM-facing.
        write_scan_perf(ctx, &notes, browse_ms, &ocr_timings, &note_perfs);
        if ocr {
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
            "search": search,
            "notes": notes,
            "sampling": {
                "num_notes": num_notes,
                "selected": selected.len(),
                "comments_per_note": comment_count,
                "include_media": include_media,
                "download_media": download_media,
                "ocr": ocr,
                "transcribe_audio": transcribe_audio,
            },
            "timing": {
                "media": media_timing,
            }
        });
        // Even the artifact keeps only the compact filter summary (changed +
        // active values); the raw panel readback with click coordinates is
        // never persisted.
        if let Some(summary) = compact_filter_result(&filter_result) {
            if let Some(map) = payload.as_object_mut() {
                map.insert("filters".into(), summary);
            }
        }
        // Scan perf (per-note phases + OCR model / EP / machine timing) is
        // written to `stats/scan.json` (see write_scan_perf above), not the
        // JSON artifact.
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
        attach_pro_skip_note(&mut payload, pro_skipped);
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
    /// See [`ReadNoteTool::pro`].
    pro: bool,
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
         card count). Pass `ocr=true` to OCR each opened note locally \
         (PP-OCRv6 small) — every carousel image, or a video note's cover — and \
         attach a per-note ocr_text; it downloads what it reads, but video \
         files still require download_media. \
         Pass `preview=true` for a fast cards-only pass that \
         returns the note cards (titles/likes/covers) without opening any note. \
         Use this for creator research — it's like `search` but scoped to one \
         author instead of a query."
    }

    fn input_schema(&self) -> Value {
        let mut schema = json!({
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
                "num_comments": {
                    "type": "integer",
                    "description": "Comments to load per note. Scrolls the comment area and expands reply threads to reach this many; replies count toward the total. Higher values add latency per note. Ignored in preview mode.",
                    "default": TOP_COMMENTS_PER_NOTE,
                    "minimum": 0
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
                    "description": "Run local OCR (PP-OCRv6 small) on each opened note — every carousel image, or a video note's cover — attaching per-image ocr_text in the artifact and a joined per-note ocr_text in the returned notes. Downloads what it reads on its own; video files still require download_media. In preview mode, OCRs each note card's cover instead.",
                    "default": false
                },
                "transcribe_audio": {
                    "type": "boolean",
                    "description": "For opened video notes, download the video file and transcribe audio through socai pro. Ignored in preview mode.",
                    "default": false
                }
            },
            "required": ["author_id"]
        });
        if !self.pro {
            strip_pro_schema(&mut schema);
        }
        schema
    }

    fn effective_input(&self, input: &Value) -> Value {
        effective_macro_input(input, None, self.always_download_media, self.always_ocr)
    }

    async fn call(&self, mut input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let pro_skipped = !self.pro && strip_pro_input(&mut input);
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
        let transcribe_audio = !preview
            && get_bool(&input, "transcribe_audio", false);
        // Explicit request: download everything, video files included. OCR
        // still implies downloading what it reads (carousel images, video
        // posters) — but only an explicit request fetches video files.
        let download_media_requested = !preview
            && (self.always_download_media
                || get_bool(&input, "download_media", false)
                || transcribe_audio);
        let download_media = !preview && (ocr || download_media_requested);
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
        let comment_count = get_i64(&input, "num_comments", TOP_COMMENTS_PER_NOTE).max(0);

        // Media processor only needed when downloading note media (no vision,
        // so no LLM provider required).
        let media = media_for(ctx, None, download_media, transcribe_audio)?;
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
            // OCR and cloud ASR run in the background so they overlap the next
            // note's read + download; tasks are joined after the loop.
            let ocr_sem = Arc::new(tokio::sync::Semaphore::new(OCR_PIPELINE_CONCURRENCY));
            let mut pending_ocr: Vec<(usize, tokio::task::JoinHandle<NoteOcrResult>)> = Vec::new();
            let asr_sem = Arc::new(tokio::sync::Semaphore::new(ASR_PIPELINE_CONCURRENCY));
            let mut pending_transcribe: Vec<(
                usize,
                tokio::task::JoinHandle<NoteTranscribeResult>,
            )> = Vec::new();
            let mut note_perfs: Vec<Value> = Vec::new();
            let scan_progress = ScanProgress::new(ctx, cards.len());
            let browse_t0 = std::time::Instant::now();
            for card in &cards {
                if !card.note_id.is_empty() {
                    ctx.add_search_note_ids(std::slice::from_ref(&card.note_id));
                }
                let item_index = notes.len() + 1;
                let title = progress_title(card);
                scan_progress.reading_started(item_index, title.clone());
                let note_started_ms = browse_t0.elapsed().as_millis() as u64;
                let entry = scan_card_note(
                    &xhs,
                    &self.history,
                    ctx,
                    card,
                    "deep",
                    comment_count,
                    false,
                    download_media,
                    download_media_requested,
                    ocr,
                    transcribe_audio,
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
                if transcribe_audio {
                    if let Some(handle) =
                        spawn_note_transcribe(&media, &asr_sem, &notes[idx], browse_t0)
                    {
                        pending_transcribe.push((idx, handle));
                    }
                }
                let t_close = std::time::Instant::now();
                let _ = xhs.close_note(0.6).await;
                let close_ms = t_close.elapsed().as_millis() as u64;
                scan_progress.reading_completed(item_index, title);
                let total_ms =
                    (browse_t0.elapsed().as_millis() as u64).saturating_sub(note_started_ms);
                note_perfs.push(harvest_note_perf(
                    &mut notes[idx],
                    idx,
                    note_started_ms,
                    close_ms,
                    total_ms,
                ));
            }
            scan_progress.finish_reading(notes.len());
            let browse_ms = browse_t0.elapsed().as_millis() as u64;
            let ocr_timings =
                join_note_ocr(&mut notes, pending_ocr, &self.history, "deep", false).await;
            join_note_transcribe(
                &mut notes,
                pending_transcribe,
                &mut note_perfs,
                &self.history,
                ctx,
                "deep",
                false,
            )
            .await;
            write_scan_perf(ctx, &notes, browse_ms, &ocr_timings, &note_perfs);
            if ocr {
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
                "comments_per_note": if preview { 0 } else { comment_count },
                "download_media": download_media,
                "ocr": ocr,
                "transcribe_audio": transcribe_audio,
            },
            "timing": { "media": media_timing },
        });

        // Scan perf is written to `stats/scan.json` (see above), not the artifact.

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
        attach_pro_skip_note(&mut payload, pro_skipped);
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

// AGENTS.md: do NOT add new Rust tests unless the user explicitly asks. Update
// the existing ones when an API they cover changes; don't grow this module.
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

    #[test]
    fn strip_pro_schema_hides_pro_args() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "transcribe_audio": { "type": "boolean" }
            }
        });
        strip_pro_schema(&mut schema);
        assert!(schema["properties"].get("transcribe_audio").is_none());
        assert!(schema["properties"].get("query").is_some());
    }

    #[test]
    fn strip_pro_input_drops_args_and_reports_requests() {
        let mut input = json!({ "query": "咖啡", "transcribe_audio": true });
        assert!(strip_pro_input(&mut input));
        assert!(input.get("transcribe_audio").is_none());
        assert_eq!(input["query"], json!("咖啡"));

        // Not requested (absent or false) → no skip note owed.
        let mut plain = json!({ "query": "咖啡" });
        assert!(!strip_pro_input(&mut plain));
        let mut off = json!({ "query": "咖啡", "transcribe_audio": false });
        assert!(!strip_pro_input(&mut off));
    }
}
