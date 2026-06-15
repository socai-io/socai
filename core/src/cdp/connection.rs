use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chromiumoxide::Browser;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex};

use crate::cdp::endpoint::Endpoint;

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
}

impl ChromeProfile {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "existing" => Ok(Self::Existing),
            "managed" => Ok(Self::Managed),
            "auto" => Ok(Self::Auto),
            other => Err(anyhow::anyhow!(
                "invalid chrome profile {other:?}; expected existing, managed, or auto"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Existing => "existing",
            Self::Managed => "managed",
            Self::Auto => "auto",
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
        #[allow(dead_code)] // held to tie WS lifetime to the variant
        browser: Arc<Browser>,
        /// Abort handle for the spawned WS pump task. On user disconnect we
        /// must abort it so the underlying WebSocket actually closes — Chrome
        /// keeps the "controlled by automated software" banner up until every
        /// CDP client disconnects.
        handler_task: tokio::task::AbortHandle,
        endpoint: Endpoint,
        browser_version: String,
        targets: HashMap<String, TargetInfo>,
    },
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
                ..
            } => Self::Connected {
                endpoint: endpoint.browser_ws_url.clone(),
                browser_version: browser_version.clone(),
                page_count: targets.values().filter(|t| t.r#type == "page").count(),
                source: endpoint.source.clone(),
                managed: endpoint.managed,
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
}

impl Cdp {
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Self {
            state: Arc::new(Mutex::new(CdpState::initial())),
            events,
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
