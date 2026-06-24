//! Raw managed-chrome launcher.
//!
//! socai's managed profile launches its own isolated Chrome rather than
//! attaching to the user's daily browser. Previously that used
//! `chromiumoxide::Browser::launch`, but the scoped-CDP runtime no longer
//! depends on chromiumoxide at all (see [`crate::cdp::raw_client`]). We only
//! need chrome's *process* spawned with a remote-debugging port — control then
//! goes through the same scoped raw client used for the existing-browser path.
//!
//! Chrome is started with `--remote-debugging-port=0` so it picks a free port
//! and writes it (plus the browser websocket path) to
//! `<user_data_dir>/DevToolsActivePort`, which
//! [`crate::cdp::endpoint::endpoint_from_active_port`] already knows how to
//! read.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, Context};
use tokio::process::{Child, Command};
use tokio::time::Instant;
use tracing::info;

use crate::cdp::endpoint::{self, Endpoint};

const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Owns a managed Chrome child process. Killed on drop, so a disconnect,
/// reconnect, or daemon shutdown tears down the socai-launched browser —
/// mirroring the previous chromiumoxide `Browser` drop semantics. Chrome we
/// merely *reused* (already running, discovered via `DevToolsActivePort`) is
/// never wrapped in this; we don't own those processes and must not kill them.
pub struct ChromeProcess {
    child: Child,
}

impl Drop for ChromeProcess {
    fn drop(&mut self) {
        // `start_kill` is non-blocking; the tokio reaper collects the zombie.
        // Best-effort: if the process already exited this is a harmless no-op.
        let _ = self.child.start_kill();
    }
}

/// Launch socai's managed Chrome against `user_data_dir` and wait for its
/// remote-debugging endpoint to come up. Returns the (managed-marked) endpoint
/// and a process guard the caller must keep alive for the connection's
/// lifetime.
pub(crate) async fn launch_managed_chrome(
    user_data_dir: &Path,
) -> anyhow::Result<(Endpoint, ChromeProcess)> {
    let executable = find_chrome_executable().ok_or_else(|| {
        anyhow!(
            "no chrome/chromium executable found. install Google Chrome, or set \
             SOCAI_CHROME_EXECUTABLE / CHROME to its path."
        )
    })?;

    // Remove any stale DevToolsActivePort before launching. We kill managed
    // chrome with SIGKILL (see `ChromeProcess::drop`), which leaves the marker
    // behind, so a stale one pointing at a dead/recycled port is the *normal*
    // state on the persistent managed profile. Deleting it first guarantees
    // `wait_for_active_port` only observes the port THIS process writes —
    // otherwise the first poll races and may return the stale endpoint, after
    // which we'd connect to nothing (or a foreign browser) while killing the
    // chrome we just launched.
    let marker = user_data_dir.join("DevToolsActivePort");
    let _ = tokio::fs::remove_file(&marker).await;

    let mut command = Command::new(&executable);
    command
        .arg(format!("--user-data-dir={}", user_data_dir.display()))
        .arg("--remote-debugging-port=0")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-blink-features=AutomationControlled")
        .arg("--window-size=1280,900")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    info!(
        executable = %executable.display(),
        profile = %user_data_dir.display(),
        "launching managed chrome"
    );
    let child = command
        .spawn()
        .with_context(|| format!("failed to launch chrome at {}", executable.display()))?;
    let process = ChromeProcess { child };

    let endpoint = wait_for_active_port(user_data_dir, LAUNCH_TIMEOUT).await?;
    Ok((
        endpoint::mark_managed_endpoint(endpoint, user_data_dir),
        process,
    ))
}

/// Poll `<user_data_dir>/DevToolsActivePort` until chrome has written its
/// chosen port, or `timeout` elapses.
async fn wait_for_active_port(user_data_dir: &Path, timeout: Duration) -> anyhow::Result<Endpoint> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(endpoint) = endpoint::endpoint_from_active_port(user_data_dir).await {
            return Ok(endpoint);
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "managed chrome did not expose a debugging endpoint within {}s",
                timeout.as_secs()
            ));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Resolve the chrome/chromium executable: explicit env override first
/// (`SOCAI_CHROME_EXECUTABLE`, then `CHROME`), then well-known per-OS install
/// locations.
pub(crate) fn find_chrome_executable() -> Option<PathBuf> {
    if let Some(path) = chrome_executable_override() {
        return Some(path);
    }
    if let Some(found) = default_chrome_executables()
        .into_iter()
        .find(|path| path.exists())
    {
        return Some(found);
    }
    // Last resort: a PATH-resolvable command name, left for `Command` to
    // resolve at spawn time. Only set on platforms where launching by bare
    // name is conventional (Linux); `None` elsewhere.
    fallback_chrome_command()
}

fn chrome_executable_override() -> Option<PathBuf> {
    for key in ["SOCAI_CHROME_EXECUTABLE", "CHROME"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(PathBuf::from(value));
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn default_chrome_executables() -> Vec<PathBuf> {
    [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
    ]
    .iter()
    .map(PathBuf::from)
    .collect()
}

#[cfg(target_os = "linux")]
fn default_chrome_executables() -> Vec<PathBuf> {
    let names = [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "microsoft-edge",
        "brave-browser",
    ];
    let roots = ["/usr/bin", "/usr/local/bin", "/snap/bin", "/opt/google/chrome"];
    let mut out = Vec::new();
    for root in roots {
        for name in names {
            out.push(PathBuf::from(root).join(name));
        }
    }
    out
}

#[cfg(target_os = "windows")]
fn default_chrome_executables() -> Vec<PathBuf> {
    let suffixes = [
        r"Google\Chrome\Application\chrome.exe",
        r"Chromium\Application\chrome.exe",
        r"Microsoft\Edge\Application\msedge.exe",
        r"BraveSoftware\Brave-Browser\Application\brave.exe",
    ];
    let mut out = Vec::new();
    for key in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
        if let Ok(base) = std::env::var(key) {
            if !base.trim().is_empty() {
                for suffix in suffixes {
                    out.push(PathBuf::from(&base).join(suffix));
                }
            }
        }
    }
    out
}

#[cfg(target_os = "linux")]
fn fallback_chrome_command() -> Option<PathBuf> {
    Some(PathBuf::from("google-chrome"))
}

#[cfg(not(target_os = "linux"))]
fn fallback_chrome_command() -> Option<PathBuf> {
    None
}
