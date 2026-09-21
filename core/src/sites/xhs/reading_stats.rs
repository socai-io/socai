//! Observed reading coverage, independent of model-authored claims.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ReadingStats {
    /// Disjoint from detail_notes: cards/previews without a saved detail read.
    pub preview_notes: usize,
    pub detail_notes: usize,
    /// Actual collected comments including replies, not public comment counters.
    pub comments: usize,
    pub authors: usize,
}

pub fn for_run(run_dir: &Path) -> Option<ReadingStats> {
    let run: Value = serde_json::from_slice(&std::fs::read(run_dir.join("run.json")).ok()?).ok()?;
    if matches!(run["status"].as_str(), Some("running" | "queued")) {
        return None;
    }
    let stamp = |name: &str| {
        std::fs::metadata(run_dir.join(name))
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|t| t.as_nanos().to_string())
            .unwrap_or_default()
    };
    let signature = format!("v2:{}:{}", stamp("run.json"), stamp("notes.json"));
    let cache_path = run_dir.join("reading-stats.json");
    if let Ok(bytes) = std::fs::read(&cache_path) {
        if let Ok(cache) = serde_json::from_slice::<Value>(&bytes) {
            if cache["signature"].as_str() == Some(&signature) {
                return serde_json::from_value(cache["stats"].clone()).ok();
            }
        }
    }
    let sources = super::saved_notes::sources_from_run(run_dir);
    let details: BTreeSet<_> = crate::agent::note_store::load_notes(run_dir)
        .into_iter()
        .filter(|n| n["level"] == "deep")
        .filter_map(|n| n["note_id"].as_str().map(str::to_string))
        .filter(|id| sources.get(id).is_some_and(|n| n.get("content").is_some()))
        .collect();
    let mut exposed = BTreeSet::new();
    collect_returned_ids(&run_dir.join("tools"), 0, &mut exposed);
    let mut authors = BTreeSet::new();
    collect_profiles(&run_dir.join("tools"), 0, &mut authors);
    collect_profiles(&run_dir.join("artifacts"), 0, &mut authors);
    let mut comments = BTreeSet::new();
    for (id, note) in &sources {
        comment_ids(
            id,
            note.get("top_comments")
                .or_else(|| note.get("comments"))
                .unwrap_or(&Value::Null),
            &mut comments,
        );
    }
    let stats = ReadingStats {
        preview_notes: exposed.difference(&details).count(),
        detail_notes: details.len(),
        comments: comments.len(),
        authors: authors.len(),
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(&json!({"signature":signature,"stats":stats})) {
        let tmp = cache_path.with_extension("json.tmp");
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(tmp, cache_path);
        }
    }
    Some(stats)
}

fn comment_ids(note_id: &str, value: &Value, out: &mut BTreeSet<String>) {
    let Some(items) = value.as_array() else {
        return;
    };
    for c in items {
        let text = c["text"]
            .as_str()
            .or_else(|| c["content"].as_str())
            .or_else(|| c.as_str())
            .unwrap_or("");
        if !text.trim().is_empty() {
            let id = c["comment_id"]
                .as_str()
                .or_else(|| c["id"].as_str())
                .map(str::to_string)
                .unwrap_or_else(|| format!("{}:{text}", c.get("author").unwrap_or(&Value::Null)));
            out.insert(format!("{note_id}:{id}"));
        }
        for key in ["replies", "sub_comments"] {
            comment_ids(note_id, &c[key], out);
        }
    }
}

fn collect_profiles(path: &Path, depth: usize, out: &mut BTreeSet<String>) {
    if depth > 10 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for e in entries.flatten() {
        let Ok(kind) = e.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir()
            && !matches!(
                e.file_name().to_str(),
                Some("llm" | "stats" | "site_media" | "outputs")
            )
        {
            collect_profiles(&e.path(), depth + 1, out);
        } else if kind.is_file()
            && path.file_name().is_some_and(|n| n == "artifacts")
            && e.path().extension().is_some_and(|e| e == "json")
        {
            if let Ok(bytes) = std::fs::read(e.path()) {
                if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                    profile_ids(&v, out);
                }
            }
        }
    }
}

fn profile_ids(v: &Value, out: &mut BTreeSet<String>) {
    if let Some(profile) = v.get("profile").filter(|p| p.is_object()) {
        if profile.get("display_name").is_some() || profile.get("bio").is_some() {
            if let Some(id) = v["author_id"]
                .as_str()
                .or_else(|| profile["author_id"].as_str())
                .filter(|s| !s.is_empty())
            {
                out.insert(id.to_string());
            }
        }
    }
    match v {
        Value::Array(items) => {
            for item in items {
                profile_ids(item, out);
            }
        }
        Value::Object(m) => {
            for (key, item) in m {
                if key != "media" {
                    profile_ids(item, out);
                }
            }
        }
        _ => {}
    }
}

/// Full scan artifacts contain extra search cards removed before model delivery.
/// Count only returned cards, including the candidate reader's nested outputs.
fn collect_returned_ids(path: &Path, depth: usize, out: &mut BTreeSet<String>) {
    if depth > 10 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir()
            && !matches!(
                entry.file_name().to_str(),
                Some("llm" | "artifacts" | "stats" | "site_media" | "outputs")
            )
        {
            collect_returned_ids(&entry.path(), depth + 1, out);
        } else if kind.is_file()
            && matches!(
                entry.file_name().to_str(),
                Some("output.json" | "profile.txt" | "result.txt")
            )
        {
            if let Ok(bytes) = std::fs::read(entry.path()) {
                if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                    returned_ids(&value, out);
                }
            }
        }
    }
}

fn returned_ids(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                returned_ids(item, out);
            }
        }
        Value::Object(map) => {
            if let Some(id) = map.get("note_id").and_then(Value::as_str) {
                if map.contains_key("title") || map.contains_key("content") {
                    out.insert(id.to_string());
                }
            }
            for (key, v) in map {
                if key == "text" {
                    if let Some(text) = v.as_str() {
                        if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                            returned_ids(&parsed, out);
                        }
                    }
                } else if !matches!(key.as_str(), "media" | "artifact") {
                    returned_ids(v, out);
                }
            }
        }
        _ => {}
    }
}
