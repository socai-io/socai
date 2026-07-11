use std::time::{Duration, Instant};

use serde_json::Value;

use crate::media::common::{insert_string, short, url_suffix};
use crate::media::processor::MediaProcessor;

/// Wall-clock cap for fetching a note's video file. Generous because XHS video
/// CDN throughput varies; still bounded so a stalled download can't hang a scan.
const VIDEO_DOWNLOAD_TIMEOUT_S: u64 = 180;

impl MediaProcessor {
    /// OCR a video note's already-downloaded poster in place: read
    /// `poster_local_path` and attach `poster_ocr` (or `poster_ocr_error`).
    /// The video-note counterpart of `ocr_downloaded_images` — the poster is
    /// the note's cover, so it's the whole OCR surface without touching the
    /// video file. Returns the batch inference time. No-op (returns zero) when
    /// OCR is disabled, the poster isn't on disk, or `poster_ocr` is already
    /// present.
    pub async fn ocr_downloaded_video_poster(&self, video: &mut Value) -> Duration {
        if !self.config.use_ocr || video.get("poster_ocr").is_some() {
            return Duration::ZERO;
        }
        let Some(path) = video
            .get("poster_local_path")
            .and_then(Value::as_str)
            .filter(|path| !path.is_empty())
            .map(str::to_string)
        else {
            return Duration::ZERO;
        };
        let batch = tokio::task::spawn_blocking(move || {
            let items = std::fs::read(&path)
                .map(|bytes| vec![(0, bytes)])
                .unwrap_or_default();
            crate::media::ocr::ocr_images_bytes(items)
        })
        .await;
        let Ok(mut batch) = batch else {
            return Duration::ZERO;
        };
        self.timing.record("ocr_predict", batch.predict);
        match batch.results.pop() {
            Some((_, Ok(text))) if !text.trim().is_empty() => {
                insert_string(video, "poster_ocr", text)
            }
            Some((_, Err(err))) => insert_string(video, "poster_ocr_error", err),
            _ => {}
        }
        batch.predict
    }

    /// Download only a video note's poster (its cover image) to the run's
    /// media directory, at the stable note-local path
    /// `site_media/<note>/post.jpg`. The returned video object preserves the
    /// input shape and adds `poster_local_path` (or `poster_download_error`).
    /// The video file itself is never fetched — this is the cheap path for
    /// OCR-only runs, where the poster is the whole OCR surface.
    pub async fn download_video_poster(
        &self,
        video: &Value,
        note_id: &str,
        title: &str,
        referer: &str,
    ) -> Value {
        let mut result = video.clone();
        if !result.is_object() {
            result = crate::media::common::empty_object();
        }
        let label = if !note_id.trim().is_empty() {
            note_id
        } else if !title.trim().is_empty() {
            title
        } else {
            "video"
        };

        let poster_url = result
            .get("poster_url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if !poster_url.is_empty() {
            let t_poster = Instant::now();
            match self
                .download_named_file(&poster_url, referer, label, "post.jpg")
                .await
            {
                Ok(path) => insert_string(&mut result, "poster_local_path", path.to_string_lossy()),
                Err(err) => insert_string(&mut result, "poster_download_error", format!("{err:#}")),
            }
            if let Some(map) = result.as_object_mut() {
                map.insert(
                    "poster_download_ms".into(),
                    serde_json::json!(t_poster.elapsed().as_millis() as u64),
                );
            }
        }
        result
    }

    /// Download the playable video (and poster, when present) to the run's
    /// media directory without transcription, frame extraction, OCR, or vision.
    /// The returned video object preserves the input shape and adds
    /// `local_path` / `poster_local_path` where downloads succeed. Posters are
    /// saved at the stable note-local path `site_media/<note>/post.jpg`.
    pub async fn download_video(
        &self,
        video: &Value,
        note_id: &str,
        title: &str,
        referer: &str,
    ) -> Value {
        let mut result = self
            .download_video_poster(video, note_id, title, referer)
            .await;
        let label = if !note_id.trim().is_empty() {
            note_id
        } else if !title.trim().is_empty() {
            title
        } else {
            "video"
        };

        let source = downloadable_video_url(&result);
        if source.is_empty() {
            insert_string(
                &mut result,
                "download_error",
                "downloadable video URL not found (blob: URLs cannot be downloaded)",
            );
            return result;
        }

        let t_file = Instant::now();
        match self
            .download_file_with_timeout(
                &source,
                referer,
                label,
                &url_suffix(&source, ".mp4"),
                Duration::from_secs(VIDEO_DOWNLOAD_TIMEOUT_S.max(self.config.request_timeout_s)),
            )
            .await
        {
            Ok(path) => insert_string(&mut result, "local_path", path.to_string_lossy()),
            Err(err) => insert_string(&mut result, "download_error", format!("{err:#}")),
        }
        if let Some(map) = result.as_object_mut() {
            map.insert(
                "download_ms".into(),
                serde_json::json!(t_file.elapsed().as_millis() as u64),
            );
        }
        result
    }

    /// Transcribe an already-downloaded video file in place. The caller must
    /// have downloaded the video first and stored `local_path` on the object.
    pub async fn transcribe_downloaded_video(&self, video: &mut Value) {
        if video
            .get("transcript")
            .and_then(Value::as_str)
            .is_some_and(|text| !text.trim().is_empty())
        {
            return;
        }
        let Some(path) = video
            .get("local_path")
            .and_then(Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .map(str::to_string)
        else {
            insert_string(video, "transcript_error", "video local_path is missing");
            return;
        };
        // A cached entity may carry an error from an older transcription
        // implementation. Once a real retry starts, that error is no longer
        // authoritative; replace it only if this attempt also fails.
        if let Some(map) = video.as_object_mut() {
            map.remove("transcript_error");
        }
        let t0 = Instant::now();
        match self.transcribe_audio(&path, "", "").await {
            Ok(transcript) if !transcript.trim().is_empty() => {
                insert_string(video, "transcript", transcript);
            }
            Ok(_) => {}
            Err(err) => insert_string(video, "transcript_error", format!("{err:#}")),
        }
        if let Some(map) = video.as_object_mut() {
            map.insert(
                "transcript_ms".into(),
                serde_json::json!(t0.elapsed().as_millis() as u64),
            );
        }
    }

    pub async fn enrich_video(
        &self,
        video: &Value,
        note_id: &str,
        title: &str,
        referer: &str,
        run_vision: bool,
    ) -> Value {
        let t_total = Instant::now();
        let mut result = video.clone();
        if !result.is_object() {
            result = crate::media::common::empty_object();
        }
        let source = result
            .get("resolved_url")
            .or_else(|| result.get("url"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let poster_url = result
            .get("poster_url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let label = if !note_id.trim().is_empty() {
            note_id
        } else if !title.trim().is_empty() {
            title
        } else {
            "video"
        };

        if !poster_url.is_empty() {
            self.enrich_video_poster(&mut result, &poster_url, referer, label, title, run_vision)
                .await;
        }

        if !source.is_empty() {
            if let Some(map) = result.as_object_mut() {
                map.remove("transcript_error");
            }
            match self.transcribe_audio(&source, referer, "").await {
                Ok(transcript) if !transcript.trim().is_empty() => {
                    insert_string(&mut result, "transcript", transcript);
                }
                Ok(_) => {}
                Err(err) => insert_string(&mut result, "transcript_error", format!("{err:#}")),
            }
        }

        self.timing.record("video_enrich_total", t_total.elapsed());
        result
    }

    async fn enrich_video_poster(
        &self,
        result: &mut Value,
        poster_url: &str,
        referer: &str,
        label: &str,
        title: &str,
        run_vision: bool,
    ) {
        match self.download_bytes(poster_url, referer).await {
            Ok(poster) if !poster.is_empty() => {
                match self.save_named_bytes(&poster, label, "post.jpg") {
                    Ok(path) => insert_string(result, "poster_local_path", path.to_string_lossy()),
                    Err(err) => insert_string(result, "poster_save_error", format!("{err:#}")),
                }
                if self.config.use_ocr {
                    match self.ocr_image(&poster) {
                        Ok(text) => insert_string(result, "poster_ocr", short(&text, 800)),
                        Err(err) => insert_string(result, "poster_ocr_error", format!("{err:#}")),
                    }
                }
                if run_vision && self.config.use_vision {
                    match self
                        .describe_image(
                            &poster,
                            &format!("Describe the poster image for Xiaohongshu video: {title}"),
                            180,
                        )
                        .await
                    {
                        Ok(text) => insert_string(result, "poster_description", text),
                        Err(err) => {
                            insert_string(result, "poster_vision_error", format!("{err:#}"))
                        }
                    }
                }
            }
            Ok(_) => {}
            Err(err) => insert_string(result, "poster_download_error", format!("{err:#}")),
        }
    }
}

fn downloadable_video_url(video: &Value) -> String {
    fn clean(value: &str) -> Option<String> {
        let trimmed = value.trim();
        if (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
            && !trimmed.starts_with("blob:")
        {
            Some(trimmed.to_string())
        } else {
            None
        }
    }

    for key in ["resolved_url", "master_url", "url"] {
        if let Some(url) = video.get(key).and_then(Value::as_str).and_then(clean) {
            return url;
        }
    }

    for key in ["source_urls", "backup_urls"] {
        if let Some(arr) = video.get(key).and_then(Value::as_array) {
            for item in arr {
                if let Some(url) = item.as_str().and_then(clean) {
                    return url;
                }
            }
        }
    }

    if let Some(candidates) = video.get("candidates").and_then(Value::as_array) {
        for item in candidates {
            if let Some(url) = item.get("url").and_then(Value::as_str).and_then(clean) {
                return url;
            }
        }
    }

    String::new()
}
