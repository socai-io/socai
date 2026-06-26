//! Artifact helper tools for macro-agent runs.
//!
//! Macro site tools intentionally return compact payloads to keep the LLM
//! context usable, while the full evidence bundle lives under the current
//! run directory and is registered in `RunState`. These tools give the agent a
//! two-level artifact workflow:
//!
//! - `artifact_list`: high-level inventory of saved artifacts and post-like
//!   records without local media paths.
//! - `artifact_read`: focused deep dive into one artifact or note, optionally
//!   inlining selected local media files for vision-capable models.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::run_state::ArtifactRecord;
use crate::agent::tool::{SharedTool, Tool, ToolContext, ToolResult, ToolResultBlock};

const DEFAULT_LIST_LIMIT: usize = 50;
const DEFAULT_READ_MAX_CHARS: usize = 40_000;
const DEFAULT_MAX_MEDIA: usize = 4;
const HARD_MAX_MEDIA: usize = 8;
const MAX_INLINE_MEDIA_BYTES: u64 = 4 * 1024 * 1024;

pub fn artifact_agent_tools() -> Vec<SharedTool> {
    vec![Arc::new(ArtifactListTool), Arc::new(ArtifactReadTool)]
}

pub struct ArtifactListTool;

#[async_trait]
impl Tool for ArtifactListTool {
    fn name(&self) -> &str {
        "artifact_list"
    }

    fn description(&self) -> &str {
        "List saved artifacts from this run and summarize post-like records at a high level. Use this before deep artifact reads to see what evidence is available without opening every full JSON/media file. The high-level post list intentionally omits local media paths; use artifact_read for a focused deep dive."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of high-level posts to return.",
                    "default": DEFAULT_LIST_LIMIT,
                    "minimum": 1
                },
                "include_posts": {
                    "type": "boolean",
                    "description": "Extract high-level note/post summaries from known macro artifacts.",
                    "default": true
                }
            }
        })
    }

    fn always_available(&self) -> bool {
        true
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let limit = input
            .get("limit")
            .and_then(Value::as_u64)
            .map(|n| n.max(1) as usize)
            .unwrap_or(DEFAULT_LIST_LIMIT);
        let include_posts = input
            .get("include_posts")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        let artifacts = artifact_records(ctx);
        let artifact_values: Vec<Value> = artifacts
            .iter()
            .map(|record| artifact_summary(record))
            .collect();

        let mut posts = Vec::new();
        if include_posts {
            for record in &artifacts {
                if posts.len() >= limit {
                    break;
                }
                let Some(payload) = read_artifact_json(ctx, record)? else {
                    continue;
                };
                let remaining = limit.saturating_sub(posts.len());
                posts.extend(high_level_posts(&payload, record, remaining));
            }
        }

        let result = json!({
            "run": {
                "id": ctx.run_id,
            },
            "artifacts": artifact_values,
            "posts": posts,
            "posts_count": posts.len(),
        });
        Ok(json_tool_result(&result))
    }
}

pub struct ArtifactReadTool;

#[async_trait]
impl Tool for ArtifactReadTool {
    fn name(&self) -> &str {
        "artifact_read"
    }

    fn description(&self) -> &str {
        "Read a specific saved artifact or deep-dive into one note/post from a macro artifact. Use `artifact_list` first, then pass an artifact `key`/`path` and optionally `note_id`. Set `include_media=true` only for especially interesting notes when you want to inspect downloaded local media."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "description": "Provide at least one of key, path, or note_id. The tool validates this at runtime.",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "Artifact registry key from artifact_list, e.g. 001_xhs_topic_scan_query."
                },
                "path": {
                    "type": "string",
                    "description": "Registered artifact path from artifact_list, relative to the current run dir."
                },
                "note_id": {
                    "type": "string",
                    "description": "Optional XHS note id to extract from the selected artifact, or from any macro artifact if key/path is omitted."
                },
                "json_pointer": {
                    "type": "string",
                    "description": "Optional RFC 6901 JSON Pointer to read a subsection of the artifact. Ignored when note_id is used."
                },
                "include_media": {
                    "type": "boolean",
                    "description": "Inline selected downloaded image media for the focused note/artifact. Use sparingly after choosing interesting posts.",
                    "default": false
                },
                "max_media": {
                    "type": "integer",
                    "description": "Maximum number of local image media files to inline when include_media=true.",
                    "default": DEFAULT_MAX_MEDIA,
                    "minimum": 1,
                    "maximum": HARD_MAX_MEDIA
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum JSON/text characters returned in the text block.",
                    "default": DEFAULT_READ_MAX_CHARS,
                    "minimum": 1000
                }
            }
        })
    }

    fn always_available(&self) -> bool {
        true
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let max_chars = input
            .get("max_chars")
            .and_then(Value::as_u64)
            .map(|n| n.max(1_000) as usize)
            .unwrap_or(DEFAULT_READ_MAX_CHARS);
        let include_media = input
            .get("include_media")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let max_media = input
            .get("max_media")
            .and_then(Value::as_u64)
            .map(|n| n.max(1) as usize)
            .unwrap_or(DEFAULT_MAX_MEDIA)
            .min(HARD_MAX_MEDIA);
        let note_id = input
            .get("note_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());

        let artifacts = artifact_records(ctx);
        let selected = select_artifact(ctx, &artifacts, &input, note_id)?;
        let Some((record, payload)) = selected else {
            anyhow::bail!("no matching artifact found");
        };

        let mut media_paths = Vec::new();
        let value = if let Some(note_id) = note_id {
            let note = find_note_entry(&payload, note_id)
                .ok_or_else(|| anyhow::anyhow!("note_id not found in artifact: {note_id}"))?;
            if include_media {
                collect_local_media_paths(&note, &mut media_paths);
            }
            json!({
                "artifact": artifact_summary(&record),
                "note": note,
            })
        } else if let Some(pointer) = input
            .get("json_pointer")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let sub = payload.pointer(pointer).ok_or_else(|| {
                anyhow::anyhow!(
                    "json_pointer not found in artifact {}: {pointer}",
                    record.path
                )
            })?;
            if include_media {
                collect_local_media_paths(sub, &mut media_paths);
            }
            json!({
                "artifact": artifact_summary(&record),
                "json_pointer": pointer,
                "value": sub,
            })
        } else {
            if include_media {
                collect_local_media_paths(&payload, &mut media_paths);
            }
            json!({
                "artifact": artifact_summary(&record),
                "value": payload,
            })
        };

        let mut blocks = vec![ToolResultBlock::text(truncate_text(
            &serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
            max_chars,
        ))];
        if include_media {
            append_media_blocks(ctx, &mut blocks, &media_paths, max_media)?;
        }
        Ok(ToolResult::blocks(blocks))
    }
}

fn artifact_records(ctx: &ToolContext) -> Vec<ArtifactRecord> {
    ctx.run_state
        .as_ref()
        .map(|state| state.artifact_records())
        .unwrap_or_default()
}

fn artifact_summary(record: &ArtifactRecord) -> Value {
    json!({
        "key": record.key,
        "path": record.path,
        "label": record.label,
        "kind": record.kind,
        "source_tool": record.source_tool,
        "turn": record.turn,
        "summary": record.summary,
        "metadata": record.metadata,
    })
}

fn read_artifact_json(ctx: &ToolContext, record: &ArtifactRecord) -> anyhow::Result<Option<Value>> {
    let path = safe_run_path(&ctx.run_dir, &record.path)?;
    if !path.is_file() {
        return Ok(None);
    }
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return Ok(None);
    };
    if !ext.eq_ignore_ascii_case("json") {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&text).ok())
}

fn safe_run_path(run_dir: &Path, raw: &str) -> anyhow::Result<PathBuf> {
    let raw_path = PathBuf::from(raw.trim());
    let candidate = if raw_path.is_absolute() {
        raw_path
    } else {
        run_dir.join(raw_path)
    };
    let run_dir = run_dir
        .canonicalize()
        .unwrap_or_else(|_| run_dir.to_path_buf());
    let canonical = candidate.canonicalize().map_err(|err| {
        anyhow::anyhow!("cannot access artifact path {}: {err}", candidate.display())
    })?;
    if !canonical.starts_with(&run_dir) {
        anyhow::bail!("artifact path escapes current run directory: {}", raw);
    }
    Ok(canonical)
}

fn select_artifact(
    ctx: &ToolContext,
    artifacts: &[ArtifactRecord],
    input: &Value,
    note_id: Option<&str>,
) -> anyhow::Result<Option<(ArtifactRecord, Value)>> {
    let key = input
        .get("key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let path = input
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let Some(key) = key {
        let record = artifacts
            .iter()
            .find(|record| record.key == key)
            .ok_or_else(|| anyhow::anyhow!("artifact key not found: {key}"))?;
        let payload = read_artifact_json(ctx, record)?
            .ok_or_else(|| anyhow::anyhow!("artifact is not readable JSON: {}", record.path))?;
        return Ok(Some((record.clone(), payload)));
    }

    if let Some(path) = path {
        let record = artifacts
            .iter()
            .find(|record| record.path == path)
            .ok_or_else(|| {
                anyhow::anyhow!("artifact path is not registered in this run: {path}")
            })?;
        let payload = read_artifact_json(ctx, record)?
            .ok_or_else(|| anyhow::anyhow!("artifact is not readable JSON: {}", record.path))?;
        return Ok(Some((record.clone(), payload)));
    }

    if let Some(note_id) = note_id {
        for record in artifacts {
            let Some(payload) = read_artifact_json(ctx, record)? else {
                continue;
            };
            if find_note_entry(&payload, note_id).is_some() {
                return Ok(Some((record.clone(), payload)));
            }
        }
    }

    Ok(None)
}

fn high_level_posts(payload: &Value, record: &ArtifactRecord, limit: usize) -> Vec<Value> {
    let mut out = Vec::new();
    if let Some(notes) = payload.get("notes").and_then(Value::as_array) {
        for (idx, note) in notes.iter().enumerate().take(limit) {
            if let Some(summary) = note_summary(note, idx, record) {
                out.push(summary);
            }
        }
    }
    out
}

fn note_summary(note: &Value, index: usize, record: &ArtifactRecord) -> Option<Value> {
    let entity = note
        .get("entity")
        .filter(|value| value.is_object())
        .unwrap_or(note);
    let note_id = string_field(entity, "note_id");
    let title = string_field(entity, "title");
    if note_id.is_empty() && title.is_empty() {
        return None;
    }
    let media_counts = media_counts(entity);
    Some(json!({
        "index": index,
        "note_id": empty_to_null(note_id),
        "title": empty_to_null(title),
        "author": empty_to_null(string_field(entity, "author")),
        "author_id": empty_to_null(string_field(entity, "author_id")),
        "url": empty_to_null(string_field(entity, "url")),
        "type": empty_to_null(string_field(entity, "type")),
        "likes": empty_to_null(string_field(entity, "likes")),
        "favorites": empty_to_null(string_field(entity, "favorites")),
        "comments_count": empty_to_null(string_field(entity, "comments_count")),
        "image_count": entity.get("image_count").cloned().unwrap_or(Value::Null),
        "media_count": media_counts.0,
        "local_media_count": media_counts.1,
        "has_local_media": media_counts.1 > 0,
        "artifact_key": record.key,
        "artifact_path": record.path,
    }))
}

fn find_note_entry(payload: &Value, note_id: &str) -> Option<Value> {
    let notes = payload.get("notes").and_then(Value::as_array)?;
    for note in notes {
        let entity = note
            .get("entity")
            .filter(|value| value.is_object())
            .unwrap_or(note);
        if string_field(entity, "note_id") == note_id {
            return Some(note.clone());
        }
    }
    None
}

fn media_counts(value: &Value) -> (usize, usize) {
    let mut total = 0usize;
    let mut local = 0usize;
    if let Some(images) = value.get("images").and_then(Value::as_array) {
        total += images.len();
        local += images
            .iter()
            .filter(|image| !string_field(image, "local_path").is_empty())
            .count();
    }
    if let Some(video) = value.get("video").filter(|video| video.is_object()) {
        if !string_field(video, "url").is_empty() || !string_field(video, "resolved_url").is_empty()
        {
            total += 1;
        }
        if !string_field(video, "local_path").is_empty()
            || !string_field(video, "poster_local_path").is_empty()
        {
            local += 1;
        }
    }
    (total, local)
}

fn collect_local_media_paths(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for key in ["local_path", "poster_local_path", "frame_path"] {
                if let Some(path) = map.get(key).and_then(Value::as_str) {
                    let path = path.trim();
                    if !path.is_empty() && !out.iter().any(|existing| existing == path) {
                        out.push(path.to_string());
                    }
                }
            }
            for value in map.values() {
                collect_local_media_paths(value, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_local_media_paths(item, out);
            }
        }
        _ => {}
    }
}

fn append_media_blocks(
    ctx: &ToolContext,
    blocks: &mut Vec<ToolResultBlock>,
    paths: &[String],
    max_media: usize,
) -> anyhow::Result<()> {
    let mut added = 0usize;
    for raw_path in paths {
        if added >= max_media {
            break;
        }
        let path = match safe_run_path(&ctx.run_dir, raw_path) {
            Ok(path) => path,
            Err(_) => continue,
        };
        let Some(media_type) = image_media_type(&path) else {
            continue;
        };
        let meta = match std::fs::metadata(&path) {
            Ok(meta) if meta.is_file() && meta.len() <= MAX_INLINE_MEDIA_BYTES => meta,
            _ => continue,
        };
        let bytes = std::fs::read(&path)?;
        use base64::Engine as _;
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        blocks.push(ToolResultBlock::text(format!(
            "Media {} ({} bytes)",
            raw_path,
            meta.len()
        )));
        blocks.push(ToolResultBlock::Image {
            data,
            media_type: media_type.to_string(),
        });
        added += 1;
    }
    if added == 0 && !paths.is_empty() {
        blocks.push(ToolResultBlock::text(
            "No supported local image media could be inlined; inspect the listed paths outside the agent if needed.",
        ));
    }
    Ok(())
}

fn image_media_type(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
        Some("webp") => Some("image/webp"),
        Some("gif") => Some("image/gif"),
        _ => None,
    }
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn empty_to_null(value: String) -> Value {
    if value.is_empty() {
        Value::Null
    } else {
        Value::String(value)
    }
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars).collect();
    format!("{kept}\n…[truncated at {max_chars} chars]")
}

fn json_tool_result(value: &Value) -> ToolResult {
    ToolResult::text(serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> ArtifactRecord {
        ArtifactRecord {
            key: "001_scan".into(),
            path: "artifacts/001_scan.json".into(),
            label: "scan".into(),
            kind: "json".into(),
            source_tool: "xhs_search".into(),
            turn: Some(1),
            summary: "scan summary".into(),
            metadata: json!({"site": "xhs"}),
        }
    }

    #[test]
    fn high_level_posts_summarizes_notes_without_media_paths() {
        let payload = json!({
            "notes": [{
                "ok": true,
                "entity": {
                    "note_id": "note1",
                    "title": "Title",
                    "author": "Author",
                    "author_id": "user1",
                    "url": "https://example.test/note1",
                    "likes": "10",
                    "comments_count": "2",
                    "images": [{"local_path": "site_media/note1/0.jpg"}]
                }
            }]
        });

        let posts = high_level_posts(&payload, &record(), 10);

        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0]["note_id"], json!("note1"));
        assert_eq!(posts[0]["local_media_count"], json!(1));
        assert!(serde_json::to_string(&posts[0])
            .unwrap()
            .find("site_media/note1/0.jpg")
            .is_none());
    }

    #[test]
    fn find_note_entry_finds_nested_entity() {
        let payload = json!({"notes": [{"entity": {"note_id": "note1", "title": "Title"}}]});

        let note = find_note_entry(&payload, "note1").expect("note exists");

        assert_eq!(note["entity"]["title"], json!("Title"));
    }

    #[test]
    fn safe_run_path_rejects_outside_paths() {
        let root =
            std::env::temp_dir().join(format!("socai_artifact_tool_test_{}", std::process::id()));
        let run_dir = root.join("run");
        let outside = root.join("outside.json");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("inside.json"), "{}").unwrap();
        std::fs::write(&outside, "{}").unwrap();

        assert!(safe_run_path(&run_dir, "inside.json").is_ok());
        assert!(safe_run_path(&run_dir, outside.to_string_lossy().as_ref()).is_err());

        let _ = std::fs::remove_dir_all(&root);
    }
}
