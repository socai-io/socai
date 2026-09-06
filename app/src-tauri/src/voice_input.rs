use std::io::Write;
use std::time::Duration;

use base64::Engine;
use serde::Serialize;
use tauri::Emitter;

const MAX_WAV_BYTES: usize = 4 * 1024 * 1024;
const EXPECTED_SAMPLE_RATE: u32 = 16_000;
const MAX_RECORDING_SECONDS: u64 = 120;

#[derive(Serialize)]
pub struct VoiceInputStatus {
    ready: bool,
    route: String,
    state: String,
    local_state: String,
    downloaded_bytes: u64,
    total_bytes: u64,
    error: Option<String>,
}

#[derive(Serialize)]
pub struct VoiceTranscript {
    text: String,
}

#[tauri::command]
pub async fn voice_input_status() -> Result<VoiceInputStatus, String> {
    let access = socai_core::cloud::paid_asr_access().await;
    if access.as_ref().is_ok_and(|access| access.ready) {
        return Ok(VoiceInputStatus {
            ready: true,
            route: "cloud".into(),
            state: "cloud_ready".into(),
            local_state: "not_checked".into(),
            downloaded_bytes: 0,
            total_bytes: 0,
            error: None,
        });
    }

    let local = socai_core::media::local_asr_status()
        .await
        .map_err(|error| format!("failed to inspect local ASR: {error:#}"))?;
    // A missing or downloading model is still a usable local route: the first
    // transcription installs the fixed model and waits for it to become ready.
    // A previous setup error is retried on the next transcription as well.
    let local_ready = matches!(
        local.state.as_str(),
        "ready" | "model_missing" | "downloading" | "error"
    );
    let (state, error) = match access {
        Ok(access) if !access.logged_in => ("login_required", None),
        Ok(access) if !access.active_subscription => ("subscription_required", None),
        Ok(access) if access.balance_points <= 0 => ("credits_required", None),
        Ok(_) => ("local_only", None),
        Err(error) => ("billing_unavailable", Some(format!("{error:#}"))),
    };
    Ok(VoiceInputStatus {
        ready: local_ready,
        route: "local".into(),
        state: state.into(),
        local_state: local.state,
        downloaded_bytes: local.downloaded_bytes,
        total_bytes: local.total_bytes,
        error: error.or(local.error),
    })
}

#[tauri::command]
pub async fn voice_input_local_status() -> Result<socai_core::media::LocalAsrStatus, String> {
    socai_core::media::local_asr_status()
        .await
        .map_err(|error| format!("failed to inspect local ASR: {error:#}"))
}

#[tauri::command]
pub async fn voice_input_transcribe(
    app: tauri::AppHandle,
    audio_base64: String,
) -> Result<VoiceTranscript, String> {
    if audio_base64.len() > MAX_WAV_BYTES.saturating_mul(4).div_ceil(3) + 16 {
        return Err("recorded audio is too large".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(audio_base64.as_bytes())
        .map_err(|_| "recorded audio is not valid base64".to_string())?;
    let duration_s = validate_pcm_wav(&bytes)?;

    let mut audio = tempfile::Builder::new()
        .prefix("socai-voice-")
        .suffix(".wav")
        .tempfile()
        .map_err(|error| format!("could not create temporary voice recording: {error}"))?;
    audio
        .write_all(&bytes)
        .map_err(|error| format!("could not save temporary voice recording: {error}"))?;
    audio
        .flush()
        .map_err(|error| format!("could not flush temporary voice recording: {error}"))?;

    let cloud_ready = socai_core::cloud::paid_asr_access()
        .await
        .is_ok_and(|access| access.ready);
    let selected_route = if cloud_ready { "cloud" } else { "local" };
    let _ = app.emit("voice_input:route", selected_route);
    let text = if cloud_ready {
        let client_task_id = format!("voice-{}", uuid::Uuid::new_v4());
        let result = socai_core::cloud::transcribe_audio_file(
            audio.path(),
            duration_s as i64,
            Duration::from_secs(180),
            Some(&client_task_id),
        )
        .await;
        if result
            .as_ref()
            .err()
            .is_some_and(socai_core::cloud::cloud_asr_access_rejected)
        {
            let _ = app.emit("voice_input:route", "local");
            transcribe_local_voice(audio.path()).await?
        } else {
            let final_status = if result.is_ok() {
                "completed"
            } else {
                "failed"
            };
            // Settlement is idempotent. If these retries all hit a transient
            // network error, the durable server-side task remains available for
            // stale-task recovery; never discard a transcript the provider
            // already produced.
            let _settlement =
                crate::commands::settle_hosted_task_with_retry(&client_task_id, final_status).await;
            result
                .map(|result| result.transcript)
                .map_err(|error| format!("{error:#}"))?
        }
    } else {
        transcribe_local_voice(audio.path()).await?
    };
    Ok(VoiceTranscript {
        text: text.trim().to_string(),
    })
}

async fn transcribe_local_voice(path: &std::path::Path) -> Result<String, String> {
    socai_core::media::transcribe_local_file_with_timeout(
        path,
        MAX_RECORDING_SECONDS,
        Duration::from_secs(1_800),
    )
    .await
    .map_err(|error| format!("{error:#}"))
}

fn validate_pcm_wav(bytes: &[u8]) -> Result<u64, String> {
    if bytes.len() < 44 || bytes.len() > MAX_WAV_BYTES {
        return Err("recorded audio has an invalid size".into());
    }
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err("recorded audio is not a WAV file".into());
    }
    let riff_bytes = u32::from_le_bytes(
        bytes[4..8]
            .try_into()
            .map_err(|_| "recorded WAV header is invalid")?,
    ) as usize;
    if riff_bytes.checked_add(8) != Some(bytes.len()) {
        return Err("recorded WAV length does not match its header".into());
    }

    let mut cursor = 12usize;
    let mut format = None;
    let mut data_bytes = None;
    while cursor.saturating_add(8) <= bytes.len() {
        let id = &bytes[cursor..cursor + 4];
        let size = u32::from_le_bytes(
            bytes[cursor + 4..cursor + 8]
                .try_into()
                .map_err(|_| "recorded WAV chunk is invalid")?,
        ) as usize;
        let start = cursor + 8;
        let end = start
            .checked_add(size)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| "recorded WAV chunk is truncated".to_string())?;
        if id == b"fmt " {
            if format.is_some() {
                return Err("recorded WAV has duplicate format chunks".into());
            }
            if size < 16 {
                return Err("recorded WAV format chunk is too short".into());
            }
            let audio_format = u16::from_le_bytes([bytes[start], bytes[start + 1]]);
            let channels = u16::from_le_bytes([bytes[start + 2], bytes[start + 3]]);
            let sample_rate = u32::from_le_bytes(
                bytes[start + 4..start + 8]
                    .try_into()
                    .map_err(|_| "recorded WAV format is invalid")?,
            );
            let byte_rate = u32::from_le_bytes(
                bytes[start + 8..start + 12]
                    .try_into()
                    .map_err(|_| "recorded WAV byte rate is invalid")?,
            );
            let block_align = u16::from_le_bytes([bytes[start + 12], bytes[start + 13]]);
            let bits_per_sample = u16::from_le_bytes([bytes[start + 14], bytes[start + 15]]);
            format = Some((
                audio_format,
                channels,
                sample_rate,
                byte_rate,
                block_align,
                bits_per_sample,
            ));
        } else if id == b"data" {
            if data_bytes.is_some() {
                return Err("recorded WAV has duplicate audio data chunks".into());
            }
            data_bytes = Some(size);
        }
        cursor = end.saturating_add(size % 2);
    }
    if cursor != bytes.len() {
        return Err("recorded WAV has trailing or incomplete chunk data".into());
    }

    let (audio_format, channels, sample_rate, byte_rate, block_align, bits_per_sample) =
        format.ok_or_else(|| "recorded WAV has no format chunk".to_string())?;
    if audio_format != 1
        || channels != 1
        || sample_rate != EXPECTED_SAMPLE_RATE
        || byte_rate != EXPECTED_SAMPLE_RATE * 2
        || block_align != 2
        || bits_per_sample != 16
    {
        return Err("recorded audio must be mono 16 kHz PCM WAV".into());
    }
    let data_bytes = data_bytes.ok_or_else(|| "recorded WAV has no audio data".to_string())?;
    if data_bytes % usize::from(block_align) != 0 {
        return Err("recorded WAV audio data is not sample-aligned".into());
    }
    let bytes_per_second = u64::from(sample_rate) * u64::from(channels) * 2;
    let duration_s = (data_bytes as u64).div_ceil(bytes_per_second);
    if duration_s == 0 {
        return Err("recorded audio is empty".into());
    }
    if duration_s > MAX_RECORDING_SECONDS {
        return Err("recorded audio is longer than two minutes".into());
    }
    Ok(duration_s)
}
