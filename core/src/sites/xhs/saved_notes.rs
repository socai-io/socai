//! Local source retrieval, shared by the main agent and exploration workers.

use crate::agent::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

pub struct ReadSavedNotesTool;

/// Include preview cards and original entities, not just the UI's lean archive.
/// Only this run's source artifacts are eligible; never scan other runs or media.
pub(crate) fn sources(ctx: &ToolContext) -> BTreeMap<String, Value> {
    sources_from_run(&ctx.run_dir)
}

pub(crate) fn sources_from_run(run_dir: &Path) -> BTreeMap<String, Value> {
    fn collect(value: &Value, out: &mut BTreeMap<String, Value>) {
        match value {
            Value::Object(map) => {
                if let Some(id) = map.get("note_id").and_then(Value::as_str) {
                    if map.contains_key("title") || map.contains_key("content") {
                        let existing = out.entry(id.to_string()).or_insert_with(|| json!({}));
                        for (key, val) in map {
                            if val.is_null() {
                                continue;
                            }
                            let old = &existing[key];
                            // Retain richer text/arrays when a later preview is sparse.
                            let richer = old.is_null()
                                || val
                                    .as_str()
                                    .is_some_and(|s| s.len() > old.as_str().unwrap_or("").len())
                                || val.as_array().is_some_and(|a| {
                                    a.len() > old.as_array().map_or(0, Vec::len)
                                        || (a.len() == old.as_array().map_or(0, Vec::len)
                                            && val.to_string().len() > old.to_string().len())
                                });
                            if richer {
                                existing[key] = val.clone();
                            }
                        }
                    }
                }
                for (key, val) in map {
                    if key != "media" {
                        collect(val, out);
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    collect(item, out);
                }
            }
            _ => {}
        }
    }
    fn visit(path: &Path, depth: usize, out: &mut BTreeMap<String, Value>) {
        if depth > 10 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir()
                && !matches!(
                    name.as_str(),
                    "llm" | "stats" | "site_media" | "outputs" | "snapshots"
                )
            {
                visit(&entry.path(), depth + 1, out);
            } else if kind.is_file()
                && path.file_name().is_some_and(|n| n == "artifacts")
                && entry.path().extension().is_some_and(|e| e == "json")
            {
                if let Ok(bytes) = std::fs::read(entry.path()) {
                    if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                        collect(&value, out);
                    }
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    for note in crate::agent::note_store::load_notes(run_dir) {
        collect(&note, &mut out);
    }
    visit(&run_dir.join("tools"), 0, &mut out);
    visit(&run_dir.join("artifacts"), 0, &mut out);
    out
}

fn field<'a>(v: &'a Value, names: &[&str]) -> &'a str {
    names
        .iter()
        .find_map(|name| v.get(name).and_then(Value::as_str))
        .unwrap_or("")
}

pub(crate) fn render_note(id: &str, v: &Value, section: &str) -> String {
    let mut out = format!(
        "## {} · {}\n{}\n",
        field(v, &["title"]),
        id,
        field(v, &["url", "link"])
    );
    let author = v
        .get("author")
        .map(|a| {
            a.as_str()
                .map(str::to_string)
                .unwrap_or_else(|| a.to_string())
        })
        .unwrap_or_default();
    out.push_str(&format!(
        "作者：{author}\n主页：{}\n日期：{}\n",
        field(v, &["author_url"]),
        field(v, &["date"])
    ));
    if section == "all" || section == "body" {
        out.push_str(&format!("\n正文\n{}\n", field(v, &["content", "desc"])));
        if let Some(text) = v
            .pointer("/video/transcript")
            .and_then(Value::as_str)
            .or_else(|| v.get("transcript").and_then(Value::as_str))
        {
            out.push_str(&format!("\n视频文字\n{text}\n"));
        }
    }
    if section == "all" || section == "ocr" {
        out.push_str("\n图片文字（已有采集，不重新 OCR）\n");
        if let Some(images) = v.get("images").and_then(Value::as_array) {
            for (index, image) in images.iter().enumerate() {
                if let Some(text) = image.get("ocr_text").and_then(Value::as_str) {
                    out.push_str(&format!("\n图 {}\n{text}\n", index + 1));
                }
            }
        } else if let Some(text) = v.get("ocr_text") {
            out.push_str(&format!("{text}\n"));
        }
        if let Some(text) = v.pointer("/video/poster_ocr").and_then(Value::as_str) {
            out.push_str(text);
        }
    }
    if section == "all" || section == "comments" {
        out.push_str("\n评论（仅已采集部分）\n");
        fn comments(v: &Value, out: &mut String) {
            if let Some(items) = v.as_array() {
                for item in items {
                    if let Some(text) = item.as_str() {
                        out.push_str(&format!("- {text}\n"));
                    } else {
                        let author = item
                            .get("author")
                            .and_then(|a| {
                                a.as_str().or_else(|| a.get("name").and_then(Value::as_str))
                            })
                            .unwrap_or("");
                        out.push_str(&format!(
                            "- {author}：{}\n",
                            field(item, &["text", "content"])
                        ));
                        for key in ["replies", "sub_comments"] {
                            comments(&item[key], out);
                        }
                    }
                }
            }
        }
        comments(
            v.get("top_comments")
                .or_else(|| v.get("comments"))
                .unwrap_or(&Value::Null),
            &mut out,
        );
    }
    out
}

#[async_trait]
impl Tool for ReadSavedNotesTool {
    fn available_in_local_delivery(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "read_saved_notes"
    }
    fn description(&self) -> &str {
        "Read this run's saved XHS sources by note ID: body, full image OCR and collected comments, without browser access or shell. Use section=links for a compact ID/title/original URL index when writing reports, without body/OCR. Omit note_ids to list saved IDs/titles (including previews). Returns readable text with character pagination; use offset to continue. Preview-only notes do not contain a full body."
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{
            "note_ids":{"type":"array","items":{"type":"string"}},
            "section":{"type":"string","enum":["all","body","ocr","comments","links"],"default":"all"},
            "offset":{"type":"integer","minimum":0,"default":0},
            "max_chars":{"type":"integer","minimum":1000,"maximum":30000,"default":12000}
        }})
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let sources = sources(ctx);
        let ids: Vec<_> = input["note_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let section = input["section"].as_str().unwrap_or("all");
        let text = if ids.is_empty() && section == "links" {
            sources
                .iter()
                .map(|(id, note)| render_note(id, note, "links"))
                .collect::<Vec<_>>()
                .join("\n")
        } else if ids.is_empty() {
            sources
                .iter()
                .map(|(id, n)| {
                    format!(
                        "{id} · {} · {}\n",
                        field(n, &["title"]),
                        if field(n, &["content"]).is_empty() {
                            "预览或正文为空"
                        } else {
                            "已存正文"
                        }
                    )
                })
                .collect::<String>()
        } else {
            ids.iter()
                .map(|id| {
                    sources
                        .get(*id)
                        .map(|v| render_note(id, v, section))
                        .unwrap_or_else(|| format!("未在本次 run 找到 {id}。\n"))
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let offset = input["offset"].as_u64().unwrap_or(0) as usize;
        let limit = input["max_chars"]
            .as_u64()
            .unwrap_or(12000)
            .clamp(1000, 30000) as usize;
        let total = text.chars().count();
        let page: String = text.chars().skip(offset).take(limit).collect();
        let end = offset.saturating_add(page.chars().count()).min(total);
        let next = if end < total {
            format!("继续读取：offset={end}，其余参数不变。")
        } else {
            "本页已到所选材料末尾。".into()
        };
        Ok(ToolResult::text(format!(
            "已存材料，字符 {offset}–{end}/{total}。{next}\n\n{page}"
        )))
    }
}
