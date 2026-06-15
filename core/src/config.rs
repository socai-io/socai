use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::cdp::{ChromeConnectOptions, ChromeProfile};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SocaiConfig {
    #[serde(default)]
    pub chrome: ChromeConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChromeConfig {
    /// Browser profile preference. Missing means the product default:
    /// attach to the user's existing chrome profile.
    #[serde(default)]
    pub profile: Option<ChromeProfile>,
    /// Optional managed chrome user-data-dir. Used only when `profile` is
    /// `managed` or `auto`.
    #[serde(default)]
    pub profile_dir: Option<String>,
}

impl SocaiConfig {
    pub fn chrome_connect_options(&self) -> ChromeConnectOptions {
        let profile = self.chrome.profile.unwrap_or_default();
        let managed_user_data_dir = match profile {
            ChromeProfile::Managed | ChromeProfile::Auto => self
                .chrome
                .profile_dir
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(expand_tilde_path),
            ChromeProfile::Existing => None,
        };
        ChromeConnectOptions {
            profile,
            managed_user_data_dir,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ConfigKey {
    ChromeProfile,
    ChromeProfileDir,
}

impl ConfigKey {
    fn parse(key: &str) -> Result<Self> {
        match key.trim() {
            "chrome.profile" => Ok(Self::ChromeProfile),
            "chrome.profile_dir" | "chrome.profile-dir" => Ok(Self::ChromeProfileDir),
            other => anyhow::bail!(
                "unknown config key {other:?}; supported keys: chrome.profile, chrome.profile_dir"
            ),
        }
    }

    fn path(self) -> &'static [&'static str] {
        match self {
            Self::ChromeProfile => &["chrome", "profile"],
            Self::ChromeProfileDir => &["chrome", "profile_dir"],
        }
    }

    fn display(self) -> &'static str {
        match self {
            Self::ChromeProfile => "chrome.profile",
            Self::ChromeProfileDir => "chrome.profile_dir",
        }
    }
}

pub fn config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not resolve $HOME for ~/.socai/config.json")?;
    Ok(home.join(".socai/config.json"))
}

pub fn load_config() -> Result<SocaiConfig> {
    let value = load_config_value()?;
    serde_json::from_value(value).context("failed to parse ~/.socai/config.json")
}

pub fn load_config_value() -> Result<Value> {
    let path = config_path()?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Value::Object(Map::new()));
        }
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };
    let value: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if !value.is_object() {
        anyhow::bail!("{} must contain a JSON object", path.display());
    }
    Ok(value)
}

pub fn save_config_value(value: &Value) -> Result<PathBuf> {
    if !value.is_object() {
        anyhow::bail!("socai config root must be a JSON object");
    }
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut rendered = serde_json::to_string_pretty(value)?;
    rendered.push('\n');
    fs::write(&path, rendered).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

pub fn get_config_key(key: &str) -> Result<Option<Value>> {
    let key = ConfigKey::parse(key)?;
    let value = load_config_value()?;
    Ok(get_path(&value, key.path()).cloned())
}

pub fn set_config_key(key: &str, raw_value: &str) -> Result<PathBuf> {
    let key = ConfigKey::parse(key)?;
    let value = parse_key_value(key, raw_value)?;
    let mut config = load_config_value()?;
    set_path(&mut config, key.path(), value)?;
    save_config_value(&config)
}

pub fn unset_config_key(key: &str) -> Result<PathBuf> {
    let key = ConfigKey::parse(key)?;
    let mut config = load_config_value()?;
    unset_path(&mut config, key.path());
    prune_empty_objects(&mut config);
    save_config_value(&config)
}

pub fn canonical_config_key(key: &str) -> Result<&'static str> {
    Ok(ConfigKey::parse(key)?.display())
}

fn parse_key_value(key: ConfigKey, raw_value: &str) -> Result<Value> {
    let value = raw_value.trim();
    if value.is_empty() {
        anyhow::bail!(
            "config value cannot be empty; use `socai config unset {}` to clear it",
            key.display()
        );
    }
    match key {
        ConfigKey::ChromeProfile => Ok(Value::String(ChromeProfile::parse(value)?.as_str().into())),
        ConfigKey::ChromeProfileDir => Ok(Value::String(value.to_string())),
    }
}

fn get_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for part in path {
        current = current.get(*part)?;
    }
    Some(current)
}

fn set_path(value: &mut Value, path: &[&str], new_value: Value) -> Result<()> {
    if path.is_empty() {
        *value = new_value;
        return Ok(());
    }
    let mut current = value;
    for part in &path[..path.len() - 1] {
        if !current.is_object() {
            *current = Value::Object(Map::new());
        }
        let object = current.as_object_mut().expect("object just created");
        current = object
            .entry((*part).to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    if !current.is_object() {
        *current = Value::Object(Map::new());
    }
    current
        .as_object_mut()
        .expect("object just created")
        .insert(path[path.len() - 1].to_string(), new_value);
    Ok(())
}

fn unset_path(value: &mut Value, path: &[&str]) -> bool {
    let Some((leaf, parents)) = path.split_last() else {
        return false;
    };
    let mut current = value;
    for part in parents {
        let Some(next) = current.get_mut(*part) else {
            return false;
        };
        current = next;
    }
    current
        .as_object_mut()
        .and_then(|object| object.remove(*leaf))
        .is_some()
}

fn prune_empty_objects(value: &mut Value) -> bool {
    let Some(object) = value.as_object_mut() else {
        return false;
    };
    object.retain(|_, child| !prune_empty_objects(child));
    object.is_empty()
}

fn expand_tilde_path(value: &str) -> PathBuf {
    if value == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_connect_options_default_to_existing() {
        let options = SocaiConfig::default().chrome_connect_options();
        assert_eq!(options.profile, ChromeProfile::Existing);
        assert_eq!(options.managed_user_data_dir, None);
    }

    #[test]
    fn chrome_connect_options_use_managed_profile_dir_only_for_managed_modes() {
        let mut config = SocaiConfig::default();
        config.chrome.profile = Some(ChromeProfile::Managed);
        config.chrome.profile_dir = Some("/tmp/socai-profile".into());
        let options = config.chrome_connect_options();
        assert_eq!(options.profile, ChromeProfile::Managed);
        assert_eq!(
            options.managed_user_data_dir,
            Some(PathBuf::from("/tmp/socai-profile"))
        );

        config.chrome.profile = Some(ChromeProfile::Existing);
        let options = config.chrome_connect_options();
        assert_eq!(options.profile, ChromeProfile::Existing);
        assert_eq!(options.managed_user_data_dir, None);
    }

    #[test]
    fn parse_chrome_profile_config_is_strict() {
        assert!(parse_key_value(ConfigKey::ChromeProfile, "managed").is_ok());
        assert!(parse_key_value(ConfigKey::ChromeProfile, "isolated").is_err());
    }
}
