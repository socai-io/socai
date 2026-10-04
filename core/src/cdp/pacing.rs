//! Per-platform browser pacing, shared by pages on the same platform.
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, OwnedMutexGuard};
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserActionSpeed {
    #[default]
    Instant,
    Normal,
    Slow,
}

impl BrowserActionSpeed {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "instant" => Ok(Self::Instant),
            "normal" => Ok(Self::Normal),
            "slow" => Ok(Self::Slow),
            _ => anyhow::bail!(
                "invalid browser action speed {value:?}; expected instant, normal, or slow"
            ),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Instant => "instant",
            Self::Normal => "normal",
            Self::Slow => "slow",
        }
    }

    pub(crate) fn bounds_ms(self) -> (u64, u64) {
        match self {
            Self::Instant => (0, 0),
            Self::Normal => (1_000, 3_000),
            Self::Slow => (3_000, 6_000),
        }
    }

    fn delay(self) -> Duration {
        let (min, max) = self.bounds_ms();
        let random = uuid::Uuid::new_v4().as_u128() as u64;
        Duration::from_millis(min + random % (max - min + 1))
    }
}

type ActionLane = Arc<Mutex<Option<Instant>>>;
static ACTIONS: Mutex<BTreeMap<String, ActionLane>> = Mutex::const_new(BTreeMap::new());

/// Holds one platform's action lane through execution and the following cooldown.
/// Cancellation records the cooldown before releasing the lane, so a cancelled
/// sleep cannot let the next action skip its interval. Instant keeps the old
/// concurrency and does not acquire the lane or sleep.
pub(crate) struct BrowserAction {
    state: Option<OwnedMutexGuard<Option<Instant>>>,
    pub(crate) speed: BrowserActionSpeed,
    cooling: bool,
}

impl BrowserAction {
    pub(crate) async fn begin(url: &str) -> anyhow::Result<Self> {
        // Read on each action so CLI changes also reach an already-running daemon.
        let site_id = crate::sites::site_skills_for_url(url)?
            .into_iter()
            .next()
            .map(|skill| skill.id)
            .unwrap_or_else(|| "other".into());
        let mut speed = crate::config::load_config()?
            .browser
            .action_speed_for(&site_id);
        let state = if speed == BrowserActionSpeed::Instant {
            None
        } else {
            let lane = ACTIONS
                .lock()
                .await
                .entry(site_id.clone())
                .or_default()
                .clone();
            let state = lane.lock_owned().await;
            // A queued action observes edits made while another tab was busy.
            speed = crate::config::load_config()?
                .browser
                .action_speed_for(&site_id);
            if speed == BrowserActionSpeed::Instant {
                None
            } else {
                if let Some(deadline) = *state {
                    tokio::time::sleep_until(deadline).await;
                }
                Some(state)
            }
        };
        Ok(Self {
            state,
            speed,
            cooling: false,
        })
    }

    pub(crate) async fn finish(mut self) {
        if let Some(state) = &mut self.state {
            let deadline = Instant::now() + self.speed.delay();
            **state = Some(deadline);
            self.cooling = true;
            tokio::time::sleep_until(deadline).await;
        }
    }
}

impl Drop for BrowserAction {
    fn drop(&mut self) {
        if !self.cooling {
            if let Some(state) = &mut self.state {
                **state = Some(Instant::now() + self.speed.delay());
            }
        }
    }
}
