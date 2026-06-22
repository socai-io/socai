use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// XHS note — wire-ready. Field order, names, and types match the public JSON
/// shape used by run artifacts and app timelines.
///
/// All normalization (strip URL fragment, clip hashtags to 12, image_count
/// fallback to images.len()) is performed during parsing, so serializing
/// directly with serde_json yields stable output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XhsNote {
    pub note_id: String,
    pub url: String,
    pub r#type: String,
    pub title: String,
    pub author: String,
    pub author_id: String,
    pub author_url: String,
    pub content: String,
    pub content_source: String,
    pub hashtags: Vec<String>,
    pub date: String,
    pub location: String,
    pub ip_location: String,
    pub likes: String,
    pub favorites: String,
    pub comments_count: String,
    pub image_count: i64,
    pub images: Vec<Value>,
    pub video: Value,
    pub extraction_level: String,

    /// Serialized as `"wait"` to keep the public note shape compact.
    #[serde(rename = "wait", skip_serializing_if = "Option::is_none", default)]
    pub wait_meta: Option<Value>,

    /// Only present when consecutive extracts return the same note_id —
    /// MVP doesn't implement the cross-call tracking, so this stays None.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub stale_warning: Option<String>,
}

/// Author profile entity. Wire shape includes the derived `_value` integer
/// fields produced by [`parse_count_text`].
#[derive(Debug, Clone, Default)]
pub struct XhsAuthorProfile {
    pub display_name: String,
    pub xhs_id: String,
    pub profile_url: String,
    pub bio: String,
    pub ip_location: String,
    pub followers: String,
    pub following: String,
    pub likes_and_collections: String,
    pub note_cards: Vec<XhsNoteCard>,
}

impl XhsAuthorProfile {
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("entity_type".into(), json!("author"));
        map.insert("display_name".into(), json!(self.display_name));
        map.insert("title".into(), json!(self.display_name));
        map.insert("xhs_id".into(), json!(self.xhs_id));
        map.insert(
            "profile_url".into(),
            json!(normalize_url(&self.profile_url)),
        );
        map.insert("url".into(), json!(normalize_url(&self.profile_url)));
        map.insert("bio".into(), json!(self.bio));
        map.insert("ip_location".into(), json!(self.ip_location));
        map.insert("followers".into(), json!(self.followers));
        map.insert(
            "followers_value".into(),
            json!(parse_count_text(&self.followers)),
        );
        map.insert("following".into(), json!(self.following));
        map.insert(
            "following_value".into(),
            json!(parse_count_text(&self.following)),
        );
        map.insert(
            "likes_and_collections".into(),
            json!(self.likes_and_collections),
        );
        map.insert(
            "likes_and_collections_value".into(),
            json!(parse_count_text(&self.likes_and_collections)),
        );
        map.insert("note_count".into(), json!(self.note_cards.len()));
        let cards: Vec<Value> = self
            .note_cards
            .iter()
            .map(|c| serde_json::to_value(c).unwrap_or(Value::Null))
            .collect();
        map.insert("note_cards".into(), Value::Array(cards));
        Value::Object(map)
    }
}

/// Parse a Xiaohongshu count text like "1.2k", "3万", "1,234". Returns
/// 0 on anything unparseable.
pub fn parse_count_text(raw: &str) -> i64 {
    let value: String = raw.trim().to_lowercase().replace([',', '+'], "");
    if value.is_empty() {
        return 0;
    }
    // Find leading numeric prefix (with optional decimal) + optional unit suffix.
    let bytes = value.as_bytes();
    let mut end = 0usize;
    let mut saw_dot = false;
    for (i, ch) in value.char_indices() {
        if ch.is_ascii_digit() {
            end = i + ch.len_utf8();
            continue;
        }
        if ch == '.' && !saw_dot {
            saw_dot = true;
            end = i + ch.len_utf8();
            continue;
        }
        break;
    }
    if end == 0 {
        return 0;
    }
    let number: f64 = std::str::from_utf8(&bytes[..end])
        .unwrap_or("0")
        .parse()
        .unwrap_or(0.0);
    let unit: String = value[end..]
        .chars()
        .take(1)
        .collect::<String>()
        .to_lowercase();
    let multiplier = match unit.as_str() {
        "万" | "w" => 10_000.0,
        "k" => 1_000.0,
        _ => 1.0,
    };
    (number * multiplier).round() as i64
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct XhsNoteCard {
    pub note_id: String,
    pub title: String,
    pub author: String,
    pub author_id: String,
    pub author_url: String,
    pub likes: String,
    pub link: String,
    pub cover_url: String,
    pub r#type: String,
    pub position: i64,
    pub xsec_token: String,
}

impl Default for XhsNote {
    fn default() -> Self {
        Self {
            note_id: String::new(),
            url: String::new(),
            r#type: String::new(),
            title: String::new(),
            author: String::new(),
            author_id: String::new(),
            author_url: String::new(),
            content: String::new(),
            content_source: String::new(),
            hashtags: Vec::new(),
            date: String::new(),
            location: String::new(),
            ip_location: String::new(),
            likes: String::new(),
            favorites: String::new(),
            comments_count: String::new(),
            image_count: 0,
            images: Vec::new(),
            // Keep video as {}, not null, so the wire shape stays consistent
            // for video-less notes.
            video: Value::Object(Default::default()),
            extraction_level: "lite".into(),
            wait_meta: None,
            stale_warning: None,
        }
    }
}

/// Drop the URL fragment, keep scheme/netloc/path/query intact for the URL
/// shapes XHS emits.
pub(crate) fn normalize_url(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    match trimmed.find('#') {
        Some(idx) => trimmed[..idx].to_string(),
        None => trimmed.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_url_strips_fragment() {
        assert_eq!(
            normalize_url("https://www.xiaohongshu.com/explore/abc?x=1#frag"),
            "https://www.xiaohongshu.com/explore/abc?x=1"
        );
    }

    #[test]
    fn normalize_url_passes_through_clean_urls() {
        assert_eq!(
            normalize_url("https://www.xiaohongshu.com/explore/abc?x=1"),
            "https://www.xiaohongshu.com/explore/abc?x=1"
        );
    }

    #[test]
    fn normalize_url_handles_empty() {
        assert_eq!(normalize_url(""), "");
        assert_eq!(normalize_url("   "), "");
    }

    #[test]
    fn parse_count_basic() {
        assert_eq!(parse_count_text(""), 0);
        assert_eq!(parse_count_text("0"), 0);
        assert_eq!(parse_count_text("1234"), 1234);
        assert_eq!(parse_count_text("1,234"), 1234);
        assert_eq!(parse_count_text("999+"), 999);
    }

    #[test]
    fn parse_count_chinese_wan() {
        assert_eq!(parse_count_text("1万"), 10_000);
        assert_eq!(parse_count_text("1.2万"), 12_000);
        assert_eq!(parse_count_text("3.5w"), 35_000);
    }

    #[test]
    fn parse_count_k_suffix() {
        assert_eq!(parse_count_text("1.5k"), 1_500);
        assert_eq!(parse_count_text("12K"), 12_000);
    }

    #[test]
    fn parse_count_unparseable() {
        assert_eq!(parse_count_text("none"), 0);
        assert_eq!(parse_count_text("--"), 0);
    }
}
