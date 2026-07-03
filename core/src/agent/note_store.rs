//! Per-run archive of the notes the agent actually saw.
//!
//! As site tools fully read a note, they record it (full content + resolved
//! local media) into `<run_dir>/notes.json` — a `{ "<note_id>": <record> }`
//! map. This is the local, re-fetch-free source the desktop app renders as
//! rich embedded note cards in the run timeline and the final answer.
//!
//! The record *shape* is site-specific and built by the site's tools (see
//! `sites/xhs/tools.rs`); this module only owns the file path, persistence,
//! and load. Records are keyed by note id, so re-reading a note overwrites the
//! prior entry (latest read wins) and the archive stays deduped.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Path to a run's note archive: `<run_dir>/notes.json`.
pub fn notes_path(run_dir: &Path) -> PathBuf {
    run_dir.join("notes.json")
}

/// Persist the full `note_id -> record` map to `<run_dir>/notes.json`.
///
/// Rewrites the whole file each call; the archive is small (tens of notes per
/// run) and recording is sequential within a run, so this stays cheap.
pub(crate) fn write_notes(run_dir: &Path, notes: &BTreeMap<String, Value>) -> std::io::Result<()> {
    std::fs::create_dir_all(run_dir)?;
    let map: Map<String, Value> = notes.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let rendered =
        serde_json::to_string_pretty(&Value::Object(map)).map_err(std::io::Error::other)?;
    // The desktop app polls this file mid-run to show notes as they are
    // recorded, so go through a temp file + rename: readers must never see a
    // half-written JSON.
    let path = notes_path(run_dir);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, rendered)?;
    // Windows can't rename over an existing file; a reader hitting the gap
    // sees a missing file, which load_notes treats as "no notes yet".
    #[cfg(windows)]
    let _ = std::fs::remove_file(&path);
    std::fs::rename(&tmp, &path)
}

/// Load a run's recorded notes as an array of records (empty when the run
/// recorded none or hasn't scanned yet). A missing file is normal and silent;
/// a file that exists but fails to parse degrades to an empty list with a
/// warning — never an error. The array form is tolerated alongside the
/// canonical object map.
pub fn load_notes(run_dir: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(notes_path(run_dir)) else {
        return Vec::new();
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => map.into_iter().map(|(_, v)| v).collect(),
        Ok(Value::Array(items)) => items,
        Ok(_) => {
            tracing::warn!(
                path = %notes_path(run_dir).display(),
                "notes.json is not an object map or array; ignoring"
            );
            Vec::new()
        }
        Err(err) => {
            tracing::warn!(
                error = %err,
                path = %notes_path(run_dir).display(),
                "failed to parse notes.json; ignoring"
            );
            Vec::new()
        }
    }
}
