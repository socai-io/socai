use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const INSPECT_URL: &str = "chrome://inspect/#remote-debugging";
const DEFAULT_DEVTOOLS_PORTS: &[u16] = &[9222, 9223];
const HTTP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Serialize)]
pub struct Endpoint {
    pub source: String,
    pub browser_ws_url: String,
    pub http_version_url: Option<String>,
    pub version: Option<VersionInfo>,
    /// True when socai owns or reuses its isolated chrome user-data-dir rather
    /// than attaching to the user's daily browser profile.
    pub managed: bool,
    /// User-data-dir backing the browser, when known. For managed chrome this
    /// is the persistent isolated profile under ~/.socai by default.
    pub user_data_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VersionInfo {
    #[serde(rename = "Browser", default)]
    pub browser: Option<String>,
    #[serde(rename = "Protocol-Version", default)]
    pub protocol_version: Option<String>,
    #[serde(rename = "User-Agent", default)]
    pub user_agent: Option<String>,
    #[serde(rename = "V8-Version", default)]
    pub v8_version: Option<String>,
    #[serde(rename = "webSocketDebuggerUrl", default)]
    pub web_socket_debugger_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DebugTarget {
    #[serde(rename = "id", default)]
    pub target_id: String,
    #[serde(rename = "type", default)]
    pub r#type: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(rename = "webSocketDebuggerUrl", default)]
    pub web_socket_debugger_url: Option<String>,
}

/// Resolve only explicitly supplied endpoints (args + env vars). Returns None
/// when nothing explicit was set; the caller decides whether to fall back to
/// profile-scan discovery.
pub async fn resolve_explicit_endpoint(
    browser_ws_url: Option<&str>,
    http_url: Option<&str>,
) -> anyhow::Result<Option<Endpoint>> {
    if let Some(url) = non_empty(browser_ws_url) {
        return Ok(Some(Endpoint {
            source: "argument".into(),
            browser_ws_url: url.into(),
            http_version_url: None,
            version: None,
            managed: false,
            user_data_dir: None,
        }));
    }
    if let Some(url) = non_empty(http_url) {
        return Ok(Some(endpoint_from_http_url(url, "argument").await?));
    }
    if let Some(url) = env_var("SOCAI_CDP_WS") {
        return Ok(Some(Endpoint {
            source: "SOCAI_CDP_WS".into(),
            browser_ws_url: url,
            http_version_url: None,
            version: None,
            managed: false,
            user_data_dir: None,
        }));
    }
    if let Some(url) = env_var("SOCAI_CDP_URL") {
        return Ok(Some(endpoint_from_http_url(&url, "SOCAI_CDP_URL").await?));
    }
    Ok(None)
}

/// One-shot discovery: explicit endpoints first, then `DevToolsActivePort` /
/// `/json/version` probes on the standard Chrome profile roots and ports.
pub async fn discover_existing_chrome_endpoint() -> anyhow::Result<Option<Endpoint>> {
    if let Some(endpoint) = resolve_explicit_endpoint(None, None).await? {
        return Ok(Some(endpoint));
    }
    discover_running_chrome_endpoint().await
}

/// One-shot discovery for already-running browsers only. Unlike
/// [`discover_existing_chrome_endpoint`], this intentionally ignores explicit
/// SOCAI_CDP_* overrides; callers that need explicit endpoints should resolve
/// them first.
pub(crate) async fn discover_running_chrome_endpoint() -> anyhow::Result<Option<Endpoint>> {
    for profile in chrome_profile_roots() {
        if let Some(endpoint) = endpoint_from_active_port(&profile).await {
            return Ok(Some(endpoint));
        }
    }
    for port in DEFAULT_DEVTOOLS_PORTS {
        let url = format!("http://127.0.0.1:{port}");
        if let Ok(endpoint) = endpoint_from_http_url(&url, &format!("port:{port}")).await {
            return Ok(Some(endpoint));
        }
    }
    Ok(None)
}

/// Poll `discover_existing_chrome_endpoint` until it succeeds or `timeout`
/// elapses. Used by the existing-browser attach flow; the opt-in
/// managed-profile flow launches chrome itself and does not need this prompt.
pub async fn wait_for_existing_chrome_endpoint(
    timeout: Duration,
    poll: Duration,
) -> anyhow::Result<Option<Endpoint>> {
    let deadline = Instant::now() + timeout.max(Duration::from_millis(100));
    let step = poll.max(Duration::from_millis(100));
    loop {
        if let Some(endpoint) = discover_existing_chrome_endpoint().await? {
            return Ok(Some(endpoint));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        tokio::time::sleep(step).await;
    }
}

pub async fn list_debug_targets(endpoint: &Endpoint) -> anyhow::Result<Vec<DebugTarget>> {
    let url = endpoint_url(endpoint, "json/list")?;
    get_json(url.as_str()).await
}

/// Best-effort fetch of the browser product/revision string via the HTTP debug
/// API (`/json/version`). Used to populate the connected status when the
/// endpoint was discovered without a cached version (e.g. `SOCAI_CDP_WS`),
/// where we would otherwise show "unknown browser". Returns `None` if the HTTP
/// debug API is unavailable.
pub async fn fetch_browser_version(endpoint: &Endpoint) -> Option<String> {
    let url = endpoint_url(endpoint, "json/version").ok()?;
    let version: VersionInfo = get_json(url.as_str()).await.ok()?;
    version.browser.or(version.user_agent)
}

pub async fn create_debug_page(
    endpoint: &Endpoint,
    start_url: &str,
) -> anyhow::Result<DebugTarget> {
    let mut url = endpoint_url(endpoint, "json/new")?;
    let start_url = start_url.trim();
    if !start_url.is_empty() {
        url.set_query(Some(start_url));
    }
    let resp = debug_http_client()?
        .put(url)
        .send()
        .await?
        .error_for_status()?;
    Ok(resp.json().await?)
}

pub async fn close_debug_target(endpoint: &Endpoint, target_id: &str) -> anyhow::Result<bool> {
    let target_id = target_id.trim();
    if target_id.is_empty() {
        return Ok(false);
    }
    let url = endpoint_url(endpoint, &format!("json/close/{target_id}"))?;
    debug_http_client()?
        .get(url)
        .send()
        .await?
        .error_for_status()?;
    Ok(true)
}

pub fn http_base_url(endpoint: &Endpoint) -> anyhow::Result<String> {
    if let Some(version_url) = endpoint.http_version_url.as_deref() {
        let mut url = reqwest::Url::parse(version_url)?;
        url.set_path("");
        url.set_query(None);
        url.set_fragment(None);
        return Ok(url.as_str().trim_end_matches('/').to_string());
    }

    let mut url = reqwest::Url::parse(&endpoint.browser_ws_url)?;
    match url.scheme() {
        "ws" => url
            .set_scheme("http")
            .map_err(|_| anyhow::anyhow!("failed to convert ws endpoint to http"))?,
        "wss" => url
            .set_scheme("https")
            .map_err(|_| anyhow::anyhow!("failed to convert wss endpoint to https"))?,
        "http" | "https" => {}
        scheme => anyhow::bail!("unsupported CDP endpoint scheme: {scheme}"),
    }
    url.set_path("");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.as_str().trim_end_matches('/').to_string())
}

/// Open the chrome://inspect remote-debugging page in the user's default
/// chrome for the existing-browser attach flow. Best-effort; failures
/// are swallowed so setup UX still degrades gracefully to printed instructions.
pub fn open_remote_debugging_page() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .args(["-a", "Google Chrome", INSPECT_URL])
            .status();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open")
            .arg(INSPECT_URL)
            .status();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", INSPECT_URL])
            .status();
    }
}

async fn endpoint_from_http_url(url: &str, source: &str) -> anyhow::Result<Endpoint> {
    let version_url = format!("{}/json/version", url.trim_end_matches('/'));
    let version: VersionInfo = get_json(&version_url).await?;
    let ws = version
        .web_socket_debugger_url
        .clone()
        .ok_or_else(|| anyhow::anyhow!("/json/version missing webSocketDebuggerUrl"))?;
    Ok(Endpoint {
        source: source.into(),
        browser_ws_url: ws,
        http_version_url: Some(version_url),
        version: Some(version),
        managed: false,
        user_data_dir: None,
    })
}

pub(crate) async fn endpoint_from_active_port(profile: &Path) -> Option<Endpoint> {
    let marker = profile.join("DevToolsActivePort");
    let contents = fs::read_to_string(&marker).ok()?;
    let mut lines = contents.lines();
    let port: u16 = lines.next()?.trim().parse().ok()?;
    let ws_path = lines.next().map(str::trim).unwrap_or("").to_string();

    // Prefer richer info via HTTP /json/version. If the HTTP debugging API is
    // unavailable (some user Chrome launches expose only the websocket path),
    // keep the browser websocket fallback. The scoped-CDP runtime can use that
    // websocket with raw `Target.getTargets`/`Target.createTarget` without
    // chromiumoxide's global discovery/auto-init behavior.
    let http_url = format!("http://127.0.0.1:{port}");
    let source = format!("active_port:{}", marker.display());
    if let Ok(endpoint) = endpoint_from_http_url(&http_url, &source).await {
        return Some(endpoint);
    }
    if ws_path.is_empty() {
        return None;
    }
    let browser_ws_url = format!("ws://127.0.0.1:{port}{ws_path}");
    Some(Endpoint {
        source,
        browser_ws_url,
        http_version_url: None,
        version: None,
        managed: false,
        user_data_dir: None,
    })
}

pub fn managed_chrome_user_data_dir() -> anyhow::Result<PathBuf> {
    if let Some(home) = env_var("SOCAI_HOME") {
        return Ok(PathBuf::from(shellexpand(&home)).join("chrome-profile"));
    }
    if let Some(home) = home_dir() {
        return Ok(home.join(".socai/chrome-profile"));
    }
    Ok(PathBuf::from(".socai/chrome-profile"))
}

pub(crate) fn mark_managed_endpoint(mut endpoint: Endpoint, user_data_dir: &Path) -> Endpoint {
    endpoint.source = format!("managed_profile:{}", user_data_dir.display());
    endpoint.managed = true;
    endpoint.user_data_dir = Some(user_data_dir.display().to_string());
    endpoint
}

fn chrome_profile_roots() -> Vec<PathBuf> {
    if let Some(override_path) = env_var("SOCAI_CHROME_USER_DATA_DIR") {
        return vec![PathBuf::from(shellexpand(&override_path))];
    }
    let Some(home) = home_dir() else {
        return Vec::new();
    };

    #[cfg(target_os = "macos")]
    let candidates: &[&str] = &[
        "Library/Application Support/Google/Chrome",
        "Library/Application Support/Comet",
        "Library/Application Support/Arc/User Data",
        "Library/Application Support/Microsoft Edge",
        "Library/Application Support/BraveSoftware/Brave-Browser",
    ];

    #[cfg(target_os = "linux")]
    let candidates: &[&str] = &[
        ".config/google-chrome",
        ".config/chromium",
        ".config/microsoft-edge",
        ".config/BraveSoftware/Brave-Browser",
    ];

    #[cfg(target_os = "windows")]
    let candidates: &[&str] = &[
        "AppData/Local/Google/Chrome/User Data",
        "AppData/Local/Microsoft/Edge/User Data",
        "AppData/Local/BraveSoftware/Brave-Browser/User Data",
    ];

    candidates.iter().map(|c| home.join(c)).collect()
}

fn home_dir() -> Option<PathBuf> {
    if let Some(home) = env_var("HOME") {
        return Some(PathBuf::from(home));
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(profile) = env_var("USERPROFILE") {
            return Some(PathBuf::from(profile));
        }
        if let (Some(drive), Some(path)) = (env_var("HOMEDRIVE"), env_var("HOMEPATH")) {
            return Some(PathBuf::from(format!("{drive}{path}")));
        }
    }
    None
}

fn env_var(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.is_empty())
}

fn non_empty(s: Option<&str>) -> Option<&str> {
    s.filter(|v| !v.is_empty())
}

fn shellexpand(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    s.to_string()
}

fn endpoint_url(endpoint: &Endpoint, path: &str) -> anyhow::Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(&http_base_url(endpoint)?)?;
    url.set_path(path.trim_start_matches('/'));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

async fn get_json<T: for<'de> serde::Deserialize<'de>>(url: &str) -> anyhow::Result<T> {
    let resp = debug_http_client()?
        .get(url)
        .send()
        .await?
        .error_for_status()?;
    Ok(resp.json().await?)
}

fn debug_http_client() -> anyhow::Result<reqwest::Client> {
    // Built once and cloned thereafter (clone is a cheap Arc bump that shares the
    // connection pool). The connected-state poller calls this every ~2s, so
    // rebuilding the client + pool each time would be needless CPU/latency.
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client.clone());
    }
    let client = reqwest::Client::builder()
        // Chrome's remote-debugging HTTP API is always a local control plane
        // endpoint (`127.0.0.1:<port>`). Do not honor HTTP(S)_PROXY / ALL_PROXY
        // here: proxy tools such as Clash can otherwise intercept `/json/list`
        // and return 502 even though Chrome is healthy.
        .no_proxy()
        .timeout(HTTP_TIMEOUT)
        .build()?;
    Ok(CLIENT.get_or_init(|| client).clone())
}
