//! Per-run archive of the notes the agent actually saw.
//!
//! As site tools fully read a note, they record it (full content + resolved
//! local media) into `<run_dir>/notes.json` — an array of records in the
//! order they were first recorded (for `search`, result order). This is the
//! local, re-fetch-free source the desktop app renders as rich embedded note
//! cards in the run timeline and the final answer; keeping recorded order on
//! disk is what lets the mid-run live strip and the finished result row show
//! the same order. Older runs wrote a `{ "<note_id>": <record> }` map, which
//! still loads (in id order).
//!
//! The record *shape* is site-specific and built by the site's tools (see
//! `sites/xhs/tools.rs`); this module only owns the file path, persistence,
//! and load. Entries are deduped by note id upstream (`ToolContext::
//! record_note`): re-reading a note overwrites the prior record but keeps its
//! position.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Path to a run's note archive: `<run_dir>/notes.json`.
pub fn notes_path(run_dir: &Path) -> PathBuf {
    run_dir.join("notes.json")
}

/// Persist the note records to `<run_dir>/notes.json` as an array, in the
/// order given (first-recorded order). Each record is expected to carry its
/// own `note_id`; the entry id is injected into object records that omit it.
/// A non-object record has nowhere to hold an id and is written verbatim.
///
/// Rewrites the whole file each call; the archive is small (tens of notes per
/// run) and recording is sequential within a run, so this stays cheap.
pub(crate) fn write_notes(run_dir: &Path, notes: &[(String, Value)]) -> std::io::Result<()> {
    std::fs::create_dir_all(run_dir)?;
    let items: Vec<Value> = notes
        .iter()
        .map(|(id, record)| {
            let mut record = record.clone();
            if let Some(map) = record.as_object_mut() {
                map.entry("note_id".to_string())
                    .or_insert_with(|| Value::String(id.clone()));
            }
            record
        })
        .collect();
    let rendered =
        serde_json::to_string_pretty(&Value::Array(items)).map_err(std::io::Error::other)?;
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
/// warning — never an error. The legacy object-map form (older runs) is
/// tolerated alongside the canonical array; it loads in id order since the
/// map never carried recorded order.
pub fn load_notes(run_dir: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(notes_path(run_dir)) else {
        return Vec::new();
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => {
            // Sort explicitly: map iteration is only key-ordered while
            // serde_json's `preserve_order` feature stays off, and feature
            // unification anywhere in the dependency graph could flip it.
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            entries.into_iter().map(|(_, v)| v).collect()
        }
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
