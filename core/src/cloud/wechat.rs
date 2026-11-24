//! WeChat desktop OAuth in the system browser, completed via www.socai.work callback + polling.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::json;

use super::auth::{self, AuthSession, LoginResponse};

#[derive(Deserialize)]
struct WechatStartResponse {
    authorize_url: String,
    poll_token: String,
    #[serde(default = "default_poll_after")]
    poll_after_seconds: u64,
}

#[derive(Deserialize)]
struct WechatPollResponse {
    status: String,
    #[serde(default = "default_poll_after")]
    poll_after_seconds: u64,
    login: Option<LoginResponse>,
}

fn default_poll_after() -> u64 {
    2
}

pub async fn sign_in_with_wechat(
    link: bool,
    open_browser: impl FnOnce(&str) -> Result<()>,
) -> Result<AuthSession> {
    tokio::time::timeout(Duration::from_secs(300), login(link, open_browser))
        .await
        .context("WeChat login timed out")?
}

async fn login(link: bool, open_browser: impl FnOnce(&str) -> Result<()>) -> Result<AuthSession> {
    let base_url = auth::configured_base_url()
        .ok_or_else(|| anyhow::anyhow!("socai pro server URL is not configured"))?;
    let credentials = if link {
        Some(
            auth::load_credentials()
                .filter(|c| !c.user_id.is_empty() && !c.device_token.is_empty())
                .ok_or_else(|| anyhow::anyhow!("sign in before linking WeChat"))?,
        )
    } else {
        None
    };
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not resolve $HOME"))?;
    let install_id = crate::identity::load_or_create_install_id(&home.join(".socai"));
    let client = auth::http_client()?;
    let endpoint = if link { "link/start" } else { "start" };
    let mut start_request = client
        .post(format!("{base_url}/v1/auth/wechat/{endpoint}"))
        .json(&json!({
            "install_id": install_id,
            "app_version": env!("CARGO_PKG_VERSION"),
            "label": "desktop",
        }));
    if let Some(creds) = credentials.as_ref() {
        start_request = start_request.bearer_auth(&creds.device_token);
    }
    let start_response = start_request
        .send()
        .await
        .context("failed to start WeChat login")?;
    if matches!(start_response.status().as_u16(), 404 | 503) {
        bail!("WeChat login is not configured");
    }
    let start: WechatStartResponse = auth::require_success(start_response, "WeChat login start")
        .await?
        .json()
        .await?;
    if start.authorize_url.trim().is_empty() || start.poll_token.trim().is_empty() {
        bail!("WeChat login is not configured");
    }

    open_browser(start.authorize_url.as_str())?;

    let mut wait_seconds = start.poll_after_seconds.clamp(1, 10);
    loop {
        tokio::time::sleep(Duration::from_secs(wait_seconds)).await;
        let poll_response = client
            .post(format!("{base_url}/v1/auth/wechat/poll"))
            .json(&json!({ "poll_token": start.poll_token }))
            .send()
            .await
            .context("WeChat login status check failed")?;
        let poll: WechatPollResponse = auth::require_success(poll_response, "WeChat login")
            .await?
            .json()
            .await?;
        match poll.status.as_str() {
            "pending" => {
                wait_seconds = poll.poll_after_seconds.clamp(1, 10);
            }
            "approved" => {
                let body = poll
                    .login
                    .ok_or_else(|| anyhow::anyhow!("invalid WeChat login response"))?;
                return auth::save_login_credentials(&base_url, String::new(), body);
            }
            other => bail!("unexpected WeChat login status: {other}"),
        }
    }
}
