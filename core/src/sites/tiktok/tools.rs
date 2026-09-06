use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

use crate::agent::tool::ToolProgressSender;
use crate::agent::{Backend as LlmProvider, Tool, ToolContext, ToolResult};
use crate::cdp::PageSession;
use crate::media::MediaProcessor;
use crate::sites::registry::{
    required_string, ArgKind, BoxFuture, CommandArg, SiteCommand, SiteSpec, SlowWhen,
};
use crate::sites::runner::{get_f64, get_i64, json_result, run_tool_command, ToolCommand};
use crate::sites::tiktok::TikTokPageRuntime;

pub const TIKTOK_KNOWLEDGE: &str = include_str!("knowledge.md");

const MAX_VIDEO_DOWNLOAD_BYTES: usize = 128 * 1024 * 1024;
const MAX_POSTER_DOWNLOAD_BYTES: usize = 20 * 1024 * 1024;

pub fn tiktok_tools(page: Arc<PageSession>) -> Vec<Arc<dyn Tool>> {
    tiktok_tools_with_llm_provider(page, None)
}

pub fn tiktok_tools_with_llm_provider(
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
        Arc::new(PageStateTool { page }),
    ]
}

pub async fn tiktok_agent_tools(
    page: Arc<PageSession>,
    llm_provider: Arc<dyn LlmProvider>,
) -> anyhow::Result<Vec<Arc<dyn Tool>>> {
    let _ = TikTokPageRuntime::new(&page).ensure_tiktok(true).await;
    Ok(tiktok_tools_with_llm_provider(page, Some(llm_provider)))
}

pub fn tiktok_agent_instructions(extra: &str) -> String {
    let base = TIKTOK_KNOWLEDGE.trim().to_string();
    let extra = extra.trim();
    if extra.is_empty() {
        base
    } else {
        format!("{extra}\n\n{base}")
    }
}

pub static TIKTOK_SITE: SiteSpec = SiteSpec {
    id: "tiktok",
    about: "TikTok (tiktok.com)",
    home_url: "",
    agent_tools: |page, llm| Box::pin(tiktok_agent_tools(page, llm)),
    default_agent_tools: None,
    agent_instructions: tiktok_agent_instructions,
    default_agent_instructions: None,
    commands: &[
        SiteCommand {
            name: "search",
            tool_name: "search",
            about: "Search TikTok and print video result cards as JSON.",
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
                    help: "Maximum wait for the search page transition. Defaults to 30.",
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
            about: "Read TikTok video details and top comments by id or URL.",
            args: &[
                CommandArg {
                    key: "videos",
                    long: Some("video"),
                    value_name: "ID_OR_URL",
                    help: "TikTok video id or URL. Repeat to read multiple videos.",
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
            about: "Read a TikTok author profile and collect visible video cards.",
            args: &[
                CommandArg {
                    key: "author",
                    long: None,
                    value_name: "HANDLE_OR_URL",
                    help: "TikTok author handle or profile URL.",
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
            about: "Open or reuse TikTok and print page state as JSON.",
            args: &[CommandArg {
                key: "wait_seconds",
                long: Some("wait-seconds"),
                value_name: "SECONDS",
                help: "Maximum wait for a non-blank TikTok page. Defaults to 30.",
                required: false,
                kind: ArgKind::Int,
            }],
            slow: SlowWhen::Always,
            run: run_page_state,
        },
    ],
};

fn run_search(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    run_registered_command(
        page,
        args,
        debug_snapshot,
        progress,
        ToolCommand {
            site_id: "tiktok",
            command_name: "search",
            tool_name: "search",
            before: None,
            after: None,
            include_run_metadata: false,
        },
    )
}

fn run_get_videos(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    let mut args = args;
    if args
        .get("transcribe_audio")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        args["download_media"] = Value::Bool(true);
    }
    run_registered_command(
        page,
        args,
        debug_snapshot,
        progress,
        ToolCommand {
            site_id: "tiktok",
            command_name: "get-videos",
            tool_name: "get_videos",
            before: None,
            after: None,
            include_run_metadata: true,
        },
    )
}

fn run_author_scan(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    run_registered_command(
        page,
        args,
        debug_snapshot,
        progress,
        ToolCommand {
            site_id: "tiktok",
            command_name: "author",
            tool_name: "author_scan",
            before: None,
            after: None,
            include_run_metadata: false,
        },
    )
}

fn run_page_state(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    let wait_seconds = get_f64(&args, "wait_seconds", 30.0);
    run_registered_command(
        page,
        args,
        debug_snapshot,
        progress,
        ToolCommand {
            site_id: "tiktok",
            command_name: "page_state",
            tool_name: "page_state",
            before: Some(Box::new(move |page| {
                Box::pin(async move {
                    let runtime = TikTokPageRuntime::new(&page);
                    runtime.ensure_tiktok(true).await?;
                    let _ = runtime.wait_until_interactive(wait_seconds).await?;
                    Ok(())
                })
            })),
            after: None,
            include_run_metadata: false,
        },
    )
}

fn run_registered_command(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
    command: ToolCommand<'static>,
) -> BoxFuture<Value> {
    Box::pin(async move {
        run_tool_command(
            command,
            page.clone(),
            &tiktok_tools(page),
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

#[async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }

    fn description(&self) -> &str {
        "Search TikTok for public videos matching query and return normalized video cards."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" },
                "num": { "type": "integer", "default": 10, "minimum": 1 },
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1 }
            },
            "required": ["query"]
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let query = required_string(&input, "query")?;
        let num = get_i64(&input, "num", 10).max(1) as usize;
        let wait_seconds = get_f64(&input, "wait_seconds", 30.0);
        let result = TikTokPageRuntime::new(&self.page)
            .search_videos(&query, wait_seconds, num)
            .await?;
        Ok(json_result(&result))
    }
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
        "Read one or more TikTok videos by id or URL. Returns normalized video, creator, \
         engagement, media, and top-comment fields. Media can be downloaded and sent to \
         the paid socai ASR service when requested."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "videos": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1,
                    "description": "TikTok video ids, canonical URLs, player URLs, or short links."
                },
                "num_comments": { "type": "integer", "default": 8, "minimum": 0 },
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
                Ok((processor, SafeTikTokMediaClient::new(self.page.clone())?))
            })();
            match setup {
                Ok(media) => (Some(media), None),
                Err(error) => (None, Some(format!("{error:#}"))),
            }
        } else {
            (None, None)
        };
        let runtime = TikTokPageRuntime::new(&self.page);
        let mut results = Vec::with_capacity(videos.len());
        for locator in videos {
            let mut result = match runtime
                .read_video(&locator, wait_seconds, num_comments, download_media)
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
        Ok(json_result(&json!({
            "ok": failures == 0,
            "count": results.len(),
            "failures": failures,
            "videos": results,
        })))
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
        "Read a TikTok public author profile and collect normalized visible video cards. \
         Use the cards with get_videos when full details are needed."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "author": { "type": "string", "description": "TikTok @handle or profile URL." },
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
        let result = TikTokPageRuntime::new(&self.page)
            .read_author(&author, wait_seconds, num)
            .await?;
        Ok(json_result(&result))
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
        "Read TikTok URL, route readiness, login gate, CAPTCHA, media, and result-card state."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1 }
            }
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let wait_seconds = get_f64(&input, "wait_seconds", 30.0);
        let runtime = TikTokPageRuntime::new(&self.page);
        runtime.ensure_tiktok(true).await?;
        let result = runtime.wait_until_interactive(wait_seconds).await?;
        Ok(json_result(&result))
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
    downloader: &SafeTikTokMediaClient,
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
        media.transcribe_downloaded_video(&mut enriched).await;
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

struct SafeTikTokMediaClient {
    client: reqwest::Client,
    page: Arc<PageSession>,
}

impl SafeTikTokMediaClient {
    fn new(page: Arc<PageSession>) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 || !tiktok_media_url_allowed(attempt.url()) {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .build()?;
        Ok(Self { client, page })
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
            .filter(|url| tiktok_media_url_allowed_str(url))
        else {
            insert_string(
                video,
                "poster_download_error",
                "downloadable TikTok poster URL not found on an allowed host",
            );
            return;
        };
        match self
            .download_file(
                media,
                poster_url,
                referer,
                MAX_POSTER_DOWNLOAD_BYTES,
                "image",
                video_id,
                "post.jpg",
            )
            .await
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
        let Some(source) = downloadable_tiktok_video_url(video) else {
            insert_string(
                video,
                "download_error",
                "downloadable TikTok media URL not found on an allowed host",
            );
            return;
        };
        match self
            .download_file(
                media,
                &source,
                referer,
                MAX_VIDEO_DOWNLOAD_BYTES,
                "video",
                video_id,
                "video.mp4",
            )
            .await
        {
            Ok(path) => {
                insert_string(video, "resolved_url", &source);
                insert_string(video, "local_path", &path.to_string_lossy());
            }
            Err(err) => insert_string(video, "download_error", &format!("{err:#}")),
        }
    }

    async fn download_file(
        &self,
        media: &MediaProcessor,
        url: &str,
        referer: &str,
        max_bytes: usize,
        expected_type: &str,
        label: &str,
        filename: &str,
    ) -> anyhow::Result<PathBuf> {
        struct PartialFileGuard(Option<PathBuf>);

        impl Drop for PartialFileGuard {
            fn drop(&mut self) {
                if let Some(path) = self.0.take() {
                    let _ = std::fs::remove_file(path);
                }
            }
        }

        let parsed = reqwest::Url::parse(url)?;
        if !tiktok_media_url_allowed(&parsed) {
            anyhow::bail!("media URL host or scheme is not allowed");
        }
        let path = media.named_path(label, filename)?;
        let part_path = path.with_extension(format!(
            "{}.{}.part",
            path.extension()
                .and_then(|value| value.to_str())
                .unwrap_or("bin"),
            uuid::Uuid::new_v4()
        ));
        let mut partial = PartialFileGuard(Some(part_path.clone()));

        let http_result = self
            .fetch_http_file_limited(parsed, referer, max_bytes, expected_type, &part_path)
            .await
            .and_then(|_| validate_downloaded_file(&part_path, expected_type));
        if let Err(http_error) = http_result {
            let _ = tokio::fs::remove_file(&part_path).await;
            self.fetch_browser_file_limited(url, max_bytes, expected_type, &part_path)
                .await
                .and_then(|_| validate_downloaded_file(&part_path, expected_type))
                .map_err(|browser_error| {
                    anyhow::anyhow!(
                        "direct media fetch failed: {http_error:#}; browser-session fetch failed: {browser_error:#}"
                    )
                })?;
        }

        if std::fs::metadata(&path).is_ok_and(|meta| meta.is_file() && meta.len() > 0) {
            tokio::fs::remove_file(&part_path).await?;
        } else {
            tokio::fs::rename(&part_path, &path).await?;
        }
        partial.0 = None;
        Ok(path)
    }

    async fn fetch_http_file_limited(
        &self,
        parsed: reqwest::Url,
        referer: &str,
        max_bytes: usize,
        expected_type: &str,
        destination: &Path,
    ) -> anyhow::Result<()> {
        let mut request = self.client.get(parsed);
        if !referer.trim().is_empty() {
            request = request.header(reqwest::header::REFERER, referer.trim());
        }
        let response = request.send().await?;
        if response.status().is_redirection() {
            anyhow::bail!("media redirect left the allowed host set");
        }
        let response = response.error_for_status()?;
        if !tiktok_media_url_allowed(response.url()) {
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
            if !accepted_media_content_type(content_type, expected_type) {
                anyhow::bail!("unexpected media content type: {content_type}");
            }
        }
        let mut file = tokio::fs::File::create(destination).await?;
        let mut written = 0usize;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if written.saturating_add(chunk.len()) > max_bytes {
                anyhow::bail!("media response exceeds the {max_bytes} byte limit");
            }
            file.write_all(&chunk).await?;
            written += chunk.len();
        }
        if written == 0 {
            anyhow::bail!("media response returned an empty body");
        }
        file.flush().await?;
        Ok(())
    }

    async fn fetch_browser_file_limited(
        &self,
        url: &str,
        max_bytes: usize,
        expected_type: &str,
        destination: &Path,
    ) -> anyhow::Result<()> {
        let (content_type, final_url) = self
            .page
            .fetch_file_with_browser(url, max_bytes, destination)
            .await?;
        let final_url = reqwest::Url::parse(&final_url)?;
        if !tiktok_media_url_allowed(&final_url) {
            anyhow::bail!("browser media response URL host or scheme is not allowed");
        }
        if expected_type == "video" && is_hls_url(final_url.as_str()) {
            anyhow::bail!("HLS playlists are not supported as downloadable MP4 media");
        }
        if !content_type.is_empty() && !accepted_media_content_type(&content_type, expected_type) {
            anyhow::bail!("unexpected browser media content type: {content_type}");
        }
        Ok(())
    }
}

fn validate_downloaded_file(path: &Path, expected_type: &str) -> anyhow::Result<()> {
    let mut file = std::fs::File::open(path)?;
    let mut prefix = [0u8; 4096];
    let read = std::io::Read::read(&mut file, &mut prefix)?;
    let bytes = &prefix[..read];
    if bytes.is_empty() {
        anyhow::bail!("downloaded media file is empty");
    }
    if expected_type == "video" {
        if looks_like_hls(bytes) {
            anyhow::bail!("HLS playlists are not supported as downloadable MP4 media");
        }
        if !looks_like_mp4(bytes) {
            anyhow::bail!("downloaded video payload is not an MP4 container");
        }
    }
    Ok(())
}

fn accepted_media_content_type(content_type: &str, expected_type: &str) -> bool {
    let content_type = content_type.to_ascii_lowercase();
    if expected_type == "image" {
        content_type.starts_with("image/")
    } else {
        content_type.starts_with("video/mp4")
            || content_type.starts_with("application/mp4")
            || content_type.starts_with("application/octet-stream")
            || content_type.starts_with("binary/octet-stream")
    }
}

fn downloadable_tiktok_video_url(video: &Value) -> Option<String> {
    for key in ["resolved_url", "url"] {
        if let Some(url) = video
            .get(key)
            .and_then(Value::as_str)
            .filter(|url| tiktok_media_url_allowed_str(url) && !is_hls_url(url))
        {
            return Some(url.to_string());
        }
    }
    for key in ["source_urls", "backup_urls"] {
        if let Some(values) = video.get(key).and_then(Value::as_array) {
            for value in values {
                if let Some(url) = value
                    .as_str()
                    .filter(|url| tiktok_media_url_allowed_str(url) && !is_hls_url(url))
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
                .filter(|url| tiktok_media_url_allowed_str(url) && !is_hls_url(url))
            {
                return Some(url.to_string());
            }
        }
    }
    None
}

fn tiktok_media_url_allowed_str(value: &str) -> bool {
    reqwest::Url::parse(value)
        .ok()
        .is_some_and(|url| tiktok_media_url_allowed(&url))
}

fn tiktok_media_url_allowed(url: &reqwest::Url) -> bool {
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
        "tiktokcdn.com",
        "tiktokcdn-us.com",
        "tiktokv.com",
        "tiktok.com",
        "byteoversea.com",
        "ibytedtos.com",
        "muscdn.com",
        "akamaized.net",
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
