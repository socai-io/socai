//! Account-scoped scheduled research shared with the Web workspace.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::auth::{configured_base_url, http_client, load_credentials, require_success};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub id: String,
    pub title: String,
    pub prompt: String,
    pub platform: String,
    #[serde(default)]
    pub platforms: Vec<String>,
    pub scheduled_for: String,
    pub timezone: String,
    pub recurrence: String,
    pub status: String,
    pub conversation_id: Option<String>,
    pub screenshot_count: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTaskInput {
    pub prompt: String,
    pub platforms: Vec<String>,
    pub scheduled_for: String,
    pub timezone: String,
    pub recurrence: String,
    #[serde(default)]
    pub client_request_id: Option<String>,
}

fn request(method: reqwest::Method, path: &str) -> Result<reqwest::RequestBuilder> {
    let base_url = configured_base_url()
        .ok_or_else(|| anyhow::anyhow!("socai service URL is not configured"))?;
    let credentials = load_credentials()
        .filter(|value| !value.user_id.trim().is_empty() && !value.device_token.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("sign in to use scheduled tasks"))?;
    Ok(http_client()?
        .request(method, format!("{base_url}/v1/desktop/growth{path}"))
        .bearer_auth(credentials.device_token))
}

pub async fn scheduled_tasks() -> Result<Vec<ScheduledTask>> {
    let response = request(reqwest::Method::GET, "/schedules")?
        .send()
        .await
        .context("failed to load scheduled tasks")?;
    Ok(require_success(response, "scheduled tasks")
        .await?
        .json()
        .await?)
}

pub async fn create_scheduled_task(input: &ScheduledTaskInput) -> Result<ScheduledTask> {
    let response = request(reqwest::Method::POST, "/schedules")?
        .json(&serde_json::json!({
            "prompt": input.prompt,
            "platforms": input.platforms,
            "scheduled_for": input.scheduled_for,
            "timezone": input.timezone,
            "recurrence": input.recurrence,
            "client_request_id": input.client_request_id,
            "confirm_external_write": true,
        }))
        .send()
        .await
        .context("failed to create scheduled task")?;
    Ok(require_success(response, "scheduled task")
        .await?
        .json()
        .await?)
}

pub async fn update_scheduled_task(id: &str, input: &ScheduledTaskInput) -> Result<ScheduledTask> {
    let response = request(
        reqwest::Method::PATCH,
        &format!("/schedules/{}", safe_id(id)?),
    )?
    .json(&serde_json::json!({
        "prompt": input.prompt,
        "platforms": input.platforms,
        "scheduled_for": input.scheduled_for,
        "timezone": input.timezone,
        "recurrence": input.recurrence,
        "confirm_external_write": true,
    }))
    .send()
    .await
    .context("failed to update scheduled task")?;
    Ok(require_success(response, "scheduled task")
        .await?
        .json()
        .await?)
}

pub async fn cancel_scheduled_task(id: &str) -> Result<ScheduledTask> {
    let response = request(
        reqwest::Method::DELETE,
        &format!("/schedules/{}", safe_id(id)?),
    )?
    .send()
    .await
    .context("failed to cancel scheduled task")?;
    Ok(require_success(response, "scheduled task")
        .await?
        .json()
        .await?)
}

pub async fn delete_scheduled_task(id: &str) -> Result<()> {
    let response = request(
        reqwest::Method::DELETE,
        &format!("/schedules/{}/record", safe_id(id)?),
    )?
    .send()
    .await
    .context("failed to delete scheduled task")?;
    require_success(response, "scheduled task").await?;
    Ok(())
}

fn safe_id(value: &str) -> Result<&str> {
    if value.len() != 36
        || !value
            .chars()
            .all(|character| character.is_ascii_hexdigit() || character == '-')
    {
        anyhow::bail!("invalid scheduled task id");
    }
    Ok(value)
}
