use std::collections::HashSet;
use std::time::Instant;

use crate::agent::{Block, Message, MessageRole, ToolSchema};
use anyhow::Result;
use base64::Engine;
use futures::StreamExt;
use serde_json::Value;

use crate::media::common::{detect_media_type, insert_string, url_suffix, MediaUnavailable};
use crate::media::md5;
use crate::media::processor::MediaProcessor;

/// Max simultaneous image downloads for one note. Topic-scan media downloads
/// may request a full carousel, so keep CDN requests bounded.
const IMAGE_DOWNLOAD_CONCURRENCY: usize = 8;
/// Max simultaneous vision (LLM) calls when enriching a note's images. Bounded
/// so a 12-image note doesn't fire a dozen concurrent requests at the provider.
const VISION_CONCURRENCY: usize = 4;
/// Images per 2x2 vision grid.
const VISION_GRID_BATCH: usize = 4;
/// Side length of each grid cell (px). 4 cells → a 1024x1024 grid, i.e. roughly
/// one image's worth of tokens for four images (~1 MP is the sweet spot shared
/// by OpenAI, Claude and Qwen-VL). Images are letterboxed (aspect preserved,
/// white padding) into their square cell.
const VISION_GRID_CELL: u32 = 512;

impl MediaProcessor {
    pub fn ocr_image(&self, payload: &[u8]) -> Result<String> {
        if !self.config.use_ocr {
            anyhow::bail!(MediaUnavailable("OCR is disabled".into()));
        }
        if payload.is_empty() {
            return Ok(String::new());
        }
        let mut batch = crate::media::ocr::ocr_images_bytes(vec![(0, payload.to_vec())]);
        match batch.results.pop() {
            Some((_, result)) => result.map_err(|err| anyhow::anyhow!(err)),
            None => Ok(String::new()),
        }
    }

    /// OCR the already-downloaded images of a note in place: read each image with
    /// a `local_path`, run them as one batched PP-OCRv6 `predict`, and attach
    /// `ocr_text` (or `ocr_error`) per image. Returns the batch inference time so
    /// the caller can record per-note OCR cost. Runs the CPU-bound inference on a
    /// blocking thread. No-op (returns zero) when OCR is disabled or no image has
    /// a local path.
    pub async fn ocr_downloaded_images(&self, images: &mut [Value]) -> std::time::Duration {
        if !self.config.use_ocr || images.is_empty() {
            return std::time::Duration::ZERO;
        }
        let jobs: Vec<(usize, String)> = images
            .iter()
            .enumerate()
            .filter(|(_, item)| item.get("ocr_text").is_none())
            .filter_map(|(idx, item)| {
                item.get("local_path")
                    .and_then(Value::as_str)
                    .filter(|path| !path.is_empty())
                    .map(|path| (idx, path.to_string()))
            })
            .collect();
        if jobs.is_empty() {
            return std::time::Duration::ZERO;
        }

        let batch = tokio::task::spawn_blocking(move || {
            let items: Vec<(usize, Vec<u8>)> = jobs
                .into_iter()
                .filter_map(|(idx, path)| std::fs::read(&path).ok().map(|bytes| (idx, bytes)))
                .collect();
            crate::media::ocr::ocr_images_bytes(items)
        })
        .await;

        let Ok(batch) = batch else {
            return std::time::Duration::ZERO;
        };
        self.timing.record("ocr_predict", batch.predict);
        for (idx, result) in batch.results {
            let Some(item) = images.get_mut(idx) else {
                continue;
            };
            match result {
                Ok(text) if !text.trim().is_empty() => {
                    insert_string(item, "ocr_text", text);
                }
                Ok(_) => {}
                Err(err) => insert_string(item, "ocr_error", err),
            }
        }
        batch.predict
    }

    /// OCR card cover images *without persisting them* — used by the cards-only
    /// preview path so a search-results scan reads each cover like a human
    /// glancing at the page. Downloads each `cover_url` to memory, OCRs them in
    /// one batch, and attaches `ocr_text` (or `ocr_error`) to the card in place;
    /// the downloaded bytes are dropped (nothing written to the run dir). Returns
    /// the batch inference time.
    pub async fn ocr_cover_images(
        &self,
        cards: &mut [Value],
        referer: &str,
    ) -> std::time::Duration {
        if !self.config.use_ocr || cards.is_empty() {
            return std::time::Duration::ZERO;
        }
        let jobs: Vec<(usize, String)> = cards
            .iter()
            .enumerate()
            .filter_map(|(idx, card)| {
                card.get("cover_url")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|url| !url.is_empty())
                    .map(|url| (idx, url.to_string()))
            })
            .collect();
        if jobs.is_empty() {
            return std::time::Duration::ZERO;
        }

        // Fetch covers concurrently (network-bound), then OCR on a blocking
        // thread (CPU-bound). Covers are small (one per card) and never saved.
        let t_dl = Instant::now();
        let downloads = jobs
            .iter()
            .map(|(idx, url)| {
                let idx = *idx;
                let url = url.clone();
                let referer = referer.to_string();
                async move {
                    let (bytes, _err) = self.safe_download(url, referer).await;
                    (idx, bytes)
                }
            })
            .collect::<Vec<_>>();
        let fetched: Vec<(usize, Vec<u8>)> = futures::stream::iter(downloads)
            .buffered(IMAGE_DOWNLOAD_CONCURRENCY)
            .filter(|(_, bytes)| futures::future::ready(!bytes.is_empty()))
            .collect()
            .await;
        self.timing
            .record("ocr_cover_download_batch", t_dl.elapsed());

        let batch =
            tokio::task::spawn_blocking(move || crate::media::ocr::ocr_images_bytes(fetched)).await;

        let Ok(batch) = batch else {
            return std::time::Duration::ZERO;
        };
        self.timing.record("ocr_predict", batch.predict);
        for (idx, result) in batch.results {
            let Some(card) = cards.get_mut(idx) else {
                continue;
            };
            match result {
                Ok(text) if !text.trim().is_empty() => {
                    insert_string(card, "ocr_text", text);
                }
                Ok(_) => {}
                Err(err) => insert_string(card, "ocr_error", err),
            }
        }
        batch.predict
    }

    pub async fn describe_image(
        &self,
        payload: &[u8],
        prompt: &str,
        max_tokens: u32,
    ) -> Result<String> {
        if !self.config.use_vision {
            anyhow::bail!(MediaUnavailable("Vision is disabled".into()));
        }
        let Some(llm_provider) = &self.llm_provider else {
            anyhow::bail!(MediaUnavailable(
                "No agent LLM provider was provided for image vision".into()
            ));
        };
        if payload.is_empty() {
            return Ok(String::new());
        }
        let t0 = Instant::now();
        let media_type = detect_media_type(payload);
        let data = base64::engine::general_purpose::STANDARD.encode(payload);
        let response = llm_provider
            .send(
                "You describe images concisely and only state visible evidence.",
                &[Message {
                    role: MessageRole::User,
                    content: crate::agent::MessageContent::Blocks(vec![
                        Block::Text {
                            text: prompt.to_string(),
                        },
                        Block::Image { data, media_type },
                    ]),
                }],
                &[] as &[ToolSchema],
                max_tokens,
            )
            .await?;
        self.timing.record("vision_image", t0.elapsed());
        Ok(response.text_blocks.join("\n").trim().to_string())
    }

    /// Describe a batch of images with a single vision call by compositing them
    /// into a 2x2 grid. Returns one description per input image (same order).
    /// Falls back to a single-image call when there's only one image, or when
    /// compositing fails.
    async fn describe_image_batch(&self, payloads: &[&[u8]]) -> Result<Vec<String>> {
        if payloads.len() == 1 {
            let desc = self
                .describe_image(
                    payloads[0],
                    "Describe this Xiaohongshu image for the note. Focus on concrete visible facts.",
                    180,
                )
                .await?;
            return Ok(vec![desc]);
        }
        match compose_grid(payloads, VISION_GRID_CELL) {
            Some(grid) => self.describe_grid(&grid, payloads.len()).await,
            None => {
                // Compositing failed (e.g. all decodes failed) — degrade to
                // empty descriptions rather than scrambling.
                Ok(vec![String::new(); payloads.len()])
            }
        }
    }

    /// Send one 2x2 grid image and parse `n` per-cell descriptions back out.
    async fn describe_grid(&self, grid_jpeg: &[u8], n: usize) -> Result<Vec<String>> {
        let Some(llm_provider) = &self.llm_provider else {
            anyhow::bail!(MediaUnavailable(
                "No agent LLM provider was provided for image vision".into()
            ));
        };
        let t0 = Instant::now();
        let data = base64::engine::general_purpose::STANDARD.encode(grid_jpeg);
        let prompt = format!(
            "This is a 2-column grid combining {n} separate Xiaohongshu note images, \
             numbered left-to-right then top-to-bottom (image 1 = top-left). Each cell \
             may be letterboxed with white padding. Describe EACH image separately, \
             focusing on concrete visible facts (text, products, layout). Reply with \
             exactly {n} lines, each beginning with its number and a colon, e.g.\n\
             1: <description of image 1>\n2: <description of image 2>\n\
             Do not merge images or add extra commentary."
        );
        let response = llm_provider
            .send(
                "You describe images concisely and only state visible evidence.",
                &[Message {
                    role: MessageRole::User,
                    content: crate::agent::MessageContent::Blocks(vec![
                        Block::Text { text: prompt },
                        Block::Image {
                            data,
                            media_type: "image/jpeg".to_string(),
                        },
                    ]),
                }],
                &[] as &[ToolSchema],
                (180 * n as u32).clamp(360, 1024),
            )
            .await?;
        self.timing.record("vision_grid", t0.elapsed());
        Ok(parse_grid_descriptions(&response.text_blocks.join("\n"), n))
    }

    /// Download note images to the run's media directory without OCR or vision
    /// enrichment. The returned image objects preserve the input shape and add
    /// `local_path` (or a `download_error` / `save_error`) per image.
    pub async fn download_images(
        &self,
        images: &[Value],
        referer: &str,
        label: &str,
    ) -> Vec<Value> {
        if images.is_empty() {
            return Vec::new();
        }

        let t_batch = Instant::now();
        let urls: Vec<String> = images
            .iter()
            .map(|image| {
                image
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            })
            .collect();
        let downloads = urls.into_iter().map(|url| {
            let referer = referer.to_string();
            // Per-item wall time (from admission into the concurrency window to
            // completion) so a single slow CDN fetch is visible inside the
            // otherwise-concurrent batch.
            async move {
                let t_item = Instant::now();
                let result = self.safe_download(url, referer).await;
                (result, t_item.elapsed().as_millis() as u64)
            }
        });
        let download_results: Vec<_> = futures::stream::iter(downloads)
            .buffered(IMAGE_DOWNLOAD_CONCURRENCY)
            .collect()
            .await;
        self.timing
            .record("image_download_batch", t_batch.elapsed());

        images
            .iter()
            .zip(download_results)
            .map(|(image, ((payload, error), download_ms))| {
                let url = image
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                let mut item = image.clone();
                if let Some(map) = item.as_object_mut() {
                    map.insert("download_ms".into(), serde_json::json!(download_ms));
                }
                if url.is_empty() {
                    insert_string(&mut item, "download_error", "image URL is empty");
                    return item;
                }
                if let Some(error) = error {
                    insert_string(&mut item, "download_error", error);
                    return item;
                }
                if payload.is_empty() {
                    insert_string(&mut item, "download_error", "download returned no bytes");
                    return item;
                }
                let path = self.save_bytes(&payload, label, &url_suffix(url, ".jpg"));
                match path {
                    Ok(path) => insert_string(&mut item, "local_path", path.to_string_lossy()),
                    Err(err) => insert_string(&mut item, "save_error", format!("{err:#}")),
                }
                item
            })
            .collect()
    }

    pub async fn enrich_images(
        &self,
        images: &[Value],
        referer: &str,
        label: &str,
        run_vision: bool,
    ) -> Vec<Value> {
        if images.is_empty() {
            return Vec::new();
        }

        let t_batch = Instant::now();
        let downloads = images
            .iter()
            .map(|image| {
                let url = image
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                self.safe_download(url, referer.to_string())
            })
            .collect::<Vec<_>>();
        let download_results = futures::future::join_all(downloads).await;
        self.timing
            .record("image_download_batch", t_batch.elapsed());

        let mut seen = HashSet::new();
        let mut deduped: Vec<(Value, Vec<u8>)> = Vec::new();
        for (image, (payload, error)) in images.iter().zip(download_results) {
            let url = image
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if url.is_empty() {
                continue;
            }
            let mut item = image.clone();
            if let Some(error) = error {
                insert_string(&mut item, "download_error", error);
                deduped.push((item, Vec::new()));
                continue;
            }
            if payload.is_empty() {
                continue;
            }
            let digest = md5::md5_hex(&payload);
            if !seen.insert(digest) {
                continue;
            }
            let path = self.save_bytes(&payload, label, &url_suffix(url, ".jpg"));
            match path {
                Ok(path) => insert_string(&mut item, "local_path", path.to_string_lossy()),
                Err(err) => insert_string(&mut item, "save_error", format!("{err:#}")),
            }
            deduped.push((item, payload));
        }

        let t_enrich = Instant::now();
        let mut items: Vec<(Value, Vec<u8>)> = deduped;

        // OCR is local + synchronous — do it inline.
        for (item, payload) in items.iter_mut() {
            if !payload.is_empty() && self.config.use_ocr && item.get("ocr_text").is_none() {
                match self.ocr_image(payload) {
                    Ok(text) if !text.trim().is_empty() => {
                        insert_string(item, "ocr_text", text);
                    }
                    Ok(_) => {}
                    Err(err) => insert_string(item, "ocr_error", format!("{err:#}")),
                }
            }
        }

        // Vision calls hit the LLM and used to run strictly one-at-a-time,
        // which dominated read_note latency (e.g. 12 images ≈ 5 min). Run them
        // with bounded concurrency instead.
        if run_vision && self.config.use_vision {
            let targets: Vec<usize> = items
                .iter()
                .enumerate()
                .filter(|(_, (item, payload))| {
                    !payload.is_empty() && item.get("vision_description").is_none()
                })
                .map(|(idx, _)| idx)
                .collect();
            // Batch images into 2x2 grids (4 per vision call) — ~1/4 the calls,
            // latency, and image tokens vs one call per image. Batches run with
            // bounded concurrency.
            let batches: Vec<Vec<usize>> = targets
                .chunks(VISION_GRID_BATCH)
                .map(<[usize]>::to_vec)
                .collect();
            let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(VISION_CONCURRENCY));
            let results = futures::future::join_all(batches.into_iter().map(|batch| {
                let permits = permits.clone();
                let payloads: Vec<&[u8]> = batch.iter().map(|&i| items[i].1.as_slice()).collect();
                async move {
                    let _permit = permits.acquire().await;
                    let descs = self.describe_image_batch(&payloads).await;
                    (batch, descs)
                }
            }))
            .await;
            for (batch, descs) in results {
                match descs {
                    Ok(descs) => {
                        for (k, &idx) in batch.iter().enumerate() {
                            match descs.get(k) {
                                Some(text) if !text.trim().is_empty() => {
                                    insert_string(
                                        &mut items[idx].0,
                                        "vision_description",
                                        text.clone(),
                                    );
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(err) => {
                        for &idx in &batch {
                            insert_string(&mut items[idx].0, "vision_error", format!("{err:#}"));
                        }
                    }
                }
            }
        }

        self.timing.record("image_enrich_batch", t_enrich.elapsed());
        items.into_iter().map(|(item, _)| item).collect()
    }

    async fn safe_download(&self, url: String, referer: String) -> (Vec<u8>, Option<String>) {
        if url.trim().is_empty() {
            return (Vec::new(), None);
        }
        match self.download_bytes(&url, &referer).await {
            Ok(bytes) => (bytes, None),
            Err(err) => (Vec::new(), Some(format!("{err:#}"))),
        }
    }
}

/// Composite up to a handful of images into a 2-column grid, each letterboxed
/// (aspect preserved, white padding) into a `cell`×`cell` square. Returns JPEG
/// bytes, or `None` if no image decoded.
fn compose_grid(payloads: &[&[u8]], cell: u32) -> Option<Vec<u8>> {
    use image::{imageops, DynamicImage, ImageFormat, Rgb, RgbImage};

    let cols = 2u32;
    let rows = (payloads.len() as u32).div_ceil(cols);
    let mut canvas = RgbImage::from_pixel(cols * cell, rows * cell, Rgb([255, 255, 255]));
    let mut any = false;
    for (i, payload) in payloads.iter().enumerate() {
        let Ok(decoded) = image::load_from_memory(payload) else {
            continue; // leave a blank cell on decode failure
        };
        any = true;
        // `resize` fits within cell×cell preserving aspect ratio.
        let fitted = decoded
            .resize(cell, cell, imageops::FilterType::Triangle)
            .to_rgb8();
        let (nw, nh) = (fitted.width(), fitted.height());
        let col = i as u32 % cols;
        let row = i as u32 / cols;
        let ox = (col * cell + (cell - nw) / 2) as i64;
        let oy = (row * cell + (cell - nh) / 2) as i64;
        imageops::overlay(&mut canvas, &fitted, ox, oy);
    }
    if !any {
        return None;
    }
    let mut buf = Vec::new();
    DynamicImage::ImageRgb8(canvas)
        .write_to(&mut std::io::Cursor::new(&mut buf), ImageFormat::Jpeg)
        .ok()?;
    Some(buf)
}

/// Parse `N` per-cell descriptions from a grid vision reply. Lines beginning
/// with `k:` / `k)` / `k.` (k = 1..=n) start description k; subsequent
/// unlabelled lines extend the current one. Missing entries stay empty rather
/// than scrambling onto the wrong image.
fn parse_grid_descriptions(text: &str, n: usize) -> Vec<String> {
    let mut out = vec![String::new(); n];
    let mut current: Option<usize> = None;
    for line in text.lines() {
        if let Some((num, rest)) = split_leading_index(line.trim_start()) {
            if (1..=n).contains(&num) {
                current = Some(num - 1);
                out[num - 1] = rest.trim().to_string();
                continue;
            }
        }
        if let Some(idx) = current {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                if !out[idx].is_empty() {
                    out[idx].push(' ');
                }
                out[idx].push_str(trimmed);
            }
        }
    }
    out
}

/// `"3: foo"` / `"3) foo"` / `"3. foo"` → `Some((3, "foo"))`.
fn split_leading_index(text: &str) -> Option<(usize, &str)> {
    let digits_end = text.find(|c: char| !c.is_ascii_digit())?;
    if digits_end == 0 {
        return None;
    }
    let num: usize = text[..digits_end].parse().ok()?;
    let rest = text[digits_end..].trim_start();
    let rest = rest
        .strip_prefix(':')
        .or_else(|| rest.strip_prefix(')'))
        .or_else(|| rest.strip_prefix('.'))?;
    Some((num, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numbered_grid_descriptions() {
        let reply = "1: first image\n2: second image\nmore detail\n3: third\n4: fourth";
        let out = parse_grid_descriptions(reply, 4);
        assert_eq!(out[0], "first image");
        assert_eq!(out[1], "second image more detail");
        assert_eq!(out[2], "third");
        assert_eq!(out[3], "fourth");
    }

    #[test]
    fn missing_entries_stay_empty_not_scrambled() {
        let out = parse_grid_descriptions("1: only the first", 3);
        assert_eq!(out[0], "only the first");
        assert!(out[1].is_empty());
        assert!(out[2].is_empty());
    }

    #[test]
    fn compose_grid_letterboxes_into_2x2() {
        use image::{DynamicImage, ImageFormat, RgbImage};
        // Two solid images of different aspect ratios.
        let mk = |w, h| {
            let img = RgbImage::from_pixel(w, h, image::Rgb([10, 20, 30]));
            let mut buf = Vec::new();
            DynamicImage::ImageRgb8(img)
                .write_to(&mut std::io::Cursor::new(&mut buf), ImageFormat::Png)
                .unwrap();
            buf
        };
        let a = mk(200, 400);
        let b = mk(400, 200);
        let payloads: Vec<&[u8]> = vec![&a, &b];
        let grid = compose_grid(&payloads, 512).expect("grid");
        let decoded = image::load_from_memory(&grid).expect("decode grid");
        // 2 images → 1 row, 2 cols → 1024x512.
        assert_eq!(decoded.width(), 1024);
        assert_eq!(decoded.height(), 512);
    }
}
