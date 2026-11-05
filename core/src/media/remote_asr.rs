//! Run-bound video transcription supplied by the Web orchestrator.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt;
use serde_json::{json, Value};

#[derive(Clone)]
pub struct RemoteAsrConfig {
    endpoint: String,
    bearer_token: String,
    request_context: Value,
    run_dir: PathBuf,
    client: reqwest::Client,
}

impl std::fmt::Debug for RemoteAsrConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteAsrConfig")
            .field("bearer_token", &"[redacted]")
            .finish_non_exhaustive()
    }
}

impl RemoteAsrConfig {
    pub fn new(
        endpoint: impl Into<String>,
        bearer_token: impl Into<String>,
        request_context: Value,
        run_dir: impl AsRef<Path>,
    ) -> Result<Self> {
        let endpoint = endpoint.into();
        let bearer_token = bearer_token.into();
        let url = reqwest::Url::parse(&endpoint).context("managed ASR endpoint is invalid")?;
        if url.scheme() != "http"
            || !matches!(url.host_str(), Some("127.0.0.1" | "::1" | "localhost"))
            || url.port().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || url.query().is_some()
            || !(32..=512).contains(&bearer_token.len())
            || !request_context.is_object()
        {
            anyhow::bail!("managed ASR requires an authenticated HTTP loopback endpoint");
        }
        Ok(Self {
            endpoint,
            bearer_token,
            request_context,
            run_dir: run_dir
                .as_ref()
                .canonicalize()
                .context("managed ASR run directory is unavailable")?,
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        })
    }

    pub(crate) async fn transcribe(&self, wav: &Path) -> Result<Value> {
        let wav = wav
            .canonicalize()
            .context("managed ASR audio is unavailable")?;
        let relative = wav
            .strip_prefix(&self.run_dir)
            .context("managed ASR audio is outside the current run")?;
        let mut request = self
            .request_context
            .as_object()
            .cloned()
            .context("managed ASR context is unavailable")?;
        request.insert("tool".into(), json!("internal_transcribe_audio"));
        request.insert("input".into(), json!({"path": relative.to_string_lossy()}));
        // Never automatically retry: a lost response does not mean the paid
        // upstream request was not submitted. The host owns durable dedup.
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.bearer_token)
            .json(&request)
            .timeout(Duration::from_secs(1_900))
            .send()
            .await
            .context("managed video transcription request failed")?;
        let status = response.status();
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("managed ASR response could not be read")?;
            if bytes.len().saturating_add(chunk.len()) > 1024 * 1024 {
                anyhow::bail!("managed ASR response exceeded 1 MiB");
            }
            bytes.extend_from_slice(&chunk);
        }
        let body: Value =
            serde_json::from_slice(&bytes).context("managed ASR returned invalid JSON")?;
        if !status.is_success() || body.get("ok").and_then(Value::as_bool) != Some(true) {
            anyhow::bail!(
                "{}",
                body.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("managed video transcription failed")
                    .chars()
                    .take(1000)
                    .collect::<String>()
            );
        }
        let result = body
            .get("result")
            .context("managed ASR returned no result")?;
        let text = result
            .get("text")
            .and_then(Value::as_str)
            .context("managed ASR returned no transcript")?;
        if text.trim().is_empty() || text.chars().count() > 100_000 {
            anyhow::bail!("managed ASR transcript is empty or exceeds the text limit");
        }
        let seconds = result
            .get("duration_s")
            .and_then(Value::as_u64)
            .filter(|seconds| (1..=1200).contains(seconds))
            .context("managed ASR duration is invalid")?;
        let segments = result
            .get("segments")
            .and_then(Value::as_array)
            .context("managed ASR sentence list is invalid")?;
        let timed = result
            .get("timestamps_available")
            .and_then(Value::as_bool)
            .context("managed ASR timing status is missing")?;
        if timed == segments.is_empty() {
            anyhow::bail!("managed ASR timing status disagrees with the sentence list");
        }
        for segment in segments {
            let start = segment
                .get("start_ms")
                .and_then(Value::as_u64)
                .context("invalid ASR sentence start")?;
            let end = segment
                .get("end_ms")
                .and_then(Value::as_u64)
                .context("invalid ASR sentence end")?;
            if start > end
                || end > (seconds + 1) * 1000
                || segment
                    .get("text")
                    .and_then(Value::as_str)
                    .is_none_or(|text| text.trim().is_empty())
            {
                anyhow::bail!("managed ASR sentence is invalid");
            }
        }
        Ok(result.clone())
    }
}
