//! Local OCR via PP-OCRv6 small (ONNX Runtime, through the `oar-ocr` crate).
//!
//! The detection/recognition models and the recognition character dictionary
//! are embedded into the binary (`include_bytes!`) so a release ships with OCR
//! self-contained — no runtime model download. `oar-ocr` wants on-disk paths,
//! so the embedded bytes are written once into a per-user cache dir and the
//! pipeline is built from there. The built pipeline is process-global and
//! reused across calls (model load is expensive); it's serialized behind a
//! `Mutex` since OCR runs CPU-bound and we already drive it from
//! `spawn_blocking`.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use oar_ocr::core::config::OrtExecutionProvider;
use oar_ocr::core::config::OrtSessionConfig;
use oar_ocr::oarocr::{OAROCRBuilder, OAROCR};
use serde_json::{json, Value};

/// PP-OCRv6 small models + dict, baked into the binary. Paths are relative to
/// this source file (`core/src/media/ocr.rs` → `core/assets/ocr/`).
const DET_ONNX: &[u8] = include_bytes!("../../assets/ocr/ppocrv6_small_det.onnx");
const REC_ONNX: &[u8] = include_bytes!("../../assets/ocr/ppocrv6_small_rec.onnx");
const REC_DICT: &[u8] = include_bytes!("../../assets/ocr/ppocrv6_rec_dict.txt");

/// Human-readable model identity, surfaced in OCR diagnostics.
pub const MODEL_NAME: &str = "PP-OCRv6_small (det+rec, ONNX)";

static ENGINE: OnceLock<std::result::Result<Mutex<OAROCR>, String>> = OnceLock::new();

/// One image's OCR outcome: recognized text (lines joined by `\n`) plus the
/// wall-clock inference time for that single image.
pub struct OcrOutcome {
    pub text: String,
    pub elapsed_ms: u128,
}

/// Run OCR on a batch of already-decoded image byte blobs, tagged with a caller
/// index so results can be mapped back. Each image is timed and predicted
/// individually (so per-image timing is exact); decode/engine/predict failures
/// are returned per item as `Err(message)` and never panic. Designed to be
/// called from inside `tokio::task::spawn_blocking`.
pub fn ocr_images_bytes(
    items: Vec<(usize, Vec<u8>)>,
) -> Vec<(usize, std::result::Result<OcrOutcome, String>)> {
    let mut out: Vec<(usize, std::result::Result<OcrOutcome, String>)> = Vec::new();

    let engine = match engine() {
        Ok(engine) => engine,
        Err(err) => {
            for (idx, _) in items {
                out.push((idx, Err(err.clone())));
            }
            return out;
        }
    };

    for (idx, bytes) in items {
        let image = match image::load_from_memory(&bytes) {
            Ok(image) => image.to_rgb8(),
            Err(err) => {
                out.push((idx, Err(format!("image decode failed: {err}"))));
                continue;
            }
        };
        let t0 = Instant::now();
        let predicted = match engine.lock() {
            Ok(ocr) => ocr.predict(vec![image]),
            Err(_) => {
                out.push((idx, Err("ocr engine mutex poisoned".into())));
                continue;
            }
        };
        let elapsed_ms = t0.elapsed().as_millis();
        match predicted {
            Ok(results) => {
                let text = results
                    .into_iter()
                    .next()
                    .map(|result| {
                        result
                            .text_regions
                            .iter()
                            .filter_map(|region| region.text.as_ref().map(|t| t.to_string()))
                            .map(|line| line.trim().to_string())
                            .filter(|line| !line.is_empty())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                out.push((idx, Ok(OcrOutcome { text, elapsed_ms })));
            }
            Err(err) => out.push((idx, Err(format!("ocr predict failed: {err}")))),
        }
    }
    out
}

/// Static OCR diagnostics for the run artifact: model identity, ONNX runtime,
/// the resolved execution provider, and host machine parameters. Useful for
/// debugging the cost/quality of OCR across machines.
pub fn diagnostics() -> Value {
    json!({
        "model": MODEL_NAME,
        "runtime": "onnxruntime (ort 2.0.0-rc.12)",
        "execution_provider": active_ep_label(),
        "machine": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "logical_cpus": std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(0),
        },
    })
}

/// The execution provider that will actually be used at runtime, accounting for
/// `SOCAI_OCR_EP` and which EPs were compiled in for this target.
fn active_ep_label() -> &'static str {
    match requested_ep().as_str() {
        "coreml" if cfg!(target_os = "macos") => "coreml",
        "directml" if cfg!(target_os = "windows") => "directml",
        _ => "cpu",
    }
}

fn requested_ep() -> String {
    std::env::var("SOCAI_OCR_EP")
        .unwrap_or_default()
        .to_lowercase()
}

/// Lazily build (once) and return the process-global OCR pipeline.
fn engine() -> std::result::Result<&'static Mutex<OAROCR>, String> {
    ENGINE
        .get_or_init(|| build_engine().map_err(|err| format!("{err:#}")))
        .as_ref()
        .map_err(|err| err.clone())
}

fn build_engine() -> Result<Mutex<OAROCR>> {
    let dir = model_cache_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create ocr model cache dir {}", dir.display()))?;
    let det = write_asset(&dir, "ppocrv6_small_det.onnx", DET_ONNX)?;
    let rec = write_asset(&dir, "ppocrv6_small_rec.onnx", REC_ONNX)?;
    let dict = write_asset(&dir, "ppocrv6_rec_dict.txt", REC_DICT)?;

    let mut builder = OAROCRBuilder::new(
        det.to_string_lossy().into_owned(),
        rec.to_string_lossy().into_owned(),
        dict.to_string_lossy().into_owned(),
    );
    if let Some(config) = ep_config() {
        builder = builder.ort_session(config);
    }
    let ocr = builder.build().context("build PP-OCRv6 OCR pipeline")?;
    Ok(Mutex::new(ocr))
}

/// Build the ONNX Runtime session config for the requested execution provider.
/// `None` means the default (CPU). EP variants only apply on a target where the
/// matching provider was compiled in (macOS→CoreML, Windows→DirectML); a
/// request that can't be honored warns and falls back to CPU.
fn ep_config() -> Option<OrtSessionConfig> {
    match requested_ep().as_str() {
        "" | "cpu" => None,
        "coreml" => {
            #[cfg(target_os = "macos")]
            {
                Some(OrtSessionConfig::new().with_execution_providers(vec![
                    OrtExecutionProvider::CoreML {
                        ane_only: None,
                        subgraphs: None,
                    },
                    OrtExecutionProvider::CPU,
                ]))
            }
            #[cfg(not(target_os = "macos"))]
            {
                tracing::warn!("SOCAI_OCR_EP=coreml is only available on macOS; using CPU");
                None
            }
        }
        "directml" => {
            #[cfg(target_os = "windows")]
            {
                Some(OrtSessionConfig::new().with_execution_providers(vec![
                    OrtExecutionProvider::DirectML { device_id: None },
                    OrtExecutionProvider::CPU,
                ]))
            }
            #[cfg(not(target_os = "windows"))]
            {
                tracing::warn!("SOCAI_OCR_EP=directml is only available on Windows; using CPU");
                None
            }
        }
        other => {
            tracing::warn!("unknown SOCAI_OCR_EP value {other:?}; using CPU");
            None
        }
    }
}

/// Per-user cache dir for the extracted model files, e.g.
/// `~/Library/Caches/socai/ocr` on macOS. Versioned by the model set so a
/// future model swap writes a fresh directory instead of reusing stale bytes.
fn model_cache_dir() -> Result<PathBuf> {
    let base = dirs::cache_dir().ok_or_else(|| anyhow!("no user cache dir available"))?;
    Ok(base.join("socai").join("ocr").join("ppocrv6-small-v1"))
}

/// Write an embedded asset to `dir/name` if it's missing or a different size.
/// Returns the path. Size check is a cheap "already extracted" guard — the
/// bytes are immutable, baked into the binary.
fn write_asset(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf> {
    let path = dir.join(name);
    let needs_write = match std::fs::metadata(&path) {
        Ok(meta) => meta.len() != bytes.len() as u64,
        Err(_) => true,
    };
    if needs_write {
        std::fs::write(&path, bytes)
            .with_context(|| format!("write ocr asset {}", path.display()))?;
    }
    Ok(path)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Manual smoke test: exercises the full embedded-model → ort → predict path
    /// against a real image. Ignored by default (loads ~31 MB of models + runs
    /// inference). Run with:
    ///   SOCAI_OCR_TEST_IMAGE=/path/to/img.png cargo test -p socai-core \
    ///     --lib media::ocr -- --ignored --nocapture
    #[test]
    #[ignore]
    fn ocr_smoke_test() {
        let path = std::env::var("SOCAI_OCR_TEST_IMAGE")
            .expect("set SOCAI_OCR_TEST_IMAGE to an image path");
        let bytes = std::fs::read(&path).expect("read test image");
        let mut results = ocr_images_bytes(vec![(0, bytes)]);
        let (_, result) = results.pop().expect("one result");
        let outcome = result.expect("ocr ran");
        eprintln!(
            "--- OCR result ({} ms) ---\n{}\n--- end ---",
            outcome.elapsed_ms, outcome.text
        );
        assert!(!outcome.text.trim().is_empty(), "expected some recognized text");
    }

    /// Manual benchmark: OCR a single image `SOCAI_OCR_BENCH_N` times (default
    /// 30) after a warm-up, reporting min/median/mean/max per-image latency for
    /// the active EP (set via SOCAI_OCR_EP). Run with:
    ///   SOCAI_OCR_TEST_IMAGE=img.png SOCAI_OCR_BENCH_N=50 SOCAI_OCR_EP=coreml \
    ///     cargo test -p socai-core --lib media::ocr::tests::ocr_benchmark \
    ///     -- --ignored --nocapture
    #[test]
    #[ignore]
    fn ocr_benchmark() {
        let path = std::env::var("SOCAI_OCR_TEST_IMAGE")
            .expect("set SOCAI_OCR_TEST_IMAGE to an image path");
        let bytes = std::fs::read(&path).expect("read test image");
        let n: usize = std::env::var("SOCAI_OCR_BENCH_N")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30);

        // Warm-up (builds engine, compiles EP graph, primes caches).
        let warm = ocr_images_bytes(vec![(0, bytes.clone())]);
        let warm_ms = warm
            .into_iter()
            .next()
            .and_then(|(_, r)| r.ok())
            .map(|o| o.elapsed_ms)
            .unwrap_or(0);

        let mut samples: Vec<u128> = Vec::with_capacity(n);
        for _ in 0..n {
            let r = ocr_images_bytes(vec![(0, bytes.clone())]);
            if let Some((_, Ok(o))) = r.into_iter().next() {
                samples.push(o.elapsed_ms);
            }
        }
        samples.sort_unstable();
        let sum: u128 = samples.iter().sum();
        let median = samples[samples.len() / 2];
        eprintln!(
            "EP={} warmup={}ms N={} min={} median={} mean={:.1} max={} (ms/image)",
            active_ep_label(),
            warm_ms,
            samples.len(),
            samples.first().copied().unwrap_or(0),
            median,
            sum as f64 / samples.len() as f64,
            samples.last().copied().unwrap_or(0),
        );
    }
}
