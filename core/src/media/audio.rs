use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::process::Command;

use crate::media::common::{
    ensure_dir, find_in_path, nonempty, run_command, url_suffix, MediaUnavailable,
};
use crate::media::processor::MediaProcessor;

impl MediaProcessor {
    pub async fn transcribe_audio(
        &self,
        source: &str,
        referer: &str,
        language: &str,
    ) -> Result<String> {
        let t0 = Instant::now();
        let result = self.transcribe_audio_inner(source, referer, language).await;
        self.timing.record("whisper_transcribe", t0.elapsed());
        result
    }

    async fn transcribe_audio_inner(
        &self,
        source: &str,
        referer: &str,
        language: &str,
    ) -> Result<String> {
        let source_path = self.local_audio_source(source, referer).await?;
        if self.config.use_cloud_asr {
            let wav = self.extract_audio_wav(&source_path).await?;
            // Real clip duration (the wav is already capped at
            // max_audio_seconds by ffmpeg); the server uses it for usage
            // accounting. 0 when ffprobe is unavailable.
            let duration_s = probe_duration_seconds(&wav)
                .await
                .map(|secs| secs.ceil() as i64)
                .unwrap_or(0);
            let result = crate::cloud::transcribe_audio_file(
                &wav,
                duration_s,
                Duration::from_secs(self.config.whisper_timeout_s.max(60)),
            )
            .await?;
            self.timing.record(
                "cloud_asr_total",
                Duration::from_millis(result.total_latency_ms as u64),
            );
            if result.provider_latency_ms > 0 {
                self.timing.record(
                    "cloud_asr_provider",
                    Duration::from_millis(result.provider_latency_ms as u64),
                );
            }
            return Ok(result.transcript.trim().to_string());
        }

        if !self.config.use_whisper {
            anyhow::bail!(MediaUnavailable("Whisper transcription is disabled".into()));
        }
        if let Some(whisper_cli) = find_in_path("whisper") {
            let out_dir = ensure_dir(&self.config.base_dir.join("transcripts"))?;
            run_command(
                Command::new(whisper_cli)
                    .arg(&source_path)
                    .arg("--language")
                    .arg(nonempty(language, &self.config.default_language))
                    .arg("--output_format")
                    .arg("txt")
                    .arg("--output_dir")
                    .arg(&out_dir),
                Duration::from_secs(self.config.whisper_timeout_s),
            )
            .await?;
            let txt = out_dir.join(format!(
                "{}.txt",
                source_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
            ));
            return Ok(tokio::fs::read_to_string(txt)
                .await
                .unwrap_or_default()
                .trim()
                .to_string());
        }

        let whisper_cpp = find_in_path("whisper-cli").or_else(|| find_in_path("main"));
        let whisper_model = std::env::var("SOCAI_WHISPER_MODEL").unwrap_or_default();
        if let (Some(whisper_cpp), model) = (whisper_cpp, whisper_model.trim().to_string()) {
            if !model.is_empty() {
                let wav = self.extract_audio_wav(&source_path).await?;
                let out_prefix = self.config.base_dir.join("transcripts").join(
                    source_path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("audio"),
                );
                ensure_dir(out_prefix.parent().unwrap_or(&self.config.base_dir))?;
                run_command(
                    Command::new(whisper_cpp)
                        .arg("-m")
                        .arg(model)
                        .arg("-f")
                        .arg(&wav)
                        .arg("-l")
                        .arg(nonempty(language, &self.config.default_language))
                        .arg("-otxt")
                        .arg("-of")
                        .arg(&out_prefix),
                    Duration::from_secs(self.config.whisper_timeout_s),
                )
                .await?;
                return Ok(tokio::fs::read_to_string(out_prefix.with_extension("txt"))
                    .await
                    .unwrap_or_default()
                    .trim()
                    .to_string());
            }
        }

        anyhow::bail!(MediaUnavailable(
            "No whisper provider configured. Install `whisper`, or set SOCAI_WHISPER_MODEL for whisper.cpp.".into(),
        ));
    }

    async fn local_audio_source(&self, source: &str, referer: &str) -> Result<PathBuf> {
        let value = source.trim();
        if value.is_empty() {
            anyhow::bail!("audio source is required");
        }
        if value.starts_with("http://") || value.starts_with("https://") {
            self.download_file(value, referer, "audio", &url_suffix(value, ".mp4"))
                .await
        } else {
            Ok(PathBuf::from(value))
        }
    }

    async fn extract_audio_wav(&self, source_path: &Path) -> Result<PathBuf> {
        if find_in_path("ffmpeg").is_none() {
            anyhow::bail!(MediaUnavailable(
                "ffmpeg is required for whisper.cpp audio extraction".into()
            ));
        }
        let out = self.audio_output_path(source_path)?;
        run_command(
            Command::new("ffmpeg")
                .arg("-hide_banner")
                .arg("-loglevel")
                .arg("error")
                .arg("-t")
                .arg(self.config.max_audio_seconds.to_string())
                .arg("-i")
                .arg(source_path)
                .arg("-ar")
                .arg("16000")
                .arg("-ac")
                .arg("1")
                .arg("-y")
                .arg(&out),
            Duration::from_secs(self.config.ffmpeg_timeout_s),
        )
        .await?;
        Ok(out)
    }

    fn audio_output_path(&self, source_path: &Path) -> Result<PathBuf> {
        let dir = if source_path.starts_with(&self.config.base_dir) {
            source_path
                .parent()
                .unwrap_or_else(|| self.config.base_dir.as_path())
                .to_path_buf()
        } else {
            self.config.base_dir.join("audio")
        };
        let dir = ensure_dir(&dir)?;
        Ok(dir.join("audio.wav"))
    }
}

async fn probe_duration_seconds(path: &Path) -> Option<f64> {
    let ffprobe = find_in_path("ffprobe")?;
    let output = Command::new(ffprobe)
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("csv=p=0")
        .arg(path)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|secs| secs.is_finite() && *secs >= 0.0)
}
