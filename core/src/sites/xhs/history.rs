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
    /// True once any past read had media (vision) enabled.
    #[serde(default)]
    pub include_media: bool,
    /// True once any past read downloaded the note's media (images/video carry
    /// a `local_path`).
    #[serde(default)]
    pub downloaded: bool,
    /// True once any past read ran OCR (the entity carries `ocr_text`).
    #[serde(default)]
    pub ocr: bool,
    /// True once any past read attached a non-empty video audio transcript.
    #[serde(default)]
    pub transcribed: bool,
    /// Most comments (primary + replies) ever cached for this note. Lets a later
    /// scan asking for more comments than we have re-read instead of short-
    /// circuiting on the stale, smaller set.
    #[serde(default)]
    pub comments_loaded: u32,
    /// The note's own total comment count (from `comments_count`, includes
    /// replies) as last seen. When `comments_loaded` reaches this we've captured
    /// everything, so a bigger request is still satisfied. `0` when unknown.
    #[serde(default)]
    pub comments_total: u32,
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

    /// True when a prior analysis already covers what's being requested: recorded
    /// level is >= requested AND every requested enrichment (vision, downloaded
    /// media, OCR) was present in a prior read. This is what lets a repeated
    /// search/scan reuse the cached entity instead of re-opening, re-downloading,
    /// and re-OCR'ing the same note.
    ///
    /// A download request is additionally checked against the disk: cached
    /// `local_path`s point into the run dir that downloaded them, and run dirs
    /// are user-deletable (deleting a task wipes them). Once any cached media
    /// file is gone the request is not satisfied, so the note is re-read and
    /// re-downloaded into the current run instead of being archived media-less
    /// on every future reuse.
    pub fn is_satisfied_by(
        &self,
        note_id: &str,
        level: &str,
        include_media: bool,
        download_media: bool,
        ocr: bool,
        transcribe_audio: bool,
        min_comments: i64,
    ) -> bool {
        let Some(prev) = self.get(note_id) else {
            return false;
        };
        if level_value(&prev.level) < level_value(level) {
            return false;
        }
        if include_media && !prev.include_media {
            return false;
        }
        if download_media {
            if !prev.downloaded {
                return false;
            }
            if let Some(entity) = prev.entity.as_ref() {
                if !entity_media_intact(entity) {
                    return false;
                }
            }
        }
        if ocr && !prev.ocr {
            return false;
        }
        if transcribe_audio && !prev.transcribed {
            return false;
        }
        // A request for more comments than we cached forces a re-read — unless we
        // already captured the whole thread (loaded >= the note's own total), in
        // which case there is nothing more to fetch.
        if min_comments > 0
            && (prev.comments_loaded as i64) < min_comments
            && (prev.comments_total == 0 || prev.comments_loaded < prev.comments_total)
        {
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
            // Infer the downloaded/OCR flags from the entity itself so a reused
            // note is only short-circuited when the cache actually covers what a
            // later run asks for.
            if entity_has_downloaded(entity) {
                entry.downloaded = true;
            }
            if entity_has_ocr(entity) {
                entry.ocr = true;
            }
            if entity_has_transcript(entity) {
                entry.transcribed = true;
            }
            // Track comment coverage (primary + replies) so a later, larger
            // request re-reads instead of reusing a smaller cached set.
            let loaded = entity_comment_count(entity);
            if loaded > entry.comments_loaded {
                entry.comments_loaded = loaded;
            }
            let total = entity_comment_total(entity);
            if total > entry.comments_total {
                entry.comments_total = total;
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

    /// Forget downloaded media that lived under run dirs being deleted: strip
    /// the cached entities' `local_path`s pointing into them and downgrade the
    /// `downloaded` flag, so the next request re-reads (and re-downloads) the
    /// note instead of trusting paths that no longer exist. Returns how many
    /// entries were scrubbed. Best-effort hygiene: a concurrently running
    /// agent holds its own store instance and its next record can clobber
    /// this write — the disk check in `is_satisfied_by` is the backstop.
    pub fn scrub_media_under(&self, run_dirs: &[PathBuf]) -> usize {
        let (snapshot, scrubbed) = {
            let Ok(mut guard) = self.inner.lock() else {
                return 0;
            };
            let mut scrubbed = 0usize;
            for entry in guard.notes.values_mut() {
                let Some(entity) = entry.entity.as_mut() else {
                    continue;
                };
                if !scrub_entity_media_under(entity, run_dirs) {
                    continue;
                }
                entry.downloaded = entity_has_downloaded(entity);
                scrubbed += 1;
            }
            if scrubbed == 0 {
                return 0;
            }
            (guard.clone(), scrubbed)
        };
        let _ = save_file(&self.path, &snapshot);
        scrubbed
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

/// Count the comments cached on an entity: primary comments plus their replies.
/// `top_comments` may be full objects (with `sub_comments`) or already-leaned
/// (a string, or `{text, replies}`), so cover both shapes.
fn entity_comment_count(entity: &Value) -> u32 {
    let Some(comments) = entity.get("top_comments").and_then(Value::as_array) else {
        return 0;
    };
    let mut count = 0u32;
    for comment in comments {
        count += 1;
        let replies = comment
            .get("sub_comments")
            .or_else(|| comment.get("replies"))
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        count += replies as u32;
    }
    count
}

/// Parse the note's own total comment count from `comments_count` (e.g. "170",
/// "1.2万", "3,456"). `0` when absent or unparseable.
fn entity_comment_total(entity: &Value) -> u32 {
    let raw = entity
        .get("comments_count")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if raw.is_empty() {
        return 0;
    }
    let cleaned: String = raw
        .chars()
        .filter(|c| !matches!(c, ',' | '+' | ' '))
        .collect();
    let digits: String = cleaned
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let Ok(base) = digits.parse::<f64>() else {
        return 0;
    };
    let mult = if cleaned.contains('万') || cleaned.contains('w') || cleaned.contains('W') {
        10_000.0
    } else if cleaned.contains('k') || cleaned.contains('K') {
        1_000.0
    } else {
        1.0
    };
    (base * mult).round() as u32
}

/// True when the entity carries OCR output: a non-empty note-level `ocr_text`
/// array, any image with a non-empty per-image `ocr_text`, or a video with a
/// non-empty `poster_ocr` (the video-note cover OCR).
fn entity_has_ocr(entity: &Value) -> bool {
    let note_level = entity
        .get("ocr_text")
        .and_then(Value::as_array)
        .is_some_and(|arr| {
            arr.iter()
                .any(|v| v.as_str().is_some_and(|s| !s.trim().is_empty()))
        });
    if note_level {
        return true;
    }
    let poster = entity
        .get("video")
        .and_then(|video| video.get("poster_ocr"))
        .and_then(Value::as_str)
        .is_some_and(|s| !s.trim().is_empty());
    if poster {
        return true;
    }
    entity
        .get("images")
        .and_then(Value::as_array)
        .is_some_and(|imgs| {
            imgs.iter().any(|im| {
                im.get("ocr_text")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.trim().is_empty())
            })
        })
}

/// True when the entity's media was downloaded to disk (an image or the video
/// carries a non-empty `local_path`).
fn entity_has_downloaded(entity: &Value) -> bool {
    let has_local = |v: &Value| {
        v.get("local_path")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
    };
    let image_local = entity
        .get("images")
        .and_then(Value::as_array)
        .is_some_and(|imgs| imgs.iter().any(has_local));
    image_local || entity.get("video").is_some_and(has_local)
}

/// The on-disk media files a cached entity claims: image `local_path`s plus
/// the video's `local_path` / `poster_local_path` — the same fields the media
/// manifest validates and the app renders. Empty fields are skipped (they
/// mean "never downloaded", not "downloaded then lost").
fn entity_media_paths(entity: &Value) -> Vec<&str> {
    fn trimmed_path(value: Option<&Value>) -> Option<&str> {
        value
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|path| !path.is_empty())
    }
    let mut paths = Vec::new();
    if let Some(images) = entity.get("images").and_then(Value::as_array) {
        for image in images {
            paths.extend(trimmed_path(image.get("local_path")));
        }
    }
    if let Some(video) = entity.get("video") {
        paths.extend(trimmed_path(video.get("local_path")));
        paths.extend(trimmed_path(video.get("poster_local_path")));
    }
    paths
}

/// True while every media file the entity claims still is a non-empty file on
/// disk — the same bar the media manifest uses to call an asset "downloaded".
/// Cached paths are absolute (they point into the run dir that downloaded
/// them); a path we can't verify fails the check.
fn entity_media_intact(entity: &Value) -> bool {
    entity_media_paths(entity).into_iter().all(|path| {
        let path = Path::new(path);
        path.is_absolute() && fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() > 0)
    })
}

/// Remove the entity's image/video `local_path`-style fields that point under
/// any of `run_dirs`. True when anything was removed.
fn scrub_entity_media_under(entity: &mut Value, run_dirs: &[PathBuf]) -> bool {
    fn scrub_field(owner: &mut Value, key: &str, run_dirs: &[PathBuf]) -> bool {
        let hit = owner.get(key).and_then(Value::as_str).is_some_and(|path| {
            let path = Path::new(path.trim());
            run_dirs.iter().any(|dir| path.starts_with(dir))
        });
        if hit {
            if let Some(map) = owner.as_object_mut() {
                map.remove(key);
            }
        }
        hit
    }
    let mut changed = false;
    if let Some(images) = entity.get_mut("images").and_then(Value::as_array_mut) {
        for image in images {
            changed |= scrub_field(image, "local_path", run_dirs);
        }
    }
    if let Some(video) = entity.get_mut("video") {
        changed |= scrub_field(video, "local_path", run_dirs);
        changed |= scrub_field(video, "poster_local_path", run_dirs);
    }
    changed
}

fn entity_has_transcript(entity: &Value) -> bool {
    entity
        .get("video")
        .and_then(|video| video.get("transcript"))
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty())
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

// AGENTS.md: do NOT add new Rust tests unless the user explicitly asks. Update
// the existing ones when an API they cover changes; don't grow this module.
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

        assert!(store.is_satisfied_by("n1", "card", false, false, false, false, 0));
        assert!(store.is_satisfied_by("n1", "lite", false, false, false, false, 0));
        assert!(!store.is_satisfied_by("n1", "deep", false, false, false, false, 0));
        assert!(!store.is_satisfied_by("n1", "lite", true, false, false, false, 0));
        assert!(!store.is_satisfied_by("unknown", "card", false, false, false, false, 0));
        // download / ocr dimensions: a plain read doesn't satisfy them.
        assert!(!store.is_satisfied_by("n1", "lite", false, true, false, false, 0));
        assert!(!store.is_satisfied_by("n1", "lite", false, false, true, false, 0));
        assert!(!store.is_satisfied_by("n1", "lite", false, false, false, true, 0));
    }

    #[test]
    fn satisfied_tracks_downloaded_and_ocr_from_entity() {
        let dir = tempdir().unwrap();
        let store = XhsHistoryStore::open(dir.path().join("h.json"));
        let media = dir.path().join("x.jpg");
        std::fs::write(&media, b"jpg").unwrap();
        store.record(
            &json!({
                "note_id": "n2",
                "ocr_text": ["cover text", ""],
                "images": [{"url": "u", "local_path": media.to_string_lossy(), "ocr_text": "cover text"}],
            }),
            "deep",
            false,
        );

        assert!(store.is_satisfied_by("n2", "deep", false, true, true, false, 0));
        let entry = store.get("n2").unwrap();
        assert!(entry.downloaded);
        assert!(entry.ocr);

        // Deleting the downloaded file voids download satisfaction (the note
        // must be re-read, not resurrected with a dead path) — while text-only
        // coverage is untouched.
        std::fs::remove_file(&media).unwrap();
        assert!(!store.is_satisfied_by("n2", "deep", false, true, true, false, 0));
        assert!(store.is_satisfied_by("n2", "deep", false, false, true, false, 0));
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
