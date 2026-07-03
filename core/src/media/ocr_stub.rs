//! OCR stub for Intel macOS (`x86_64-apple-darwin`).
//!
//! ort ships no prebuilt ONNX Runtime for this target (upstream ONNX Runtime
//! dropped Intel macOS), so `oar-ocr` cannot build and the real `ocr` module
//! is excluded from this slice of the universal binary. This stub keeps the
//! same public surface: every OCR request resolves to a per-item error —
//! the shape callers already handle — and diagnostics report OCR as
//! unavailable instead of a model identity.

use std::time::Duration;

use serde_json::{json, Value};

const UNAVAILABLE: &str =
    "local ocr is unavailable on intel macs (no x86_64-apple-darwin onnx runtime build)";

/// Mirror of the real module's batch result: per-image text outcomes plus the
/// wall time of the (never run) predict call.
pub struct OcrBatch {
    pub results: Vec<(usize, std::result::Result<String, String>)>,
    pub predict: Duration,
}

/// Resolve every requested image to the unavailability error.
pub fn ocr_images_bytes(items: Vec<(usize, Vec<u8>)>) -> OcrBatch {
    OcrBatch {
        results: items
            .into_iter()
            .map(|(idx, _)| (idx, Err(UNAVAILABLE.to_string())))
            .collect(),
        predict: Duration::ZERO,
    }
}

/// No engine to warm.
pub fn warm_up() {}

/// Same key shape as the real module's diagnostics so perf records stay
/// uniform across slices, with the model identity replaced by the reason OCR
/// is unavailable.
pub fn diagnostics() -> Value {
    let machine = crate::util::machine::machine_info();
    json!({
        "model": null,
        "runtime": null,
        "execution_provider": null,
        "available": false,
        "reason": UNAVAILABLE,
        "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        "machine": {
            "os": machine.os,
            "arch": machine.arch,
            "cpu_model": machine.cpu_model,
            "cpu_count": machine.cpu_count,
            "memory_total_mb": machine.memory_total_mb,
        },
    })
}
