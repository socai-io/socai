use std::time::Duration;

use anyhow::Result;
use serde::Deserialize;

use super::auth::{bearer, configured_base_url, http_client, load_credentials};

#[derive(Debug, Clone, Deserialize)]
pub struct BrowserSessionInfo {
    pub session_id: String,
    pub connect_url: String,
    /// Server-side session lifetime; Browserbase hard-kills the session this
    /// many seconds after mint. Defaulted for servers that predate the field.
    #[serde(default = "default_browser_session_timeout_seconds")]
    pub timeout_seconds: u64,
}

fn default_browser_session_timeout_seconds() -> u64 {
    900
}

/// Browser-session requests get tighter budgets than the shared client's 120s
/// cap: a stalled mint should leave room for another attempt rather than
/// swallowing the connect loop's whole budget. The guarantee that no session is
/// minted after the caller gave up comes from `cdp::lifecycle::CONNECT_BUDGET`,
/// which bounds the entire connect regardless of these per-phase values.
const BROWSER_SESSION_CREATE_TIMEOUT: Duration = Duration::from_secs(25);
/// Release is best-effort and runs detached from a drop; the server-side
/// session timeout is the backstop, so it should never linger.
const BROWSER_SESSION_RELEASE_TIMEOUT: Duration = Duration::from_secs(10);

/// Mint a remote hosted browser session via socai-server. The server holds
/// all Browserbase credentials and authorizes the device token; the returned
/// connect URL carries only a session-scoped token.
///
/// Errors are single-level messages (no `context` chains): the CDP connect
/// loop surfaces only the outermost message as the disconnect reason.
pub async fn create_browser_session() -> Result<BrowserSessionInfo> {
    let base_url = configured_base_url()
        .ok_or_else(|| anyhow::anyhow!("socai pro server URL is not configured"))?;
    let creds = load_credentials().ok_or_else(|| {
        anyhow::anyhow!("socai pro is not activated; run `socai pro activate <invite_code>`")
    })?;
    let client = http_client()?;
    let response = bearer(
        client
            .post(format!("{base_url}/v1/browser/sessions"))
            .timeout(BROWSER_SESSION_CREATE_TIMEOUT),
        &creds.device_token,
    )
    .send()
    .await
    .map_err(|err| anyhow::anyhow!("remote browser session request failed: {err}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "remote browser session request failed ({status}): {}",
            short_error(&body)
        );
    }
    response
        .json()
        .await
        .map_err(|err| anyhow::anyhow!("remote browser session response was malformed: {err}"))
}

/// Best-effort early release of a remote browser session. The server-side
/// session timeout is the backstop, so callers may ignore failures.
pub async fn release_browser_session(session_id: &str) -> Result<()> {
    let base_url = configured_base_url()
        .ok_or_else(|| anyhow::anyhow!("socai pro server URL is not configured"))?;
    let creds = load_credentials().ok_or_else(|| anyhow::anyhow!("socai pro is not activated"))?;
    let client = http_client()?;
    bearer(
        client
            .post(format!(
                "{base_url}/v1/browser/sessions/{session_id}/release"
            ))
            .timeout(BROWSER_SESSION_RELEASE_TIMEOUT),
        &creds.device_token,
    )
    .send()
    .await?
    .error_for_status()?;
    Ok(())
}

fn short_error(error: &str) -> String {
    const MAX: usize = 300;
    let trimmed = error.trim();
    if trimmed.chars().count() <= MAX {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(MAX).collect();
    format!("{head}… (truncated)")
}
