//! Logical execution budgets exclude acknowledged manual takeover pauses.
//!
//! This does not change wall-clock telemetry or freeze an already issued HTTP /
//! CDP request. Background tasks do not inherit Tokio task-local state; callers
//! that need a logical budget there must explicitly carry this clock.

use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

tokio::task_local! {
    static EXECUTION_CLOCK: ExecutionClock;
}

#[derive(Clone)]
pub struct ExecutionClock(Arc<Mutex<ClockState>>);

struct ClockState {
    started: Instant,
    paused_at: Option<Instant>,
    paused_total: Duration,
}

impl Default for ExecutionClock {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(ClockState {
            started: Instant::now(),
            paused_at: None,
            paused_total: Duration::ZERO,
        })))
    }
}

impl ExecutionClock {
    /// Entrypoints without pause controls keep ordinary elapsed-time behavior.
    pub fn current() -> Self {
        EXECUTION_CLOCK.try_with(Clone::clone).unwrap_or_default()
    }

    pub async fn scope<F: Future>(&self, execution: F) -> F::Output {
        EXECUTION_CLOCK.scope(self.clone(), execution).await
    }

    fn state(&self) -> MutexGuard<'_, ClockState> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn elapsed(&self) -> Duration {
        let state = self.state();
        let until = state.paused_at.unwrap_or_else(Instant::now);
        until
            .duration_since(state.started)
            .saturating_sub(state.paused_total)
    }

    /// Idempotent: a repeated pause must not move the pause boundary.
    pub fn pause(&self) {
        let mut state = self.state();
        state.paused_at.get_or_insert_with(Instant::now);
    }

    pub fn resume(&self) {
        let mut state = self.state();
        if let Some(paused_at) = state.paused_at.take() {
            state.paused_total += paused_at.elapsed();
        }
    }

    pub fn deadline_after(&self, budget: Duration) -> ExecutionDeadline {
        self.deadline_at(self.elapsed() + budget)
    }

    pub fn deadline_at(&self, expires_at: Duration) -> ExecutionDeadline {
        ExecutionDeadline {
            clock: self.clone(),
            expires_at,
        }
    }
}

#[derive(Clone)]
pub struct ExecutionDeadline {
    clock: ExecutionClock,
    expires_at: Duration,
}

impl ExecutionDeadline {
    pub fn after(budget: Duration) -> Self {
        ExecutionClock::current().deadline_after(budget)
    }

    pub fn remaining(&self) -> Duration {
        self.expires_at.saturating_sub(self.clock.elapsed())
    }

    pub fn is_elapsed(&self) -> bool {
        self.remaining().is_zero()
    }

    pub async fn wait(self) {
        loop {
            let remaining = self.remaining();
            if remaining.is_zero() {
                return;
            }
            // The wall timer may fire during a takeover. On the next poll,
            // recheck the logical budget rather than expiring immediately.
            tokio::time::sleep(remaining).await;
        }
    }
}
