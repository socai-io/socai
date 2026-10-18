use serde_json::{json, Value};
use std::{future::Future, time::Duration};

pub fn blocked(value: &Value) -> bool {
    matches!(
        value["reason"].as_str(),
        Some("challenge_required" | "rate_limited" | "retry_after")
    )
}

pub fn response_gate(status: u16, retry_after: Option<&str>) -> Option<Value> {
    if status != 429 && retry_after.is_none() {
        return None;
    }
    let seconds = retry_after
        .and_then(|value| {
            value.trim().parse::<u64>().ok().or_else(|| {
                chrono::DateTime::parse_from_rfc2822(value)
                    .ok()
                    .map(|date| {
                        (date.with_timezone(&chrono::Utc) - chrono::Utc::now())
                            .num_seconds()
                            .max(0) as u64
                    })
            })
        })
        .unwrap_or(0);
    Some(
        json!({"ok": false, "reason": if status == 429 { "rate_limited" } else { "retry_after" },
        "http_status": status, "retry_after_seconds": seconds}),
    )
}

pub async fn run<F, Fut>(mut operation: F) -> anyhow::Result<Value>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<Value>>,
{
    let mut retries = 0;
    loop {
        let mut value = operation().await?;
        if !blocked(&value) {
            return Ok(value);
        }
        if retries == 3 {
            value["retry_exhausted"] = json!(true);
            value["retries"] = json!(retries);
            return Ok(value);
        }
        retries += 1;
        let seconds = (retries * 60).max(value["retry_after_seconds"].as_u64().unwrap_or(0));
        tracing::info!(retries, seconds, reason = ?value["reason"], "WeChat access blocked; waiting before retry");
        tokio::time::sleep(Duration::from_secs(seconds)).await;
    }
}
