use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use tokio::sync::{broadcast, watch, Semaphore};

#[derive(Debug, Clone, Serialize)]
pub struct BackgroundMediaEvent {
    pub run_dir: String,
    pub note_id: String,
}

struct BackgroundMediaState {
    generation: AtomicU64,
    generation_tx: watch::Sender<u64>,
    events_tx: broadcast::Sender<BackgroundMediaEvent>,
    cancelled_runs: Mutex<HashSet<String>>,
    in_flight_videos: Mutex<HashSet<String>>,
}

fn state() -> &'static BackgroundMediaState {
    static STATE: OnceLock<BackgroundMediaState> = OnceLock::new();
    STATE.get_or_init(|| {
        let (generation_tx, _) = watch::channel(1);
        let (events_tx, _) = broadcast::channel(128);
        BackgroundMediaState {
            generation: AtomicU64::new(1),
            generation_tx,
            events_tx,
            cancelled_runs: Mutex::new(HashSet::new()),
            in_flight_videos: Mutex::new(HashSet::new()),
        }
    })
}

/// Start a new user turn and cancel every unfinished background media fetch
/// from older turns. The returned generation belongs on that turn's
/// `ToolContext`, so a queued/older agent cannot enqueue work after the user
/// has already moved on.
pub fn begin_background_media_generation() -> u64 {
    let generation = state().generation.fetch_add(1, Ordering::AcqRel) + 1;
    if let Ok(mut cancelled) = state().cancelled_runs.lock() {
        cancelled.clear();
    }
    state().generation_tx.send_replace(generation);
    generation
}

pub fn current_background_media_generation() -> u64 {
    state().generation.load(Ordering::Acquire)
}

pub fn background_media_generation_is_current(generation: u64) -> bool {
    current_background_media_generation() == generation
}

pub fn cancel_background_media_for_run(run_dir: &str) {
    let run_dir = run_dir.trim();
    if run_dir.is_empty() {
        return;
    }
    if let Ok(mut cancelled) = state().cancelled_runs.lock() {
        cancelled.insert(run_dir.to_string());
    }
    // Wake generation watchers too; their cancellation predicate also checks
    // the persistent run set, so subscribers cannot miss this targeted signal.
    state()
        .generation_tx
        .send_replace(current_background_media_generation());
}

pub(crate) fn background_media_run_is_cancelled(run_dir: &str) -> bool {
    state()
        .cancelled_runs
        .lock()
        .is_ok_and(|cancelled| cancelled.contains(run_dir))
}

pub(crate) fn subscribe_background_media_cancellation() -> watch::Receiver<u64> {
    state().generation_tx.subscribe()
}

pub(crate) fn background_video_download_semaphore() -> Arc<Semaphore> {
    static SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEMAPHORE
        .get_or_init(|| Arc::new(Semaphore::new(3)))
        .clone()
}

pub(crate) struct BackgroundVideoReservation {
    key: String,
}

impl Drop for BackgroundVideoReservation {
    fn drop(&mut self) {
        if let Ok(mut in_flight) = state().in_flight_videos.lock() {
            in_flight.remove(&self.key);
        }
    }
}

pub(crate) fn reserve_background_video_download(
    generation: u64,
    run_dir: &str,
    note_id: &str,
) -> Option<BackgroundVideoReservation> {
    let key = format!("{generation}\0{run_dir}\0{note_id}");
    let mut in_flight = state().in_flight_videos.lock().ok()?;
    if !in_flight.insert(key.clone()) {
        return None;
    }
    Some(BackgroundVideoReservation { key })
}

pub(crate) async fn wait_for_background_media_cancellation(
    generation: u64,
    run_dir: &str,
    receiver: &mut watch::Receiver<u64>,
) {
    loop {
        if *receiver.borrow() != generation || background_media_run_is_cancelled(run_dir) {
            return;
        }
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

pub fn subscribe_background_media_events() -> broadcast::Receiver<BackgroundMediaEvent> {
    state().events_tx.subscribe()
}

pub(crate) fn emit_background_media_event(event: BackgroundMediaEvent) {
    let _ = state().events_tx.send(event);
}
