use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::config;

const AUTH_KEY: &str = "socai_pro";
const LEGACY_AUTH_KEY: &str = "socai_cloud";
const ASR_TASK_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Consecutive poll failures tolerated before giving up on a submitted task.
const ASR_MAX_POLL_FAILURES: u32 = 3;

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
}

#[derive(Debug, Deserialize)]
struct ActivateResponse {
    device_id: String,
    device_token: String,
}

#[derive(Debug, Deserialize)]
struct UploadUrlResponse {
    task_id: String,
    upload_url: String,
    headers: std::collections::HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct TaskResponse {
    status: String,
    transcript: Option<String>,
    error: Option<String>,
    provider_latency_ms: i64,
}

#[derive(Debug, Clone)]
pub struct CloudAsrResult {
    pub transcript: String,
    pub provider_latency_ms: i64,
    pub total_latency_ms: u128,
}

/// Whether socai pro is activated on this device. Local check only (reads
/// ~/.socai/auth.json, no network) — gates which pro-only tool args are
/// exposed to the agent; the server still authorizes every actual call.
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
    let client = http_client()?;
    let response = client
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
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("socai pro activation failed ({status}): {body}");
    }
    let body: ActivateResponse = response.json().await?;
    // Persist the URL only after the server accepted the activation, so a
    // failed attempt doesn't leave the config pointing at a bad server.
    config::set_config_key("cloud.base_url", &base_url)?;
    save_credentials(&CloudCredentials {
        device_id: body.device_id.clone(),
        device_token: body.device_token,
    })?;
    Ok(CloudStatus {
        base_url,
        activated: true,
        device_id: body.device_id,
    })
}

pub async fn transcribe_audio_file(
    path: &Path,
    duration_s: i64,
    timeout: Duration,
) -> Result<CloudAsrResult> {
    let base_url = configured_base_url()
        .ok_or_else(|| anyhow::anyhow!("socai pro server URL is not configured"))?;
    let creds = load_credentials().ok_or_else(|| {
        anyhow::anyhow!("socai pro is not activated; run `socai pro activate <invite_code>`")
    })?;
    // The extension matters: the server passes the filename through to
    // DashScope, which detects the audio format from it.
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("audio.aac");
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;
    let client = http_client()?;
    let started = Instant::now();
    let upload: UploadUrlResponse = bearer(
        client
            .post(format!("{base_url}/v1/asr/upload-url"))
            .json(&json!({
                "filename": filename,
                "content_type": "audio/aac",
                "size_bytes": bytes.len(),
                "duration_s": duration_s.max(0),
            })),
        &creds.device_token,
    )
    .send()
    .await?
    .error_for_status()?
    .json()
    .await?;

    let mut put = client.put(&upload.upload_url).body(bytes);
    for (key, value) in &upload.headers {
        put = put.header(key, value);
    }
    put.send().await?.error_for_status()?;

    bearer(
        client.post(format!("{base_url}/v1/asr/tasks/{}/submit", upload.task_id)),
        &creds.device_token,
    )
    .send()
    .await?
    .error_for_status()?;

    let deadline = Instant::now() + timeout;
    let mut poll_failures: u32 = 0;
    loop {
        // The upload is already done at this point; tolerate a few transient
        // poll errors (5xx, network blips) instead of abandoning the task.
        match poll_task(&client, &base_url, &upload.task_id, &creds.device_token).await {
            Ok(task) => {
                poll_failures = 0;
                match task.status.as_str() {
                    "succeeded" => {
                        return Ok(CloudAsrResult {
                            transcript: task.transcript.unwrap_or_default(),
                            provider_latency_ms: task.provider_latency_ms,
                            total_latency_ms: started.elapsed().as_millis(),
                        });
                    }
                    "failed" => {
                        let error = task.error.unwrap_or_else(|| "unknown error".into());
                        // Fun-ASR reports an audio track it decoded fine but
                        // heard no speech in (music/SFX-only videos) as a
                        // failure. Surface the meaning, not the provider blob,
                        // so the agent tells the user the video has no
                        // narration instead of inventing a tooling problem.
                        if error.contains("ASR_RESPONSE_HAVE_NO_WORDS") {
                            anyhow::bail!(
                                "no speech detected in the video's audio track (it likely \
                                 contains only music or sound effects), so there is no \
                                 spoken content to transcribe"
                            );
                        }
                        anyhow::bail!("socai pro ASR failed: {}", short_error(&error));
                    }
                    _ => {}
                }
            }
            Err(err) => {
                poll_failures += 1;
                if poll_failures >= ASR_MAX_POLL_FAILURES {
                    return Err(err.context("socai pro ASR status polling failed"));
                }
            }
        }
        if Instant::now() >= deadline {
            anyhow::bail!("socai pro ASR timed out after {}s", timeout.as_secs());
        }
        tokio::time::sleep(ASR_TASK_POLL_INTERVAL).await;
    }
}

/// Trim a provider error to something an agent can read. Raw DashScope task
/// dumps run to kilobytes of nested JSON with signed URLs; the leading part
/// carries the task id + failure code, which is all a transcript_error needs.
fn short_error(error: &str) -> String {
    const MAX: usize = 300;
    let trimmed = error.trim();
    if trimmed.chars().count() <= MAX {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(MAX).collect();
    format!("{head}… (truncated)")
}

async fn poll_task(
    client: &reqwest::Client,
    base_url: &str,
    task_id: &str,
    token: &str,
) -> Result<TaskResponse> {
    Ok(bearer(
        client.get(format!("{base_url}/v1/asr/tasks/{task_id}")),
        token,
    )
    .send()
    .await?
    .error_for_status()?
    .json()
    .await?)
}

fn http_client() -> Result<reqwest::Client> {
    // Cap every request so a hung server can't stall an agent run (the poll
    // deadline is only checked between requests).
    Ok(reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .build()?)
}

fn bearer(request: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    request.bearer_auth(token.trim())
}

fn configured_base_url() -> Option<String> {
    if let Some(value) = env_base_url() {
        return Some(value);
    }
    let config = config::load_config().ok()?;
    let configured = config
        .cloud
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string());
    configured.or_else(compiled_base_url)
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

fn auth_path() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not resolve $HOME"))?;
    Ok(home.join(".socai/auth.json"))
}

fn load_credentials() -> Option<CloudCredentials> {
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
    let rendered = serde_json::to_string_pretty(&Value::Object(data))?;
    std::fs::write(&path, rendered)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}
