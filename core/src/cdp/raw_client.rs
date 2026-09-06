use std::collections::HashMap;
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
}

struct CommandRequest {
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
        tokio::spawn(run_connection(ws, rx));
        Ok(Self { tx })
    }

    pub async fn execute(&self, method: impl Into<String>, params: Value) -> Result<Value> {
        self.execute_for_session(None, method, params).await
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
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(CommandRequest {
                method: method.clone(),
                params,
                session_id: session_id.map(ToOwned::to_owned),
                resp: resp_tx,
            })
            .await
            .map_err(|_| anyhow!("CDP session is closed"))?;

        let response = tokio::time::timeout(timeout, resp_rx)
            .await
            .map_err(|_| anyhow!("CDP command timed out: {method}"))?
            .map_err(|_| anyhow!("CDP session closed while waiting for: {method}"))?;
        response.map_err(|err| anyhow!("CDP command failed ({method}): {err}"))
    }
}

async fn run_connection<S>(
    ws: async_tungstenite::WebSocketStream<S>,
    mut rx: mpsc::Receiver<CommandRequest>,
) where
    S: futures::AsyncRead + futures::AsyncWrite + Unpin,
{
    let (mut write, mut read) = ws.split();
    let mut next_id: u64 = 1;
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
                let id = next_id;
                next_id = next_id.wrapping_add(1).max(1);
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
                        fail_all(&mut pending, "websocket stream ended without a close frame");
                        break;
                    }
                    Some(Ok(WsMessage::Ping(_))) | Some(Ok(WsMessage::Pong(_))) => {}
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
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
        tracing::debug!(target: "socai::cdp::raw", msg = text, "failed to parse CDP message");
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
