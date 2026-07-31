//! Optional local/media processing used by site runtimes.
//!
//! Nothing here shells out to external media tools: video covers come from
//! their own CDN URL, audio transcription is cloud-only (socai pro takes the
//! demuxed aac as-is), and OCR runs in-process. Site runtimes can opt into
//! this crate for heavier media enrichment while keeping plain DOM extraction
//! fast and portable.

mod audio;
mod background;
mod common;
mod image;
mod md5;
mod ocr;
mod processor;
mod timing;
mod video;

pub use self::background::{
    background_media_generation_is_current, begin_background_media_generation,
    cancel_background_media_for_run, current_background_media_generation,
    subscribe_background_media_events, BackgroundMediaEvent,
};
pub(crate) use self::background::{
    background_media_run_is_cancelled, background_video_download_semaphore,
    emit_background_media_event, reserve_background_video_download,
    subscribe_background_media_cancellation, wait_for_background_media_cancellation,
};
pub use self::common::{MediaConfig, MediaUnavailable};
pub use self::ocr::diagnostics as ocr_diagnostics;
pub(crate) use self::ocr::ocr_images_bytes;
pub use self::ocr::warm_up as ocr_warm_up;
pub use self::processor::MediaProcessor;
pub use self::timing::{timing_delta, TimingRecord, TimingSnapshot};
