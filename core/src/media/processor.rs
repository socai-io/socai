use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::agent::Backend as LlmProvider;
use anyhow::Result;
use futures::StreamExt;
use serde_json::Value;
use tokio::io::AsyncWriteExt;

use crate::media::common::{
    ensure_dir, named_file_path, save_bytes, save_named_bytes, MediaConfig, USER_AGENT,
};
use crate::media::timing::TimingRecord;

#[derive(Clone)]
pub struct MediaProcessor {
    pub(crate) config: MediaConfig,
    pub(crate) llm_provider: Option<Arc<dyn LlmProvider>>,
    pub(crate) client: reqwest::Client,
    pub(crate) timing: Arc<TimingRecord>,
}

impl MediaProcessor {
    pub fn new(config: MediaConfig, llm_provider: Option<Arc<dyn LlmProvider>>) -> Result<Self> {
        ensure_dir(&config.base_dir)?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.request_timeout_s))
            .user_agent(USER_AGENT)
            .build()?;
        Ok(Self {
            config,
            llm_provider,
            client,
            timing: Arc::new(TimingRecord::default()),
        })
    }

    pub fn for_run_dir(
        run_dir: impl AsRef<Path>,
        llm_provider: Option<Arc<dyn LlmProvider>>,
    ) -> Result<Self> {
        Self::new(
            MediaConfig::new(run_dir.as_ref().join("site_media")),
            llm_provider,
        )
    }

    pub fn timing(&self) -> Arc<TimingRecord> {
        self.timing.clone()
    }

    pub fn set_cloud_asr(&mut self, enabled: bool) {
        self.config.use_cloud_asr = enabled;
    }

    pub fn timing_summary(&self) -> Value {
        self.timing.summary()
    }

    pub fn reset_timing(&self) {
        self.timing.reset();
    }

    pub async fn download_bytes(&self, url: &str, referer: &str) -> Result<Vec<u8>> {
        self.download_bytes_with_timeout(
            url,
            referer,
            Duration::from_secs(self.config.request_timeout_s),
        )
        .await
    }

    pub async fn download_bytes_with_timeout(
        &self,
        url: &str,
        referer: &str,
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        let t0 = Instant::now();
        let result = async {
            let target = url.trim();
            if target.is_empty() {
                return Ok(Vec::new());
            }
            let mut request = self.client.get(target).timeout(timeout);
            if !referer.trim().is_empty() {
                request = request.header("Referer", referer.trim());
            }
            let bytes = request.send().await?.error_for_status()?.bytes().await?;
            Ok::<Vec<u8>, anyhow::Error>(bytes.to_vec())
        }
        .await;
        self.timing.record("download", t0.elapsed());
        result
    }

    pub fn save_bytes(&self, payload: &[u8], label: &str, suffix: &str) -> Result<PathBuf> {
        save_bytes(&self.config.base_dir, payload, label, suffix)
    }

    pub fn save_named_bytes(&self, payload: &[u8], label: &str, filename: &str) -> Result<PathBuf> {
        save_named_bytes(&self.config.base_dir, payload, label, filename)
    }

    pub async fn download_file(
        &self,
        url: &str,
        referer: &str,
        label: &str,
        suffix: &str,
    ) -> Result<PathBuf> {
        let payload = self.download_bytes(url, referer).await?;
        self.save_bytes(&payload, label, suffix)
    }

    pub async fn download_named_file(
        &self,
        url: &str,
        referer: &str,
        label: &str,
        filename: &str,
    ) -> Result<PathBuf> {
        let payload = self.download_bytes(url, referer).await?;
        self.save_named_bytes(&payload, label, filename)
    }

    pub async fn download_file_with_timeout(
        &self,
        url: &str,
        referer: &str,
        label: &str,
        suffix: &str,
        timeout: Duration,
    ) -> Result<PathBuf> {
        let payload = self
            .download_bytes_with_timeout(url, referer, timeout)
            .await?;
        self.save_bytes(&payload, label, suffix)
    }

    /// Stream a large download to a stable named file without buffering the
    /// whole payload in memory. The `.part` file is removed on timeout,
    /// cancellation (future drop), or failure; the final path only appears
    /// after a complete atomic rename.
    pub async fn download_named_file_streaming_with_timeout(
        &self,
        url: &str,
        referer: &str,
        label: &str,
        filename: &str,
        timeout: Duration,
    ) -> Result<PathBuf> {
        struct PartialFileGuard(Option<PathBuf>);
        impl Drop for PartialFileGuard {
            fn drop(&mut self) {
                if let Some(path) = self.0.take() {
                    let _ = std::fs::remove_file(path);
                }
            }
        }

        let target = url.trim();
        if target.is_empty() {
            anyhow::bail!("download URL is empty");
        }
        let path = named_file_path(&self.config.base_dir, label, filename)?;
        if std::fs::metadata(&path).is_ok_and(|meta| meta.is_file() && meta.len() > 0) {
            return Ok(path);
        }
        let part_path = path.with_extension(format!(
            "{}.{}.part",
            path.extension()
                .and_then(|value| value.to_str())
                .unwrap_or("bin"),
            uuid::Uuid::new_v4()
        ));
        let mut partial = PartialFileGuard(Some(part_path.clone()));
        let t0 = Instant::now();
        let result = tokio::time::timeout(timeout, async {
            let mut request = self.client.get(target).timeout(timeout);
            if !referer.trim().is_empty() {
                request = request.header("Referer", referer.trim());
            }
            let response = request.send().await?.error_for_status()?;
            let mut stream = response.bytes_stream();
            let mut file = tokio::fs::File::create(&part_path).await?;
            while let Some(chunk) = stream.next().await {
                file.write_all(&chunk?).await?;
            }
            file.flush().await?;
            drop(file);
            if let Err(err) = tokio::fs::rename(&part_path, &path).await {
                // Another concurrent read of the same note may have completed
                // first. Its non-empty stable file is equivalent; otherwise
                // preserve the real rename error.
                if !std::fs::metadata(&path).is_ok_and(|meta| meta.is_file() && meta.len() > 0) {
                    return Err(err.into());
                }
                let _ = tokio::fs::remove_file(&part_path).await;
            }
            Ok::<PathBuf, anyhow::Error>(path.clone())
        })
        .await
        .map_err(|_| anyhow::anyhow!("download timed out after {}s", timeout.as_secs()))?;
        self.timing.record("download", t0.elapsed());
        if result.is_ok() {
            partial.0 = None;
        }
        result
    }
}
