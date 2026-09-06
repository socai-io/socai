use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::agent::{
    config_for, configured_default_model_for, load_provider_credential, resolve_provider,
    run_agent_with_events, AgentEvent, AgentOptions, AgentOutcome, AnthropicBackend, Backend,
    Message, OpenAICompatBackend, Provider, Tool,
};
use crate::cdp::{
    BrowserEvent, Cdp, ChromeConnectOptions, ChromeProfile, PageSession, PageSessionManager,
    StatusPayload, TargetInfo,
};
use anyhow::{anyhow, Result};
use tokio::sync::{broadcast, Mutex};
use tokio::time::{sleep, Instant};

use super::BrowserStatus;

/// How long a caller waits for a connect. Keep this above
/// `cdp::lifecycle::CONNECT_BUDGET` so the connect task always settles first:
/// the waiter cannot cancel it, and a task still running past this point could
/// acquire a browser (or mint a hosted session) nobody is waiting for.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(90);

/// How long a remote hosted session may sit connected with no work in flight
/// before it is released. Remote sessions bill by the minute and die at a
/// fixed server-side timeout anyway, so an idle one is pure cost: holding it
/// only saves the few seconds a re-mint takes. Local/existing browsers are
/// never touched by this — keeping them warm is free.
const REMOTE_IDLE_RELEASE: Duration = Duration::from_secs(90);
const REMOTE_IDLE_TICK: Duration = Duration::from_secs(15);

/// Minimum remaining session lifetime worth starting new work on. A remote
/// session's clock starts at mint, so one reused late would hand the run an
/// invisible, shortened deadline; re-minting up front trades a few seconds of
/// connect latency for not dying mid-run. This check runs only at task start
/// (tools hold the page for the whole turn), so it must cover a realistic
/// turn, not a single call: at 300s, turns inheriting a part-used session
/// died mid-run — real batch turns chain multi-minute searches for 10-15
/// minutes.
const REMOTE_MIN_RUN_BUDGET: Duration = Duration::from_secs(600);

/// Global cap on agent runs that may hold this process's browser at once.
/// Parallel tasks all drive one browser — one hosted session on the remote
/// profile — so the ceiling belongs to the browser, not to any one task kind.
/// Kept in step with the desktop's `MAX_CONCURRENT_AGENT_TASKS`; the env
/// override exists for support cases, not as a product knob.
const DEFAULT_MAX_BROWSER_RUNS: usize = 3;

/// How long a run waits before asking for the browser again. Every refusal is
/// a "come back later", never a failure: whatever blocks admission (another
/// run finishing, a session being re-minted, a server hiccup) resolves on its
/// own within seconds.
const BROWSER_BUSY_RETRY: Duration = Duration::from_millis(1_500);

/// Shared backoff after a hosted session fails to mint. Without it every
/// queued run would fire its own request at a server that just said no.
const REMOTE_MINT_COOLDOWN: Duration = Duration::from_secs(10);

/// Smallest remaining hosted-session lifetime a run will start on while other
/// runs are still on that session. Below `REMOTE_MIN_RUN_BUDGET` a lone run
/// re-mints; a run that cannot re-mint without killing its neighbours takes
/// the shortened budget instead, and only waits once the session is this close
/// to dying — at which point the neighbours are about to lose it anyway.
const REMOTE_MIN_SHARED_BUDGET: Duration = Duration::from_secs(60);

/// Work-in-flight signal for the remote idle reaper. Guards are held by the
/// entrypoints around browser-touching work (a daemon command, a whole agent
/// task), so the reaper never releases a session under an active run — LLM
/// thinking pauses between tool calls included.
struct Activity {
    /// Admission gate between runs and teardown. In-flight work holds the
    /// read side; the reaper only tears down under `try_write`, which
    /// succeeds exactly when no work is in flight. This makes "a run cannot
    /// have its session released from under it" true by construction: a run
    /// arriving mid-teardown blocks in `begin_activity` for the few seconds
    /// teardown takes, then reconnects through `ensure_site_page`.
    gate: Arc<tokio::sync::RwLock<()>>,
    last_done: std::sync::Mutex<std::time::Instant>,
    reaper_started: std::sync::atomic::AtomicBool,
}

impl Activity {
    fn new() -> Self {
        Self {
            gate: Arc::new(tokio::sync::RwLock::new(())),
            last_done: std::sync::Mutex::new(std::time::Instant::now()),
            reaper_started: std::sync::atomic::AtomicBool::new(false),
        }
    }

    // A poisoned lock still holds a valid instant, so both accessors recover
    // the guard instead of propagating a panic from an unrelated thread.
    fn touch(&self) {
        *self
            .last_done
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = std::time::Instant::now();
    }

    fn idle_for(&self) -> Duration {
        self.last_done
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .elapsed()
    }
}

/// The browser cannot take this run right now, but it will shortly. Callers
/// are expected to give back whatever else they hold (a task permit, a queue
/// slot), sleep for `retry_after`, and ask again — never to fail the run.
#[derive(Debug, Clone)]
pub struct BrowserBusy {
    pub retry_after: Duration,
    pub reason: String,
    pub kind: BrowserBusyKind,
}

/// Why admission was refused. Capacity refusals clear themselves as soon as
/// the runs ahead finish, so they are retried indefinitely; connect refusals
/// may be a server outage or a spent daily quota, so callers bound them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserBusyKind {
    Capacity,
    Connect,
}

impl BrowserBusy {
    fn capacity(reason: impl Into<String>, retry_after: Duration) -> Self {
        Self {
            retry_after,
            reason: reason.into(),
            kind: BrowserBusyKind::Capacity,
        }
    }

    fn connect(reason: impl Into<String>, retry_after: Duration) -> Self {
        Self {
            retry_after,
            reason: reason.into(),
            kind: BrowserBusyKind::Connect,
        }
    }

    /// Recover the refusal from an `anyhow` chain. Page acquisition returns
    /// `anyhow::Result`, so this is how a caller tells "wait and retry" apart
    /// from a genuine browser failure.
    pub fn find(error: &anyhow::Error) -> Option<&BrowserBusy> {
        error
            .chain()
            .find_map(|cause| cause.downcast_ref::<BrowserBusy>())
    }
}

impl std::fmt::Display for BrowserBusy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            // A capacity refusal has no underlying failure to report, so it
            // says why it is waiting. A connect refusal carries the real
            // browser error and shows that instead — it is what the user sees
            // if the retries run out.
            BrowserBusyKind::Capacity => write!(formatter, "browser is busy: {}", self.reason),
            BrowserBusyKind::Connect => formatter.write_str(&self.reason),
        }
    }
}

impl std::error::Error for BrowserBusy {}

/// Process-wide admission to the browser. One connection serves every run, so
/// the count of runs holding it, the decision to tear it down, and the backoff
/// after a failed hosted mint all live in one place.
#[derive(Debug)]
struct BrowserAdmission {
    max_runs: usize,
    leases: AtomicUsize,
    /// Serializes the connect/teardown decision. Without it two runs starting
    /// together could both observe "needs re-mint" and disconnect each other's
    /// freshly minted session.
    gate: Mutex<()>,
    cooldown_until: std::sync::Mutex<Option<Instant>>,
    /// Set once a run has found that the browser has to be reopened. It stops
    /// a second lease from being handed out, so the runs waiting for the swap
    /// drain to one and that one can do it. Without it, several waiting runs
    /// keep each other non-exclusive and none of them ever gets to reconnect.
    /// Any successful page acquisition clears it, so it cannot outlive the
    /// swap it was raised for.
    draining: AtomicBool,
}

impl BrowserAdmission {
    fn new() -> Self {
        Self {
            max_runs: configured_max_browser_runs(),
            leases: AtomicUsize::new(0),
            gate: Mutex::new(()),
            cooldown_until: std::sync::Mutex::new(None),
            draining: AtomicBool::new(false),
        }
    }

    fn cooldown_remaining(&self) -> Option<Duration> {
        let until = (*self
            .cooldown_until
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))?;
        let remaining = until.saturating_duration_since(Instant::now());
        (!remaining.is_zero()).then_some(remaining)
    }

    fn start_cooldown(&self) {
        *self
            .cooldown_until
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(Instant::now() + REMOTE_MINT_COOLDOWN);
    }

    fn clear_cooldown(&self) {
        *self
            .cooldown_until
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    fn try_acquire(self: &Arc<Self>) -> Result<BrowserLease, BrowserBusy> {
        if let Some(remaining) = self.cooldown_remaining() {
            return Err(BrowserBusy::connect(
                "waiting out the backoff from a failed browser connect",
                remaining,
            ));
        }
        let draining = self.draining.load(Ordering::Acquire);
        let mut current = self.leases.load(Ordering::Acquire);
        loop {
            if draining && current >= 1 {
                return Err(BrowserBusy::capacity(
                    "the browser is being reopened for a queued run",
                    BROWSER_BUSY_RETRY,
                ));
            }
            if current >= self.max_runs {
                return Err(BrowserBusy::capacity(
                    format!(
                        "{current} of {} browser run slots are in use",
                        self.max_runs
                    ),
                    BROWSER_BUSY_RETRY,
                ));
            }
            match self.leases.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
        Ok(BrowserLease {
            admission: self.clone(),
        })
    }

    /// True when the caller is the only run holding the browser, i.e. tearing
    /// the connection down would not pull a page out from under anyone else.
    /// A caller without a lease (the CLI and TUI, which run one task at a
    /// time) reads zero and is exclusive by construction.
    fn is_exclusive(&self) -> bool {
        self.leases.load(Ordering::Acquire) <= 1
    }

    fn begin_draining(&self) {
        self.draining.store(true, Ordering::Release);
    }

    fn finish_draining(&self) {
        self.draining.store(false, Ordering::Release);
    }
}

/// Whether waiting could plausibly fix a failed connect. Transport trouble, a
/// server hiccup and a chrome that lost a race all clear on their own; an
/// account that is not entitled to the hosted browser, a server with it turned
/// off, and a device that has spent its daily allowance do not.
fn connect_failure_is_retryable(reason: &str) -> bool {
    const PERMANENT: [&str; 5] = [
        "requires socai pro",
        "pro access is required",
        "is not enabled on this server",
        "daily remote browser time limit",
        "is not configured",
    ];
    let reason = reason.to_ascii_lowercase();
    !PERMANENT.iter().any(|signal| reason.contains(signal))
}

fn configured_max_browser_runs() -> usize {
    std::env::var("SOCAI_MAX_BROWSER_RUNS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_BROWSER_RUNS)
}

/// One run's claim on the shared browser, held for the whole run. Dropping it
/// frees the slot and lets a waiting run in.
#[derive(Debug)]
pub struct BrowserLease {
    admission: Arc<BrowserAdmission>,
}

impl Drop for BrowserLease {
    fn drop(&mut self) {
        self.admission.leases.fetch_sub(1, Ordering::AcqRel);
    }
}

/// RAII marker for in-flight browser work; see [`SocaiRuntime::begin_activity`].
pub struct ActivityGuard {
    activity: Arc<Activity>,
    /// Read permit on the teardown gate. Dropped after `Drop::drop` stamps
    /// the idle clock, so the reaper can never observe "gate free" with a
    /// stale `last_done`.
    _permit: tokio::sync::OwnedRwLockReadGuard<()>,
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        self.activity.touch();
    }
}

/// Shared in-process runtime handle for one entrypoint. Tauri, TUI, and the
/// CLI daemon each construct their own instance; the daemon is only an IPC
/// wrapper around this same object graph.
#[derive(Clone)]
pub struct SocaiRuntime {
    cdp: Cdp,
    site_pages: Arc<Mutex<HashMap<String, Arc<PageSession>>>>,
    activity: Arc<Activity>,
    admission: Arc<BrowserAdmission>,
}

/// Own a newly created target until navigation and cache insertion complete.
/// Async task cancellation drops local values, so this guard closes a target
/// created immediately before a cancelled navigation await.
struct UncachedPageGuard {
    runtime: SocaiRuntime,
    target_id: Option<String>,
}

impl UncachedPageGuard {
    fn disarm(&mut self) {
        self.target_id = None;
    }
}

impl Drop for UncachedPageGuard {
    fn drop(&mut self) {
        let Some(target_id) = self.target_id.take() else {
            return;
        };
        let runtime = self.runtime.clone();
        tokio::spawn(async move {
            let _ = runtime.close_target(&target_id).await;
        });
    }
}

impl SocaiRuntime {
    pub fn new() -> Self {
        Self {
            cdp: Cdp::new(),
            site_pages: Arc::new(Mutex::new(HashMap::new())),
            activity: Arc::new(Activity::new()),
            admission: Arc::new(BrowserAdmission::new()),
        }
    }

    /// Claim one of the global browser run slots for a whole run. Refusals are
    /// always temporary — see [`BrowserBusy`] — so a caller that gets one gives
    /// back its other resources, waits, and asks again.
    pub fn try_acquire_browser_lease(&self) -> Result<BrowserLease, BrowserBusy> {
        self.admission.try_acquire()
    }

    /// Mark browser work as in flight until the returned guard drops. Every
    /// entrypoint wraps its browser-touching unit of work in one of these —
    /// the daemon around each site command, the app and TUI around a whole
    /// agent task — so the remote idle reaper only counts true idle time.
    /// Blocks only while an idle teardown is mid-flight (a few seconds), in
    /// which case the caller proceeds onto a fresh connection.
    pub async fn begin_activity(&self) -> ActivityGuard {
        let permit = self.activity.gate.clone().read_owned().await;
        ActivityGuard {
            activity: self.activity.clone(),
            _permit: permit,
        }
    }

    /// Lazily start the loop that releases an idle remote session. One task
    /// per runtime, alive for the process; it is a no-op every tick unless a
    /// socai-minted remote session is connected.
    fn ensure_idle_reaper(&self) {
        use std::sync::atomic::Ordering;
        if self
            .activity
            .reaper_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            // No runtime yet (sync construction path); the next async caller
            // retries.
            self.activity.reaper_started.store(false, Ordering::SeqCst);
            return;
        };
        let runtime = self.clone();
        handle.spawn(async move {
            // Tracks which session the idle window applies to. A connect can
            // eat most of its 85s budget before succeeding, so the idle clock
            // restarts when a new session's deadline first appears — without
            // that, a slow connect could be reaped seconds after becoming
            // usable.
            let mut watched_deadline = None;
            loop {
                sleep(REMOTE_IDLE_TICK).await;
                let activity = &runtime.activity;
                let Some(deadline) = runtime.cdp.remote_session_deadline().await else {
                    watched_deadline = None;
                    continue;
                };
                if watched_deadline != Some(deadline) {
                    watched_deadline = Some(deadline);
                    activity.touch();
                    continue;
                }
                if activity.idle_for() < REMOTE_IDLE_RELEASE {
                    continue;
                }
                // The write side admits teardown only while no run holds a
                // read permit; a run arriving after this point waits in
                // begin_activity until teardown finishes, then reconnects.
                let Ok(_teardown) = activity.gate.clone().try_write_owned() else {
                    continue;
                };
                tracing::info!(
                    idle_secs = REMOTE_IDLE_RELEASE.as_secs(),
                    "releasing idle remote browser session"
                );
                runtime.drop_site_sessions().await;
                runtime.disconnect_browser().await;
            }
        });
    }

    pub fn browser(&self) -> Cdp {
        self.cdp.clone()
    }

    pub fn subscribe_browser_events(&self) -> broadcast::Receiver<BrowserEvent> {
        self.cdp.subscribe()
    }

    pub fn connect_browser(&self) {
        // A fresh connect counts as activity: without the touch, a stale idle
        // clock could reap a remote session seconds after the user asked for
        // it. Both fns run inside an async runtime (cdp.connect spawns), so
        // the reaper start never falls back.
        self.activity.touch();
        self.ensure_idle_reaper();
        self.cdp.connect();
    }

    pub fn connect_browser_once(&self) {
        self.activity.touch();
        self.ensure_idle_reaper();
        self.cdp.connect_once();
    }

    pub fn connect_browser_with_options(&self, options: ChromeConnectOptions) {
        self.activity.touch();
        self.ensure_idle_reaper();
        self.cdp.connect_with_options(options);
    }

    pub async fn disconnect_browser(&self) {
        self.cdp.disconnect().await;
    }

    pub async fn browser_status(&self) -> StatusPayload {
        self.cdp.status().await
    }

    pub async fn browser_pages(&self) -> Vec<TargetInfo> {
        self.cdp.pages().await
    }

    pub async fn wait_browser_connected(&self) -> Result<()> {
        self.cdp.wait_connected().await
    }

    pub fn page_sessions(&self) -> PageSessionManager {
        PageSessionManager::new(self.cdp.clone())
    }

    pub async fn create_page(&self, start_url: &str) -> Result<PageSession> {
        self.page_sessions().create_page(start_url).await
    }

    pub async fn close_target(&self, target_id: &str) -> Result<bool> {
        self.page_sessions().close_target(target_id).await
    }

    /// Return the reusable page for a site within this process, creating it
    /// on first use. This is intentionally site-agnostic; site-specific
    /// readiness checks live in `socai-core`.
    pub async fn ensure_site_page(
        &self,
        site_id: &str,
        start_url: &str,
    ) -> Result<Arc<PageSession>> {
        self.ensure_site_page_inner(site_id, start_url, None).await
    }

    pub async fn ensure_site_page_with_browser_options(
        &self,
        site_id: &str,
        start_url: &str,
        options: ChromeConnectOptions,
    ) -> Result<Arc<PageSession>> {
        self.ensure_site_page_inner(site_id, start_url, Some(options))
            .await
    }

    /// Return one reusable page owned by a conversation session. Different
    /// sessions always use different Chrome targets; later turns in the same
    /// conversation keep using that target even when the selected site changes.
    ///
    /// Takes the run's [`BrowserLease`] because acquiring a page can require
    /// re-connecting the shared browser, which is only safe while no other run
    /// is on it. When it is not safe the call returns a [`BrowserBusy`] error
    /// instead of tearing down a browser someone else is driving.
    pub async fn ensure_session_site_page_with_browser_options(
        &self,
        _lease: &BrowserLease,
        session_id: &str,
        _site_id: &str,
        start_url: &str,
        options: ChromeConnectOptions,
    ) -> Result<Arc<PageSession>> {
        let session_id = session_id.trim();
        if session_id.is_empty() {
            anyhow::bail!("session_id is empty");
        }
        let page_key = format!("session:{session_id}");
        self.ensure_site_page_inner(&page_key, start_url, Some(options))
            .await
    }

    async fn ensure_site_page_inner(
        &self,
        page_key: &str,
        start_url: &str,
        options: Option<ChromeConnectOptions>,
    ) -> Result<Arc<PageSession>> {
        let page_key = page_key.trim();
        if page_key.is_empty() {
            anyhow::bail!("page key is empty");
        }
        self.ensure_idle_reaper();

        // One run at a time decides whether the shared browser has to be torn
        // down and re-opened. Parallel runs would otherwise disconnect each
        // other's freshly opened browser, or each mint their own hosted
        // session.
        let _admission_gate = self.admission.gate.lock().await;
        let exclusive = self.admission.is_exclusive();

        if let Some(options) = options.as_ref() {
            if !browser_status_matches_options(&self.browser_status().await, options) {
                // The live browser is not the one this run is configured for
                // (the user switched local/managed/remote between turns).
                // Reconnecting closes every page, so it waits for the runs
                // already driving the old browser to finish.
                if !exclusive {
                    self.admission.begin_draining();
                    return Err(BrowserBusy::capacity(
                        "the browser source changed while other runs are still on the previous browser",
                        BROWSER_BUSY_RETRY,
                    )
                    .into());
                }
                let _ = self.close_all_site_sessions().await;
                self.disconnect_browser().await;
            }
        }

        // A remote session near its server-side end-of-life is re-minted
        // before work starts on it rather than dying mid-run: release now,
        // and the reconnect below mints a session with a full budget. Only a
        // run that has the hosted session to itself may do that; otherwise it
        // shares whatever budget is left, and waits only once that budget is
        // too short to be worth starting on.
        if let Some(deadline) = self.cdp.remote_session_deadline().await {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining < REMOTE_MIN_RUN_BUDGET {
                if !exclusive {
                    if remaining < REMOTE_MIN_SHARED_BUDGET {
                        self.admission.begin_draining();
                        return Err(BrowserBusy::capacity(
                            "the hosted browser session is about to expire and other runs are still on it",
                            BROWSER_BUSY_RETRY,
                        )
                        .into());
                    }
                    tracing::info!(
                        remaining_secs = remaining.as_secs(),
                        "remote browser session near timeout; sharing it with the runs already in flight"
                    );
                } else {
                    tracing::info!(
                        remaining_secs = remaining.as_secs(),
                        "remote browser session near timeout; re-minting before new work"
                    );
                    self.drop_site_sessions().await;
                    self.disconnect_browser().await;
                }
            }
        }

        let mut pages = self.site_pages.lock().await;
        if let Some(page) = pages.get(page_key) {
            if page.page_info().await.is_ok() {
                self.admission.finish_draining();
                return Ok(page.clone());
            }
            pages.remove(page_key);
        }

        let profile = options.as_ref().map(|options| options.profile);
        let connect = match options {
            Some(options) => wait_browser_connected_with_options(self, options).await,
            None => wait_browser_connected(self).await,
        };
        match connect {
            Ok(()) => self.admission.clear_cooldown(),
            // Connecting failed for everyone, not just this run: a hosted mint
            // that the server refused, a chrome that would not come up. Back
            // the whole process off once and let the caller retry rather than
            // failing a run on what is usually a transient condition.
            Err(error) => {
                let reason = format!("{error:#}");
                // Only the run path (which passes connect options) retries;
                // the CLI and TUI ask once and want the raw failure verbatim.
                // A failure that waiting cannot fix — an unentitled account, a
                // spent daily quota — is reported straight away too, so the
                // user reads what actually went wrong instead of watching the
                // task sit in the queue.
                if profile.is_none() || !connect_failure_is_retryable(&reason) {
                    tracing::warn!(profile = ?profile, error = %reason, "browser connect failed");
                    return Err(error);
                }
                self.admission.start_cooldown();
                tracing::warn!(profile = ?profile, error = %reason, "browser connect failed; backing off");
                return Err(anyhow::Error::new(BrowserBusy::connect(
                    reason,
                    REMOTE_MINT_COOLDOWN,
                )));
            }
        }
        let page = Arc::new(self.create_page("about:blank").await?);
        let mut page_guard = UncachedPageGuard {
            runtime: self.clone(),
            target_id: Some(page.target_id().to_string()),
        };
        if !start_url.trim().is_empty() {
            page.navigate_with_timeout(start_url, 60.0).await?;
        }
        pages.insert(page_key.to_string(), page.clone());
        page_guard.disarm();
        // Whatever swap this run was waiting for is done; let the queued runs
        // back in.
        self.admission.finish_draining();
        Ok(page)
    }

    pub async fn close_site_session(&self, site_id: &str) -> Result<bool> {
        let Some(page) = self.site_pages.lock().await.remove(site_id.trim()) else {
            return Ok(false);
        };
        if let Ok(page) = Arc::try_unwrap(page) {
            page.close().await?;
        }
        Ok(true)
    }

    pub async fn close_all_site_sessions(&self) -> Result<usize> {
        let pages = std::mem::take(&mut *self.site_pages.lock().await);
        let count = pages.len();
        for (_, page) in pages {
            if let Ok(page) = Arc::try_unwrap(page) {
                page.close().await?;
            }
        }
        Ok(count)
    }

    /// Drop the reusable site pages without issuing any CDP commands. For the
    /// remote teardown paths (idle release, pre-run re-mint): the pages'
    /// targets die with the browser anyway, and a `page.close()` against a
    /// wedged remote websocket can stall far past the teardown budget — the
    /// command channel send is unbounded and each command can then wait
    /// `raw_client::COMMAND_TIMEOUT`. `disconnect()` sweeps owned-target
    /// bookkeeping itself.
    async fn drop_site_sessions(&self) {
        self.site_pages.lock().await.clear();
    }
}

impl Default for SocaiRuntime {
    fn default() -> Self {
        Self::new()
    }
}

fn browser_status_matches_options(status: &StatusPayload, options: &ChromeConnectOptions) -> bool {
    let StatusPayload::Connected {
        managed,
        remote,
        user_data_dir,
        ..
    } = status
    else {
        return true;
    };

    match options.profile {
        ChromeProfile::Existing => !managed && !remote,
        ChromeProfile::Managed => {
            if !managed {
                return false;
            }
            match &options.managed_user_data_dir {
                Some(expected) => {
                    let expected = expected.to_string_lossy();
                    user_data_dir.as_deref() == Some(expected.as_ref())
                }
                None => true,
            }
        }
        // An explicit SOCAI_CDP_* override connected under `remote` reports
        // `remote: false` (socai didn't mint it); that combination is a debug
        // facility and this options-matching path currently has no callers.
        ChromeProfile::Remote => *remote,
        // Auto resolves to managed or existing — never a hosted browser — so
        // a remote connection satisfies neither and must be torn down.
        ChromeProfile::Auto => !remote,
    }
}

/// Wait until the runtime reports a connected browser, kicking off a connect
/// if it isn't already in flight. Times out after 90s.
pub async fn wait_browser_connected(runtime: &SocaiRuntime) -> Result<()> {
    wait_browser_connected_inner(runtime, None).await
}

pub async fn wait_browser_connected_with_options(
    runtime: &SocaiRuntime,
    options: ChromeConnectOptions,
) -> Result<()> {
    wait_browser_connected_inner(runtime, Some(options)).await
}

async fn wait_browser_connected_inner(
    runtime: &SocaiRuntime,
    options: Option<ChromeConnectOptions>,
) -> Result<()> {
    match options {
        Some(options) => runtime.connect_browser_with_options(options),
        None => runtime.connect_browser(),
    }
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let mut saw_connect_attempt = false;
    loop {
        match runtime.browser_status().await {
            BrowserStatus::Connected { .. } => return Ok(()),
            BrowserStatus::Connecting { .. } => saw_connect_attempt = true,
            BrowserStatus::Disconnected { reason }
                if reason != "not_yet_connected" && saw_connect_attempt =>
            {
                return Err(anyhow!("CDP disconnected: {reason}"));
            }
            BrowserStatus::Disconnected { .. } => {}
        }
        if Instant::now() >= deadline {
            return Err(anyhow!("CDP did not connect within {:?}", CONNECT_TIMEOUT));
        }
        sleep(Duration::from_millis(250)).await;
    }
}

pub fn resolve_llm_model(model: Option<&str>) -> Result<(Provider, String)> {
    resolve_llm_model_for(None, model)
}

pub fn resolve_llm_model_for(
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<(Provider, String)> {
    let provider = resolve_provider(provider, model)?;
    let effective_model = model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| configured_default_model_for(provider));
    Ok((provider, effective_model))
}

pub fn create_llm_provider(model: Option<&str>) -> Result<Arc<dyn Backend>> {
    create_llm_provider_for(None, model)
}

pub fn create_llm_provider_for(
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<Arc<dyn Backend>> {
    let (provider, effective_model) = resolve_llm_model_for(provider, model)?;
    let llm_provider: Arc<dyn Backend> = match provider {
        Provider::Anthropic => Arc::new(AnthropicBackend::new(&effective_model)?),
        other => Arc::new(OpenAICompatBackend::new(other, &effective_model)?),
    };
    Ok(llm_provider)
}

pub fn create_llm_provider_for_task(
    provider: Option<&str>,
    model: Option<&str>,
    task_id: &str,
) -> Result<Arc<dyn Backend>> {
    let (provider, effective_model) = resolve_llm_model_for(provider, model)?;
    let llm_provider: Arc<dyn Backend> = match provider {
        Provider::Anthropic => Arc::new(AnthropicBackend::new(&effective_model)?),
        other => Arc::new(OpenAICompatBackend::new_for_task(
            other,
            &effective_model,
            Some(task_id),
        )?),
    };
    Ok(llm_provider)
}

pub fn ensure_llm_provider_configured(model: Option<&str>) -> Result<Provider> {
    ensure_llm_provider_configured_for(None, model)
}

pub fn ensure_llm_provider_configured_for(
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<Provider> {
    let provider = resolve_provider(provider, model)?;
    if load_provider_credential(provider).is_none() {
        let cfg = config_for(provider);
        if provider == Provider::OpenAI {
            anyhow::bail!(
                "missing OpenAI credential — set OPENAI_API_KEY, save an OpenAI API key in socai, or run `codex login`."
            );
        } else {
            anyhow::bail!(
                "missing API key for {} — set {} in your environment or via the CLI before running.",
                cfg.display_name,
                cfg.env_keys.join(" or ")
            );
        }
    }
    Ok(provider)
}

#[derive(Debug, Clone)]
pub struct AgentRunConfig {
    pub max_steps: u32,
    pub max_tokens: u32,
    pub compact_after_messages: usize,
    pub keep_recent_messages: usize,
    pub extra_instructions: String,
    pub enabled_sites: Vec<String>,
    pub run_dir: Option<PathBuf>,
    /// Prior chat-level messages to continue an ongoing conversation from.
    pub seed_messages: Vec<Message>,
    pub session_id: Option<String>,
    pub background_media_generation: Option<u64>,
    pub billing_task_id: Option<String>,
}

impl Default for AgentRunConfig {
    fn default() -> Self {
        Self {
            max_steps: 30,
            // Thinking tokens count against max_tokens on Anthropic thinking
            // models (Sonnet 5 thinks by default), so 4096 starves the final
            // report. 16000 is the recommended non-streaming ceiling.
            max_tokens: 16000,
            compact_after_messages: crate::agent::memory::DEFAULT_COMPACT_AFTER_MESSAGES,
            keep_recent_messages: crate::agent::memory::DEFAULT_KEEP_RECENT_MESSAGES,
            extra_instructions: String::new(),
            enabled_sites: Vec::new(),
            run_dir: None,
            seed_messages: Vec::new(),
            session_id: None,
            background_media_generation: None,
            billing_task_id: None,
        }
    }
}

pub async fn run_agent_task(
    task: &str,
    llm_provider: Arc<dyn Backend>,
    tools: Vec<Arc<dyn Tool>>,
    config: AgentRunConfig,
    events_tx: broadcast::Sender<AgentEvent>,
) -> Result<AgentOutcome> {
    let task = task.trim();
    if task.is_empty() {
        anyhow::bail!("task is empty");
    }
    let options = AgentOptions {
        max_steps: config.max_steps,
        max_tokens: config.max_tokens,
        extra_instructions: config.extra_instructions,
        run_dir: config.run_dir,
        enabled_sites: config.enabled_sites,
        compact_after_messages: config.compact_after_messages,
        keep_recent_messages: config.keep_recent_messages,
        seed_messages: config.seed_messages,
        session_id: config.session_id,
        background_media_generation: config.background_media_generation,
        billing_task_id: config.billing_task_id,
    };
    run_agent_with_events(task, llm_provider, tools, options, events_tx).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admission(max_runs: usize) -> Arc<BrowserAdmission> {
        Arc::new(BrowserAdmission {
            max_runs,
            leases: AtomicUsize::new(0),
            gate: Mutex::new(()),
            cooldown_until: std::sync::Mutex::new(None),
            draining: AtomicBool::new(false),
        })
    }

    #[test]
    fn browser_admission_caps_concurrent_runs_and_frees_the_slot_on_drop() {
        let admission = admission(3);
        let first = admission.try_acquire().expect("first run admitted");
        let second = admission.try_acquire().expect("second run admitted");
        let third = admission.try_acquire().expect("third run admitted");

        let refused = admission.try_acquire().expect_err("fourth run refused");
        assert_eq!(refused.kind, BrowserBusyKind::Capacity);

        drop(first);
        admission.try_acquire().expect("slot freed by the drop");
        drop((second, third));
    }

    #[test]
    fn browser_admission_is_exclusive_only_for_a_lone_run() {
        let admission = admission(3);
        let first = admission.try_acquire().expect("first run admitted");
        assert!(admission.is_exclusive());

        let second = admission.try_acquire().expect("second run admitted");
        assert!(!admission.is_exclusive());

        drop(second);
        assert!(admission.is_exclusive());
        drop(first);
    }

    #[test]
    fn draining_admits_one_run_at_a_time_so_the_browser_can_be_reopened() {
        let admission = admission(3);
        let running = admission.try_acquire().expect("run in flight");
        admission.begin_draining();

        // Nothing may join the run that still holds the old browser…
        let refused = admission
            .try_acquire()
            .expect_err("no second run while draining");
        assert_eq!(refused.kind, BrowserBusyKind::Capacity);

        // …but once it finishes, the next run gets in alone and can swap.
        drop(running);
        let swapper = admission.try_acquire().expect("lone run admitted");
        assert!(admission.is_exclusive());
        assert!(admission.try_acquire().is_err());

        admission.finish_draining();
        admission.try_acquire().expect("normal admission resumes");
        drop(swapper);
    }

    #[test]
    fn a_failed_connect_backs_every_run_off_until_the_cooldown_clears() {
        let admission = admission(3);
        admission.start_cooldown();

        let refused = admission
            .try_acquire()
            .expect_err("held off by the backoff");
        assert_eq!(refused.kind, BrowserBusyKind::Connect);
        assert!(refused.retry_after <= REMOTE_MINT_COOLDOWN);

        admission.clear_cooldown();
        admission
            .try_acquire()
            .expect("admitted once the backoff clears");
    }

    #[test]
    fn a_spent_daily_quota_is_reported_rather_than_retried() {
        assert!(!connect_failure_is_retryable(
            "remote browser session request failed (429): daily remote browser time limit reached"
        ));
        assert!(!connect_failure_is_retryable(
            "socai remote browser requires socai pro — run `socai pro activate <invite_code>`"
        ));
        assert!(connect_failure_is_retryable(
            "CDP disconnected: browser websocket wss://…/ did not respond within 20s"
        ));
    }

    #[test]
    fn browser_busy_survives_an_anyhow_chain() {
        let error: anyhow::Error =
            BrowserBusy::capacity("all slots in use", BROWSER_BUSY_RETRY).into();
        let wrapped = error.context("failed to open the tab for this run");

        let busy = BrowserBusy::find(&wrapped).expect("marker recovered from the chain");
        assert_eq!(busy.kind, BrowserBusyKind::Capacity);
        assert_eq!(busy.retry_after, BROWSER_BUSY_RETRY);
    }

    #[test]
    fn resolve_llm_model_for_prefers_explicit_provider_over_model_prefix() {
        let (provider, model) = resolve_llm_model_for(Some("qwen"), Some("custom-model")).unwrap();

        assert_eq!(provider, Provider::Qwen);
        assert_eq!(model, "custom-model");
    }
}
