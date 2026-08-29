use std::collections::{HashSet, VecDeque};
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
    cancelled_runs: Mutex<CancelledRuns>,
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
            cancelled_runs: Mutex::new(CancelledRuns::default()),
            in_flight_videos: Mutex::new(HashSet::new()),
        }
    })
}

/// Allocate a unique generation for one user turn. Parallel turns keep their
/// own background media work; cancellation is scoped by run directory.
pub fn begin_background_media_generation() -> u64 {
    let generation = state().generation.fetch_add(1, Ordering::AcqRel) + 1;
    state().generation_tx.send_replace(generation);
    generation
}

pub fn current_background_media_generation() -> u64 {
    state().generation.load(Ordering::Acquire)
}

pub fn cancel_background_media_for_run(run_dir: &str) {
    let run_dir = run_dir.trim();
    if run_dir.is_empty() {
        return;
    }
    if let Ok(mut cancelled) = state().cancelled_runs.lock() {
        cancelled.insert(run_dir);
    }
    // Wake generation watchers too; their cancellation predicate also checks
    // the persistent run set, so subscribers cannot miss this targeted signal.
    let signal = state().generation.fetch_add(1, Ordering::AcqRel) + 1;
    state().generation_tx.send_replace(signal);
}

pub(crate) fn background_media_run_is_cancelled(run_dir: &str) -> bool {
    state()
        .cancelled_runs
        .lock()
        .is_ok_and(|cancelled| cancelled.contains(run_dir))
}

/// Run dirs whose background media work has been called off. Cancellation is
/// per run now that runs go in parallel, so this can no longer be cleared at
/// the start of each turn; it is bounded instead, oldest first. The bound is
/// far above the number of runs that could still have downloads in flight, so
/// an evicted entry can only belong to a run that finished long ago.
#[derive(Default)]
struct CancelledRuns {
    order: VecDeque<String>,
    members: HashSet<String>,
}

impl CancelledRuns {
    const MAX: usize = 512;

    fn insert(&mut self, run_dir: &str) {
        if !self.members.insert(run_dir.to_string()) {
            return;
        }
        self.order.push_back(run_dir.to_string());
        while self.order.len() > Self::MAX {
            if let Some(evicted) = self.order.pop_front() {
                self.members.remove(&evicted);
            }
        }
    }

    fn contains(&self, run_dir: &str) -> bool {
        self.members.contains(run_dir)
    }
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
    run_dir: &str,
    receiver: &mut watch::Receiver<u64>,
) {
    loop {
        if background_media_run_is_cancelled(run_dir) {
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

#[cfg(test)]
mod tests {
    use super::CancelledRuns;

    #[test]
    fn cancelled_runs_stay_bounded_and_evict_the_oldest() {
        let mut cancelled = CancelledRuns::default();
        for index in 0..(CancelledRuns::MAX + 2) {
            cancelled.insert(&format!("run-{index}"));
        }

        assert_eq!(cancelled.members.len(), CancelledRuns::MAX);
        assert!(!cancelled.contains("run-0"));
        assert!(!cancelled.contains("run-1"));
        assert!(cancelled.contains("run-2"));
        assert!(cancelled.contains(&format!("run-{}", CancelledRuns::MAX + 1)));
    }

    #[test]
    fn cancelling_the_same_run_twice_does_not_grow_the_set() {
        let mut cancelled = CancelledRuns::default();
        cancelled.insert("run-a");
        cancelled.insert("run-a");

        assert_eq!(cancelled.order.len(), 1);
        assert!(cancelled.contains("run-a"));
    }
}
