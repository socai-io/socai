use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::auth::{configured_base_url, http_client, load_credentials, require_success};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletBalance {
    pub balance_points: i64,
    pub points_per_cny: i64,
    pub starter_points: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RechargeReceipt {
    pub balance_points: i64,
    pub points_per_cny: i64,
    pub starter_points: i64,
    pub order_id: String,
    pub added_points: i64,
    pub amount_fen: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmSettlement {
    pub status: String,
    pub billed_points: i64,
    pub balance_points: i64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

fn authenticated_request(method: reqwest::Method, path: &str) -> Result<reqwest::RequestBuilder> {
    let base_url = configured_base_url()
        .ok_or_else(|| anyhow::anyhow!("socai service URL is not configured"))?;
    let credentials = load_credentials()
        .filter(|creds| !creds.user_id.trim().is_empty() && !creds.device_token.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("sign in to use socai cloud"))?;
    Ok(http_client()?
        .request(method, format!("{base_url}{path}"))
        .bearer_auth(credentials.device_token))
}

pub async fn wallet_balance() -> Result<WalletBalance> {
    let response = authenticated_request(reqwest::Method::GET, "/v1/billing/wallet")?
        .send()
        .await
        .context("failed to load point balance")?;
    Ok(require_success(response, "point balance")
        .await?
        .json()
        .await?)
}

pub async fn mock_recharge(points: i64, request_id: &str) -> Result<RechargeReceipt> {
    let response = authenticated_request(reqwest::Method::POST, "/v1/billing/mock-recharge")?
        .json(&json!({"points": points, "request_id": request_id}))
        .send()
        .await
        .context("failed to mock recharge")?;
    Ok(require_success(response, "mock recharge")
        .await?
        .json()
        .await?)
}

pub async fn settle_llm_task(task_id: &str, final_status: &str) -> Result<LlmSettlement> {
    if !task_id
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        anyhow::bail!("invalid hosted LLM task id");
    }
    if !matches!(
        final_status,
        "completed" | "failed" | "cancelled" | "interrupted"
    ) {
        anyhow::bail!("invalid hosted LLM final status");
    }
    let path = format!("/v1/llm/tasks/{task_id}/settle?final_status={final_status}");
    let response = authenticated_request(reqwest::Method::POST, &path)?
        .send()
        .await
        .context("failed to settle hosted LLM task")?;
    Ok(require_success(response, "hosted LLM settlement")
        .await?
        .json()
        .await?)
}
