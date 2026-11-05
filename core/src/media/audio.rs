use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_AAC};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::media::asr::transcribe_local_file_with_timeout;
use crate::media::common::{ensure_dir, url_suffix, MediaUnavailable};
use crate::media::processor::MediaProcessor;

const CLOUD_ASR_SAMPLE_RATE: u32 = 16_000;
const MAX_AUDIO_CHANNELS: usize = 8;
const MAX_PACKET_FRAMES: u64 = 1_048_576;

impl MediaProcessor {
    /// Transcribe a video/audio source through managed cloud ASR for paid
    /// sessions, or through the bundled local Whisper small worker otherwise.
    pub async fn transcribe_audio(&self, source: &str, referer: &str) -> Result<String> {
        let result = self.transcribe_audio_result(source, referer).await?;
        Ok(result["text"].as_str().unwrap_or_default().to_string())
    }

    pub(crate) async fn transcribe_audio_result(&self, source: &str, referer: &str) -> Result<serde_json::Value> {
        let t0 = Instant::now();
        let result = if let Some(remote) = &self.remote_asr {
            self.transcribe_hosted_audio(remote, source, referer).await
        } else {
            self.transcribe_audio_inner(source, referer).await.map(|text| serde_json::json!({
                "text": text, "segments": [], "timestamps_available": false,
            }))
        };
        self.timing.record("asr_transcribe", t0.elapsed());
        result
    }

    async fn transcribe_hosted_audio(&self, remote: &crate::media::RemoteAsrConfig,
        source: &str, referer: &str) -> Result<serde_json::Value> {
        if !self.config.use_cloud_asr {
            anyhow::bail!("video transcription is disabled for this media request");
        }
        let source = self.local_audio_source(source, referer).await?;
        let (wav, duration) = self.extract_audio_wav(&source).await?;
        let result = if duration >= self.config.max_audio_seconds as f64 {
            // The shared decoder caps native previews. A hosted full-transcript
            // request must not silently label that clipped preview as complete.
            Err(anyhow::anyhow!("video reaches the transcription duration limit; full transcription is unavailable"))
        } else {
            remote.transcribe(&wav).await
        };
        let _ = tokio::fs::remove_file(&wav).await;
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
        let (wav, duration) = self.extract_audio_wav(&source_path).await?;
        let duration_s = duration.ceil() as i64;
        let result = match crate::cloud::transcribe_audio_file(
            &wav,
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

    /// Decode the source's AAC track into a mono PCM WAV capped at
    /// `max_audio_seconds`. WAV avoids provider-specific raw AAC handling while
    /// 16 kHz mono keeps the upload bounded for speech recognition.
    async fn extract_audio_wav(&self, source_path: &Path) -> Result<(PathBuf, f64)> {
        let out = self.audio_output_path(source_path)?;
        let source = source_path.to_path_buf();
        let target = out.clone();
        let max_seconds = self.config.max_audio_seconds;
        let duration = tokio::task::spawn_blocking(move || {
            decode_aac_to_pcm_wav(&source, &target, max_seconds, false)
        })
        .await
        .context("audio decode task panicked")??;
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
        // Parallel hosted video requests must never overwrite one another's
        // provider input. Desktop retains its existing stable preview path.
        Ok(dir.join(if self.remote_asr.is_some() {
            format!("audio-{}.wav", uuid::Uuid::new_v4())
        } else {
            "audio.wav".into()
        }))
    }
}

/// Decode an owned archived video without downloading it or running ASR.
/// The output must be new. Never present a capped or corrupt decode as a full
/// transcript input; callers own cleanup of their private temporary directory.
pub fn extract_audio_wav_file(source: &Path, target: &Path, max_seconds: u64) -> Result<f64> {
    if !(1..=1200).contains(&max_seconds) {
        anyhow::bail!("archived audio duration limit must be between 1 and 1200 seconds");
    }
    let duration = decode_aac_to_pcm_wav(source, target, max_seconds, true)?;
    if duration >= max_seconds as f64 {
        anyhow::bail!("video reaches the duration limit; full audio extraction is unavailable");
    }
    Ok(duration)
}

/// Decode the first AAC track of `source` into a mono PCM WAV, keeping at most
/// `max_seconds` of audio. Returns the written duration in seconds.
fn decode_aac_to_pcm_wav(
    source: &Path,
    target: &Path,
    max_seconds: u64,
    exclusive: bool,
) -> Result<f64> {
    if max_seconds == 0 {
        anyhow::bail!(MediaUnavailable(
            "managed audio duration limit must be positive".into()
        ));
    }
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
    let expected_duration = track
        .codec_params
        .time_base
        .zip(track.codec_params.n_frames)
        .map(|(base, frames)| {
            let time = base.calc_time(frames);
            time.seconds as f64 + time.frac
        });
    if exclusive && expected_duration.is_some_and(|duration| duration >= max_seconds as f64) {
        anyhow::bail!("video reaches the duration limit; full audio extraction is unavailable");
    }
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .context("failed to initialize the AAC decoder")?;
    let mut writer = None;
    let mut resampler = None;
    let mut decoded_sample_rate = None;
    let mut written_samples = 0u64;
    let mut max_samples = 0u64;
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(SymphoniaError::ResetRequired) => {
                anyhow::bail!(MediaUnavailable(format!(
                    "AAC stream changed format in {}",
                    source.display()
                )))
            }
            Err(err) => return Err(err.into()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(_)) if !exclusive => continue,
            Err(err) => return Err(err.into()),
        };
        let spec = *decoded.spec();
        if !(8_000..=384_000).contains(&spec.rate) {
            anyhow::bail!(MediaUnavailable(format!(
                "unsupported audio sample rate {} Hz",
                spec.rate
            )));
        }
        let channels = spec.channels.count();
        if !(1..=MAX_AUDIO_CHANNELS).contains(&channels) {
            anyhow::bail!(MediaUnavailable(format!(
                "unsupported audio channel count {channels}"
            )));
        }
        if decoded.capacity() as u64 > MAX_PACKET_FRAMES {
            anyhow::bail!(MediaUnavailable(format!(
                "decoded audio packet is too large: {} frames",
                decoded.capacity()
            )));
        }
        if decoded_sample_rate.is_some_and(|rate| rate != spec.rate) {
            anyhow::bail!(MediaUnavailable(format!(
                "audio sample rate changed in {}",
                source.display()
            )));
        }
        if decoded_sample_rate.is_none() {
            let target_rate = spec.rate.min(CLOUD_ASR_SAMPLE_RATE);
            decoded_sample_rate = Some(spec.rate);
            max_samples = max_seconds.saturating_mul(u64::from(target_rate));
            writer = Some(PcmWavWriter::create(target, target_rate, exclusive)?);
            resampler = Some(LinearResampler::new(spec.rate, target_rate));
        }

        let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buffer.copy_interleaved_ref(decoded);
        let mono = buffer
            .samples()
            .chunks(channels)
            .map(|frame| frame.iter().copied().sum::<f32>() / frame.len() as f32);
        let remaining = max_samples.saturating_sub(written_samples) as usize;
        let samples = resampler
            .as_mut()
            .expect("resampler is initialized with the first decoded packet")
            .push(mono, remaining);
        writer
            .as_mut()
            .expect("WAV writer is initialized with the first decoded packet")
            .write_samples(&samples)?;
        written_samples += samples.len() as u64;
        if written_samples >= max_samples {
            break;
        }
    }

    if written_samples < max_samples {
        if let Some(resampler) = resampler.as_mut() {
            let remaining = max_samples.saturating_sub(written_samples) as usize;
            let samples = resampler.finish(remaining);
            writer
                .as_mut()
                .expect("WAV writer exists when the resampler exists")
                .write_samples(&samples)?;
            written_samples += samples.len() as u64;
        }
    }
    let Some(writer) = writer else {
        anyhow::bail!(MediaUnavailable(format!(
            "AAC track in {} contains no decodable audio",
            source.display()
        )));
    };
    writer.finish()?;
    let sample_rate = decoded_sample_rate
        .expect("a decoded sample rate exists when the WAV writer exists")
        .min(CLOUD_ASR_SAMPLE_RATE);
    let duration = written_samples as f64 / f64::from(sample_rate);
    if exclusive && expected_duration.is_some_and(|expected| (duration - expected).abs() > 0.25) {
        anyhow::bail!("decoded audio does not cover the container's complete audio track");
    }
    Ok(duration)
}

struct LinearResampler {
    step: f64,
    position: f64,
    buffered: Vec<f32>,
}

impl LinearResampler {
    fn new(source_rate: u32, target_rate: u32) -> Self {
        Self {
            step: f64::from(source_rate) / f64::from(target_rate),
            position: 0.0,
            buffered: Vec::new(),
        }
    }

    fn push(&mut self, samples: impl IntoIterator<Item = f32>, limit: usize) -> Vec<f32> {
        self.buffered.extend(samples);
        self.take(limit, false)
    }

    fn finish(&mut self, limit: usize) -> Vec<f32> {
        self.take(limit, true)
    }

    fn take(&mut self, limit: usize, final_chunk: bool) -> Vec<f32> {
        let mut output = Vec::with_capacity(limit.min(self.buffered.len()));
        while output.len() < limit && self.position < self.buffered.len() as f64 {
            let left = self.position.floor() as usize;
            let right = left.saturating_add(1);
            if right >= self.buffered.len() && !final_chunk {
                break;
            }
            let fraction = (self.position - left as f64) as f32;
            let a = self.buffered[left];
            let b = self.buffered.get(right).copied().unwrap_or(a);
            output.push(a + (b - a) * fraction);
            self.position += self.step;
        }
        let consumed = (self.position.floor() as usize).min(self.buffered.len());
        if consumed > 0 {
            self.buffered.drain(..consumed);
            self.position -= consumed as f64;
        }
        output
    }
}

struct PcmWavWriter {
    writer: std::io::BufWriter<std::fs::File>,
    samples_written: u64,
}

impl PcmWavWriter {
    fn create(target: &Path, sample_rate: u32, exclusive: bool) -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(!exclusive)
            .truncate(!exclusive)
            .create_new(exclusive)
            .open(target)
            .with_context(|| format!("failed to create {}", target.display()))?;
        let mut writer = std::io::BufWriter::new(file);
        writer.write_all(b"RIFF")?;
        writer.write_all(&36u32.to_le_bytes())?;
        writer.write_all(b"WAVEfmt ")?;
        writer.write_all(&16u32.to_le_bytes())?;
        writer.write_all(&1u16.to_le_bytes())?;
        writer.write_all(&1u16.to_le_bytes())?;
        writer.write_all(&sample_rate.to_le_bytes())?;
        writer.write_all(&sample_rate.saturating_mul(2).to_le_bytes())?;
        writer.write_all(&2u16.to_le_bytes())?;
        writer.write_all(&16u16.to_le_bytes())?;
        writer.write_all(b"data")?;
        writer.write_all(&0u32.to_le_bytes())?;
        Ok(Self {
            writer,
            samples_written: 0,
        })
    }

    fn write_samples(&mut self, samples: &[f32]) -> Result<()> {
        for sample in samples {
            let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
            self.writer.write_all(&value.to_le_bytes())?;
        }
        self.samples_written = self.samples_written.saturating_add(samples.len() as u64);
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        let data_bytes = self
            .samples_written
            .checked_mul(2)
            .and_then(|value| u32::try_from(value).ok())
            .context("decoded audio is too large for a WAV file")?;
        self.writer.seek(SeekFrom::Start(4))?;
        self.writer
            .write_all(&data_bytes.saturating_add(36).to_le_bytes())?;
        self.writer.seek(SeekFrom::Start(40))?;
        self.writer.write_all(&data_bytes.to_le_bytes())?;
        self.writer.flush()?;
        Ok(())
    }
}
