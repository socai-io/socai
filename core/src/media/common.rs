use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{Map, Value};

use crate::media::md5;

pub const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/123 Safari/537.36";

#[derive(Debug, Clone)]
pub struct MediaConfig {
    pub base_dir: PathBuf,
    pub request_timeout_s: u64,
    /// Wall-clock cap for one cloud ASR round trip (upload + poll).
    pub asr_timeout_s: u64,
    pub max_audio_seconds: u64,
    pub use_ocr: bool,
    pub use_vision: bool,
    pub use_cloud_asr: bool,
    pub vision_concurrency: usize,
}

impl MediaConfig {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            request_timeout_s: 25,
            asr_timeout_s: 300,
            max_audio_seconds: 300,
            use_ocr: true,
            use_vision: true,
            use_cloud_asr: false,
            vision_concurrency: 3,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MediaUnavailable(pub String);

impl std::fmt::Display for MediaUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MediaUnavailable {}

pub(crate) fn ensure_dir(path: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(path)?;
    Ok(path.to_path_buf())
}

pub(crate) fn save_bytes(
    base_dir: &Path,
    payload: &[u8],
    label: &str,
    suffix: &str,
) -> Result<PathBuf> {
    let digest = md5::md5_hex(payload);
    let safe_label = sanitize_label(label, "media");
    let dir = ensure_dir(&base_dir.join(&safe_label))?;
    let suffix = if suffix.trim().is_empty() {
        ".bin"
    } else {
        suffix
    };
    let path = dir.join(format!("{safe_label}_{}{suffix}", &digest[..10]));
    std::fs::write(&path, payload)?;
    Ok(path)
}

pub(crate) fn save_named_bytes(
    base_dir: &Path,
    payload: &[u8],
    label: &str,
    filename: &str,
) -> Result<PathBuf> {
    let path = named_file_path(base_dir, label, filename)?;
    std::fs::write(&path, payload)?;
    Ok(path)
}

pub(crate) fn named_file_path(base_dir: &Path, label: &str, filename: &str) -> Result<PathBuf> {
    let safe_label = sanitize_label(label, "media");
    let safe_filename = sanitize_filename(filename, "asset.bin");
    let dir = ensure_dir(&base_dir.join(&safe_label))?;
    Ok(dir.join(safe_filename))
}

pub(crate) fn url_suffix(url: &str, fallback: &str) -> String {
    let without_query = url.split('?').next().unwrap_or("");
    let suffix = Path::new(without_query)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| format!(".{}", s.to_ascii_lowercase()))
        .unwrap_or_default();
    if !suffix.is_empty() && suffix.len() <= 8 {
        suffix
    } else {
        fallback.to_string()
    }
}

pub(crate) fn detect_media_type(payload: &[u8]) -> String {
    if payload.starts_with(&[0xff, 0xd8]) {
        "image/jpeg".into()
    } else if payload.starts_with(b"\x89PNG") {
        "image/png".into()
    } else if payload.starts_with(b"RIFF") && payload.get(8..12) == Some(b"WEBP") {
        "image/webp".into()
    } else {
        "application/octet-stream".into()
    }
}

pub(crate) fn short(text: &str, max_chars: usize) -> String {
    let value = text.trim();
    if value.chars().count() <= max_chars {
        value.to_string()
    } else {
        format!(
            "{}... [truncated]",
            value.chars().take(max_chars).collect::<String>()
        )
    }
}

pub(crate) fn insert_string(value: &mut Value, key: &str, text: impl Into<String>) {
    insert_value(value, key, Value::String(text.into()));
}

pub(crate) fn insert_value(value: &mut Value, key: &str, item: Value) {
    if let Some(map) = value.as_object_mut() {
        map.insert(key.to_string(), item);
    }
}

pub(crate) fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn sanitize_label(label: &str, fallback: &str) -> String {
    let cleaned: String = label
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

fn sanitize_filename(filename: &str, fallback: &str) -> String {
    let raw_name = Path::new(filename)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let cleaned: String = raw_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(['.', '_', '-']);
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn empty_object() -> Value {
    Value::Object(Map::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_named_bytes_writes_fixed_file_inside_label_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = save_named_bytes(dir.path(), b"poster", "note/one", "post.jpg").unwrap();

        assert_eq!(path, dir.path().join("note_one").join("post.jpg"));
        assert_eq!(std::fs::read(path).unwrap(), b"poster");
    }

    #[test]
    fn save_named_bytes_strips_path_components_from_filename() {
        let dir = tempfile::tempdir().unwrap();
        let path = save_named_bytes(dir.path(), b"poster", "note", "../post.jpg").unwrap();

        assert_eq!(path, dir.path().join("note").join("post.jpg"));
        assert_eq!(std::fs::read(path).unwrap(), b"poster");
    }
}
