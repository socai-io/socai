//! Managed socai services shared by the CLI and desktop app.

mod asr;
mod auth;
mod browser;
mod billing;

pub use asr::{transcribe_audio_file, CloudAsrResult};
pub use auth::{
    activate, activate_with_base_url, auth_session, hosted_llm_selected, llm_gateway_config,
    logout, pro_activated, send_sms_code, set_hosted_llm_selected, status, take_hosted_llm_default,
    verify_sms_code, AuthSession, CloudCredentials, CloudStatus, LlmGatewayConfig,
    SmsChallengeResponse,
};
pub use billing::{
    mock_recharge, settle_llm_task, wallet_balance, LlmSettlement, RechargeReceipt, WalletBalance,
};
pub use browser::{create_browser_session, release_browser_session, BrowserSessionInfo};
