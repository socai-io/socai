pub mod connection;
pub mod endpoint;
pub mod lifecycle;
pub mod pages;
pub mod session;
pub mod snapshot;

pub use self::connection::{
    BrowserEvent, Cdp, CdpState, ChromeConnectOptions, ChromeProfile, StatusPayload, TargetInfo,
};
pub use self::endpoint::{
    discover_existing_chrome_endpoint, managed_chrome_user_data_dir, open_remote_debugging_page,
    resolve_explicit_endpoint, wait_for_existing_chrome_endpoint, Endpoint,
};
pub use self::pages::PageSessionManager;
pub use self::session::PageSession;
pub use self::snapshot::{with_snapshot_recording, SnapshotRecorder};
