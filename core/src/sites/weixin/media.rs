//! Background WeChat image downloads. Article HTML is still read only in Chrome.
use crate::agent::ToolContext;
use crate::media::{
    background_media_is_stopped, background_video_download_semaphore,
    current_background_media_generation, emit_background_media_event,
    reserve_background_video_download, subscribe_background_media_cancellation,
    wait_for_background_media_cancellation, BackgroundMediaEvent,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;

fn allowed(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some_and(|host| {
            ["qpic.cn", "qlogo.cn"]
                .iter()
                .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
        })
}

pub(crate) fn schedule(ctx: &ToolContext, note_id: &str, record: &Value) {
    let generation = ctx
        .background_media_generation
        .unwrap_or_else(current_background_media_generation);
    let run_dir = ctx.run_dir.to_string_lossy().into_owned();
    if background_media_is_stopped(generation, &run_dir) {
        return;
    }
    let mut images = Vec::new();
    if let Some(url) = record
        .pointer("/author/avatar_source")
        .and_then(Value::as_str)
    {
        images.push((None, url.to_string()));
    }
    for (index, image) in record["media"].as_array().into_iter().flatten().enumerate() {
        if let Some(url) = image["source_url"].as_str() {
            images.push((Some(index), url.to_string()));
        }
    }
    if images.is_empty() {
        return;
    }
    let Some(reservation) = reserve_background_video_download(generation, &run_dir, note_id) else {
        return;
    };
    let ctx = ctx.clone();
    let note_id = note_id.to_string();
    tokio::spawn(async move {
        let _reservation = reservation;
        let mut cancellation = subscribe_background_media_cancellation();
        let work = async {
            let semaphore = background_video_download_semaphore();
            let _permit = semaphore.acquire_owned().await?;
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    if attempt.previous().len() < 5 && allowed(attempt.url()) {
                        attempt.follow()
                    } else {
                        attempt.error("image redirect outside WeChat CDN")
                    }
                }))
                .build()?;
            for (index, url) in images {
                if background_media_is_stopped(generation, &run_dir) {
                    break;
                }
                let outcome = download(&client, &ctx, &url).await;
                ctx.update_recorded_note(&note_id, |note| {
                    let target = if let Some(index) = index {
                        // Ignore stale completion if this article's image order changed.
                        let Some(target) = note
                            .get_mut("media")
                            .and_then(Value::as_array_mut)
                            .and_then(|m| m.get_mut(index))
                        else {
                            return;
                        };
                        if target["source_url"] != url {
                            return;
                        }
                        target
                    } else {
                        if note
                            .pointer("/author/avatar_source")
                            .and_then(Value::as_str)
                            != Some(url.as_str())
                        {
                            return;
                        }
                        &mut note["author"]
                    };
                    match &outcome {
                        Ok(path) => {
                            target[if index.is_some() { "src" } else { "avatar" }] = json!(path);
                            target["download_state"] = json!("ready");
                        }
                        Err(error) => {
                            target["download_state"] = json!("failed");
                            target["download_error"] = json!(error.to_string());
                        }
                    }
                });
                emit_background_media_event(BackgroundMediaEvent {
                    run_dir: run_dir.clone(),
                    note_id: note_id.clone(),
                });
            }
            Ok::<(), anyhow::Error>(())
        };
        let final_state = tokio::select! {
            result = work => {
                if let Err(error) = result { tracing::warn!(%error, "WeChat image downloads failed"); }
                if background_media_is_stopped(generation, &run_dir) { "cancelled" } else { "failed" }
            },
            _ = wait_for_background_media_cancellation(generation, &run_dir, &mut cancellation) => "cancelled",
        };
        // Also clear pending flags if cancellation wins while waiting for the
        // queue or client setup fails before any individual request starts.
        ctx.update_recorded_note(&note_id, |note| {
            if let Some(images) = note["media"].as_array_mut() {
                for image in images {
                    if image["download_state"] == "loading" {
                        image["download_state"] = json!(final_state);
                    }
                }
            }
        });
        emit_background_media_event(BackgroundMediaEvent { run_dir, note_id });
    });
}

async fn download(
    client: &reqwest::Client,
    ctx: &ToolContext,
    source: &str,
) -> anyhow::Result<String> {
    let url = reqwest::Url::parse(source)?;
    anyhow::ensure!(allowed(&url), "image outside WeChat CDN");
    let directory = ctx.run_dir.join("site_media/weixin");
    tokio::fs::create_dir_all(&directory).await?;
    let path = directory.join(format!("{:x}.img", Sha256::digest(source.as_bytes())));
    if path.is_file() {
        return Ok(path.to_string_lossy().into_owned());
    }
    let mut response = client
        .get(url)
        .header("Referer", "https://mp.weixin.qq.com/")
        .send()
        .await?
        .error_for_status()?;
    const MAX_BYTES: usize = 20 * 1024 * 1024;
    anyhow::ensure!(
        response.content_length().unwrap_or(0) <= MAX_BYTES as u64,
        "image exceeds size limit"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            bytes.len() + chunk.len() <= MAX_BYTES,
            "image exceeds size limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    image::guess_format(&bytes)?;
    // No partial file becomes visible to the UI. Cancellation before this point
    // drops the request and bytes; the final small file write is synchronous.
    let partial = path.with_extension(format!("{}.part", uuid::Uuid::new_v4()));
    let saved = std::fs::write(&partial, bytes).and_then(|_| std::fs::rename(&partial, &path));
    if saved.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    saved?;
    Ok(path.to_string_lossy().into_owned())
}
