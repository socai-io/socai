//! An installation-scoped anonymous trial, isolated from account credentials.
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
};

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use super::auth::{self, LlmGatewayConfig};

#[derive(Default, Deserialize, Serialize)]
struct Trial {
    secret: String,
    #[serde(default)]
    device_token: String,
    #[serde(default)]
    used: bool,
    #[serde(default)]
    task_ids: Vec<String>,
}

fn directory() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("home directory unavailable")?
        .join(".socai"))
}

fn read_trial() -> Result<Trial> {
    let path = directory()?.join("guest.json");
    if !path.exists() {
        return Ok(Trial::default());
    }
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

fn private_file(path: &std::path::Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

fn save_trial(trial: &Trial) -> Result<()> {
    let dir = directory()?;
    std::fs::create_dir_all(&dir)?;
    let temporary = dir.join("guest.json.tmp");
    let mut file = private_file(&temporary)?;
    file.set_len(0)?;
    file.write_all(&serde_json::to_vec(trial)?)?;
    file.sync_all()?;
    std::fs::rename(temporary, dir.join("guest.json"))?;
    Ok(())
}

/// Held for the entire answer, including every agent/tool/parallel-branch step.
/// A process-wide AND cross-process lock prevents simultaneous free answers.
pub struct GuestTrialGuard {
    _lock: File,
}

impl GuestTrialGuard {
    pub fn bind_task(&self, task_id: &str) -> Result<()> {
        let mut trial = read_trial()?;
        if !trial.task_ids.iter().any(|id| id == task_id) {
            trial.task_ids.push(task_id.into());
        }
        save_trial(&trial)
    }

    pub fn finish(&self, completed: bool) -> Result<()> {
        if completed {
            let mut trial = read_trial()?;
            trial.used = true;
            save_trial(&trial)?;
        }
        Ok(())
    }
}

pub async fn prepare_guest_trial() -> Result<GuestTrialGuard> {
    let dir = directory()?;
    std::fs::create_dir_all(&dir)?;
    let lock = private_file(&dir.join("guest.lock"))?;
    lock.try_lock_exclusive()
        .context("Your free answer is already running")?;
    let mut trial = read_trial()?;
    if trial.used {
        bail!("guest_login_required");
    }
    if trial.secret.is_empty() {
        trial.secret = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        save_trial(&trial)?;
    }
    let base = auth::configured_base_url().context("socai service URL is not configured")?;
    let response = auth::http_client()?
        .post(format!("{base}/v1/auth/guest"))
        .json(&serde_json::json!({"secret": trial.secret}))
        .send()
        .await?;
    #[derive(Deserialize)]
    struct GuestResponse {
        device_token: String,
        used: bool,
    }
    let result: GuestResponse = auth::require_success(response, "free answer")
        .await?
        .json()
        .await?;
    trial.device_token = result.device_token;
    trial.used |= result.used;
    save_trial(&trial)?;
    if trial.used {
        bail!("guest_login_required");
    }
    Ok(GuestTrialGuard { _lock: lock })
}

/// Pin an in-flight guest answer even if the user signs in before it finishes.
pub fn llm_gateway_config_for_task(task_id: Option<&str>) -> Result<LlmGatewayConfig> {
    if let Ok(trial) = read_trial() {
        if !trial.device_token.is_empty()
            && (task_id.is_some_and(|id| trial.task_ids.iter().any(|saved| saved == id))
                || task_id.is_none() && !auth::auth_session()?.logged_in && !trial.used)
        {
            return Ok(LlmGatewayConfig {
                base_url: auth::configured_base_url()
                    .context("socai service URL is not configured")?,
                device_token: trial.device_token,
            });
        }
    }
    auth::llm_gateway_config()
}

pub fn is_guest_task(task_id: &str) -> bool {
    read_trial().is_ok_and(|trial| trial.task_ids.iter().any(|id| id == task_id))
}

/// A signed-in follow-up reuses the conversation id but bills the real account.
pub fn prepare_account_task(task_id: &str) -> Result<()> {
    if is_guest_task(task_id) {
        let mut trial = read_trial()?;
        trial.task_ids.retain(|id| id != task_id);
        save_trial(&trial)?;
    }
    Ok(())
}
