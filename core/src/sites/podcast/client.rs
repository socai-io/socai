//! Public BidClub HTTP API. Only explicit projections cross the model boundary.
use anyhow::{bail, Context, Result};
use futures::{stream, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;

const ORIGIN: &str = "https://bidclub.ai";

async fn get(path: &str, query: &[(String, String)]) -> Result<Value> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(40))
        .user_agent("Socai/Podcast")
        .build()?;
    // One bounded retry for transient read failures; never retry invalid input.
    for attempt in 0..2 {
        let response = client
            .get(format!("{ORIGIN}{path}"))
            .query(query)
            .send()
            .await;
        match response {
            Ok(response) => {
                let status = response.status();
                if (status.is_server_error() || status.as_u16() == 429) && attempt == 0 {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
                if !status.is_success() {
                    bail!("BidClub HTTP {status} for {path}");
                }
                return response
                    .json()
                    .await
                    .context("Invalid BidClub JSON response");
            }
            Err(_) if attempt == 0 => tokio::time::sleep(Duration::from_secs(1)).await,
            Err(error) => return Err(error).context("BidClub request failed"),
        }
    }
    bail!("BidClub request failed")
}

fn integer(args: &Value, key: &str, default: u64, min: u64, max: u64) -> Result<u64> {
    let n = match args.get(key) {
        None => default,
        Some(value) => value
            .as_u64()
            .with_context(|| format!("{key} must be an integer"))?,
    };
    anyhow::ensure!(
        (min..=max).contains(&n),
        "{key} must be between {min} and {max}"
    );
    Ok(n)
}

fn text<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>> {
    match args.get(key) {
        None => Ok(None),
        Some(value) => Ok(Some(
            value
                .as_str()
                .with_context(|| format!("{key} must be text"))?
                .trim(),
        )),
    }
}

fn metadata(value: &Value) -> Value {
    let mut out = json!({});
    for key in [
        "original_lang",
        "requested_language",
        "show_avatar_url",
        "slug",
        "show_id",
        "title",
        "display_title",
        "dek",
        "lang",
        "date",
        "published_at",
        "duration_min",
        "source_url",
        "source_label",
        "rss_url",
        "youtube_url",
        "thumbnail_url",
        "chips",
        "provenance",
        "shows",
    ] {
        if let Some(value) = value.get(key).filter(|v| !v.is_null()) {
            out[key] = value.clone();
        }
    }
    if let Some(slug) = value["slug"].as_str() {
        out["url"] = json!(format!("{ORIGIN}/e/{slug}"));
    }
    out
}

async fn detail(slug: &str) -> Result<Value> {
    anyhow::ensure!(
        !slug.is_empty()
            && slug.len() <= 300
            && slug
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "Provide the episode slug returned by search or episodes"
    );
    let mut value = get(&format!("/api/v1/episodes/{slug}"), &[]).await?;
    anyhow::ensure!(
        value["slug"].as_str() == Some(slug),
        "Unexpected BidClub episode response"
    );
    if let Some(show) = value["show_id"].as_str() {
        if let Some(avatar) = show_avatar(show).await {
            value["show_avatar_url"] = json!(avatar);
        }
    }
    Ok(value)
}

// Display language is independent from the original-language search filter.
fn localized(value: &Value, language: Option<&str>) -> Value {
    let mut out = value.clone();
    let original = value["lang"].as_str().unwrap_or("");
    out["original_lang"] = json!(original);
    let requested = language.unwrap_or(original);
    out["requested_language"] = json!(requested);
    let alternate = value["lang_alt"]
        .as_str()
        .is_some_and(|v| v.eq_ignore_ascii_case(requested));
    for key in ["title", "display_title", "dek", "tldr_md", "digest_md"] {
        let alt_key = format!("{key}_alt");
        let translated = alternate
            .then(|| value[&alt_key].as_str())
            .flatten()
            .filter(|s| !s.trim().is_empty());
        if let Some(body) = translated {
            out[key] = json!(body);
        }
        out[format!("{key}_language")] = json!(if translated.is_some() {
            requested
        } else {
            original
        });
    }
    if alternate
        && value["display_title_alt"]
            .as_str()
            .is_none_or(|v| v.trim().is_empty())
    {
        if let Some(title) = value["title_alt"].as_str().filter(|v| !v.trim().is_empty()) {
            out["display_title"] = json!(title);
        }
    }
    out
}

fn section(value: &Value, kind: &str, offset: usize, limit: usize) -> Result<Value> {
    let mut out = metadata(value);
    out["section"] = json!(kind);
    if kind == "metadata" {
        return Ok(out);
    }
    let content_language = match kind {
        "tldr" => value["tldr_md_language"].clone(),
        "digest" => value["digest_md_language"].clone(),
        _ => value["lang"].clone(),
    };
    out["content_language"] = content_language.clone();
    out["language_fallback"] =
        json!(kind != "full" && content_language != value["requested_language"]);
    if kind == "full" {
        let languages = [
            &value["tldr_md_language"],
            &value["digest_md_language"],
            &value["lang"],
        ];
        out["content_language"] = if languages.iter().all(|v| *v == languages[0]) {
            languages[0].clone()
        } else {
            json!("mixed")
        };
        out["language_fallback"] =
            json!(languages.iter().any(|v| *v != &value["requested_language"]));
        out["section_languages"] = json!({"tldr":value["tldr_md_language"], "digest":value["digest_md_language"], "transcript":value["lang"]});
    }
    let body = if kind == "full" {
        let mut parts = Vec::new();
        let mut missing = Vec::new();
        for (name, key) in [
            ("TL;DR", "tldr_md"),
            ("Digest", "digest_md"),
            ("Transcript", "transcript_md"),
        ] {
            match value[key].as_str().filter(|s| !s.trim().is_empty()) {
                Some(body) => parts.push(format!("# {name}\n\n{body}")),
                None => missing.push(name),
            }
        }
        if !missing.is_empty() {
            out["missing_sections"] = json!(missing);
        }
        parts.join("\n\n")
    } else {
        let key = match kind {
            "tldr" => "tldr_md",
            "digest" => "digest_md",
            "transcript" => "transcript_md",
            _ => bail!("section must be metadata, tldr, digest, transcript or full"),
        };
        value[key].as_str().unwrap_or_default().to_owned()
    };
    let total = body.chars().count();
    anyhow::ensure!(
        offset <= total,
        "offset {offset} exceeds section length {total}"
    );
    out["available"] = json!(!body.trim().is_empty());
    out["content"] = json!(body.chars().skip(offset).take(limit).collect::<String>());
    out["offset"] = json!(offset);
    out["total_chars"] = json!(total);
    out["next_offset"] = if offset.saturating_add(limit) < total {
        json!(offset + limit)
    } else {
        Value::Null
    };
    Ok(out)
}

pub async fn execute(command: &str, args: Value) -> Result<Value> {
    let language = text(&args, "language")?.map(str::to_uppercase);
    anyhow::ensure!(
        language
            .as_deref()
            .is_none_or(|v| ["EN", "ZH"].contains(&v)),
        "language must be EN or ZH"
    );
    if command == "read" {
        let slug = crate::sites::required_string(&args, "slug")?;
        let kind = text(&args, "section")?.unwrap_or("tldr");
        anyhow::ensure!(
            ["metadata", "tldr", "digest", "transcript", "full"].contains(&kind),
            "Unknown section: {kind}"
        );
        let offset = integer(&args, "offset", 0, 0, 10_000_000)? as usize;
        let limit = integer(&args, "max_chars", 12000, 1, 20000)? as usize;
        return section(
            &localized(&detail(&slug).await?, language.as_deref()),
            kind,
            offset,
            limit,
        );
    }
    if command == "shows" {
        let value = get("/api/v1/shows", &[]).await?;
        let shows = value["shows"]
            .as_array()
            .context("Missing BidClub shows array")?;
        let query = text(&args, "query")?.unwrap_or("").to_lowercase();
        let matches: Vec<_> = shows
            .iter()
            .filter(|show| {
                ["name", "id", "hosts"]
                    .iter()
                    .any(|key| show[*key].to_string().to_lowercase().contains(&query))
            })
            .cloned()
            .collect();
        return Ok(
            json!({"provider":"bidclub", "coverage":"BidClub indexed shows, not all podcasts", "count":matches.len(), "shows":matches}),
        );
    }
    anyhow::ensure!(
        command == "search" || command == "episodes",
        "Unknown podcast command"
    );
    let limit = integer(&args, "limit", 5, 1, 20)? as usize;
    let mut query = Vec::new();
    for key in ["show", "lang"] {
        if let Some(value) = text(&args, key)?.filter(|s| !s.is_empty()) {
            let value = if key == "lang" {
                value.to_uppercase()
            } else {
                value.to_owned()
            };
            anyhow::ensure!(
                key != "lang" || ["EN", "ZH"].contains(&value.as_str()),
                "lang must be EN or ZH"
            );
            query.push((key.to_owned(), value));
        }
    }
    let path = if command == "search" {
        let q = crate::sites::required_string(&args, "query")?;
        anyhow::ensure!(
            (2..=100).contains(&q.chars().count()),
            "query must contain 2–100 characters"
        );
        query.push(("q".into(), q));
        "/api/v1/search"
    } else {
        let offset = integer(&args, "offset", 0, 0, 10_000_000)?;
        query.push(("limit".into(), limit.to_string()));
        query.push(("offset".into(), offset.to_string()));
        "/api/v1/episodes"
    };
    let response = get(path, &query).await?;
    let key = if command == "search" {
        "results"
    } else {
        "episodes"
    };
    let rows = response[key]
        .as_array()
        .context("Missing BidClub episode array")?;
    // Keep upstream order, at most two in-flight detail reads, and never expose
    // the upstream digest/transcript payload in default search/list output.
    let language = language.as_deref();
    let episodes: Vec<Value> =
        stream::iter(rows.iter().take(limit).cloned().map(|row| async move {
            let result = match row["slug"].as_str() {
                Some(slug) => detail(slug)
                    .await
                    .and_then(|v| section(&localized(&v, language), "tldr", 0, 4000)),
                None => Err(anyhow::anyhow!("Episode has no slug")),
            };
            match result {
                Ok(value) => value,
                Err(error) => {
                    let mut value = metadata(&row);
                    value["detail_error"] = json!(error.to_string());
                    value
                }
            }
        }))
        .buffered(2)
        .collect()
        .await;
    let incomplete = episodes.iter().any(|v| v.get("detail_error").is_some());
    let mut out = json!({"provider":"bidclub", "coverage":"BidClub indexed episodes, not all podcasts", "count":episodes.len(), "episodes":episodes});
    if incomplete {
        out["details_incomplete"] = json!(true);
    }
    if command == "search" {
        out["query"] = response["q"].clone();
        out["returned_by_provider"] = json!(rows.len());
        out["omitted_by_limit"] = json!(rows.len().saturating_sub(limit));
        out["partial"] = response.get("partial").cloned().unwrap_or(json!(false));
        if let Some(recent) = response.get("searched_recent") {
            out["searched_recent"] = recent.clone();
        }
        out["search_scope"] = json!("Matches may be in title, summary or transcript. Provider returns at most 20 hits without pagination; count is not a total match count. partial=true means only a recent subset was searched; zero hits is not exhaustive.");
    } else {
        out["pagination"] = response
            .get("pagination")
            .cloned()
            .context("Missing BidClub pagination")?;
    }
    Ok(out)
}

// RSS channel artwork is the show's identity; episode/video thumbnails are not.
async fn show_avatar(show: &str) -> Option<String> {
    use std::{collections::HashMap, sync::OnceLock, time::Instant};
    type Cache = HashMap<String, (Instant, Option<String>)>;
    static CACHE: OnceLock<tokio::sync::Mutex<Cache>> = OnceLock::new();
    static SHOWS: tokio::sync::OnceCell<Value> = tokio::sync::OnceCell::const_new();
    let mut cache = CACHE
        .get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
        .lock()
        .await;
    if let Some((time, image)) = cache.get(show) {
        if time.elapsed() < Duration::from_secs(3600) {
            return image.clone();
        }
    }
    let image = async {
        let shows = SHOWS
            .get_or_try_init(|| get("/api/v1/shows", &[]))
            .await
            .ok()?;
        let feed = shows["shows"]
            .as_array()?
            .iter()
            .find(|s| s["id"] == show)?["sources"]["rss"]
            .as_str()?;
        let url = reqwest::Url::parse(feed).ok()?;
        if url.scheme() != "https" || url.domain().is_none() {
            return None;
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()
            .ok()?;
        let mut response = client.get(url).send().await.ok()?.error_for_status().ok()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.ok()? {
            let remaining = 262144usize.saturating_sub(bytes.len());
            bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            // The channel artwork normally precedes the first item. Stop as soon
            // as it is available instead of downloading a multi-megabyte feed.
            if let Some(image) = feed_avatar(&bytes) {
                return Some(image);
            }
            if bytes.len() >= 262144 {
                break;
            }
        }
        None
    }
    .await;
    cache.insert(show.to_owned(), (Instant::now(), image.clone()));
    image
}

fn feed_avatar(bytes: &[u8]) -> Option<String> {
    use quick_xml::{events::Event, Reader};
    let mut reader = Reader::from_reader(bytes);
    let mut in_image = false;
    let mut in_url = false;
    loop {
        match reader.read_event().ok()? {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let name = e.name();
                if name.as_ref() == b"item" {
                    return None;
                }
                if name.as_ref() == b"itunes:image" {
                    for a in e.attributes().flatten() {
                        if a.key.as_ref() == b"href" {
                            let url = a
                                .decoded_and_normalized_value(
                                    quick_xml::XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .ok()?;
                            if url.starts_with("https://") {
                                return Some(url.into_owned());
                            }
                        }
                    }
                }
                if name.as_ref() == b"image" {
                    in_image = true;
                }
                if in_image && name.as_ref() == b"url" {
                    in_url = true;
                }
            }
            Event::Text(e) if in_url => {
                let value = e.decode().ok()?;
                let url = quick_xml::escape::unescape(&value).ok()?;
                if url.starts_with("https://") {
                    return Some(url.into_owned());
                }
            }
            Event::End(e) => {
                if e.name().as_ref() == b"image" {
                    in_image = false;
                }
                if e.name().as_ref() == b"url" {
                    in_url = false;
                }
            }
            Event::Eof => return None,
            _ => {}
        }
    }
}

pub(super) async fn complete_section(slug: &str, kind: &str, language: &str) -> Result<Value> {
    section(
        &localized(&detail(slug).await?, Some(language)),
        kind,
        0,
        usize::MAX,
    )
}

pub(super) async fn participants(slug: &str) -> Result<Vec<String>> {
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?
        .get(format!("{ORIGIN}/e/{slug}"))
        .send()
        .await?
        .error_for_status()?;
    let html = response.text().await?;
    let document = dom_query::Document::from(html);
    for script in document.select("script").iter() {
        let text = script.text();
        let Some(data) = text
            .trim()
            .strip_prefix("self.__next_f.push(")
            .and_then(|s| s.strip_suffix(')'))
        else {
            continue;
        };
        let Ok(frame) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        let Some(payload) = frame[1].as_str() else {
            continue;
        };
        let Some((_, tail)) = payload.split_once("\"participants\":") else {
            continue;
        };
        if let Some(Ok(Value::Array(people))) = serde_json::Deserializer::from_str(tail)
            .into_iter::<Value>()
            .next()
        {
            let people = people
                .into_iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .filter(|s| !s.trim().is_empty())
                .collect::<Vec<_>>();
            if !people.is_empty() {
                return Ok(people);
            }
        }
    }
    Ok(Vec::new())
}
