use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::target::{
    EventTargetCreated, EventTargetDestroyed, EventTargetInfoChanged, GetTargetsParams,
    SetDiscoverTargetsParams,
};
use chromiumoxide::{Browser, BrowserConfig, Handler};
use futures::StreamExt;
use tracing::{debug, info, warn};

use crate::cdp::connection::{
    BrowserEvent, Cdp, CdpState, ChromeConnectOptions, ChromeProfile, StatusPayload, TargetInfo,
};
use crate::cdp::endpoint::{self, Endpoint};

const MAX_ATTEMPTS: u8 = 3;
const ATTEMPT_DELAY: Duration = Duration::from_millis(500);
const MANAGED_LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);

struct OpenBrowser {
    endpoint: Endpoint,
    browser: Browser,
    handler: Handler,
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
    let OpenBrowser {
        endpoint,
        browser,
        mut handler,
    } = open_browser(options).await?;

    let cdp_for_pump = cdp.clone();
    let pump = tokio::spawn(async move {
        // Handler stream yields Result<_, CdpError>. Individual decode errors
        // are non-fatal — only stream exhaustion (None) means the WS closed.
        while let Some(event) = handler.next().await {
            if let Err(e) = event {
                debug!(error = ?e, "cdp handler non-fatal error");
            }
        }
        on_connection_dropped(cdp_for_pump).await;
    });
    let handler_task = pump.abort_handle();

    let version = browser.version().await?;
    let browser_version = format!("{} v{}", version.product, version.revision);

    browser
        .execute(SetDiscoverTargetsParams {
            discover: true,
            filter: None,
        })
        .await?;

    let initial = browser.execute(GetTargetsParams::default()).await?;
    let targets: HashMap<String, TargetInfo> = initial
        .result
        .target_infos
        .iter()
        .map(target_info_to_pair)
        .collect();

    let browser = Arc::new(browser);

    {
        let state = cdp.state();
        let mut guard = state.lock().await;
        if !guard.is_connecting() {
            return Err(anyhow::anyhow!("connect cancelled"));
        }
        *guard = CdpState::Connected {
            browser: Arc::clone(&browser),
            handler_task,
            endpoint,
            browser_version,
            targets,
        };
        let payload: StatusPayload = (&*guard).into();
        cdp.emit(BrowserEvent::StatusChanged(payload));
    }

    // initial targets emit so subscribers can hydrate
    let initial_pages = cdp.pages().await;
    cdp.emit(BrowserEvent::TargetsChanged(initial_pages));

    spawn_target_event_loop(Arc::clone(&browser), cdp.clone());

    Ok(())
}

async fn open_browser(options: Option<ChromeConnectOptions>) -> anyhow::Result<OpenBrowser> {
    let options = match options {
        Some(options) => options,
        None => ChromeConnectOptions::from_config()?,
    };

    if !matches!(options.profile, ChromeProfile::Managed) {
        if let Some(endpoint) = endpoint::resolve_explicit_endpoint(None, None).await? {
            return connect_to_endpoint(endpoint).await;
        }
    }

    match options.profile {
        ChromeProfile::Managed => open_managed_browser(&options).await,
        ChromeProfile::Existing => open_existing_browser().await,
        ChromeProfile::Auto => match open_managed_browser(&options).await {
            Ok(connection) => Ok(connection),
            Err(managed_err) => {
                warn!(error = %managed_err, "managed chrome launch failed; falling back to existing browser discovery");
                open_existing_browser().await.map_err(|existing_err| {
                    anyhow::anyhow!(
                        "managed chrome launch failed: {managed_err:#}; existing-browser fallback failed: {existing_err:#}"
                    )
                })
            }
        },
    }
}

async fn connect_to_endpoint(endpoint: Endpoint) -> anyhow::Result<OpenBrowser> {
    let (browser, handler) = Browser::connect(&endpoint.browser_ws_url).await?;
    Ok(OpenBrowser {
        endpoint,
        browser,
        handler,
    })
}

async fn open_existing_browser() -> anyhow::Result<OpenBrowser> {
    let endpoint: Endpoint = endpoint::discover_running_chrome_endpoint()
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no running chrome with --remote-debugging-port found. \
                 start chrome with the debug flag, set SOCAI_CDP_WS, \
                 or run `socai config set chrome.profile managed` to use socai's managed chrome profile."
            )
        })?;
    connect_to_endpoint(endpoint).await
}

async fn open_managed_browser(options: &ChromeConnectOptions) -> anyhow::Result<OpenBrowser> {
    let user_data_dir = match &options.managed_user_data_dir {
        Some(path) => path.clone(),
        None => endpoint::managed_chrome_user_data_dir()?,
    };
    tokio::fs::create_dir_all(&user_data_dir).await?;

    // if another socai entrypoint already launched the isolated profile, reuse
    // it instead of attempting a second chrome process with the same profile.
    if let Some(endpoint) = endpoint::endpoint_from_active_port(&user_data_dir).await {
        match connect_to_endpoint(endpoint::mark_managed_endpoint(endpoint, &user_data_dir)).await {
            Ok(connection) => return Ok(connection),
            Err(err) => {
                warn!(profile = %user_data_dir.display(), error = %err, "managed chrome active-port marker was stale");
            }
        }
    }

    let mut builder = BrowserConfig::builder()
        .with_head()
        .user_data_dir(&user_data_dir)
        .launch_timeout(MANAGED_LAUNCH_TIMEOUT)
        .viewport(None)
        .window_size(1280, 900)
        .arg("--disable-blink-features=AutomationControlled")
        .arg("--no-default-browser-check")
        .arg("--no-first-run");

    if let Some(executable) = chrome_executable_override() {
        builder = builder.chrome_executable(executable);
    }

    info!(profile = %user_data_dir.display(), "launching managed chrome profile");
    let (browser, handler) = Browser::launch(builder.build().map_err(anyhow::Error::msg)?).await?;
    let endpoint = Endpoint {
        source: format!("managed_profile:{}", user_data_dir.display()),
        browser_ws_url: browser.websocket_address().clone(),
        http_version_url: None,
        version: None,
        managed: true,
        user_data_dir: Some(user_data_dir.display().to_string()),
    };
    Ok(OpenBrowser {
        endpoint,
        browser,
        handler,
    })
}

fn chrome_executable_override() -> Option<PathBuf> {
    env::var("SOCAI_CHROME_EXECUTABLE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("CHROME")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .map(PathBuf::from)
}

async fn on_connection_dropped(cdp: Cdp) {
    let state = cdp.state();
    let mut guard = state.lock().await;
    if matches!(*guard, CdpState::Connected { .. }) {
        *guard = CdpState::Disconnected {
            reason: "connection_lost".into(),
        };
        let payload: StatusPayload = (&*guard).into();
        cdp.emit(BrowserEvent::StatusChanged(payload));
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
    abort_pump_if_connected(&guard);
    *guard = new;
    let payload: StatusPayload = (&*guard).into();
    cdp.emit(BrowserEvent::StatusChanged(payload));
}

/// On user-initiated disconnect we have to terminate the WS pump task — its
/// `Handler` owns the WebSocket, so dropping the `Arc<Browser>` alone won't
/// close the socket. Aborting causes the task to be dropped, which drops the
/// `Handler`, which closes the WS — only then does Chrome remove the
/// "controlled by automated software" banner.
fn abort_pump_if_connected(state: &CdpState) {
    if let CdpState::Connected { handler_task, .. } = state {
        handler_task.abort();
    }
}

/// Subscribe to Target.* events from chromiumoxide; fold them into the cached
/// targets map and emit `BrowserEvent::TargetsChanged` whenever the visible
/// page list actually changes. Replaces the previous 100ms `Target.getTargets`
/// polling loop.
fn spawn_target_event_loop(browser: Arc<Browser>, cdp: Cdp) {
    tokio::spawn(async move {
        let (created, destroyed, changed) = match try_join_listeners(&browser).await {
            Ok(streams) => streams,
            Err(e) => {
                warn!(error = %e, "failed to subscribe to target events");
                return;
            }
        };
        let mut created = Box::pin(created);
        let mut destroyed = Box::pin(destroyed);
        let mut changed = Box::pin(changed);

        let mut last_emitted = cdp.pages().await;

        loop {
            let dirty = tokio::select! {
                Some(ev) = created.next() => {
                    apply_target_change(&cdp, |targets| {
                        targets.insert(ev.target_info.target_id.inner().clone(), to_target_info(&ev.target_info));
                    }).await
                }
                Some(ev) = destroyed.next() => {
                    apply_target_change(&cdp, |targets| {
                        targets.remove(ev.target_id.inner().as_str());
                    }).await
                }
                Some(ev) = changed.next() => {
                    apply_target_change(&cdp, |targets| {
                        targets.insert(ev.target_info.target_id.inner().clone(), to_target_info(&ev.target_info));
                    }).await
                }
                else => break,
            };
            if !dirty {
                break;
            }
            let pages = cdp.pages().await;
            if pages != last_emitted {
                last_emitted = pages.clone();
                cdp.emit(BrowserEvent::TargetsChanged(pages));
            }
        }
    });
}

async fn try_join_listeners(
    browser: &Browser,
) -> anyhow::Result<(
    impl futures::Stream<Item = Arc<EventTargetCreated>>,
    impl futures::Stream<Item = Arc<EventTargetDestroyed>>,
    impl futures::Stream<Item = Arc<EventTargetInfoChanged>>,
)> {
    let created = browser.event_listener::<EventTargetCreated>().await?;
    let destroyed = browser.event_listener::<EventTargetDestroyed>().await?;
    let changed = browser.event_listener::<EventTargetInfoChanged>().await?;
    Ok((created, destroyed, changed))
}

/// Apply a closure under the state lock. Returns false if state moved out of
/// Connected (loop should exit).
async fn apply_target_change<F>(cdp: &Cdp, f: F) -> bool
where
    F: FnOnce(&mut HashMap<String, TargetInfo>),
{
    let state = cdp.state();
    let mut guard = state.lock().await;
    match &mut *guard {
        CdpState::Connected { targets, .. } => {
            f(targets);
            true
        }
        _ => false,
    }
}

fn target_info_to_pair(
    info: &chromiumoxide::cdp::browser_protocol::target::TargetInfo,
) -> (String, TargetInfo) {
    let ti = to_target_info(info);
    (ti.target_id.clone(), ti)
}

fn to_target_info(info: &chromiumoxide::cdp::browser_protocol::target::TargetInfo) -> TargetInfo {
    TargetInfo {
        target_id: info.target_id.inner().clone(),
        r#type: info.r#type.clone(),
        title: info.title.clone(),
        url: info.url.clone(),
    }
}
