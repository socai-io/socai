//! Stable media manifest generation for XHS searches.
//!
//! `search --download-media` keeps the large per-asset manifest in a
//! run-dir JSON file so command stdout can stay compact while callers still get
//! a stable path to a machine-readable media registry.

use std::path::{Path, PathBuf};

use crate::agent::ToolContext;
use serde_json::{json, Value};

use super::entities::XhsNoteCard;

pub(super) fn write_media_manifest_file(
    ctx: &ToolContext,
    manifest: &Value,
) -> std::io::Result<String> {
    std::fs::create_dir_all(ctx.output_dir())?;
    let path = ctx.output_dir().join("media_manifest.json");
    let rendered = serde_json::to_string_pretty(manifest).map_err(std::io::Error::other)?;
    std::fs::write(&path, rendered)?;
    let _ = ctx.register_artifact(
        &path,
        "media_manifest",
        "json",
        "Search media manifest",
        json!({"site": "xhs", "category": "media_manifest"}),
        Some(manifest),
        "search",
    );
    Ok(path.to_string_lossy().to_string())
}

pub(super) fn search_media_manifest(notes: &[Value], run_dir: &Path) -> Value {
    let mut assets = Vec::new();
    for note in notes {
        collect_note_media_assets(note, run_dir, &mut assets);
    }
    Value::Array(assets)
}

fn collect_note_media_assets(note_entry: &Value, run_dir: &Path, assets: &mut Vec<Value>) {
    if note_entry.get("ok").and_then(Value::as_bool) != Some(true) {
        return;
    }
    let Some(entity) = note_entry.get("entity").filter(|v| v.is_object()) else {
        return;
    };
    if entity
        .get("stale_warning")
        .and_then(Value::as_str)
        .is_some_and(|warning| !warning.trim().is_empty())
    {
        return;
    }
    let note_id = string_field_value(entity, "note_id");
    if note_id.is_empty() {
        return;
    }
    let note_type = string_field_value(entity, "type");

    if let Some(images) = entity.get("images").and_then(Value::as_array) {
        for (fallback_index, image) in images.iter().enumerate() {
            if image.is_object() {
                assets.push(image_manifest_entry(
                    &note_id,
                    image,
                    fallback_index as i64,
                    run_dir,
                ));
            }
        }
    }

    let Some(video) = entity.get("video").filter(|v| v.is_object()) else {
        return;
    };
    if is_video_manifest_candidate(video, &note_type) {
        assets.push(video_manifest_entry(&note_id, video, run_dir));
    }
    if let Some(poster) = poster_manifest_entry(&note_id, video, run_dir) {
        assets.push(poster);
    }
}

pub(super) fn ensure_entity_note_id(entity: &mut Value, card: &XhsNoteCard) {
    if !string_field_value(entity, "note_id").is_empty() {
        return;
    }
    let fallback_note_id = note_id_fallback(entity, card);
    if fallback_note_id.is_empty() {
        return;
    }
    let Some(map) = entity.as_object_mut() else {
        return;
    };
    map.insert("note_id".into(), Value::String(fallback_note_id));
}

/// Preserve trustworthy search-card identity/count fields when the detail DOM
/// is still partially hydrated. Detail values win whenever they are present.
pub(super) fn fill_entity_from_card(entity: &mut Value, card: &XhsNoteCard) {
    let Some(map) = entity.as_object_mut() else {
        return;
    };
    for (key, fallback) in [
        ("title", card.title.as_str()),
        ("author", card.author.as_str()),
        ("author_id", card.author_id.as_str()),
        ("author_url", card.author_url.as_str()),
        ("likes", card.likes.as_str()),
        ("type", card.r#type.as_str()),
    ] {
        let current = map.get(key).and_then(Value::as_str).unwrap_or("").trim();
        if current.is_empty() && !fallback.trim().is_empty() {
            map.insert(key.into(), Value::String(fallback.trim().to_string()));
        }
    }
}

fn note_id_fallback(entity: &Value, card: &XhsNoteCard) -> String {
    let card_note_id = card.note_id.trim();
    if !card_note_id.is_empty() {
        return card_note_id.to_string();
    }
    note_id_from_url(&string_field_value(entity, "url"))
        .or_else(|| note_id_from_url(&string_field_value(entity, "link")))
        .or_else(|| note_id_from_url(&card.link))
        .unwrap_or_default()
}

fn note_id_from_url(value: &str) -> Option<String> {
    let path = value.trim().split(['?', '#']).next().unwrap_or("").trim();
    let segments: Vec<&str> = path
        .split('/')
        .filter(|segment| !segment.trim().is_empty())
        .collect();
    for window in segments.windows(2) {
        if matches!(window[0], "explore" | "discovery" | "search_result") {
            let note_id = window[1].trim();
            if !note_id.is_empty() {
                return Some(note_id.to_string());
            }
        }
    }
    None
}

fn image_manifest_entry(
    note_id: &str,
    image: &Value,
    fallback_index: i64,
    run_dir: &Path,
) -> Value {
    let index = integer_field(image, "index").unwrap_or(fallback_index);
    let source_url = string_field_value(image, "url");
    let local_path = string_field_value(image, "local_path");
    let download_error = first_string_field(image, &["download_error", "save_error"]);
    let local_file = local_path_buf(run_dir, &local_path);
    let (status, error) = download_status_and_error(
        &local_path,
        local_file.as_deref(),
        &source_url,
        download_error,
    );
    let (width, height) = image_dimensions_or_fields(local_file.as_deref(), image);

    json!({
        "note_id": note_id,
        "type": "image",
        "role": "image",
        "index": index,
        "local_path": string_or_null(&local_path),
        "size_bytes": file_size_or_null(local_file.as_deref()),
        "width": option_i64_or_null(width),
        "height": option_i64_or_null(height),
        "duration_s": Value::Null,
        "codec": Value::Null,
        "source_url": string_or_null(&source_url),
        "resolved_url_status": direct_resolution_status(&source_url),
        "download_status": status,
        "download_error": option_string_or_null(error.as_deref()),
        "download_ms": option_i64_or_null(integer_field(image, "download_ms")),
    })
}

fn video_manifest_entry(note_id: &str, video: &Value, run_dir: &Path) -> Value {
    let source_url = video_source_url(video);
    let local_path = string_field_value(video, "local_path");
    let download_error = first_string_field(video, &["download_error", "save_error"]);
    let local_file = local_path_buf(run_dir, &local_path);
    let (status, error) = download_status_and_error(
        &local_path,
        local_file.as_deref(),
        &source_url,
        download_error,
    );
    let size_bytes = file_size(local_file.as_deref());

    json!({
        "note_id": note_id,
        "type": "video",
        "role": "video",
        "index": Value::Null,
        "local_path": string_or_null(&local_path),
        "size_bytes": option_u64_or_null(size_bytes),
        "width": option_i64_or_null(integer_field(video, "width")),
        "height": option_i64_or_null(integer_field(video, "height")),
        "duration_s": option_f64_or_null(f64_field(video, "duration_s")),
        "codec": string_or_null(&string_field_value(video, "codec")),
        "source_url": string_or_null(&source_url),
        "resolved_url_status": video_resolution_status(video, &source_url),
        "download_status": status,
        "download_error": option_string_or_null(error.as_deref()),
        "download_ms": option_i64_or_null(integer_field(video, "download_ms")),
    })
}

fn poster_manifest_entry(note_id: &str, video: &Value, run_dir: &Path) -> Option<Value> {
    let source_url = string_field_value(video, "poster_url");
    let local_path = string_field_value(video, "poster_local_path");
    let download_error = first_string_field(video, &["poster_download_error", "poster_save_error"]);
    if source_url.is_empty() && local_path.is_empty() && download_error.is_none() {
        return None;
    }
    let local_file = local_path_buf(run_dir, &local_path);
    let (status, error) = download_status_and_error(
        &local_path,
        local_file.as_deref(),
        &source_url,
        download_error,
    );
    let (width, height) = image_dimensions_or_fields(local_file.as_deref(), &Value::Null);

    Some(json!({
        "note_id": note_id,
        "type": "image",
        "index": Value::Null,
        "role": "video_poster",
        "local_path": string_or_null(&local_path),
        "size_bytes": file_size_or_null(local_file.as_deref()),
        "width": option_i64_or_null(width),
        "height": option_i64_or_null(height),
        "duration_s": Value::Null,
        "codec": Value::Null,
        "source_url": string_or_null(&source_url),
        "resolved_url_status": direct_resolution_status(&source_url),
        "download_status": status,
        "download_error": option_string_or_null(error.as_deref()),
        "download_ms": option_i64_or_null(integer_field(video, "poster_download_ms")),
    }))
}

fn is_video_manifest_candidate(video: &Value, note_type: &str) -> bool {
    note_type == "video"
        || !string_field_value(video, "local_path").is_empty()
        || first_string_field(video, &["download_error", "save_error"]).is_some()
        || has_video_source_candidate(video)
}

fn download_status_and_error(
    local_path: &str,
    local_file: Option<&Path>,
    source_url: &str,
    download_error: Option<String>,
) -> (&'static str, Option<String>) {
    if let Some(path) = local_file {
        match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_file() && metadata.len() > 0 => {
                return ("downloaded", None)
            }
            Ok(metadata) if metadata.is_file() && !local_path.trim().is_empty() => {
                return ("failed", Some(format!("local file is empty: {local_path}")));
            }
            _ => {}
        }
    }
    if !local_path.trim().is_empty() {
        return (
            "failed",
            Some(format!("local file is missing or unreadable: {local_path}")),
        );
    }
    if let Some(error) = download_error.filter(|s| !s.trim().is_empty()) {
        return ("failed", Some(error));
    }
    if source_url.trim().is_empty() {
        return ("failed", Some("source URL is empty".to_string()));
    }
    (
        "failed",
        Some("download did not produce local_path".to_string()),
    )
}

fn image_dimensions_or_fields(
    local_file: Option<&Path>,
    source: &Value,
) -> (Option<i64>, Option<i64>) {
    if let Some(path) = local_file {
        if let Ok((width, height)) = image::image_dimensions(path) {
            return (Some(i64::from(width)), Some(i64::from(height)));
        }
    }
    (
        integer_field(source, "width"),
        integer_field(source, "height"),
    )
}

fn file_size(local_file: Option<&Path>) -> Option<u64> {
    local_file
        .and_then(|path| std::fs::metadata(path).ok())
        .map(|metadata| metadata.len())
}

fn file_size_or_null(local_file: Option<&Path>) -> Value {
    option_u64_or_null(file_size(local_file))
}

fn local_path_buf(run_dir: &Path, local_path: &str) -> Option<PathBuf> {
    let trimmed = local_path.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        return Some(path);
    }
    let run_dir_path = run_dir.join(&path);
    Some(if run_dir_path.exists() {
        run_dir_path
    } else if path.exists() {
        path
    } else {
        run_dir_path
    })
}

fn string_field_value(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

fn first_string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        let candidate = string_field_value(value, key);
        (!candidate.is_empty()).then_some(candidate)
    })
}

fn integer_field(value: &Value, key: &str) -> Option<i64> {
    let value = value.get(key)?;
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|n| i64::try_from(n).ok()))
        .or_else(|| {
            value
                .as_f64()
                .filter(|n| n.is_finite())
                .map(|n| n.round() as i64)
        })
}

fn f64_field(value: &Value, key: &str) -> Option<f64> {
    value
        .get(key)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
}

fn string_or_null(value: &str) -> Value {
    if value.trim().is_empty() {
        Value::Null
    } else {
        json!(value)
    }
}

fn option_string_or_null(value: Option<&str>) -> Value {
    value.map(string_or_null).unwrap_or(Value::Null)
}

fn option_i64_or_null(value: Option<i64>) -> Value {
    value.map(|n| json!(n)).unwrap_or(Value::Null)
}

fn option_u64_or_null(value: Option<u64>) -> Value {
    value.map(|n| json!(n)).unwrap_or(Value::Null)
}

fn option_f64_or_null(value: Option<f64>) -> Value {
    value.map(|n| json!(n)).unwrap_or(Value::Null)
}

fn direct_resolution_status(source_url: &str) -> &'static str {
    if source_url.trim().is_empty() {
        "missing"
    } else {
        "resolved"
    }
}

fn video_resolution_status(video: &Value, source_url: &str) -> &'static str {
    if !source_url.trim().is_empty() {
        "resolved"
    } else if has_video_source_candidate(video) {
        "unresolved"
    } else {
        "missing"
    }
}

fn video_source_url(video: &Value) -> String {
    for key in ["resolved_url", "master_url", "url"] {
        if let Some(url) = video
            .get(key)
            .and_then(Value::as_str)
            .and_then(clean_media_url)
        {
            return url;
        }
    }

    for key in ["source_urls", "backup_urls"] {
        if let Some(arr) = video.get(key).and_then(Value::as_array) {
            for item in arr {
                if let Some(url) = item.as_str().and_then(clean_media_url) {
                    return url;
                }
            }
        }
    }

    if let Some(candidates) = video.get("candidates").and_then(Value::as_array) {
        for item in candidates {
            if let Some(url) = item
                .get("url")
                .and_then(Value::as_str)
                .and_then(clean_media_url)
            {
                return url;
            }
        }
    }

    String::new()
}

fn clean_media_url(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
        && !trimmed.starts_with("blob:")
    {
        Some(trimmed.to_string())
    } else {
        None
    }
}

fn has_video_source_candidate(video: &Value) -> bool {
    for key in ["resolved_url", "master_url", "url"] {
        if !string_field_value(video, key).is_empty() {
            return true;
        }
    }
    for key in ["source_urls", "backup_urls"] {
        if video
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item.as_str().is_some_and(|s| !s.trim().is_empty()))
            })
        {
            return true;
        }
    }
    video
        .get("candidates")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("url")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.trim().is_empty())
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_entity_note_id_uses_card_and_url_fallbacks_when_missing() {
        let mut missing = json!({ "note_id": "", "images": [] });
        ensure_entity_note_id(
            &mut missing,
            &XhsNoteCard {
                note_id: "card-note".into(),
                ..Default::default()
            },
        );
        assert_eq!(missing["note_id"], "card-note");

        let mut from_entity_url = json!({
            "note_id": "",
            "url": "https://www.xiaohongshu.com/explore/url-note?xsec_token=1",
        });
        ensure_entity_note_id(&mut from_entity_url, &XhsNoteCard::default());
        assert_eq!(from_entity_url["note_id"], "url-note");

        let mut from_card_link = json!({ "note_id": "" });
        ensure_entity_note_id(
            &mut from_card_link,
            &XhsNoteCard {
                link: "https://www.xiaohongshu.com/search_result/card-link-note".into(),
                ..Default::default()
            },
        );
        assert_eq!(from_card_link["note_id"], "card-link-note");

        let mut existing = json!({ "note_id": "entity-note", "images": [] });
        ensure_entity_note_id(
            &mut existing,
            &XhsNoteCard {
                note_id: "card-note".into(),
                ..Default::default()
            },
        );
        assert_eq!(existing["note_id"], "entity-note");
    }

    #[test]
    fn media_manifest_note_id_from_url_rejects_query_only_urls() {
        assert_eq!(
            note_id_from_url("https://www.xiaohongshu.com/explore/real-note?xsec_token=1"),
            Some("real-note".to_string())
        );
        assert_eq!(
            note_id_from_url("https://www.xiaohongshu.com/discovery/real-discovery#comments"),
            Some("real-discovery".to_string())
        );
        assert_eq!(
            note_id_from_url("/search_result/real-search?x=1"),
            Some("real-search".to_string())
        );
        assert_eq!(
            note_id_from_url("https://www.xiaohongshu.com/search_result?keyword=abc"),
            None
        );
        assert_eq!(note_id_from_url("/explore?x=1"), None);
        assert_eq!(note_id_from_url("/discovery#hash"), None);
    }

    #[test]
    fn media_manifest_local_path_buf_prefers_run_dir_relative_path() {
        let cwd = std::env::current_dir().unwrap();
        let cwd_dir = tempfile::Builder::new()
            .prefix("media-manifest-shadow-")
            .tempdir_in(&cwd)
            .unwrap();
        let recorded_path = cwd_dir.path().strip_prefix(&cwd).unwrap().join("asset.bin");
        std::fs::write(&recorded_path, b"cwd").unwrap();

        let run_dir = tempfile::tempdir().unwrap();
        let run_dir_asset = run_dir.path().join(&recorded_path);
        std::fs::create_dir_all(run_dir_asset.parent().unwrap()).unwrap();
        std::fs::write(&run_dir_asset, b"run-dir").unwrap();

        let resolved =
            local_path_buf(run_dir.path(), &recorded_path.to_string_lossy()).expect("path");

        assert_eq!(resolved, run_dir_asset);
        assert_eq!(file_size(Some(&resolved)), Some(7));
    }

    #[test]
    fn local_path_buf_prefers_existing_relative_path_as_recorded() {
        let cwd = std::env::current_dir().unwrap();
        let dir = tempfile::Builder::new()
            .prefix("media-manifest-relative-")
            .tempdir_in(&cwd)
            .unwrap();
        let file_path = dir.path().join("asset.bin");
        std::fs::write(&file_path, b"asset").unwrap();
        let recorded_path = file_path.strip_prefix(&cwd).unwrap();
        let unrelated_run_dir = tempfile::tempdir().unwrap();

        let resolved = local_path_buf(unrelated_run_dir.path(), &recorded_path.to_string_lossy())
            .expect("path");

        assert_eq!(resolved, recorded_path);
        assert_eq!(file_size(Some(&resolved)), Some(5));
    }

    #[test]
    fn media_manifest_skips_unsuccessful_stale_skipped_and_unidentified_entries() {
        let dir = tempfile::tempdir().unwrap();
        let notes = vec![
            json!({
                "ok": false,
                "entity": {
                    "note_id": "failed-note",
                    "type": "image",
                    "images": [{ "url": "https://img.example/failed.jpg", "index": 0 }],
                }
            }),
            json!({
                "ok": true,
                "entity": {
                    "note_id": "stale-note",
                    "type": "image",
                    "stale_warning": "This note was already extracted in the previous read.",
                    "images": [{ "url": "https://img.example/stale.jpg", "index": 0 }],
                }
            }),
            json!({
                "skipped": { "reason": "already_processed" },
                "entity": {
                    "note_id": "skipped-note",
                    "type": "image",
                    "images": [{ "url": "https://img.example/skipped.jpg", "index": 0 }],
                }
            }),
            json!({
                "ok": true,
                "entity": {
                    "note_id": "",
                    "type": "image",
                    "images": [{ "url": "https://img.example/unidentified.jpg", "index": 0 }],
                }
            }),
        ];

        let manifest = search_media_manifest(&notes, dir.path());

        assert_eq!(manifest.as_array().unwrap().len(), 0);
    }

    #[test]
    fn media_manifest_marks_zero_byte_local_file_failed() {
        let dir = tempfile::tempdir().unwrap();
        let image_path = dir.path().join("empty.jpg");
        std::fs::File::create(&image_path).unwrap();
        let notes = vec![json!({
            "ok": true,
            "entity": {
                "note_id": "empty-image",
                "type": "image",
                "images": [{
                    "url": "https://img.example/empty.jpg",
                    "index": 0,
                    "local_path": image_path.to_string_lossy(),
                }],
                "video": {},
            }
        })];

        let manifest = search_media_manifest(&notes, dir.path());
        let assets = manifest.as_array().unwrap();

        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0]["download_status"], "failed");
        assert_eq!(assets[0]["size_bytes"], 0);
        assert_eq!(
            assets[0]["download_error"],
            format!("local file is empty: {}", image_path.to_string_lossy())
        );
    }

    #[test]
    fn media_manifest_records_downloaded_image_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let image_path = dir.path().join("image.png");
        let image = image::RgbImage::from_pixel(2, 3, image::Rgb([1, 2, 3]));
        image.save(&image_path).unwrap();
        let size = std::fs::metadata(&image_path).unwrap().len();
        let notes = vec![json!({
            "ok": true,
            "entity": {
                "note_id": "note-image",
                "type": "image",
                "images": [{
                    "url": "https://img.example/1.jpg",
                    "index": 0,
                    "local_path": image_path.to_string_lossy(),
                }],
                "video": {},
            }
        })];

        let manifest = search_media_manifest(&notes, dir.path());
        let assets = manifest.as_array().unwrap();

        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0]["note_id"], "note-image");
        assert_eq!(assets[0]["type"], "image");
        assert_eq!(assets[0]["index"], 0);
        assert_eq!(
            assets[0]["local_path"],
            image_path.to_string_lossy().as_ref()
        );
        assert_eq!(assets[0]["size_bytes"], size);
        assert_eq!(assets[0]["width"], 2);
        assert_eq!(assets[0]["height"], 3);
        assert_eq!(assets[0]["source_url"], "https://img.example/1.jpg");
        assert_eq!(assets[0]["resolved_url_status"], "resolved");
        assert_eq!(assets[0]["download_status"], "downloaded");
        assert!(assets[0]["download_error"].is_null());
    }

    #[test]
    fn media_manifest_records_video_failure_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let notes = vec![json!({
            "ok": true,
            "entity": {
                "note_id": "note-video",
                "type": "video",
                "video": {
                    "url": "blob:https://www.xiaohongshu.com/not-downloadable",
                    "duration_s": 42.5,
                    "width": 1080,
                    "height": 1920,
                    "codec": "h264",
                    "download_error": "downloadable video URL not found (blob: URLs cannot be downloaded)",
                }
            }
        })];

        let manifest = search_media_manifest(&notes, dir.path());
        let assets = manifest.as_array().unwrap();

        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0]["note_id"], "note-video");
        assert_eq!(assets[0]["type"], "video");
        assert!(assets[0]["local_path"].is_null());
        assert!(assets[0]["source_url"].is_null());
        assert_eq!(assets[0]["resolved_url_status"], "unresolved");
        assert_eq!(assets[0]["download_status"], "failed");
        assert_eq!(
            assets[0]["download_error"],
            "downloadable video URL not found (blob: URLs cannot be downloaded)"
        );
        assert_eq!(assets[0]["duration_s"], 42.5);
        assert_eq!(assets[0]["width"], 1080);
        assert_eq!(assets[0]["height"], 1920);
        assert_eq!(assets[0]["codec"], "h264");
    }

    #[test]
    fn media_manifest_writes_stable_run_dir_file() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new("run", dir.path());
        let manifest = json!([{ "note_id": "note-1", "type": "image" }]);

        let path = write_media_manifest_file(&ctx, &manifest).unwrap();
        let expected = dir.path().join("media_manifest.json");
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();

        assert_eq!(Path::new(&path), expected.as_path());
        assert_eq!(saved, manifest);
    }
}
