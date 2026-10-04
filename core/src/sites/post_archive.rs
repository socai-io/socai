//! Normalize extracted social posts into the desktop's durable card archive.
//!
//! Page selectors and navigation remain owned by each site package. This module
//! starts only after a site tool has returned JSON: it maps that JSON to the
//! small, site-neutral `NoteData` contract consumed by the desktop, merges
//! later detail/comment reads, and keeps the original result as a downloadable
//! JSON artifact.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::DateTime;
use serde_json::{json, Map, Value};

use crate::agent::ToolContext;
use crate::cdp::PageSession;
use crate::media::{
    background_media_is_stopped, background_video_download_semaphore, emit_background_media_event,
    reserve_background_video_download, subscribe_background_media_cancellation,
    wait_for_background_media_cancellation, BackgroundMediaEvent,
};

const INSTAGRAM_MEDIA_HOST_SUFFIXES: &[&str] = &["cdninstagram.com", "fbcdn.net", "instagram.com"];
const TIKTOK_MEDIA_HOST_SUFFIXES: &[&str] = &[
    "tiktokcdn.com",
    "tiktokcdn-us.com",
    "tiktokv.com",
    "tiktok.com",
    "byteoversea.com",
    "ibytedtos.com",
    "muscdn.com",
    "akamaized.net",
];
const DOUYIN_MEDIA_HOST_SUFFIXES: &[&str] = &[
    "douyinvod.com",
    "douyinpic.com",
    "douyin.com",
    "byteimg.com",
    "zjcdn.com",
    "bytecdn.cn",
    "snssdk.com",
    "pstatp.com",
    "volccdn.com",
];
const MAX_INSTAGRAM_VIDEO_BYTES: usize = 512 * 1024 * 1024;
const MAX_PREVIEW_VIDEO_BYTES: usize = 512 * 1024 * 1024;

/// Persist an extraction result and update any post cards it contains.
pub fn persist_site_tool_result(
    ctx: &ToolContext,
    site_id: &str,
    tool_name: &str,
    result: &Value,
    page_url: Option<&str>,
) {
    if is_content_tool(tool_name) {
        let _ = ctx.write_json_artifact(
            &format!("{site_id}_{tool_name}"),
            result,
            "artifacts",
            tool_name,
            "json",
            &format!("{site_id} {tool_name} extraction result"),
            json!({ "site": site_id, "tool": tool_name }),
        );
    }

    for (note_id, record) in records_for_site_tool(site_id, tool_name, result) {
        upsert_record(ctx, &note_id, record);
    }

    if tool_name == "comments" {
        let comments = normalized_comments(site_id, result);
        if comments.is_empty() {
            return;
        }
        if let Some((note_id, url)) = note_identity_from_url(site_id, page_url.unwrap_or_default())
        {
            if !ctx.update_recorded_note(&note_id, |record| merge_comments(record, &comments)) {
                let mut record = Map::new();
                record.insert("note_id".into(), Value::String(note_id.clone()));
                record.insert("site".into(), Value::String(site_id.to_string()));
                record.insert("url".into(), Value::String(url));
                record.insert(
                    "title".into(),
                    Value::String(format!("{} post", site_name(site_id))),
                );
                record.insert("comments".into(), Value::Array(comments));
                record.insert("media".into(), Value::Array(Vec::new()));
                record.insert("archived".into(), Value::Bool(true));
                record.insert("level".into(), Value::String("deep".into()));
                ctx.record_note(&note_id, Value::Object(record));
            }
        }
    }
}

/// Download playable video files for posts just archived from TikTok, Douyin,
/// or Instagram. Runs beside the agent, one file at a time, and stops when the
/// user sends another question. Covers stay in the note until each file lands.
pub fn schedule_background_preview_videos(
    page: Arc<PageSession>,
    ctx: &ToolContext,
    site_id: &str,
    tool_name: &str,
    result: &Value,
) {
    let Some(generation) = ctx.background_media_generation else {
        return;
    };
    let hosts: &'static [&'static str] = match site_id {
        "instagram" => INSTAGRAM_MEDIA_HOST_SUFFIXES,
        "tiktok" => TIKTOK_MEDIA_HOST_SUFFIXES,
        "dy" => DOUYIN_MEDIA_HOST_SUFFIXES,
        _ => return,
    };
    let run_dir = ctx.run_dir.to_string_lossy().into_owned();
    if background_media_is_stopped(generation, &run_dir) {
        return;
    }
    for note in preview_video_notes(site_id, tool_name, result) {
        let files: Vec<(usize, String)> = note
            .files
            .into_iter()
            .filter(|(_, url)| preview_video_url_allowed(url, hosts))
            .collect();
        if files.is_empty() {
            continue;
        }
        for (index, _) in &files {
            set_preview_video_state(ctx, &note.note_id, *index, Some("loading"), None, None);
        }
        let Some(reservation) =
            reserve_background_video_download(generation, &run_dir, &note.note_id)
        else {
            continue;
        };
        let page = page.clone();
        let ctx = ctx.clone();
        let note_id = note.note_id.clone();
        let run_dir = run_dir.clone();
        tokio::spawn(async move {
            let _reservation = reservation;
            let mut cancellation = subscribe_background_media_cancellation();
            let semaphore = background_video_download_semaphore();
            let permit = tokio::select! {
                permit = semaphore.acquire_owned() => match permit {
                    Ok(permit) => permit,
                    Err(_) => return,
                },
                _ = wait_for_background_media_cancellation(generation, &run_dir, &mut cancellation) => {
                    clear_preview_video_downloads(&ctx, &note_id, &files);
                    return;
                }
            };
            for (index, source_url) in &files {
                if background_media_is_stopped(generation, &run_dir) {
                    clear_preview_video_downloads(&ctx, &note_id, &files);
                    return;
                }
                let relative = preview_video_relative(&note_id, *index);
                let destination = PathBuf::from(&run_dir).join(&relative);
                if let Some(parent) = destination.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let partial = destination.with_extension("mp4.part");
                let download = page.fetch_file_with_browser(
                    source_url,
                    MAX_PREVIEW_VIDEO_BYTES,
                    &partial,
                    hosts,
                );
                let fetched = tokio::select! {
                    fetched = download => fetched,
                    _ = wait_for_background_media_cancellation(generation, &run_dir, &mut cancellation) => {
                        page.cancel_browser_resource_fetches().await;
                        let _ = std::fs::remove_file(&partial);
                        clear_preview_video_downloads(&ctx, &note_id, &files);
                        return;
                    }
                };
                if background_media_is_stopped(generation, &run_dir) {
                    page.cancel_browser_resource_fetches().await;
                    let _ = std::fs::remove_file(&partial);
                    clear_preview_video_downloads(&ctx, &note_id, &files);
                    return;
                }
                let saved = match fetched {
                    Ok((content_type, _))
                        if preview_video_content_type_ok(&content_type)
                            && mp4_file_ok(&partial) =>
                    {
                        if std::fs::rename(&partial, &destination).is_ok() {
                            true
                        } else {
                            let _ = std::fs::remove_file(&partial);
                            false
                        }
                    }
                    Ok(_) => {
                        let _ = std::fs::remove_file(&partial);
                        false
                    }
                    Err(error) => {
                        let _ = std::fs::remove_file(&partial);
                        tracing::warn!(
                            note_id,
                            error = %error,
                            "background preview video download failed"
                        );
                        false
                    }
                };
                if saved {
                    if set_preview_video_state(
                        &ctx,
                        &note_id,
                        *index,
                        None,
                        None,
                        Some(relative.as_str()),
                    ) {
                        emit_background_media_event(BackgroundMediaEvent {
                            run_dir: run_dir.clone(),
                            note_id: note_id.clone(),
                        });
                    }
                } else if set_preview_video_state(
                    &ctx,
                    &note_id,
                    *index,
                    Some("failed"),
                    Some("video download failed"),
                    None,
                ) {
                    emit_background_media_event(BackgroundMediaEvent {
                        run_dir: run_dir.clone(),
                        note_id: note_id.clone(),
                    });
                }
            }
            drop(permit);
        });
    }
}

struct PreviewVideoNote {
    note_id: String,
    files: Vec<(usize, String)>,
}

fn preview_video_notes(site_id: &str, tool_name: &str, result: &Value) -> Vec<PreviewVideoNote> {
    match (site_id, tool_name) {
        ("instagram", "searchResults" | "profilePosts") => result
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| instagram_card_video(item))
            .collect(),
        ("instagram", "postDetail") => instagram_detail_videos(result).into_iter().collect(),
        ("tiktok" | "dy", "videoCards") => result
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| platform_card_video(site_id, item))
            .collect(),
        ("tiktok" | "dy", "search") => result
            .get("cards")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| platform_card_video(site_id, item))
            .collect(),
        ("tiktok" | "dy", "author_scan") => result
            .pointer("/profile/video_cards")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| platform_card_video(site_id, item))
            .collect(),
        ("tiktok" | "dy", "videoDetail") => {
            platform_detail_video(site_id, result).into_iter().collect()
        }
        ("tiktok" | "dy", "get_videos") => result
            .get("videos")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| platform_detail_video(site_id, item.get("entity").unwrap_or(item)))
            .collect(),
        _ => Vec::new(),
    }
}

fn instagram_card_video(item: &Value) -> Option<PreviewVideoNote> {
    let native_id = text_at(item, &["shortcode", "id"]);
    let source = text_at(item, &["video_url"]);
    if native_id.is_empty() || !is_remote_url(&source) {
        return None;
    }
    Some(PreviewVideoNote {
        note_id: post_note_id("instagram", &native_id),
        files: vec![(0, source)],
    })
}

fn instagram_detail_videos(item: &Value) -> Option<PreviewVideoNote> {
    let native_id = text_at(item, &["shortcode", "id"]);
    if native_id.is_empty() {
        return None;
    }
    let from_media = video_files_from_media_list(item.get("media"));
    let files = if from_media.is_empty() {
        let source = text_at(item, &["video_url"]);
        if is_remote_url(&source) && !is_byte_range_preview(&source) {
            vec![(0, source)]
        } else {
            Vec::new()
        }
    } else {
        from_media
    };
    if files.is_empty() {
        return None;
    }
    Some(PreviewVideoNote {
        note_id: post_note_id("instagram", &native_id),
        files,
    })
}

fn platform_card_video(site_id: &str, item: &Value) -> Option<PreviewVideoNote> {
    let native_id = text_at(item, &["video_id", "id"]);
    let source = text_at(item, &["video_url"]);
    if native_id.is_empty() || !is_remote_url(&source) {
        return None;
    }
    Some(PreviewVideoNote {
        note_id: post_note_id(site_id, &native_id),
        files: vec![(0, source)],
    })
}

fn platform_detail_video(site_id: &str, item: &Value) -> Option<PreviewVideoNote> {
    let native_id = text_at(item, &["video_id", "id"]);
    let video = item.get("video").unwrap_or(&Value::Null);
    let local = text_at(video, &["local_path"]);
    if native_id.is_empty() || (!local.is_empty() && !is_remote_url(&local)) {
        return None;
    }
    let source = remote_video_url(video);
    if source.is_empty() {
        return None;
    }
    Some(PreviewVideoNote {
        note_id: post_note_id(site_id, &native_id),
        files: vec![(0, source)],
    })
}

fn video_files_from_media_list(media: Option<&Value>) -> Vec<(usize, String)> {
    let mut files = Vec::new();
    let mut video_index = 0usize;
    for item in media.and_then(Value::as_array).into_iter().flatten() {
        if text_at(item, &["type", "kind"]) != "video" {
            continue;
        }
        let raw_src = text_at(item, &["local_path", "url", "src"]);
        let poster = text_at(item, &["poster_local_path", "poster_url", "poster"]);
        let local = text_at(item, &["local_path"]);
        let has_local = !local.is_empty() && !is_remote_url(&local);
        let remote =
            if is_remote_url(&raw_src) && !is_hls_url(&raw_src) && !is_byte_range_preview(&raw_src)
            {
                raw_src
            } else {
                String::new()
            };
        if !has_local && poster.is_empty() && remote.is_empty() {
            continue;
        }
        if !has_local && !remote.is_empty() {
            files.push((video_index, remote));
        }
        video_index += 1;
    }
    files
}

fn clear_preview_video_downloads(ctx: &ToolContext, note_id: &str, files: &[(usize, String)]) {
    let mut changed = false;
    for (index, _) in files {
        changed |= set_preview_video_state(ctx, note_id, *index, None, None, None);
    }
    if changed {
        emit_background_media_event(BackgroundMediaEvent {
            run_dir: ctx.run_dir.to_string_lossy().into_owned(),
            note_id: note_id.to_string(),
        });
    }
}

fn set_preview_video_state(
    ctx: &ToolContext,
    note_id: &str,
    video_index: usize,
    status: Option<&str>,
    error: Option<&str>,
    src: Option<&str>,
) -> bool {
    ctx.update_recorded_note(note_id, |record| {
        let wrote_src = {
            let Some(items) = record.get_mut("media").and_then(Value::as_array_mut) else {
                return;
            };
            let Some(video) = items
                .iter_mut()
                .filter(|item| item.get("kind").and_then(Value::as_str) == Some("video"))
                .nth(video_index)
                .and_then(Value::as_object_mut)
            else {
                return;
            };
            match status {
                Some(status) => {
                    video.insert("status".into(), Value::String(status.into()));
                }
                None => {
                    video.remove("status");
                }
            }
            match error {
                Some(error) => {
                    video.insert("error".into(), Value::String(error.into()));
                }
                None => {
                    video.remove("error");
                }
            }
            if let Some(src) = src {
                video.insert("src".into(), Value::String(src.into()));
                true
            } else {
                false
            }
        };
        if wrote_src {
            if let Some(map) = record.as_object_mut() {
                map.insert("saved".into(), Value::Bool(true));
            }
            // The cover is media[0]. A grid card may have archived the thumbnail
            // as an image before the playable file existed; the downloaded video
            // is the preview the app should play.
            if video_index == 0 {
                if let Some(items) = record.get_mut("media").and_then(Value::as_array_mut) {
                    if let Some(position) = items
                        .iter()
                        .position(|item| item.get("kind").and_then(Value::as_str) == Some("video"))
                    {
                        if position > 0 {
                            let video = items.remove(position);
                            items.insert(0, video);
                        }
                    }
                }
            }
        }
    })
}

fn preview_video_relative(note_id: &str, index: usize) -> String {
    let safe: String = note_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if index == 0 {
        format!("site_media/{safe}/video.mp4")
    } else {
        format!("site_media/{safe}/video-{index}.mp4")
    }
}

fn preview_video_url_allowed(url: &str, suffixes: &[&str]) -> bool {
    if is_hls_url(url) || !is_remote_url(url) {
        return false;
    }
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
    {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    let host_allowed = suffixes
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")));
    host_allowed && !is_site_page_url(&parsed)
}

/// www.tiktok.com / www.douyin.com / www.instagram.com share a suffix with real
/// media hosts. A search or profile page is not a video file.
fn is_site_page_url(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str().map(|host| host.to_ascii_lowercase()) else {
        return false;
    };
    let site = matches!(
        host.as_str(),
        "tiktok.com"
            | "www.tiktok.com"
            | "m.tiktok.com"
            | "douyin.com"
            | "www.douyin.com"
            | "m.douyin.com"
            | "instagram.com"
            | "www.instagram.com"
    );
    if !site {
        return false;
    }
    let path = url.path().to_ascii_lowercase();
    let query = url.query().unwrap_or("").to_ascii_lowercase();
    let media = path.ends_with(".mp4")
        || path.contains("/video/tos/")
        || path.contains("/aweme/v1/play")
        || query.contains("mime_type=video");
    !media
}

fn preview_video_content_type_ok(content_type: &str) -> bool {
    let kind = content_type.to_ascii_lowercase();
    kind.is_empty()
        || kind.starts_with("video/mp4")
        || kind.starts_with("application/mp4")
        || kind.starts_with("application/octet-stream")
        || kind.starts_with("binary/octet-stream")
}

fn mp4_file_ok(path: &Path) -> bool {
    let mut prefix = [0u8; 32];
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(count) = file.read(&mut prefix) else {
        return false;
    };
    count >= 12 && &prefix[4..8] == b"ftyp"
}

fn remote_video_url(video: &Value) -> String {
    for key in ["resolved_url", "play_url", "url"] {
        let value = text_at(video, &[key]);
        if is_remote_url(&value) && !is_hls_url(&value) {
            return value;
        }
    }
    if let Some(values) = video.get("source_urls").and_then(Value::as_array) {
        for value in values {
            if let Some(url) = value.as_str() {
                if is_remote_url(url) && !is_hls_url(url) {
                    return url.to_string();
                }
            }
        }
    }
    if let Some(values) = video.get("candidates").and_then(Value::as_array) {
        for value in values {
            if let Some(url) = value.get("url").and_then(Value::as_str) {
                if is_remote_url(url) && !is_hls_url(url) {
                    return url.to_string();
                }
            }
        }
    }
    String::new()
}

pub(crate) fn is_byte_range_preview(url: &str) -> bool {
    reqwest::Url::parse(url.trim()).ok().is_some_and(|parsed| {
        parsed.query_pairs().any(|(key, _)| {
            let key = key.to_ascii_lowercase();
            key == "bytestart" || key == "byteend"
        })
    })
}

fn is_remote_url(value: &str) -> bool {
    let value = value.trim();
    value.starts_with("https://") || value.starts_with("http://")
}

fn is_hls_url(value: &str) -> bool {
    reqwest::Url::parse(value.trim())
        .ok()
        .is_some_and(|url| url.path().to_ascii_lowercase().ends_with(".m3u8"))
        || value.to_ascii_lowercase().contains(".m3u8")
}

/// Save playable Instagram Reel/video files through the logged-in browser
/// session before the result is archived. A cover-only fallback remains a
/// preview and is never reported as a successfully saved video.
pub async fn save_site_media(
    page: &PageSession,
    ctx: &ToolContext,
    site_id: &str,
    tool_name: &str,
    result: &mut Value,
) {
    if site_id != "instagram" || tool_name != "postDetail" {
        return;
    }
    let native_id = text_at(result, &["shortcode", "id"]);
    let title = text_at(result, &["caption"]);
    let referer = text_at(result, &["url"]);
    let fallback_video = text_at(result, &["video_url"]);
    let Some(media) = result.get_mut("media").and_then(Value::as_array_mut) else {
        return;
    };
    for (index, item) in media.iter_mut().enumerate() {
        if text_at(item, &["type", "kind"]) != "video" || !text_at(item, &["local_path"]).is_empty()
        {
            continue;
        }
        let mut source = text_at(item, &["url", "src"]);
        if is_byte_range_preview(&source) {
            source.clear();
        }
        if source.is_empty() {
            if is_remote_url(&fallback_video) && !is_byte_range_preview(&fallback_video) {
                source = fallback_video.clone();
            }
        }
        if source.is_empty() {
            item["download_error"] =
                Value::String("playable Instagram video URL was not exposed by the page".into());
            continue;
        }
        let label = format!(
            "instagram_{}_video_{}",
            if native_id.is_empty() {
                "post"
            } else {
                &native_id
            },
            index + 1
        );
        let destination = ctx.next_artifact_path(&label, ".mp4", "artifacts/instagram");
        let fetched = page
            .fetch_file_with_browser(
                &source,
                MAX_INSTAGRAM_VIDEO_BYTES,
                &destination,
                INSTAGRAM_MEDIA_HOST_SUFFIXES,
            )
            .await
            .and_then(|(content_type, _)| {
                validate_instagram_video(&destination, &content_type)?;
                Ok(())
            });
        match fetched {
            Ok(()) => {
                item["local_path"] = Value::String(destination.to_string_lossy().into_owned());
                item["downloaded"] = Value::Bool(true);
                ctx.register_artifact(
                    &destination,
                    &label,
                    "video",
                    if title.is_empty() {
                        "Downloaded Instagram video"
                    } else {
                        &title
                    },
                    json!({ "site": "instagram", "post_id": native_id.clone(), "source_url": source.clone() }),
                    None,
                    tool_name,
                );
            }
            Err(error) => {
                let _ = std::fs::remove_file(&destination);
                item["download_error"] = Value::String(format!("{error:#}"));
                tracing::warn!(post_id = native_id, %referer, %error, "failed to save Instagram video");
            }
        }
    }
}

fn validate_instagram_video(path: &std::path::Path, content_type: &str) -> anyhow::Result<()> {
    let kind = content_type.to_ascii_lowercase();
    if !kind.is_empty()
        && !kind.starts_with("video/mp4")
        && !kind.starts_with("application/mp4")
        && !kind.starts_with("application/octet-stream")
        && !kind.starts_with("binary/octet-stream")
    {
        anyhow::bail!("unexpected Instagram video content type: {content_type}");
    }
    let mut prefix = [0u8; 32];
    let count = std::fs::File::open(path)?.read(&mut prefix)?;
    if count < 12 || &prefix[4..8] != b"ftyp" {
        anyhow::bail!("downloaded Instagram video is not an MP4 container");
    }
    Ok(())
}

/// Return desktop-card records carried by one site tool result.
///
/// This is public so the desktop timeline can derive references from the same
/// mapping used to write `notes.json`; it performs no I/O.
pub fn records_for_site_tool(
    site_id: &str,
    tool_name: &str,
    result: &Value,
) -> Vec<(String, Value)> {
    match site_id {
        "linkedin" => linkedin_records(tool_name, result),
        "instagram" => instagram_records(tool_name, result),
        "dy" | "tiktok" => video_platform_records(site_id, tool_name, result),
        _ => Vec::new(),
    }
}

fn is_content_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "searchResults"
            | "profilePosts"
            | "postDetail"
            | "comments"
            | "videoCards"
            | "videoDetail"
            | "search"
            | "get_videos"
            | "author_scan"
    )
}

fn upsert_record(ctx: &ToolContext, note_id: &str, mut incoming: Value) {
    deduplicate_post_media(&mut incoming);
    if !ctx.update_recorded_note(note_id, |existing| merge_post_record(existing, &incoming)) {
        ctx.record_note(note_id, incoming);
    }
}

/// Merge a later card/detail observation without downgrading data already read.
/// Used both within one run and when the desktop combines conversation runs.
pub fn merge_post_record(existing: &mut Value, incoming: &Value) {
    if !existing.is_object() || !incoming.is_object() {
        if value_is_empty(existing) && !value_is_empty(incoming) {
            *existing = incoming.clone();
        }
        return;
    }
    let Some(source) = incoming.as_object() else {
        return;
    };
    for (key, value) in source {
        if value_is_empty(value) {
            continue;
        }
        let Some(target) = existing.as_object_mut() else {
            return;
        };
        if let Some(current) = target.get_mut(key) {
            merge_record_field(key, current, value);
        } else {
            target.insert(key.clone(), value.clone());
        }
    }
    deduplicate_post_media(existing);
}

fn merge_record_field(key: &str, current: &mut Value, incoming: &Value) {
    if value_is_empty(incoming) {
        return;
    }
    if value_is_empty(current) {
        *current = incoming.clone();
        return;
    }
    match (current, incoming) {
        (Value::Object(target), Value::Object(source)) => {
            for (child_key, child_value) in source {
                if value_is_empty(child_value) {
                    continue;
                }
                if let Some(child) = target.get_mut(child_key) {
                    merge_record_field(child_key, child, child_value);
                } else {
                    target.insert(child_key.clone(), child_value.clone());
                }
            }
        }
        (Value::Array(target), Value::Array(source)) if matches!(key, "comments" | "replies") => {
            merge_comment_arrays(target, source);
        }
        (Value::Array(target), Value::Array(source)) if key == "media" => {
            merge_media(target, source);
        }
        (Value::Array(target), Value::Array(source)) => {
            if source.len() > target.len() {
                *target = source.clone();
            }
        }
        (Value::Bool(target), Value::Bool(source)) if matches!(key, "saved" | "archived") => {
            *target |= source;
        }
        (Value::Number(target), Value::Number(source)) => {
            if source.as_f64().unwrap_or_default() > target.as_f64().unwrap_or_default() {
                *target = source.clone();
            }
        }
        (Value::String(target), Value::String(source)) if key == "level" => {
            if record_level(source) > record_level(target) {
                *target = source.clone();
            }
        }
        (Value::String(target), Value::String(source))
            if matches!(key, "title" | "content" | "excerpt" | "transcript") =>
        {
            if source.chars().count() > target.chars().count() {
                *target = source.clone();
            }
        }
        (target, source) => *target = source.clone(),
    }
}

fn record_level(value: &str) -> u8 {
    match value {
        "deep" => 3,
        "detail" => 2,
        "card" | "preview" => 1,
        _ => 0,
    }
}

fn merge_media(target: &mut Vec<Value>, source: &[Value]) {
    // A search card often archives a reel before its cover exists: a video
    // with no file and no poster. The opened post then has the same video
    // plus the still. Merging those as two items paints a gray slide first.
    let incoming_videos = source
        .iter()
        .filter(|item| text_at(item, &["kind", "type"]) == "video")
        .count();
    let bare_videos = target.iter().filter(|item| bare_video(item)).count();
    for incoming in source {
        let key = media_key(incoming);
        if let Some(existing) = target.iter_mut().find(|item| media_key(item) == key) {
            merge_record_field("media_item", existing, incoming);
            continue;
        }
        if incoming_videos == 1
            && bare_videos == 1
            && text_at(incoming, &["kind", "type"]) == "video"
        {
            if let Some(existing) = target.iter_mut().find(|item| bare_video(item)) {
                merge_record_field("media_item", existing, incoming);
                continue;
            }
        }
        target.push(incoming.clone());
    }
}

fn bare_video(value: &Value) -> bool {
    text_at(value, &["kind", "type"]) == "video"
        && text_at(value, &["src", "url"]).is_empty()
        && text_at(value, &["poster", "poster_url", "poster_local_path"]).is_empty()
}

/// Also normalize historical archives whose repeated observations used different
/// CDN signatures. Preserve the first position and the most recent URL.
pub fn deduplicate_post_media(record: &mut Value) {
    if let Some(media) = record.get_mut("media").and_then(Value::as_array_mut) {
        let original = std::mem::take(media);
        merge_media(media, &original);
    }
}

fn media_key(value: &Value) -> String {
    let kind = text_at(value, &["kind", "type"]);
    let source = text_at(value, &["src", "url"]);
    if kind == "image" {
        if let Ok(url) = reqwest::Url::parse(&source) {
            let host = url.host_str().unwrap_or_default();
            if matches!(url.scheme(), "http" | "https")
                && (host == "cdninstagram.com" || host.ends_with(".cdninstagram.com"))
            {
                // CDN host, resize settings, and signatures vary between reads.
                // The full path identifies the image; different carousel paths
                // must remain separate. Keep the original URL for rendering.
                return format!("image|instagram:{}", url.path());
            }
        }
    }
    format!(
        "{}|{}|{}",
        text_at(value, &["kind", "type"]),
        text_at(value, &["src", "url"]),
        text_at(value, &["poster", "poster_url"])
    )
}

fn merge_comments(record: &mut Value, incoming: &[Value]) {
    let Some(record) = record.as_object_mut() else {
        return;
    };
    let mut merged = record
        .get("comments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    merge_comment_arrays(&mut merged, incoming);
    record.insert("comments".into(), Value::Array(merged));
    record.insert("level".into(), Value::String("deep".into()));
}

fn merge_comment_arrays(target: &mut Vec<Value>, source: &[Value]) {
    for incoming in source {
        let key = comment_key(incoming);
        if let Some(existing) = target.iter_mut().find(|item| comment_key(item) == key) {
            merge_record_field("comment", existing, incoming);
        } else if !key.is_empty() {
            target.push(incoming.clone());
        }
    }
}

fn comment_key(comment: &Value) -> String {
    let id = text_at(comment, &["comment_id", "id"]);
    if !id.is_empty() {
        return format!("id:{id}");
    }
    format!(
        "content:{}|{}",
        text_at(comment, &["author"]),
        text_at(comment, &["text"])
    )
}

fn linkedin_records(tool_name: &str, result: &Value) -> Vec<(String, Value)> {
    match tool_name {
        "searchResults" => result
            .as_array()
            .into_iter()
            .flatten()
            .filter(|item| item.get("kind").and_then(Value::as_str) == Some("post"))
            .filter_map(linkedin_search_record)
            .collect(),
        "postDetail" => linkedin_detail_record(result).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn linkedin_search_record(item: &Value) -> Option<(String, Value)> {
    let native_id = text_at(item, &["id"]);
    let url = text_at(item, &["url"]);
    if native_id.is_empty() || url.is_empty() {
        return None;
    }
    let content = text_at(item, &["snippet", "subtitle"]);
    let author = text_at(item, &["title"]);
    let mut record = base_record(
        "linkedin",
        &native_id,
        &url,
        title_from(&content, &author),
        &content,
    );
    insert_author(&mut record, &author, "", "");
    record.insert("media".into(), Value::Array(media_items(item.get("media"))));
    Some((post_note_id("linkedin", &native_id), Value::Object(record)))
}

fn linkedin_detail_record(item: &Value) -> Option<(String, Value)> {
    if item.get("ok").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let native_id = text_at(item, &["post_id"]);
    let url = text_at(item, &["url"]);
    if native_id.is_empty() || url.is_empty() {
        return None;
    }
    let content = text_at(item, &["text"]);
    let author = item.get("author");
    let author_name = author
        .map(|value| text_at(value, &["name"]))
        .unwrap_or_default();
    let mut record = base_record(
        "linkedin",
        &native_id,
        &url,
        title_from(&content, &author_name),
        &content,
    );
    insert_author(
        &mut record,
        &author_name,
        author
            .map(|value| text_at(value, &["profile_id"]))
            .unwrap_or_default()
            .as_str(),
        author
            .map(|value| text_at(value, &["url"]))
            .unwrap_or_default()
            .as_str(),
    );
    if let Some(stats) = item.get("engagement") {
        insert_stats(
            &mut record,
            parse_metric(&text_at(stats, &["reactions"])),
            None,
            parse_metric(&text_at(stats, &["comments"])),
            None,
        );
    }
    let media = media_items(item.get("media"));
    record.insert(
        "saved".into(),
        Value::Bool(media.iter().any(media_is_local)),
    );
    record.insert("media".into(), Value::Array(media));
    record.insert("level".into(), Value::String("deep".into()));
    Some((post_note_id("linkedin", &native_id), Value::Object(record)))
}

fn instagram_records(tool_name: &str, result: &Value) -> Vec<(String, Value)> {
    match tool_name {
        "searchResults" | "profilePosts" => result
            .as_array()
            .into_iter()
            .flatten()
            .filter(|item| {
                matches!(
                    item.get("kind").and_then(Value::as_str),
                    Some("post" | "reel")
                )
            })
            .filter_map(instagram_card_record)
            .collect(),
        "postDetail" => instagram_detail_record(result).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn instagram_card_record(item: &Value) -> Option<(String, Value)> {
    let native_id = text_at(item, &["shortcode", "id"]);
    let url = text_at(item, &["url"]);
    if native_id.is_empty() || url.is_empty() {
        return None;
    }
    let content = text_at(item, &["title", "media_description", "subtitle"]);
    let mut record = base_record(
        "instagram",
        &native_id,
        &url,
        title_from(&content, "Instagram post"),
        &content,
    );
    let thumbnail = text_at(item, &["thumbnail_url"]);
    let video_url = text_at(item, &["video_url"]);
    let is_video =
        !video_url.is_empty() || item.get("kind").and_then(Value::as_str) == Some("reel");
    let media = if is_video {
        let mut media = Map::new();
        media.insert("kind".into(), Value::String("video".into()));
        media.insert("ratio".into(), Value::String("1:1".into()));
        insert_nonempty(&mut media, "poster", &thumbnail);
        vec![Value::Object(media)]
    } else if !thumbnail.is_empty() {
        vec![json!({ "kind": "image", "src": thumbnail, "ratio": "1:1" })]
    } else {
        Vec::new()
    };
    record.insert("media".into(), Value::Array(media));
    Some((post_note_id("instagram", &native_id), Value::Object(record)))
}

fn instagram_detail_record(item: &Value) -> Option<(String, Value)> {
    if item.get("ok").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let native_id = text_at(item, &["shortcode", "id"]);
    let url = text_at(item, &["url"]);
    if native_id.is_empty() || url.is_empty() {
        return None;
    }
    let content = text_at(item, &["caption"]);
    let author = item.get("author");
    let username = author
        .map(|value| text_at(value, &["username"]))
        .unwrap_or_default();
    let mut record = base_record(
        "instagram",
        &native_id,
        &url,
        title_from(&content, &username),
        &content,
    );
    insert_author(
        &mut record,
        &username,
        &username,
        author
            .map(|value| text_at(value, &["url"]))
            .unwrap_or_default()
            .as_str(),
    );
    insert_posted_at(&mut record, &text_at(item, &["published_at"]));
    if let Some(stats) = item.get("engagement") {
        insert_stats(
            &mut record,
            stats.get("likes").and_then(Value::as_u64),
            None,
            stats.get("comments").and_then(Value::as_u64),
            None,
        );
    }
    let prepared = normalize_instagram_detail_media(item);
    let media = media_items(Some(&prepared));
    record.insert(
        "saved".into(),
        Value::Bool(media.iter().any(media_is_local)),
    );
    record.insert("media".into(), Value::Array(media));
    record.insert("level".into(), Value::String("deep".into()));
    Some((post_note_id("instagram", &native_id), Value::Object(record)))
}

/// Drop unplayable byte-range previews and use the sibling still as the video
/// poster. Otherwise the card leads with an empty gray frame and hides the
/// cover on the next slide.
fn normalize_instagram_detail_media(item: &Value) -> Value {
    let mut media = item
        .get("media")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for entry in &mut media {
        if text_at(entry, &["type", "kind"]) != "video" {
            continue;
        }
        let url = text_at(entry, &["url", "src"]);
        if !is_byte_range_preview(&url) {
            continue;
        }
        if let Some(object) = entry.as_object_mut() {
            object.insert("url".into(), Value::String(String::new()));
            object.insert("src".into(), Value::String(String::new()));
        }
    }
    let fallback = text_at(item, &["video_url"]);
    if is_remote_url(&fallback) && !is_byte_range_preview(&fallback) {
        if let Some(video) = media
            .iter_mut()
            .find(|entry| text_at(entry, &["type", "kind"]) == "video")
        {
            if text_at(video, &["url", "src"]).is_empty() {
                if let Some(object) = video.as_object_mut() {
                    object.insert("url".into(), Value::String(fallback));
                }
            }
        }
    }
    let is_video = |entry: &Value| text_at(entry, &["type", "kind"]) == "video";
    let is_image = |entry: &Value| text_at(entry, &["type", "kind"]) == "image";
    let video_at = media.iter().position(is_video);
    let image_at = media.iter().position(is_image);
    let video_count = media.iter().filter(|entry| is_video(entry)).count();
    let image_count = media.iter().filter(|entry| is_image(entry)).count();
    if video_count == 1 && image_count == 1 {
        if let (Some(video_at), Some(image_at)) = (video_at, image_at) {
            let image_url = text_at(&media[image_at], &["url", "src"]);
            let poster = text_at(
                &media[video_at],
                &["poster_url", "poster", "poster_local_path"],
            );
            if poster.is_empty() && !image_url.is_empty() {
                if let Some(object) = media[video_at].as_object_mut() {
                    object.insert("poster_url".into(), Value::String(image_url));
                }
                media.remove(image_at);
            }
        }
    }
    Value::Array(media)
}

fn video_platform_records(site_id: &str, tool_name: &str, result: &Value) -> Vec<(String, Value)> {
    match tool_name {
        "search" => result
            .get("cards")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| video_card_record(site_id, item))
            .collect(),
        "videoCards" => result
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| video_card_record(site_id, item))
            .collect(),
        "author_scan" => result
            .pointer("/profile/video_cards")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| video_card_record(site_id, item))
            .collect(),
        "get_videos" => result
            .get("videos")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("entity"))
            .filter_map(|item| video_detail_record(site_id, item))
            .collect(),
        "videoDetail" => video_detail_record(site_id, result).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn video_card_record(site_id: &str, item: &Value) -> Option<(String, Value)> {
    let native_id = text_at(item, &["video_id", "id"]);
    let url = text_at(item, &["url"]);
    if native_id.is_empty() || url.is_empty() {
        return None;
    }
    let content = text_at(item, &["title", "description"]);
    let author = text_at(item, &["author"]);
    let mut record = base_record(
        site_id,
        &native_id,
        &url,
        title_from(&content, &author),
        &content,
    );
    insert_author(
        &mut record,
        &author,
        &text_at(item, &["author_id"]),
        &text_at(item, &["author_url"]),
    );
    insert_stats(
        &mut record,
        parse_metric(&text_at(item, &["likes"])),
        None,
        parse_metric(&text_at(item, &["comments", "comments_count"])),
        parse_metric(&text_at(item, &["shares"])),
    );
    let cover = http_url(&text_at(item, &["cover_url"]));
    let media = if cover.is_empty() && text_at(item, &["video_url"]).is_empty() {
        Vec::new()
    } else {
        let mut media = Map::new();
        media.insert("kind".into(), Value::String("video".into()));
        media.insert("ratio".into(), Value::String("9:16".into()));
        insert_nonempty(&mut media, "poster", &cover);
        vec![Value::Object(media)]
    };
    record.insert("media".into(), Value::Array(media));
    Some((post_note_id(site_id, &native_id), Value::Object(record)))
}

fn video_detail_record(site_id: &str, item: &Value) -> Option<(String, Value)> {
    let native_id = text_at(item, &["video_id", "id"]);
    let url = text_at(item, &["url"]);
    if native_id.is_empty() || url.is_empty() {
        return None;
    }
    let content = text_at(item, &["description", "title"]);
    let author = text_at(item, &["author"]);
    let mut record = base_record(
        site_id,
        &native_id,
        &url,
        title_from(&content, &author),
        &content,
    );
    insert_author(
        &mut record,
        &author,
        &text_at(item, &["author_id"]),
        &text_at(item, &["author_url"]),
    );
    insert_posted_at(&mut record, &text_at(item, &["created_at"]));
    insert_stats(
        &mut record,
        parse_metric(&text_at(item, &["likes"])),
        parse_metric(&text_at(item, &["favorites"])),
        parse_metric(&text_at(item, &["comments_count"])),
        parse_metric(&text_at(item, &["shares"])),
    );
    let comments = normalized_comments(site_id, item.get("top_comments").unwrap_or(&Value::Null));
    if !comments.is_empty() {
        record.insert("comments".into(), Value::Array(comments));
    }
    let media = video_media(item);
    record.insert(
        "saved".into(),
        Value::Bool(media.iter().any(media_is_local)),
    );
    record.insert("media".into(), Value::Array(media));
    if let Some(transcript) = item
        .get("video")
        .and_then(|video| video.get("transcript"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        record.insert("transcript".into(), Value::String(transcript.to_string()));
    }
    record.insert("level".into(), Value::String("deep".into()));
    Some((post_note_id(site_id, &native_id), Value::Object(record)))
}

fn base_record(
    site_id: &str,
    native_id: &str,
    url: &str,
    title: String,
    content: &str,
) -> Map<String, Value> {
    let mut record = Map::new();
    record.insert(
        "note_id".into(),
        Value::String(post_note_id(site_id, native_id)),
    );
    record.insert("native_id".into(), Value::String(native_id.to_string()));
    record.insert("site".into(), Value::String(site_id.to_string()));
    record.insert("url".into(), Value::String(url.to_string()));
    record.insert("title".into(), Value::String(title));
    if !content.trim().is_empty() {
        record.insert("content".into(), Value::String(content.trim().to_string()));
        record.insert(
            "excerpt".into(),
            Value::String(truncate_chars(content.trim(), 120)),
        );
    }
    record.insert("media".into(), Value::Array(Vec::new()));
    record.insert("archived".into(), Value::Bool(true));
    record.insert("saved".into(), Value::Bool(false));
    record.insert("level".into(), Value::String("card".into()));
    record
}

fn insert_author(record: &mut Map<String, Value>, name: &str, handle: &str, url: &str) {
    if name.trim().is_empty() && handle.trim().is_empty() && url.trim().is_empty() {
        return;
    }
    let mut author = Map::new();
    insert_nonempty(&mut author, "name", name);
    insert_nonempty(&mut author, "handle", handle);
    insert_nonempty(&mut author, "url", url);
    record.insert("author".into(), Value::Object(author));
}

fn insert_stats(
    record: &mut Map<String, Value>,
    likes: Option<u64>,
    collects: Option<u64>,
    comments: Option<u64>,
    shares: Option<u64>,
) {
    let mut stats = Map::new();
    for (key, value) in [
        ("likes", likes),
        ("collects", collects),
        ("comments", comments),
        ("shares", shares),
    ] {
        if let Some(value) = value {
            stats.insert(key.into(), Value::from(value));
        }
    }
    if !stats.is_empty() {
        record.insert("stats".into(), Value::Object(stats));
    }
}

fn insert_posted_at(record: &mut Map<String, Value>, value: &str) {
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value.trim()) {
        record.insert("posted_at".into(), Value::from(parsed.timestamp_millis()));
    }
}

fn media_items(value: Option<&Value>) -> Vec<Value> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let media_type = text_at(item, &["type", "kind"]);
            if media_type == "document" {
                return None;
            }
            let raw_src = text_at(item, &["local_path", "url", "src"]);
            let poster = text_at(item, &["poster_local_path", "poster_url", "poster"]);
            let kind = if media_type == "video" {
                "video"
            } else {
                "image"
            };
            // Remote video URLs expire and are blocked in the webview. Keep the
            // cover, and let the background downloader fill `src` with a file.
            let remote_video = kind == "video" && is_remote_url(&raw_src);
            let src = if remote_video { String::new() } else { raw_src };
            if src.is_empty() && poster.is_empty() && !remote_video {
                return None;
            }
            let mut media = Map::new();
            media.insert("kind".into(), Value::String(kind.into()));
            insert_nonempty(&mut media, "src", &src);
            insert_nonempty(&mut media, "poster", &poster);
            media.insert(
                "ratio".into(),
                Value::String(if kind == "video" { "9:16" } else { "3:4" }.into()),
            );
            Some(Value::Object(media))
        })
        .collect()
}

fn video_media(item: &Value) -> Vec<Value> {
    let video = item.get("video").unwrap_or(&Value::Null);
    let local_src = text_at(video, &["local_path"]);
    let src = if is_remote_url(&local_src) {
        String::new()
    } else {
        local_src
    };
    let poster = {
        let local = text_at(video, &["poster_local_path"]);
        if local.is_empty() {
            let remote = text_at(video, &["poster_url", "cover_url"]);
            if remote.is_empty() {
                text_at(item, &["cover_url"])
            } else {
                remote
            }
        } else {
            local
        }
    };
    if src.is_empty() && poster.is_empty() && remote_video_url(video).is_empty() {
        return Vec::new();
    }
    vec![json!({
        "kind": "video",
        "src": src,
        "poster": poster,
        "ratio": "9:16",
    })]
}

fn media_is_local(media: &Value) -> bool {
    ["src", "poster"].iter().any(|key| {
        media.get(*key).and_then(Value::as_str).is_some_and(|path| {
            !path.is_empty() && !path.starts_with("http://") && !path.starts_with("https://")
        })
    })
}

fn normalized_comments(site_id: &str, value: &Value) -> Vec<Value> {
    let items = value.as_array().cloned().unwrap_or_default();
    items
        .iter()
        .filter_map(|comment| normalized_comment(site_id, comment))
        .collect()
}

fn normalized_comment(site_id: &str, comment: &Value) -> Option<Value> {
    let text = text_at(comment, &["text"]);
    if text.is_empty() {
        return None;
    }
    let author_value = comment.get("author");
    let author = match author_value {
        Some(Value::Object(_)) => author_value
            .map(|value| text_at(value, &["username", "name"]))
            .unwrap_or_default(),
        _ => text_at(comment, &["author", "username"]),
    };
    let mut normalized = Map::new();
    let comment_id = text_at(comment, &["comment_id", "id"]);
    insert_nonempty(&mut normalized, "comment_id", &comment_id);
    normalized.insert("text".into(), Value::String(text));
    insert_nonempty(&mut normalized, "author", &author);
    let time = text_at(comment, &["published_label", "published_at", "time"]);
    insert_nonempty(&mut normalized, "time", &time);
    if let Some(likes) = parse_metric(&text_at(comment, &["likes", "reactions", "like_count"])) {
        normalized.insert("likes".into(), Value::from(likes));
    }
    let replies = comment
        .get("replies")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|reply| normalized_comment(site_id, reply))
        .collect::<Vec<_>>();
    if !replies.is_empty() {
        normalized.insert("replies".into(), Value::Array(replies));
    }
    normalized.insert("site".into(), Value::String(site_id.to_string()));
    Some(Value::Object(normalized))
}

fn note_identity_from_url(site_id: &str, raw_url: &str) -> Option<(String, String)> {
    let parsed = reqwest::Url::parse(raw_url).ok()?;
    let path = parsed.path();
    let native_id = match site_id {
        "linkedin" => path
            .split("urn:li:activity:")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .or_else(|| {
                path.split("-activity-")
                    .nth(1)
                    .and_then(|rest| rest.split(|ch: char| !ch.is_ascii_digit()).next())
            }),
        "instagram" => path
            .split('/')
            .collect::<Vec<_>>()
            .windows(2)
            .find(|pair| matches!(pair[0], "p" | "reel"))
            .map(|pair| pair[1]),
        "dy" => ["/video/", "/note/"].into_iter().find_map(|marker| {
            path.split(marker)
                .nth(1)
                .and_then(|rest| rest.split('/').next())
        }),
        "tiktok" => path
            .split("/video/")
            .nth(1)
            .and_then(|rest| rest.split('/').next()),
        _ => None,
    }?
    .trim();
    if native_id.is_empty() {
        return None;
    }
    Some((post_note_id(site_id, native_id), raw_url.to_string()))
}

fn post_note_id(site_id: &str, native_id: &str) -> String {
    format!("{site_id}:{native_id}")
}

fn site_name(site_id: &str) -> &'static str {
    match site_id {
        "linkedin" => "LinkedIn",
        "instagram" => "Instagram",
        "dy" => "Douyin",
        "tiktok" => "TikTok",
        _ => "Social",
    }
}

fn title_from(content: &str, fallback: &str) -> String {
    let first = content
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    if first.is_empty() {
        fallback.trim().to_string()
    } else {
        truncate_chars(first, 72)
    }
}

fn truncate_chars(value: &str, max: usize) -> String {
    let mut chars = value.chars();
    let head = chars.by_ref().take(max).collect::<String>();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn parse_metric(value: &str) -> Option<u64> {
    let compact = value.trim().replace(',', "");
    if compact.is_empty() {
        return None;
    }
    let start = compact.find(|ch: char| ch.is_ascii_digit())?;
    let numeric = compact[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>();
    let number = numeric.parse::<f64>().ok()?;
    let suffix = compact[start + numeric.len()..]
        .trim_start()
        .to_ascii_lowercase();
    let multiplier = if suffix.starts_with('k') {
        1_000.0
    } else if suffix.starts_with('m') {
        1_000_000.0
    } else if suffix.starts_with('b') {
        1_000_000_000.0
    } else if suffix.starts_with('万') {
        10_000.0
    } else if suffix.starts_with('亿') {
        100_000_000.0
    } else {
        1.0
    };
    Some((number * multiplier).round() as u64)
}

fn http_url(value: &str) -> String {
    let value = value.trim();
    if value.starts_with("https://") || value.starts_with("http://") {
        value.to_string()
    } else {
        String::new()
    }
}

fn text_at(value: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string()
}

fn insert_nonempty(map: &mut Map<String, Value>, key: &str, value: &str) {
    if !value.trim().is_empty() {
        map.insert(key.into(), Value::String(value.trim().to_string()));
    }
}

fn value_is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(value) => value.trim().is_empty(),
        Value::Array(value) => value.is_empty(),
        Value::Object(value) => value.is_empty(),
        _ => false,
    }
}
