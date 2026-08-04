use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::auth::{
    cache_active_until, cache_balance_points, cache_wallet_snapshot, configured_base_url,
    http_client, load_credentials, require_success,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletBalance {
    pub balance_points: i64,
    pub points_per_cny: i64,
    pub starter_points: i64,
    pub active_until: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentPlan {
    pub enabled: bool,
    pub wechat_enabled: bool,
    pub alipay_enabled: bool,
    pub plan_id: String,
    pub name: String,
    pub amount_fen: i64,
    pub points: i64,
    pub duration_days: i64,
    pub auto_renews: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentOrder {
    pub order_id: String,
    pub status: String,
    pub code_url: Option<String>,
    pub payment_url: Option<String>,
    pub amount_fen: i64,
    pub points: i64,
    pub duration_days: i64,
    pub expires_at: Option<String>,
    pub paid_at: Option<String>,
    pub active_until: Option<String>,
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
        .ok_or_else(|| anyhow::anyhow!("sign in to use Socai Agent"))?;
    Ok(http_client()?
        .request(method, format!("{base_url}{path}"))
        .bearer_auth(credentials.device_token))
}

pub async fn wallet_balance() -> Result<WalletBalance> {
    let response = authenticated_request(reqwest::Method::GET, "/v1/billing/wallet")?
        .send()
        .await
        .context("failed to load point balance")?;
    let wallet: WalletBalance = require_success(response, "point balance")
        .await?
        .json()
        .await?;
    cache_wallet_snapshot(wallet.balance_points, wallet.active_until.clone());
    Ok(wallet)
}

pub async fn payment_plan() -> Result<PaymentPlan> {
    let response = authenticated_request(reqwest::Method::GET, "/v1/billing/plan")?
        .send()
        .await
        .context("failed to load payment plan")?;
    Ok(require_success(response, "payment plan")
        .await?
        .json()
        .await?)
}

pub async fn create_wechat_order(plan_id: &str, request_id: &str) -> Result<PaymentOrder> {
    let response = authenticated_request(reqwest::Method::POST, "/v1/billing/wechat/orders")?
        .json(&json!({"plan_id": plan_id, "request_id": request_id}))
        .send()
        .await
        .context("failed to create WeChat Pay order")?;
    Ok(require_success(response, "WeChat Pay order")
        .await?
        .json()
        .await?)
}

pub async fn create_alipay_order(plan_id: &str, request_id: &str) -> Result<PaymentOrder> {
    let response = authenticated_request(reqwest::Method::POST, "/v1/billing/alipay/orders")?
        .json(&json!({"plan_id": plan_id, "request_id": request_id}))
        .send()
        .await
        .context("failed to create Alipay order")?;
    Ok(require_success(response, "Alipay order")
        .await?
        .json()
        .await?)
}

pub async fn payment_order(order_id: &str) -> Result<PaymentOrder> {
    if !order_id
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        anyhow::bail!("invalid payment order id");
    }
    let path = format!("/v1/billing/orders/{order_id}");
    let response = authenticated_request(reqwest::Method::GET, &path)?
        .send()
        .await
        .context("failed to load payment order")?;
    let order: PaymentOrder = require_success(response, "payment order")
        .await?
        .json()
        .await?;
    if order.status == "paid" {
        cache_active_until(order.active_until.clone());
    }
    Ok(order)
}

pub async fn mock_recharge(points: i64, request_id: &str) -> Result<RechargeReceipt> {
    let response = authenticated_request(reqwest::Method::POST, "/v1/billing/mock-recharge")?
        .json(&json!({"points": points, "request_id": request_id}))
        .send()
        .await
        .context("failed to mock recharge")?;
    let receipt: RechargeReceipt = require_success(response, "mock recharge")
        .await?
        .json()
        .await?;
    cache_balance_points(receipt.balance_points);
    Ok(receipt)
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
    let settlement: LlmSettlement = require_success(response, "hosted LLM settlement")
        .await?
        .json()
        .await?;
    cache_balance_points(settlement.balance_points);
    Ok(settlement)
}
