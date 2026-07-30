use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex};

use crate::cdp::endpoint::Endpoint;
use crate::cdp::launch::ChromeProcess;
use crate::cdp::raw_client::RawCdpClient;

const EVENT_CHANNEL_CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChromeProfile {
    /// Attach to an already-running browser/profile that exposes CDP.
    Existing,
    /// Launch or reuse socai's isolated chrome user-data-dir.
    Managed,
    /// Try managed first, then fall back to existing-browser discovery.
    Auto,
    /// Drive a remote hosted browser minted via socai-server (socai pro,
    /// beta).
    Remote,
}

impl ChromeProfile {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "existing" => Ok(Self::Existing),
            "managed" => Ok(Self::Managed),
            "auto" => Ok(Self::Auto),
            "remote" => Ok(Self::Remote),
            other => Err(anyhow::anyhow!(
                "invalid chrome profile {other:?}; expected existing, managed, auto, or remote"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Existing => "existing",
            Self::Managed => "managed",
            Self::Auto => "auto",
            Self::Remote => "remote",
        }
    }
}

impl Default for ChromeProfile {
    fn default() -> Self {
        Self::Existing
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChromeConnectOptions {
    pub profile: ChromeProfile,
    pub managed_user_data_dir: Option<PathBuf>,
}

impl ChromeConnectOptions {
    pub fn existing() -> Self {
        Self {
            profile: ChromeProfile::Existing,
            managed_user_data_dir: None,
        }
    }

    pub fn managed(user_data_dir: Option<PathBuf>) -> Self {
        Self {
            profile: ChromeProfile::Managed,
            managed_user_data_dir: user_data_dir,
        }
    }

    pub fn from_config() -> anyhow::Result<Self> {
        Ok(crate::config::load_config()?.chrome_connect_options())
    }
}

impl Default for ChromeConnectOptions {
    fn default() -> Self {
        Self::existing()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TargetInfo {
    pub target_id: String,
    pub r#type: String,
    pub title: String,
    pub url: String,
}

#[allow(clippy::large_enum_variant)] // only one CdpState exists at a time
pub enum CdpState {
    Disconnected {
        reason: String,
    },
    Connecting {
        attempt: u8,
    },
    Connected {
        /// Remote-debugging endpoint. The endpoint may be discovered through
        /// HTTP (`/json/version`) or `DevToolsActivePort`, but active target
        /// inventory/lifecycle always uses `browser_client` and raw `Target.*`
        /// commands without chromiumoxide's global auto-init.
        endpoint: Endpoint,
        browser_client: RawCdpClient,
        browser_version: String,
        targets: HashMap<String, TargetInfo>,
        monitor_task: tokio::task::AbortHandle,
        /// Browser resource socai owns for this connection. Held here so it is
        /// torn down on drop — disconnect, reconnect, or daemon shutdown all
        /// replace this state and release the browser, mirroring the old
        /// chromiumoxide `Browser` drop semantics.
        owner: BrowserOwner,
    },
}

/// What socai owns behind the current connection, torn down when the
/// `Connected` state is replaced. The variant payloads exist for their `Drop`
/// side effects (kill the launched chrome / release the minted session).
pub enum BrowserOwner {
    /// Attached to a browser socai does not own (the user's existing profile,
    /// a reused managed chrome, or an explicit endpoint) — nothing to release.
    None,
    /// Managed chrome process socai launched; killed on drop.
    Local(ChromeProcess),
    /// Remote hosted browser session socai minted via socai-server. Released
    /// awaited on the disconnect/cancel paths (see `release_owner_now`);
    /// `Drop` spawns a best-effort release for the remaining state swaps,
    /// with the server-side session timeout as the final backstop.
    Remote(RemoteSession),
}

pub struct RemoteSession {
    pub session_id: String,
    /// Hard end-of-life for this session: the server mints it with a fixed
    /// timeout and Browserbase kills it at that instant no matter what the
    /// client is doing. The runtime uses this to re-mint before starting new
    /// work on a session that is nearly out of budget.
    pub deadline: std::time::Instant,
}

impl RemoteSession {
    /// Hand the session id to a caller that will release it explicitly,
    /// leaving `Drop` a no-op. `None` if the id was already taken.
    pub(crate) fn take_session_id(&mut self) -> Option<String> {
        let session_id = std::mem::take(&mut self.session_id);
        (!session_id.is_empty()).then_some(session_id)
    }
}

impl Drop for RemoteSession {
    fn drop(&mut self) {
        // Backstop only: `Cdp::disconnect` takes the id and awaits the release
        // itself. This path covers remaining state swaps (connection loss,
        // cancelled connects), where the process is staying alive and the
        // spawned task can finish.
        let session_id = std::mem::take(&mut self.session_id);
        if session_id.is_empty() {
            return;
        }
        // Outside a tokio runtime (process teardown) the release is skipped;
        // the server-side session timeout reaps it instead.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = crate::cloud::release_browser_session(&session_id).await;
            });
        }
    }
}

impl CdpState {
    pub fn initial() -> Self {
        Self::Disconnected {
            reason: "not_yet_connected".into(),
        }
    }

    pub fn is_disconnected(&self) -> bool {
        matches!(self, Self::Disconnected { .. })
    }

    pub fn is_connecting(&self) -> bool {
        matches!(self, Self::Connecting { .. })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum StatusPayload {
    Disconnected {
        reason: String,
    },
    Connecting {
        attempt: u8,
    },
    Connected {
        endpoint: String,
        browser_version: String,
        page_count: usize,
        source: String,
        managed: bool,
        /// True when socai minted this connection's browser remotely via
        /// socai-server (chrome.profile `remote`).
        remote: bool,
        user_data_dir: Option<String>,
    },
}

impl From<&CdpState> for StatusPayload {
    fn from(state: &CdpState) -> Self {
        match state {
            CdpState::Disconnected { reason } => Self::Disconnected {
                reason: reason.clone(),
            },
            CdpState::Connecting { attempt } => Self::Connecting { attempt: *attempt },
            CdpState::Connected {
                endpoint,
                browser_version,
                targets,
                owner,
                ..
            } => Self::Connected {
                // This payload crosses the Tauri IPC boundary on every status
                // change, so a credential-bearing endpoint is redacted at the
                // source rather than merely hidden by the UI. `display_ws_url`
                // covers both minted sessions and credential-shaped overrides.
                endpoint: endpoint.display_ws_url(),
                browser_version: browser_version.clone(),
                page_count: targets.values().filter(|t| t.r#type == "page").count(),
                source: endpoint.source.clone(),
                managed: endpoint.managed,
                // Ownership, not URL shape: `remote` drives teardown/release
                // and profile matching, so it means "socai minted this".
                remote: matches!(owner, BrowserOwner::Remote(_)),
                user_data_dir: endpoint.user_data_dir.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserEvent {
    StatusChanged(StatusPayload),
    TargetsChanged(Vec<TargetInfo>),
}

/// Connection state + event broadcast. Cheaply cloneable; each clone shares the
/// same underlying state and broadcast sender.
#[derive(Clone)]
pub struct Cdp {
    state: Arc<Mutex<CdpState>>,
    events: broadcast::Sender<BrowserEvent>,
    owned_targets: Arc<Mutex<HashSet<String>>>,
    /// Held for the lifetime of a connect loop so only one runs at a time.
    /// Reading the state cannot provide that on its own: two callers can both
    /// observe `Disconnected` before either transitions, and each would then
    /// acquire its own browser — for a hosted profile, its own billed session.
    connect_lock: Arc<Mutex<()>>,
    /// Serializes `disconnect()` end to end. Without it, a second disconnect
    /// (the idle reaper racing an app quit, say) finds no owner in the state,
    /// returns immediately, and lets process exit abort the first caller's
    /// still-in-flight session release.
    teardown_lock: Arc<Mutex<()>>,
}

impl Cdp {
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Self {
            state: Arc::new(Mutex::new(CdpState::initial())),
            events,
            owned_targets: Arc::new(Mutex::new(HashSet::new())),
            connect_lock: Arc::new(Mutex::new(())),
            teardown_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BrowserEvent> {
        self.events.subscribe()
    }

    pub async fn status(&self) -> StatusPayload {
        (&*self.state.lock().await).into()
    }

    pub async fn pages(&self) -> Vec<TargetInfo> {
        match &*self.state.lock().await {
            CdpState::Connected { targets, .. } => page_list_from(targets),
            _ => Vec::new(),
        }
    }

    pub(crate) async fn browser_client(&self) -> Option<RawCdpClient> {
        match &*self.state.lock().await {
            CdpState::Connected { browser_client, .. } => Some(browser_client.clone()),
            _ => None,
        }
    }

    /// The browser websocket plus whether it belongs to a remote hosted
    /// browser, read under one lock. Page creation must learn both together:
    /// deriving remote-ness in a second lookup could observe a different
    /// connection if a disconnect/reconnect lands in between.
    pub(crate) async fn browser_client_with_mode(&self) -> Option<(RawCdpClient, bool)> {
        match &*self.state.lock().await {
            CdpState::Connected {
                browser_client,
                owner,
                ..
            } => Some((
                browser_client.clone(),
                matches!(owner, BrowserOwner::Remote(_)),
            )),
            _ => None,
        }
    }

    /// End-of-life instant of the current remote hosted session; `None` when
    /// not connected or the browser is not a socai-minted remote session.
    pub(crate) async fn remote_session_deadline(&self) -> Option<std::time::Instant> {
        match &*self.state.lock().await {
            CdpState::Connected {
                owner: BrowserOwner::Remote(session),
                ..
            } => Some(session.deadline),
            _ => None,
        }
    }

    pub(crate) async fn register_owned_target(&self, target_id: impl Into<String>) {
        self.owned_targets.lock().await.insert(target_id.into());
    }

    pub(crate) async fn unregister_owned_target(&self, target_id: &str) {
        self.owned_targets.lock().await.remove(target_id);
    }

    pub(crate) async fn take_owned_targets(&self) -> Vec<String> {
        self.owned_targets.lock().await.drain().collect()
    }

    /// Block until status transitions to Connected, or surface Disconnected
    /// as an error. Subscribes before checking current state so we never miss
    /// an event that fires between subscribe and check.
    pub async fn wait_connected(&self) -> anyhow::Result<()> {
        let mut rx = self.subscribe();
        if let StatusPayload::Connected { .. } = self.status().await {
            return Ok(());
        }
        loop {
            match rx.recv().await {
                Ok(BrowserEvent::StatusChanged(StatusPayload::Connected { .. })) => return Ok(()),
                Ok(BrowserEvent::StatusChanged(StatusPayload::Disconnected { reason })) => {
                    return Err(anyhow::anyhow!("disconnected: {reason}"));
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(anyhow::anyhow!("event channel closed"));
                }
            }
        }
    }

    pub(crate) fn state(&self) -> Arc<Mutex<CdpState>> {
        Arc::clone(&self.state)
    }

    pub(crate) fn connect_lock(&self) -> Arc<Mutex<()>> {
        Arc::clone(&self.connect_lock)
    }

    pub(crate) fn teardown_lock(&self) -> Arc<Mutex<()>> {
        Arc::clone(&self.teardown_lock)
    }

    pub(crate) fn emit(&self, event: BrowserEvent) {
        let _ = self.events.send(event);
    }
}

impl Default for Cdp {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn page_list_from(targets: &HashMap<String, TargetInfo>) -> Vec<TargetInfo> {
    let mut pages: Vec<TargetInfo> = targets
        .values()
        .filter(|t| t.r#type == "page")
        .cloned()
        .collect();
    pages.sort_by(|a, b| a.target_id.cmp(&b.target_id));
    pages
}
