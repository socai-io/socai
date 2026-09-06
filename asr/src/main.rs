use std::fs::File;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use sherpa_onnx::{
    LinearResampler, OfflineRecognizer, OfflineRecognizerConfig, OfflineWhisperModelConfig,
    VadModelConfig, VoiceActivityDetector,
};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

const PROTOCOL_VERSION: u32 = 1;
const TARGET_SAMPLE_RATE: i32 = 16_000;
const MAX_AUDIO_SECONDS: u64 = 60 * 60;
const MAX_AUDIO_CHANNELS: usize = 8;
const MAX_PACKET_FRAMES: u64 = 1_048_576;

#[derive(Deserialize)]
struct Request {
    protocol: u32,
    id: u64,
    path: String,
    max_seconds: u64,
}

#[derive(Serialize)]
struct Response {
    protocol: u32,
    id: u64,
    ok: bool,
    transcript: Option<String>,
    error: Option<String>,
}

struct ModelPaths {
    encoder: PathBuf,
    decoder: PathBuf,
    tokens: PathBuf,
    vad: PathBuf,
}

impl ModelPaths {
    fn new(root: &Path) -> Self {
        Self {
            encoder: root.join("small-encoder.int8.onnx"),
            decoder: root.join("small-decoder.int8.onnx"),
            tokens: root.join("small-tokens.txt"),
            vad: root.join("silero_vad.onnx"),
        }
    }
}

fn main() -> Result<()> {
    let model_dir = parse_model_dir()?;
    let paths = ModelPaths::new(&model_dir);
    let recognizer = create_recognizer(&paths)?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::BufWriter::new(std::io::stdout().lock());

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) if request.protocol == PROTOCOL_VERSION => {
                match transcribe_file(
                    Path::new(&request.path),
                    request.max_seconds,
                    &paths,
                    &recognizer,
                ) {
                    Ok(transcript) => Response {
                        protocol: PROTOCOL_VERSION,
                        id: request.id,
                        ok: true,
                        transcript: Some(transcript),
                        error: None,
                    },
                    Err(error) => Response {
                        protocol: PROTOCOL_VERSION,
                        id: request.id,
                        ok: false,
                        transcript: None,
                        error: Some(format!("{error:#}")),
                    },
                }
            }
            Ok(request) => Response {
                protocol: PROTOCOL_VERSION,
                id: request.id,
                ok: false,
                transcript: None,
                error: Some(format!(
                    "unsupported ASR protocol {}; expected {PROTOCOL_VERSION}",
                    request.protocol
                )),
            },
            Err(error) => Response {
                protocol: PROTOCOL_VERSION,
                id: 0,
                ok: false,
                transcript: None,
                error: Some(format!("invalid ASR request: {error}")),
            },
        };
        serde_json::to_writer(&mut stdout, &response)?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
    }
    Ok(())
}

fn parse_model_dir() -> Result<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    let mut serve = false;
    let mut model_dir = None;
    while let Some(arg) = args.next() {
        if arg == "--serve" {
            serve = true;
        } else if arg == "--model-dir" {
            model_dir = args.next().map(PathBuf::from);
        } else {
            anyhow::bail!("unknown argument: {}", PathBuf::from(arg).display());
        }
    }
    if !serve {
        anyhow::bail!("socai-asr is an internal worker and requires --serve");
    }
    model_dir.context("--model-dir is required")
}

fn transcribe_file(
    path: &Path,
    max_seconds: u64,
    paths: &ModelPaths,
    recognizer: &OfflineRecognizer,
) -> Result<String> {
    let samples = decode_audio_file(path, max_seconds)?;
    if samples.is_empty() {
        anyhow::bail!("{} contains no decoded audio samples", path.display());
    }

    let mut vad_config = VadModelConfig::default();
    vad_config.silero_vad.model = Some(paths.vad.display().to_string());
    vad_config.silero_vad.threshold = 0.5;
    vad_config.silero_vad.min_silence_duration = 0.2;
    vad_config.silero_vad.min_speech_duration = 0.2;
    vad_config.silero_vad.max_speech_duration = 20.0;
    vad_config.silero_vad.window_size = 512;
    vad_config.sample_rate = 16_000;
    vad_config.num_threads = 1;
    vad_config.provider = Some("cpu".into());
    let vad = VoiceActivityDetector::create(&vad_config, 30.0)
        .ok_or_else(|| anyhow!("failed to initialize local Silero VAD"))?;

    let mut transcripts = Vec::new();
    for chunk in samples.chunks(512) {
        vad.accept_waveform(chunk);
        decode_ready_segments(&vad, recognizer, &mut transcripts);
    }
    vad.flush();
    decode_ready_segments(&vad, recognizer, &mut transcripts);

    if transcripts.is_empty() && samples.len() <= 30 * 16_000 {
        decode_samples(recognizer, &samples, &mut transcripts);
    }
    let transcript = transcripts.join("\n").trim().to_string();
    if transcript.is_empty() {
        anyhow::bail!("local Whisper small found no speech in {}", path.display());
    }
    Ok(transcript)
}

fn create_recognizer(paths: &ModelPaths) -> Result<OfflineRecognizer> {
    let mut config = OfflineRecognizerConfig::default();
    config.model_config.whisper = OfflineWhisperModelConfig {
        encoder: Some(paths.encoder.display().to_string()),
        decoder: Some(paths.decoder.display().to_string()),
        task: Some("transcribe".into()),
        ..Default::default()
    };
    config.model_config.tokens = Some(paths.tokens.display().to_string());
    config.model_config.provider = Some("cpu".into());
    config.model_config.num_threads = std::thread::available_parallelism()
        .map(|count| count.get().clamp(2, 4) as i32)
        .unwrap_or(2);
    OfflineRecognizer::create(&config).ok_or_else(|| anyhow!("failed to load Whisper small model"))
}

fn decode_ready_segments(
    vad: &VoiceActivityDetector,
    recognizer: &OfflineRecognizer,
    transcripts: &mut Vec<String>,
) {
    while let Some(segment) = vad.front() {
        let samples = segment.samples().to_vec();
        drop(segment);
        vad.pop();
        decode_samples(recognizer, &samples, transcripts);
    }
}

fn decode_samples(recognizer: &OfflineRecognizer, samples: &[f32], transcripts: &mut Vec<String>) {
    if samples.is_empty() {
        return;
    }
    let stream = recognizer.create_stream();
    stream.accept_waveform(16_000, samples);
    recognizer.decode(&stream);
    if let Some(result) = stream.get_result() {
        let text = result.text.trim();
        if !text.is_empty() {
            transcripts.push(text.to_string());
        }
    }
}

fn decode_audio_file(path: &Path, max_seconds: u64) -> Result<Vec<f32>> {
    if !(1..=MAX_AUDIO_SECONDS).contains(&max_seconds) {
        anyhow::bail!("max_seconds must be between 1 and {MAX_AUDIO_SECONDS}, got {max_seconds}");
    }
    let max_samples = max_seconds
        .checked_mul(TARGET_SAMPLE_RATE as u64)
        .and_then(|value| usize::try_from(value).ok())
        .context("requested audio duration is too large")?;
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .with_context(|| format!("unsupported media container in {}", path.display()))?;
    let mut format = probed.format;
    let (track_id, mut decoder) = audio_decoder(&mut *format)
        .ok_or_else(|| anyhow!("no supported audio track in {}", path.display()))?;
    let mut output = Vec::with_capacity(max_samples.min(4 * 1024 * 1024));
    let mut sample_rate = None;
    let mut resampler = None;

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(SymphoniaError::ResetRequired) => {
                anyhow::bail!("audio stream changed format in {}", path.display())
            }
            Err(err) => return Err(err.into()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(err) => return Err(err.into()),
        };
        let spec = *decoded.spec();
        if !(8_000..=384_000).contains(&spec.rate) {
            anyhow::bail!("unsupported audio sample rate {} Hz", spec.rate);
        }
        let channels = spec.channels.count();
        if !(1..=MAX_AUDIO_CHANNELS).contains(&channels) {
            anyhow::bail!("unsupported audio channel count {channels}");
        }
        if decoded.capacity() as u64 > MAX_PACKET_FRAMES {
            anyhow::bail!(
                "decoded audio packet is too large: {} frames",
                decoded.capacity()
            );
        }
        let rate = spec.rate as i32;
        if sample_rate.is_some_and(|current| current != rate) {
            anyhow::bail!("audio sample rate changed in {}", path.display());
        }
        if sample_rate.is_none() {
            sample_rate = Some(rate);
            if rate != TARGET_SAMPLE_RATE {
                resampler = Some(
                    LinearResampler::create(rate, TARGET_SAMPLE_RATE).ok_or_else(|| {
                        anyhow!("failed to create {rate} Hz to {TARGET_SAMPLE_RATE} Hz resampler")
                    })?,
                );
            }
        }
        let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buffer.copy_interleaved_ref(decoded);
        let mut mono = Vec::with_capacity(buffer.samples().len() / channels);
        for frame in buffer.samples().chunks(channels) {
            mono.push(frame.iter().copied().sum::<f32>() / frame.len() as f32);
        }
        let chunk = match &resampler {
            Some(resampler) => resampler.resample(&mono, false),
            None => mono,
        };
        let remaining = max_samples.saturating_sub(output.len());
        output.extend(chunk.into_iter().take(remaining));
        if output.len() >= max_samples {
            return Ok(output);
        }
    }
    sample_rate.ok_or_else(|| anyhow!("{} contains no decoded audio", path.display()))?;
    if let Some(resampler) = resampler {
        let remaining = max_samples.saturating_sub(output.len());
        output.extend(resampler.resample(&[], true).into_iter().take(remaining));
    }
    Ok(output)
}

fn audio_decoder(
    format: &mut dyn symphonia::core::formats::FormatReader,
) -> Option<(u32, Box<dyn Decoder>)> {
    for track in format.tracks() {
        if let Ok(decoder) =
            symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())
        {
            return Some((track.id, decoder));
        }
    }
    None
}
