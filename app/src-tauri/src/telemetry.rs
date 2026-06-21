use serde_json::{json, Value};
use socai_core::agent::{
    configured_default_model_for, configured_default_provider, provider_credential_kind,
};
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

    /// A no-op handle that never emits. Used by legacy/compat paths that run a
    /// task outside the start/end-bracketed lifecycle and must not produce
    /// orphan telemetry of their own.
    pub(crate) fn disabled() -> Self {
        Self(None)
    }

    pub(crate) fn capture(&self, name: &str, properties: Value) {
        if let Some(telemetry) = &self.0 {
            telemetry.capture(name, properties);
        }
    }

    /// One event per app launch. Carries the persisted default provider/model and
    /// whether a credential is present — no content, just setup state.
    pub(crate) fn emit_app_open(&self) {
        if self.0.is_none() {
            return;
        }
        let (default_provider, default_model, has_api_key) = match configured_default_provider() {
            Some(provider) => (
                Some(provider.as_str().to_string()),
                Some(configured_default_model_for(provider)),
                provider_credential_kind(provider).is_some(),
            ),
            None => (None, None, false),
        };
        self.capture(
            "socai_desktop_app_open",
            json!({
                "default_provider": default_provider,
                "default_model": default_model,
                "has_api_key": has_api_key,
            }),
        );
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
