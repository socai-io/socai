use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TikTokVideoCard {
    pub video_id: String,
    pub url: String,
    pub title: String,
    pub author: String,
    pub author_id: String,
    pub author_url: String,
    pub likes: String,
    pub comments: String,
    pub shares: String,
    pub views: String,
    pub cover_url: String,
    pub duration_seconds: i64,
    pub position: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TikTokComment {
    pub comment_id: String,
    pub author: String,
    pub author_id: String,
    pub author_url: String,
    pub text: String,
    pub likes: String,
    pub time: String,
    pub replies: Vec<TikTokComment>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TikTokVideo {
    pub entity_type: String,
    pub platform: String,
    pub video_id: String,
    pub url: String,
    pub title: String,
    pub description: String,
    pub hashtags: Vec<String>,
    pub created_at: String,
    pub author: String,
    pub author_id: String,
    pub author_internal_id: String,
    pub author_url: String,
    pub likes: String,
    pub comments_count: String,
    pub shares: String,
    pub favorites: String,
    pub views: String,
    pub duration_seconds: i64,
    pub cover_url: String,
    pub video: Value,
    pub top_comments: Vec<TikTokComment>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TikTokAuthorProfile {
    pub entity_type: String,
    pub platform: String,
    pub author_id: String,
    pub author_internal_id: String,
    pub display_name: String,
    pub handle: String,
    pub url: String,
    pub bio: String,
    pub verified: bool,
    pub followers: String,
    pub following: String,
    pub likes: String,
    pub video_count: String,
    pub video_cards: Vec<TikTokVideoCard>,
}

pub fn normalize_tiktok_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.starts_with("//") {
        return format!("https:{trimmed}");
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return trimmed.to_string();
    }
    if trimmed.starts_with("www.")
        || trimmed.starts_with("vm.tiktok.com")
        || trimmed.starts_with("vt.tiktok.com")
    {
        return format!("https://{trimmed}");
    }
    trimmed.to_string()
}

pub fn extract_video_id(url: &str) -> String {
    for marker in ["/video/", "/player/v1/"] {
        let Some(rest) = url.split(marker).nth(1) else {
            continue;
        };
        let value = rest
            .split(['?', '#', '/', '&'])
            .next()
            .unwrap_or_default()
            .trim();
        if !value.is_empty() {
            return value.to_string();
        }
    }
    String::new()
}

pub fn extract_handle(url: &str) -> String {
    let Some(rest) = url.split("/@").nth(1) else {
        return String::new();
    };
    rest.split(['?', '#', '/', '&'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub fn tiktok_video_url(locator: &str) -> anyhow::Result<String> {
    let value = normalize_tiktok_url(locator);
    if value.chars().all(|ch| ch.is_ascii_digit()) && !value.is_empty() {
        return Ok(format!("https://www.tiktok.com/player/v1/{value}"));
    }
    if let Some(url) = parse_tiktok_url(&value) {
        if !extract_video_id(url.as_str()).is_empty() || is_tiktok_short_url(&url) {
            return Ok(url.to_string());
        }
    }
    anyhow::bail!("invalid TikTok video id or URL")
}

pub fn tiktok_author_url(locator: &str) -> anyhow::Result<String> {
    let value = normalize_tiktok_url(locator);
    let handle = value.trim_start_matches('@');
    if valid_handle(handle) {
        return Ok(format!("https://www.tiktok.com/@{handle}"));
    }
    if let Some(url) = parse_tiktok_url(&value) {
        if valid_handle(&extract_handle(url.as_str())) {
            return Ok(url.to_string());
        }
    }
    anyhow::bail!("invalid TikTok author handle or URL")
}

pub(crate) fn is_tiktok_page_url(value: &str) -> bool {
    parse_tiktok_url(value).is_some()
}

fn parse_tiktok_url(value: &str) -> Option<reqwest::Url> {
    let url = reqwest::Url::parse(value).ok()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    if host == "tiktok.com" || host.ends_with(".tiktok.com") {
        Some(url)
    } else {
        None
    }
}

fn is_tiktok_short_url(url: &reqwest::Url) -> bool {
    matches!(
        url.host_str()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "vm.tiktok.com" | "vt.tiktok.com"
    )
}

fn valid_handle(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 24
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
}
