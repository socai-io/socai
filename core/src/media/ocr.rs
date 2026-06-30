//! Local OCR via PP-OCRv6 tiny (ONNX Runtime, through the `oar-ocr` crate).
//!
//! The detection/recognition models and the recognition character dictionary
//! are embedded into the binary (`include_bytes!`) so a release ships with OCR
//! self-contained — no runtime model download. `oar-ocr` wants on-disk paths,
//! so the embedded bytes are written once into a per-user cache dir and the
//! pipeline is built from there. The built pipeline is process-global and
//! reused across calls (model load is expensive); it's serialized behind a
//! `Mutex` since OCR runs CPU-bound and we already drive it from
//! `spawn_blocking`.
//!
//! Execution is CPU-only: benchmarking PP-OCRv6 on Apple Silicon showed the
//! CoreML EP runs ~2× slower (small dynamic OCR graphs defeat ANE/GPU offload),
//! so we ship the ONNX Runtime CPU provider and don't compile in CoreML/DirectML.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use oar_ocr::oarocr::{OAROCRBuilder, OAROCR};
use serde_json::{json, Value};

/// PP-OCRv6 tiny models + dict, baked into the binary. Paths are relative to
/// this source file (`core/src/media/ocr.rs` → `core/assets/ocr/`).
const DET_ONNX: &[u8] = include_bytes!("../../assets/ocr/ppocrv6_tiny_det.onnx");
const REC_ONNX: &[u8] = include_bytes!("../../assets/ocr/ppocrv6_tiny_rec.onnx");
const REC_DICT: &[u8] = include_bytes!("../../assets/ocr/ppocrv6_tiny_rec_dict.txt");

/// Human-readable model identity, surfaced in OCR diagnostics.
pub const MODEL_NAME: &str = "PP-OCRv6_tiny (det+rec, ONNX)";

static ENGINE: OnceLock<std::result::Result<Mutex<OAROCR>, String>> = OnceLock::new();

/// Run OCR on a batch of already-decoded image byte blobs, tagged with a caller
/// index so results can be mapped back. All successfully-decoded images are run
/// in a **single batched `predict`** call — oar-ocr parallelizes the batch across
/// cores (rayon), which is ~1.5× faster than predicting one image at a time —
/// and `predict` is the timed wall of that one call. Per-image inference time is
/// therefore not available (that's the cost of batching); callers measure per
/// note/batch instead. Decode/engine/predict failures are returned per item as
/// `Err(message)` and never panic. Designed to be called from inside
/// `tokio::task::spawn_blocking`.
pub fn ocr_images_bytes(items: Vec<(usize, Vec<u8>)>) -> OcrBatch {
    let mut out: Vec<(usize, std::result::Result<String, String>)> = Vec::new();
    let mut idxs: Vec<usize> = Vec::new();
    let mut imgs: Vec<image::RgbImage> = Vec::new();
    for (idx, bytes) in items {
        match image::load_from_memory(&bytes) {
            Ok(image) => {
                idxs.push(idx);
                imgs.push(image.to_rgb8());
            }
            Err(err) => out.push((idx, Err(format!("image decode failed: {err}")))),
        }
    }
    if imgs.is_empty() {
        return OcrBatch {
            results: out,
            predict: Duration::ZERO,
        };
    }

    let engine = match engine() {
        Ok(engine) => engine,
        Err(err) => {
            for idx in idxs {
                out.push((idx, Err(err.clone())));
            }
            return OcrBatch {
                results: out,
                predict: Duration::ZERO,
            };
        }
    };

    // Time only the predict() call (inside the lock) so the reported cost is the
    // true batch inference time, not engine-mutex wait.
    let (predicted, predict) = match engine.lock() {
        Ok(ocr) => {
            let t0 = Instant::now();
            let predicted = ocr.predict(imgs);
            (predicted, t0.elapsed())
        }
        Err(_) => {
            for idx in idxs {
                out.push((idx, Err("ocr engine mutex poisoned".into())));
            }
            return OcrBatch {
                results: out,
                predict: Duration::ZERO,
            };
        }
    };

    match predicted {
        Ok(results) => {
            for (idx, result) in idxs.into_iter().zip(results) {
                let text = result
                    .text_regions
                    .iter()
                    .filter_map(|region| region.text.as_ref().map(|t| t.to_string()))
                    .map(|line| line.trim().to_string())
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                out.push((idx, Ok(text)));
            }
        }
        Err(err) => {
            let msg = format!("ocr predict failed: {err}");
            for idx in idxs {
                out.push((idx, Err(msg.clone())));
            }
        }
    }
    OcrBatch {
        results: out,
        predict,
    }
}

/// Result of one batched OCR call: per-image text outcomes (in input order) plus
/// the wall time of the single `predict` call (excludes decode and mutex wait).
pub struct OcrBatch {
    pub results: Vec<(usize, std::result::Result<String, String>)>,
    pub predict: Duration,
}

/// Build the OCR engine and run one tiny prediction so the model load + ORT
/// session init + graph compilation happen off the critical path. Safe to call
/// from `spawn_blocking` at the start of an OCR run; the global engine is built
/// once, so a later real OCR reuses it. Best-effort: errors are ignored.
pub fn warm_up() {
    let Ok(engine) = engine() else {
        return;
    };
    let probe = image::RgbImage::from_pixel(32, 32, image::Rgb([255, 255, 255]));
    if let Ok(ocr) = engine.lock() {
        let _ = ocr.predict(vec![probe]);
    }
}

/// OCR diagnostics for the performance/debug record: model identity, ONNX
/// runtime, execution provider, and host machine parameters (incl. the concrete
/// CPU/chip model, e.g. "Apple M4"). The machine info comes from the shared
/// [`crate::machine`] snapshot — the same source telemetry uploads — so the
/// local `ocr_perf.json` and the reported device info stay consistent. Not
/// written into the LLM-facing JSON artifact.
pub fn diagnostics() -> Value {
    let machine = crate::util::machine::machine_info();
    json!({
        "model": MODEL_NAME,
        "runtime": "onnxruntime (ort 2.0.0-rc.12)",
        "execution_provider": "cpu",
        // Debug builds run OCR ~7-8× slower than release; surface it so a slow
        // perf record is obviously attributable to an unoptimized build.
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
    let det = write_asset(&dir, "ppocrv6_tiny_det.onnx", DET_ONNX)?;
    let rec = write_asset(&dir, "ppocrv6_tiny_rec.onnx", REC_ONNX)?;
    let dict = write_asset(&dir, "ppocrv6_tiny_rec_dict.txt", REC_DICT)?;

    // CPU execution provider (oar-ocr's default when no ort_session is set).
    let ocr = OAROCRBuilder::new(
        det.to_string_lossy().into_owned(),
        rec.to_string_lossy().into_owned(),
        dict.to_string_lossy().into_owned(),
    )
    .build()
    .context("build PP-OCRv6 OCR pipeline")?;
    Ok(Mutex::new(ocr))
}

/// Per-user cache dir for the extracted model files, e.g.
/// `~/Library/Caches/socai/ocr` on macOS. Versioned by the model set so a
/// future model swap writes a fresh directory instead of reusing stale bytes.
fn model_cache_dir() -> Result<PathBuf> {
    let base = dirs::cache_dir().ok_or_else(|| anyhow!("no user cache dir available"))?;
    Ok(base.join("socai").join("ocr").join("ppocrv6-tiny-v1"))
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

    #[test]
    fn diagnostics_reports_machine_and_model() {
        let d = diagnostics();
        assert_eq!(d["execution_provider"], "cpu");
        assert!(d["model"].as_str().is_some_and(|s| s.contains("PP-OCRv6")));
        let machine = &d["machine"];
        assert_eq!(machine["os"], std::env::consts::OS);
        assert_eq!(machine["arch"], std::env::consts::ARCH);
        // cpu_model / cpu_count / memory_total_mb are best-effort; the keys must
        // always be present (sourced from the shared crate::machine snapshot).
        assert!(machine.get("cpu_model").is_some());
        assert!(machine.get("cpu_count").is_some());
        assert!(machine.get("memory_total_mb").is_some());
    }

    /// Manual smoke test: exercises the full embedded-model → ort → predict path
    /// against a real image. Ignored by default (loads the models + runs
    /// inference). Run with:
    ///   SOCAI_OCR_TEST_IMAGE=/path/to/img.png cargo test -p socai-core \
    ///     --lib media::ocr -- --ignored --nocapture
    #[test]
    #[ignore]
    fn ocr_smoke_test() {
        let path = std::env::var("SOCAI_OCR_TEST_IMAGE")
            .expect("set SOCAI_OCR_TEST_IMAGE to an image path");
        let bytes = std::fs::read(&path).expect("read test image");
        let mut batch = ocr_images_bytes(vec![(0, bytes)]);
        let (_, result) = batch.results.pop().expect("one result");
        let text = result.expect("ocr ran");
        eprintln!(
            "--- OCR result (batch predict {} ms) ---\n{text}\n--- end ---",
            batch.predict.as_millis()
        );
        assert!(!text.trim().is_empty(), "expected some recognized text");
    }

    /// Build a standalone pipeline from explicit model paths (bypasses the
    /// embedded global engine) so different model tiers can be benchmarked.
    fn build_ocr_from_env() -> OAROCR {
        let det = std::env::var("SOCAI_OCR_DET").expect("SOCAI_OCR_DET");
        let rec = std::env::var("SOCAI_OCR_REC").expect("SOCAI_OCR_REC");
        let dict = std::env::var("SOCAI_OCR_DICT").expect("SOCAI_OCR_DICT");
        OAROCRBuilder::new(det, rec, dict)
            .build()
            .expect("build ocr from env paths")
    }

    /// Compare ways to OCR a batch of `B` already-decoded images (CPU):
    ///   serial     — B× predict(vec![img]) one at a time
    ///   batch      — one predict(vec![B imgs]) (oar-ocr internal parallelism)
    ///   concurrent — `available_parallelism` threads sharing one engine
    /// Model via SOCAI_OCR_DET/REC/DICT, batch via SOCAI_OCR_BENCH_B (default 20).
    #[test]
    #[ignore]
    fn ocr_bench_modes() {
        let path = std::env::var("SOCAI_OCR_TEST_IMAGE").expect("SOCAI_OCR_TEST_IMAGE");
        let bytes = std::fs::read(&path).expect("read test image");
        let img = image::load_from_memory(&bytes).expect("decode").to_rgb8();
        let b: usize = std::env::var("SOCAI_OCR_BENCH_B")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20);
        let ocr = build_ocr_from_env();
        let warm = ocr.predict(vec![img.clone()]).expect("warmup");
        if std::env::var("SOCAI_OCR_BENCH_TEXT").is_ok() {
            let text = warm
                .first()
                .map(|r| {
                    r.text_regions
                        .iter()
                        .filter_map(|reg| reg.text.as_ref().map(|t| t.to_string()))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            eprintln!("--- recognized text ---\n{text}\n--- end ---");
        }

        let t = Instant::now();
        for _ in 0..b {
            let _ = ocr.predict(vec![img.clone()]).expect("predict");
        }
        let serial = t.elapsed();

        let batch: Vec<image::RgbImage> = (0..b).map(|_| img.clone()).collect();
        let t = Instant::now();
        let _ = ocr.predict(batch).expect("predict batch");
        let batched = t.elapsed();

        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let ocr_ref = &ocr;
        let t = Instant::now();
        std::thread::scope(|scope| {
            for c in 0..threads {
                let img = img.clone();
                scope.spawn(move || {
                    let count = b / threads + usize::from(c < b % threads);
                    for _ in 0..count {
                        let _ = ocr_ref.predict(vec![img.clone()]);
                    }
                });
            }
        });
        let concurrent = t.elapsed();

        let per = |d: std::time::Duration| d.as_secs_f64() * 1000.0 / b as f64;
        eprintln!("EP=cpu B={b} threads={threads}");
        eprintln!("  serial:     {serial:>8.2?}  ({:.1} ms/img)", per(serial));
        eprintln!("  batch:      {batched:>8.2?}  ({:.1} ms/img)", per(batched));
        eprintln!("  concurrent: {concurrent:>8.2?}  ({:.1} ms/img)", per(concurrent));
    }
}
