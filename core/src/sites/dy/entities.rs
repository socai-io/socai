use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DouyinVideoCard {
    pub video_id: String,
    pub url: String,
    pub title: String,
    pub author: String,
    pub author_url: String,
    pub likes: String,
    pub comments: String,
    pub shares: String,
    pub cover_url: String,
    pub position: usize,
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
    for marker in ["/video/", "/note/"] {
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
    }
}
