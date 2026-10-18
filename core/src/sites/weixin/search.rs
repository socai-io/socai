//! HTTP-only Sogou result retrieval. Never follows an article URL.
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct SearchSession {
    pub cookies: Vec<Value>,
    pub referrer: String,
    created: Instant,
}
// Only retain anonymous Sogou session cookies in memory, never in tool output.
static SESSIONS: LazyLock<Mutex<HashMap<String, SearchSession>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
pub fn session_for(url: &str) -> Option<SearchSession> {
    SESSIONS
        .lock()
        .ok()?
        .get(url)
        .filter(|s| s.created.elapsed() < Duration::from_secs(3600))
        .cloned()
}

use dom_query::Document;
use serde_json::{json, Value};

pub async fn search_page(
    client: &reqwest::Client,
    query: &str,
    page: i64,
    limit: usize,
) -> anyhow::Result<Value> {
    let response = client
        .get("https://weixin.sogou.com/weixin")
        .query(&[("type", "2"), ("query", query), ("page", &page.to_string())])
        .send()
        .await?;
    let url = response.url().clone();
    let status = response.status();
    if let Some(gate) = super::retry::response_gate(
        status.as_u16(),
        response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok()),
    ) {
        return Ok(gate);
    }
    if !status.is_success() {
        return Ok(json!({"ok":false,"reason":"search_http_error","http_status":status.as_u16()}));
    }
    let cookies = response.headers().get_all(reqwest::header::SET_COOKIE).iter()
        .filter_map(|header| cookie::Cookie::parse(header.to_str().ok()?).ok())
        .filter(|cookie| matches!(cookie.domain().unwrap_or("weixin.sogou.com"), "sogou.com" | "weixin.sogou.com"))
        .map(|cookie| json!({ "name":cookie.name(), "value":cookie.value(),
            "domain": cookie.domain().map(|d| format!(".{d}")).unwrap_or_else(|| "weixin.sogou.com".into()),
            "path": cookie.path().unwrap_or("/"), "secure":cookie.secure().unwrap_or(false),
            "httpOnly": cookie.http_only().unwrap_or(false) }))
        .collect::<Vec<_>>();
    let html = response.text().await?;
    let result = parse_results(&html, &url, query, page, limit)?;
    if let Ok(mut sessions) = SESSIONS.lock() {
        sessions.retain(|_, s| s.created.elapsed() < Duration::from_secs(3600));
        if sessions.len() >= 300 {
            sessions.clear();
        }
        for card in result["results"].as_array().into_iter().flatten() {
            if let Some(link) = card["url"].as_str() {
                sessions.insert(
                    link.to_string(),
                    SearchSession {
                        cookies: cookies.clone(),
                        referrer: url.to_string(),
                        created: Instant::now(),
                    },
                );
            }
        }
    }
    Ok(result)
}

pub fn client(wait: u64) -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(wait))
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        // Search redirects stay on Sogou. Article links are opened only in Chrome.
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 { return attempt.error("too many Sogou redirects"); }
            if attempt.url().scheme() == "https" && attempt.url().host_str() == Some("weixin.sogou.com") {
                attempt.follow()
            } else { attempt.stop() }
        }))
        .build()?)
}

fn parse_results(
    html: &str,
    url: &reqwest::Url,
    query: &str,
    page: i64,
    limit: usize,
) -> anyhow::Result<Value> {
    let document = Document::from(html);
    let body = document.select("body").text().to_string();
    let has_results = document.select(".news-list h3 a").exists();
    let failure = |reason| json!({"ok":false,"reason":reason,"url":url.as_str()});
    if url.path().contains("antispider")
        || document.select("#seccodeImage, #tcaptcha_iframe").exists()
        || !has_results
            && ["请输入验证码", "安全验证", "完成验证"]
                .iter()
                .any(|word| body.contains(word))
    {
        return Ok(failure("challenge_required"));
    }
    if !has_results
        && ["访问过于频繁", "操作频繁"]
            .iter()
            .any(|word| body.contains(word))
    {
        return Ok(failure("rate_limited"));
    }
    if url.path() != "/weixin" || !url.query_pairs().any(|(k, v)| k == "query" && v == query) {
        return Ok(failure("unexpected_search_redirect"));
    }
    let mut results = Vec::new();
    for row in document.select(".news-list > li").iter() {
        let anchor = row.select("h3 a");
        let Some(href) = anchor.attr("href") else {
            continue;
        };
        let link = url.join(&href)?;
        if super::validate_article_url(link.as_str()).is_err() {
            continue;
        }
        let mut card = json!({ "title": anchor.text().trim(), "url": link.as_str(),
            "snippet": row.select(".txt-info").text().trim(),
            "author": row.select(".s-p .all-time-y2").text().trim() });
        let time = row.select(".s-p .s2");
        let source = time.text();
        if let Some(timestamp) = source
            .split("timeConvert('")
            .nth(1)
            .and_then(|s| s.split('\'').next())
            .and_then(|s| s.parse::<i64>().ok())
        {
            card["published_at"] = json!(chrono::DateTime::from_timestamp(timestamp, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default());
        } else if !time.select("script").exists() {
            card["published_at"] = json!(source.trim());
        }
        results.push(card);
        if results.len() >= limit {
            break;
        }
    }
    if results.is_empty()
        && !["没有找到", "未找到", "暂无相关", "找不到和"]
            .iter()
            .any(|word| body.contains(word))
    {
        return Ok(failure("search_results_unavailable"));
    }
    let has_more = document
        .select("a")
        .iter()
        .any(|a| a.text().trim() == "下一页");
    Ok(json!({"query":query,"page":page,"results":results,"has_more":has_more}))
}
