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

const CONNECT_TIMEOUT: Duration = Duration::from_secs(90);

/// Shared in-process runtime handle for one entrypoint. Tauri, TUI, and the
/// CLI daemon each construct their own instance; the daemon is only an IPC
/// wrapper around this same object graph.
#[derive(Clone)]
pub struct SocaiRuntime {
    cdp: Cdp,
    site_pages: Arc<Mutex<HashMap<String, Arc<PageSession>>>>,
}

impl SocaiRuntime {
    pub fn new() -> Self {
        Self {
            cdp: Cdp::new(),
            site_pages: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn browser(&self) -> Cdp {
        self.cdp.clone()
    }

    pub fn subscribe_browser_events(&self) -> broadcast::Receiver<BrowserEvent> {
        self.cdp.subscribe()
    }

    pub fn connect_browser(&self) {
        self.cdp.connect();
    }

    pub fn connect_browser_with_options(&self, options: ChromeConnectOptions) {
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

        if let Some(options) = options.as_ref() {
            if !browser_status_matches_options(&self.browser_status().await, options) {
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
        user_data_dir,
        ..
    } = status
    else {
        return true;
    };

    match options.profile {
        ChromeProfile::Existing => !managed,
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
        ChromeProfile::Auto => true,
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
    pub max_turns: u32,
    pub max_tokens: u32,
    pub keep_recent_messages: usize,
    pub memory_max_chars: usize,
    pub extra_instructions: String,
    pub enabled_sites: Vec<String>,
    pub run_dir: Option<PathBuf>,
    /// Prior chat-level messages to continue a multi-turn session from.
    pub seed_messages: Vec<Message>,
}

impl Default for AgentRunConfig {
    fn default() -> Self {
        Self {
            max_turns: 30,
            max_tokens: 4096,
            keep_recent_messages: 12,
            memory_max_chars: 6000,
            extra_instructions: String::new(),
            enabled_sites: Vec::new(),
            run_dir: None,
            seed_messages: Vec::new(),
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
        max_turns: config.max_turns,
        max_tokens: config.max_tokens,
        extra_instructions: config.extra_instructions,
        run_dir: config.run_dir,
        enabled_sites: config.enabled_sites,
        keep_recent_messages: config.keep_recent_messages,
        memory_max_chars: config.memory_max_chars,
        seed_messages: config.seed_messages,
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
