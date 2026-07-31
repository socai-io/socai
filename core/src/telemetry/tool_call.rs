//! Shared summarization of a site tool call for the `socai_tool_call` telemetry
//! event. Both the CLI daemon and the desktop app feed their tool invocations
//! through here, so every site (xhs, dy, and any future site) is reported
//! through one code path — there is no per-site or per-arg whitelist to keep in
//! sync, and newly-added commands/params are captured automatically.

use serde_json::{json, Map, Value};

const PAGE_OCR_MAX_CHARS: usize = 200;

/// Summarize a tool call's input arguments. The search `query` is the one gated
/// arg: its character length is always reported, but the raw text only when
/// `include_query_text` is on. Every other arg flows into a nested `metadata`
/// object as-is, so newly-added params/commands/sites are captured
/// automatically. Only the arguments are summarized here — ordinary tool
/// output (note bodies, comments) is never included; see
/// [`summarize_tool_result`] for bounded failure-page OCR diagnostics.
pub fn summarize_tool_args(args: &Value, include_query_text: bool) -> Map<String, Value> {
    let mut props = Map::new();
    let Some(obj) = args.as_object() else {
        return props;
    };
    let mut metadata = Map::new();
    for (key, value) in obj {
        if key == "query" {
            if let Some(query) = value.as_str() {
                let query = query.trim();
                if !query.is_empty() {
                    props.insert("query_len".into(), json!(query.chars().count()));
                    props.insert("query_text_enabled".into(), json!(include_query_text));
                    if include_query_text {
                        props.insert(
                            "query_text".into(),
                            json!(super::trace::redact_secrets(query)),
                        );
                    }
                }
            }
            continue;
        }
        if let Some(mut value) = meaningful_metadata_value(value) {
            super::trace::redact_secrets_in_value(&mut value);
            metadata.insert(key.clone(), value);
        }
    }
    if !metadata.is_empty() {
        props.insert("metadata".into(), Value::Object(metadata));
    }
    props
}

/// Normalize an arg value for `metadata`, returning `None` for the
/// "unset"/default shapes that shouldn't be reported: `null`, `false`,
/// empty/whitespace-only strings, and empty arrays/objects. Strings are trimmed;
/// every other value is reported verbatim.
fn meaningful_metadata_value(value: &Value) -> Option<Value> {
    match value {
        Value::Null | Value::Bool(false) => None,
        Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                None
            } else {
                Some(json!(text))
            }
        }
        Value::Array(items) if items.is_empty() => None,
        Value::Object(map) if map.is_empty() => None,
        other => Some(other.clone()),
    }
}

/// Extract safe metrics from a tool call's output. Reports collection sizes and
/// presence flags, plus the explicitly bounded unexpected-page OCR diagnostic;
/// note bodies and comments are never copied. The value may be the raw tool
/// result or wrapped in a `data` envelope (CLI daemon); both shapes are handled.
pub fn summarize_tool_result(value: &Value) -> Map<String, Value> {
    let mut props = Map::new();
    let data = value.get("data").unwrap_or(value);
    if let Some(ok) = data.get("ok").and_then(Value::as_bool) {
        props.insert("result_ok".into(), json!(ok));
    }
    if let Some(cards) = data.get("cards").and_then(Value::as_array) {
        props.insert("cards_count".into(), json!(cards.len()));
    }
    if let Some(cards) = data
        .get("search")
        .and_then(|search| search.get("cards"))
        .and_then(Value::as_array)
    {
        props.insert("search_cards_count".into(), json!(cards.len()));
    }
    if let Some(cards) = data.get("selected_cards").and_then(Value::as_array) {
        props.insert("selected_cards_count".into(), json!(cards.len()));
    }
    if let Some(notes) = data.get("notes").and_then(Value::as_array) {
        props.insert("notes_count".into(), json!(notes.len()));
        let skipped = notes
            .iter()
            .filter(|note| note.get("skipped").is_some())
            .count();
        props.insert("notes_skipped_count".into(), json!(skipped));
    }
    if value.get("run_dir").is_some() {
        props.insert("has_run_dir".into(), json!(true));
    }
    if let Some(text) = find_string(data, "page_ocr_text") {
        props.insert(
            "page_ocr_text".into(),
            json!(super::trace::redact_secrets(text)
                .chars()
                .take(PAGE_OCR_MAX_CHARS)
                .collect::<String>()),
        );
    }
    if let Some(region) = find_string(data, "page_ocr_region") {
        props.insert("page_ocr_region".into(), json!(region));
    }
    if let Some(truncated) = find_bool(data, "page_ocr_truncated") {
        props.insert("page_ocr_truncated".into(), json!(truncated));
    }
    if let Some(detected) = find_bool(data, "rate_limit_detected") {
        props.insert("rate_limit_detected".into(), json!(detected));
    }
    if let Some(marker) = find_string(data, "rate_limit_marker") {
        props.insert("rate_limit_marker".into(), json!(marker));
    }
    if let Some(tool) = find_string(data, "recovery_tool") {
        props.insert("recovery_tool".into(), json!(tool));
    }
    if let Some(waited_seconds) = find_u64(data, "waited_seconds") {
        props.insert("waited_seconds".into(), json!(waited_seconds));
    }
    if let Some(error) = find_string(data, "page_ocr_error") {
        props.insert(
            "page_ocr_error".into(),
            json!(super::trace::redact_secrets(error)
                .chars()
                .take(PAGE_OCR_MAX_CHARS)
                .collect::<String>()),
        );
    }
    if let Some(error) = find_string(data, "page_error") {
        props.insert(
            "page_error".into(),
            json!(super::trace::redact_secrets(error)
                .chars()
                .take(240)
                .collect::<String>()),
        );
    }
    if let Some(url) = find_string(data, "page_url") {
        let path_only = url.split(['?', '#']).next().unwrap_or(url);
        props.insert(
            "page_url".into(),
            json!(super::trace::redact_secrets(path_only)
                .chars()
                .take(500)
                .collect::<String>()),
        );
    }
    if data.get("ok").and_then(Value::as_bool) == Some(false) {
        if let Some(reason) = data.get("reason").and_then(Value::as_str) {
            props.insert(
                "failure_reason".into(),
                json!(reason.chars().take(120).collect::<String>()),
            );
        }
    }
    props
}

fn find_string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    match value {
        Value::Object(map) => map
            .get(key)
            .and_then(Value::as_str)
            .or_else(|| map.values().find_map(|value| find_string(value, key))),
        Value::Array(items) => items.iter().find_map(|value| find_string(value, key)),
        _ => None,
    }
}

fn find_bool(value: &Value, key: &str) -> Option<bool> {
    match value {
        Value::Object(map) => map
            .get(key)
            .and_then(Value::as_bool)
            .or_else(|| map.values().find_map(|value| find_bool(value, key))),
        Value::Array(items) => items.iter().find_map(|value| find_bool(value, key)),
        _ => None,
    }
}

fn find_u64(value: &Value, key: &str) -> Option<u64> {
    match value {
        Value::Object(map) => map
            .get(key)
            .and_then(Value::as_u64)
            .or_else(|| map.values().find_map(|value| find_u64(value, key))),
        Value::Array(items) => items.iter().find_map(|value| find_u64(value, key)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn includes_query_text_when_enabled() {
        let props = summarize_tool_args(&json!({ "query": "运营爆款思路" }), true);
        assert_eq!(props.get("query_text"), Some(&json!("运营爆款思路")));
        assert_eq!(props.get("query_text_enabled"), Some(&json!(true)));
        assert_eq!(props.get("query_len"), Some(&json!(6)));
    }

    #[test]
    fn redacts_query_text_when_disabled() {
        let props = summarize_tool_args(&json!({ "query": "运营爆款思路" }), false);
        assert!(!props.contains_key("query_text"));
        assert_eq!(props.get("query_text_enabled"), Some(&json!(false)));
        assert_eq!(props.get("query_len"), Some(&json!(6)));
    }

    #[test]
    fn omits_defaulted_optional_params() {
        let props = summarize_tool_args(&json!({ "query": "x", "debug_snapshot": false }), true);
        assert!(!props.contains_key("metadata"));
    }

    #[test]
    fn keeps_query_out_of_metadata_and_nests_other_params() {
        let props = summarize_tool_args(
            &json!({
                "query": "x",
                "author_id": "abc",
                "num_notes": 12,
                "debug_snapshot": true
            }),
            true,
        );
        assert!(props.contains_key("query_len"));
        let metadata = props
            .get("metadata")
            .and_then(Value::as_object)
            .expect("metadata object");
        assert!(!metadata.contains_key("query"));
        assert_eq!(metadata.get("author_id"), Some(&json!("abc")));
        assert_eq!(metadata.get("num_notes"), Some(&json!(12)));
        assert_eq!(metadata.get("debug_snapshot"), Some(&json!(true)));
    }

    #[test]
    fn reports_arbitrary_params_without_a_whitelist() {
        // `author_id` (xhs) and any future site's params flow through generically;
        // defaulted flags (`preview: false`) stay out of the payload.
        let props = summarize_tool_args(
            &json!({
                "author_id": "5ff00000000000000000abcd",
                "num_notes": 8,
                "preview": false,
                "download_media": true
            }),
            true,
        );
        let metadata = props
            .get("metadata")
            .and_then(Value::as_object)
            .expect("metadata object");
        assert_eq!(
            metadata.get("author_id"),
            Some(&json!("5ff00000000000000000abcd"))
        );
        assert_eq!(metadata.get("num_notes"), Some(&json!(8)));
        assert_eq!(metadata.get("download_media"), Some(&json!(true)));
        assert!(!metadata.contains_key("preview"), "defaulted false dropped");
    }

    #[test]
    fn result_summary_reports_dy_and_xhs_card_counts() {
        // dy search returns top-level `cards`; the same path covers any site that
        // returns a `cards` array.
        let props = summarize_tool_result(&json!({ "ok": true, "cards": [{}, {}, {}] }));
        assert_eq!(props.get("result_ok"), Some(&json!(true)));
        assert_eq!(props.get("cards_count"), Some(&json!(3)));
    }

    #[test]
    fn result_summary_extracts_safe_counts_without_copying_text() {
        let page_ocr = "限".repeat(PAGE_OCR_MAX_CHARS + 1);
        let props = summarize_tool_result(&json!({
            "run_dir": "/tmp/socai-run",
            "data": {
                "ok": false,
                "reason": "not_profile_page",
                "page_error": "not_profile_page",
                "page_url": "https://www.xiaohongshu.com/explore/abc?xsec_token=secret",
                "page_ocr_text": page_ocr,
                "page_ocr_region": "center_70_percent",
                "page_ocr_truncated": true,
                "rate_limit_detected": true,
                "rate_limit_marker": "300013",
                "recovery_tool": "wait_for_rate_limit",
                "cards": [{}, {}],
                "search": { "cards": [{}, {}, {}] },
                "selected_cards": [{}],
                "notes": [
                    { "id": "1", "body": "must not be copied" },
                    { "skipped": "missing note" },
                    { "skipped": true, "comments": ["must not be copied"] }
                ]
            }
        }));
        assert_eq!(props.get("result_ok"), Some(&json!(false)));
        assert_eq!(props.get("cards_count"), Some(&json!(2)));
        assert_eq!(props.get("search_cards_count"), Some(&json!(3)));
        assert_eq!(props.get("selected_cards_count"), Some(&json!(1)));
        assert_eq!(props.get("notes_count"), Some(&json!(3)));
        assert_eq!(props.get("notes_skipped_count"), Some(&json!(2)));
        assert_eq!(props.get("has_run_dir"), Some(&json!(true)));
        assert_eq!(
            props.get("failure_reason"),
            Some(&json!("not_profile_page"))
        );
        assert_eq!(props.get("page_error"), Some(&json!("not_profile_page")));
        assert_eq!(
            props.get("page_url"),
            Some(&json!("https://www.xiaohongshu.com/explore/abc"))
        );
        assert_eq!(
            props
                .get("page_ocr_text")
                .and_then(Value::as_str)
                .map(|text| text.chars().count()),
            Some(PAGE_OCR_MAX_CHARS)
        );
        assert_eq!(
            props.get("page_ocr_region"),
            Some(&json!("center_70_percent"))
        );
        assert_eq!(props.get("page_ocr_truncated"), Some(&json!(true)));
        assert_eq!(props.get("rate_limit_detected"), Some(&json!(true)));
        assert_eq!(props.get("rate_limit_marker"), Some(&json!("300013")));
        assert_eq!(
            props.get("recovery_tool"),
            Some(&json!("wait_for_rate_limit"))
        );
        assert!(!props.contains_key("body"));
        assert!(!props.contains_key("comments"));
    }
}
