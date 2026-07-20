//! Managed socai services shared by the CLI and desktop app.

mod asr;
mod auth;
mod browser;

pub use asr::{transcribe_audio_file, CloudAsrResult};
pub use auth::{
    activate, activate_with_base_url, auth_session, auth_session_with_dev_login, logout,
    pro_activated, send_sms_code, status, verify_sms_code, AuthSession, CloudCredentials,
    CloudStatus, SmsChallengeResponse,
};
pub use browser::{create_browser_session, release_browser_session, BrowserSessionInfo};
