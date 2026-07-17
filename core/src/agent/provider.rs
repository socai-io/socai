//! LLM provider catalog + API key resolution.
//!
//! Credential precedence:
//! 1. environment variables (`ANTHROPIC_API_KEY`, etc.)
//! 2. `~/.socai/auth.json` — `{provider: {api_key: ...}}`
//! 3. OpenAI only: Codex CLI auth in `~/.codex/auth.json`

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    Anthropic,
    OpenAI,
    Kimi,
    /// Qwen via mainland DashScope (dashscope.aliyuncs.com). DashScope api
    /// keys are region-bound, so the international deployment is a separate
    /// provider with its own key rather than a toggle on this one.
    Qwen,
    /// Qwen via international DashScope (dashscope-intl.aliyuncs.com),
    /// keyed from modelstudio.console.alibabacloud.com.
    QwenIntl,
    /// Doubao via its OpenAI-compatible API.
    Doubao,
    DeepSeek,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenAI => "openai",
            Provider::Kimi => "kimi",
            Provider::Qwen => "qwen",
            Provider::QwenIntl => "qwen-intl",
            Provider::Doubao => "doubao",
            Provider::DeepSeek => "deepseek",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "anthropic" | "claude" => Some(Self::Anthropic),
            "openai" | "gpt" => Some(Self::OpenAI),
            "kimi" | "moonshot" => Some(Self::Kimi),
            "qwen" | "dashscope" => Some(Self::Qwen),
            "qwen-intl" | "qwen_intl" | "dashscope-intl" => Some(Self::QwenIntl),
            "doubao" => Some(Self::Doubao),
            "deepseek" => Some(Self::DeepSeek),
            _ => None,
        }
    }

    /// Key into the generated model catalog. Qwen International serves the
    /// same model lineup as mainland Qwen, so both share the "qwen" entry.
    fn catalog_key(self) -> &'static str {
        match self {
            Provider::QwenIntl => Provider::Qwen.as_str(),
            other => other.as_str(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub provider: Provider,
    pub display_name: &'static str,
    pub default_model: &'static str,
    /// Env var names tried in order.
    pub env_keys: &'static [&'static str],
    /// Used for OpenAI-compatible providers; None for the Anthropic native
    /// endpoint (which lives at api.anthropic.com).
    pub base_url: Option<&'static str>,
    /// Model id prefix → provider matching (e.g. "claude-" → Anthropic).
    pub model_prefixes: &'static [&'static str],
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelCatalogEntry {
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub recommended: bool,
    #[serde(default)]
    pub pricing: Option<ModelPricing>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub currency: String,
    pub source: String,
    pub tiers: Vec<ModelPricingTier>,
    #[serde(default)]
    pub overrides: HashMap<String, ModelPricing>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelPricingTier {
    #[serde(default)]
    pub max_input_tokens: Option<u64>,
    pub input_per_million: f64,
    pub output_per_million: f64,
    #[serde(default)]
    pub cache_read_per_million: f64,
    #[serde(default)]
    pub cache_write_per_million: f64,
}

impl ModelCatalogEntry {
    pub fn label(&self) -> &str {
        self.display_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.id)
    }

    fn fallback(id: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            display_name: Some(display_name.into()),
            source: Some("compiled-default".into()),
            recommended: true,
            pricing: None,
        }
    }
}

impl ModelPricing {
    pub fn for_provider(&self, provider: Provider) -> &Self {
        self.overrides.get(provider.as_str()).unwrap_or(self)
    }

    pub fn tier_for_input(&self, input_tokens: u64) -> Option<&ModelPricingTier> {
        self.tiers
            .iter()
            .find(|tier| {
                tier.max_input_tokens
                    .is_none_or(|limit| input_tokens <= limit)
            })
            .or_else(|| self.tiers.last())
    }
}

#[derive(Debug, Deserialize)]
struct ModelCatalogFile {
    providers: HashMap<String, Vec<ModelCatalogEntry>>,
}

static MODEL_CATALOG: OnceLock<Option<ModelCatalogFile>> = OnceLock::new();

fn generated_model_catalog() -> Option<&'static ModelCatalogFile> {
    MODEL_CATALOG
        .get_or_init(|| serde_json::from_str(include_str!("model_catalog.generated.json")).ok())
        .as_ref()
}

#[derive(Debug, Clone)]
pub enum Credential {
    ApiKey(String),
    CodexOAuth {
        access_token: String,
        account_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    ApiKey,
    CodexOAuth,
}

/// Static table. Order is the preference order when no provider is
/// explicitly chosen.
pub static PROVIDERS: &[ProviderConfig] = &[
    ProviderConfig {
        provider: Provider::Anthropic,
        display_name: "Anthropic",
        default_model: "claude-sonnet-5",
        env_keys: &["ANTHROPIC_API_KEY"],
        base_url: None,
        model_prefixes: &["claude-"],
    },
    ProviderConfig {
        provider: Provider::OpenAI,
        display_name: "OpenAI",
        default_model: "gpt-5.5",
        env_keys: &["OPENAI_API_KEY"],
        base_url: Some("https://api.openai.com/v1"),
        model_prefixes: &["gpt-", "o1", "o3", "o4", "chatgpt-"],
    },
    ProviderConfig {
        provider: Provider::Kimi,
        display_name: "Kimi",
        default_model: "kimi-k2.6",
        env_keys: &["KIMI_API_KEY", "MOONSHOT_API_KEY"],
        base_url: Some("https://api.moonshot.cn/v1"),
        model_prefixes: &["kimi-", "moonshot-"],
    },
    ProviderConfig {
        provider: Provider::Qwen,
        display_name: "Qwen",
        default_model: "qwen3.7-plus",
        env_keys: &["QWEN_API_KEY", "DASHSCOPE_API_KEY"],
        base_url: Some("https://dashscope.aliyuncs.com/compatible-mode/v1"),
        model_prefixes: &["qwen", "qwq-", "qvq-"],
    },
    ProviderConfig {
        provider: Provider::QwenIntl,
        display_name: "Qwen International",
        default_model: "qwen3.7-plus",
        env_keys: &["QWEN_INTL_API_KEY", "DASHSCOPE_INTL_API_KEY"],
        base_url: Some("https://dashscope-intl.aliyuncs.com/compatible-mode/v1"),
        // Model ids are identical to mainland Qwen, so prefix inference
        // intentionally resolves to the mainland entry; the international
        // deployment is reached by explicit provider selection.
        model_prefixes: &[],
    },
    ProviderConfig {
        provider: Provider::Doubao,
        display_name: "Doubao",
        default_model: "doubao-seed-2-1-turbo-260628",
        env_keys: &["DOUBAO_API_KEY"],
        base_url: Some("https://ark.cn-beijing.volces.com/api/v3"),
        model_prefixes: &["doubao-"],
    },
    ProviderConfig {
        provider: Provider::DeepSeek,
        display_name: "DeepSeek",
        default_model: "deepseek-v4-pro",
        env_keys: &["DEEPSEEK_API_KEY"],
        base_url: Some("https://api.deepseek.com/v1"),
        model_prefixes: &["deepseek-"],
    },
];

pub fn config_for(provider: Provider) -> &'static ProviderConfig {
    PROVIDERS
        .iter()
        .find(|c| c.provider == provider)
        // safe: PROVIDERS covers every Provider variant by construction.
        .unwrap_or(&PROVIDERS[0])
}

pub fn default_model_for(provider: Provider) -> &'static str {
    config_for(provider).default_model
}

/// Curated model versions for a provider, loaded from the generated model
/// catalog. The catalog is refreshed by maintainer tooling, not at app runtime.
/// The provider's compiled default is always present so persisted/default model
/// selection cannot point at an invisible row after a stale catalog sync.
pub fn catalog_models_for(provider: Provider) -> Vec<ModelCatalogEntry> {
    let cfg = config_for(provider);
    let mut models = generated_model_catalog()
        .and_then(|catalog| catalog.providers.get(cfg.provider.catalog_key()))
        .cloned()
        .unwrap_or_default();

    models.retain(|model| !model.id.trim().is_empty());
    if models.is_empty() {
        models.push(ModelCatalogEntry::fallback(
            cfg.default_model,
            cfg.display_name,
        ));
        return models;
    }

    if !models.iter().any(|model| model.id == cfg.default_model) {
        models.insert(
            0,
            ModelCatalogEntry::fallback(cfg.default_model, cfg.display_name),
        );
    }
    models
}

pub fn catalog_model_display_name(provider: Provider, model_id: &str) -> String {
    let trimmed = model_id.trim();
    if trimmed.is_empty() {
        return config_for(provider).display_name.to_string();
    }
    catalog_models_for(provider)
        .into_iter()
        .find(|model| model.id == trimmed)
        .map(|model| model.label().to_string())
        .unwrap_or_else(|| trimmed.to_string())
}

pub fn catalog_model_pricing(provider: Provider, model_id: &str) -> Option<ModelPricing> {
    catalog_models_for(provider)
        .into_iter()
        .find(|model| model.id == model_id.trim())
        .and_then(|model| model.pricing)
        .map(|pricing| pricing.for_provider(provider).clone())
}

#[derive(Debug, Deserialize)]
struct AuthFile {
    #[serde(flatten)]
    providers: HashMap<String, Value>,
}

fn read_auth_file(path: &PathBuf) -> HashMap<String, Value> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return HashMap::new(),
    };
    match serde_json::from_slice::<AuthFile>(&bytes) {
        Ok(f) => f.providers,
        Err(_) => HashMap::new(),
    }
}

fn auth_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".socai/auth.json"));
    }
    paths
}

fn codex_auth_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".codex/auth.json"))
}

/// Re-reads on every call. The file is tiny, and not caching means
/// `save_api_key` is visible to the same process.
fn auth_blobs() -> Vec<(PathBuf, HashMap<String, Value>)> {
    auth_paths()
        .into_iter()
        .map(|p| {
            let blob = read_auth_file(&p);
            (p, blob)
        })
        .collect()
}

/// Load an API key for the given provider. Searches env vars first, then
/// each `auth.json` in `auth_paths()` order.
pub fn load_api_key(provider: Provider) -> Option<String> {
    let cfg = config_for(provider);
    for env in cfg.env_keys {
        if let Ok(value) = std::env::var(env) {
            let trimmed = value.trim().to_string();
            if trimmed.len() >= 8 {
                return Some(trimmed);
            }
        }
    }
    let name = provider.as_str();
    for (_path, blob) in &auth_blobs() {
        if let Some(block) = blob.get(name).and_then(Value::as_object) {
            if let Some(key) = block.get("api_key").and_then(Value::as_str) {
                let trimmed = key.trim().to_string();
                if trimmed.len() >= 8 {
                    return Some(trimmed);
                }
            }
        }
    }
    None
}

pub fn provider_has_key(provider: Provider) -> bool {
    load_provider_credential(provider).is_some()
}

pub fn provider_credential_kind(provider: Provider) -> Option<CredentialKind> {
    match load_provider_credential(provider) {
        Some(Credential::ApiKey(_)) => Some(CredentialKind::ApiKey),
        Some(Credential::CodexOAuth { .. }) => Some(CredentialKind::CodexOAuth),
        None => None,
    }
}

pub fn load_provider_credential(provider: Provider) -> Option<Credential> {
    if provider == Provider::OpenAI {
        return load_openai_credential();
    }
    load_api_key(provider).map(Credential::ApiKey)
}

pub fn load_openai_credential() -> Option<Credential> {
    if let Some(key) = load_api_key(Provider::OpenAI) {
        return Some(Credential::ApiKey(key));
    }
    load_codex_oauth_credential()
}

#[derive(Debug, Deserialize)]
struct CodexAuthFile {
    #[serde(rename = "OPENAI_API_KEY")]
    openai_api_key: Option<String>,
    tokens: Option<CodexTokenData>,
}

#[derive(Debug, Deserialize)]
struct CodexTokenData {
    access_token: String,
    account_id: Option<String>,
}

fn load_codex_auth_file() -> Option<(PathBuf, CodexAuthFile)> {
    let path = codex_auth_path()?;
    let bytes = std::fs::read(&path).ok()?;
    let auth = serde_json::from_slice::<CodexAuthFile>(&bytes).ok()?;
    Some((path, auth))
}

fn load_codex_oauth_credential() -> Option<Credential> {
    let (_path, auth) = load_codex_auth_file()?;
    if let Some(api_key) = auth.openai_api_key {
        let trimmed = api_key.trim().to_string();
        if trimmed.len() >= 8 {
            return Some(Credential::ApiKey(trimmed));
        }
    }
    let tokens = auth.tokens?;
    let access_token = tokens.access_token.trim().to_string();
    let account_id = tokens.account_id?.trim().to_string();
    if access_token.len() < 8 || account_id.is_empty() {
        return None;
    }
    Some(Credential::CodexOAuth {
        access_token,
        account_id,
    })
}

/// Persist an API key to `~/.socai/auth.json`: refuses keys shorter than 8
/// chars, also sets `defaults.provider`, and chmods the file to 0600. Returns
/// the path.
pub fn save_api_key(provider: Provider, api_key: &str) -> anyhow::Result<std::path::PathBuf> {
    let trimmed = api_key.trim();
    if trimmed.len() < 8 {
        anyhow::bail!(
            "API key looks too short ({} chars). Paste the full key.",
            trimmed.len()
        );
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not resolve $HOME"))?;
    let path = home.join(".socai/auth.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut data: serde_json::Map<String, Value> = match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(map)) => map,
            _ => serde_json::Map::new(),
        },
        Err(_) => serde_json::Map::new(),
    };

    let name = provider.as_str();
    let mut block = data
        .get(name)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    block.insert("api_key".into(), Value::String(trimmed.to_string()));
    data.insert(name.into(), Value::Object(block));

    let mut defaults = data
        .get("defaults")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    defaults.insert("provider".into(), Value::String(name.into()));
    data.insert("defaults".into(), Value::Object(defaults));

    let rendered = serde_json::to_string_pretty(&Value::Object(data))?;
    std::fs::write(&path, rendered)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&path, perms);
    }
    Ok(path)
}

/// Persist `provider` + `model` as the new defaults in
/// `~/.socai/auth.json`. Does not touch the api_key block — used by the
/// TUI's `/model` command to remember the selection across runs.
/// Uses `defaults.provider` + `defaults.{provider}_model` so all Rust
/// entrypoints share one config file.
pub fn save_default_model(provider: Provider, model: &str) -> anyhow::Result<std::path::PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not resolve $HOME"))?;
    let path = home.join(".socai/auth.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut data: serde_json::Map<String, Value> = match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(map)) => map,
            _ => serde_json::Map::new(),
        },
        Err(_) => serde_json::Map::new(),
    };

    let name = provider.as_str();
    let mut defaults = data
        .get("defaults")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    defaults.insert("provider".into(), Value::String(name.into()));
    let trimmed = model.trim();
    if !trimmed.is_empty() {
        defaults.insert(format!("{name}_model"), Value::String(trimmed.to_string()));
    }
    data.insert("defaults".into(), Value::Object(defaults));

    let rendered = serde_json::to_string_pretty(&Value::Object(data))?;
    std::fs::write(&path, rendered)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

/// Default provider saved in `~/.socai/auth.json`, if present. Unlike
/// `resolve_provider`, this intentionally ignores credential availability so
/// UI pickers can restore a provider that still needs an API key.
pub fn configured_default_provider() -> Option<Provider> {
    for (_path, blob) in &auth_blobs() {
        if let Some(defaults) = blob.get("defaults").and_then(Value::as_object) {
            if let Some(name) = defaults.get("provider").and_then(Value::as_str) {
                if let Some(provider) = Provider::from_name(name) {
                    return Some(provider);
                }
            }
        }
    }
    None
}

/// Default model for `provider`, honoring `defaults.{provider}_model`
/// in any auth blob. Falls back to the compiled-in static default.
pub fn configured_default_model_for(provider: Provider) -> String {
    let key = format!("{}_model", provider.as_str());
    for (_path, blob) in &auth_blobs() {
        if let Some(defaults) = blob.get("defaults").and_then(Value::as_object) {
            if let Some(model) = defaults.get(&key).and_then(Value::as_str) {
                let trimmed = model.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
        }
    }
    default_model_for(provider).to_string()
}

/// Providers with a usable key on disk, in PROVIDERS order.
pub fn list_available_providers() -> Vec<Provider> {
    PROVIDERS
        .iter()
        .map(|c| c.provider)
        .filter(|p| provider_has_key(*p))
        .collect()
}

/// Pick a provider:
/// - explicit `provider` arg wins
/// - else `SOCAI_LLM_PROVIDER`
/// - else infer from `model` prefix
/// - else `SOCAI_MODEL` prefix
/// - else `defaults.provider` from auth.json, if it has a key
/// - else first PROVIDERS entry with a key
/// - else first PROVIDERS entry
pub fn resolve_provider(provider: Option<&str>, model: Option<&str>) -> anyhow::Result<Provider> {
    if let Some(name) = provider {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return Provider::from_name(trimmed)
                .ok_or_else(|| anyhow::anyhow!("unknown provider: {trimmed:?}"));
        }
    }
    if let Ok(env) = std::env::var("SOCAI_LLM_PROVIDER") {
        let trimmed = env.trim();
        if !trimmed.is_empty() {
            return Provider::from_name(trimmed)
                .ok_or_else(|| anyhow::anyhow!("unknown SOCAI_LLM_PROVIDER: {trimmed:?}"));
        }
    }
    let model_str = model
        .map(str::to_string)
        .or_else(|| std::env::var("SOCAI_MODEL").ok())
        .unwrap_or_default();
    let lower = model_str.trim().to_ascii_lowercase();
    if !lower.is_empty() {
        for cfg in PROVIDERS {
            if cfg.model_prefixes.iter().any(|p| lower.starts_with(p)) {
                return Ok(cfg.provider);
            }
        }
    }
    // defaults.provider in any auth blob
    for (_path, blob) in &auth_blobs() {
        if let Some(defaults) = blob.get("defaults").and_then(Value::as_object) {
            if let Some(name) = defaults.get("provider").and_then(Value::as_str) {
                if let Some(p) = Provider::from_name(name) {
                    if provider_has_key(p) {
                        return Ok(p);
                    }
                }
            }
        }
    }
    if let Some(p) = list_available_providers().into_iter().next() {
        return Ok(p);
    }
    Ok(Provider::OpenAI)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_name_aliases() {
        assert_eq!(Provider::from_name("claude"), Some(Provider::Anthropic));
        assert_eq!(Provider::from_name("MOONSHOT"), Some(Provider::Kimi));
        assert_eq!(Provider::from_name("DashScope"), Some(Provider::Qwen));
        assert_eq!(Provider::from_name("qwen-intl"), Some(Provider::QwenIntl));
        assert_eq!(Provider::from_name("doubao"), Some(Provider::Doubao));
        assert_eq!(Provider::from_name("DeepSeek"), Some(Provider::DeepSeek));
        assert_eq!(Provider::from_name("nope"), None);
    }

    #[test]
    fn config_for_each_variant() {
        for p in [
            Provider::Anthropic,
            Provider::OpenAI,
            Provider::Kimi,
            Provider::Qwen,
            Provider::QwenIntl,
            Provider::Doubao,
            Provider::DeepSeek,
        ] {
            let cfg = config_for(p);
            assert_eq!(cfg.provider, p);
            assert!(!cfg.default_model.is_empty());
        }
    }
}
