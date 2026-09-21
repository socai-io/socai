use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_tungstenite::tokio::connect_async_with_config;
use async_tungstenite::tungstenite::protocol::WebSocketConfig;
use async_tungstenite::tungstenite::Message as WsMessage;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

const COMMAND_CHANNEL_CAPACITY: usize = 64;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Minimal CDP websocket client.
///
/// This intentionally avoids `chromiumoxide::Handler`: we send only explicit
/// commands socai needs. Events are ignored, and no browser-wide target
/// discovery/auto-attach/domain enabling is performed. The runtime uses a
/// browser websocket for target inventory/lifecycle and routes page commands
/// with an explicit `sessionId` attached to one socai-owned target.
#[derive(Clone)]
pub struct RawCdpClient {
    tx: mpsc::Sender<CommandRequest>,
    unhealthy: Arc<AtomicBool>,
    unhealthy_sessions: Arc<Mutex<std::collections::HashSet<String>>>,
    diagnostic: Arc<ConnectionDiagnostic>,
}

struct ConnectionDiagnostic {
    id: String,
    next_id: AtomicU64,
    first_failure: Mutex<Option<Value>>,
}

impl ConnectionDiagnostic {
    fn event(&self, event: &str, data: Value, failure: bool) {
        let value = serde_json::json!({"connection_id":self.id,"event":event,"at":chrono::Utc::now().to_rfc3339(),"data":data});
        if failure {
            if let Ok(mut first) = self.first_failure.lock() {
                if first.is_none() {
                    *first = Some(value.clone());
                }
            }
        }
        super::diagnostics::record(event, value);
    }
}

struct CommandRequest {
    id: u64,
    method: String,
    params: Value,
    session_id: Option<String>,
    resp: oneshot::Sender<std::result::Result<Value, String>>,
}

#[derive(Debug, Deserialize)]
struct IncomingMessage {
    id: Option<u64>,
    result: Option<Value>,
    error: Option<CdpErrorPayload>,
    #[allow(dead_code)]
    method: Option<String>,
    #[allow(dead_code)]
    params: Option<Value>,
    #[serde(rename = "sessionId")]
    #[allow(dead_code)]
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CdpErrorPayload {
    code: i64,
    message: String,
}

impl RawCdpClient {
    pub async fn connect(ws_url: &str) -> Result<Self> {
        let config = WebSocketConfig {
            max_message_size: None,
            max_frame_size: None,
            ..Default::default()
        };
        let (ws, _) = connect_async_with_config(ws_url, Some(config))
            .await
            .with_context(|| format!("failed to connect CDP websocket: {ws_url}"))?;
        let (tx, rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
        let diagnostic = Arc::new(ConnectionDiagnostic {
            id: uuid::Uuid::new_v4().to_string(),
            next_id: AtomicU64::new(1),
            first_failure: Mutex::new(None),
        });
        diagnostic.event("connected", serde_json::json!({}), false);
        tokio::spawn(run_connection(ws, rx, diagnostic.clone()));
        Ok(Self {
            tx,
            unhealthy: Arc::new(AtomicBool::new(false)),
            unhealthy_sessions: Arc::new(Mutex::new(std::collections::HashSet::new())),
            diagnostic,
        })
    }

    pub async fn execute(&self, method: impl Into<String>, params: Value) -> Result<Value> {
        self.execute_for_session(None, method, params).await
    }

    /// Whether the websocket command loop has terminated. This is a typed
    /// transport-health signal for recovery code; callers do not need to
    /// recognize the user-facing error strings returned by `execute`.
    pub(crate) fn is_closed(&self) -> bool {
        self.tx.is_closed() || self.unhealthy.load(Ordering::Acquire)
    }

    pub(crate) fn is_session_closed(&self, session: Option<&str>) -> bool {
        self.is_closed()
            || session.is_some_and(|id| {
                self.unhealthy_sessions
                    .lock()
                    .expect("poisoned")
                    .contains(id)
            })
    }

    pub(crate) fn forget_session(&self, session: Option<&str>) {
        if let Some(id) = session {
            self.unhealthy_sessions.lock().expect("poisoned").remove(id);
        }
    }

    pub(crate) fn health_diagnostic(&self) -> Value {
        serde_json::json!({"connection_id":self.diagnostic.id,"socket_closed":self.tx.is_closed(),
            "marked_unhealthy":self.unhealthy.load(Ordering::Acquire),
            "unhealthy_session_count":self.unhealthy_sessions.lock().map(|s| s.len()).unwrap_or_default(),
            "first_failure":self.diagnostic.first_failure.lock().ok().and_then(|v| v.clone())})
    }

    pub async fn execute_for_session(
        &self,
        session_id: Option<&str>,
        method: impl Into<String>,
        params: Value,
    ) -> Result<Value> {
        self.execute_for_session_with_timeout(session_id, method, params, COMMAND_TIMEOUT)
            .await
    }

    pub(crate) async fn execute_for_session_with_timeout(
        &self,
        session_id: Option<&str>,
        method: impl Into<String>,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        let method = method.into();
        let id = self.diagnostic.next_id.fetch_add(1, Ordering::Relaxed);
        let started = std::time::Instant::now();
        if self.is_session_closed(session_id) {
            anyhow::bail!("CDP transport is unhealthy after a command timeout: {method}");
        }
        let (resp_tx, resp_rx) = oneshot::channel();
        let response = match tokio::time::timeout(timeout, async {
            self.tx
                .send(CommandRequest {
                    id,
                    method: method.clone(),
                    params,
                    session_id: session_id.map(ToOwned::to_owned),
                    resp: resp_tx,
                })
                .await
                .map_err(|_| anyhow!("CDP session is closed"))?;
            resp_rx
                .await
                .map_err(|_| anyhow!("CDP session closed while waiting for: {method}"))
        })
        .await
        {
            Ok(response) => response?,
            Err(_) => {
                self.diagnostic.event("command_timeout", serde_json::json!({"command_id":id,"method":method,"elapsed_ms":started.elapsed().as_millis(),"session_id":session_id,"scope":if session_id.is_some(){"page"}else{"browser"}}), true);
                if let Some(session) = session_id {
                    self.unhealthy_sessions
                        .lock()
                        .expect("poisoned")
                        .insert(session.into());
                } else {
                    self.unhealthy.store(true, Ordering::Release);
                }
                return Err(anyhow!("CDP command timed out: {method}"));
            }
        };
        if response.is_err() || started.elapsed().as_secs() >= 5 {
            self.diagnostic.event(if response.is_err() { "command_error" } else { "slow_command" }, serde_json::json!({"command_id":id,"method":method,"elapsed_ms":started.elapsed().as_millis(),"session_id":session_id}), false);
        }
        response.map_err(|err| anyhow!("CDP command failed ({method}): {err}"))
    }
}

async fn run_connection<S>(
    ws: async_tungstenite::WebSocketStream<S>,
    mut rx: mpsc::Receiver<CommandRequest>,
    diagnostic: Arc<ConnectionDiagnostic>,
) where
    S: futures::AsyncRead + futures::AsyncWrite + Unpin,
{
    let (mut write, mut read) = ws.split();
    let mut pending: HashMap<u64, oneshot::Sender<std::result::Result<Value, String>>> =
        HashMap::new();

    // Periodically drop entries whose caller already gave up — `execute_*` times
    // out after COMMAND_TIMEOUT and drops its receiver, which marks the sender
    // closed. Without this, a target that stops answering (but keeps the socket
    // open) would leak one `pending` entry per timed-out command for the life of
    // the connection.
    let mut prune = tokio::time::interval(COMMAND_TIMEOUT);
    prune.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = prune.tick() => {
                pending.retain(|_, resp| !resp.is_closed());
            }
            command = rx.recv() => {
                let Some(command) = command else {
                    fail_all(&mut pending, "command channel closed");
                    break;
                };
                let id = command.id;
                let mut payload = serde_json::json!({
                    "id": id,
                    "method": command.method,
                    "params": command.params,
                });
                if let Some(session_id) = command.session_id {
                    payload["sessionId"] = Value::String(session_id);
                }
                let text = match serde_json::to_string(&payload) {
                    Ok(text) => text,
                    Err(err) => {
                        let _ = command.resp.send(Err(format!("failed to serialize CDP command: {err}")));
                        continue;
                    }
                };
                pending.insert(id, command.resp);
                if let Err(err) = write.send(WsMessage::Text(text)).await {
                    diagnostic.event("websocket_send_failed", serde_json::json!({"command_id":id,"pending":pending.len()}), true);
                    fail_one(&mut pending, id, format!("websocket send failed: {err}"));
                    fail_all(&mut pending, "websocket send failed");
                    break;
                }
            }
            message = read.next() => {
                match message {
                    Some(Ok(WsMessage::Text(text))) => handle_text_message(&mut pending, &text),
                    Some(Ok(WsMessage::Binary(bytes))) => {
                        if let Ok(text) = std::str::from_utf8(&bytes) {
                            handle_text_message(&mut pending, text);
                        }
                    }
                    Some(Ok(WsMessage::Close(frame))) => {
                        diagnostic.event("websocket_close", serde_json::json!({"close_code":frame.as_ref().map(|f| u16::from(f.code)),"pending":pending.len()}), true);
                        let detail = frame.map_or_else(
                            || "websocket closed without a close frame".to_string(),
                            |frame| {
                                format!(
                                    "websocket closed: code={:?}, reason={}",
                                    frame.code, frame.reason
                                )
                            },
                        );
                        fail_all(&mut pending, &detail);
                        break;
                    }
                    None => {
                        diagnostic.event("websocket_eof", serde_json::json!({"pending":pending.len()}), true);
                        fail_all(&mut pending, "websocket stream ended without a close frame");
                        break;
                    }
                    Some(Ok(WsMessage::Ping(_))) | Some(Ok(WsMessage::Pong(_))) => {}
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
                        diagnostic.event("websocket_receive_failed", serde_json::json!({"pending":pending.len(),"kind":match &err { async_tungstenite::tungstenite::Error::Io(e) => format!("io:{:?}",e.kind()), _ => "protocol_or_transport".into() }}), true);
                        fail_all(&mut pending, &format!("websocket receive failed: {err}"));
                        break;
                    }
                }
            }
        }
    }
}

fn handle_text_message(
    pending: &mut HashMap<u64, oneshot::Sender<std::result::Result<Value, String>>>,
    text: &str,
) {
    let Ok(message) = serde_json::from_str::<IncomingMessage>(text) else {
        tracing::debug!(target: "socai::cdp::raw", bytes = text.len(), "failed to parse CDP message");
        return;
    };
    let Some(id) = message.id else {
        // Target-scoped events are intentionally ignored. We do not enable noisy
        // domains, but Chrome may still emit a few lifecycle/runtime messages in
        // response to commands.
        return;
    };
    let Some(resp) = pending.remove(&id) else {
        return;
    };
    let result = if let Some(err) = message.error {
        Err(format!("{} ({})", err.message, err.code))
    } else {
        Ok(message.result.unwrap_or_else(|| serde_json::json!({})))
    };
    let _ = resp.send(result);
}

fn fail_one(
    pending: &mut HashMap<u64, oneshot::Sender<std::result::Result<Value, String>>>,
    id: u64,
    reason: String,
) {
    if let Some(resp) = pending.remove(&id) {
        let _ = resp.send(Err(reason));
    }
}

fn fail_all(
    pending: &mut HashMap<u64, oneshot::Sender<std::result::Result<Value, String>>>,
    reason: &str,
) {
    for (_, resp) in pending.drain() {
        let _ = resp.send(Err(reason.to_string()));
    }
}
