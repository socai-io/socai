//! Bounded visual diagnostics for unexpected Xiaohongshu page failures.
//!
//! Failure callers only need [`attach`], [`copy`], or [`promote`]. Screenshot
//! capture, center cropping, OCR, truncation, login-state exclusion, and reuse
//! of an already-captured diagnostic stay isolated here.

use anyhow::Result;
use serde_json::{json, Map, Value};

use crate::cdp::PageSession;

/// Read the center of the current viewport so XHS's header and side navigation
/// do not drown out the actual error page or missing-control state.
const OCR_VIEWPORT_PERCENT: u32 = 70;
/// Bound agent and telemetry payloads if navigation landed on a content page.
const OCR_MAX_CHARS: usize = 200;
const DIAGNOSTIC_FIELDS: &[&str] = &[
    "page_ocr_text",
    "page_ocr_region",
    "page_ocr_truncated",
    "page_ocr_error",
    "page_error",
    "page_url",
    "rate_limit_detected",
    "rate_limit_marker",
    "security_verification_detected",
    "security_verification_marker",
    "recovery_tool",
];

/// Attach or promote a failure diagnostic exactly once. Low-level failures
/// such as `search_input_not_found` may already carry OCR; wrappers reuse it
/// rather than taking another screenshot.
pub(super) async fn attach(page: &PageSession, result: &mut Value) {
    if failure_is_login(result) {
        return;
    }
    if let Some(existing) = diagnostic_fields(result) {
        merge(result, Value::Object(existing));
        classify_page_blocker(result);
        return;
    }
    if result.get("url").is_none() {
        if let Some(url) = current_url(page).await {
            result["url"] = json!(url);
        }
    }
    merge(result, capture(page).await);
    classify_page_blocker(result);
}

/// Copy a nested diagnostic from `source` onto a different result object.
pub(super) fn copy(target: &mut Value, source: &Value) {
    if let Some(diagnostic) = diagnostic_fields(source) {
        merge(target, Value::Object(diagnostic));
        classify_page_blocker(target);
    }
}

/// Promote a nested diagnostic before compacting away its original wrapper.
pub(super) fn promote(value: &mut Value) {
    let Some(diagnostic) = diagnostic_fields(value) else {
        return;
    };
    merge(value, Value::Object(diagnostic));
    classify_page_blocker(value);
}

fn classify_page_blocker(value: &mut Value) {
    classify_rate_limit(value);
    classify_security_verification(value);
}

async fn capture(page: &PageSession) -> Value {
    let mut diagnostic = json!({
        "page_ocr_region": "center_70_percent",
    });
    let screenshot = match page.screenshot_png(false).await {
        Ok(bytes) => bytes,
        Err(err) => {
            diagnostic["page_ocr_error"] = json!(truncate(&format!("screenshot failed: {err}")));
            return diagnostic;
        }
    };
    let recognized =
        tokio::task::spawn_blocking(move || ocr_center(screenshot, OCR_VIEWPORT_PERCENT)).await;
    match recognized {
        Ok(Ok(text)) => {
            let trimmed = text.trim();
            if let Some(marker) = detect_rate_limit_marker(trimmed) {
                diagnostic["rate_limit_detected"] = json!(true);
                diagnostic["rate_limit_marker"] = json!(marker);
                diagnostic["recovery_tool"] = json!("wait_for_rate_limit");
            }
            if let Some(marker) = detect_security_verification_marker(trimmed) {
                diagnostic["security_verification_detected"] = json!(true);
                diagnostic["security_verification_marker"] = json!(marker);
            }
            diagnostic["page_ocr_truncated"] = json!(trimmed.chars().count() > OCR_MAX_CHARS);
            diagnostic["page_ocr_text"] = json!(truncate(trimmed));
        }
        Ok(Err(err)) => diagnostic["page_ocr_error"] = json!(truncate(&err.to_string())),
        Err(err) => {
            diagnostic["page_ocr_error"] = json!(truncate(&format!("OCR task failed: {err}")))
        }
    }
    diagnostic
}

async fn current_url(page: &PageSession) -> Option<String> {
    page.page_info()
        .await
        .ok()?
        .get("url")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn diagnostic_fields(value: &Value) -> Option<Map<String, Value>> {
    match value {
        Value::Object(map) => {
            if map.contains_key("page_ocr_region") {
                return Some(
                    DIAGNOSTIC_FIELDS
                        .iter()
                        .filter_map(|field| {
                            map.get(*field)
                                .cloned()
                                .map(|value| ((*field).to_string(), value))
                        })
                        .collect(),
                );
            }
            preferred_diagnostic(map.values())
        }
        Value::Array(items) => preferred_diagnostic(items.iter()),
        _ => None,
    }
}

fn preferred_diagnostic<'a>(values: impl Iterator<Item = &'a Value>) -> Option<Map<String, Value>> {
    let mut fallback = None;
    for value in values {
        let Some(diagnostic) = diagnostic_fields(value) else {
            continue;
        };
        if diagnostic
            .get("rate_limit_detected")
            .and_then(Value::as_bool)
            == Some(true)
            || diagnostic
                .get("security_verification_detected")
                .and_then(Value::as_bool)
                == Some(true)
        {
            return Some(diagnostic);
        }
        fallback.get_or_insert(diagnostic);
    }
    fallback
}

fn failure_is_login(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            map.get("login_required").and_then(Value::as_bool) == Some(true)
                || ["reason", "error"]
                    .iter()
                    .any(|key| map.get(*key).and_then(Value::as_str) == Some("login_required"))
                || map.values().any(failure_is_login)
        }
        Value::Array(items) => items.iter().any(failure_is_login),
        _ => false,
    }
}

fn merge(target: &mut Value, diagnostic: Value) {
    let page_url = target.get("url").cloned();
    let page_error = target
        .get("reason")
        .or_else(|| target.get("error"))
        .cloned();
    let (Some(target), Some(diagnostic)) = (target.as_object_mut(), diagnostic.as_object()) else {
        return;
    };
    for (key, value) in diagnostic {
        target.insert(key.clone(), value.clone());
    }
    if let Some(page_url) = page_url {
        target.insert("page_url".into(), page_url);
    }
    if let Some(page_error) = page_error {
        target.entry("page_error").or_insert(page_error);
    }
}

fn detect_rate_limit_marker(text: &str) -> Option<&'static str> {
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    if digits.contains("300013") {
        return Some("300013");
    }
    let compact: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
    compact.contains("访问频繁").then_some("访问频繁")
}

fn detect_security_verification_marker(text: &str) -> Option<&'static str> {
    let compact: String = text
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if compact.contains("securityverification") {
        return Some("Security Verification");
    }
    if compact.contains("pleaseselectthetwoimages") {
        return Some("image captcha");
    }
    None
}

fn classify_rate_limit(value: &mut Value) {
    let marker = value
        .get("rate_limit_marker")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            value
                .get("page_ocr_text")
                .and_then(Value::as_str)
                .and_then(detect_rate_limit_marker)
                .map(str::to_string)
        });
    let Some(marker) = marker else {
        return;
    };
    let Some(map) = value.as_object_mut() else {
        return;
    };
    map.insert("ok".into(), json!(false));
    map.insert("reason".into(), json!("rate_limited"));
    map.insert("rate_limit_detected".into(), json!(true));
    map.insert("rate_limit_marker".into(), json!(marker));
    map.insert("recovery_tool".into(), json!("wait_for_rate_limit"));
}

fn classify_security_verification(value: &mut Value) {
    let marker = value
        .get("security_verification_marker")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            value
                .get("page_ocr_text")
                .and_then(Value::as_str)
                .and_then(detect_security_verification_marker)
                .map(str::to_string)
        });
    let Some(marker) = marker else {
        return;
    };
    let Some(map) = value.as_object_mut() else {
        return;
    };
    map.insert("ok".into(), json!(false));
    map.insert("reason".into(), json!("security_verification"));
    map.insert("security_verification_detected".into(), json!(true));
    map.insert("security_verification_marker".into(), json!(marker));
}

fn ocr_center(screenshot: Vec<u8>, percent: u32) -> Result<String> {
    let image = image::load_from_memory(&screenshot)?;
    let percent = percent.clamp(1, 100);
    let width = image.width().max(1);
    let height = image.height().max(1);
    let crop_width = ((u64::from(width) * u64::from(percent)) / 100).max(1) as u32;
    let crop_height = ((u64::from(height) * u64::from(percent)) / 100).max(1) as u32;
    let x = (width.saturating_sub(crop_width)) / 2;
    let y = (height.saturating_sub(crop_height)) / 2;
    let cropped = image.crop_imm(x, y, crop_width, crop_height);
    let mut encoded = std::io::Cursor::new(Vec::new());
    cropped.write_to(&mut encoded, image::ImageFormat::Png)?;
    let mut batch = crate::media::ocr_images_bytes(vec![(0, encoded.into_inner())]);
    match batch.results.pop() {
        Some((_, Ok(text))) => Ok(text),
        Some((_, Err(err))) => Err(anyhow::anyhow!(err)),
        None => Ok(String::new()),
    }
}

fn truncate(text: &str) -> String {
    text.chars().take(OCR_MAX_CHARS).collect()
}
