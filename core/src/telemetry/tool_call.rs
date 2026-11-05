//! Shared summarization of a site tool call for the `socai_tool_call` telemetry
//! event. Both the CLI daemon and the desktop app feed their tool invocations
//! through here, so every site (xhs, dy, and any future site) is reported
//! through one code path. Newly-added commands and parameters are captured as
//! privacy-safe shapes by default instead of forwarding arbitrary text.

use serde_json::{json, Map, Value};

const PAGE_OCR_MAX_CHARS: usize = 200;

/// Summarize a tool call's input arguments. The search `query` is the one gated
/// arg: its character length is always reported, but the raw text only when
/// `include_query_text` is on. Other booleans and numbers are retained, while
/// strings, arrays, and objects become lengths/counts so post locators,
/// usernames, comments, and future free-text fields cannot enter telemetry by
/// default. Only the arguments are summarized here — ordinary tool output
/// (note bodies, comments, page OCR text) is never included; see
/// [`summarize_tool_result`] for bounded failure diagnostics.
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
        summarize_metadata_value(&mut metadata, key, value);
    }
    if !metadata.is_empty() {
        props.insert("metadata".into(), Value::Object(metadata));
    }
    props
}

/// Keep useful parameter shape without retaining arbitrary user/platform text.
/// Derived keys remain shallow primitives so the telemetry proxy can enforce
/// its existing bounded `metadata` contract.
fn summarize_metadata_value(metadata: &mut Map<String, Value>, key: &str, value: &Value) {
    match value {
        Value::Null | Value::Bool(false) => {}
        Value::Bool(true) | Value::Number(_) => {
            metadata.insert(key.to_string(), value.clone());
        }
        Value::String(text) => {
            let text = text.trim();
            if !text.is_empty() {
                metadata.insert(format!("{key}_len"), json!(text.chars().count()));
            }
        }
        Value::Array(items) if !items.is_empty() => {
            metadata.insert(format!("{key}_count"), json!(items.len()));
        }
        Value::Object(fields) if !fields.is_empty() => {
            metadata.insert(format!("{key}_field_count"), json!(fields.len()));
        }
        Value::Array(_) | Value::Object(_) => {}
    }
}

/// Extract safe metrics from a tool call's output. Reports collection sizes and
/// presence flags, plus the unexpected-page OCR length and fixed diagnostics;
/// note bodies, comments, and OCR text are never copied. The value may be the
/// raw tool result or wrapped in a `data` envelope (CLI daemon); both shapes
/// are handled.
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
    if let Some(strategy) = find_string(data, "strategy") {
        props.insert(
            "result_strategy".into(),
            json!(strategy.chars().take(120).collect::<String>()),
        );
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
    if let Some(logged_in) = find_bool(data, "logged_in") {
        props.insert("login_detected".into(), json!(logged_in));
    }
    if let Some(timed_out) = find_bool(data, "timed_out") {
        props.insert("login_wait_timed_out".into(), json!(timed_out));
    }
    if let Some(remote_browser) = find_bool(data, "remote_browser") {
        props.insert("remote_browser".into(), json!(remote_browser));
    }
    if let Some(text) = find_string(data, "page_ocr_text") {
        props.insert("page_ocr_text_len".into(), json!(text.chars().count()));
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
    if let Some(detected) = find_bool(data, "security_verification_detected") {
        props.insert("security_verification_detected".into(), json!(detected));
    }
    if let Some(marker) = find_string(data, "security_verification_marker") {
        props.insert("security_verification_marker".into(), json!(marker));
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
            json!(super::trace::redact_telemetry_error(error)
                .chars()
                .take(PAGE_OCR_MAX_CHARS)
                .collect::<String>()),
        );
    }
    if let Some(error) = find_string(data, "page_error") {
        props.insert(
            "page_error".into(),
            json!(super::trace::redact_telemetry_error(error)
                .chars()
                .take(240)
                .collect::<String>()),
        );
    }
    if let Some(url) = find_string(data, "page_url") {
        if let Some((origin, path_depth)) = page_url_shape(url) {
            props.insert("page_url".into(), json!(origin));
            props.insert("page_path_depth".into(), json!(path_depth));
        }
    }
    // Only explicit operational fields; never recursively pick an error
    // from page content, note bodies or the model's recovery instructions.
    let record_error = |props: &mut Map<String, Value>, key: &str, error: &str| {
        let safe = super::trace::redact_telemetry_error(error);
        props.insert(
            key.into(),
            json!(safe
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(480)
                .collect::<String>()),
        );
    };
    for field in ["search_retry_error", "search_reset_error"] {
        if let Some(error) = data.get(field).and_then(Value::as_str) {
            record_error(&mut props, field, error);
        }
    }
    if data.get("ok").and_then(Value::as_bool) == Some(false) {
        if let Some(error) = data.get("error").and_then(Value::as_str) {
            record_error(&mut props, "failure_detail", error);
        }
        if let Some(reason) = data
            .get("reason")
            .and_then(Value::as_str)
            .or_else(|| batch_failure_reason(data))
        {
            props.insert(
                "failure_reason".into(),
                json!(super::trace::redact_telemetry_error(reason)
                    .chars()
                    .take(120)
                    .collect::<String>()),
            );
        }
    }
    props
}

fn batch_failure_reason(value: &Value) -> Option<&str> {
    ["videos", "notes", "posts"].iter().find_map(|key| {
        value.get(*key).and_then(Value::as_array).and_then(|items| {
            items.iter().find_map(|item| {
                (item.get("ok").and_then(Value::as_bool) != Some(true))
                    .then(|| item.get("reason").and_then(Value::as_str))
                    .flatten()
            })
        })
    })
}

fn page_url_shape(value: &str) -> Option<(String, usize)> {
    let url = reqwest::Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    let path_depth = url
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|segment| !segment.is_empty())
        .count();
    Some((url.origin().ascii_serialization(), path_depth))
}

/// Summarize the model-visible content blocks emitted by a trusted site tool.
/// Local tools such as `read_file` and `shell` may return arbitrary user JSON,
/// so their text blocks must remain opaque to telemetry.
pub fn summarize_site_tool_result(tool_name: &str, value: &Value) -> Map<String, Value> {
    if !is_site_tool_result(tool_name) {
        return Map::new();
    }
    result_json_from_content_blocks(value)
        .as_ref()
        .map(summarize_tool_result)
        .unwrap_or_default()
}

pub fn is_site_tool_result(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "search"
            | "get_notes"
            | "author_scan"
            | "page_state"
            | "wait_for_login"
            | "wait_for_douyin_login"
            | "wait_for_tiktok_login"
            | "wait_for_instagram_login"
            | "wait_for_linkedin_login"
            | "wait_for_x_login"
    )
}

/// Desktop and trace events serialize a tool result as model-visible content:
/// `[{"type":"text","text":"<tool JSON>"}, ...]`. Parse only a complete
/// JSON text block after the caller has established that the tool is a site
/// tool. Non-JSON prose and image blocks stay opaque.
fn result_json_from_content_blocks(value: &Value) -> Option<Value> {
    let blocks = value.as_array()?;
    blocks.iter().find_map(|block| {
        let block = block.as_object()?;
        if block.get("type").and_then(Value::as_str) != Some("text") {
            return None;
        }
        let text = block.get("text").and_then(Value::as_str)?.trim();
        if !(text.starts_with('{') || text.starts_with('[')) {
            return None;
        }
        serde_json::from_str(text).ok()
    })
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
        assert_eq!(metadata.get("author_id_len"), Some(&json!(3)));
        assert!(!metadata.contains_key("author_id"));
        assert_eq!(metadata.get("num_notes"), Some(&json!(12)));
        assert_eq!(metadata.get("debug_snapshot"), Some(&json!(true)));
    }

    #[test]
    fn reports_arbitrary_param_shapes_without_forwarding_text() {
        // Future params flow through generically as safe scalar/count metadata;
        // defaulted flags (`preview: false`) stay out of the payload.
        let props = summarize_tool_args(
            &json!({
                "author_id": "5ff00000000000000000abcd",
                "posts": ["https://www.instagram.com/p/private-shortcode/"],
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
        assert_eq!(metadata.get("author_id_len"), Some(&json!(24)));
        assert_eq!(metadata.get("posts_count"), Some(&json!(1)));
        assert!(!metadata.contains_key("author_id"));
        assert!(!metadata.contains_key("posts"));
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
        let result = json!({
            "run_dir": "/tmp/socai-run",
            "data": {
                "ok": false,
                "strategy": "direct_search_result_navigation",
                "reason": "not_profile_page",
                "error": "read search page URL: failed at https://example.org/private?q=secret",
                "search_retry_error": "submit search: context was destroyed",
                "search_reset_error": "reset failed at https://example.org/private?q=secret",
                "page_error": "not_profile_page",
                "page_url": "https://www.xiaohongshu.com/explore/abc?xsec_token=secret",
                "page_ocr_text": page_ocr,
                "page_ocr_region": "center_70_percent",
                "page_ocr_truncated": true,
                "rate_limit_detected": true,
                "rate_limit_marker": "300013",
                "security_verification_detected": true,
                "security_verification_marker": "Security Verification",
                "recovery_tool": "wait_for_rate_limit",
                "logged_in": false,
                "timed_out": true,
                "remote_browser": true,
                "cards": [{}, {}],
                "search": { "cards": [{}, {}, {}] },
                "selected_cards": [{}],
                "notes": [
                    { "id": "1", "body": "must not be copied" },
                    { "skipped": "missing note" },
                    { "skipped": true, "comments": ["must not be copied"] }
                ]
            }
        });
        let props = summarize_tool_result(&result);
        assert_eq!(props.get("result_ok"), Some(&json!(false)));
        assert_eq!(
            props.get("result_strategy"),
            Some(&json!("direct_search_result_navigation"))
        );
        assert_eq!(props.get("cards_count"), Some(&json!(2)));
        assert_eq!(props.get("search_cards_count"), Some(&json!(3)));
        assert_eq!(props.get("selected_cards_count"), Some(&json!(1)));
        assert_eq!(props.get("notes_count"), Some(&json!(3)));
        assert_eq!(props.get("notes_skipped_count"), Some(&json!(2)));
        assert_eq!(props.get("has_run_dir"), Some(&json!(true)));
        assert_eq!(props.get("login_detected"), Some(&json!(false)));
        assert_eq!(props.get("login_wait_timed_out"), Some(&json!(true)));
        assert_eq!(props.get("remote_browser"), Some(&json!(true)));
        assert_eq!(
            props.get("failure_reason"),
            Some(&json!("not_profile_page"))
        );
        assert_eq!(
            props.get("failure_detail"),
            Some(&json!(
                "read search page URL: failed at https://example.org/<redacted>"
            ))
        );
        assert_eq!(
            props.get("search_retry_error"),
            Some(&json!("submit search: context was destroyed"))
        );
        assert_eq!(
            props.get("search_reset_error"),
            Some(&json!("reset failed at https://example.org/<redacted>"))
        );
        assert_eq!(props.get("page_error"), Some(&json!("not_profile_page")));
        assert_eq!(
            props.get("page_url"),
            Some(&json!("https://www.xiaohongshu.com"))
        );
        assert_eq!(props.get("page_path_depth"), Some(&json!(2)));
        assert!(!props.contains_key("page_ocr_text"));
        assert_eq!(
            props.get("page_ocr_text_len"),
            Some(&json!(PAGE_OCR_MAX_CHARS + 1))
        );
        assert_eq!(
            props.get("page_ocr_region"),
            Some(&json!("center_70_percent"))
        );
        assert_eq!(props.get("page_ocr_truncated"), Some(&json!(true)));
        assert_eq!(props.get("rate_limit_detected"), Some(&json!(true)));
        assert_eq!(props.get("rate_limit_marker"), Some(&json!("300013")));
        assert_eq!(
            props.get("security_verification_detected"),
            Some(&json!(true))
        );
        assert_eq!(
            props.get("security_verification_marker"),
            Some(&json!("Security Verification"))
        );
        assert_eq!(
            props.get("recovery_tool"),
            Some(&json!("wait_for_rate_limit"))
        );
        assert!(!props.contains_key("body"));
        assert!(!props.contains_key("comments"));

        let batch_props = summarize_tool_result(&json!({
            "ok": false,
            "videos": [
                { "ok": true, "reason": "must_not_win" },
                { "ok": false, "reason": "video_navigation_timeout" }
            ]
        }));
        assert_eq!(
            batch_props.get("failure_reason"),
            Some(&json!("video_navigation_timeout"))
        );

        let note_batch_props = summarize_tool_result(&json!({
            "ok": false,
            "notes": [
                { "ok": false, "reason": "note_read_failed" }
            ]
        }));
        assert_eq!(
            note_batch_props.get("failure_reason"),
            Some(&json!("note_read_failed"))
        );

        let post_batch_props = summarize_tool_result(&json!({
            "ok": false,
            "posts": [
                { "ok": false, "reason": "post_unavailable" }
            ]
        }));
        assert_eq!(
            post_batch_props.get("failure_reason"),
            Some(&json!("post_unavailable"))
        );

        let sensitive_batch_props = summarize_tool_result(&json!({
            "ok": false,
            "videos": [
                { "ok": false, "reason": "request rejected: Bearer private-batch-token-123456" }
            ]
        }));
        assert_eq!(
            sensitive_batch_props.get("failure_reason"),
            Some(&json!("request rejected: Bearer [redacted]"))
        );

        let content = json!([
            { "type": "text", "text": serde_json::to_string_pretty(&result).unwrap() },
            { "type": "image", "media_type": "image/png" }
        ]);
        assert_eq!(summarize_site_tool_result("search", &content), props);
        for tool_name in [
            "wait_for_login",
            "wait_for_douyin_login",
            "wait_for_tiktok_login",
            "wait_for_instagram_login",
            "wait_for_linkedin_login",
            "wait_for_x_login",
        ] {
            assert_eq!(summarize_site_tool_result(tool_name, &content), props);
        }

        let hostile_local_json = json!([
            {
                "type": "text",
                "text": r#"{"ok":false,"reason":"private user value","page_ocr_text":"private file contents","recovery_tool":"private command"}"#
            }
        ]);
        assert!(summarize_site_tool_result("read_file", &hostile_local_json).is_empty());
        assert!(summarize_site_tool_result("shell", &hostile_local_json).is_empty());
        assert!(!is_site_tool_result("read_file"));
        assert!(!is_site_tool_result("shell"));
        assert!(summarize_site_tool_result(
            "search",
            &json!([{ "type": "image", "media_type": "image/png" }])
        )
        .is_empty());
        assert!(summarize_site_tool_result(
            "search",
            &json!([{ "type": "text", "text": "ordinary non-JSON output" }])
        )
        .is_empty());
    }
}
