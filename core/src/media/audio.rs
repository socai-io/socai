use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use symphonia::core::codecs::CODEC_TYPE_AAC;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::media::asr::transcribe_local_file_with_timeout;
use crate::media::common::{ensure_dir, url_suffix, MediaUnavailable};
use crate::media::processor::MediaProcessor;

impl MediaProcessor {
    /// Transcribe a video/audio source through managed cloud ASR for paid
    /// sessions, or through the bundled local Whisper small worker otherwise.
    pub async fn transcribe_audio(&self, source: &str, referer: &str) -> Result<String> {
        let t0 = Instant::now();
        let result = self.transcribe_audio_inner(source, referer).await;
        self.timing.record("asr_transcribe", t0.elapsed());
        result
    }

    async fn transcribe_audio_inner(&self, source: &str, referer: &str) -> Result<String> {
        if !self.config.use_cloud_asr {
            anyhow::bail!(MediaUnavailable(
                "video transcription is disabled for this media request".into()
            ));
        }
        let source_path = self.local_audio_source(source, referer).await?;
        // Cloud ASR is a paid account capability, independent of the selected
        // LLM provider. If the live wallet cannot be checked, stay functional
        // and private by falling back to the bundled local model.
        let cloud_ready = match crate::cloud::paid_asr_access().await {
            Ok(access) => access.ready,
            Err(error) => {
                tracing::warn!(%error, "cloud ASR access check failed; using local ASR");
                false
            }
        };
        if !cloud_ready || self.billing_task_id.is_none() {
            return self.transcribe_local_source(&source_path).await;
        }
        // Real clip duration (the clip is already capped at
        // max_audio_seconds); the server uses it for usage accounting.
        let (aac, duration) = self.extract_audio_aac(&source_path).await?;
        let duration_s = duration.ceil() as i64;
        let result = match crate::cloud::transcribe_audio_file(
            &aac,
            duration_s,
            Duration::from_secs(self.config.asr_timeout_s.max(60)),
            self.billing_task_id.as_deref(),
        )
        .await
        {
            Ok(result) => result,
            Err(error) if crate::cloud::cloud_asr_access_rejected(&error) => {
                tracing::warn!(%error, "cloud ASR access changed before upload; using local ASR");
                return self.transcribe_local_source(&source_path).await;
            }
            Err(error) => return Err(error),
        };
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
        Ok(result.transcript.trim().to_string())
    }

    async fn transcribe_local_source(&self, source_path: &Path) -> Result<String> {
        transcribe_local_file_with_timeout(
            source_path,
            self.config.max_audio_seconds,
            Duration::from_secs(self.config.asr_timeout_s.max(60)),
        )
        .await
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

    /// Extract the source's AAC track into an ADTS `.aac` file without
    /// re-encoding, capped at `max_audio_seconds`. Cloud ASR (Fun-ASR) accepts
    /// aac as-is, so no local decode is needed. Returns the file path and its
    /// duration in seconds.
    async fn extract_audio_aac(&self, source_path: &Path) -> Result<(PathBuf, f64)> {
        let out = self.audio_output_path(source_path)?;
        let source = source_path.to_path_buf();
        let target = out.clone();
        let max_seconds = self.config.max_audio_seconds;
        let duration =
            tokio::task::spawn_blocking(move || demux_aac_to_adts(&source, &target, max_seconds))
                .await
                .context("audio demux task panicked")??;
        Ok((out, duration))
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
        Ok(dir.join("audio.aac"))
    }
}

/// Copy the first AAC track of `source` into `target` as raw ADTS frames
/// (one 7-byte header per packet, no re-encoding), keeping at most
/// `max_seconds` of audio. Returns the written duration in seconds.
fn demux_aac_to_adts(source: &Path, target: &Path, max_seconds: u64) -> Result<f64> {
    let file = std::fs::File::open(source)
        .with_context(|| format!("failed to open {}", source.display()))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = source.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .with_context(|| format!("unsupported media container in {}", source.display()))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec == CODEC_TYPE_AAC)
        .ok_or_else(|| MediaUnavailable(format!("no AAC audio track in {}", source.display())))?;
    let track_id = track.id;
    let sample_rate = track
        .codec_params
        .sample_rate
        .ok_or_else(|| MediaUnavailable("AAC track has no sample rate".into()))?;
    let asc = track
        .codec_params
        .extra_data
        .as_deref()
        .ok_or_else(|| MediaUnavailable("AAC track has no decoder config".into()))?;
    let header = AdtsHeader::from_asc(asc)?;
    let mut writer = std::io::BufWriter::new(
        std::fs::File::create(target)
            .with_context(|| format!("failed to create {}", target.display()))?,
    );
    // The emitted ADTS header declares one 1024-sample AAC frame per packet.
    // Bound the output using that exact duration contract, which is also what
    // socai-server validates after upload. Using the source packet timestamp
    // allowed the last frame to cross the configured limit while the client
    // still reported the capped value.
    let max_samples = max_seconds.saturating_mul(u64::from(sample_rate));
    let mut written_samples = 0u64;
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(err) => return Err(err.into()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        const SAMPLES_PER_AAC_FRAME: u64 = 1024;
        if written_samples.saturating_add(SAMPLES_PER_AAC_FRAME) > max_samples {
            break;
        }
        writer.write_all(&header.for_frame(packet.data.len())?)?;
        writer.write_all(&packet.data)?;
        written_samples += SAMPLES_PER_AAC_FRAME;
    }
    writer.flush()?;
    if written_samples == 0 {
        anyhow::bail!(MediaUnavailable(format!(
            "AAC track in {} contains no audio packets",
            source.display()
        )));
    }
    Ok(written_samples as f64 / f64::from(sample_rate))
}

/// Fixed part of an ADTS header for an AAC-LC stream; only the per-frame
/// length bits vary between frames.
struct AdtsHeader {
    sample_rate_index: u8,
    channel_config: u8,
}

impl AdtsHeader {
    /// Read the sample rate index and channel configuration from the track's
    /// AudioSpecificConfig (the mp4 `esds` decoder config).
    fn from_asc(asc: &[u8]) -> Result<Self> {
        if asc.len() < 2 {
            anyhow::bail!(MediaUnavailable("AAC decoder config is too short".into()));
        }
        let object_type = asc[0] >> 3;
        if object_type == 31 {
            // Escape-coded object type shifts every following field.
            anyhow::bail!(MediaUnavailable(format!(
                "unsupported AAC object type {object_type}"
            )));
        }
        let sample_rate_index = ((asc[0] & 0x7) << 1) | (asc[1] >> 7);
        if sample_rate_index >= 13 {
            anyhow::bail!(MediaUnavailable(
                "AAC stream uses an explicit sample rate, unsupported in ADTS".into(),
            ));
        }
        let channel_config = (asc[1] >> 3) & 0xF;
        if !(1..=7).contains(&channel_config) {
            anyhow::bail!(MediaUnavailable(format!(
                "unsupported AAC channel configuration {channel_config}"
            )));
        }
        Ok(Self {
            sample_rate_index,
            channel_config,
        })
    }

    fn for_frame(&self, payload_len: usize) -> Result<[u8; 7]> {
        let frame_len = payload_len + 7;
        if frame_len > 0x1FFF {
            anyhow::bail!("AAC frame too large for ADTS: {payload_len} bytes");
        }
        let frame_len = frame_len as u16;
        Ok([
            0xFF,                                                                  // syncword
            0xF1, // syncword end, MPEG-4, layer 0, no CRC
            (1 << 6) | (self.sample_rate_index << 2) | (self.channel_config >> 2), // AAC-LC
            ((self.channel_config & 0x3) << 6) | ((frame_len >> 11) as u8 & 0x3),
            (frame_len >> 3) as u8,
            (((frame_len & 0x7) as u8) << 5) | 0x1F, // buffer fullness (VBR)
            0xFC,                                    // buffer fullness end, 1 frame
        ])
    }
}
