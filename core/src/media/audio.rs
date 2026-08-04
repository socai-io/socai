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
use symphonia::core::units::TimeBase;

use crate::media::common::{ensure_dir, url_suffix, MediaUnavailable};
use crate::media::processor::MediaProcessor;

impl MediaProcessor {
    /// Transcribe a video/audio source through socai pro cloud ASR — the only
    /// transcription path (local whisper was removed as uncontrollable).
    pub async fn transcribe_audio(&self, source: &str, referer: &str) -> Result<String> {
        let t0 = Instant::now();
        let result = self.transcribe_audio_inner(source, referer).await;
        self.timing.record("asr_transcribe", t0.elapsed());
        result
    }

    async fn transcribe_audio_inner(&self, source: &str, referer: &str) -> Result<String> {
        if !self.config.use_cloud_asr {
            anyhow::bail!(MediaUnavailable(
                "audio transcription requires socai pro (cloud ASR)".into()
            ));
        }
        let source_path = self.local_audio_source(source, referer).await?;
        // Real clip duration (the clip is already capped at
        // max_audio_seconds); the server uses it for usage accounting.
        let (aac, duration) = self.extract_audio_aac(&source_path).await?;
        let duration_s = duration.ceil() as i64;
        let result = crate::cloud::transcribe_audio_file(
            &aac,
            duration_s,
            Duration::from_secs(self.config.asr_timeout_s.max(60)),
            self.billing_task_id.as_deref(),
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
        Ok(result.transcript.trim().to_string())
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
    let time_base = track
        .codec_params
        .time_base
        .unwrap_or_else(|| TimeBase::new(1, sample_rate));

    let mut writer = std::io::BufWriter::new(
        std::fs::File::create(target)
            .with_context(|| format!("failed to create {}", target.display()))?,
    );
    let mut end_ts = 0u64;
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
        if timestamp_seconds(time_base, packet.ts) >= max_seconds as f64 {
            break;
        }
        writer.write_all(&header.for_frame(packet.data.len())?)?;
        writer.write_all(&packet.data)?;
        end_ts = packet.ts + packet.dur;
    }
    writer.flush()?;
    if end_ts == 0 {
        anyhow::bail!(MediaUnavailable(format!(
            "AAC track in {} contains no audio packets",
            source.display()
        )));
    }
    Ok(timestamp_seconds(time_base, end_ts).min(max_seconds as f64))
}

fn timestamp_seconds(time_base: TimeBase, ts: u64) -> f64 {
    let time = time_base.calc_time(ts);
    time.seconds as f64 + time.frac
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
