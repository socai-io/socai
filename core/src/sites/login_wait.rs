use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

struct LoginResumeEntry {
    generation: u64,
    sender: tokio::sync::watch::Sender<u64>,
    subscribers: usize,
}

/// One human-ready generation per active run. Hosted frontends use this to
/// wake a login waiter after the user releases browser control; every waiter
/// still verifies the live page before it resumes.
static LOGIN_RESUME_SIGNALS: OnceLock<Mutex<HashMap<String, LoginResumeEntry>>> = OnceLock::new();
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

fn login_resume_signals() -> &'static Mutex<HashMap<String, LoginResumeEntry>> {
    LOGIN_RESUME_SIGNALS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub struct LoginResumeSubscription {
    run_id: String,
    generation: u64,
    receiver: tokio::sync::watch::Receiver<u64>,
}

impl LoginResumeSubscription {
    pub fn was_signaled(&mut self) -> bool {
        *self.receiver.borrow_and_update() > 0
    }

    pub async fn changed(&mut self) -> Result<(), tokio::sync::watch::error::RecvError> {
        self.receiver.changed().await
    }
}

impl Drop for LoginResumeSubscription {
    fn drop(&mut self) {
        let mut signals = login_resume_signals()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(entry) = signals
            .get_mut(&self.run_id)
            .filter(|entry| entry.generation == self.generation)
        {
            if entry.subscribers <= 1 {
                signals.remove(&self.run_id);
            } else {
                entry.subscribers -= 1;
            }
        }
    }
}

pub fn login_resume_receiver(run_id: &str) -> LoginResumeSubscription {
    let mut signals = login_resume_signals()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let entry = signals
        .entry(run_id.to_owned())
        .or_insert_with(|| LoginResumeEntry {
            generation: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
            sender: tokio::sync::watch::channel(0).0,
            subscribers: 0,
        });
    entry.subscribers += 1;
    LoginResumeSubscription {
        run_id: run_id.to_owned(),
        generation: entry.generation,
        receiver: entry.sender.subscribe(),
    }
}

/// Install a zero-generation gate before publishing its tool-call event. A
/// fast human-ready click may arrive before the execution future subscribes.
pub fn prepare_login_resume(run_id: &str) {
    let mut signals = login_resume_signals()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    signals
        .entry(run_id.to_owned())
        .or_insert_with(|| LoginResumeEntry {
            generation: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
            sender: tokio::sync::watch::channel(0).0,
            subscribers: 0,
        });
}

pub fn signal_login_resume(run_id: &str) {
    let mut signals = login_resume_signals()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Ending an ordinary proactive pause must not pre-confirm a future gate.
    let Some(entry) = signals.get_mut(run_id) else {
        return;
    };
    let next = entry.sender.borrow().saturating_add(1);
    entry.sender.send_replace(next);
}

pub fn clear_login_resume(run_id: &str) {
    login_resume_signals()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(run_id);
}

pub fn interactive_remote_login_enabled() -> bool {
    std::env::var("SOCAI_REMOTE_LOGIN_MODE")
        .ok()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("interactive"))
}
