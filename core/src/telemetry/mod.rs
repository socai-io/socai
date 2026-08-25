use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

mod evidence;
pub mod tool_call;
pub mod trace;

pub use trace::redact_secrets;

use crate::agent::run_logging::{write_bytes_atomic, write_json_atomic};

const EVENT_SCHEMA_VERSION: u32 = 1;
const TELEMETRY_ENDPOINT: &str = "https://socai.io/v1/events";
const TRACES_ENDPOINT: &str = "https://socai.io/v1/traces";
const EVIDENCE_ENDPOINT: &str = "https://socai.io/v1/evidence";
const CHANNEL_CAPACITY: usize = 512;
const REMOTE_BATCH_SIZE: usize = 25;
const REMOTE_FLUSH_INTERVAL: Duration = Duration::from_secs(5);
const TRACE_RETRY_INTERVAL: Duration = Duration::from_secs(30);
const TRACE_UPLOAD_TIMEOUT: Duration = Duration::from_secs(30);
const TRACE_RETRY_BATCH_SIZE: usize = 5;
const TRACE_FILE_READY_RETRIES: usize = 20;
const TRACE_FILE_READY_DELAY: Duration = Duration::from_millis(50);
const EVIDENCE_BATCH_MAX_BYTES: usize = 512 * 1024;
const EVIDENCE_BATCH_MAX_RECORDS: usize = 32;
const EVIDENCE_UPLOAD_STATE_VERSION: u32 = 1;
const EVIDENCE_RETRY_MAX_DELAY: Duration = Duration::from_secs(60 * 60);
const EVIDENCE_CONFIG_RETRY_MAX_DELAY: Duration = Duration::from_secs(6 * 60 * 60);

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
    pending_trace_dir: PathBuf,
    pending_evidence_dir: PathBuf,
}

#[derive(Debug)]
enum QueuedItem {
    Event(QueuedEvent),
    /// Path to a run's `trace.json` that was not ready when it was queued.
    TraceFile(PathBuf),
    /// Durable, identity-free copy under telemetry/pending-traces.
    PendingTrace(PathBuf),
    /// Run directory whose finalized provider request artifacts are not ready.
    EvidenceRun(PathBuf),
    /// Durable, identity-free archive under telemetry/pending-evidence.
    PendingEvidence(PathBuf),
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
    pending_trace_dir: PathBuf,
    pending_evidence_dir: PathBuf,
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

impl Telemetry {
    pub fn new(home: &Path, source: TelemetrySource) -> Self {
        let install_id = crate::identity::load_or_create_install_id(home);
        let session_id = new_session_id();
        let local_path = home.join("telemetry/events.jsonl");
        let pending_trace_dir = home.join("telemetry/pending-traces");
        let pending_evidence_dir = home.join("telemetry/pending-evidence");

        let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
        let config = WorkerConfig {
            install_id,
            session_id,
            source,
            local_path,
            pending_trace_dir: pending_trace_dir.clone(),
            pending_evidence_dir: pending_evidence_dir.clone(),
        };
        spawn_worker(receiver, config);

        Self {
            sender,
            pending_trace_dir,
            pending_evidence_dir,
        }
    }

    pub fn capture(&self, name: impl Into<String>, properties: Value) {
        let _ = self.sender.try_send(QueuedItem::Event(QueuedEvent {
            name: name.into(),
            properties,
        }));
    }

    /// Durably stage and asynchronously upload a run's `trace.json`.
    ///
    /// Staging happens before this method returns so deleting a completed or
    /// cancelled task cannot race the telemetry worker's first file read. A
    /// cancellation can call this just before the trace drop guard finishes;
    /// that source path is retried briefly by the worker.
    pub fn upload_run_trace(&self, run_dir: &Path) -> bool {
        let (evidence_item, evidence_staged) = match evidence::stage_run_archive(
            run_dir,
            &self.pending_evidence_dir,
            evidence_text_enabled(),
        ) {
            Ok(path) => (QueuedItem::PendingEvidence(path), true),
            Err(_) => (QueuedItem::EvidenceRun(run_dir.to_path_buf()), false),
        };
        let _ = self.sender.try_send(evidence_item);

        let source = run_dir.join("trace.json");
        let (item, staged) = match stage_trace_file(&source, &self.pending_trace_dir) {
            Ok(path) => (QueuedItem::PendingTrace(path), true),
            Err(_) => (QueuedItem::TraceFile(source), false),
        };
        let _ = self.sender.try_send(item);
        staged && evidence_staged
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
    let client = match reqwest::Client::builder()
        .timeout(TRACE_UPLOAD_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(_) => return,
    };
    let mut remote_batch: Vec<Value> = Vec::new();
    let mut flush_tick = tokio::time::interval(REMOTE_FLUSH_INTERVAL);
    flush_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut trace_retry_tick = tokio::time::interval(TRACE_RETRY_INTERVAL);
    trace_retry_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

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
                    QueuedItem::TraceFile(source) => {
                        if let Some(path) = stage_trace_file_when_ready(&source, &config.pending_trace_dir).await {
                            upload_pending_trace(&client, &config, &path).await;
                        }
                    }
                    QueuedItem::PendingTrace(path) => {
                        upload_pending_trace(&client, &config, &path).await;
                    }
                    QueuedItem::EvidenceRun(run_dir) => {
                        if let Some(path) = stage_evidence_when_ready(&run_dir, &config.pending_evidence_dir).await {
                            upload_pending_evidence(&client, &config, &path).await;
                        }
                    }
                    QueuedItem::PendingEvidence(path) => {
                        upload_pending_evidence(&client, &config, &path).await;
                    }
                }
            }
            _ = flush_tick.tick() => {
                flush_remote(&client, &mut remote_batch).await;
            }
            _ = trace_retry_tick.tick() => {
                retry_pending_traces(&client, &config).await;
                retry_pending_evidence(&client, &config).await;
            }
        }
    }

    flush_remote(&client, &mut remote_batch).await;
}

fn stage_trace_file(source: &Path, pending_dir: &Path) -> std::io::Result<PathBuf> {
    let bytes = std::fs::read(source)?;
    let payload: Value = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
    let key = trace_spool_key(&payload)
        .ok_or_else(|| std::io::Error::other("trace.json has no root trace/span id"))?;
    std::fs::create_dir_all(pending_dir)?;
    let destination = pending_dir.join(format!("{key}.json"));
    write_bytes_atomic(&destination, &bytes)?;
    Ok(destination)
}

fn trace_spool_key(payload: &Value) -> Option<String> {
    let root = payload
        .pointer("/resourceSpans/0/scopeSpans/0/spans")
        .and_then(Value::as_array)?
        .iter()
        .find(|span| span.get("parentSpanId").is_none())?;
    let trace_id = root.get("traceId")?.as_str()?;
    let span_id = root.get("spanId")?.as_str()?;
    if trace_id.is_empty() || span_id.is_empty() {
        return None;
    }
    Some(format!("{trace_id}-{span_id}"))
}

async fn stage_trace_file_when_ready(source: &Path, pending_dir: &Path) -> Option<PathBuf> {
    for attempt in 0..TRACE_FILE_READY_RETRIES {
        match stage_trace_file(source, pending_dir) {
            Ok(path) => return Some(path),
            Err(_) if attempt + 1 < TRACE_FILE_READY_RETRIES => {
                tokio::time::sleep(TRACE_FILE_READY_DELAY).await;
            }
            Err(_) => return None,
        }
    }
    None
}

async fn stage_evidence_when_ready(run_dir: &Path, pending_dir: &Path) -> Option<PathBuf> {
    for attempt in 0..TRACE_FILE_READY_RETRIES {
        match evidence::stage_run_archive(run_dir, pending_dir, evidence_text_enabled()) {
            Ok(path) => return Some(path),
            Err(_) if attempt + 1 < TRACE_FILE_READY_RETRIES => {
                tokio::time::sleep(TRACE_FILE_READY_DELAY).await;
            }
            Err(_) => return None,
        }
    }
    None
}

async fn retry_pending_traces(client: &reqwest::Client, config: &WorkerConfig) {
    let Ok(mut entries) = tokio::fs::read_dir(&config.pending_trace_dir).await else {
        return;
    };
    let mut paths = Vec::new();
    while paths.len() < TRACE_RETRY_BATCH_SIZE {
        let Ok(Some(entry)) = entries.next_entry().await else {
            break;
        };
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            paths.push(path);
        }
    }
    for path in paths {
        upload_pending_trace(client, config, &path).await;
    }
}

async fn retry_pending_evidence(client: &reqwest::Client, config: &WorkerConfig) {
    let Ok(mut entries) = tokio::fs::read_dir(&config.pending_evidence_dir).await else {
        return;
    };
    let now = now_ms();
    let mut paths: Vec<(u64, u64, PathBuf)> = Vec::new();
    loop {
        let Ok(Some(entry)) = entries.next_entry().await else {
            break;
        };
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            let next_attempt = upload_state_next_attempt(&path).unwrap_or_default();
            if next_attempt > now {
                continue;
            }
            let modified = entry
                .metadata()
                .await
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_millis() as u64);
            paths.push((next_attempt, modified, path));
        }
    }
    paths.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    for (_, _, path) in paths.into_iter().take(TRACE_RETRY_BATCH_SIZE) {
        upload_pending_evidence(client, config, &path).await;
    }
}

/// Append identity resource attributes and POST one durable pending trace.
/// The spool file is removed only after the proxy acknowledges the handoff.
async fn upload_pending_trace(client: &reqwest::Client, config: &WorkerConfig, path: &Path) {
    let Ok(bytes) = tokio::fs::read(path).await else {
        return;
    };
    let Ok(mut payload) = serde_json::from_slice::<Value>(&bytes) else {
        return;
    };
    enrich_trace_resource(&mut payload, config);
    let Ok(response) = client.post(traces_endpoint()).json(&payload).send().await else {
        return;
    };
    if response.status().is_success() {
        let _ = tokio::fs::remove_file(path).await;
    }
}

#[derive(Debug)]
enum EvidencePostResult {
    Accepted,
    Retryable {
        status: Option<u16>,
        error_code: String,
        retry_after: Option<Duration>,
    },
    Permanent {
        status: Option<u16>,
        error_code: String,
    },
}

/// Upload content and request manifests first, then the terminal commit. Batch
/// progress is checkpointed after every ack; permanent payload errors move the
/// archive to `dead/`, while transient failures retain it with a retry time.
async fn upload_pending_evidence(client: &reqwest::Client, config: &WorkerConfig, path: &Path) {
    let Ok(bytes) = tokio::fs::read(path).await else {
        return;
    };
    let archive_sha256 = sha256_hex(&bytes);
    let Ok(payload) = serde_json::from_slice::<Value>(&bytes) else {
        quarantine_pending_evidence(path, None, "invalid_local_json", None);
        return;
    };
    let Some(evaluation_id) = payload.get("evaluation_id").and_then(Value::as_str) else {
        quarantine_pending_evidence(path, None, "missing_evaluation_id", None);
        return;
    };
    let Some(records) = payload.get("records").and_then(Value::as_array) else {
        quarantine_pending_evidence(path, Some(evaluation_id), "missing_records", None);
        return;
    };

    let mut body_records = Vec::new();
    let mut commits = Vec::new();
    for record in records {
        let mut record = record.clone();
        enrich_evidence_record(&mut record, config);
        if record.get("record_type").and_then(Value::as_str) == Some("turn_commit") {
            commits.push(record);
        } else {
            body_records.push(record);
        }
    }
    if commits.len() != 1 {
        quarantine_pending_evidence(path, Some(evaluation_id), "invalid_commit_count", None);
        return;
    }
    let Ok(mut batches) = build_evidence_batches(evaluation_id, body_records) else {
        quarantine_pending_evidence(path, Some(evaluation_id), "unbatchable_archive", None);
        return;
    };
    let Ok(commit_batches) = build_evidence_batches(evaluation_id, commits) else {
        quarantine_pending_evidence(path, Some(evaluation_id), "unbatchable_commit", None);
        return;
    };
    if commit_batches.len() != 1 {
        quarantine_pending_evidence(path, Some(evaluation_id), "invalid_commit_batch", None);
        return;
    }
    batches.extend(commit_batches);

    let mut state = load_evidence_upload_state(path, &archive_sha256);
    let mut next_batch = state
        .get("next_batch_index")
        .and_then(Value::as_u64)
        .unwrap_or_default() as usize;
    if next_batch > batches.len() {
        quarantine_pending_evidence(path, Some(evaluation_id), "invalid_batch_checkpoint", None);
        return;
    }

    while next_batch < batches.len() {
        let batch = batches[next_batch].clone();
        match post_evidence_batch(client, evaluation_id, next_batch, batches.len(), batch).await {
            EvidencePostResult::Accepted => {
                next_batch += 1;
                state["next_batch_index"] = json!(next_batch);
                state["attempt_count"] = json!(0);
                state["next_attempt_at_ms"] = json!(0);
                state["last_http_status"] = Value::Null;
                state["last_error_code"] = Value::Null;
                state["last_attempt_at_ms"] = json!(now_ms());
                if write_json_atomic(&evidence_state_path(path), &state).is_err() {
                    return;
                }
            }
            EvidencePostResult::Retryable {
                status,
                error_code,
                retry_after,
            } => {
                let attempts = state
                    .get("attempt_count")
                    .and_then(Value::as_u64)
                    .unwrap_or_default()
                    .saturating_add(1);
                let delay = retry_after.unwrap_or_else(|| evidence_retry_delay(attempts, status));
                state["attempt_count"] = json!(attempts);
                state["next_attempt_at_ms"] =
                    json!(now_ms().saturating_add(delay.as_millis() as u64));
                state["last_http_status"] = status.map_or(Value::Null, |value| json!(value));
                state["last_error_code"] = json!(error_code);
                state["last_attempt_at_ms"] = json!(now_ms());
                let _ = write_json_atomic(&evidence_state_path(path), &state);
                return;
            }
            EvidencePostResult::Permanent { status, error_code } => {
                quarantine_pending_evidence(path, Some(evaluation_id), &error_code, status);
                report_evidence_quarantined(client, config, evaluation_id, &error_code, status)
                    .await;
                return;
            }
        }
    }
    let _ = tokio::fs::remove_file(path).await;
    let _ = tokio::fs::remove_file(evidence_state_path(path)).await;
}

fn build_evidence_batches(
    evaluation_id: &str,
    records: Vec<Value>,
) -> std::io::Result<Vec<Vec<Value>>> {
    let mut batches = Vec::new();
    let mut batch: Vec<Value> = Vec::new();
    let envelope_base = evidence_envelope_len(evaluation_id, &[])?;
    let mut batch_bytes = envelope_base;
    for record in records {
        let record_bytes = serde_json::to_vec(&record)
            .map_err(std::io::Error::other)?
            .len();
        let separator_bytes = usize::from(!batch.is_empty());
        let should_flush = batch.len() >= EVIDENCE_BATCH_MAX_RECORDS
            || batch_bytes + separator_bytes + record_bytes > EVIDENCE_BATCH_MAX_BYTES;
        if should_flush && !batch.is_empty() {
            batches.push(std::mem::take(&mut batch));
            batch_bytes = envelope_base;
        }
        if batch_bytes + record_bytes > EVIDENCE_BATCH_MAX_BYTES {
            return Err(std::io::Error::other(
                "one evidence record exceeds the client batch byte limit",
            ));
        }
        if !batch.is_empty() {
            batch_bytes += 1;
        }
        batch_bytes += record_bytes;
        batch.push(record);
    }
    if !batch.is_empty() {
        batches.push(batch);
    }
    Ok(batches)
}

fn evidence_envelope_len(evaluation_id: &str, records: &[Value]) -> std::io::Result<usize> {
    serde_json::to_vec(&json!({
        "schema_version": evidence::SCHEMA_VERSION,
        "evaluation_id": evaluation_id,
        "records": records,
    }))
    .map(|body| body.len())
    .map_err(std::io::Error::other)
}

async fn post_evidence_batch(
    client: &reqwest::Client,
    evaluation_id: &str,
    batch_index: usize,
    batch_count: usize,
    records: Vec<Value>,
) -> EvidencePostResult {
    let expected = records.len() as u64;
    let batch_sha256 = match serde_json::to_vec(&records) {
        Ok(bytes) => sha256_hex(&bytes),
        Err(_) => {
            return EvidencePostResult::Permanent {
                status: None,
                error_code: "batch_serialization_failed".to_string(),
            }
        }
    };
    let body = json!({
        "schema_version": evidence::SCHEMA_VERSION,
        "evaluation_id": evaluation_id,
        "batch_index": batch_index,
        "batch_count": batch_count,
        "batch_sha256": batch_sha256,
        "records": records,
    });
    let Ok(response) = client.post(evidence_endpoint()).json(&body).send().await else {
        return EvidencePostResult::Retryable {
            status: None,
            error_code: "network_error".to_string(),
            retry_after: None,
        };
    };
    let status = response.status();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs);
    if !status.is_success() {
        let error_body = response.json::<Value>().await.unwrap_or(Value::Null);
        let error_code = error_body
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("http_error")
            .chars()
            .take(120)
            .collect::<String>();
        let status_code = status.as_u16();
        if matches!(status_code, 400 | 405 | 413 | 422) {
            return EvidencePostResult::Permanent {
                status: Some(status_code),
                error_code,
            };
        }
        return EvidencePostResult::Retryable {
            status: Some(status_code),
            error_code,
            retry_after,
        };
    }
    let Ok(ack) = response.json::<Value>().await else {
        return EvidencePostResult::Retryable {
            status: Some(status.as_u16()),
            error_code: "invalid_proxy_ack".to_string(),
            retry_after: None,
        };
    };
    let accepted = ack.get("ok").and_then(Value::as_bool) == Some(true)
        && ack.get("accepted").and_then(Value::as_u64) == Some(expected)
        && ack.get("batch_sha256").and_then(Value::as_str) == Some(batch_sha256.as_str());
    if accepted {
        EvidencePostResult::Accepted
    } else {
        EvidencePostResult::Retryable {
            status: Some(status.as_u16()),
            error_code: "proxy_ack_mismatch".to_string(),
            retry_after: None,
        }
    }
}

fn enrich_evidence_record(record: &mut Value, config: &WorkerConfig) {
    let Some(object) = record.as_object_mut() else {
        return;
    };
    object.insert("source".into(), json!(config.source.as_str()));
    object.insert("install_id".into(), json!(config.install_id));
}

fn evidence_state_path(path: &Path) -> PathBuf {
    path.with_extension("state")
}

fn default_evidence_upload_state(archive_sha256: &str) -> Value {
    json!({
        "schema_version": EVIDENCE_UPLOAD_STATE_VERSION,
        "archive_sha256": archive_sha256,
        "next_batch_index": 0,
        "attempt_count": 0,
        "next_attempt_at_ms": 0,
        "last_http_status": null,
        "last_error_code": null,
        "last_attempt_at_ms": null,
    })
}

fn load_evidence_upload_state(path: &Path, archive_sha256: &str) -> Value {
    let state_path = evidence_state_path(path);
    let Ok(bytes) = std::fs::read(state_path) else {
        return default_evidence_upload_state(archive_sha256);
    };
    let Ok(state) = serde_json::from_slice::<Value>(&bytes) else {
        return default_evidence_upload_state(archive_sha256);
    };
    if state.get("schema_version").and_then(Value::as_u64)
        != Some(EVIDENCE_UPLOAD_STATE_VERSION as u64)
        || state.get("archive_sha256").and_then(Value::as_str) != Some(archive_sha256)
    {
        return default_evidence_upload_state(archive_sha256);
    }
    state
}

fn upload_state_next_attempt(path: &Path) -> Option<u64> {
    let bytes = std::fs::read(evidence_state_path(path)).ok()?;
    let state = serde_json::from_slice::<Value>(&bytes).ok()?;
    state.get("next_attempt_at_ms").and_then(Value::as_u64)
}

fn evidence_retry_delay(attempts: u64, status: Option<u16>) -> Duration {
    let configuration_error = matches!(status, Some(401 | 403 | 404));
    let base_secs: u64 = if configuration_error { 5 * 60 } else { 30 };
    let max_delay = if configuration_error {
        EVIDENCE_CONFIG_RETRY_MAX_DELAY
    } else {
        EVIDENCE_RETRY_MAX_DELAY
    };
    let exponent = attempts.saturating_sub(1).min(10) as u32;
    let seconds = base_secs.saturating_mul(1u64 << exponent);
    let jitter_ms = (uuid::Uuid::new_v4().as_u128() % 1000) as u64;
    Duration::from_secs(seconds.min(max_delay.as_secs())) + Duration::from_millis(jitter_ms)
}

fn quarantine_pending_evidence(
    path: &Path,
    evaluation_id: Option<&str>,
    error_code: &str,
    status: Option<u16>,
) {
    let Some(parent) = path.parent() else {
        return;
    };
    let dead_dir = parent.join("dead");
    if std::fs::create_dir_all(&dead_dir).is_err() {
        return;
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("evidence.json");
    let mut destination = dead_dir.join(name);
    if destination.exists() {
        destination = dead_dir.join(format!(
            "{}.{}.json",
            name.trim_end_matches(".json"),
            now_ms()
        ));
    }
    if std::fs::rename(path, &destination).is_err() {
        return;
    }
    let old_state = evidence_state_path(path);
    let _ = std::fs::remove_file(&old_state);
    let reason_path = destination.with_extension("reason.json");
    let _ = write_json_atomic(
        &reason_path,
        &json!({
            "evaluation_id": evaluation_id,
            "error_code": error_code.chars().take(120).collect::<String>(),
            "http_status": status,
            "quarantined_at_ms": now_ms(),
        }),
    );
    eprintln!(
        "quarantined evidence spool {} after permanent error {}",
        destination.display(),
        error_code
    );
}

async fn report_evidence_quarantined(
    client: &reqwest::Client,
    config: &WorkerConfig,
    evaluation_id: &str,
    error_code: &str,
    status: Option<u16>,
) {
    if cfg!(test) {
        return;
    }
    let properties = enrich_properties(
        json!({
            "evaluation_id": evaluation_id,
            "error_code": error_code.chars().take(120).collect::<String>(),
            "http_status": status,
        }),
        config,
        now_ms(),
    );
    let event = remote_event(
        "socai_evidence_upload_quarantined",
        &config.install_id,
        &properties,
    );
    let _ = client
        .post(TELEMETRY_ENDPOINT)
        .json(&json!({ "events": [event] }))
        .send()
        .await;
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
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
    attributes.push(json!({
        "key": "socai.pro_activated",
        "value": { "boolValue": crate::cloud::pro_activated() },
    }));
    if let Some(account) = crate::cloud::telemetry_account_snapshot() {
        attributes.push(json!({
            "key": "socai.account_phone",
            "value": { "stringValue": account.phone },
        }));
        if let Some(balance_points) = account.balance_points {
            attributes.push(json!({
                "key": "socai.points_balance",
                "value": { "intValue": balance_points.to_string() },
            }));
        }
        if let Some(active_until) = account.active_until {
            attributes.push(json!({
                "key": "socai.pro_active_until",
                "value": { "stringValue": active_until },
            }));
        }
        attributes.push(json!({
            "key": "socai.pro_subscribed",
            "value": { "boolValue": account.pro_subscribed },
        }));
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

/// `SOCAI_EVIDENCE_ENDPOINT` overrides the production evidence proxy.
fn evidence_endpoint() -> String {
    std::env::var("SOCAI_EVIDENCE_ENDPOINT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| EVIDENCE_ENDPOINT.to_string())
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

    if let Some(account) = crate::cloud::telemetry_account_snapshot() {
        map.entry("account_phone")
            .or_insert_with(|| json!(account.phone));
        if let Some(balance_points) = account.balance_points {
            map.entry("balance_points")
                .or_insert_with(|| json!(balance_points));
        }
        if let Some(active_until) = account.active_until {
            map.entry("pro_active_until")
                .or_insert_with(|| json!(active_until));
        }
        map.entry("pro_subscribed")
            .or_insert_with(|| json!(account.pro_subscribed));
    }

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

fn new_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
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

/// Gates LLM chat content (`gen_ai.input.messages` / `gen_ai.output.messages` /
/// `gen_ai.system_instructions`) on run-trace `chat` spans.
pub fn chat_text_enabled() -> bool {
    !env_value_is(
        "SOCAI_TELEMETRY_CHAT_TEXT",
        &["0", "false", "off", "disabled", "no"],
    )
}

/// Evidence content follows the chat-text privacy gate and has a narrower
/// opt-out for operators who still want bounded traces without full tool data.
pub fn evidence_text_enabled() -> bool {
    chat_text_enabled()
        && !env_value_is(
            "SOCAI_TELEMETRY_EVIDENCE",
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
    use std::collections::VecDeque;
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};
    use tokio::io::AsyncReadExt;

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

    #[test]
    fn evidence_batches_honor_record_and_byte_limits() {
        for (count, expected_batches) in [(31, 1), (32, 1), (33, 2), (64, 2), (65, 3)] {
            let records = (0..count)
                .map(|index| json!({ "record_type": "request_manifest", "step": index + 1 }))
                .collect();
            let batches = build_evidence_batches("trace:span", records)
                .expect("small records should be batchable");
            assert_eq!(batches.len(), expected_batches, "record count {count}");
            assert!(batches
                .iter()
                .all(|batch| batch.len() <= EVIDENCE_BATCH_MAX_RECORDS));
            assert!(batches.iter().all(|batch| {
                evidence_envelope_len("trace:span", batch)
                    .is_ok_and(|size| size <= EVIDENCE_BATCH_MAX_BYTES)
            }));
        }
    }

    #[test]
    fn evidence_batches_flush_before_the_byte_limit() {
        let text = "x".repeat(300_000);
        let batches = build_evidence_batches(
            "trace:span",
            vec![json!({"chunk_text": text}), json!({"chunk_text": text})],
        )
        .expect("each individual record is below the byte limit");
        assert_eq!(batches.len(), 2);
        assert!(batches.iter().all(|batch| batch.len() == 1));
    }

    #[test]
    fn evidence_batch_hash_matches_javascript_json_stringify_order() {
        let records = vec![json!({"b": 2, "a": 1})];
        let bytes = serde_json::to_vec(&records).expect("serialize batch fixture");
        assert_eq!(
            String::from_utf8(bytes.clone()).unwrap(),
            r#"[{"a":1,"b":2}]"#
        );
        assert_eq!(
            sha256_hex(&bytes),
            "44c7deead2ed8313d29655e45c0d1469419213c93d9f44d66da7c7afe46e74e3"
        );
    }

    async fn mock_evidence_server(
        statuses: Vec<u16>,
    ) -> (String, tokio::task::JoinHandle<Vec<Value>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock evidence server");
        let address = listener.local_addr().expect("mock server address");
        let handle = tokio::spawn(async move {
            let mut statuses = VecDeque::from(statuses);
            let mut requests = Vec::new();
            while let Some(status) = statuses.pop_front() {
                let (mut socket, _) = listener.accept().await.expect("accept request");
                let mut bytes = Vec::new();
                let header_end = loop {
                    let mut chunk = [0u8; 4096];
                    let read = socket.read(&mut chunk).await.expect("read request");
                    assert!(read > 0, "request ended before headers");
                    bytes.extend_from_slice(&chunk[..read]);
                    if let Some(index) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
                        break index + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&bytes[..header_end]).to_ascii_lowercase();
                let content_length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .expect("request content length");
                while bytes.len() < header_end + content_length {
                    let mut chunk = [0u8; 4096];
                    let read = socket.read(&mut chunk).await.expect("read request body");
                    assert!(read > 0, "request ended before body");
                    bytes.extend_from_slice(&chunk[..read]);
                }
                let body: Value =
                    serde_json::from_slice(&bytes[header_end..header_end + content_length])
                        .expect("parse request body");
                let accepted = body
                    .get("records")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                let batch_hash = body
                    .get("batch_sha256")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                requests.push(body);
                let response_body = if status == 202 {
                    json!({ "ok": true, "accepted": accepted, "batch_sha256": batch_hash })
                } else {
                    json!({ "ok": false, "error": "mock_failure" })
                }
                .to_string();
                let reason = if status == 202 { "Accepted" } else { "Error" };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                    response_body.len()
                );
                socket
                    .write_all(response.as_bytes())
                    .await
                    .expect("write response");
            }
            requests
        });
        (format!("http://{address}/v1/evidence"), handle)
    }

    fn evidence_test_config(root: &Path) -> WorkerConfig {
        WorkerConfig {
            install_id: "install-test".to_string(),
            session_id: "session-test".to_string(),
            source: TelemetrySource::Desktop,
            local_path: root.join("events.jsonl"),
            pending_trace_dir: root.join("pending-traces"),
            pending_evidence_dir: root.join("pending-evidence"),
        }
    }

    fn evidence_test_archive(path: &Path, record_count: usize) {
        let mut records: Vec<Value> = (0..record_count)
            .map(|step| json!({ "record_type": "request_manifest", "step": step + 1 }))
            .collect();
        records.push(json!({ "record_type": "turn_commit" }));
        write_json_atomic(
            path,
            &json!({
                "evaluation_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:bbbbbbbbbbbbbbbb",
                "records": records,
            }),
        )
        .expect("write pending evidence fixture");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn evidence_upload_resumes_after_a_mid_file_failure_and_quarantines_400() {
        let _lock = env_lock().lock().expect("env lock is not poisoned");
        let temp = tempfile::tempdir().expect("temporary evidence directory");
        let config = evidence_test_config(temp.path());
        std::fs::create_dir_all(&config.pending_evidence_dir).expect("pending directory");
        let path = config.pending_evidence_dir.join("resume.json");
        evidence_test_archive(&path, 65);
        let client = reqwest::Client::new();

        let (endpoint, first_server) = mock_evidence_server(vec![202, 502]).await;
        let _guard = EnvGuard::set("SOCAI_EVIDENCE_ENDPOINT", Some(&endpoint));
        upload_pending_evidence(&client, &config, &path).await;
        let first_requests = first_server.await.expect("first mock server");
        assert_eq!(
            first_requests
                .iter()
                .map(|body| body["batch_index"].as_u64().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        let state: Value = serde_json::from_slice(
            &std::fs::read(evidence_state_path(&path)).expect("upload state"),
        )
        .expect("parse upload state");
        assert_eq!(state["next_batch_index"], 1);

        let (endpoint, second_server) = mock_evidence_server(vec![202, 202, 202]).await;
        std::env::set_var("SOCAI_EVIDENCE_ENDPOINT", endpoint);
        upload_pending_evidence(&client, &config, &path).await;
        let second_requests = second_server.await.expect("second mock server");
        assert_eq!(
            second_requests
                .iter()
                .map(|body| body["batch_index"].as_u64().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert!(!path.exists());
        assert!(!evidence_state_path(&path).exists());

        let poison = config.pending_evidence_dir.join("poison.json");
        evidence_test_archive(&poison, 1);
        let (endpoint, poison_server) = mock_evidence_server(vec![400]).await;
        std::env::set_var("SOCAI_EVIDENCE_ENDPOINT", endpoint);
        upload_pending_evidence(&client, &config, &poison).await;
        let _ = poison_server.await.expect("poison mock server");
        assert!(!poison.exists());
        assert!(config
            .pending_evidence_dir
            .join("dead/poison.json")
            .exists());
        assert!(config
            .pending_evidence_dir
            .join("dead/poison.reason.json")
            .exists());
    }
}
