use std::collections::HashMap;
use std::path::PathBuf;
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
/// connect latency for not dying mid-run.
const REMOTE_MIN_RUN_BUDGET: Duration = Duration::from_secs(300);

/// Work-in-flight signal for the remote idle reaper. Guards are held by the
/// entrypoints around browser-touching work (a daemon command, a whole agent
/// task), so the reaper never releases a session under an active run — LLM
/// thinking pauses between tool calls included.
struct Activity {
    in_flight: std::sync::atomic::AtomicUsize,
    /// Bumped on every guard acquisition; the reaper re-reads it right before
    /// releasing to shrink the check-to-act race with a just-started run.
    generation: std::sync::atomic::AtomicU64,
    last_done: std::sync::Mutex<std::time::Instant>,
    reaper_started: std::sync::atomic::AtomicBool,
}

impl Activity {
    fn new() -> Self {
        Self {
            in_flight: std::sync::atomic::AtomicUsize::new(0),
            generation: std::sync::atomic::AtomicU64::new(0),
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

/// RAII marker for in-flight browser work; see [`SocaiRuntime::begin_activity`].
pub struct ActivityGuard {
    activity: Arc<Activity>,
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        self.activity.touch();
        self.activity
            .in_flight
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
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
}

impl SocaiRuntime {
    pub fn new() -> Self {
        Self {
            cdp: Cdp::new(),
            site_pages: Arc::new(Mutex::new(HashMap::new())),
            activity: Arc::new(Activity::new()),
        }
    }

    /// Mark browser work as in flight until the returned guard drops. Every
    /// entrypoint wraps its browser-touching unit of work in one of these —
    /// the daemon around each site command, the app and TUI around a whole
    /// agent task — so the remote idle reaper only counts true idle time.
    pub fn begin_activity(&self) -> ActivityGuard {
        self.activity
            .in_flight
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.activity
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        ActivityGuard {
            activity: self.activity.clone(),
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
            loop {
                sleep(REMOTE_IDLE_TICK).await;
                let activity = &runtime.activity;
                if runtime.cdp.remote_session_deadline().await.is_none() {
                    continue;
                }
                if activity.in_flight.load(Ordering::SeqCst) > 0
                    || activity.idle_for() < REMOTE_IDLE_RELEASE
                {
                    continue;
                }
                // Re-read the generation right before acting: a run that
                // started between the checks above and here bumps it, and we
                // must not yank the session out from under that run.
                let generation = activity.generation.load(Ordering::SeqCst);
                let _ = runtime.close_all_site_sessions().await;
                if activity.generation.load(Ordering::SeqCst) != generation
                    || activity.in_flight.load(Ordering::SeqCst) > 0
                {
                    continue;
                }
                tracing::info!(
                    idle_secs = REMOTE_IDLE_RELEASE.as_secs(),
                    "releasing idle remote browser session"
                );
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

    async fn ensure_site_page_inner(
        &self,
        site_id: &str,
        start_url: &str,
        options: Option<ChromeConnectOptions>,
    ) -> Result<Arc<PageSession>> {
        let site_id = site_id.trim();
        if site_id.is_empty() {
            anyhow::bail!("site_id is empty");
        }
        self.ensure_idle_reaper();

        if let Some(options) = options.as_ref() {
            if !browser_status_matches_options(&self.browser_status().await, options) {
                let _ = self.close_all_site_sessions().await;
                self.disconnect_browser().await;
            }
        }

        // A remote session near its server-side end-of-life is re-minted
        // before work starts on it rather than dying mid-run: release now,
        // and the reconnect below mints a session with a full budget.
        if let Some(deadline) = self.cdp.remote_session_deadline().await {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining < REMOTE_MIN_RUN_BUDGET {
                tracing::info!(
                    remaining_secs = remaining.as_secs(),
                    "remote browser session near timeout; re-minting before new work"
                );
                let _ = self.close_all_site_sessions().await;
                self.disconnect_browser().await;
            }
        }

        let mut pages = self.site_pages.lock().await;
        if let Some(page) = pages.get(site_id) {
            if page.page_info().await.is_ok() {
                return Ok(page.clone());
            }
            pages.remove(site_id);
        }

        match options {
            Some(options) => wait_browser_connected_with_options(self, options).await?,
            None => wait_browser_connected(self).await?,
        }
        let page = Arc::new(self.create_page("about:blank").await?);
        if !start_url.trim().is_empty() {
            page.navigate_with_timeout(start_url, 60.0).await?;
        }
        pages.insert(site_id.to_string(), page.clone());
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
    };
    run_agent_with_events(task, llm_provider, tools, options, events_tx).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_llm_model_for_prefers_explicit_provider_over_model_prefix() {
        let (provider, model) = resolve_llm_model_for(Some("qwen"), Some("custom-model")).unwrap();

        assert_eq!(provider, Provider::Qwen);
        assert_eq!(model, "custom-model");
    }
}
