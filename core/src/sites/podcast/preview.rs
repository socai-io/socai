//! Persistent, language-separated preview cache; never added to agent context.
use anyhow::{Context, Result};
use serde_json::{json, Value};

pub async fn read(slug: &str, kind: &str, language: &str) -> Result<Value> {
    anyhow::ensure!(
        !slug.is_empty()
            && slug.len() <= 300
            && slug
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
        "Invalid podcast slug"
    );
    anyhow::ensure!(
        ["metadata", "tldr", "digest", "transcript"].contains(&kind),
        "Invalid preview section"
    );
    let language = language.to_uppercase();
    anyhow::ensure!(
        ["ZH", "EN"].contains(&language.as_str()),
        "Invalid preview language"
    );
    let home = std::env::var_os("SOCAI_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or(
            crate::config::config_path()?
                .parent()
                .context("Missing config directory")?
                .to_path_buf(),
        );
    let dir = home.join("cache/podcast/v1").join(slug).join(&language);
    let path = dir.join(format!("{kind}.json"));
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = LOCK.lock().await;
    if let Ok(bytes) = tokio::fs::read(&path).await {
        if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
            if value["slug"] == slug
                && value["section"] == kind
                && value["requested_language"] == language
            {
                return Ok(value);
            }
        }
    }
    let value = if kind == "metadata" {
        json!({"slug":slug, "section":kind,"requested_language":language,"participants":super::client::participants(slug).await?})
    } else {
        super::client::complete_section(slug, kind, &language).await?
    };
    // Do not cache missing content: the publisher may add it later.
    if (kind == "metadata"
        && value["participants"]
            .as_array()
            .is_some_and(|v| !v.is_empty()))
        || value["available"] == true
    {
        tokio::fs::create_dir_all(&dir).await?;
        let temp = dir.join(format!("{}.tmp", uuid::Uuid::new_v4()));
        tokio::fs::write(&temp, serde_json::to_vec(&value)?).await?;
        if let Err(error) = tokio::fs::rename(&temp, &path).await {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(error).context("Could not save podcast preview cache");
        }
    }
    Ok(value)
}
