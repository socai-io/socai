use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DouyinVideoCard {
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
pub struct DouyinComment {
    pub comment_id: String,
    pub author: String,
    pub author_id: String,
    pub author_url: String,
    pub text: String,
    pub likes: String,
    pub time: String,
    pub replies: Vec<DouyinComment>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DouyinVideo {
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
    pub author_url: String,
    pub likes: String,
    pub comments_count: String,
    pub shares: String,
    pub favorites: String,
    pub views: String,
    pub duration_seconds: i64,
    pub cover_url: String,
    pub video: Value,
    pub top_comments: Vec<DouyinComment>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DouyinAuthorProfile {
    pub entity_type: String,
    pub platform: String,
    pub author_id: String,
    pub display_name: String,
    pub handle: String,
    pub url: String,
    pub bio: String,
    pub verified: bool,
    pub followers: String,
    pub following: String,
    pub likes: String,
    pub video_count: String,
    pub video_cards: Vec<DouyinVideoCard>,
}

pub fn normalize_douyin_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.starts_with("//") {
        return format!("https:{trimmed}");
    }
    if trimmed.starts_with('/') {
        return format!("https://www.douyin.com{trimmed}");
    }
    trimmed.to_string()
}

pub fn extract_video_id(url: &str) -> String {
    for marker in ["/video/", "/note/", "/share/video/"] {
        if let Some(rest) = url.split(marker).nth(1) {
            let id = rest
                .split(['?', '#', '/', '&'])
                .next()
                .unwrap_or_default()
                .trim();
            if !id.is_empty() {
                return id.to_string();
            }
        }
    }
    String::new()
}

pub fn extract_author_id(url: &str) -> String {
    let Some(rest) = url.split("/user/").nth(1) else {
        return String::new();
    };
    rest.split(['?', '#', '/', '&'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub fn douyin_video_url(locator: &str) -> anyhow::Result<String> {
    let value = normalize_douyin_url(locator);
    if value.chars().all(|ch| ch.is_ascii_digit()) && !value.is_empty() {
        return Ok(format!("https://www.douyin.com/video/{value}"));
    }
    if let Some(url) = parse_douyin_url(&value) {
        if !extract_video_id(url.as_str()).is_empty() || is_douyin_short_url(&url) {
            return Ok(url.to_string());
        }
    }
    anyhow::bail!("invalid Douyin video id or URL")
}

pub fn douyin_author_url(locator: &str) -> anyhow::Result<String> {
    let value = normalize_douyin_url(locator);
    if valid_author_id(&value) {
        return Ok(format!("https://www.douyin.com/user/{value}"));
    }
    if let Some(url) = parse_douyin_url(&value) {
        if valid_author_id(&extract_author_id(url.as_str())) {
            return Ok(url.to_string());
        }
    }
    anyhow::bail!("invalid Douyin author id or URL")
}

pub(crate) fn is_douyin_page_url(value: &str) -> bool {
    parse_douyin_url(value).is_some()
}

fn parse_douyin_url(value: &str) -> Option<reqwest::Url> {
    let url = reqwest::Url::parse(value).ok()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    if host == "douyin.com"
        || host.ends_with(".douyin.com")
        || host == "iesdouyin.com"
        || host.ends_with(".iesdouyin.com")
    {
        Some(url)
    } else {
        None
    }
}

fn is_douyin_short_url(url: &reqwest::Url) -> bool {
    matches!(
        url.host_str()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "v.douyin.com" | "iesdouyin.com" | "www.iesdouyin.com"
    )
}

fn valid_author_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_relative_douyin_url() {
        assert_eq!(
            normalize_douyin_url("/video/123"),
            "https://www.douyin.com/video/123"
        );
        assert_eq!(
            normalize_douyin_url("//www.douyin.com/video/123"),
            "https://www.douyin.com/video/123"
        );
        assert_eq!(
            douyin_video_url("123").unwrap(),
            "https://www.douyin.com/video/123"
        );
        assert_eq!(
            douyin_author_url("MS4wLjABAAAA").unwrap(),
            "https://www.douyin.com/user/MS4wLjABAAAA"
        );
        assert!(douyin_video_url("https://example.com/video/123").is_err());
        assert!(douyin_video_url("https://douyin.com:443@evil.example/video/123").is_err());
        assert!(douyin_video_url("http://www.douyin.com/video/123").is_err());
        assert_eq!(
            douyin_video_url("https://v.douyin.com/abc123/").unwrap(),
            "https://v.douyin.com/abc123/"
        );
    }

    #[test]
    fn extracts_video_id_from_video_or_note_url() {
        assert_eq!(
            extract_video_id("https://www.douyin.com/video/7354264417699204363?foo=1"),
            "7354264417699204363"
        );
        assert_eq!(
            extract_video_id("https://www.douyin.com/note/123456#comment"),
            "123456"
        );
        assert_eq!(
            extract_video_id("https://www.iesdouyin.com/share/video/654321/"),
            "654321"
        );
        assert_eq!(
            extract_author_id("https://www.douyin.com/user/MS4wLjABAAAA?from_tab_name=main"),
            "MS4wLjABAAAA"
        );
    }
}
