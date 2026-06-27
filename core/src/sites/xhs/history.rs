//! Cross-run XHS analysis history. Tracks which notes have already been
//! analyzed (and at what level) in a project-local JSON file so the agent
//! can skip repeats across separate runs.
//!
//! - In-run dedup still lives on `ToolContext::processed_notes`; this store
//!   only handles cross-run persistence.
//! - Besides the lookup metadata (`note_id`, `title`, `author`, `url`, `level`,
//!   `include_media`, `analysis_count`, `first_seen_at`, `last_seen_at`) we also
//!   cache the full last-read `entity` (body + comments + images + location), so
//!   a reused note returns its complete data instead of degrading to the bare
//!   search card. Run dirs and artifact paths are dropped — they live in the
//!   per-run logs and would mostly point at stale paths anyway.
//! - File at `~/.socai/xhs/history.json` (overridable via `SOCAI_HOME`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub note_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub url: String,
    /// Deepest level ever recorded: "card" | "lite" | "deep".
    #[serde(default)]
    pub level: String,
    /// True once any past read had media enabled.
    #[serde(default)]
    pub include_media: bool,
    #[serde(default)]
    pub analysis_count: u32,
    #[serde(default)]
    pub first_seen_at: String,
    #[serde(default)]
    pub last_seen_at: String,
    /// Full last-read entity (body, comments, images, location, …) so a reused
    /// note can be returned complete without re-opening it. `None` for entries
    /// written before this field existed or recorded from a card only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HistoryFile {
    #[serde(default)]
    notes: BTreeMap<String, HistoryEntry>,
}

pub struct XhsHistoryStore {
    path: PathBuf,
    inner: Mutex<HistoryFile>,
}

impl XhsHistoryStore {
    /// `$SOCAI_HOME/xhs/history.json`, else `~/.socai/xhs/history.json`,
    /// else `.socai/xhs/history.json` relative to cwd.
    pub fn default_path() -> PathBuf {
        if let Ok(env) = std::env::var("SOCAI_HOME") {
            return PathBuf::from(env).join("xhs/history.json");
        }
        if let Some(home) = dirs::home_dir() {
            return home.join(".socai/xhs/history.json");
        }
        PathBuf::from(".socai/xhs/history.json")
    }

    pub fn open_default() -> Self {
        Self::open(Self::default_path())
    }

    pub fn open(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let inner = load_file(&path).unwrap_or_default();
        Self {
            path,
            inner: Mutex::new(inner),
        }
    }

    pub fn get(&self, note_id: &str) -> Option<HistoryEntry> {
        let id = note_id.trim();
        if id.is_empty() {
            return None;
        }
        let guard = self.inner.lock().ok()?;
        guard.notes.get(id).cloned()
    }

    /// True when we have the full cached entity for a note (so a reuse can
    /// return complete data). False for pre-upgrade entries that only stored
    /// lookup metadata — those should be re-read to backfill the cache.
    /// Cheap: checks presence without cloning the entity.
    pub fn has_cached_entity(&self, note_id: &str) -> bool {
        let id = note_id.trim();
        if id.is_empty() {
            return false;
        }
        let Ok(guard) = self.inner.lock() else {
            return false;
        };
        guard
            .notes
            .get(id)
            .is_some_and(|entry| entry.entity.is_some())
    }

    /// True when a prior analysis already covers what's being requested:
    /// recorded level is >= requested AND, if media was requested, media
    /// was included previously.
    pub fn is_satisfied_by(&self, note_id: &str, level: &str, include_media: bool) -> bool {
        let Some(prev) = self.get(note_id) else {
            return false;
        };
        if level_value(&prev.level) < level_value(level) {
            return false;
        }
        if include_media && !prev.include_media {
            return false;
        }
        true
    }

    /// Add `already_analyzed` / `history_level` / `history_include_media`
    /// flags onto any card whose `note_id` is in the store. Mutates in place.
    pub fn annotate_cards(&self, cards: &mut Value) {
        let Ok(guard) = self.inner.lock() else {
            return;
        };
        annotate_cards_from(&guard.notes, cards);
    }

    /// Take an owned snapshot of all entries currently in the store. Use
    /// this when a tool mutates history during its own call (e.g.
    /// `search` records every note it reads) but still wants to
    /// annotate output cards based on what was known *before* the call —
    /// otherwise the annotation reflects this run's own writes.
    pub fn snapshot(&self) -> HistorySnapshot {
        let entries = self
            .inner
            .lock()
            .map(|guard| guard.notes.clone())
            .unwrap_or_default();
        HistorySnapshot { entries }
    }

    /// Upsert an entry after a successful read. Never downgrades the
    /// recorded level or media flag — once a note was read deeply, that
    /// stays.
    pub fn record(&self, entity: &Value, level: &str, include_media: bool) {
        let Some(note_id) = entity
            .get("note_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
        else {
            return;
        };
        let now = Utc::now().to_rfc3339();
        let title = string_field(entity, "title");
        let author = string_field(entity, "author");
        let url = string_field(entity, "url");

        let snapshot = {
            let Ok(mut guard) = self.inner.lock() else {
                return;
            };
            let entry = guard.notes.entry(note_id.clone()).or_insert_with(|| {
                let mut e = HistoryEntry::default();
                e.note_id = note_id.clone();
                e.first_seen_at = now.clone();
                e
            });
            entry.note_id = note_id;
            if !title.is_empty() {
                entry.title = title;
            }
            if !author.is_empty() {
                entry.author = author;
            }
            if !url.is_empty() {
                entry.url = url;
            }
            if level_value(level) > level_value(&entry.level) {
                entry.level = level.to_string();
            }
            if include_media {
                entry.include_media = true;
            }
            // Cache the full entity so a later reuse returns complete data.
            entry.entity = Some(entity.clone());
            entry.analysis_count = entry.analysis_count.saturating_add(1);
            entry.last_seen_at = now;
            guard.clone()
        };

        // Best-effort write. A failure here just means the next process
        // won't see this entry — agent still works.
        let _ = save_file(&self.path, &snapshot);
    }
}

/// Owned snapshot of the history at a point in time. Cheap to pass around
/// since it's a plain map.
pub struct HistorySnapshot {
    entries: BTreeMap<String, HistoryEntry>,
}

impl HistorySnapshot {
    pub fn annotate_cards(&self, cards: &mut Value) {
        annotate_cards_from(&self.entries, cards);
    }
}

fn annotate_cards_from(entries: &BTreeMap<String, HistoryEntry>, cards: &mut Value) {
    let Some(arr) = cards.as_array_mut() else {
        return;
    };
    for card in arr {
        let Some(map) = card.as_object_mut() else {
            continue;
        };
        let note_id = map
            .get("note_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let Some(note_id) = note_id else { continue };
        if let Some(entry) = entries.get(&note_id) {
            map.insert("already_analyzed".into(), json!(true));
            map.insert("history_level".into(), json!(entry.level));
            map.insert("history_include_media".into(), json!(entry.include_media));
        }
    }
}

fn level_value(level: &str) -> i32 {
    match level.trim().to_ascii_lowercase().as_str() {
        "deep" => 3,
        "lite" => 2,
        "card" => 1,
        _ => 0,
    }
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn load_file(path: &Path) -> Option<HistoryFile> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn save_file(path: &Path, data: &HistoryFile) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(data)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn records_and_recalls_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.json");
        let store = XhsHistoryStore::open(&path);

        store.record(
            &json!({"note_id": "abc", "title": "T", "author": "A", "url": "u"}),
            "lite",
            false,
        );
        let entry = store.get("abc").expect("entry present");
        assert_eq!(entry.note_id, "abc");
        assert_eq!(entry.level, "lite");
        assert_eq!(entry.analysis_count, 1);
        assert!(!entry.first_seen_at.is_empty());

        // Reopen from disk — entries persist.
        let store2 = XhsHistoryStore::open(&path);
        assert!(store2.get("abc").is_some());
    }

    #[test]
    fn level_never_downgrades_but_media_upgrades() {
        let dir = tempdir().unwrap();
        let store = XhsHistoryStore::open(dir.path().join("h.json"));

        store.record(&json!({"note_id": "n1"}), "deep", true);
        store.record(&json!({"note_id": "n1"}), "lite", false);
        let entry = store.get("n1").unwrap();
        assert_eq!(entry.level, "deep");
        assert!(entry.include_media);
        assert_eq!(entry.analysis_count, 2);
    }

    #[test]
    fn satisfied_when_prior_is_deeper_or_equal() {
        let dir = tempdir().unwrap();
        let store = XhsHistoryStore::open(dir.path().join("h.json"));
        store.record(&json!({"note_id": "n1"}), "lite", false);

        assert!(store.is_satisfied_by("n1", "card", false));
        assert!(store.is_satisfied_by("n1", "lite", false));
        assert!(!store.is_satisfied_by("n1", "deep", false));
        assert!(!store.is_satisfied_by("n1", "lite", true));
        assert!(!store.is_satisfied_by("unknown", "card", false));
    }

    #[test]
    fn snapshot_freezes_pre_call_state() {
        let dir = tempdir().unwrap();
        let store = XhsHistoryStore::open(dir.path().join("h.json"));
        store.record(&json!({"note_id": "old"}), "lite", false);

        let pre = store.snapshot();
        // Writes after the snapshot must not show up when annotating with it.
        store.record(&json!({"note_id": "new_this_run"}), "deep", true);

        let mut cards = json!([
            {"note_id": "old"},
            {"note_id": "new_this_run"},
        ]);
        pre.annotate_cards(&mut cards);
        let arr = cards.as_array().unwrap();
        assert_eq!(arr[0]["already_analyzed"], json!(true));
        assert!(arr[1].get("already_analyzed").is_none());
    }

    #[test]
    fn annotate_cards_marks_known_notes() {
        let dir = tempdir().unwrap();
        let store = XhsHistoryStore::open(dir.path().join("h.json"));
        store.record(&json!({"note_id": "seen", "title": "x"}), "deep", true);

        let mut cards = json!([
            {"note_id": "seen", "title": "x"},
            {"note_id": "fresh", "title": "y"},
        ]);
        store.annotate_cards(&mut cards);
        let arr = cards.as_array().unwrap();
        assert_eq!(arr[0]["already_analyzed"], json!(true));
        assert_eq!(arr[0]["history_level"], json!("deep"));
        assert_eq!(arr[0]["history_include_media"], json!(true));
        assert!(arr[1].get("already_analyzed").is_none());
    }
}
