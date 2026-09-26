use socai_core::telemetry::tool_call::{summarize_tool_args, summarize_tool_result};
use socai_core::telemetry::{
    query_text_enabled, redact_secrets, telemetry_enabled, Telemetry, TelemetrySource,
};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use socai_core::agent::tool::{ToolProgressEvent, ToolProgressSender};
use socai_core::runtime::{BrowserStatus, ChromeConnectOptions, RuntimeBrowserEvent, SocaiRuntime};
use socai_core::sites::{
    all_native_site_adapters, find_native_site_adapter, NativeSiteAdapter, SiteCommand,
};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Stdio;
#[cfg(test)]
use std::sync::Mutex as StdMutex;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::fs;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
#[cfg(windows)]
use tokio::net::{TcpListener, TcpStream};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, Mutex, Notify};
use tokio::time::{sleep, timeout, Instant};

pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(600);
pub const LONG_COMMAND_TIMEOUT: Duration = Duration::from_secs(1_200);

#[cfg(windows)]
type DaemonListener = TcpListener;
#[cfg(windows)]
type DaemonStream = TcpStream;
#[cfg(unix)]
type DaemonListener = UnixListener;
#[cfg(unix)]
type DaemonStream = UnixStream;

#[cfg(unix)]
const SOCKET_NAME: &str = "rust-daemon.sock";
#[cfg(windows)]
const ENDPOINT_NAME: &str = "rust-daemon-endpoint.json";
const PID_NAME: &str = "rust-daemon.pid";
const LOG_NAME: &str = "rust-daemon.log";
const IDLE_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(90);

/// The daemon only serves a CLI of the exact same build. A version or build
/// mismatch restarts the daemon from the current binary so a leftover process
/// never blocks the command the user just ran.
const PROTOCOL_VERSION: &str = env!("CARGO_PKG_VERSION");
const CODE_VERSION_MISMATCH: &str = "version-mismatch";
const CODE_STALE_DAEMON: &str = "stale-daemon";

static BUILD_ID: OnceLock<String> = OnceLock::new();

/// Fingerprint (size + mtime) of the executable this process started from.
/// The daemon pins it at startup — before a rebuild can swap the file under
/// the same path — so comparing it against the calling CLI detects a stale
/// daemon even when the package version did not change.
fn process_build_id() -> &'static str {
    BUILD_ID.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| std::fs::metadata(exe).ok())
            .and_then(|meta| {
                let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
                Some(format!("{}-{}", meta.len(), mtime.as_nanos()))
            })
            .unwrap_or_else(|| "unknown".to_string())
    })
}

/// Daemon failures the client recovers by replacing the running daemon.
#[derive(Debug)]
enum DaemonClientError {
    VersionMismatch(String),
    StaleDaemon(String),
    CommandFailed(String),
}

impl std::fmt::Display for DaemonClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DaemonClientError::VersionMismatch(message)
            | DaemonClientError::StaleDaemon(message)
            | DaemonClientError::CommandFailed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for DaemonClientError {}

#[derive(Debug, Serialize, Deserialize)]
struct DaemonRequest {
    id: String,
    /// Site id the command belongs to. Empty (legacy clients) means "xhs".
    #[serde(default)]
    site: String,
    command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auth: Option<String>,
    /// CLI package version + binary fingerprint. Empty for legacy clients.
    #[serde(default)]
    version: String,
    #[serde(default)]
    build_id: String,
    #[serde(default)]
    args: Value,
    #[serde(default)]
    telemetry: DaemonTelemetry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DaemonTelemetry {
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default = "default_true")]
    include_query_text: bool,
}

impl Default for DaemonTelemetry {
    fn default() -> Self {
        Self {
            enabled: true,
            include_query_text: true,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize)]
struct DaemonResponse {
    id: String,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// Machine-readable failure class (e.g. version-mismatch, stale-daemon).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    /// Daemon build identity; missing on responses from legacy daemons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    build_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DaemonProgressFrame {
    #[serde(rename = "type")]
    kind: String,
    id: String,
    event: ToolProgressEvent,
}

enum DaemonLine {
    Progress(ToolProgressEvent),
    Response(DaemonResponse),
}

impl DaemonResponse {
    fn success(id: String, result: Value) -> Self {
        Self {
            id,
            ok: true,
            result: Some(result),
            error: None,
            code: None,
            version: Some(PROTOCOL_VERSION.to_string()),
            build_id: Some(process_build_id().to_string()),
        }
    }

    fn failure(id: String, code: Option<&str>, error: String) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(error),
            code: code.map(str::to_string),
            version: Some(PROTOCOL_VERSION.to_string()),
            build_id: Some(process_build_id().to_string()),
        }
    }
}

struct DaemonPaths {
    home: PathBuf,
    #[cfg(unix)]
    socket: PathBuf,
    #[cfg(windows)]
    endpoint: PathBuf,
    pid: PathBuf,
    log: PathBuf,
}

#[cfg(windows)]
#[derive(Debug, Serialize, Deserialize)]
struct DaemonEndpoint {
    host: String,
    port: u16,
    token: String,
}

struct DaemonState {
    runtime: SocaiRuntime,
    telemetry: Telemetry,
}

/// Request metadata and read-only runtime access that must stay available
/// while a long site command holds the mutable daemon-state lock.
struct DaemonControl {
    runtime: SocaiRuntime,
    auth_token: Option<String>,
    last_activity: Mutex<Instant>,
}

#[derive(Clone, Debug)]
struct ToolTraceContext {
    request_id: String,
    site_id: String,
    command: String,
    tool_name: String,
}

#[cfg(test)]
type RecordedEvents = Arc<StdMutex<Vec<(String, Value)>>>;

pub async fn run_daemon() -> Result<()> {
    // Pin the binary fingerprint before a rebuild can swap the file under us.
    let _ = process_build_id();
    let paths = daemon_paths()?;
    fs::create_dir_all(&paths.home).await?;
    #[cfg(unix)]
    fs::set_permissions(&paths.home, std::fs::Permissions::from_mode(0o700)).await?;
    cleanup_stale_ipc(&paths).await?;

    let listener = bind_daemon_listener(&paths).await?;
    let auth_token = daemon_auth_token();
    write_daemon_endpoint(&paths, &listener, auth_token.as_deref()).await?;
    fs::write(&paths.pid, std::process::id().to_string()).await?;

    let runtime = SocaiRuntime::new();
    // Kept outside the DaemonState mutex for the shutdown path below: a site
    // command holds that mutex for its whole execution (minutes for e.g.
    // wait-for-login), and shutdown must not queue behind it — the SIGTERM
    // kill grace is 6 seconds. The handle is a cheap Arc-backed clone of the
    // same runtime.
    let runtime_for_shutdown = runtime.clone();
    let control = Arc::new(DaemonControl {
        runtime: runtime.clone(),
        auth_token: auth_token.clone(),
        last_activity: Mutex::new(Instant::now()),
    });
    let telemetry = Telemetry::new(&paths.home, TelemetrySource::CliDaemon);
    let state = Arc::new(Mutex::new(DaemonState { runtime, telemetry }));
    let command_gate = Arc::new(Mutex::new(()));
    let stop = Arc::new(Notify::new());
    let mut idle_check = tokio::time::interval(Duration::from_secs(60));
    let terminate = terminate_signal();
    tokio::pin!(terminate);

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                let (stream, _) = accept_result.context("accept daemon client")?;
                let state = state.clone();
                let control = control.clone();
                let stop = stop.clone();
                let command_gate = command_gate.clone();
                tokio::spawn(async move {
                    if let Err(err) =
                        serve_client(stream, state, control, stop, command_gate).await
                    {
                        eprintln!("daemon client error: {err:#}");
                    }
                });
            }
            _ = idle_check.tick() => {
                if control.last_activity.lock().await.elapsed() > IDLE_TIMEOUT {
                    break;
                }
            }
            _ = &mut terminate => break,
            _ = stop.notified() => break,
        }
    }

    // Unlink our IPC endpoint before the slow browser teardown: a successor
    // daemon may bind a fresh socket at this path right away, and removing it
    // after shutdown would yank the new daemon's endpoint from under it.
    cleanup_stale_ipc(&paths).await?;
    let _ = fs::remove_file(&paths.pid).await;
    // Tear down directly on the runtime handle, not through the DaemonState
    // mutex — an in-flight command may hold that mutex for minutes. Stopping
    // means stopping: the browser is yanked from under any such command (it
    // fails, the daemon exits), and the bounded remote-session release runs
    // right away instead of after the command finishes. disconnect() itself
    // sweeps socai-owned tabs (bounded, and skipped for remote sessions
    // whose browser dies with the release) — an extra page-close pass here
    // could stall ~30s per command against a wedged browser and eat the
    // SIGTERM kill grace before the release starts.
    runtime_for_shutdown.disconnect_browser().await;
    Ok(())
}

/// Resolves when the daemon receives SIGTERM; pends forever on non-unix.
/// `kill_stale_daemons` (and a plain `kill`) send SIGTERM expecting a graceful
/// exit — without a handler the process dies before `shutdown()`, which for a
/// remote browser session means no release and a session that runs out its
/// full server-side timeout.
async fn terminate_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    }
    #[cfg(not(unix))]
    {
        std::future::pending::<()>().await
    }
}

pub async fn send_or_spawn(
    site: &str,
    command: &str,
    args: Value,
    command_timeout: Duration,
    on_progress: &mut dyn FnMut(ToolProgressEvent),
) -> Result<Value> {
    let err = match send_request(site, command, args.clone(), command_timeout, on_progress).await {
        Ok(result) => return Ok(result),
        Err(err) => err,
    };
    match err.downcast_ref::<DaemonClientError>() {
        // A leftover daemon from another release or binary cannot run this
        // command. Replace it with the CLI the user just invoked.
        Some(DaemonClientError::VersionMismatch(_) | DaemonClientError::StaleDaemon(_)) => {
            eprintln!("socai daemon does not match this CLI; restarting it");
            let _ = stop_daemon().await;
            wait_for_daemon_exit().await;
        }
        // The daemon is alive and returned an application/browser error. Keep
        // its CDP websocket and reusable site tab intact so a corrected follow-
        // up command can continue in the same browser session.
        Some(DaemonClientError::CommandFailed(_)) => return Err(err),
        None => {}
    }
    spawn_daemon().await?;
    send_request(site, command, args, command_timeout, on_progress).await
}

pub async fn stop_daemon() -> Result<bool> {
    match send_request(
        "",
        "shutdown",
        json!({}),
        Duration::from_secs(10),
        &mut |_| {},
    )
    .await
    {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// Read the current daemon/browser state without spawning the daemon or
/// initiating a browser connection. A missing, stale, or older daemon is an
/// explicit unknown state rather than a reason to touch Chrome.
pub async fn read_status() -> Value {
    match send_request("", "status", json!({}), Duration::from_secs(2), &mut |_| {}).await {
        Ok(status) => status,
        Err(_) => {
            let daemon_running =
                send_request("", "ping", json!({}), Duration::from_secs(2), &mut |_| {})
                    .await
                    .is_ok();
            status_snapshot(None, daemon_running, false)
        }
    }
}

fn status_snapshot(
    browser: Option<BrowserStatus>,
    daemon_running: bool,
    daemon_compatible: bool,
) -> Value {
    let configured_profile = ChromeConnectOptions::from_config()
        .map(|options| options.profile.as_str())
        .unwrap_or("unknown");
    let platforms = all_native_site_adapters()
        .iter()
        .map(|site| {
            json!({
                "id": site.id,
                "available": true,
                "login_state": "unknown",
                "operations": site.commands.iter().map(|command| command.name).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();

    let (browser_state, browser_connected, active_profile_mode, error_code, next_step) =
        match browser {
            Some(BrowserStatus::Connected {
                managed, remote, ..
            }) => (
                "connected",
                true,
                Some(if remote {
                    "remote"
                } else if managed {
                    "managed"
                } else {
                    "existing"
                }),
                None,
                None,
            ),
            Some(BrowserStatus::Connecting { .. }) => ("connecting", false, None, None, None),
            Some(BrowserStatus::Disconnected { reason }) => {
                let (code, action) = safe_browser_failure(&reason, configured_profile);
                ("disconnected", false, None, Some(code), Some(action))
            }
            None if daemon_running => (
                "unknown",
                false,
                None,
                Some("DAEMON_STATUS_UNAVAILABLE"),
                Some("Update or restart socai once, then retry `socai status --json`."),
            ),
            None => (
                "unknown",
                false,
                None,
                Some("DAEMON_UNAVAILABLE"),
                Some("Run any read-only socai platform command, then retry `socai status --json`."),
            ),
        };

    json!({
        "schema_version": 1,
        "cli_available": true,
        "cli_version": PROTOCOL_VERSION,
        "daemon_running": daemon_running,
        "daemon_compatible": daemon_compatible,
        "browser_connected": browser_connected,
        "browser_state": browser_state,
        "profile_mode": configured_profile,
        "active_profile_mode": active_profile_mode,
        "error_code": error_code,
        "next_step": next_step,
        "platforms": platforms,
    })
}

fn safe_browser_failure(reason: &str, configured_profile: &str) -> (&'static str, &'static str) {
    let reason = reason.to_ascii_lowercase();
    if reason == "not_yet_connected" {
        return (
            "BROWSER_NOT_CONNECTED",
            "Run a read-only socai platform command when browser access is needed.",
        );
    }
    if reason == "user_disconnected" {
        return (
            "BROWSER_DISCONNECTED",
            "Reconnect from socai before starting browser research.",
        );
    }
    if reason.contains("permission denied")
        || reason.contains("operation not permitted")
        || reason.contains("access denied")
    {
        return (
            "BROWSER_PERMISSION_REQUIRED",
            "Allow Chrome data access and remote debugging, then retry once.",
        );
    }
    if configured_profile == "remote"
        || reason.contains("remote browser")
        || reason.contains("browser session")
        || reason.contains("socai pro")
    {
        return (
            "REMOTE_SESSION_UNAVAILABLE",
            "Check socai pro and the remote browser service, then retry later.",
        );
    }
    if reason.contains("no running chrome")
        || reason.contains("failed to connect")
        || reason.contains("did not respond")
        || reason.contains("connection refused")
        || reason.contains("endpoint")
        || reason.contains("cdp")
    {
        return (
            "BROWSER_ENDPOINT_UNREACHABLE",
            "Start a compatible Chrome session or select the managed profile, then retry.",
        );
    }
    (
        "BROWSER_CONNECTION_FAILED",
        "Check the socai browser setup, then retry once.",
    )
}

async fn serve_client(
    stream: DaemonStream,
    state: Arc<Mutex<DaemonState>>,
    control: Arc<DaemonControl>,
    stop: Arc<Notify>,
    command_gate: Arc<Mutex<()>>,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    while reader.read_line(&mut line).await? != 0 {
        let request: DaemonRequest = serde_json::from_str(line.trim_end())?;
        let request_id = request.id.clone();
        let served = serve_request(
            request,
            request_id,
            &mut reader,
            &mut writer,
            state.clone(),
            control.clone(),
            stop.clone(),
            command_gate.clone(),
        )
        .await;
        match served {
            Ok(true) => line.clear(),
            Ok(false) => return Ok(()),
            Err(error) => return Err(error),
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn serve_request<R, W>(
    request: DaemonRequest,
    request_id: String,
    reader: &mut R,
    writer: &mut W,
    state: Arc<Mutex<DaemonState>>,
    control: Arc<DaemonControl>,
    stop: Arc<Notify>,
    command_gate: Arc<Mutex<()>>,
) -> Result<bool>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
    let mut disconnect_probe = String::new();
    let response = handle_request(
        request,
        state,
        control,
        stop,
        Some(progress_tx),
        command_gate,
    );
    tokio::pin!(response);
    let disconnect = reader.read_line(&mut disconnect_probe);
    tokio::pin!(disconnect);
    let mut progress_open = true;
    loop {
        tokio::select! {
            biased;
            event = progress_rx.recv(), if progress_open => {
                match event {
                    Some(event) => {
                        let frame = DaemonProgressFrame {
                            kind: "progress".to_string(),
                            id: request_id.clone(),
                            event,
                        };
                        writer
                            .write_all(serde_json::to_string(&frame)?.as_bytes())
                            .await?;
                        writer.write_all(b"\n").await?;
                    }
                    None => progress_open = false,
                }
            }
            response = &mut response => {
                writer
                    .write_all(serde_json::to_string(&response)?.as_bytes())
                    .await?;
                writer.write_all(b"\n").await?;
                return Ok(true);
            }
            read = &mut disconnect => {
                if read? == 0 {
                    return Ok(false);
                }
                anyhow::bail!("daemon client sent another request before the previous response");
            }
        }
    }
}

async fn handle_request(
    request: DaemonRequest,
    state: Arc<Mutex<DaemonState>>,
    control: Arc<DaemonControl>,
    stop: Arc<Notify>,
    progress: Option<ToolProgressSender>,
    command_gate: Arc<Mutex<()>>,
) -> DaemonResponse {
    let id = request.id.clone();
    let command = request.command.clone();
    let telemetry = request.telemetry.clone();
    if !daemon_request_authorized(request.auth.as_deref(), control.auth_token.as_deref()) {
        return DaemonResponse::failure(id, None, "daemon authentication failed".into());
    }

    // Site commands only run for a CLI of the exact same build. ping and
    // shutdown stay exempt so `socai stop` works across any version pairing.
    if !matches!(command.as_str(), "ping" | "shutdown") {
        if request.version != PROTOCOL_VERSION {
            let cli_version = if request.version.is_empty() {
                "<unknown>"
            } else {
                request.version.as_str()
            };
            return DaemonResponse::failure(
                id,
                Some(CODE_VERSION_MISMATCH),
                format!(
                    "socai daemon {PROTOCOL_VERSION} cannot serve CLI {cli_version}; \
                     run `socai stop`, then update or rebuild so both use the same version"
                ),
            );
        }
        if request.build_id != process_build_id() {
            return DaemonResponse::failure(
                id,
                Some(CODE_STALE_DAEMON),
                format!(
                    "socai daemon was started from a different build of {PROTOCOL_VERSION} \
                     (the binary changed since it started)"
                ),
            );
        }
    }

    let result = async {
        if command == "ping" {
            return Ok(json!({ "ok": true }));
        }

        if command == "shutdown" {
            stop.notify_waiters();
            return Ok(json!({ "ok": true }));
        }

        if command == "status" {
            return Ok(status_snapshot(
                Some(control.runtime.browser_status().await),
                true,
                true,
            ));
        }

        let site_id = if request.site.trim().is_empty() {
            "xhs"
        } else {
            request.site.trim()
        };
        let site =
            find_native_site_adapter(site_id).ok_or_else(|| anyhow!("unknown site: {site_id}"))?;
        let spec = site
            .command(&command)
            .ok_or_else(|| anyhow!("unknown {site_id} command: {command}"))?;

        *control.last_activity.lock().await = Instant::now();
        let command_gate = command_gate.lock_owned().await;
        let mut state = state.lock().await;
        state
            .run_site_command(
                &id,
                site,
                spec,
                request.args,
                &telemetry,
                progress,
                command_gate,
            )
            .await
    }
    .await;

    match result {
        Ok(result) => DaemonResponse::success(id, result),
        Err(err) => DaemonResponse::failure(id, None, format!("{err:#}")),
    }
}

#[cfg(unix)]
fn daemon_request_authorized(_request_auth: Option<&str>, _daemon_auth: Option<&str>) -> bool {
    true
}

#[cfg(windows)]
fn daemon_request_authorized(request_auth: Option<&str>, daemon_auth: Option<&str>) -> bool {
    request_auth.is_some() && request_auth == daemon_auth
}

impl DaemonState {
    #[allow(clippy::too_many_arguments)]
    async fn run_site_command(
        &mut self,
        request_id: &str,
        site: &'static NativeSiteAdapter,
        spec: &'static SiteCommand,
        args: Value,
        telemetry: &DaemonTelemetry,
        progress: Option<ToolProgressSender>,
        command_gate: tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<Value> {
        let tool_trace = ToolCallTrace::start(
            self.telemetry.clone(),
            self.runtime.clone(),
            command_gate,
            request_id,
            site.id,
            spec.name,
            spec.tool_name,
            &args,
            telemetry,
        );
        // Marks browser work in flight for the whole command, so the remote
        // idle reaper never releases the session under a running tool.
        let _activity = self.runtime.begin_activity().await;
        let runtime = self.runtime.clone();
        let mut browser_events = runtime.subscribe_browser_events();
        let command = async {
            let debug_snapshot = debug_snapshot_flag(&args);
            // Create the session tab blank and let the command navigate itself:
            // every site command either opens its own entry URL (e.g. `author`
            // opens the profile directly) or has a `before` hook that reaches
            // the right page (search via ensure_search_ready). Passing
            // home_url here would force an extra `/explore` load before the
            // command then navigates again — wasted time for no benefit.
            let page = runtime.ensure_site_page(site.id, "").await?;
            (spec.run)(page, args.clone(), debug_snapshot, progress).await
        };
        tokio::pin!(command);
        let mut browser_events_open = true;
        let result = loop {
            tokio::select! {
                biased;
                event = browser_events.recv(), if browser_events_open => {
                    match event {
                        Ok(RuntimeBrowserEvent::StatusChanged(status)) => {
                            tool_trace.capture_browser_status(&status);
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            browser_events_open = false;
                        }
                    }
                }
                result = &mut command => break result,
            }
        };
        tool_trace.finish(&result);
        result
    }
}

struct ToolCallTrace {
    telemetry: ToolTelemetry,
    cancel_runtime: Option<SocaiRuntime>,
    command_gate: Option<tokio::sync::OwnedMutexGuard<()>>,
    context: ToolTraceContext,
    properties: Map<String, Value>,
    started: Instant,
    finished: bool,
}

#[derive(Clone)]
struct ToolTelemetry {
    enabled: bool,
    telemetry: Option<Telemetry>,
    #[cfg(test)]
    recorded: Option<RecordedEvents>,
}

impl ToolTelemetry {
    fn production(telemetry: Telemetry, enabled: bool) -> Self {
        Self {
            enabled,
            telemetry: enabled.then_some(telemetry),
            #[cfg(test)]
            recorded: None,
        }
    }

    fn capture(&self, name: &str, properties: Value) {
        if !self.enabled {
            return;
        }
        #[cfg(test)]
        if let Some(recorded) = &self.recorded {
            if let Ok(mut events) = recorded.lock() {
                events.push((name.to_string(), properties.clone()));
            }
        }
        if let Some(telemetry) = &self.telemetry {
            telemetry.capture(name, properties);
        }
    }

    #[cfg(test)]
    fn recording(enabled: bool) -> (Self, RecordedEvents) {
        let recorded = Arc::new(StdMutex::new(Vec::new()));
        (
            Self {
                enabled,
                telemetry: None,
                recorded: Some(recorded.clone()),
            },
            recorded,
        )
    }
}

impl ToolCallTrace {
    #[allow(clippy::too_many_arguments)]
    fn start(
        telemetry_client: Telemetry,
        runtime: SocaiRuntime,
        command_gate: tokio::sync::OwnedMutexGuard<()>,
        request_id: &str,
        site_id: &str,
        command: &str,
        tool_name: &str,
        input: &Value,
        telemetry: &DaemonTelemetry,
    ) -> Self {
        Self::start_with_telemetry(
            ToolTelemetry::production(telemetry_client, telemetry.enabled),
            Some(runtime),
            Some(command_gate),
            request_id,
            site_id,
            command,
            tool_name,
            input,
            telemetry.include_query_text,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn start_with_telemetry(
        telemetry: ToolTelemetry,
        cancel_runtime: Option<SocaiRuntime>,
        command_gate: Option<tokio::sync::OwnedMutexGuard<()>>,
        request_id: &str,
        site_id: &str,
        command: &str,
        tool_name: &str,
        input: &Value,
        include_query_text: bool,
    ) -> Self {
        let context = ToolTraceContext {
            request_id: request_id.to_string(),
            site_id: site_id.to_string(),
            command: command.to_string(),
            tool_name: tool_name.to_string(),
        };

        let mut properties = trace_context_props(&context);
        merge_object(
            &mut properties,
            Value::Object(summarize_tool_args(input, include_query_text)),
        );
        telemetry.capture("socai_tool_call_start", Value::Object(properties.clone()));

        Self {
            telemetry,
            cancel_runtime,
            command_gate,
            context,
            properties,
            started: Instant::now(),
            finished: false,
        }
    }

    fn finish(mut self, result: &Result<Value>) {
        let mut properties = self.properties.clone();
        finish_tool_call_props(
            &mut properties,
            self.started.elapsed().as_millis() as u64,
            Some(result),
        );
        self.telemetry
            .capture("socai_tool_call", Value::Object(properties));
        self.finished = true;
    }

    fn capture_browser_status(&self, status: &BrowserStatus) {
        let Some(properties) = browser_connect_props(status, &self.context) else {
            return;
        };
        self.telemetry
            .capture("socai_browser_connect", Value::Object(properties));
    }
}

impl Drop for ToolCallTrace {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let mut properties = self.properties.clone();
        finish_tool_call_props(
            &mut properties,
            self.started.elapsed().as_millis() as u64,
            None,
        );
        self.telemetry
            .capture("socai_tool_call_interrupted", Value::Object(properties));
        let (Some(runtime), Some(command_gate)) =
            (self.cancel_runtime.take(), self.command_gate.take())
        else {
            return;
        };
        // The CDP connect loop is detached from the command future. Keep the
        // daemon's command gate across its cancellation/settlement so a later
        // request cannot inherit retries and misattribute their telemetry.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                // Always invalidate the connect generation. The detached task
                // may have been spawned but not yet published `Connecting`.
                runtime.cancel_browser_connect_and_wait().await;
                drop(command_gate);
            });
        }
    }
}

fn trace_context_props(context: &ToolTraceContext) -> Map<String, Value> {
    let mut props = Map::new();
    props.insert("request_id".into(), json!(context.request_id));
    props.insert("command".into(), json!(context.command));
    props.insert("tool_name".into(), json!(context.tool_name));
    props.insert("site".into(), json!(context.site_id));
    props
}

fn finish_tool_call_props(
    props: &mut Map<String, Value>,
    duration_ms: u64,
    result: Option<&Result<Value>>,
) {
    props.insert("duration_ms".into(), json!(duration_ms));
    match result {
        Some(Ok(value)) => {
            props.insert("outcome".into(), json!("completed"));
            props.insert("ok".into(), json!(true));
            merge_object(props, Value::Object(summarize_tool_result(value)));
        }
        Some(Err(err)) => {
            props.insert("outcome".into(), json!("failed"));
            props.insert("ok".into(), json!(false));
            props.insert("error".into(), json!(error_summary(err)));
        }
        None => {
            props.insert("outcome".into(), json!("interrupted"));
            props.insert("ok".into(), json!(false));
            props.insert("error_type".into(), json!("command_interrupted"));
            props.insert(
                "error".into(),
                json!("command interrupted before completion"),
            );
        }
    }
}

fn merge_object(target: &mut Map<String, Value>, value: Value) {
    let Value::Object(map) = value else {
        return;
    };
    for (key, value) in map {
        target.insert(key, value);
    }
}

fn error_summary(err: &anyhow::Error) -> String {
    short_redacted_error(&format!("{err:#}"))
}

fn short_redacted_error(error: &str) -> String {
    redact_secrets(error)
        .lines()
        .next()
        .unwrap_or("command failed")
        .trim()
        .chars()
        .take(240)
        .collect()
}

fn browser_connect_props(
    status: &BrowserStatus,
    context: &ToolTraceContext,
) -> Option<Map<String, Value>> {
    let mut props = trace_context_props(context);
    match status {
        BrowserStatus::Connecting { attempt } => {
            props.insert("outcome".into(), json!("requested"));
            props.insert("attempt".into(), json!(attempt));
        }
        BrowserStatus::Connected {
            managed,
            remote,
            source,
            remote_timeout_seconds,
            remote_remaining_seconds,
            ..
        } => {
            let profile = if *remote {
                "remote"
            } else if *managed {
                "managed"
            } else {
                "existing"
            };
            props.insert("outcome".into(), json!("completed"));
            props.insert("browser_profile".into(), json!(profile));
            props.insert(
                "browser_source".into(),
                json!(browser_source_category(source)),
            );
            if let Some(seconds) = remote_timeout_seconds {
                props.insert("remote_timeout_seconds".into(), json!(seconds));
            }
            if let Some(seconds) = remote_remaining_seconds {
                props.insert("remote_remaining_seconds".into(), json!(seconds));
            }
        }
        BrowserStatus::Disconnected { reason } if reason != "not_yet_connected" => {
            let (error_type, error) = browser_disconnect_details(reason);
            props.insert(
                "outcome".into(),
                json!(if reason == "user_disconnected" {
                    "disconnected"
                } else {
                    "failed"
                }),
            );
            props.insert("error_type".into(), json!(error_type));
            props.insert("error".into(), json!(error));
        }
        BrowserStatus::Disconnected { .. } => return None,
    }
    Some(props)
}

fn browser_disconnect_details(reason: &str) -> (&'static str, &'static str) {
    let reason = reason.to_ascii_lowercase();
    if reason == "user_disconnected" {
        ("user_disconnected", "browser disconnected by user")
    } else if reason.contains("permission denied") || reason.contains("access denied") {
        (
            "browser_profile_access_denied",
            "browser profile access denied",
        )
    } else if reason.contains("no chrome/chromium executable") {
        ("chrome_not_found", "chrome executable not found")
    } else if reason.contains("singleton") || reason.contains("another chrome instance") {
        (
            "browser_profile_conflict",
            "browser profile is already in use",
        )
    } else if reason.contains("managed chrome") {
        (
            "managed_chrome_launch_failed",
            "managed chrome failed to start",
        )
    } else if reason.contains("remote browser") || reason.contains("hosted") {
        ("remote_browser_failed", "remote browser connection failed")
    } else if reason.contains("websocket") {
        (
            "browser_websocket_failed",
            "browser websocket connection failed",
        )
    } else if reason.contains("timed out") || reason.contains("timeout") {
        ("browser_connect_timeout", "browser connection timed out")
    } else if reason.contains("connection lost") || reason.contains("transport") {
        (
            "browser_transport_disconnected",
            "browser transport disconnected",
        )
    } else {
        ("browser_connect_failed", "browser connection failed")
    }
}

fn browser_source_category(source: &str) -> &str {
    source
        .split_once(':')
        .map_or(source, |(category, _)| category)
}

async fn send_request(
    site: &str,
    command: &str,
    args: Value,
    request_timeout: Duration,
    on_progress: &mut dyn FnMut(ToolProgressEvent),
) -> Result<Value> {
    let paths = daemon_paths()?;
    let (stream, auth) = connect_daemon(&paths).await?;
    let request = DaemonRequest {
        id: request_id(),
        site: site.to_string(),
        command: command.to_string(),
        auth,
        version: PROTOCOL_VERSION.to_string(),
        build_id: process_build_id().to_string(),
        args,
        telemetry: DaemonTelemetry {
            enabled: telemetry_enabled(),
            include_query_text: query_text_enabled(),
        },
    };
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    timeout(request_timeout, async {
        writer
            .write_all(serde_json::to_string(&request)?.as_bytes())
            .await?;
        writer.write_all(b"\n").await?;
        loop {
            line.clear();
            let read = reader.read_line(&mut line).await?;
            if read == 0 || line.trim().is_empty() {
                return Err(anyhow!("empty daemon response"));
            }
            let response = match parse_daemon_line(line.trim_end())? {
                DaemonLine::Progress(event) => {
                    on_progress(event);
                    continue;
                }
                DaemonLine::Response(response) => response,
            };
            if !response.ok {
                let message = response
                    .error
                    .unwrap_or_else(|| "daemon command failed".to_string());
                return Err(match response.code.as_deref() {
                    Some(CODE_VERSION_MISMATCH) => {
                        anyhow::Error::new(DaemonClientError::VersionMismatch(message))
                    }
                    Some(CODE_STALE_DAEMON) => {
                        anyhow::Error::new(DaemonClientError::StaleDaemon(message))
                    }
                    _ => anyhow::Error::new(DaemonClientError::CommandFailed(message)),
                });
            }
            // Legacy daemons (pre build checking) execute commands without
            // validating; their responses lack the build identity. Treat them as
            // stale so they get replaced rather than silently serving old code.
            if !matches!(command, "ping" | "shutdown")
                && (response.version.as_deref() != Some(PROTOCOL_VERSION)
                    || response.build_id.as_deref() != Some(process_build_id()))
            {
                return Err(anyhow::Error::new(DaemonClientError::StaleDaemon(
                    "socai daemon predates build checking or runs a different build".to_string(),
                )));
            }
            return response
                .result
                .ok_or_else(|| anyhow!("daemon response missing result"));
        }
    })
    .await
    .map_err(|_| anyhow!("daemon request timed out after {:?}", request_timeout))?
}

fn parse_daemon_line(line: &str) -> Result<DaemonLine> {
    let value: Value = serde_json::from_str(line)?;
    if value.get("type").and_then(Value::as_str) == Some("progress") {
        let frame: DaemonProgressFrame = serde_json::from_value(value)?;
        return Ok(DaemonLine::Progress(frame.event));
    }
    Ok(DaemonLine::Response(serde_json::from_value(value)?))
}

async fn spawn_daemon() -> Result<()> {
    let paths = daemon_paths()?;
    fs::create_dir_all(&paths.home).await?;
    #[cfg(unix)]
    fs::set_permissions(&paths.home, std::fs::Permissions::from_mode(0o700)).await?;
    cleanup_stale_ipc(&paths).await?;

    spawn_detached_subcommand("__daemon", &paths.log, |_| {})?;

    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while Instant::now() < deadline {
        if send_request("", "ping", json!({}), Duration::from_secs(2), &mut |_| {})
            .await
            .is_ok()
        {
            return Ok(());
        }
        sleep(Duration::from_millis(250)).await;
    }

    Err(anyhow!(
        "socai rust daemon did not become ready; see {}",
        paths.log.display()
    ))
}

#[cfg(unix)]
async fn bind_daemon_listener(paths: &DaemonPaths) -> Result<DaemonListener> {
    let listener = UnixListener::bind(&paths.socket)
        .with_context(|| format!("bind daemon socket {}", paths.socket.display()))?;
    fs::set_permissions(&paths.socket, std::fs::Permissions::from_mode(0o600)).await?;
    Ok(listener)
}

#[cfg(windows)]
async fn bind_daemon_listener(_paths: &DaemonPaths) -> Result<DaemonListener> {
    TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("bind daemon TCP listener")
}

#[cfg(unix)]
async fn write_daemon_endpoint(
    _paths: &DaemonPaths,
    _listener: &DaemonListener,
    _auth_token: Option<&str>,
) -> Result<()> {
    Ok(())
}

#[cfg(windows)]
async fn write_daemon_endpoint(
    paths: &DaemonPaths,
    listener: &DaemonListener,
    auth_token: Option<&str>,
) -> Result<()> {
    let endpoint = DaemonEndpoint {
        host: "127.0.0.1".into(),
        port: listener.local_addr()?.port(),
        token: auth_token
            .ok_or_else(|| anyhow!("missing daemon auth token"))?
            .to_string(),
    };
    fs::write(&paths.endpoint, serde_json::to_vec_pretty(&endpoint)?)
        .await
        .with_context(|| format!("write daemon endpoint {}", paths.endpoint.display()))?;
    Ok(())
}

#[cfg(unix)]
async fn connect_daemon(paths: &DaemonPaths) -> Result<(DaemonStream, Option<String>)> {
    let stream = UnixStream::connect(&paths.socket)
        .await
        .with_context(|| format!("connect daemon socket {}", paths.socket.display()))?;
    Ok((stream, None))
}

#[cfg(windows)]
async fn connect_daemon(paths: &DaemonPaths) -> Result<(DaemonStream, Option<String>)> {
    let text = fs::read_to_string(&paths.endpoint)
        .await
        .with_context(|| format!("read daemon endpoint {}", paths.endpoint.display()))?;
    let endpoint: DaemonEndpoint = serde_json::from_str(&text)
        .with_context(|| format!("parse daemon endpoint {}", paths.endpoint.display()))?;
    let stream = TcpStream::connect((endpoint.host.as_str(), endpoint.port))
        .await
        .with_context(|| {
            format!(
                "connect daemon TCP listener {}:{}",
                endpoint.host, endpoint.port
            )
        })?;
    Ok((stream, Some(endpoint.token)))
}

/// Give a just-stopped daemon a moment to unlink its IPC endpoint so the
/// successor's pre-spawn cleanup doesn't race its exit cleanup.
async fn wait_for_daemon_exit() {
    let Ok(paths) = daemon_paths() else { return };
    #[cfg(unix)]
    let marker = paths.socket.clone();
    #[cfg(windows)]
    let marker = paths.endpoint.clone();
    let deadline = Instant::now() + Duration::from_secs(2);
    while marker.exists() && Instant::now() < deadline {
        sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(unix)]
async fn cleanup_stale_ipc(paths: &DaemonPaths) -> Result<()> {
    // A just-stopped daemon races us removing the same socket (its exit
    // cleanup vs our pre-spawn cleanup), so a missing file is success.
    match fs::remove_file(&paths.socket).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => {
            Err(err).with_context(|| format!("remove stale socket {}", paths.socket.display()))
        }
    }
}

#[cfg(windows)]
async fn cleanup_stale_ipc(paths: &DaemonPaths) -> Result<()> {
    let _ = fs::remove_file(&paths.endpoint).await;
    Ok(())
}

#[cfg(unix)]
fn daemon_auth_token() -> Option<String> {
    None
}

#[cfg(windows)]
fn daemon_auth_token() -> Option<String> {
    Some(uuid::Uuid::new_v4().to_string())
}

fn debug_snapshot_flag(args: &Value) -> bool {
    args.get("debug_snapshot")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Spawn `socai <subcommand>` as a detached background process (own session
/// on unix) with stdout/stderr appended to `log_path`. `configure` can adjust
/// the command (e.g. env) before spawning. Used by the daemon to relaunch
/// itself detached.
pub(crate) fn spawn_detached_subcommand(
    subcommand: &str,
    log_path: &std::path::Path,
    configure: impl FnOnce(&mut std::process::Command),
) -> Result<std::process::Child> {
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("open log {}", log_path.display()))?;
    let stderr = log.try_clone()?;

    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .arg(subcommand)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr));
    configure(&mut command);
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
        .spawn()
        .with_context(|| format!("spawn socai {subcommand}"))
}

/// The socai state dir (`$SOCAI_HOME` or `~/.socai`).
pub(crate) fn socai_home() -> Result<PathBuf> {
    match std::env::var_os("SOCAI_HOME") {
        Some(path) => Ok(PathBuf::from(path)),
        None => Ok(home_dir()
            .context("could not locate user home directory for ~/.socai")?
            .join(".socai")),
    }
}

fn daemon_paths() -> Result<DaemonPaths> {
    let home = socai_home()?;

    Ok(DaemonPaths {
        #[cfg(unix)]
        socket: home.join(SOCKET_NAME),
        #[cfg(windows)]
        endpoint: home.join(ENDPOINT_NAME),
        pid: home.join(PID_NAME),
        log: home.join(LOG_NAME),
        home,
    })
}

fn home_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("HOME") {
        return Some(PathBuf::from(home));
    }
    #[cfg(windows)]
    {
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            return Some(PathBuf::from(profile));
        }
        let drive = std::env::var_os("HOMEDRIVE")?;
        let path = std::env::var_os("HOMEPATH")?;
        return Some(PathBuf::from(format!(
            "{}{}",
            drive.to_string_lossy(),
            path.to_string_lossy()
        )));
    }
    #[allow(unreachable_code)]
    None
}

fn request_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{}-{millis}", std::process::id())
}

/// Best-effort sweep: terminate every lingering socai `__daemon` process, no
/// matter which binary or `SOCAI_HOME` spawned it. The graceful socket shutdown
/// only reaches whoever currently owns the IPC endpoint, so this catches
/// orphans left by restart races or crashes. Returns the number of processes
/// signalled.
pub async fn kill_lingering_helpers() -> usize {
    let pids = lingering_helper_pids();
    if pids.is_empty() {
        return 0;
    }
    for pid in &pids {
        signal_pid(*pid, false);
    }
    // Wait for graceful exits before escalating. SIGTERM routes daemons
    // through browser teardown, whose worst-case chain is bounded in the
    // core: owned-tab close (≤5s, local browsers only) + awaited remote
    // release (≤5s) + the connect-settle wait for a mid-flight connect
    // attempt (≤8s) ≈ 18s. The grace must outlast that whole chain — a
    // SIGKILL landing inside it recreates the timed-out-session leak this
    // teardown exists to prevent. Healthy daemons exit in well under a
    // second, so the poll usually ends on its first iterations; the full
    // wait is only ever paid for genuinely wedged processes.
    const KILL_GRACE: Duration = Duration::from_secs(20);
    const KILL_POLL: Duration = Duration::from_millis(200);
    let deadline = Instant::now() + KILL_GRACE;
    while Instant::now() < deadline {
        if pids.iter().all(|pid| !pid_alive(*pid)) {
            return pids.len();
        }
        sleep(KILL_POLL).await;
    }
    for pid in &pids {
        if pid_alive(*pid) {
            signal_pid(*pid, true);
        }
    }
    pids.len()
}

/// Whether a process still exists, probed with the null signal.
#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    // Safe FFI: kill() with signal 0 checks existence without delivering
    // anything. EPERM would also mean "exists", but socai daemons run as the
    // caller's own user, so a plain success check suffices.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(not(unix))]
fn pid_alive(_pid: u32) -> bool {
    false
}

/// PIDs of running `socai __daemon` processes (excluding the caller).
/// Identified by command line so it spans every install path.
#[cfg(unix)]
fn lingering_helper_pids() -> Vec<u32> {
    let me = std::process::id();
    let Ok(output) = std::process::Command::new("ps")
        .args(["-axo", "pid=,command="])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid_str, cmd) = line.split_once(' ')?;
            let pid: u32 = pid_str.trim().parse().ok()?;
            if pid == me {
                return None;
            }
            // The binary is always named `socai`; matching the exact
            // `socai __daemon` tail avoids hitting the `socai stop` process or
            // unrelated programs.
            cmd.contains("socai __daemon").then_some(pid)
        })
        .collect()
}

#[cfg(windows)]
fn lingering_helper_pids() -> Vec<u32> {
    // No cheap command-line process filter on Windows; the graceful socket
    // shutdown remains the stop path there.
    Vec::new()
}

#[cfg(unix)]
fn signal_pid(pid: u32, force: bool) {
    let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
    // Safe FFI: kill() with a signal number; failures (already exited, not
    // ours) are ignored on purpose.
    unsafe {
        libc::kill(pid as libc::pid_t, sig);
    }
}

#[cfg(windows)]
fn signal_pid(_pid: u32, _force: bool) {}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn trace_context() -> ToolTraceContext {
        ToolTraceContext {
            request_id: "request-1".into(),
            site_id: "xhs".into(),
            command: "search".into(),
            tool_name: "search".into(),
        }
    }

    fn recording_trace(enabled: bool) -> (ToolCallTrace, RecordedEvents) {
        let (telemetry, recorded) = ToolTelemetry::recording(enabled);
        let trace = ToolCallTrace::start_with_telemetry(
            telemetry,
            None,
            None,
            "request-1",
            "xhs",
            "search",
            "search",
            &json!({ "query": "synthetic", "num_notes": 1 }),
            false,
        );
        (trace, recorded)
    }

    #[test]
    fn tool_call_trace_records_exact_success_lifecycle() {
        let (trace, recorded) = recording_trace(true);
        trace.capture_browser_status(&BrowserStatus::Connecting { attempt: 1 });
        let result = Ok(json!({ "data": { "ok": true, "notes": [{}] } }));
        trace.finish(&result);

        let events = recorded.lock().expect("recorded telemetry lock");
        let names: Vec<_> = events.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "socai_tool_call_start",
                "socai_browser_connect",
                "socai_tool_call"
            ]
        );
        assert_eq!(events[2].1.get("outcome"), Some(&json!("completed")));
        assert_eq!(events[2].1.get("ok"), Some(&json!(true)));
    }

    #[test]
    fn tool_call_trace_drop_records_one_distinct_interruption() {
        let recorded = {
            let (trace, recorded) = recording_trace(true);
            drop(trace);
            recorded
        };
        let events = recorded.lock().expect("recorded telemetry lock");
        let names: Vec<_> = events.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            vec!["socai_tool_call_start", "socai_tool_call_interrupted"]
        );
        assert_eq!(events[1].1.get("outcome"), Some(&json!("interrupted")));
        assert_eq!(
            events[1].1.get("error_type"),
            Some(&json!("command_interrupted"))
        );
    }

    #[test]
    fn tool_call_trace_opt_out_records_no_lifecycle_events() {
        let (trace, recorded) = recording_trace(false);
        trace.capture_browser_status(&BrowserStatus::Connecting { attempt: 1 });
        drop(trace);
        assert!(recorded.lock().expect("recorded telemetry lock").is_empty());
    }

    #[tokio::test]
    async fn interrupted_trace_hands_command_gate_to_cleanup() {
        let command_gate = Arc::new(Mutex::new(()));
        let guard = command_gate.clone().lock_owned().await;
        let (telemetry, recorded) = ToolTelemetry::recording(true);
        let trace = ToolCallTrace::start_with_telemetry(
            telemetry,
            Some(SocaiRuntime::new()),
            Some(guard),
            "request-1",
            "xhs",
            "search",
            "search",
            &json!({ "query": "synthetic" }),
            false,
        );
        drop(trace);

        let _next_command = timeout(Duration::from_secs(1), command_gate.lock())
            .await
            .expect("cleanup releases command gate");
        let events = recorded.lock().expect("recorded telemetry lock");
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].0, "socai_tool_call_interrupted");
    }

    #[test]
    fn tool_call_terminal_props_cover_success_failure_and_interruption() {
        let mut completed = trace_context_props(&trace_context());
        let success = Ok(json!({ "data": { "ok": true, "notes": [{}, {}] } }));
        finish_tool_call_props(&mut completed, 42, Some(&success));
        assert_eq!(completed.get("outcome"), Some(&json!("completed")));
        assert_eq!(completed.get("ok"), Some(&json!(true)));
        assert_eq!(completed.get("duration_ms"), Some(&json!(42)));
        assert_eq!(completed.get("notes_count"), Some(&json!(2)));

        let mut failed = trace_context_props(&trace_context());
        let failure: Result<Value> = Err(anyhow!(
            "CDP disconnected with Authorization: Bearer very-secret-token"
        ));
        finish_tool_call_props(&mut failed, 84, Some(&failure));
        assert_eq!(failed.get("outcome"), Some(&json!("failed")));
        assert_eq!(failed.get("ok"), Some(&json!(false)));
        let error = failed
            .get("error")
            .and_then(Value::as_str)
            .expect("failure error");
        assert!(!error.contains("very-secret-token"));

        let mut interrupted = trace_context_props(&trace_context());
        finish_tool_call_props(&mut interrupted, 126, None);
        assert_eq!(interrupted.get("outcome"), Some(&json!("interrupted")));
        assert_eq!(interrupted.get("ok"), Some(&json!(false)));
        assert_eq!(
            interrupted.get("error_type"),
            Some(&json!("command_interrupted"))
        );
    }

    #[test]
    fn browser_connect_props_include_request_correlation_for_each_stage() {
        let context = trace_context();
        let requested = browser_connect_props(&BrowserStatus::Connecting { attempt: 2 }, &context)
            .expect("connecting event");
        assert_eq!(requested.get("request_id"), Some(&json!("request-1")));
        assert_eq!(requested.get("outcome"), Some(&json!("requested")));
        assert_eq!(requested.get("attempt"), Some(&json!(2)));

        let connected = browser_connect_props(
            &BrowserStatus::Connected {
                endpoint: "ws://127.0.0.1:9222/devtools/browser/private".into(),
                browser_version: "Chrome/140".into(),
                page_count: 1,
                source: "managed_profile:/private/profile".into(),
                managed: true,
                remote: false,
                remote_timeout_seconds: None,
                remote_remaining_seconds: None,
                user_data_dir: Some("/private/profile".into()),
            },
            &context,
        )
        .expect("connected event");
        assert_eq!(connected.get("outcome"), Some(&json!("completed")));
        assert_eq!(connected.get("browser_profile"), Some(&json!("managed")));
        assert_eq!(
            connected.get("browser_source"),
            Some(&json!("managed_profile"))
        );
        assert!(!connected.contains_key("endpoint"));
        assert!(!connected.contains_key("user_data_dir"));

        let disconnected = browser_connect_props(
            &BrowserStatus::Disconnected {
                reason: "managed chrome failed for profile /Users/private/.socai/chrome-profile \
                         with Bearer very-secret-token"
                    .into(),
            },
            &context,
        )
        .expect("disconnected event");
        assert_eq!(disconnected.get("outcome"), Some(&json!("failed")));
        assert_eq!(
            disconnected.get("error_type"),
            Some(&json!("managed_chrome_launch_failed"))
        );
        let error = disconnected
            .get("error")
            .and_then(Value::as_str)
            .expect("disconnect error");
        assert!(!error.contains("very-secret-token"));
        assert!(!error.contains("/Users/private"));
    }

    #[test]
    fn browser_connect_props_ignore_initial_disconnected_state() {
        let event = browser_connect_props(
            &BrowserStatus::Disconnected {
                reason: "not_yet_connected".into(),
            },
            &trace_context(),
        );
        assert!(event.is_none());
    }
}
