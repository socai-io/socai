use crate::tracking::{query_text_enabled, telemetry_enabled, Telemetry};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use socai_core::runtime::SocaiRuntime;
use socai_core::sites::{find_site, SiteCommand, SiteSpec};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(windows)]
use tokio::net::{TcpListener, TcpStream};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, Notify};
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
const IDLE_TIMEOUT: Duration = Duration::from_secs(3 * 60 * 60);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(90);

/// The daemon only serves a CLI of the exact same build. A version mismatch
/// is a hard error the user has to reconcile (update or rebuild); a same-
/// version binary change (dev rebuild) restarts the daemon automatically.
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

/// Daemon failures that need different client-side recovery: a version
/// mismatch must fail, a stale daemon is restarted automatically.
#[derive(Debug)]
enum DaemonClientError {
    VersionMismatch(String),
    StaleDaemon(String),
}

impl std::fmt::Display for DaemonClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DaemonClientError::VersionMismatch(message)
            | DaemonClientError::StaleDaemon(message) => f.write_str(message),
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
    auth_token: Option<String>,
    last_activity: Instant,
}

pub async fn run_daemon() -> Result<()> {
    // Pin the binary fingerprint before a rebuild can swap the file under us.
    let _ = process_build_id();
    let paths = daemon_paths()?;
    fs::create_dir_all(&paths.home).await?;
    cleanup_stale_ipc(&paths).await?;

    let listener = bind_daemon_listener(&paths).await?;
    let auth_token = daemon_auth_token();
    write_daemon_endpoint(&paths, &listener, auth_token.as_deref()).await?;
    fs::write(&paths.pid, std::process::id().to_string()).await?;

    let runtime = SocaiRuntime::new();
    let telemetry = Telemetry::new(&paths.home);
    let state = Arc::new(Mutex::new(DaemonState {
        runtime,
        telemetry,
        auth_token,
        last_activity: Instant::now(),
    }));
    let stop = Arc::new(Notify::new());
    let mut idle_check = tokio::time::interval(Duration::from_secs(60));

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                let (stream, _) = accept_result.context("accept daemon client")?;
                let state = state.clone();
                let stop = stop.clone();
                tokio::spawn(async move {
                    if let Err(err) = serve_client(stream, state, stop).await {
                        eprintln!("daemon client error: {err:#}");
                    }
                });
            }
            _ = idle_check.tick() => {
                if state.lock().await.last_activity.elapsed() > IDLE_TIMEOUT {
                    break;
                }
            }
            _ = stop.notified() => break,
        }
    }

    state.lock().await.shutdown().await?;
    cleanup_stale_ipc(&paths).await?;
    let _ = fs::remove_file(&paths.pid).await;
    Ok(())
}

pub async fn send_or_spawn(
    site: &str,
    command: &str,
    args: Value,
    command_timeout: Duration,
) -> Result<Value> {
    let err = match send_request(site, command, args.clone(), command_timeout).await {
        Ok(result) => return Ok(result),
        Err(err) => err,
    };
    match err.downcast_ref::<DaemonClientError>() {
        // A different release serving this CLI is never acceptable — the user
        // has to bring both onto the same version.
        Some(DaemonClientError::VersionMismatch(_)) => return Err(err),
        // Same version, different binary (typically a dev rebuild): replace
        // the daemon so commands never run on stale code.
        Some(DaemonClientError::StaleDaemon(_)) => {
            eprintln!("socai daemon was started from a different build; restarting it");
            let _ = stop_daemon().await;
        }
        None => {}
    }
    spawn_daemon().await?;
    send_request(site, command, args, command_timeout).await
}

pub async fn stop_daemon() -> Result<bool> {
    match send_request("", "shutdown", json!({}), Duration::from_secs(10)).await {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

async fn serve_client(
    stream: DaemonStream,
    state: Arc<Mutex<DaemonState>>,
    stop: Arc<Notify>,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    while reader.read_line(&mut line).await? != 0 {
        let request: DaemonRequest = serde_json::from_str(line.trim_end())?;
        let mut disconnect_probe = String::new();
        tokio::select! {
            response = handle_request(request, state.clone(), stop.clone()) => {
                writer
                    .write_all(serde_json::to_string(&response)?.as_bytes())
                    .await?;
                writer.write_all(b"\n").await?;
            }
            read = reader.read_line(&mut disconnect_probe) => {
                if read? == 0 {
                    return Ok(());
                }
                anyhow::bail!("daemon client sent another request before the previous response");
            }
        }
        line.clear();
    }

    Ok(())
}

async fn handle_request(
    request: DaemonRequest,
    state: Arc<Mutex<DaemonState>>,
    stop: Arc<Notify>,
) -> DaemonResponse {
    let id = request.id.clone();
    let command = request.command.clone();
    let telemetry = request.telemetry.clone();
    let auth_token = { state.lock().await.auth_token.clone() };
    if !daemon_request_authorized(request.auth.as_deref(), auth_token.as_deref()) {
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

        let site_id = if request.site.trim().is_empty() {
            "xhs"
        } else {
            request.site.trim()
        };
        let site = find_site(site_id).ok_or_else(|| anyhow!("unknown site: {site_id}"))?;
        let spec = site
            .command(&command)
            .ok_or_else(|| anyhow!("unknown {site_id} command: {command}"))?;

        let mut state = state.lock().await;
        state.last_activity = Instant::now();
        state
            .run_site_command(&id, site, spec, request.args, &telemetry)
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
    async fn run_site_command(
        &mut self,
        request_id: &str,
        site: &'static SiteSpec,
        spec: &'static SiteCommand,
        args: Value,
        telemetry: &DaemonTelemetry,
    ) -> Result<Value> {
        let started = Instant::now();
        let result = async {
            let debug_snapshot = debug_snapshot_flag(&args);
            // Route CDP through the bridge (spawning it on first use) so the
            // chrome connection — and its allow-debugging consent — survives
            // daemon restarts. Falls back to a direct connect when the bridge
            // can't start.
            crate::bridge::ensure_bridge_env().await;
            let page = self
                .runtime
                .ensure_site_page(site.id, site.home_url)
                .await?;
            (spec.run)(page, args.clone(), debug_snapshot).await
        }
        .await;
        self.track_tool_trace(
            request_id,
            site.id,
            spec.name,
            spec.tool_name,
            &args,
            telemetry,
            started,
            &result,
        );
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn track_tool_trace(
        &self,
        request_id: &str,
        site_id: &str,
        command: &str,
        tool_name: &str,
        input: &Value,
        telemetry: &DaemonTelemetry,
        started: Instant,
        result: &Result<Value>,
    ) {
        if !telemetry.enabled {
            return;
        }

        let mut props = base_trace_props(request_id, site_id, command, tool_name);
        props.insert(
            "duration_ms".into(),
            json!(started.elapsed().as_millis() as u64),
        );
        merge_object(&mut props, command_arg_summary(telemetry, input));
        match result {
            Ok(value) => {
                props.insert("ok".into(), json!(true));
                merge_object(&mut props, result_metrics(value));
            }
            Err(err) => {
                props.insert("ok".into(), json!(false));
                props.insert("error".into(), json!(error_summary(err)));
            }
        }

        self.telemetry
            .capture("socai_cli_tool_trace", Value::Object(props));
    }

    async fn shutdown(&mut self) -> Result<()> {
        let _ = self.runtime.close_all_site_sessions().await;
        self.runtime.disconnect_browser().await;
        Ok(())
    }
}

fn base_trace_props(
    request_id: &str,
    site_id: &str,
    command: &str,
    tool_name: &str,
) -> Map<String, Value> {
    let mut props = Map::new();
    props.insert("request_id".into(), json!(request_id));
    props.insert("command".into(), json!(command));
    props.insert("tool_name".into(), json!(tool_name));
    props.insert("site".into(), json!(site_id));
    props
}

fn command_arg_summary(telemetry: &DaemonTelemetry, args: &Value) -> Value {
    let mut props = Map::new();
    if let Some(query) = args.get("query").and_then(Value::as_str) {
        insert_query_props(&mut props, telemetry, query);
    }
    if let Some(metadata) = explicit_param_metadata(args) {
        props.insert("metadata".into(), Value::Object(metadata));
    }
    if args.get("note_id").and_then(Value::as_str).is_some() {
        props.insert("note_id_present".into(), json!(true));
    }
    Value::Object(props)
}

fn explicit_param_metadata(args: &Value) -> Option<Map<String, Value>> {
    let mut metadata = Map::new();
    insert_optional_str_metadata(&mut metadata, args, "tab_label", "tab");
    insert_optional_i64_metadata(&mut metadata, args, "num_notes", "num_notes");
    insert_true_bool_metadata(&mut metadata, args, "debug_snapshot", "debug_snapshot");
    if metadata.is_empty() {
        None
    } else {
        Some(metadata)
    }
}

fn insert_query_props(props: &mut Map<String, Value>, telemetry: &DaemonTelemetry, query: &str) {
    props.insert("query_len".into(), json!(query.chars().count()));
    props.insert(
        "query_text_enabled".into(),
        json!(telemetry.include_query_text),
    );
    if telemetry.include_query_text {
        props.insert("query_text".into(), json!(query));
    }
}

fn insert_optional_str_metadata(
    metadata: &mut Map<String, Value>,
    args: &Value,
    arg_key: &str,
    metadata_key: &str,
) {
    let Some(value) = args
        .get(arg_key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    metadata.insert(metadata_key.to_string(), json!(value));
}

fn insert_optional_i64_metadata(
    metadata: &mut Map<String, Value>,
    args: &Value,
    arg_key: &str,
    metadata_key: &str,
) {
    let Some(value) = args.get(arg_key).and_then(Value::as_i64) else {
        return;
    };
    metadata.insert(metadata_key.to_string(), json!(value));
}

fn insert_true_bool_metadata(
    metadata: &mut Map<String, Value>,
    args: &Value,
    arg_key: &str,
    metadata_key: &str,
) {
    if args.get(arg_key).and_then(Value::as_bool) == Some(true) {
        metadata.insert(metadata_key.to_string(), json!(true));
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

fn result_metrics(value: &Value) -> Value {
    let mut props = Map::new();
    let data = value.get("data").unwrap_or(value);
    if let Some(ok) = data.get("ok").and_then(Value::as_bool) {
        props.insert("result_ok".into(), json!(ok));
    }
    if let Some(cards) = data.get("cards").and_then(Value::as_array) {
        props.insert("cards_count".into(), json!(cards.len()));
    }
    if let Some(cards) = data
        .get("search")
        .and_then(|search| search.get("cards"))
        .and_then(Value::as_array)
    {
        props.insert("search_cards_count".into(), json!(cards.len()));
    }
    if let Some(cards) = data.get("selected_cards").and_then(Value::as_array) {
        props.insert("selected_cards_count".into(), json!(cards.len()));
    }
    if let Some(notes) = data.get("notes").and_then(Value::as_array) {
        props.insert("notes_count".into(), json!(notes.len()));
        let skipped = notes
            .iter()
            .filter(|note| note.get("skipped").is_some())
            .count();
        props.insert("notes_skipped_count".into(), json!(skipped));
    }
    if value.get("run_dir").is_some() {
        props.insert("has_run_dir".into(), json!(true));
    }
    Value::Object(props)
}

fn error_summary(err: &anyhow::Error) -> String {
    let rendered = format!("{err:#}");
    let first = rendered.lines().next().unwrap_or("command failed").trim();
    first.chars().take(240).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_summary_includes_query_text_by_default() {
        let summary = command_arg_summary(
            &DaemonTelemetry::default(),
            &json!({ "query": "运营爆款思路" }),
        );
        let object = summary.as_object().expect("summary is an object");
        assert_eq!(object.get("query_text"), Some(&json!("运营爆款思路")));
        assert_eq!(object.get("query_text_enabled"), Some(&json!(true)));
        assert_eq!(object.get("query_len"), Some(&json!(6)));
    }

    #[test]
    fn command_summary_redacts_query_text_when_disabled() {
        let telemetry = DaemonTelemetry {
            enabled: true,
            include_query_text: false,
        };
        let summary = command_arg_summary(&telemetry, &json!({ "query": "运营爆款思路" }));
        let object = summary.as_object().expect("summary is an object");
        assert!(!object.contains_key("query_text"));
        assert_eq!(object.get("query_text_enabled"), Some(&json!(false)));
        assert_eq!(object.get("query_len"), Some(&json!(6)));
    }

    #[test]
    fn command_summary_omits_defaulted_optional_params() {
        let summary = command_arg_summary(
            &DaemonTelemetry::default(),
            &json!({ "query": "x", "debug_snapshot": false }),
        );
        let object = summary.as_object().expect("summary is an object");
        assert!(!object.contains_key("metadata"));
    }

    #[test]
    fn command_summary_tracks_explicit_optional_params_as_metadata() {
        let summary = command_arg_summary(
            &DaemonTelemetry::default(),
            &json!({
                "query": "x",
                "tab_label": "latest",
                "num_notes": 12,
                "debug_snapshot": true
            }),
        );
        let object = summary.as_object().expect("summary is an object");
        let metadata = object
            .get("metadata")
            .and_then(Value::as_object)
            .expect("metadata object");
        assert_eq!(metadata.get("tab"), Some(&json!("latest")));
        assert_eq!(metadata.get("num_notes"), Some(&json!(12)));
        assert_eq!(metadata.get("debug_snapshot"), Some(&json!(true)));
        assert!(!object.contains_key("tab_label"));
        assert!(!object.contains_key("num_notes"));
    }

    #[test]
    fn result_metrics_extracts_safe_counts() {
        let metrics = result_metrics(&json!({
            "run_dir": "/tmp/socai-run",
            "data": {
                "ok": true,
                "cards": [{}, {}],
                "search": { "cards": [{}, {}, {}] },
                "selected_cards": [{}],
                "notes": [
                    { "id": "1", "body": "must not be copied" },
                    { "skipped": "missing note" },
                    { "skipped": true, "comments": ["must not be copied"] }
                ]
            }
        }));
        let object = metrics.as_object().expect("metrics is an object");
        assert_eq!(object.get("result_ok"), Some(&json!(true)));
        assert_eq!(object.get("cards_count"), Some(&json!(2)));
        assert_eq!(object.get("search_cards_count"), Some(&json!(3)));
        assert_eq!(object.get("selected_cards_count"), Some(&json!(1)));
        assert_eq!(object.get("notes_count"), Some(&json!(3)));
        assert_eq!(object.get("notes_skipped_count"), Some(&json!(2)));
        assert_eq!(object.get("has_run_dir"), Some(&json!(true)));
        assert!(!object.contains_key("body"));
        assert!(!object.contains_key("comments"));
    }
}

async fn send_request(
    site: &str,
    command: &str,
    args: Value,
    request_timeout: Duration,
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
        reader.read_line(&mut line).await?;
        if line.trim().is_empty() {
            return Err(anyhow!("empty daemon response"));
        }
        let response: DaemonResponse = serde_json::from_str(line.trim_end())?;
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
                _ => anyhow!("{message}"),
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
        response
            .result
            .ok_or_else(|| anyhow!("daemon response missing result"))
    })
    .await
    .map_err(|_| anyhow!("daemon request timed out after {:?}", request_timeout))?
}

async fn spawn_daemon() -> Result<()> {
    let paths = daemon_paths()?;
    fs::create_dir_all(&paths.home).await?;
    cleanup_stale_ipc(&paths).await?;

    spawn_detached_subcommand("__daemon", &paths.log, |_| {})?;

    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while Instant::now() < deadline {
        if send_request("", "ping", json!({}), Duration::from_secs(2))
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
    UnixListener::bind(&paths.socket)
        .with_context(|| format!("bind daemon socket {}", paths.socket.display()))
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

#[cfg(unix)]
async fn cleanup_stale_ipc(paths: &DaemonPaths) -> Result<()> {
    if paths.socket.exists() {
        fs::remove_file(&paths.socket)
            .await
            .with_context(|| format!("remove stale socket {}", paths.socket.display()))?;
    }
    Ok(())
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
/// the command (e.g. env) before spawning. Shared by the daemon and the CDP
/// bridge.
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

/// The socai state dir (`$SOCAI_HOME` or `~/.socai`), shared by the daemon
/// and the CDP bridge.
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
