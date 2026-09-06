use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};

use crate::agent::tool::ToolProgressSender;
use crate::agent::{Backend as LlmProvider, Tool, ToolContext, ToolResult};
use crate::cdp::PageSession;
use crate::media::MediaProcessor;
use crate::sites::dy::DouyinPageRuntime;
use crate::sites::registry::{
    required_string, ArgKind, BoxFuture, CommandArg, SiteCommand, SiteSpec, SlowWhen,
};
use crate::sites::runner::{get_f64, get_i64, json_result, run_tool_command, ToolCommand};

pub const DY_KNOWLEDGE: &str = include_str!("knowledge.md");

const MAX_VIDEO_DOWNLOAD_BYTES: usize = 128 * 1024 * 1024;
const MAX_POSTER_DOWNLOAD_BYTES: usize = 20 * 1024 * 1024;

pub fn dy_tools(page: Arc<PageSession>) -> Vec<Arc<dyn Tool>> {
    dy_tools_with_llm_provider(page, None)
}

pub fn dy_tools_with_llm_provider(
    page: Arc<PageSession>,
    llm_provider: Option<Arc<dyn LlmProvider>>,
) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(SearchTool { page: page.clone() }) as Arc<dyn Tool>,
        Arc::new(GetVideosTool {
            page: page.clone(),
            llm_provider,
        }),
        Arc::new(AuthorScanTool { page: page.clone() }),
        Arc::new(PageStateTool { page: page.clone() }),
        Arc::new(WaitForLoginTool { page }),
    ]
}

pub async fn dy_agent_tools(
    page: Arc<PageSession>,
    llm_provider: Arc<dyn LlmProvider>,
) -> anyhow::Result<Vec<Arc<dyn Tool>>> {
    Ok(dy_tools_with_llm_provider(page, Some(llm_provider)))
}

pub fn dy_agent_instructions(extra: &str) -> String {
    let base = DY_KNOWLEDGE.trim().to_string();
    let extra = extra.trim();
    if extra.is_empty() {
        base
    } else {
        format!("{extra}\n\n{base}")
    }
}

pub static DY_SITE: SiteSpec = SiteSpec {
    id: "dy",
    about: "Douyin (douyin.com)",
    // Let Douyin tools own first navigation so they can use a much longer
    // timeout for the site's occasional 4-5 minute blank-page throttling.
    home_url: "",
    agent_tools: |page, llm| Box::pin(dy_agent_tools(page, llm)),
    default_agent_tools: None,
    agent_instructions: dy_agent_instructions,
    default_agent_instructions: None,
    commands: &[
        SiteCommand {
            name: "search",
            tool_name: "search",
            about: "Search Douyin and print video result cards as JSON.",
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
                    key: "num",
                    long: Some("num"),
                    value_name: "N",
                    help: "Number of video cards to collect by scrolling. Defaults to 10.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for page/search transitions. Use 300+ when Douyin web is throttled.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_search,
        },
        SiteCommand {
            name: "get-videos",
            tool_name: "get_videos",
            about: "Read Douyin video details and top comments by video id or URL.",
            args: &[
                CommandArg {
                    key: "videos",
                    long: Some("video"),
                    value_name: "ID_OR_URL",
                    help: "Douyin video id or URL. Repeat to read multiple videos.",
                    required: true,
                    kind: ArgKind::StrList,
                },
                CommandArg {
                    key: "num_comments",
                    long: Some("num-comments"),
                    value_name: "N",
                    help: "Top comments to collect per video. Defaults to 8; 0 skips comments.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "download_media",
                    long: Some("download-media"),
                    value_name: "",
                    help: "Download the playable video and cover into the run directory.",
                    required: false,
                    kind: ArgKind::Flag,
                },
                CommandArg {
                    key: "transcribe_audio",
                    long: Some("transcribe-audio"),
                    value_name: "",
                    help: "Download the video and transcribe its audio through the paid socai ASR service.",
                    required: false,
                    kind: ArgKind::Flag,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for each video page transition. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_get_videos,
        },
        SiteCommand {
            name: "author",
            tool_name: "author_scan",
            about: "Read a Douyin author profile and collect their visible video cards.",
            args: &[
                CommandArg {
                    key: "author",
                    long: None,
                    value_name: "ID_OR_URL",
                    help: "Douyin author id or profile URL.",
                    required: true,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "num",
                    long: Some("num"),
                    value_name: "N",
                    help: "Number of video cards to collect by scrolling. Omit for the first visible screen.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for the author page transition. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_author_scan,
        },
        SiteCommand {
            name: "page_state",
            tool_name: "page_state",
            about: "Open or reuse Douyin and print page state as JSON.",
            args: &[CommandArg {
                key: "wait_seconds",
                long: Some("wait-seconds"),
                value_name: "SECONDS",
                help: "Maximum wait for a non-blank Douyin page. Use 300+ when the web page is throttled.",
                required: false,
                kind: ArgKind::Int,
            }],
            slow: SlowWhen::Always,
            run: run_page_state,
        },
    ],
};

fn run_get_videos(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    Box::pin(async move {
        let mut args = args;
        if args
            .get("transcribe_audio")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            args["download_media"] = Value::Bool(true);
        }
        run_tool_command(
            ToolCommand {
                site_id: "dy",
                command_name: "get-videos",
                tool_name: "get_videos",
                before: None,
                after: None,
                include_run_metadata: true,
            },
            page.clone(),
            &dy_tools(page),
            args,
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
        run_tool_command(
            ToolCommand {
                site_id: "dy",
                command_name: "author",
                tool_name: "author_scan",
                before: None,
                after: None,
                include_run_metadata: false,
            },
            page.clone(),
            &dy_tools(page),
            args,
            debug_snapshot,
            progress,
        )
        .await
    })
}

fn run_search(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    Box::pin(async move {
        run_tool_command(
            ToolCommand {
                site_id: "dy",
                command_name: "search",
                tool_name: "search",
                before: None,
                after: None,
                include_run_metadata: false,
            },
            page.clone(),
            &dy_tools(page),
            args,
            debug_snapshot,
            progress,
        )
        .await
    })
}

fn run_page_state(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    Box::pin(async move {
        let wait_seconds = get_f64(&args, "wait_seconds", 330.0);
        run_tool_command(
            ToolCommand {
                site_id: "dy",
                command_name: "page_state",
                tool_name: "page_state",
                before: Some(Box::new(move |page| {
                    Box::pin(async move {
                        let runtime = DouyinPageRuntime::new(&page);
                        runtime.ensure_douyin(true, wait_seconds).await?;
                        let _ = runtime.wait_until_interactive(wait_seconds).await?;
                        Ok(())
                    })
                })),
                after: None,
                include_run_metadata: false,
            },
            page.clone(),
            &dy_tools(page),
            args,
            debug_snapshot,
            progress,
        )
        .await
    })
}

pub struct SearchTool {
    page: Arc<PageSession>,
}

pub struct GetVideosTool {
    page: Arc<PageSession>,
    llm_provider: Option<Arc<dyn LlmProvider>>,
}

#[async_trait]
impl Tool for GetVideosTool {
    fn name(&self) -> &str {
        "get_videos"
    }

    fn description(&self) -> &str {
        "Read one or more Douyin videos by id or URL. Returns a normalized video entity, \
         creator identity, engagement fields, playable media metadata, and top comments. \
         Set download_media to save video files or transcribe_audio to use the paid socai ASR service."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "videos": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1,
                    "description": "Douyin video ids or full douyin.com video URLs."
                },
                "num_comments": {
                    "type": "integer",
                    "default": 8,
                    "minimum": 0
                },
                "download_media": { "type": "boolean", "default": false },
                "ocr": {
                    "type": "boolean",
                    "description": "Run local OCR on the video cover without requiring a video-file download.",
                    "default": false
                },
                "transcribe_audio": { "type": "boolean", "default": false },
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1 }
            },
            "required": ["videos"]
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let videos = string_list(&input, "videos")?;
        let wait_seconds = get_f64(&input, "wait_seconds", 30.0);
        let num_comments = get_i64(&input, "num_comments", 8).max(0) as usize;
        let ocr = input.get("ocr").and_then(Value::as_bool).unwrap_or(false);
        let transcribe_audio = input
            .get("transcribe_audio")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let download_media = input
            .get("download_media")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || transcribe_audio;
        let (media, media_init_error) = if download_media || ocr {
            let setup = (|| -> anyhow::Result<_> {
                let mut processor =
                    MediaProcessor::for_run_dir(ctx.output_dir(), self.llm_provider.clone())?;
                processor.set_cloud_asr(transcribe_audio);
                processor.set_billing_task_id(ctx.billing_task_id.as_deref());
                Ok((processor, SafeDouyinMediaClient::new()?))
            })();
            match setup {
                Ok(media) => (Some(media), None),
                Err(error) => (None, Some(format!("{error:#}"))),
            }
        } else {
            (None, None)
        };
        let runtime = DouyinPageRuntime::new(&self.page);
        let mut results = Vec::with_capacity(videos.len());
        for locator in videos {
            let mut result = match runtime
                .read_video(&locator, wait_seconds, num_comments, transcribe_audio)
                .await
            {
                Ok(value) => value,
                Err(err) => json!({
                    "ok": false,
                    "reason": "video_read_failed",
                    "error": format!("{err:#}"),
                }),
            };
            result["locator"] = Value::String(locator);
            if let Some((processor, downloader)) = &media {
                enrich_video_result(
                    processor,
                    downloader,
                    &mut result,
                    download_media,
                    ocr,
                    transcribe_audio,
                )
                .await;
            } else if let Some(error) = &media_init_error {
                attach_enrichment_initialization_error(&mut result, error);
            }
            results.push(result);
        }
        let failures = results
            .iter()
            .filter(|item| item.get("ok").and_then(Value::as_bool) != Some(true))
            .count();
        let payload = json!({
            "ok": failures == 0,
            "count": results.len(),
            "failures": failures,
            "videos": results,
        });
        Ok(json_result(&payload))
    }
}

fn attach_enrichment_initialization_error(result: &mut Value, error: &str) {
    if result.get("entity").is_some_and(Value::is_object) {
        attach_optional_enrichment_error(result, "enrichment_initialization_failed", error);
    }
}

fn attach_optional_enrichment_error(result: &mut Value, reason: &str, error: &str) {
    result["enrichment"] = json!({
        "ok": false,
        "reason": reason,
        "error": error,
    });
}

pub struct AuthorScanTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for AuthorScanTool {
    fn name(&self) -> &str {
        "author_scan"
    }

    fn description(&self) -> &str {
        "Open a Douyin author profile by id or URL and return normalized author fields plus \
         visible video cards. Use the cards with get_videos when full details are needed."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "author": { "type": "string", "description": "Douyin author id or profile URL." },
                "num": {
                    "type": "integer",
                    "description": "Collect at least this many video cards by scrolling. Omit for the first visible screen.",
                    "minimum": 1
                },
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1 }
            },
            "required": ["author"]
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let author = required_string(&input, "author")?;
        let num = input
            .get("num")
            .and_then(Value::as_i64)
            .filter(|value| *value > 0)
            .map(|value| value as usize);
        let wait_seconds = get_f64(&input, "wait_seconds", 30.0);
        let runtime = DouyinPageRuntime::new(&self.page);
        let result = runtime.read_author(&author, wait_seconds, num).await?;
        Ok(json_result(&result))
    }
}

#[async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }

    fn description(&self) -> &str {
        "Search Douyin for videos matching `query` and return visible result \
         cards (video id, URL, title, author, cover, and any engagement text \
         the page exposes). Defaults to 10 cards and may wait several minutes \
         if Douyin web is throttled."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" },
                "num": {
                    "type": "integer",
                    "description": "Number of video cards to collect by scrolling.",
                    "default": 10,
                    "minimum": 1
                },
                "wait_seconds": {
                    "type": "number",
                    "description": "Maximum wait for page/search transitions.",
                    "default": 330
                }
            },
            "required": ["query"]
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let query = required_string(&input, "query")?;
        let wait_seconds = get_f64(&input, "wait_seconds", 330.0);
        let num_videos = get_i64(&input, "num", 10).max(1) as usize;
        let runtime = DouyinPageRuntime::new(&self.page);
        let value = runtime
            .search_videos(&query, wait_seconds, num_videos)
            .await?;
        Ok(json_result(&value))
    }
}

pub struct PageStateTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for PageStateTool {
    fn name(&self) -> &str {
        "page_state"
    }

    fn description(&self) -> &str {
        "Read Douyin page state, including URL, title, candidate search inputs, \
         login hints, and whether the page still looks blank/throttled. This \
         may wait several minutes on Douyin web throttling."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "wait_seconds": {
                    "type": "number",
                    "description": "Maximum wait for the Douyin page to become non-blank.",
                    "default": 330
                }
            }
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let wait_seconds = get_f64(&input, "wait_seconds", 330.0);
        let runtime = DouyinPageRuntime::new(&self.page);
        runtime.ensure_douyin(true, wait_seconds).await?;
        let state = runtime.wait_until_interactive(wait_seconds).await?;
        Ok(json_result(&state))
    }
}

pub struct WaitForLoginTool {
    page: Arc<PageSession>,
}

const WAIT_FOR_LOGIN_SECS: u64 = 600;

#[async_trait]
impl Tool for WaitForLoginTool {
    fn name(&self) -> &str {
        "wait_for_login"
    }

    fn description(&self) -> &str {
        "After a Douyin tool reports login_required, keep the current Douyin tab \
         open and poll until the user signs in. Returns logged_in:false after the \
         fixed ten-minute timeout; do not retry the wait after a timeout."
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        if self.page.is_remote_browser() {
            return Ok(json_result(&json!({
                "logged_in": false,
                "remote_browser": true,
                "message": "The hosted Douyin session cannot be signed in by the local user.",
            })));
        }

        let runtime = DouyinPageRuntime::new(&self.page);
        runtime.ensure_douyin(true, 30.0).await?;
        let deadline = std::time::Instant::now() + Duration::from_secs(WAIT_FOR_LOGIN_SECS);
        loop {
            let state = runtime.detect_state().await.unwrap_or_else(|error| {
                json!({
                    "ok": false,
                    "reason": "login_state_unavailable",
                    "error": error.to_string(),
                })
            });
            if state.get("signed_in").and_then(Value::as_bool) == Some(true) {
                return Ok(json_result(&json!({
                    "logged_in": true,
                    "message": "Douyin login detected. Re-run the original tool and continue.",
                })));
            }
            if std::time::Instant::now() >= deadline {
                return Ok(json_result(&json!({
                    "logged_in": false,
                    "timed_out": true,
                    "reason": "login_timeout",
                    "message": "Douyin login was not detected within ten minutes. Fail the task.",
                })));
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

fn string_list(value: &Value, key: &str) -> anyhow::Result<Vec<String>> {
    let values = value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("{key} must be a non-empty array"))?;
    let values: Vec<String> = values
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect();
    if values.is_empty() {
        anyhow::bail!("{key} must contain at least one id or URL");
    }
    Ok(values)
}

async fn enrich_video_result(
    media: &MediaProcessor,
    downloader: &SafeDouyinMediaClient,
    result: &mut Value,
    download_media: bool,
    ocr: bool,
    transcribe_audio: bool,
) {
    let Some(entity) = result.get_mut("entity").and_then(Value::as_object_mut) else {
        return;
    };
    let video_id = entity
        .get("video_id")
        .and_then(Value::as_str)
        .unwrap_or("video")
        .to_string();
    let referer = entity
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut enriched = entity.get("video").cloned().unwrap_or_else(|| json!({}));
    downloader
        .download_poster(media, &mut enriched, &video_id, &referer)
        .await;
    if download_media {
        downloader
            .download_video_file(media, &mut enriched, &video_id, &referer)
            .await;
    }
    if ocr {
        media.ocr_downloaded_video_poster(&mut enriched).await;
    }
    if transcribe_audio {
        downloader
            .download_audio_file(media, &mut enriched, &video_id, &referer)
            .await;
        let transcript_path = enriched
            .get("audio_local_path")
            .and_then(Value::as_str)
            .or_else(|| enriched.get("local_path").and_then(Value::as_str))
            .map(str::to_string);
        if let Some(path) = transcript_path {
            let mut transcript = json!({ "local_path": path });
            media.transcribe_downloaded_video(&mut transcript).await;
            for key in ["transcript", "transcript_error", "transcript_ms"] {
                if let Some(value) = transcript.get(key) {
                    enriched[key] = value.clone();
                }
            }
        } else {
            insert_string(
                &mut enriched,
                "transcript_error",
                "downloadable Douyin audio URL was not available",
            );
        }
    }
    let download_error = enriched
        .get("download_error")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    let poster_download_error = enriched
        .get("poster_download_error")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    let transcript_error = enriched
        .get("transcript_error")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    let transcript_present = enriched
        .get("transcript")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    entity.insert("video".into(), enriched);
    let failure = if let Some(error) = download_error {
        Some(("media_download_failed", error))
    } else if let Some(error) = poster_download_error {
        Some(("poster_download_failed", error))
    } else if transcribe_audio && !transcript_present {
        Some((
            "transcription_failed",
            transcript_error.unwrap_or_else(|| "transcript was empty".to_string()),
        ))
    } else {
        None
    };
    if let Some((reason, error)) = failure {
        result["enrichment"] = json!({
            "ok": false,
            "reason": reason,
            "error": error,
        });
    } else {
        result["enrichment"] = json!({ "ok": true });
    }
}

struct SafeDouyinMediaClient {
    client: reqwest::Client,
}

impl SafeDouyinMediaClient {
    fn new() -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 || !douyin_media_url_allowed(attempt.url()) {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .build()?;
        Ok(Self { client })
    }

    async fn download_poster(
        &self,
        media: &MediaProcessor,
        video: &mut Value,
        video_id: &str,
        referer: &str,
    ) {
        let Some(poster_url) = video
            .get("poster_url")
            .and_then(Value::as_str)
            .filter(|url| douyin_media_url_allowed_str(url))
        else {
            insert_string(
                video,
                "poster_download_error",
                "downloadable Douyin poster URL not found on an allowed host",
            );
            return;
        };
        match self
            .fetch_limited(poster_url, referer, MAX_POSTER_DOWNLOAD_BYTES, "image")
            .await
            .and_then(|bytes| media.save_named_bytes(&bytes, video_id, "post.jpg"))
        {
            Ok(path) => insert_string(video, "poster_local_path", &path.to_string_lossy()),
            Err(err) => insert_string(video, "poster_download_error", &format!("{err:#}")),
        }
    }

    async fn download_video_file(
        &self,
        media: &MediaProcessor,
        video: &mut Value,
        video_id: &str,
        referer: &str,
    ) {
        let Some(source) = downloadable_douyin_video_url(video) else {
            insert_string(
                video,
                "download_error",
                "downloadable Douyin media URL not found on an allowed host",
            );
            return;
        };
        match self
            .fetch_limited(&source, referer, MAX_VIDEO_DOWNLOAD_BYTES, "video")
            .await
            .and_then(|bytes| media.save_bytes(&bytes, video_id, ".mp4"))
        {
            Ok(path) => {
                insert_string(video, "resolved_url", &source);
                insert_string(video, "local_path", &path.to_string_lossy());
            }
            Err(err) => insert_string(video, "download_error", &format!("{err:#}")),
        }
    }

    async fn download_audio_file(
        &self,
        media: &MediaProcessor,
        video: &mut Value,
        video_id: &str,
        referer: &str,
    ) {
        let Some(source) = video
            .get("audio_url")
            .and_then(Value::as_str)
            .filter(|url| douyin_media_url_allowed_str(url) && !is_hls_url(url))
            .map(str::to_string)
        else {
            return;
        };
        match self
            .fetch_limited(&source, referer, MAX_VIDEO_DOWNLOAD_BYTES, "video")
            .await
            .and_then(|bytes| media.save_named_bytes(&bytes, video_id, "audio.m4a"))
        {
            Ok(path) => insert_string(video, "audio_local_path", &path.to_string_lossy()),
            Err(err) => insert_string(video, "audio_download_error", &format!("{err:#}")),
        }
    }

    async fn fetch_limited(
        &self,
        url: &str,
        referer: &str,
        max_bytes: usize,
        expected_type: &str,
    ) -> anyhow::Result<Vec<u8>> {
        let parsed = reqwest::Url::parse(url)?;
        if !douyin_media_url_allowed(&parsed) {
            anyhow::bail!("media URL host or scheme is not allowed");
        }
        let mut request = self.client.get(parsed);
        if !referer.trim().is_empty() {
            request = request.header(reqwest::header::REFERER, referer.trim());
        }
        let response = request.send().await?;
        if response.status().is_redirection() {
            anyhow::bail!("media redirect left the allowed host set");
        }
        let response = response.error_for_status()?;
        if !douyin_media_url_allowed(response.url()) {
            anyhow::bail!("media response URL host or scheme is not allowed");
        }
        if expected_type == "video" && is_hls_url(response.url().as_str()) {
            anyhow::bail!("HLS playlists are not supported as downloadable MP4 media");
        }
        if response
            .content_length()
            .is_some_and(|size| size > max_bytes as u64)
        {
            anyhow::bail!("media response exceeds the {max_bytes} byte limit");
        }
        if let Some(content_type) = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
        {
            let content_type = content_type.to_ascii_lowercase();
            let accepted = if expected_type == "image" {
                content_type.starts_with("image/")
            } else {
                content_type.starts_with("video/mp4")
                    || content_type.starts_with("application/mp4")
                    || content_type.starts_with("application/octet-stream")
                    || content_type.starts_with("binary/octet-stream")
            };
            if !accepted {
                anyhow::bail!("unexpected media content type: {content_type}");
            }
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if bytes.len().saturating_add(chunk.len()) > max_bytes {
                anyhow::bail!("media response exceeds the {max_bytes} byte limit");
            }
            bytes.extend_from_slice(&chunk);
        }
        if expected_type == "video" {
            if looks_like_hls(&bytes) {
                anyhow::bail!("HLS playlists are not supported as downloadable MP4 media");
            }
            if !looks_like_mp4(&bytes) {
                anyhow::bail!("downloaded video payload is not an MP4 container");
            }
        }
        Ok(bytes)
    }
}

fn downloadable_douyin_video_url(video: &Value) -> Option<String> {
    for key in ["resolved_url", "url"] {
        if let Some(url) = video
            .get(key)
            .and_then(Value::as_str)
            .filter(|url| douyin_media_url_allowed_str(url) && !is_hls_url(url))
        {
            return Some(url.to_string());
        }
    }
    for key in ["source_urls", "backup_urls"] {
        if let Some(values) = video.get(key).and_then(Value::as_array) {
            for value in values {
                if let Some(url) = value
                    .as_str()
                    .filter(|url| douyin_media_url_allowed_str(url) && !is_hls_url(url))
                {
                    return Some(url.to_string());
                }
            }
        }
    }
    if let Some(values) = video.get("candidates").and_then(Value::as_array) {
        for value in values {
            if let Some(url) = value
                .get("url")
                .and_then(Value::as_str)
                .filter(|url| douyin_media_url_allowed_str(url) && !is_hls_url(url))
            {
                return Some(url.to_string());
            }
        }
    }
    None
}

fn douyin_media_url_allowed_str(value: &str) -> bool {
    reqwest::Url::parse(value)
        .ok()
        .is_some_and(|url| douyin_media_url_allowed(&url))
}

fn douyin_media_url_allowed(url: &reqwest::Url) -> bool {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    [
        "douyinvod.com",
        "douyinpic.com",
        "douyin.com",
        "byteimg.com",
        "zjcdn.com",
        "bytecdn.cn",
        "snssdk.com",
        "pstatp.com",
        "volccdn.com",
    ]
    .iter()
    .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
}

fn is_hls_url(value: &str) -> bool {
    reqwest::Url::parse(value)
        .ok()
        .is_some_and(|url| url.path().to_ascii_lowercase().ends_with(".m3u8"))
}

fn looks_like_hls(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let first = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    bytes[first..].starts_with(b"#EXTM3U")
}

fn looks_like_mp4(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && bytes.get(4..8) == Some(b"ftyp")
}

fn insert_string(value: &mut Value, key: &str, item: &str) {
    if let Some(map) = value.as_object_mut() {
        map.insert(key.to_string(), Value::String(item.to_string()));
    }
}
