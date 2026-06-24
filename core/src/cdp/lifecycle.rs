use std::collections::HashMap;
use std::time::Duration;

use serde_json::{json, Value};
use tracing::{debug, warn};

use crate::cdp::connection::{
    page_list_from, BrowserEvent, Cdp, CdpState, ChromeConnectOptions, ChromeProfile,
    StatusPayload, TargetInfo,
};
use crate::cdp::endpoint::{self, Endpoint};
use crate::cdp::launch::{self, ChromeProcess};
use crate::cdp::raw_client::RawCdpClient;

const MAX_ATTEMPTS: u8 = 3;
const ATTEMPT_DELAY: Duration = Duration::from_millis(500);
const TARGET_POLL_INTERVAL: Duration = Duration::from_secs(2);
const TARGET_POLL_FAILURES: u8 = 3;

struct ConnectInventory {
    targets: HashMap<String, TargetInfo>,
    browser_version: String,
    browser_client: RawCdpClient,
}

/// A resolved remote-debugging endpoint plus, for managed mode, the chrome
/// process socai launched. The process guard is threaded into the `Connected`
/// state so it lives exactly as long as the connection.
struct OpenEndpoint {
    endpoint: Endpoint,
    chrome_process: Option<ChromeProcess>,
}

impl Cdp {
    /// Trigger an asynchronous connect attempt. Idempotent: if already
    /// connected or connecting, returns immediately.
    pub fn connect(&self) {
        let cdp = self.clone();
        tokio::spawn(async move {
            run_connect(cdp, None).await;
        });
    }

    /// Trigger a connect attempt with explicit chrome profile selection.
    /// `connect()` uses ~/.socai/config.json (or the default existing-profile
    /// attach mode); this method is for internal callers that have already
    /// resolved a chrome profile preference.
    pub fn connect_with_options(&self, options: ChromeConnectOptions) {
        let cdp = self.clone();
        tokio::spawn(async move {
            run_connect(cdp, Some(options)).await;
        });
    }

    pub async fn disconnect(&self) {
        // Close socai-owned page targets before dropping endpoint state. All
        // target lifecycle is routed through the browser websocket so existing
        // and managed Chrome share the same cleanup path.
        let browser_client = self.browser_client().await;
        for target_id in self.take_owned_targets().await {
            if let Some(client) = browser_client.as_ref() {
                let _ = close_target_via_browser_ws(client, &target_id).await;
            }
        }
        transition_unconditional(
            self,
            CdpState::Disconnected {
                reason: "user_disconnected".into(),
            },
        )
        .await;
    }
}

async fn run_connect(cdp: Cdp, options: Option<ChromeConnectOptions>) {
    {
        let state = cdp.state();
        let guard = state.lock().await;
        if !guard.is_disconnected() {
            return;
        }
    }

    for attempt in 1..=MAX_ATTEMPTS {
        if !transition_if_eligible(&cdp, CdpState::Connecting { attempt }).await {
            return;
        }
        match try_connect_once(&cdp, options.clone()).await {
            Ok(()) => return,
            Err(err) => {
                warn!(attempt, error = %err, "cdp connect attempt failed");
                if attempt == MAX_ATTEMPTS {
                    transition_unconditional(
                        &cdp,
                        CdpState::Disconnected {
                            reason: err.to_string(),
                        },
                    )
                    .await;
                    return;
                }
                tokio::time::sleep(ATTEMPT_DELAY).await;
            }
        }
    }
}

async fn try_connect_once(cdp: &Cdp, options: Option<ChromeConnectOptions>) -> anyhow::Result<()> {
    let OpenEndpoint {
        endpoint,
        chrome_process,
    } = open_endpoint(options).await?;

    let inventory = connect_inventory(&endpoint).await?;
    let monitor_task = spawn_target_poll_loop(cdp.clone(), inventory.browser_client.clone());

    {
        let state = cdp.state();
        let mut guard = state.lock().await;
        if !guard.is_connecting() {
            monitor_task.abort();
            // Dropping `chrome_process` here kills a just-launched managed chrome
            // if the connect was cancelled mid-flight.
            return Err(anyhow::anyhow!("connect cancelled"));
        }
        *guard = CdpState::Connected {
            endpoint,
            browser_client: inventory.browser_client,
            browser_version: inventory.browser_version,
            targets: inventory.targets,
            monitor_task,
            chrome_process,
        };
        let payload: StatusPayload = (&*guard).into();
        cdp.emit(BrowserEvent::StatusChanged(payload));
    }

    // Initial targets emit so subscribers can hydrate. Future updates come from
    // lightweight raw `Target.getTargets` polling over the browser websocket.
    // This does not enable target discovery and does not attach to user-owned
    // tabs.
    let initial_pages = cdp.pages().await;
    cdp.emit(BrowserEvent::TargetsChanged(initial_pages));

    Ok(())
}

/// Resolve the remote-debugging endpoint for this connect attempt, honoring the
/// chrome profile preference. Explicit `SOCAI_CDP_*` / argument endpoints win
/// for non-managed modes; managed mode launches (or reuses) socai's own chrome.
async fn open_endpoint(options: Option<ChromeConnectOptions>) -> anyhow::Result<OpenEndpoint> {
    let options = match options {
        Some(options) => options,
        None => ChromeConnectOptions::from_config()?,
    };

    if !matches!(options.profile, ChromeProfile::Managed) {
        if let Some(endpoint) = endpoint::resolve_explicit_endpoint(None, None).await? {
            return Ok(OpenEndpoint {
                endpoint,
                chrome_process: None,
            });
        }
    }

    match options.profile {
        ChromeProfile::Managed => open_managed_endpoint(&options).await,
        ChromeProfile::Existing => open_existing_endpoint().await,
        ChromeProfile::Auto => match open_managed_endpoint(&options).await {
            Ok(open) => Ok(open),
            Err(managed_err) => {
                warn!(error = %managed_err, "managed chrome launch failed; falling back to existing browser discovery");
                open_existing_endpoint().await.map_err(|existing_err| {
                    anyhow::anyhow!(
                        "managed chrome launch failed: {managed_err:#}; existing-browser fallback failed: {existing_err:#}"
                    )
                })
            }
        },
    }
}

async fn open_existing_endpoint() -> anyhow::Result<OpenEndpoint> {
    let endpoint = endpoint::discover_running_chrome_endpoint()
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no running chrome with --remote-debugging-port found. \
                 start chrome with the debug flag, set SOCAI_CDP_WS, \
                 or run `socai config set chrome.profile managed` to use socai's managed chrome profile."
            )
        })?;
    Ok(OpenEndpoint {
        endpoint,
        chrome_process: None,
    })
}

async fn open_managed_endpoint(options: &ChromeConnectOptions) -> anyhow::Result<OpenEndpoint> {
    let user_data_dir = match &options.managed_user_data_dir {
        Some(path) => path.clone(),
        None => endpoint::managed_chrome_user_data_dir()?,
    };
    tokio::fs::create_dir_all(&user_data_dir).await?;

    // If a prior socai entrypoint already launched this isolated profile, reuse
    // it rather than spawning a second chrome against the same profile dir. We
    // didn't launch that process, so it gets no `ChromeProcess` guard — we must
    // not kill a browser we don't own.
    if let Some(endpoint) = endpoint::endpoint_from_active_port(&user_data_dir).await {
        let endpoint = endpoint::mark_managed_endpoint(endpoint, &user_data_dir);
        if reachable(&endpoint).await {
            return Ok(OpenEndpoint {
                endpoint,
                chrome_process: None,
            });
        }
        warn!(
            profile = %user_data_dir.display(),
            "managed chrome active-port marker was stale; relaunching"
        );
    }

    let (endpoint, chrome_process) = launch::launch_managed_chrome(&user_data_dir).await?;
    Ok(OpenEndpoint {
        endpoint,
        chrome_process: Some(chrome_process),
    })
}

/// Quick liveness probe for a discovered endpoint: a `DevToolsActivePort`
/// marker can outlive the chrome that wrote it (crash / kill). Probe the same
/// browser-websocket `Target.*` path the runtime will use for target inventory
/// and lifecycle.
async fn reachable(endpoint: &Endpoint) -> bool {
    match RawCdpClient::connect(&endpoint.browser_ws_url).await {
        Ok(client) => browser_ws_targets(&client).await.is_ok(),
        Err(_) => false,
    }
}

async fn connect_inventory(endpoint: &Endpoint) -> anyhow::Result<ConnectInventory> {
    let client = RawCdpClient::connect(&endpoint.browser_ws_url).await?;
    let targets = browser_ws_targets(&client).await?;
    let browser_version = browser_ws_version(&client)
        .await
        .ok()
        .or_else(|| browser_version_opt(endpoint))
        .unwrap_or_else(|| "unknown browser".into());
    Ok(ConnectInventory {
        targets,
        browser_version,
        browser_client: client,
    })
}

async fn on_connection_lost(cdp: Cdp, reason: String) {
    let _ = cdp.take_owned_targets().await;
    let state = cdp.state();
    let mut guard = state.lock().await;
    if matches!(*guard, CdpState::Connected { .. }) {
        *guard = CdpState::Disconnected { reason };
        let payload: StatusPayload = (&*guard).into();
        cdp.emit(BrowserEvent::StatusChanged(payload));
        cdp.emit(BrowserEvent::TargetsChanged(Vec::new()));
    }
}

async fn transition_if_eligible(cdp: &Cdp, new: CdpState) -> bool {
    let state = cdp.state();
    let mut guard = state.lock().await;
    let eligible = matches!(
        *guard,
        CdpState::Disconnected { .. } | CdpState::Connecting { .. }
    );
    if !eligible {
        return false;
    }
    *guard = new;
    let payload: StatusPayload = (&*guard).into();
    cdp.emit(BrowserEvent::StatusChanged(payload));
    true
}

async fn transition_unconditional(cdp: &Cdp, new: CdpState) {
    let state = cdp.state();
    let mut guard = state.lock().await;
    let clear_targets = matches!(*guard, CdpState::Connected { .. })
        && matches!(new, CdpState::Disconnected { .. });
    abort_monitor_if_connected(&guard);
    *guard = new;
    let payload: StatusPayload = (&*guard).into();
    cdp.emit(BrowserEvent::StatusChanged(payload));
    if clear_targets {
        cdp.emit(BrowserEvent::TargetsChanged(Vec::new()));
    }
}

fn abort_monitor_if_connected(state: &CdpState) {
    if let CdpState::Connected { monitor_task, .. } = state {
        monitor_task.abort();
    }
}

fn spawn_target_poll_loop(cdp: Cdp, browser_client: RawCdpClient) -> tokio::task::AbortHandle {
    let join = tokio::spawn(async move {
        let mut failures = 0u8;
        loop {
            tokio::time::sleep(TARGET_POLL_INTERVAL).await;
            match browser_ws_targets(&browser_client).await {
                Ok(targets) => {
                    failures = 0;
                    if let Some(pages) = replace_targets(&cdp, targets).await {
                        cdp.emit(BrowserEvent::TargetsChanged(pages));
                    }
                }
                Err(err) => {
                    failures = failures.saturating_add(1);
                    debug!(failures, error = %err, "target poll failed");
                    if failures >= TARGET_POLL_FAILURES {
                        on_connection_lost(cdp.clone(), format!("connection_lost: {err}")).await;
                        break;
                    }
                }
            }
        }
    });
    join.abort_handle()
}

/// Replace cached targets. Returns the new visible page list when it changed;
/// `None` means either no visible page change or the connection is inactive.
async fn replace_targets(
    cdp: &Cdp,
    next_targets: HashMap<String, TargetInfo>,
) -> Option<Vec<TargetInfo>> {
    let state = cdp.state();
    let mut guard = state.lock().await;
    let CdpState::Connected { targets, .. } = &mut *guard else {
        return None;
    };
    let before = page_list_from(targets);
    let after = page_list_from(&next_targets);
    *targets = next_targets;
    (before != after).then_some(after)
}

async fn browser_ws_targets(client: &RawCdpClient) -> anyhow::Result<HashMap<String, TargetInfo>> {
    let value = client.execute("Target.getTargets", json!({})).await?;
    let infos = value
        .get("targetInfos")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("Target.getTargets missing targetInfos"))?;
    let mut targets = HashMap::new();
    for info in infos {
        let target = target_info_from_protocol(info);
        if !target.target_id.is_empty() {
            targets.insert(target.target_id.clone(), target);
        }
    }
    Ok(targets)
}

fn target_info_from_protocol(info: &Value) -> TargetInfo {
    TargetInfo {
        target_id: info
            .get("targetId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        r#type: info
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        title: info
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        url: info
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    }
}

async fn browser_ws_version(client: &RawCdpClient) -> anyhow::Result<String> {
    let value = client.execute("Browser.getVersion", json!({})).await?;
    let product = value
        .get("product")
        .and_then(Value::as_str)
        .unwrap_or("unknown browser");
    let revision = value
        .get("revision")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if revision.is_empty() {
        Ok(product.to_string())
    } else {
        Ok(format!("{product} v{revision}"))
    }
}

async fn close_target_via_browser_ws(client: &RawCdpClient, target_id: &str) -> anyhow::Result<()> {
    client
        .execute("Target.closeTarget", json!({ "targetId": target_id }))
        .await?;
    Ok(())
}

/// Browser version string from the cached endpoint metadata, if present.
/// `None` for endpoints discovered without a `/json/version` round-trip (e.g.
/// an explicit `SOCAI_CDP_WS`), in which case callers fetch it actively.
fn browser_version_opt(endpoint: &Endpoint) -> Option<String> {
    endpoint.version.as_ref().and_then(|version| {
        version
            .browser
            .clone()
            .or_else(|| version.user_agent.clone())
    })
}
