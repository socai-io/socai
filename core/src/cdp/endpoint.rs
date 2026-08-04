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

/// One-shot discovery: explicit endpoints first, then `DevToolsActivePort`
/// browser websocket markers, then `/json/version` probes on standard debug
/// ports when no websocket marker is available.
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

/// Open the chrome://inspect remote-debugging page in the user's default
/// chrome for the existing-browser attach flow.
pub fn open_remote_debugging_page() -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("open")
        .args(["-a", "Google Chrome", INSPECT_URL])
        .status()?;
    #[cfg(target_os = "linux")]
    let status = std::process::Command::new("xdg-open")
        .arg(INSPECT_URL)
        .status()?;
    #[cfg(target_os = "windows")]
    let status = std::process::Command::new("cmd")
        .args(["/C", "start", "", INSPECT_URL])
        .status()?;
    if !status.success() {
        anyhow::bail!("chrome could not open the remote debugging page");
    }
    Ok(())
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

    // `DevToolsActivePort` already contains the browser websocket path. Use it
    // directly instead of probing `/json/version`: some user Chrome profiles
    // expose the websocket while returning 404 from the HTTP discovery API, and
    // the runtime's target inventory/lifecycle now uses raw browser-websocket
    // `Target.*` commands for both existing and managed Chrome.
    if ws_path.is_empty() {
        return None;
    }
    let source = format!("active_port:{}", marker.display());
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

/// `Endpoint::source` for a hosted session socai minted via socai-server.
pub(crate) const REMOTE_SOURCE: &str = "remote";

impl Endpoint {
    /// True when socai minted this endpoint as a hosted browser session.
    pub(crate) fn is_remote_session(&self) -> bool {
        self.source == REMOTE_SOURCE
    }

    /// The websocket URL in the form it is safe to display, log, or ship over
    /// IPC. A hosted connect URL is a live browser-control credential (the
    /// session token rides in it), so both minted sessions and any
    /// credential-shaped override are reduced to scheme + host. Loopback
    /// devtools URLs hold no secret and pass through unchanged — they are
    /// useful in bug reports and reachable only from this machine.
    pub(crate) fn display_ws_url(&self) -> String {
        if self.is_remote_session() || url_carries_credential(&self.browser_ws_url) {
            redact_ws_url(&self.browser_ws_url)
        } else {
            self.browser_ws_url.clone()
        }
    }
}

/// Whether a URL has somewhere a secret could be hiding. A query string is
/// how hosted CDP endpoints carry their session token, and userinfo
/// (`user:pass@host`) is a credential by definition. This catches a
/// `SOCAI_CDP_WS` override pointing at a hosted browser, which socai does not
/// own and therefore cannot recognize by ownership.
fn url_carries_credential(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let before_fragment = rest.split('#').next().unwrap_or(rest);
    before_fragment.contains('?')
        || before_fragment
            .split('/')
            .next()
            .is_some_and(|authority| authority.contains('@'))
}

fn redact_ws_url(url: &str) -> String {
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (scheme, rest),
        None => return "<redacted>".into(),
    };
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|host| !host.is_empty());
    match host {
        // Userinfo (user:pass@host) would itself be a credential; drop it.
        Some(host) => match host.rsplit_once('@') {
            Some((_, bare)) => format!("{scheme}://{bare}/<redacted>"),
            None => format!("{scheme}://{host}/<redacted>"),
        },
        None => "<redacted>".into(),
    }
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

async fn get_json<T: for<'de> serde::Deserialize<'de>>(url: &str) -> anyhow::Result<T> {
    let resp = debug_http_client()?
        .get(url)
        .send()
        .await?
        .error_for_status()?;
    Ok(resp.json().await?)
}

fn debug_http_client() -> anyhow::Result<reqwest::Client> {
    // Built once and cloned thereafter (clone is a cheap Arc bump that shares
    // the connection pool). HTTP is no longer used for active target
    // lifecycle, but explicit `SOCAI_CDP_URL` / conventional-port discovery may
    // still need `/json/version`.
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client.clone());
    }
    let client = reqwest::Client::builder()
        // Chrome's remote-debugging HTTP API is always a local control plane
        // endpoint (`127.0.0.1:<port>`). Do not honor HTTP(S)_PROXY / ALL_PROXY
        // here: proxy tools such as Clash can otherwise intercept `/json/*`
        // discovery calls and return 502 even though Chrome is healthy.
        .no_proxy()
        .timeout(HTTP_TIMEOUT)
        .build()?;
    Ok(CLIENT.get_or_init(|| client).clone())
}
