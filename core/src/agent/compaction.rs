//! Shared text/JSON compaction helpers used by agent history and run-state
//! evidence summaries.
//!
//! These are used in two places:
//! - Agent loop history (keep tool_result bodies bounded so context doesn't
//!   blow up over many turns).
//! - RunState (compact entity-like payloads when summarizing
//!   evidence for working_memory.md).

use serde_json::{Map, Value};

pub const TOOL_RESULT_TEXT_MAX_CHARS: usize = 10_000;
pub const ASSISTANT_TEXT_MAX_CHARS: usize = 320;

/// Trim a string to at most `max_chars` characters, suffixing
/// `... [truncated]` when the original was longer. Char-based, not byte-based,
/// to keep UTF-8 safe.
pub fn truncate(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    let count = trimmed.chars().count();
    if count <= max_chars {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max_chars).collect();
    out.push_str("... [truncated]");
    out
}

/// Like [`truncate`] but tailored for tool_result bodies (longer ceiling,
/// "..." suffix on its own line).
pub fn truncate_result(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push_str("\n... [truncated]");
    out
}

/// Reorder + truncate a JSON value so the most "interesting" keys come
/// first and total size stays bounded.
pub fn compact_json_value(value: &Value) -> Value {
    let preferred = [
        "ok",
        "error",
        "message",
        "site",
        "action",
        "entity_type",
        "query",
        "count",
        "state",
        "result",
        "cards",
        "entity",
        "title",
        "url",
        "summary",
    ];
    match value {
        Value::Object(map) => {
            let mut ordered_keys: Vec<String> = preferred
                .iter()
                .filter_map(|p| {
                    if map.contains_key(*p) {
                        Some((*p).to_string())
                    } else {
                        None
                    }
                })
                .collect();
            for key in map.keys() {
                if !ordered_keys.iter().any(|k| k == key) {
                    ordered_keys.push(key.clone());
                }
            }
            let mut out = Map::new();
            for key in ordered_keys.iter().take(16) {
                if let Some(v) = map.get(key) {
                    out.insert(key.clone(), compact_json_value(v));
                }
            }
            Value::Object(out)
        }
        Value::Array(arr) => {
            let head: Vec<Value> = arr.iter().take(5).map(compact_json_value).collect();
            Value::Array(head)
        }
        Value::String(s) => Value::String(truncate(s, 320)),
        other => other.clone(),
    }
}

/// Bound a tool-result text. If it parses as JSON and is too long, run
/// [`compact_json_value`] over it. Otherwise truncate the string directly.
pub fn compress_text_maybe_json(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        let compact = compact_json_value(&value);
        if let Ok(rendered) = serde_json::to_string_pretty(&compact) {
            if rendered.chars().count() <= max_chars {
                return rendered;
            }
            return truncate_result(&rendered, max_chars);
        }
    }
    truncate_result(text, max_chars)
}

/// Entity-aware deep compaction with depth limit. Used by the
/// evidence/working-memory pipeline so that long tool outputs don't bloat the
/// persisted snapshot.
pub fn compact_value(value: &Value) -> Value {
    compact_value_depth(value, 0)
}

fn compact_value_depth(value: &Value, depth: usize) -> Value {
    if depth >= 3 {
        return match value {
            Value::String(s) => Value::String(truncate(s, 320)),
            other => other.clone(),
        };
    }
    match value {
        Value::Object(map) => {
            let preferred = [
                "id",
                "entity_id",
                "note_id",
                "type",
                "entity_type",
                "title",
                "author",
                "url",
                "resolved_url",
                "summary",
                "content_summary",
                "key_points",
                "top_comments",
                "likes",
                "comments_count",
                "favorites",
                "screenshot",
                "artifact_path",
            ];
            let mut ordered: Vec<String> = preferred
                .iter()
                .filter_map(|p| {
                    if map.contains_key(*p) {
                        Some((*p).to_string())
                    } else {
                        None
                    }
                })
                .collect();
            for key in map.keys() {
                if !ordered.iter().any(|k| k == key) {
                    ordered.push(key.clone());
                }
            }
            let mut out = Map::new();
            for key in ordered.iter().take(20) {
                if let Some(v) = map.get(key) {
                    out.insert(key.clone(), compact_value_depth(v, depth + 1));
                }
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(
            arr.iter()
                .take(8)
                .map(|v| compact_value_depth(v, depth + 1))
                .collect(),
        ),
        Value::String(s) => Value::String(truncate(s, 600)),
        other => other.clone(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truncate_short_passthrough() {
        assert_eq!(truncate("hello", 32), "hello");
    }

    #[test]
    fn truncate_long_suffix() {
        let out = truncate(&"a".repeat(40), 10);
        assert!(out.starts_with("aaaaaaaaaa"));
        assert!(out.ends_with("[truncated]"));
        assert!(out.chars().count() > 10);
    }

    #[test]
    fn compact_json_picks_preferred_keys_first() {
        let value = json!({
            "z_extra": "x",
            "ok": true,
            "summary": "hi"
        });
        let compact = compact_json_value(&value);
        let keys: Vec<&str> = compact
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(keys[0], "ok");
        assert_eq!(keys[1], "summary");
    }

    #[test]
    fn compress_text_falls_through_when_short() {
        let s = "small payload";
        assert_eq!(compress_text_maybe_json(s, 100), s);
    }
}
