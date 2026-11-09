//! Lossless joining of the separate MP4 tracks supplied by Douyin's player.
use anyhow::{bail, ensure, Context, Result};
use mp4io::{
    CodecConfig, InputSample, Limits, Mp4, SampleEntry, Strictness, Track, TrackKind, TrackParams,
    Writer, WriterConfig,
};

fn parse(bytes: &[u8]) -> Result<Mp4<'_>> {
    Mp4::parse_with_limits(
        bytes,
        Strictness::Normal,
        Limits::default().with_max_index_bytes(16 << 20),
    )
    .context("invalid source MP4")
}

pub(crate) fn mp4_has_audio(bytes: &[u8]) -> Result<bool> {
    let source = parse(bytes)?;
    ensure!(
        source.tracks().iter().any(|t| t.kind() == TrackKind::Video),
        "source MP4 has no video track"
    );
    Ok(source
        .tracks()
        .iter()
        .any(|t| t.kind() == TrackKind::Audio && t.sample_count() > 0))
}

fn params(track: &Track<'_>, source_timescale: u32, movie_timescale: u32) -> Result<TrackParams> {
    ensure!(
        track.timescale() > 0 && source_timescale > 0,
        "source track has no timescale"
    );
    ensure!(
        track.sample_entries().len() == 1 && track.sample_count() > 0,
        "unsupported source sample entries"
    );
    let mut result = match track.sample_entry() {
        Some(SampleEntry::Video(v)) => {
            let p = TrackParams::video(v.codec, v.width, v.height);
            match &v.config {
                CodecConfig::Avc(c) => p.avc_config(c.raw.clone()),
                CodecConfig::Hevc(c) => p.hevc_config(c.raw.clone()),
                _ => bail!("unsupported split-video codec"),
            }
        }
        Some(SampleEntry::Audio(a)) => match &a.config {
            CodecConfig::Aac(c) => {
                TrackParams::audio(a.codec, a.channels, c.sample_rate).aac_config_from(c)
            }
            _ => bail!("unsupported split-audio codec"),
        },
        _ => bail!("unsupported source track"),
    };
    let mut edits = track.edits().to_vec();
    for edit in &mut edits {
        edit.segment_duration = edit
            .segment_duration
            .checked_mul(u64::from(movie_timescale))
            .context("edit duration overflow")?
            / u64::from(source_timescale);
    }
    result = result
        .timescale(track.timescale())
        .language(track.language())
        .edit_list(edits);
    Ok(result)
}

pub(crate) fn mux_split_audio(video: &[u8], audio: &[u8], limit: usize) -> Result<Vec<u8>> {
    ensure!(
        video
            .len()
            .checked_add(audio.len())
            .is_some_and(|n| n <= limit),
        "combined source media exceeds download limit"
    );
    let video_source = parse(video)?;
    let audio_source = parse(audio)?;
    ensure!(
        video_source.tracks().len() == 1 && audio_source.tracks().len() == 1,
        "split sources must contain one track each"
    );
    let v = &video_source.tracks()[0];
    let a = &audio_source.tracks()[0];
    ensure!(
        v.kind() == TrackKind::Video && a.kind() == TrackKind::Audio,
        "source tracks are not video and audio"
    );
    let timescale = video_source.timescale();
    ensure!(timescale > 0, "source movie has no timescale");
    let mut writer = Writer::new(WriterConfig {
        timescale,
        ..WriterConfig::default()
    });
    let ids = [
        writer.add_track(params(v, timescale, timescale)?),
        writer.add_track(params(a, audio_source.timescale(), timescale)?),
    ];
    let tracks = [v, a];
    let mut positions = [0usize; 2];
    let mut ends = [0u64; 2];
    let mut payload_bytes = 0usize;
    while positions[0] < v.sample_count() || positions[1] < a.sample_count() {
        // Interleave by decode time, keeping playback possible without reading all video first.
        let index = if positions[0] == v.sample_count() {
            1
        } else if positions[1] == a.sample_count() {
            0
        } else if u128::from(ends[0]) * u128::from(a.timescale())
            <= u128::from(ends[1]) * u128::from(v.timescale())
        {
            0
        } else {
            1
        };
        let track = tracks[index];
        let sample = track
            .sample(positions[index])?
            .context("missing source sample")?;
        ensure!(
            sample.dts == ends[index],
            "non-contiguous source decode timing"
        );
        let offset = i32::try_from(
            sample
                .cts
                .checked_sub(i64::try_from(sample.dts)?)
                .context("composition timestamp overflow")?,
        )?;
        payload_bytes = payload_bytes
            .checked_add(sample.data.len())
            .context("media size overflow")?;
        ensure!(
            payload_bytes <= limit,
            "muxed payload exceeds download limit"
        );
        writer.write_sample(
            ids[index],
            InputSample {
                data: sample.data,
                duration: sample.duration,
                composition_offset: offset,
                is_sync: sample.is_sync,
            },
        )?;
        ends[index] = sample
            .dts
            .checked_add(u64::from(sample.duration))
            .context("media duration overflow")?;
        positions[index] += 1;
    }
    let video_seconds = ends[0] as f64 / f64::from(v.timescale());
    let audio_seconds = ends[1] as f64 / f64::from(a.timescale());
    ensure!(
        (video_seconds - audio_seconds).abs() <= 1.0,
        "split audio and video durations do not match"
    );
    let output = writer.finalize()?;
    ensure!(output.len() <= limit, "muxed MP4 exceeds download limit");
    Ok(output)
}
