//! Agent-callable tool wrappers around [`XhsPageRuntime`].
//!
//! Each wrapper owns an `Arc<PageSession>` — the same tab is reused across
//! tool calls so the agent's actions accumulate state (search results
//! visible, note modal open, etc.). The caller is responsible for creating
//! the page and closing it after `run_agent` returns.

use std::sync::Arc;

use crate::agent::{Backend as LlmProvider, Tool, ToolContext, ToolResult};
use crate::cdp::PageSession;
use crate::media::{timing_delta, MediaProcessor, TimingSnapshot};
use async_trait::async_trait;
use serde_json::{json, Map, Value};

use crate::sites::registry::{
    required_string, ArgKind, BoxFuture, CommandArg, SiteCommand, SiteSpec, SlowWhen,
};
use crate::sites::runner::{
    get_bool, get_f64, get_i64, get_str, json_result, run_metadata, run_tool_command,
    trimmed_required, PageHook, ToolCommand,
};
use crate::sites::xhs::media_manifest::{
    ensure_entity_note_id, search_scan_media_manifest, write_media_manifest_file,
};
use crate::sites::xhs::page::XHS_SEARCH_FILTERS;
use crate::sites::xhs::{
    ReadNoteOptions, XhsAuthorProfile, XhsHistoryStore, XhsNoteCard, XhsPageRuntime, XHS_HOME_URL,
};

/// Default number of notes `search_scan` reads when the caller doesn't specify.
const DEFAULT_NUM_NOTES: i64 = 10;

/// Top comments attached to every note read (read_note, extract_note,
/// search_scan, author). Comments are read from the already-open note's DOM
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
        Arc::new(SearchScanTool {
            page: page.clone(),
            llm_provider: llm_provider.clone(),
            history: history.clone(),
        }) as Arc<dyn Tool>,
        Arc::new(ExtractSearchCardsTool {
            page: page.clone(),
            history: history.clone(),
        }),
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
        Arc::new(AuthorScanTool {
            page: page.clone(),
            history,
        }),
        Arc::new(PageStateTool { page }),
    ]
}

pub fn xhs_macro_tools_with_llm_provider(
    page: Arc<PageSession>,
    llm_provider: Option<Arc<dyn LlmProvider>>,
) -> Vec<Arc<dyn Tool>> {
    let history = Arc::new(XhsHistoryStore::open_default());
    vec![
        Arc::new(SearchScanTool {
            page: page.clone(),
            llm_provider,
            history: history.clone(),
        }) as Arc<dyn Tool>,
        Arc::new(AuthorScanTool { page, history }),
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
            name: "search_scan",
            tool_name: "search_scan",
            about: "Search/scan Xiaohongshu. By default opens each result and returns note \
                    bodies + top comments. With --preview, returns only the result cards \
                    (titles/likes/covers) without opening any note.",
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
                           Defaults to 10.",
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
            run: run_search_scan,
        },
        SiteCommand {
            name: "author",
            tool_name: "author",
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
        // ── Deprecated aliases (kept so existing scripts keep working) ──
        // Both delegate to the `search_scan` paths and inject a `deprecation` field
        // into the returned JSON (the CLI client also echoes it to stderr).
        SiteCommand {
            name: "search_notes",
            tool_name: "search_scan",
            about: "[DEPRECATED] Use `search_scan --preview`. Returns result cards only.",
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
                    help: "Search-result filter as `group=option` (repeatable).",
                    required: false,
                    kind: ArgKind::KeyValueMap,
                },
                CommandArg {
                    key: "num_notes",
                    long: Some("num-notes"),
                    value_name: "N",
                    help: "Cards to collect by auto-scrolling; defaults to 10.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_search_notes_deprecated,
        },
        SiteCommand {
            name: "topic_scan",
            tool_name: "search_scan",
            about: "[DEPRECATED] Use `search_scan`. Opens each result and returns body + comments.",
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
                    help: "Search-result filter as `group=option` (repeatable).",
                    required: false,
                    kind: ArgKind::KeyValueMap,
                },
                CommandArg {
                    key: "num_notes",
                    long: Some("num-notes"),
                    value_name: "N",
                    help: "Number of notes to read; scrolls only if the first page holds fewer.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "download_media",
                    long: Some("download-media"),
                    value_name: "DOWNLOAD_MEDIA",
                    help: "Download note images/videos into the run_dir; add local_path fields.",
                    required: false,
                    kind: ArgKind::Flag,
                },
            ],
            slow: SlowWhen::Always,
            run: run_topic_scan_deprecated,
        },
    ],
};

/// `search_scan` dispatches on `--preview`: default opens each result for
/// body + comments; `--preview` returns result cards only.
fn run_search_scan(page: Arc<PageSession>, args: Value, debug_snapshot: bool) -> BoxFuture<Value> {
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
        if preview {
            // Cards only — download_media doesn't apply to a card-only read.
            search_scan_preview_command(
                page,
                "search_scan",
                &query,
                filters.as_ref(),
                num_notes,
                debug_snapshot,
            )
            .await
        } else {
            let download_media = args
                .get("download_media")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            search_scan_command(
                page,
                "search_scan",
                &query,
                filters.as_ref(),
                num_notes,
                download_media,
                debug_snapshot,
            )
            .await
        }
    })
}

/// Inject a `deprecation` notice into a command's result. The CLI prints the
/// envelope's `data` object to stdout, so the notice goes *inside* `data` to be
/// visible to an AI consuming the output (the CLI client also echoes it to
/// stderr for humans).
fn with_deprecation(mut envelope: Value, message: &str) -> Value {
    let notice = Value::String(message.to_string());
    if let Some(data) = envelope.get_mut("data").and_then(Value::as_object_mut) {
        data.insert("deprecation".into(), notice);
    } else if let Some(map) = envelope.as_object_mut() {
        map.insert("deprecation".into(), notice);
    }
    envelope
}

fn run_search_notes_deprecated(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
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
        let envelope = search_scan_preview_command(
            page,
            "search_notes",
            &query,
            filters.as_ref(),
            num_notes,
            debug_snapshot,
        )
        .await?;
        Ok(with_deprecation(
            envelope,
            "`search_notes` is deprecated and will be removed in a future release. \
             Use `socai xhs search_scan --preview` instead (same result cards).",
        ))
    })
}

fn run_topic_scan_deprecated(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
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
        let download_media = args
            .get("download_media")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let envelope = search_scan_command(
            page,
            "topic_scan",
            &query,
            filters.as_ref(),
            num_notes,
            download_media,
            debug_snapshot,
        )
        .await?;
        Ok(with_deprecation(
            envelope,
            "`topic_scan` is deprecated and will be removed in a future release. \
             Use `socai xhs search_scan` instead (same behavior).",
        ))
    })
}

fn run_author_scan(page: Arc<PageSession>, args: Value, debug_snapshot: bool) -> BoxFuture<Value> {
    Box::pin(async move {
        let author_id = required_string(&args, "author_id")?;
        // Default to DEFAULT_NUM_NOTES so omitting --num-notes collects a fixed
        // batch (scrolling as needed), not just whatever the first page renders.
        let num_notes = args
            .get("num_notes")
            .and_then(Value::as_i64)
            .or(Some(DEFAULT_NUM_NOTES));
        // Default reads each note; --preview returns cards only.
        let preview = args
            .get("preview")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let download_media = args
            .get("download_media")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        author_scan_command(
            page,
            &author_id,
            num_notes,
            !preview,
            download_media,
            debug_snapshot,
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

const SEARCH_SCAN_PREVIEW_COMMAND: XhsCommandSpec = XhsCommandSpec {
    command_name: "search_scan",
    tool_name: "search_scan",
    before: CommandPageAction::SearchReady,
    after: CommandPageAction::None,
    include_run_metadata: false,
};

const SEARCH_SCAN_COMMAND: XhsCommandSpec = XhsCommandSpec {
    command_name: "search_scan",
    tool_name: "search_scan",
    before: CommandPageAction::SearchReady,
    after: CommandPageAction::None,
    include_run_metadata: true,
};

const AUTHOR_COMMAND: XhsCommandSpec = XhsCommandSpec {
    command_name: "author",
    tool_name: "author",
    // The tool navigates to the profile URL itself; just make sure no stale
    // note modal is left open before/after.
    before: CommandPageAction::CloseOpenNote,
    after: CommandPageAction::CloseOpenNote,
    include_run_metadata: false,
};

pub async fn search_scan_preview_command(
    page: Arc<PageSession>,
    command_name: &'static str,
    query: &str,
    filters: Option<&Value>,
    num_notes: Option<i64>,
    debug_snapshot: bool,
) -> anyhow::Result<Value> {
    // `command_name` decouples the user-facing command (e.g. `search_scan --preview`)
    // from the underlying `search_scan` tool, so the run-dir label and envelope show
    // what was actually invoked rather than the shared implementation name.
    let spec = XhsCommandSpec {
        command_name,
        ..SEARCH_SCAN_PREVIEW_COMMAND
    };
    run_xhs_tool_command(
        page,
        spec,
        search_preview_input(query, filters, num_notes)?,
        debug_snapshot,
    )
    .await
}

pub async fn search_scan_command(
    page: Arc<PageSession>,
    command_name: &'static str,
    query: &str,
    filters: Option<&Value>,
    num_notes: Option<i64>,
    download_media: bool,
    debug_snapshot: bool,
) -> anyhow::Result<Value> {
    // The user-facing `search_scan` command shares this detail operation with
    // the deprecated `topic_scan` alias; record whichever name the caller invoked
    // so the run-dir label and envelope reflect the actual command.
    let spec = XhsCommandSpec {
        command_name,
        ..SEARCH_SCAN_COMMAND
    };
    run_xhs_tool_command(
        page,
        spec,
        search_scan_input(query, filters, num_notes, download_media)?,
        debug_snapshot,
    )
    .await
}

pub async fn author_scan_command(
    page: Arc<PageSession>,
    author_id: &str,
    num_notes: Option<i64>,
    read_notes: bool,
    download_media: bool,
    debug_snapshot: bool,
) -> anyhow::Result<Value> {
    run_xhs_tool_command(
        page,
        AUTHOR_COMMAND,
        author_scan_input(author_id, num_notes, read_notes, download_media)?,
        debug_snapshot,
    )
    .await
}

fn search_preview_input(
    query: &str,
    filters: Option<&Value>,
    num_notes: Option<i64>,
) -> anyhow::Result<Value> {
    let mut input = json!({
        "query": trimmed_required(query, "query")?,
        "preview": true,
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

fn search_scan_input(
    query: &str,
    filters: Option<&Value>,
    num_notes: Option<i64>,
    download_media: bool,
) -> anyhow::Result<Value> {
    let mut input = json!({
        "query": trimmed_required(query, "query")?,
        "preview": false,
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
    Ok(input)
}

fn author_scan_input(
    author_id: &str,
    num_notes: Option<i64>,
    read_notes: bool,
    download_media: bool,
) -> anyhow::Result<Value> {
    let mut input = json!({
        "author_id": trimmed_required(author_id, "author_id")?,
    });
    if let Some(n) = num_notes {
        input["num_notes"] = json!(n.max(1));
    }
    if read_notes {
        input["read_notes"] = json!(true);
    }
    if download_media {
        input["download_media"] = json!(true);
    }
    Ok(input)
}

async fn run_xhs_tool_command(
    page: Arc<PageSession>,
    spec: XhsCommandSpec,
    input: Value,
    debug_snapshot: bool,
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

/// Open one already-selected card, read its body at `level`, attach top
/// comments, and record it in run + cross-run history. Shared by `search_scan`
/// (cards from search results) and `author` (cards from a profile page) — the only
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
) -> Value {
    let requested_media = include_media;
    let can_use_cached_reads = !download_media;

    // Dedup: skip notes already processed at this level or deeper within the
    // same run OR in a previous run (cross-run history). Media downloads are
    // never cache-skipped: the caller expects fresh files under the run_dir.
    if can_use_cached_reads
        && !card.note_id.is_empty()
        && ctx.has_processed_note(&card.note_id, level, requested_media)
    {
        return skipped_note_entry(card, "already_processed", history);
    }
    if can_use_cached_reads
        && !card.note_id.is_empty()
        && history.is_satisfied_by(&card.note_id, level, requested_media)
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

/// Trim a search_scan / author bundle to the shape we hand back to the
/// agent/CLI so it doesn't dominate an LLM context window. The full payload is
/// still written to the run artifact (`<run_dir>/artifacts/…json`), which is the
/// place to look for everything dropped here. Diagnostic blocks (`sampling`,
/// `timing`) and the `search_scan` result block are dropped; the opened `notes`
/// (each trimmed by [`lean_scan_note`]) carry the per-note listing, so
/// `author`'s `profile.note_cards` — a duplicate of that listing — is
/// dropped too once notes were read.
fn lean_scan_payload(payload: &mut Value) {
    let Some(obj) = payload.as_object_mut() else {
        return;
    };
    obj.remove("search_scan");
    obj.remove("sampling");
    obj.remove("timing");
    let notes_read = obj
        .get("notes")
        .and_then(Value::as_array)
        .is_some_and(|notes| !notes.is_empty());
    // author only: `profile.note_cards` repeats the note listing now
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

/// Trim one scanned-note entry to the lean shape: drop the body-source marker
/// (`content_source`) and the per-extract wait diagnostics (`wait`,
/// `top_comments_wait`), and collapse `top_comments` to a plain array of comment
/// texts. Applies to both freshly-read and reused (cached) entities; the full
/// objects stay in the run artifact.
fn lean_scan_note(note: &mut Value) {
    let Some(entry) = note.as_object_mut() else {
        return;
    };
    let Some(entity) = entry.get_mut("entity").and_then(Value::as_object_mut) else {
        return;
    };
    let media_summary = lean_entity_media_summary(entity);
    entity.remove("content_source");
    entity.remove("wait");
    entity.remove("top_comments_wait");
    entity.remove("images");
    entity.remove("video");
    entity.insert("media_summary".into(), media_summary);
    if let Some(comments) = entity.get_mut("top_comments").and_then(Value::as_array_mut) {
        let texts: Vec<Value> = comments
            .iter()
            .filter_map(|comment| comment.get("text").cloned())
            .filter(|text| text.as_str().is_some_and(|s| !s.is_empty()))
            .collect();
        entity.insert("top_comments".into(), Value::Array(texts));
    }
}

fn lean_entity_media_summary(entity: &Map<String, Value>) -> Value {
    let image_count = entity
        .get("images")
        .and_then(Value::as_array)
        .map(Vec::len)
        .or_else(|| {
            entity
                .get("image_count")
                .and_then(Value::as_i64)
                .and_then(|count| usize::try_from(count).ok())
        })
        .unwrap_or_default();
    let local_image_count = entity
        .get("images")
        .and_then(Value::as_array)
        .map(|images| {
            images
                .iter()
                .filter(|image| get_str(image, "local_path").is_some_and(|path| !path.is_empty()))
                .count()
        })
        .unwrap_or_default();
    let video = entity.get("video").filter(|video| video.is_object());
    let has_video = video.is_some_and(|video| {
        get_str(video, "url").is_some_and(|url| !url.is_empty())
            || get_str(video, "resolved_url").is_some_and(|url| !url.is_empty())
            || get_str(video, "local_path").is_some_and(|path| !path.is_empty())
    });
    let local_video_count = video
        .filter(|video| {
            get_str(video, "local_path").is_some_and(|path| !path.is_empty())
                || get_str(video, "poster_local_path").is_some_and(|path| !path.is_empty())
        })
        .map(|_| 1)
        .unwrap_or_default();
    let media_count = image_count + usize::from(has_video);
    let local_media_count = local_image_count + local_video_count;
    json!({
        "media_count": media_count,
        "image_count": image_count,
        "has_video": has_video,
        "local_media_count": local_media_count,
        "has_local_media": local_media_count > 0,
    })
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
         Specify either `note_id` (from a card returned by search preview) or \
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

const SEARCH_SCAN_DOWNLOAD_MEDIA_DESCRIPTION: &str = "Always enabled for this macro. Downloads note images/videos into the run dir, includes local_path fields in artifacts, and writes a stable media manifest.";

fn search_scan_description() -> &'static str {
    "Self-contained Xiaohongshu search_scan macro: start from any page, search by query, optionally apply filters, and by default read selected notes with top comments. Set `preview=true` for card-only search results without opening notes. Use this for XHS topic, trend, keyword, market, and note-evidence research. Do not repeat the same search_scan unless the previous one was clearly insufficient."
}

fn search_scan_input_schema(
    download_media_default: bool,
    download_media_description: &str,
) -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": "Search query (Chinese works fine)" },
            "filters": search_filters_schema(),
            "num_notes": {
                "type": "integer",
                "description": "Number of notes/cards to collect. Detail mode opens each selected note for body + top comments; preview mode only scrolls/collects cards.",
                "default": DEFAULT_NUM_NOTES,
                "minimum": 1
            },
            "preview": {
                "type": "boolean",
                "description": "Return card metadata only (titles/likes/covers) without opening notes. Defaults false, which reads note bodies + top comments.",
                "default": false
            },
            "download_media": {
                "type": "boolean",
                "description": download_media_description,
                "default": download_media_default
            },
            "wait_seconds": {
                "type": "number",
                "description": "Preview mode only: extra seconds to wait for cards to load.",
                "default": 2.0
            }
        },
        "required": ["query"]
    })
}

fn force_macro_media_input(input: &mut Value, force_read_notes: bool) -> anyhow::Result<()> {
    let Some(obj) = input.as_object_mut() else {
        anyhow::bail!("macro input must be an object");
    };
    obj.insert("download_media".into(), json!(true));
    if force_read_notes {
        obj.insert("read_notes".into(), json!(true));
    }
    Ok(())
}

fn normalize_author_input(input: &mut Value) -> anyhow::Result<()> {
    if get_str(input, "author_id")
        .map(str::trim)
        .is_some_and(|id| !id.is_empty())
    {
        return Ok(());
    }

    let profile_url = get_str(input, "profile_url")
        .or_else(|| get_str(input, "url"))
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing author_id or profile_url"))?;
    let author_id = author_id_from_profile_url(profile_url).ok_or_else(|| {
        anyhow::anyhow!("profile_url must contain /user/profile/<author_id>; got {profile_url}")
    })?;

    let Some(obj) = input.as_object_mut() else {
        anyhow::bail!("author input must be an object");
    };
    obj.insert("author_id".into(), json!(author_id));
    Ok(())
}

fn author_id_from_profile_url(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let marker = "/user/profile/";
    let start = trimmed
        .find(marker)
        .map(|idx| idx + marker.len())
        .or_else(|| {
            trimmed
                .find("user/profile/")
                .map(|idx| idx + "user/profile/".len())
        })?;
    let tail = &trimmed[start..];
    let id = tail
        .split(['?', '#', '/'])
        .next()
        .unwrap_or_default()
        .trim();
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

/// search_scan(query, filters?, num_notes?, preview?, download_media?) -> search_scan bundle
///
/// Unified XHS search scan macro. By default it opens selected results and returns
/// note bodies + top comments; `preview=true` returns card metadata only.
pub struct SearchScanTool {
    page: Arc<PageSession>,
    llm_provider: Option<Arc<dyn LlmProvider>>,
    history: Arc<XhsHistoryStore>,
}

impl SearchScanTool {
    async fn search_preview(&self, input: Value) -> anyhow::Result<ToolResult> {
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
            .map(|n| n as usize)
            .or(Some(DEFAULT_NUM_NOTES as usize));
        let xhs = XhsPageRuntime::new(&self.page);
        let mut value = xhs
            .search_cards(&query, filters.as_ref(), wait_seconds, num_notes)
            .await?;
        if let Some(cards) = value.get_mut("cards") {
            self.history.annotate_cards(cards);
        }
        // `submit` is the search-submission diagnostic (strategy + page-state
        // echo). The preview path writes no artifact, so keep it only when the
        // search failed (where it explains why) and drop it on success.
        let failed = value.get("ok").and_then(Value::as_bool) == Some(false);
        if !failed {
            if let Some(obj) = value.as_object_mut() {
                obj.remove("submit");
            }
        }
        Ok(json_result(&value))
    }
}

#[async_trait]
impl Tool for SearchScanTool {
    fn name(&self) -> &str {
        "search_scan"
    }

    fn description(&self) -> &str {
        search_scan_description()
    }

    fn input_schema(&self) -> Value {
        search_scan_input_schema(true, SEARCH_SCAN_DOWNLOAD_MEDIA_DESCRIPTION)
    }

    async fn call(&self, mut input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        if get_bool(&input, "preview", false) {
            return self.search_preview(input).await;
        }
        force_macro_media_input(&mut input, false)?;

        let query = get_str(&input, "query")
            .ok_or_else(|| anyhow::anyhow!("missing query"))?
            .to_string();
        let num_notes = get_i64(&input, "num_notes", DEFAULT_NUM_NOTES).max(1);
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

        // Filters are applied after the initial search below, so don't pass
        // them here.
        let search = xhs.search_cards(&query, None, 2.0, None).await?;

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
                "search_scan": search,
                "notes": [],
                "reason": reason,
                "sampling": {
                    "num_notes": num_notes,
                    "selected": 0,
                    "comments_per_note": TOP_COMMENTS_PER_NOTE,
                    "include_media": include_media,
                    "download_media": download_media,
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
                ctx.add_search_scan_note_ids(std::slice::from_ref(&card.note_id));
            }
            selected.push(card.clone());

            let entry = scan_card_note(
                &xhs,
                &self.history,
                ctx,
                &card,
                level,
                comment_count,
                include_media,
                download_media,
            )
            .await;
            notes.push(entry);
            let _ = xhs.close_note(0.6).await;
        }

        let media_timing = match (&media, &media_baseline) {
            (Some(media), Some(before)) => timing_delta(before, &media.timing().snapshot()),
            _ => json!({}),
        };

        // Annotate cards in the search payload against the pre-call snapshot so
        // flags reflect "known before this scan" rather than "known after this
        // scan's own writes". (Only kept in the artifact; the opened notes are
        // the returned listing.)
        let mut search = search;
        if let Some(cards) = search.get_mut("cards") {
            history_snapshot.annotate_cards(cards);
        }

        let media_manifest_metadata = if download_media {
            let media_manifest = search_scan_media_manifest(&notes, &ctx.run_dir);
            let media_manifest_count = media_manifest.as_array().map(Vec::len).unwrap_or_default();
            let (media_manifest_path, media_manifest_error) = match write_media_manifest_file(
                ctx,
                &media_manifest,
                "search_scan",
                "Search scan media manifest",
            ) {
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
            "run": run_metadata(ctx, download_media),
            "filters": filter_result,
            "search_scan": search,
            "notes": notes,
            "sampling": {
                "num_notes": num_notes,
                "selected": selected.len(),
                "comments_per_note": TOP_COMMENTS_PER_NOTE,
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
            &format!("xhs_search_scan_{}", sanitize_for_filename(&query)),
            &payload,
            "artifacts",
            "search_scan",
            "json",
            &format!(
                "Search scan: {query} ({} notes)",
                payload
                    .get("notes")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0)
            ),
            json!({"site": "xhs", "category": "search_scan"}),
        );

        // Artifact above keeps the full bundle; trim what we return so the
        // agent/CLI output stays small.
        lean_scan_payload(&mut payload);
        Ok(json_result(&payload))
    }
}

/// author(author_id/profile_url, num_notes?, read_notes?) -> author profile bundle
///
/// Self-contained author macro entered from an author's profile page instead of
/// a search query: open `…/user/profile/<id>` → read the author header (bio,
/// xhs id, IP location, follower/following/like counts) → collect note summary
/// cards in page order (scrolling to reach `num_notes`) → when `read_notes` is
/// set, open each note and read its body + top comments.
pub struct AuthorScanTool {
    page: Arc<PageSession>,
    history: Arc<XhsHistoryStore>,
}

#[async_trait]
impl Tool for AuthorScanTool {
    fn name(&self) -> &str {
        "author"
    }

    fn description(&self) -> &str {
        "Self-contained Xiaohongshu author/creator macro: open an author's profile page by author_id or profile_url, read the author header (display name, xhs id, bio, IP location, follower/following/liked-&-collected counts), read recent notes with top comments, always download available media into local artifacts, and return structured results plus artifact references. Use this for creator research scoped to one author."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "description": "Provide either author_id or profile_url. The tool validates this at runtime.",
            "properties": {
                "author_id": {
                    "type": "string",
                    "description": "Xiaohongshu user id — the trailing segment of /user/profile/<id>."
                },
                "profile_url": {
                    "type": "string",
                    "description": "Xiaohongshu profile URL. The tool extracts the trailing /user/profile/<id> segment."
                },
                "num_notes": {
                    "type": "integer",
                    "description": "Collect at least this many note cards, scrolling the profile grid (lazy-loaded). Omit for the first screen only.",
                    "minimum": 1
                },
                "read_notes": {
                    "type": "boolean",
                    "description": "Always enabled for this macro. Opens each collected note and reads its body + top comments so artifacts contain post-level evidence.",
                    "default": true
                },
                "download_media": {
                    "type": "boolean",
                    "description": "Always enabled for this macro. Downloads note images/videos into the run dir while reading notes and writes a stable media manifest.",
                    "default": true
                }
            }
        })
    }

    async fn call(&self, mut input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        normalize_author_input(&mut input)?;
        force_macro_media_input(&mut input, true)?;
        let author_id = get_str(&input, "author_id")
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow::anyhow!("missing author_id"))?
            .to_string();
        let read_notes = get_bool(&input, "read_notes", false);
        let download_media = get_bool(&input, "download_media", false);
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

        // Optionally open each collected note and read body + top comments.
        let mut notes: Vec<Value> = Vec::new();
        if read_notes {
            for card in &cards {
                if !card.note_id.is_empty() {
                    ctx.add_search_scan_note_ids(std::slice::from_ref(&card.note_id));
                }
                let entry = scan_card_note(
                    &xhs,
                    &self.history,
                    ctx,
                    card,
                    "deep",
                    TOP_COMMENTS_PER_NOTE,
                    false,
                    download_media,
                )
                .await;
                notes.push(entry);
                let _ = xhs.close_note(0.6).await;
            }
        }

        let media_timing = match (&media, &media_baseline) {
            (Some(media), Some(before)) => timing_delta(before, &media.timing().snapshot()),
            _ => json!({}),
        };

        // Build the media manifest from `notes` before they move into payload.
        let media_manifest_metadata = if download_media {
            let media_manifest = search_scan_media_manifest(&notes, &ctx.run_dir);
            let media_manifest_count = media_manifest.as_array().map(Vec::len).unwrap_or_default();
            let (path, error) = match write_media_manifest_file(
                ctx,
                &media_manifest,
                "author",
                "Author media manifest",
            ) {
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
                "read_notes": read_notes,
                "comments_per_note": if read_notes { TOP_COMMENTS_PER_NOTE } else { 0 },
                "download_media": download_media,
            },
            "timing": { "media": media_timing },
        });

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
        let _ = ctx.write_json_artifact(
            &format!("xhs_author_{}", sanitize_for_filename(&author_id)),
            &payload,
            "artifacts",
            "author",
            "json",
            &format!("Author: {label} ({} notes)", cards.len()),
            json!({"site": "xhs", "category": "author"}),
        );

        // Artifact above keeps the full bundle; trim what we return so the
        // agent/CLI output stays small.
        lean_scan_payload(&mut payload);
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

        let without_media = run_metadata(&ctx, false);
        assert_eq!(without_media["id"], json!("run-1"));
        assert_eq!(without_media["dir"], json!("/tmp/socai-run"));
        assert!(without_media.get("media_dir").is_none());

        let with_media = run_metadata(&ctx, true);
        assert_eq!(with_media["media_dir"], json!("/tmp/socai-run/site_media"));
    }

    #[test]
    fn attach_run_metadata_preserves_existing_run_object() {
        let run = json!({"id": "outer", "dir": "/tmp/outer"});
        let data = crate::sites::runner::attach_run_metadata(json!({"run": {"id": "inner"}}), &run);
        assert_eq!(data["run"]["id"], json!("inner"));
    }

    #[test]
    fn standalone_run_metadata_is_detail_search_only() {
        assert!(!SEARCH_SCAN_PREVIEW_COMMAND.include_run_metadata);
        assert!(SEARCH_SCAN_COMMAND.include_run_metadata);
        assert!(!AUTHOR_COMMAND.include_run_metadata);
    }

    #[test]
    fn author_id_from_profile_url_extracts_path_segment() {
        assert_eq!(
            author_id_from_profile_url(
                "https://www.xiaohongshu.com/user/profile/5ff00000000000000000abcd?xsec_token=abc#feed"
            ),
            Some("5ff00000000000000000abcd".to_string())
        );
        assert_eq!(
            author_id_from_profile_url("/user/profile/abc123/notes"),
            Some("abc123".to_string())
        );
        assert_eq!(
            author_id_from_profile_url("https://www.xiaohongshu.com/explore/abc"),
            None
        );
    }

    #[test]
    fn normalize_author_input_accepts_profile_url() {
        let mut input = json!({
            "profile_url": "https://www.xiaohongshu.com/user/profile/abc123?xsec_token=token",
            "read_notes": true,
        });

        normalize_author_input(&mut input).expect("profile url should normalize");

        assert_eq!(input["author_id"], json!("abc123"));
        assert_eq!(input["read_notes"], json!(true));
    }

    #[test]
    fn force_macro_media_overrides_media_flags() {
        let mut search = json!({"query": "coffee", "download_media": false});
        force_macro_media_input(&mut search, false).expect("search input should normalize");
        assert_eq!(search["download_media"], json!(true));
        assert!(search.get("read_notes").is_none());

        let mut author = json!({"author_id": "abc", "read_notes": false, "download_media": false});
        force_macro_media_input(&mut author, true).expect("author input should normalize");
        assert_eq!(author["download_media"], json!(true));
        assert_eq!(author["read_notes"], json!(true));
    }

    #[test]
    fn lean_scan_note_replaces_media_paths_with_summary() {
        let mut note = json!({
            "entity": {
                "note_id": "note1",
                "images": [{"local_path": "site_media/note1/0.jpg"}],
                "video": {},
                "top_comments": [],
            }
        });

        lean_scan_note(&mut note);

        let entity = note.get("entity").unwrap();
        assert!(entity.get("images").is_none());
        assert!(entity.get("video").is_none());
        assert_eq!(entity["media_summary"]["local_media_count"], json!(1));
        assert!(!serde_json::to_string(entity)
            .unwrap()
            .contains("site_media/note1/0.jpg"));
    }
}
