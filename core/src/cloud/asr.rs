use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;
use tokio::io::AsyncReadExt;

use super::auth::{bearer, configured_base_url, http_client, load_credentials};

const TASK_POLL_INTERVAL: Duration = Duration::from_secs(1);
const AUDIO_UPLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_AUDIO_UPLOAD_BYTES: u64 = 128 * 1024 * 1024;
/// Consecutive poll failures tolerated before giving up on a submitted task.
const MAX_POLL_FAILURES: u32 = 3;

#[derive(Debug, Deserialize)]
struct UploadUrlResponse {
    task_id: String,
    upload_url: String,
    headers: std::collections::HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct TaskResponse {
    status: String,
    transcript: Option<String>,
    error: Option<String>,
    provider_latency_ms: i64,
}

#[derive(Debug, Clone)]
pub struct CloudAsrResult {
    pub transcript: String,
    pub provider_latency_ms: i64,
    pub total_latency_ms: u128,
}

pub async fn transcribe_audio_file(
    path: &Path,
    duration_s: i64,
    timeout: Duration,
    client_task_id: Option<&str>,
) -> Result<CloudAsrResult> {
    let base_url = configured_base_url()
        .ok_or_else(|| anyhow::anyhow!("socai server URL is not configured"))?;
    let creds = load_credentials().ok_or_else(|| {
        anyhow::anyhow!("sign in and select socai agent before requesting video transcription")
    })?;
    if creds.user_id.trim().is_empty() || !creds.hosted_llm_selected {
        anyhow::bail!("sign in and select socai agent before requesting video transcription");
    }
    // The extension matters: the server passes the filename through to
    // DashScope, which detects the audio format from it.
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("audio.aac");
    let size_bytes = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("failed to read metadata for {}", path.display()))?
        .len();
    if size_bytes > MAX_AUDIO_UPLOAD_BYTES {
        anyhow::bail!(
            "audio upload is too large: {} bytes exceeds the 128 MiB limit",
            size_bytes
        );
    }
    let client = http_client()?;
    let started = Instant::now();
    let upload: UploadUrlResponse = bearer(
        client
            .post(format!("{base_url}/v1/asr/upload-url"))
            .json(&json!({
                "filename": filename,
                "content_type": "audio/aac",
                "size_bytes": size_bytes,
                "duration_s": duration_s.max(0),
                "client_task_id": client_task_id.unwrap_or(""),
            })),
        &creds.device_token,
    )
    .send()
    .await?
    .error_for_status()?
    .json()
    .await?;

    let file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("failed to open {} for upload", path.display()))?;
    let stream = futures::stream::try_unfold(file, |mut file| async move {
        let mut chunk = vec![0u8; 64 * 1024];
        let read = file.read(&mut chunk).await?;
        if read == 0 {
            return Ok::<_, std::io::Error>(None);
        }
        chunk.truncate(read);
        Ok(Some((chunk, file)))
    });
    let mut put = client
        .put(&upload.upload_url)
        .timeout(AUDIO_UPLOAD_TIMEOUT)
        .header(reqwest::header::CONTENT_LENGTH, size_bytes)
        .body(reqwest::Body::wrap_stream(stream));
    for (key, value) in &upload.headers {
        put = put.header(key, value);
    }
    put.send().await?.error_for_status()?;

    bearer(
        client.post(format!("{base_url}/v1/asr/tasks/{}/submit", upload.task_id)),
        &creds.device_token,
    )
    .send()
    .await?
    .error_for_status()?;

    let deadline = Instant::now() + timeout;
    let mut poll_failures: u32 = 0;
    loop {
        // The upload is already done at this point; tolerate a few transient
        // poll errors (5xx, network blips) instead of abandoning the task.
        match poll_task(&client, &base_url, &upload.task_id, &creds.device_token).await {
            Ok(task) => {
                poll_failures = 0;
                match task.status.as_str() {
                    "succeeded" => {
                        return Ok(CloudAsrResult {
                            transcript: task.transcript.unwrap_or_default(),
                            provider_latency_ms: task.provider_latency_ms,
                            total_latency_ms: started.elapsed().as_millis(),
                        });
                    }
                    "failed" => {
                        let error = task.error.unwrap_or_else(|| "unknown error".into());
                        // Fun-ASR reports an audio track it decoded fine but
                        // heard no speech in (music/SFX-only videos) as a
                        // failure. Surface the meaning, not the provider blob.
                        if error.contains("ASR_RESPONSE_HAVE_NO_WORDS") {
                            anyhow::bail!(
                                "no speech detected in the video's audio track (it likely \
                                 contains only music or sound effects), so there is no \
                                 spoken content to transcribe"
                            );
                        }
                        anyhow::bail!("video transcription failed: {}", short_error(&error));
                    }
                    _ => {}
                }
            }
            Err(err) => {
                poll_failures += 1;
                if poll_failures >= MAX_POLL_FAILURES {
                    return Err(err.context("video transcription status polling failed"));
                }
            }
        }
        if Instant::now() >= deadline {
            anyhow::bail!("video transcription timed out after {}s", timeout.as_secs());
        }
        tokio::time::sleep(TASK_POLL_INTERVAL).await;
    }
}

async fn poll_task(
    client: &reqwest::Client,
    base_url: &str,
    task_id: &str,
    token: &str,
) -> Result<TaskResponse> {
    Ok(bearer(
        client.get(format!("{base_url}/v1/asr/tasks/{task_id}")),
        token,
    )
    .send()
    .await?
    .error_for_status()?
    .json()
    .await?)
}

/// Trim a provider error to something an agent can read. Raw DashScope task
/// dumps run to kilobytes of nested JSON with signed URLs; the leading part
/// carries the task id + failure code, which is all a transcript_error needs.
fn short_error(error: &str) -> String {
    const MAX: usize = 300;
    let trimmed = error.trim();
    if trimmed.chars().count() <= MAX {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(MAX).collect();
    format!("{head}… (truncated)")
}
