//! Desktop-only, one-time eligibility snapshot. CLI credentials remain shared,
//! but adding one after migration must not unlock the desktop's legacy UI.

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use socai_core::agent::{Provider, PROVIDERS};
use std::sync::OnceLock;

#[derive(Deserialize, Serialize)]
struct Eligibility {
    enabled: bool,
}

pub fn enabled() -> Result<bool, String> {
    static ELIGIBILITY: OnceLock<Result<bool, String>> = OnceLock::new();
    ELIGIBILITY
        .get_or_init(|| load_or_migrate().map_err(|err| format!("{err:#}")))
        .clone()
}

pub fn require() -> Result<(), String> {
    if enabled()? {
        Ok(())
    } else {
        Err("API key configuration is only available to legacy desktop users. Use the Socai managed model.".into())
    }
}

fn load_or_migrate() -> anyhow::Result<bool> {
    // Use the same home as save_api_key, independent of SOCAI_HOME. No keys or
    // account identifiers are copied into the migration file.
    let dir = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("could not resolve home directory"))?
        .join(".socai");
    std::fs::create_dir_all(&dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("desktop-byok.lock"))?;
    lock.lock_exclusive()?;
    let path = dir.join("desktop-byok.json");
    match std::fs::read(&path) {
        Ok(bytes) => return Ok(serde_json::from_slice::<Eligibility>(&bytes)?.enabled),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    let auth: serde_json::Value = match std::fs::read(dir.join("auth.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => serde_json::Value::Null,
        Err(err) => return Err(err.into()),
    };
    // Implicit Codex login and environment variables do not mean this user
    // previously added an API key in Socai.
    let enabled = PROVIDERS.iter().any(|cfg| {
        cfg.provider != Provider::Socai
            && auth
                .get(cfg.provider.as_str())
                .and_then(|block| block.get("api_key"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|key| key.trim().len() >= 8)
    });
    let mut file = tempfile::NamedTempFile::new_in(&dir)?;
    serde_json::to_writer(file.as_file_mut(), &Eligibility { enabled })?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(enabled)
}
