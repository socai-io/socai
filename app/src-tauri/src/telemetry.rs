use std::path::Path;

use serde_json::Value;
use socai_core::telemetry::{telemetry_enabled, Telemetry, TelemetrySource};

use crate::tasks::app_data_dir;

/// Desktop wrapper around the shared core `Telemetry`. Holds `None` when the user
/// opted out via `SOCAI_TELEMETRY=off`, so every call site stays a no-op without
/// branching. Cloneable so it can ride into spawned task futures and command
/// state alike.
#[derive(Clone)]
pub struct DesktopTelemetry(Option<Telemetry>);

#[allow(clippy::new_without_default)]
impl DesktopTelemetry {
    /// Build the desktop telemetry handle. Honors `SOCAI_TELEMETRY=off` as a
    /// master kill switch; otherwise writes identity/events under the same
    /// `~/.socai/app` data dir as `tasks.json`.
    pub fn new() -> Self {
        if !telemetry_enabled() {
            return Self(None);
        }
        Self(Some(Telemetry::new(
            &app_data_dir(),
            TelemetrySource::Desktop,
        )))
    }

    pub(crate) fn capture(&self, name: &str, properties: Value) {
        if let Some(telemetry) = &self.0 {
            telemetry.capture(name, properties);
        }
    }

    /// Upload a completed run's `trace.json` to the traces proxy. No-op when
    /// telemetry is off or the file is missing (e.g. cancelled runs).
    pub(crate) fn upload_run_trace(&self, run_dir: impl AsRef<Path>) {
        if let Some(telemetry) = &self.0 {
            telemetry.upload_run_trace(run_dir.as_ref());
        }
    }
}

/// First line of an error, capped to 240 chars — mirrors the CLI's `error_summary`
/// so desktop error fields never carry multi-line or content-bearing payloads.
pub(crate) fn short_error(error: &str) -> String {
    error
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(240)
        .collect()
}

/// Wall-clock task duration in ms from the snapshot timestamps, when both ends
/// are known.
pub(crate) fn duration_ms(started_at: Option<u64>, finished_at: Option<u64>) -> Option<u64> {
    match (started_at, finished_at) {
        (Some(start), Some(end)) => Some(end.saturating_sub(start)),
        _ => None,
    }
}
