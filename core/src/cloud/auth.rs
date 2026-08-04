use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::config;

const AUTH_KEY: &str = "socai_pro";
const LEGACY_AUTH_KEY: &str = "socai_cloud";
const LEGACY_PRO_BASE_URL: &str = "http://47.94.86.171";
const PRODUCTION_BASE_URL: &str = "https://api.socai.work";

#[derive(Debug, Clone, Serialize)]
pub struct AuthSession {
    pub logged_in: bool,
    pub user_id: String,
    pub phone: String,
    pub device_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteRedemption {
    pub status: String,
    pub added_points: i64,
    pub balance_points: i64,
    pub duration_days: i64,
    pub active_until: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmsChallengeResponse {
    pub challenge_id: String,
    pub expires_in_seconds: i64,
    pub retry_after_seconds: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CloudStatus {
    pub base_url: String,
    pub activated: bool,
    pub device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudCredentials {
    pub device_id: String,
    pub device_token: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub hosted_llm_default_applied: bool,
    #[serde(default)]
    pub hosted_llm_selected: bool,
    #[serde(default)]
    pub balance_points: Option<i64>,
    #[serde(default)]
    pub active_until: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct AccountTelemetrySnapshot {
    pub phone: String,
    pub balance_points: Option<i64>,
    pub active_until: Option<String>,
    pub pro_subscribed: bool,
}

#[derive(Debug, Clone)]
pub struct LlmGatewayConfig {
    pub base_url: String,
    pub device_token: String,
}

#[derive(Debug, Deserialize)]
struct LoginResponse {
    user_id: String,
    device_id: String,
    device_token: String,
    status: String,
}

#[derive(Debug, Deserialize)]
struct ActivateResponse {
    device_id: String,
    device_token: String,
}

/// Locally saved user session. Invite-code activations intentionally return a
/// logged-out session because they have a device token but no user identity.
pub fn auth_session() -> Result<AuthSession> {
    let Some(creds) = load_credentials() else {
        return Ok(logged_out_session());
    };
    if creds.user_id.trim().is_empty() || creds.phone.trim().is_empty() {
        return Ok(logged_out_session());
    }
    Ok(AuthSession {
        logged_in: true,
        user_id: creds.user_id,
        phone: creds.phone,
        device_id: creds.device_id,
        status: if creds.status.trim().is_empty() {
            "active".into()
        } else {
            creds.status
        },
    })
}

/// Whether socai pro is activated on this device. Local check only (reads
/// ~/.socai/auth.json, no network) — the server still authorizes every call.
pub fn pro_activated() -> bool {
    load_credentials().is_some_and(|creds| !creds.device_token.trim().is_empty())
}

pub fn status() -> Result<CloudStatus> {
    let base_url = configured_base_url().unwrap_or_default();
    let creds = load_credentials();
    Ok(CloudStatus {
        base_url,
        activated: creds
            .as_ref()
            .is_some_and(|c| !c.device_token.trim().is_empty()),
        device_id: creds.map(|c| c.device_id).unwrap_or_default(),
    })
}

pub async fn activate(invite_code: &str, label: &str) -> Result<CloudStatus> {
    let base_url = configured_base_url()
        .ok_or_else(|| anyhow::anyhow!("socai pro server URL is not configured"))?;
    activate_with_base_url(&base_url, invite_code, label).await
}

pub async fn activate_with_base_url(
    base_url: &str,
    invite_code: &str,
    label: &str,
) -> Result<CloudStatus> {
    let base_url = normalize_base_url(base_url)?;
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not resolve $HOME"))?;
    let install_id = crate::identity::load_or_create_install_id(&home.join(".socai"));
    let response = http_client()?
        .post(format!("{base_url}/v1/beta/activate"))
        .json(&json!({
            "invite_code": invite_code.trim(),
            "install_id": install_id,
            "app_version": env!("CARGO_PKG_VERSION"),
            "label": label.trim(),
        }))
        .send()
        .await
        .context("failed to call socai pro activation endpoint")?;
    let response = require_success(response, "socai pro activation").await?;
    let body: ActivateResponse = response.json().await?;
    config::set_config_key("cloud.base_url", &base_url)?;
    save_credentials(&CloudCredentials {
        device_id: body.device_id.clone(),
        device_token: body.device_token,
        user_id: String::new(),
        phone: String::new(),
        status: String::new(),
        hosted_llm_default_applied: false,
        hosted_llm_selected: false,
        balance_points: None,
        active_until: None,
    })?;
    Ok(CloudStatus {
        base_url,
        activated: true,
        device_id: body.device_id,
    })
}

pub async fn redeem_invite(invite_code: &str) -> Result<InviteRedemption> {
    let base_url = configured_base_url()
        .ok_or_else(|| anyhow::anyhow!("socai pro server URL is not configured"))?;
    let credentials = load_credentials()
        .filter(|creds| !creds.user_id.trim().is_empty() && !creds.device_token.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("sign in before entering an invite code"))?;
    let response = http_client()?
        .post(format!("{base_url}/v1/beta/redeem"))
        .bearer_auth(credentials.device_token)
        .json(&json!({ "invite_code": invite_code.trim() }))
        .send()
        .await
        .context("failed to redeem invite code")?;
    let redemption: InviteRedemption = require_success(response, "invite code")
        .await?
        .json()
        .await?;
    cache_wallet_snapshot(
        redemption.balance_points,
        Some(redemption.active_until.clone()),
    );
    Ok(redemption)
}

pub async fn send_sms_code(phone: &str) -> Result<SmsChallengeResponse> {
    let base_url = configured_base_url()
        .ok_or_else(|| anyhow::anyhow!("socai pro server URL is not configured"))?;
    let response = http_client()?
        .post(format!("{base_url}/v1/auth/sms/send"))
        .json(&json!({ "phone": phone.trim() }))
        .send()
        .await
        .context("failed to request an SMS code")?;
    let response = require_success(response, "SMS code request").await?;
    Ok(response.json().await?)
}

pub async fn verify_sms_code(
    challenge_id: &str,
    phone: &str,
    code: &str,
    label: &str,
) -> Result<AuthSession> {
    let base_url = configured_base_url()
        .ok_or_else(|| anyhow::anyhow!("socai pro server URL is not configured"))?;
    let canonical_phone = normalize_mainland_phone(phone)?;
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not resolve $HOME"))?;
    let install_id = crate::identity::load_or_create_install_id(&home.join(".socai"));
    let response = http_client()?
        .post(format!("{base_url}/v1/auth/sms/verify"))
        .json(&json!({
            "challenge_id": challenge_id.trim(),
            "phone": canonical_phone,
            "code": code.trim(),
            "install_id": install_id,
            "app_version": env!("CARGO_PKG_VERSION"),
            "label": label.trim(),
        }))
        .send()
        .await
        .context("failed to verify the SMS code")?;
    let response = require_success(response, "SMS verification").await?;
    let body: LoginResponse = response.json().await?;
    save_login_credentials(&base_url, canonical_phone, body)
}

fn save_login_credentials(
    base_url: &str,
    canonical_phone: String,
    body: LoginResponse,
) -> Result<AuthSession> {
    save_credentials(&CloudCredentials {
        device_id: body.device_id,
        device_token: body.device_token,
        user_id: body.user_id,
        phone: canonical_phone,
        status: body.status,
        hosted_llm_default_applied: false,
        hosted_llm_selected: false,
        balance_points: None,
        active_until: None,
    })?;
    // Keep the CLI and future app builds pointed at the same accepted service,
    // even when this build received the URL through SOCAI_PRO_BASE_URL. The
    // authenticated session is still usable in this build if config persistence
    // happens to fail after the credentials were saved.
    if let Err(err) = config::set_config_key("cloud.base_url", &base_url) {
        tracing::warn!(error = %err, "failed to persist cloud.base_url after login");
    }
    auth_session()
}

pub async fn logout() -> Result<()> {
    let credentials = load_credentials();
    let base_url = configured_base_url();
    let remote_result: Result<()> = async {
        if let (Some(creds), Some(base_url)) = (&credentials, base_url) {
            if creds.device_token.trim().is_empty() {
                return Ok(());
            }
            let response = bearer(
                http_client()?.post(format!("{base_url}/v1/auth/logout")),
                &creds.device_token,
            )
            .send()
            .await
            .context("failed to call logout endpoint")?;
            require_success(response, "logout").await.map(|_| ())
        } else {
            Ok(())
        }
    }
    .await;
    // Logging out must always take effect on this machine. If the remote call
    // failed, return that error after removing the local bearer token.
    let clear_result = clear_credentials();
    remote_result.and(clear_result)
}

pub fn llm_gateway_config() -> Result<LlmGatewayConfig> {
    let base_url = configured_base_url()
        .ok_or_else(|| anyhow::anyhow!("socai service URL is not configured"))?;
    let credentials = load_credentials()
        .filter(|creds| !creds.user_id.trim().is_empty() && !creds.device_token.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("sign in to use Socai Agent"))?;
    Ok(LlmGatewayConfig {
        base_url,
        device_token: credentials.device_token,
    })
}

/// Returns true once per saved account, allowing the desktop to select the
/// hosted model by default without overriding a later explicit BYOK choice.
pub fn take_hosted_llm_default() -> Result<bool> {
    let Some(mut credentials) = load_credentials() else {
        return Ok(false);
    };
    if credentials.user_id.trim().is_empty() || credentials.hosted_llm_default_applied {
        return Ok(false);
    }
    credentials.hosted_llm_default_applied = true;
    credentials.hosted_llm_selected = true;
    save_credentials(&credentials)?;
    Ok(true)
}

pub fn hosted_llm_selected() -> bool {
    load_credentials().is_some_and(|credentials| {
        !credentials.user_id.trim().is_empty() && credentials.hosted_llm_selected
    })
}

pub fn set_hosted_llm_selected(selected: bool) -> Result<()> {
    let Some(mut credentials) = load_credentials() else {
        return Ok(());
    };
    if credentials.user_id.trim().is_empty() {
        return Ok(());
    }
    credentials.hosted_llm_selected = selected;
    save_credentials(&credentials).map(|_| ())
}

pub(crate) fn telemetry_account_snapshot() -> Option<AccountTelemetrySnapshot> {
    let credentials = load_credentials()?;
    let phone = credentials.phone.trim().to_string();
    if credentials.user_id.trim().is_empty() || phone.is_empty() {
        return None;
    }
    let pro_subscribed = credentials
        .active_until
        .as_deref()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .is_some_and(|until| until > chrono::Utc::now());
    Some(AccountTelemetrySnapshot {
        phone,
        balance_points: credentials.balance_points,
        active_until: credentials.active_until,
        pro_subscribed,
    })
}

pub(super) fn cache_wallet_snapshot(balance_points: i64, active_until: Option<String>) {
    let Some(mut credentials) = load_credentials() else {
        return;
    };
    credentials.balance_points = Some(balance_points);
    credentials.active_until = active_until;
    let _ = save_credentials(&credentials);
}

pub(super) fn cache_balance_points(balance_points: i64) {
    let Some(mut credentials) = load_credentials() else {
        return;
    };
    credentials.balance_points = Some(balance_points);
    let _ = save_credentials(&credentials);
}

pub(super) fn cache_active_until(active_until: Option<String>) {
    let Some(mut credentials) = load_credentials() else {
        return;
    };
    credentials.active_until = active_until;
    let _ = save_credentials(&credentials);
}

fn logged_out_session() -> AuthSession {
    AuthSession {
        logged_in: false,
        user_id: String::new(),
        phone: String::new(),
        device_id: String::new(),
        status: String::new(),
    }
}

fn normalize_mainland_phone(value: &str) -> Result<String> {
    let mut compact: String = value
        .trim()
        .chars()
        .filter(|c| !matches!(c, ' ' | '(' | ')' | '-'))
        .collect();
    if let Some(rest) = compact.strip_prefix("+86") {
        compact = rest.to_string();
    } else if compact.len() == 13 {
        if let Some(rest) = compact.strip_prefix("86") {
            compact = rest.to_string();
        }
    }
    let valid_prefix = compact
        .as_bytes()
        .get(1)
        .is_some_and(|digit| matches!(digit, b'3'..=b'9'));
    if compact.len() != 11
        || !compact.starts_with('1')
        || !valid_prefix
        || !compact.bytes().all(|digit| digit.is_ascii_digit())
    {
        anyhow::bail!("invalid mainland China phone number");
    }
    Ok(format!("+86{compact}"))
}

pub(super) fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .build()?)
}

pub(super) fn bearer(request: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    request.bearer_auth(token.trim())
}

pub(super) async fn require_success(
    response: reqwest::Response,
    action: &str,
) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    anyhow::bail!("{action} failed ({status}): {body}")
}

pub(super) fn configured_base_url() -> Option<String> {
    if let Some(value) = env_base_url() {
        return Some(migrate_legacy_base_url(value, false));
    }
    let config = config::load_config().ok()?;
    let configured = config
        .cloud
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string());
    configured
        .map(|value| migrate_legacy_base_url(value, true))
        .or_else(|| compiled_base_url().map(|value| migrate_legacy_base_url(value, false)))
}

fn migrate_legacy_base_url(value: String, persist: bool) -> String {
    if value != LEGACY_PRO_BASE_URL {
        return value;
    }
    if persist {
        if let Err(err) = config::set_config_key("cloud.base_url", PRODUCTION_BASE_URL) {
            tracing::warn!(error = %err, "failed to migrate legacy cloud.base_url");
        }
    }
    PRODUCTION_BASE_URL.to_string()
}

fn normalize_base_url(value: &str) -> Result<String> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        anyhow::bail!("socai pro server URL is required");
    }
    let host = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"));
    if !host.is_some_and(|rest| !rest.is_empty()) {
        anyhow::bail!("socai pro server URL must start with http:// or https:// and name a host");
    }
    Ok(trimmed.to_string())
}

fn env_base_url() -> Option<String> {
    ["SOCAI_PRO_BASE_URL", "SOCAI_CLOUD_BASE_URL"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok())
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

fn compiled_base_url() -> Option<String> {
    option_env!("SOCAI_PRO_BASE_URL")
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

pub(super) fn load_credentials() -> Option<CloudCredentials> {
    let path = auth_path().ok()?;
    let bytes = std::fs::read(path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let block = value.get(AUTH_KEY).or_else(|| value.get(LEGACY_AUTH_KEY))?;
    serde_json::from_value(block.clone()).ok()
}

fn save_credentials(credentials: &CloudCredentials) -> Result<PathBuf> {
    let path = auth_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut data: Map<String, Value> = match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(map)) => map,
            _ => Map::new(),
        },
        Err(_) => Map::new(),
    };
    data.insert(AUTH_KEY.into(), serde_json::to_value(credentials)?);
    data.remove(LEGACY_AUTH_KEY);
    write_auth_file(&path, data)?;
    Ok(path)
}

fn clear_credentials() -> Result<()> {
    let path = auth_path()?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    let mut data: Map<String, Value> = match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    data.remove(AUTH_KEY);
    data.remove(LEGACY_AUTH_KEY);
    write_auth_file(&path, data)
}

fn auth_path() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not resolve $HOME"))?;
    Ok(home.join(".socai/auth.json"))
}

fn write_auth_file(path: &Path, data: Map<String, Value>) -> Result<()> {
    let rendered = serde_json::to_string_pretty(&Value::Object(data))?;
    std::fs::write(path, rendered)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}
