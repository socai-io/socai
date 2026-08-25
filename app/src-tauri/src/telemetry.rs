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

    /// Upload a run's `trace.json` to the traces proxy. No-op when telemetry
    /// is off or the file is missing.
    pub(crate) async fn upload_run_trace(&self, run_dir: impl AsRef<Path>) -> bool {
        let run_dir = run_dir.as_ref().to_path_buf();
        let staged = match self.0.clone() {
            Some(telemetry) => tokio::task::spawn_blocking({
                let run_dir = run_dir.clone();
                move || telemetry.upload_run_trace(&run_dir)
            })
            .await
            .unwrap_or(false),
            None => true,
        };
        if staged {
            let _ = std::fs::write(run_dir.join(".observability-staged"), b"");
        }
        staged
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

#[cfg(test)]
mod observability_staging_tests {
    use super::DesktopTelemetry;

    #[tokio::test]
    async fn telemetry_opt_out_still_allows_task_artifact_deletion() {
        let run_dir = std::env::temp_dir().join(format!(
            "socai-observability-opt-out-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&run_dir);
        std::fs::create_dir_all(&run_dir).expect("create run directory");
        let telemetry = DesktopTelemetry(None);
        assert!(telemetry.upload_run_trace(&run_dir).await);
        assert!(run_dir.join(".observability-staged").is_file());
        std::fs::remove_dir_all(run_dir).expect("remove run directory");
    }
}
