//! CDP bridge — a tiny relay process that owns the consented WebSocket to
//! Chrome, so daemon restarts (typically dev rebuilds) reuse one approved
//! connection instead of re-triggering Chrome's allow-debugging prompt.
//!
//! The bridge dials Chrome once at startup (that handshake is where the user
//! clicks "Allow"), then serves a localhost WebSocket endpoint that relays
//! CDP frames verbatim for one downstream client at a time (newest wins).
//! It exits — which also clears Chrome's "is being debugged" banner — when
//! Chrome closes the connection, on `socai stop`, or after 3 hours with no
//! connected client.
//!
//! The daemon points the core CDP discovery at the bridge by setting
//! `SOCAI_CDP_WS` in its own process; nothing in `core` knows the bridge
//! exists.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use socai_core::cdp::{
    discover_existing_chrome_endpoint, open_remote_debugging_page,
    wait_for_existing_chrome_endpoint,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex, Notify};
use tokio::time::{sleep, timeout, Instant};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{accept_hdr_async, connect_async};

const ENDPOINT_NAME: &str = "cdp-bridge.json";
const LOG_NAME: &str = "cdp-bridge.log";
/// Marker segment in bridge ws paths. Lets the bridge recognise (and discard)
/// an inherited `SOCAI_CDP_WS` that points back at a bridge rather than at a
/// real Chrome, so it never relays to itself.
const CDP_PATH_PREFIX: &str = "/socai-cdp-bridge/";
const CONTROL_PATH_PREFIX: &str = "/socai-bridge-control/";
/// Suicide after this long without a connected client, so Chrome's
/// "is being debugged" banner doesn't linger forever.
const IDLE_TIMEOUT: Duration = Duration::from_secs(3 * 60 * 60);
/// Budget for the user to click Chrome's allow-debugging dialog (the dialog
/// holds our WebSocket handshake open until they answer).
const CHROME_CONNECT_TIMEOUT: Duration = Duration::from_secs(300);
/// How long to keep polling for a discoverable endpoint after opening
/// chrome://inspect on a machine where remote debugging is off entirely.
const DISCOVER_WAIT: Duration = Duration::from_secs(240);
/// How long `ensure_bridge` waits for a freshly spawned bridge to become
/// ready; covers the first-run consent click.
const SPAWN_WAIT: Duration = Duration::from_secs(300);
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Serialize, Deserialize)]
struct BridgeEndpoint {
    ws_url: String,
    control_url: String,
    pid: u32,
}

fn endpoint_path() -> Result<PathBuf> {
    Ok(crate::daemon::socai_home()?.join(ENDPOINT_NAME))
}

async fn read_endpoint() -> Option<BridgeEndpoint> {
    let path = endpoint_path().ok()?;
    let bytes = tokio::fs::read(&path).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// One-shot request/reply on the bridge's control channel.
async fn control_request(endpoint: &BridgeEndpoint, message: &str) -> Option<String> {
    let attempt = async {
        let (mut ws, _) = connect_async(&endpoint.control_url).await.ok()?;
        ws.send(Message::Text(message.into())).await.ok()?;
        match ws.next().await {
            Some(Ok(Message::Text(reply))) => Some(reply),
            _ => None,
        }
    };
    timeout(PROBE_TIMEOUT, attempt).await.ok().flatten()
}

/// Liveness check: ping the bridge's control endpoint and expect a pong.
async fn probe(endpoint: &BridgeEndpoint) -> bool {
    control_request(endpoint, "ping").await.as_deref() == Some("pong")
}

/// Make sure `SOCAI_CDP_WS` in this (daemon) process points at a live bridge,
/// spawning one if needed. Respects a user-supplied non-bridge `SOCAI_CDP_WS`
/// by leaving it untouched. Best-effort: on failure the env var is cleared so
/// core discovery falls back to connecting to Chrome directly.
pub async fn ensure_bridge_env() {
    if let Ok(value) = std::env::var("SOCAI_CDP_WS") {
        if !value.is_empty() && !value.contains(CDP_PATH_PREFIX) {
            return; // explicit user endpoint — never wrap it in a bridge
        }
    }
    match ensure_bridge().await {
        Ok(ws_url) => std::env::set_var("SOCAI_CDP_WS", ws_url),
        Err(err) => {
            eprintln!("cdp bridge unavailable, falling back to direct chrome connect: {err:#}");
            if let Ok(value) = std::env::var("SOCAI_CDP_WS") {
                if value.contains(CDP_PATH_PREFIX) {
                    std::env::remove_var("SOCAI_CDP_WS");
                }
            }
        }
    }
}

/// Return the ws URL of a live bridge, spawning one when none is running.
pub async fn ensure_bridge() -> Result<String> {
    if let Some(endpoint) = read_endpoint().await {
        if probe(&endpoint).await {
            return Ok(endpoint.ws_url);
        }
        let _ = tokio::fs::remove_file(endpoint_path()?).await;
    }

    let mut child = spawn_bridge()?;
    let deadline = Instant::now() + SPAWN_WAIT;
    loop {
        if let Some(endpoint) = read_endpoint().await {
            if probe(&endpoint).await {
                return Ok(endpoint.ws_url);
            }
        }
        if let Ok(Some(status)) = child.try_wait() {
            anyhow::bail!(
                "cdp bridge exited during startup ({status}); see {}",
                crate::daemon::socai_home()?.join(LOG_NAME).display()
            );
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "cdp bridge did not become ready in {SPAWN_WAIT:?} \
                 (waiting for the chrome allow-debugging dialog?)"
            );
        }
        sleep(Duration::from_millis(500)).await;
    }
}

/// Ask a running bridge to shut down. Returns true when one was stopped.
pub async fn stop_bridge() -> Result<bool> {
    let Some(endpoint) = read_endpoint().await else {
        return Ok(false);
    };
    let stopped = control_request(&endpoint, "shutdown").await.as_deref() == Some("ok");
    let _ = tokio::fs::remove_file(endpoint_path()?).await;
    Ok(stopped)
}

fn spawn_bridge() -> Result<std::process::Child> {
    let home = crate::daemon::socai_home()?;
    std::fs::create_dir_all(&home)?;
    crate::daemon::spawn_detached_subcommand("__bridge", &home.join(LOG_NAME), |command| {
        // Never hand a bridge URL down to the bridge itself; a user-supplied
        // real Chrome endpoint is inherited as-is.
        if std::env::var("SOCAI_CDP_WS").is_ok_and(|value| value.contains(CDP_PATH_PREFIX)) {
            command.env_remove("SOCAI_CDP_WS");
        }
    })
}

/// The `socai __bridge` entrypoint.
pub async fn run_bridge() -> Result<()> {
    // Self-protection if a bridge URL leaked into our env via inheritance.
    if let Ok(value) = std::env::var("SOCAI_CDP_WS") {
        if value.contains(CDP_PATH_PREFIX) {
            std::env::remove_var("SOCAI_CDP_WS");
        }
    }

    let home = crate::daemon::socai_home()?;
    tokio::fs::create_dir_all(&home).await?;
    let endpoint_file = home.join(ENDPOINT_NAME);
    let _ = tokio::fs::remove_file(&endpoint_file).await;

    // Find Chrome. On first run (debugging not enabled) open chrome://inspect
    // and wait for the user to enable it.
    let chrome = match discover_existing_chrome_endpoint().await? {
        Some(endpoint) => endpoint,
        None => {
            eprintln!("bridge: no chrome debugging endpoint; opening chrome://inspect");
            open_remote_debugging_page();
            wait_for_existing_chrome_endpoint(DISCOVER_WAIT, Duration::from_secs(1))
                .await?
                .ok_or_else(|| anyhow!("no chrome debugging endpoint appeared"))?
        }
    };

    eprintln!("bridge: connecting to chrome at {}", chrome.browser_ws_url);
    let (upstream, _) = timeout(
        CHROME_CONNECT_TIMEOUT,
        connect_async(&chrome.browser_ws_url),
    )
    .await
    .context("timed out waiting for chrome to accept debugging (allow-debugging dialog)")?
    .context("connect to chrome ws endpoint")?;
    eprintln!("bridge: connected to chrome");

    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let token = uuid::Uuid::new_v4().simple().to_string();
    let cdp_path = format!("{CDP_PATH_PREFIX}{token}");
    let control_path = format!("{CONTROL_PATH_PREFIX}{token}");
    let endpoint = BridgeEndpoint {
        ws_url: format!("ws://127.0.0.1:{port}{cdp_path}"),
        control_url: format!("ws://127.0.0.1:{port}{control_path}"),
        pid: std::process::id(),
    };
    write_endpoint_file(&endpoint_file, &endpoint).await?;
    eprintln!("bridge: listening on {}", endpoint.ws_url);

    let (up_sink, mut up_stream) = upstream.split();
    // Single writer task owns the upstream sink; clients feed it via channel.
    let (to_up_tx, mut to_up_rx) = mpsc::channel::<Message>(256);
    let up_writer = tokio::spawn(async move {
        let mut sink = up_sink;
        while let Some(message) = to_up_rx.recv().await {
            if sink.send(message).await.is_err() {
                break;
            }
        }
    });

    let downstream: Arc<Mutex<Option<mpsc::Sender<Message>>>> = Arc::new(Mutex::new(None));
    let last_activity = Arc::new(Mutex::new(Instant::now()));
    let stop = Arc::new(Notify::new());
    let mut idle_check = tokio::time::interval(Duration::from_secs(60));

    loop {
        tokio::select! {
            message = up_stream.next() => match message {
                Some(Ok(message @ (Message::Text(_) | Message::Binary(_)))) => {
                    *last_activity.lock().await = Instant::now();
                    if let Some(client) = downstream.lock().await.as_ref() {
                        // Slow/gone clients lose frames rather than stall Chrome.
                        let _ = client.try_send(message);
                    }
                }
                Some(Ok(_)) => {} // ws control frames are handled by the library
                Some(Err(_)) | None => {
                    eprintln!("bridge: chrome connection closed; exiting");
                    break;
                }
            },
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                tokio::spawn(serve_client(
                    stream,
                    cdp_path.clone(),
                    control_path.clone(),
                    to_up_tx.clone(),
                    downstream.clone(),
                    last_activity.clone(),
                    stop.clone(),
                ));
            }
            _ = idle_check.tick() => {
                let idle_for = last_activity.lock().await.elapsed();
                if downstream.lock().await.is_none() && idle_for > IDLE_TIMEOUT {
                    eprintln!("bridge: idle for {idle_for:?} with no client; exiting");
                    break;
                }
            }
            _ = stop.notified() => {
                eprintln!("bridge: shutdown requested; exiting");
                break;
            }
        }
    }

    let _ = tokio::fs::remove_file(&endpoint_file).await;
    up_writer.abort();
    Ok(())
}

async fn write_endpoint_file(path: &std::path::Path, endpoint: &BridgeEndpoint) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(endpoint)?;
    tokio::fs::write(path, bytes).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await;
    }
    Ok(())
}

/// Handshake an incoming connection and route it: the CDP path becomes the
/// (single) relay client, the control path answers ping/shutdown.
async fn serve_client(
    stream: TcpStream,
    cdp_path: String,
    control_path: String,
    to_up_tx: mpsc::Sender<Message>,
    downstream: Arc<Mutex<Option<mpsc::Sender<Message>>>>,
    last_activity: Arc<Mutex<Instant>>,
    stop: Arc<Notify>,
) {
    let mut path = String::new();
    let Ok(ws) = accept_hdr_async(stream, |request: &Request, response: Response| {
        path = request.uri().path().to_string();
        Ok(response)
    })
    .await
    else {
        return;
    };

    if path == control_path {
        serve_control(ws, stop).await;
    } else if path == cdp_path {
        serve_cdp(ws, to_up_tx, downstream, last_activity).await;
    }
    // Unknown path (bad token): drop the connection without relaying anything.
}

async fn serve_control(mut ws: tokio_tungstenite::WebSocketStream<TcpStream>, stop: Arc<Notify>) {
    while let Some(Ok(message)) = ws.next().await {
        match message {
            Message::Text(text) if text == "ping" => {
                if ws.send(Message::Text("pong".into())).await.is_err() {
                    break;
                }
            }
            Message::Text(text) if text == "shutdown" => {
                let _ = ws.send(Message::Text("ok".into())).await;
                let _ = ws.close(None).await;
                stop.notify_waiters();
                break;
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
}

async fn serve_cdp(
    ws: tokio_tungstenite::WebSocketStream<TcpStream>,
    to_up_tx: mpsc::Sender<Message>,
    downstream: Arc<Mutex<Option<mpsc::Sender<Message>>>>,
    last_activity: Arc<Mutex<Instant>>,
) {
    let (mut client_sink, mut client_stream) = ws.split();
    let (to_client_tx, mut to_client_rx) = mpsc::channel::<Message>(256);
    // Weak handle for identity checks only — a strong clone here would keep
    // the channel open after the slot drops us, stalling our writer's close.
    let our_channel = to_client_tx.downgrade();

    // Newest client wins: replacing the slot drops the previous sender, which
    // ends the previous client's writer task and closes its socket.
    *downstream.lock().await = Some(to_client_tx);
    *last_activity.lock().await = Instant::now();

    let writer = tokio::spawn(async move {
        while let Some(message) = to_client_rx.recv().await {
            if client_sink.send(message).await.is_err() {
                break;
            }
        }
        let _ = client_sink.close().await;
    });

    while let Some(Ok(message)) = client_stream.next().await {
        match message {
            message @ (Message::Text(_) | Message::Binary(_)) => {
                *last_activity.lock().await = Instant::now();
                if to_up_tx.send(message).await.is_err() {
                    break; // upstream gone; bridge is exiting
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Only clear the slot if it is still ours (a newer client may have
    // replaced us already; then our weak handle no longer upgrades).
    {
        let mut slot = downstream.lock().await;
        let still_ours = our_channel
            .upgrade()
            .is_some_and(|ours| slot.as_ref().is_some_and(|s| s.same_channel(&ours)));
        if still_ours {
            *slot = None;
        }
    }
    *last_activity.lock().await = Instant::now();
    writer.abort();
}
