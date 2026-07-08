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
#[cfg(unix)]
use tracing::warn;

use crate::cdp::endpoint::{self, Endpoint};

const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(150);
#[cfg(unix)]
const EVICT_GRACE: Duration = Duration::from_secs(5);

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

    // Chrome enforces a per-profile process singleton: if another chrome
    // already owns this profile, the one we spawn below silently forwards its
    // arguments to that instance and exits without ever writing
    // `DevToolsActivePort`. The caller already tried (and failed) to attach
    // via the active-port marker, so any live singleton holder at this point
    // is unreachable — typically an orphaned automation chrome squatting the
    // socai-owned profile. Evict it so the launch below owns the profile.
    #[cfg(unix)]
    evict_profile_singleton_holder(user_data_dir).await;

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
    let mut process = ChromeProcess { child };

    let endpoint = wait_for_active_port(user_data_dir, &mut process.child, LAUNCH_TIMEOUT).await?;
    Ok((
        endpoint::mark_managed_endpoint(endpoint, user_data_dir),
        process,
    ))
}

/// Poll `<user_data_dir>/DevToolsActivePort` until chrome has written its
/// chosen port, or `timeout` elapses. Also watches the spawned process: a
/// chrome that finds the profile's singleton already held hands its arguments
/// to that instance and exits almost immediately, so the marker would never
/// appear — fail fast with a diagnosis instead of burning the full timeout.
async fn wait_for_active_port(
    user_data_dir: &Path,
    child: &mut Child,
    timeout: Duration,
) -> anyhow::Result<Endpoint> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(endpoint) = endpoint::endpoint_from_active_port(user_data_dir).await {
            return Ok(endpoint);
        }
        if let Some(status) = child
            .try_wait()
            .context("failed to poll managed chrome process state")?
        {
            return Err(anyhow!(
                "managed chrome exited during startup ({status}) without exposing a \
                 debugging endpoint. another chrome instance likely owns the managed \
                 profile ({}); quit chrome processes using that profile and retry.",
                user_data_dir.display()
            ));
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

/// Terminate a live chrome that holds this profile's process singleton.
///
/// The `SingletonLock` symlink target is `<hostname>-<pid>`. We only signal
/// when that pid is alive AND its command line references this exact
/// `--user-data-dir`, so a recycled pid, an unrelated process, or a lock
/// inherited from another machine (shared home dir) is never touched. Stale
/// locks from dead processes are left alone — chrome detects and replaces
/// them itself, and `ChromeProcess::drop` (SIGKILL) leaves that state behind
/// routinely. Best-effort: on any doubt we skip, and the child-exit check in
/// `wait_for_active_port` reports the conflict instead.
///
/// SIGTERM first so chrome cleans up its `Singleton*` files, escalating to
/// SIGKILL after a grace period (a SIGKILLed holder leaves stale lock files,
/// which the relaunch handles as above).
///
/// Unix only: Windows chrome uses a named mutex, not a `SingletonLock`
/// symlink, so there is nothing to inspect there.
#[cfg(unix)]
async fn evict_profile_singleton_holder(user_data_dir: &Path) {
    let lock = user_data_dir.join("SingletonLock");
    let Ok(target) = tokio::fs::read_link(&lock).await else {
        return; // no lock (or not a symlink): nothing holds the profile
    };
    // Target looks like "<hostname>-<pid>"; hostnames may themselves
    // contain '-', so split from the right.
    let Some(pid) = target
        .to_str()
        .and_then(|t| t.rsplit_once('-'))
        .and_then(|(_, pid)| pid.parse::<i32>().ok())
        .filter(|pid| *pid > 0)
    else {
        return;
    };
    let expected = format!("--user-data-dir={}", user_data_dir.display());
    match process_args(pid).await {
        Some(args) if args.contains(&expected) => {}
        _ => return, // dead pid, or recycled to something that isn't chrome-on-this-profile
    }
    warn!(
        pid,
        profile = %user_data_dir.display(),
        "terminating foreign chrome holding the managed profile singleton"
    );
    signal(pid, libc::SIGTERM);
    let deadline = Instant::now() + EVICT_GRACE;
    while Instant::now() < deadline {
        if !process_alive(pid) {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    warn!(pid, "chrome ignored SIGTERM; escalating to SIGKILL");
    signal(pid, libc::SIGKILL);
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline && process_alive(pid) {
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Full command line of `pid`, via `ps` (portable across macOS/Linux; macOS
/// has no /proc). `None` when the pid is gone or `ps` fails.
#[cfg(unix)]
async fn process_args(pid: i32) -> Option<String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let args = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!args.is_empty()).then_some(args)
}

#[cfg(unix)]
fn signal(pid: i32, sig: libc::c_int) {
    // SAFETY: plain kill(2); pid was parsed from the singleton lock and
    // verified against a live process's command line before any signal.
    unsafe { libc::kill(pid, sig) };
}

#[cfg(unix)]
fn process_alive(pid: i32) -> bool {
    // kill(pid, 0) probes existence without delivering a signal. EPERM (alive
    // but unsignalable) is reported as dead, which is fine: we could not have
    // terminated such a process anyway, and the child-exit check downstream
    // still surfaces the conflict.
    unsafe { libc::kill(pid, 0) == 0 }
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
