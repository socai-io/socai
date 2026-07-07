use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;
use uuid::Uuid;

pub mod tool_call;
pub mod trace;

const EVENT_SCHEMA_VERSION: u32 = 1;
const TELEMETRY_ENDPOINT: &str = "https://socai.io/v1/events";
const TRACES_ENDPOINT: &str = "https://socai.io/v1/traces";
const CHANNEL_CAPACITY: usize = 512;
const REMOTE_BATCH_SIZE: usize = 25;
const REMOTE_FLUSH_INTERVAL: Duration = Duration::from_secs(5);

/// Which socai surface is emitting telemetry. Carried verbatim into the `source`
/// field of every event and used to decide which device context is meaningful
/// (terminal/parent-process detection only applies to the CLI daemon).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetrySource {
    CliDaemon,
    Desktop,
}

impl TelemetrySource {
    fn as_str(self) -> &'static str {
        match self {
            TelemetrySource::CliDaemon => "cli_daemon",
            TelemetrySource::Desktop => "desktop",
        }
    }

    /// Terminal/parent-process detection is a CLI concept. A desktop GUI has no
    /// meaningful terminal, so those fields are skipped for `Desktop`.
    fn collects_terminal_context(self) -> bool {
        matches!(self, TelemetrySource::CliDaemon)
    }
}

#[derive(Clone)]
pub struct Telemetry {
    sender: mpsc::Sender<QueuedItem>,
}

#[derive(Debug)]
enum QueuedItem {
    Event(QueuedEvent),
    /// Path to a run's `trace.json`; the worker reads, enriches, and uploads it.
    TraceFile(PathBuf),
}

#[derive(Debug)]
struct QueuedEvent {
    name: String,
    properties: Value,
}

#[derive(Clone)]
struct WorkerConfig {
    install_id: String,
    session_id: String,
    source: TelemetrySource,
    local_path: PathBuf,
}

#[derive(Debug, Clone)]
struct DeviceInfo {
    os_version: String,
    os_kernel_version: String,
    memory_total_mb: Option<u64>,
    cpu_count: Option<usize>,
    cpu_model: String,
    terminal_app: String,
    parent_process: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct IdentityFile {
    install_id: String,
}

impl Telemetry {
    pub fn new(home: &Path, source: TelemetrySource) -> Self {
        let install_id = load_or_create_install_id(home);
        let session_id = new_session_id();
        let local_path = home.join("telemetry/events.jsonl");

        let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
        let config = WorkerConfig {
            install_id,
            session_id,
            source,
            local_path,
        };
        spawn_worker(receiver, config);

        Self { sender }
    }

    pub fn capture(&self, name: impl Into<String>, properties: Value) {
        let _ = self.sender.try_send(QueuedItem::Event(QueuedEvent {
            name: name.into(),
            properties,
        }));
    }

    /// Fire-and-forget upload of a completed run's `trace.json` (written by the
    /// agent loop). Missing file — e.g. a cancelled run — is a silent no-op.
    pub fn upload_run_trace(&self, run_dir: &Path) {
        let _ = self
            .sender
            .try_send(QueuedItem::TraceFile(run_dir.join("trace.json")));
    }
}

/// Run the flush worker. When a Tokio runtime is already entered (the CLI daemon
/// path) it spawns onto it; otherwise — e.g. the Tauri desktop shell constructs
/// `Telemetry` from a plain synchronous `fn run()` with no ambient runtime — it
/// owns a dedicated current-thread runtime so `capture()` stays fire-and-forget
/// regardless of the caller's context.
fn spawn_worker(receiver: mpsc::Receiver<QueuedItem>, config: WorkerConfig) {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(worker_loop(receiver, config));
        }
        Err(_) => {
            let _ = std::thread::Builder::new()
                .name("socai-telemetry".into())
                .spawn(move || {
                    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    else {
                        return;
                    };
                    rt.block_on(worker_loop(receiver, config));
                });
        }
    }
}

async fn worker_loop(mut receiver: mpsc::Receiver<QueuedItem>, config: WorkerConfig) {
    let client = reqwest::Client::new();
    let mut remote_batch: Vec<Value> = Vec::new();
    let mut flush_tick = tokio::time::interval(REMOTE_FLUSH_INTERVAL);
    flush_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            maybe_item = receiver.recv() => {
                let Some(item) = maybe_item else {
                    break;
                };
                match item {
                    QueuedItem::Event(event) => {
                        let timestamp_ms = now_ms();
                        let properties = enrich_properties(event.properties, &config, timestamp_ms);
                        let row = local_row(&event.name, &config.install_id, timestamp_ms, &properties);
                        let _ = append_jsonl(&config.local_path, &row).await;

                        remote_batch.push(remote_event(&event.name, &config.install_id, &properties));
                        if remote_batch.len() >= REMOTE_BATCH_SIZE {
                            flush_remote(&client, &mut remote_batch).await;
                        }
                    }
                    QueuedItem::TraceFile(path) => {
                        upload_trace_file(&client, &config, &path).await;
                    }
                }
            }
            _ = flush_tick.tick() => {
                flush_remote(&client, &mut remote_batch).await;
            }
        }
    }

    flush_remote(&client, &mut remote_batch).await;
}

/// Read a run's `trace.json`, append identity resource attributes, and POST it
/// to the traces proxy. Best-effort at every step: a missing/invalid file or a
/// failed upload is dropped silently, matching the events contract.
async fn upload_trace_file(client: &reqwest::Client, config: &WorkerConfig, path: &Path) {
    let Ok(bytes) = tokio::fs::read(path).await else {
        return;
    };
    let Ok(mut payload) = serde_json::from_slice::<Value>(&bytes) else {
        return;
    };
    enrich_trace_resource(&mut payload, config);
    let _ = client.post(traces_endpoint()).json(&payload).send().await;
}

/// The on-disk trace stays identity-free so run dirs can be shared; the
/// uploaded copy carries the same source/install identity as events.
fn enrich_trace_resource(payload: &mut Value, config: &WorkerConfig) {
    let Some(attributes) = payload
        .get_mut("resourceSpans")
        .and_then(Value::as_array_mut)
        .and_then(|spans| spans.first_mut())
        .and_then(|resource_spans| resource_spans.get_mut("resource"))
        .and_then(|resource| resource.get_mut("attributes"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for (key, value) in [
        ("socai.source", config.source.as_str()),
        ("socai.install_id", config.install_id.as_str()),
        ("socai.app_session_id", config.session_id.as_str()),
    ] {
        attributes.push(json!({ "key": key, "value": { "stringValue": value } }));
    }
}

/// `SOCAI_TRACES_ENDPOINT` overrides the production proxy for local testing
/// (e.g. a `vercel dev` instance of the site).
fn traces_endpoint() -> String {
    std::env::var("SOCAI_TRACES_ENDPOINT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| TRACES_ENDPOINT.to_string())
}

fn enrich_properties(properties: Value, config: &WorkerConfig, timestamp_ms: u64) -> Value {
    let mut map = match properties {
        Value::Object(map) => map,
        other => {
            let mut map = Map::new();
            map.insert("value".into(), other);
            map
        }
    };
    map.insert("schema_version".into(), json!(EVENT_SCHEMA_VERSION));
    map.insert("app".into(), json!("socai"));
    map.insert("source".into(), json!(config.source.as_str()));
    map.insert("app_version".into(), json!(env!("CARGO_PKG_VERSION")));
    map.insert("platform".into(), json!(std::env::consts::OS));
    map.insert("session_id".into(), json!(config.session_id));
    map.insert("created_at_ms".into(), json!(timestamp_ms));

    let device = device_info();
    insert_nonempty(&mut map, "os_version", &device.os_version);
    insert_nonempty(&mut map, "os_kernel_version", &device.os_kernel_version);
    if config.source.collects_terminal_context() {
        insert_nonempty(&mut map, "terminal_app", &device.terminal_app);
        insert_nonempty(&mut map, "parent_process", &device.parent_process);
    }
    if let Some(memory_total_mb) = device.memory_total_mb {
        map.insert("memory_total_mb".into(), json!(memory_total_mb));
    }
    if let Some(cpu_count) = device.cpu_count {
        map.insert("cpu_count".into(), json!(cpu_count));
    }
    insert_nonempty(&mut map, "cpu_model", &device.cpu_model);

    Value::Object(map)
}

fn insert_nonempty(map: &mut Map<String, Value>, key: &str, value: &str) {
    if !value.trim().is_empty() {
        map.insert(key.to_string(), json!(value));
    }
}

fn local_row(event_name: &str, install_id: &str, timestamp_ms: u64, properties: &Value) -> Value {
    json!({
        "event": event_name,
        "install_id": install_id,
        "created_at_ms": timestamp_ms,
        "properties": properties,
    })
}

fn remote_event(event_name: &str, install_id: &str, properties: &Value) -> Value {
    let mut map = match properties {
        Value::Object(map) => map.clone(),
        _ => Map::new(),
    };
    // The proxy uses this for validation/routing and strips it before Axiom.
    map.insert("event".into(), json!(event_name));
    map.insert("install_id".into(), json!(install_id));
    map.remove("created_at_ms");
    Value::Object(map)
}

async fn flush_remote(client: &reqwest::Client, batch: &mut Vec<Value>) {
    if batch.is_empty() {
        return;
    }

    let events = std::mem::take(batch);
    let body = json!({ "events": events });
    let _ = client.post(TELEMETRY_ENDPOINT).json(&body).send().await;
}

async fn append_jsonl(path: &Path, row: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    let line = serde_json::to_string(row).map_err(std::io::Error::other)?;
    file.write_all(line.as_bytes()).await?;
    file.write_all(b"\n").await
}

fn load_or_create_install_id(home: &Path) -> String {
    let path = home.join("telemetry/identity.json");
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(identity) = serde_json::from_slice::<IdentityFile>(&bytes) {
            let id = identity.install_id.trim();
            if Uuid::parse_str(id).is_ok() {
                return id.to_string();
            }
        }
    }

    let install_id = new_install_id();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        &path,
        serde_json::to_vec_pretty(&IdentityFile {
            install_id: install_id.clone(),
        })
        .unwrap_or_else(|_| b"{}".to_vec()),
    );
    install_id
}

fn new_install_id() -> String {
    Uuid::new_v4().to_string()
}

fn new_session_id() -> String {
    Uuid::new_v4().to_string()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

pub fn telemetry_enabled() -> bool {
    !env_value_is("SOCAI_TELEMETRY", &["0", "false", "off", "disabled", "no"])
}

pub fn query_text_enabled() -> bool {
    !env_value_is(
        "SOCAI_TELEMETRY_QUERY_TEXT",
        &["0", "false", "off", "disabled", "no"],
    )
}

fn device_info() -> &'static DeviceInfo {
    static DEVICE_INFO: OnceLock<DeviceInfo> = OnceLock::new();
    DEVICE_INFO.get_or_init(|| {
        // Reuse the shared machine snapshot so uploaded device fields match the
        // local OCR diagnostics exactly. Terminal/parent-process are telemetry's
        // own session context and stay here.
        let machine = crate::util::machine::machine_info();
        DeviceInfo {
            os_version: machine.os_version.clone(),
            os_kernel_version: machine.os_kernel_version.clone(),
            memory_total_mb: machine.memory_total_mb,
            cpu_count: machine.cpu_count,
            cpu_model: machine.cpu_model.clone(),
            terminal_app: terminal_app(),
            parent_process: parent_process_name(),
        }
    })
}

fn terminal_app() -> String {
    if std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some()
        || std::env::var_os("GHOSTTY_BIN_DIR").is_some()
    {
        return "Ghostty".to_string();
    }
    if std::env::var_os("WEZTERM_EXECUTABLE").is_some() {
        return "WezTerm".to_string();
    }
    if std::env::var_os("KITTY_WINDOW_ID").is_some() {
        return "kitty".to_string();
    }
    if std::env::var_os("ALACRITTY_WINDOW_ID").is_some() {
        return "Alacritty".to_string();
    }
    if std::env::var_os("VSCODE_PID").is_some() {
        return "VS Code".to_string();
    }
    if let Ok(term_program) = std::env::var("TERM_PROGRAM") {
        let trimmed = term_program.trim();
        if !trimmed.is_empty() {
            return match trimmed {
                "Apple_Terminal" => "Terminal".to_string(),
                "iTerm.app" => "iTerm".to_string(),
                other => other.to_string(),
            };
        }
    }
    if let Ok(lc_terminal) = std::env::var("LC_TERMINAL") {
        let trimmed = lc_terminal.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if parent_process_name().to_ascii_lowercase().contains("codex") {
        return "Codex".to_string();
    }
    std::env::var("TERM")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
}

#[cfg(unix)]
fn parent_process_name() -> String {
    let ppid = unsafe { libc::getppid() };
    crate::util::machine::command_output("ps", &["-p", &ppid.to_string(), "-o", "comm="])
}

#[cfg(not(unix))]
fn parent_process_name() -> String {
    String::new()
}

fn env_value_is(name: &str, values: &[&str]) -> bool {
    let Ok(value) = std::env::var(name) else {
        return false;
    };
    let value = value.trim().to_ascii_lowercase();
    values.iter().any(|candidate| value == *candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvGuard {
        name: &'static str,
        previous: Option<OsString>,
    }

    impl EnvGuard {
        fn set(name: &'static str, value: Option<&str>) -> Self {
            let previous = std::env::var_os(name);
            if let Some(value) = value {
                std::env::set_var(name, value);
            } else {
                std::env::remove_var(name);
            }
            Self { name, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                std::env::set_var(self.name, previous);
            } else {
                std::env::remove_var(self.name);
            }
        }
    }

    fn with_env<T>(name: &'static str, value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let _lock = env_lock().lock().expect("env lock is not poisoned");
        let _guard = EnvGuard::set(name, value);
        f()
    }

    #[test]
    fn telemetry_enabled_defaults_to_on() {
        with_env("SOCAI_TELEMETRY", None, || {
            assert!(telemetry_enabled());
        });
    }

    #[test]
    fn telemetry_enabled_accepts_off_values() {
        for value in ["0", "false", "off", "disabled", "no", " OFF "] {
            with_env("SOCAI_TELEMETRY", Some(value), || {
                assert!(
                    !telemetry_enabled(),
                    "value {value:?} should disable telemetry"
                );
            });
        }
    }

    #[test]
    fn telemetry_enabled_ignores_unknown_values() {
        for value in ["1", "true", "on", "yes"] {
            with_env("SOCAI_TELEMETRY", Some(value), || {
                assert!(
                    telemetry_enabled(),
                    "value {value:?} should keep telemetry enabled"
                );
            });
        }
    }

    #[test]
    fn query_text_enabled_defaults_to_on() {
        with_env("SOCAI_TELEMETRY_QUERY_TEXT", None, || {
            assert!(query_text_enabled());
        });
    }

    #[test]
    fn query_text_enabled_accepts_off_values() {
        for value in ["0", "false", "off", "disabled", "no", " OFF "] {
            with_env("SOCAI_TELEMETRY_QUERY_TEXT", Some(value), || {
                assert!(
                    !query_text_enabled(),
                    "value {value:?} should disable query text"
                );
            });
        }
    }

    #[test]
    fn source_strings_are_stable() {
        assert_eq!(TelemetrySource::CliDaemon.as_str(), "cli_daemon");
        assert_eq!(TelemetrySource::Desktop.as_str(), "desktop");
        assert!(TelemetrySource::CliDaemon.collects_terminal_context());
        assert!(!TelemetrySource::Desktop.collects_terminal_context());
    }

    #[test]
    fn remote_event_drops_client_timestamp_before_proxy_send() {
        let event = remote_event(
            "socai_tool_call",
            "install-1",
            &json!({
                "created_at_ms": 123,
                "command": "search",
                "tool_name": "search"
            }),
        );
        let object = event.as_object().expect("remote event is an object");
        assert_eq!(object.get("event"), Some(&json!("socai_tool_call")));
        assert_eq!(object.get("install_id"), Some(&json!("install-1")));
        assert!(!object.contains_key("created_at_ms"));
    }
}
