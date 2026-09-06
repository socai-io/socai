//! Managed socai services shared by the CLI and desktop app.

mod asr;
mod auth;
mod billing;
mod browser;

pub use asr::{cloud_asr_access_rejected, transcribe_audio_file, CloudAsrResult};
pub use auth::{
    activate, activate_with_base_url, auth_session, hosted_llm_selected, llm_gateway_config,
    logout, pro_activated, redeem_invite, send_sms_code, set_hosted_llm_selected, status,
    take_hosted_llm_default, verify_sms_code, AuthSession, CloudCredentials, CloudStatus,
    InviteRedemption, LlmGatewayConfig, SmsChallengeResponse,
};
pub use billing::{
    create_alipay_order, create_wechat_order, mock_recharge, paid_asr_access, payment_order,
    payment_plan, settle_llm_task, wallet_balance, LlmSettlement, PaidAsrAccess, PaymentOrder,
    PaymentPlan, RechargeReceipt, WalletBalance,
};
pub use browser::{create_browser_session, release_browser_session, BrowserSessionInfo};

pub(crate) use auth::telemetry_account_snapshot;
