//! Durable desktop previews. README/images stay outside the model context.
use anyhow::{ensure, Context, Result};
use dom_query::Document;
use futures::{stream, StreamExt};
use reqwest::Url;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

type EntryLock = Arc<tokio::sync::Mutex<()>>;
fn entry_lock(key: &str) -> EntryLock {
    static LOCKS: OnceLock<Mutex<HashMap<String, EntryLock>>> = OnceLock::new();
    LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(key.to_owned())
        .or_default()
        .clone()
}
fn active() -> &'static Mutex<HashSet<String>> {
    static ACTIVE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    ACTIVE.get_or_init(Default::default)
}
pub fn cache_root() -> Result<PathBuf> {
    let home = std::env::var_os("SOCAI_HOME").map(PathBuf::from).unwrap_or(
        crate::config::config_path()?
            .parent()
            .context("Missing config directory")?
            .to_path_buf(),
    );
    Ok(home.join("cache/github/v1"))
}

/// Repo subpages retain their original link, but share one repository snapshot.
fn target(raw: &str) -> Result<(String, String, String)> {
    let mut url = Url::parse(raw)?;
    ensure!(
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port_or_known_default() == Some(443),
        "Invalid GitHub URL"
    );
    let mut parts: Vec<_> = url
        .path_segments()
        .context("Invalid GitHub path")?
        .filter(|s| !s.is_empty())
        .collect();
    match url.host_str() {
        Some("api.github.com") if parts.first() == Some(&"repos") => {
            parts.remove(0);
        }
        Some("api.github.com")
            if matches!(parts.first(), Some(&"users" | &"orgs")) && parts.len() > 1 =>
        {
            return Ok((
                "profile".into(),
                parts[1].into(),
                format!("https://github.com/{}", parts[1]),
            ));
        }
        Some("github.com" | "raw.githubusercontent.com") => (),
        _ => anyhow::bail!("Unsupported GitHub link"),
    }
    let reserved = [
        "trending",
        "topics",
        "search",
        "collections",
        "explore",
        "marketplace",
        "sponsors",
        "orgs",
        "organizations",
        "settings",
        "login",
        "signup",
        "features",
        "about",
        "pricing",
        "readme",
        "notifications",
        "new",
        "codespaces",
        "enterprise",
        "security",
        "site",
        "apps",
        "events",
    ];
    let segment_ok = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    };
    if parts.len() >= 2
        && !reserved.contains(&parts[0])
        && segment_ok(parts[0])
        && segment_ok(parts[1])
    {
        let repo = format!("{}/{}", parts[0], parts[1].trim_end_matches(".git"));
        return Ok((
            "repository".into(),
            repo.clone(),
            format!("https://github.com/{repo}"),
        ));
    }
    if parts.len() == 1 && !reserved.contains(&parts[0]) && segment_ok(parts[0]) {
        return Ok((
            "profile".into(),
            parts[0].into(),
            format!("https://github.com/{}", parts[0]),
        ));
    }
    ensure!(
        url.host_str() == Some("github.com"),
        "Unsupported GitHub link"
    );
    url.set_fragment(None);
    Ok(("page".into(), url.to_string(), url.to_string()))
}

async fn save(path: &Path, value: &Value) -> Result<()> {
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    tokio::fs::write(&temp, serde_json::to_vec_pretty(value)?).await?;
    if let Err(error) = tokio::fs::rename(&temp, path).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error.into());
    }
    Ok(())
}
async fn cached(path: &Path) -> Option<Value> {
    serde_json::from_slice(&tokio::fs::read(path).await.ok()?).ok()
}
async fn api(path: &str, directory: &Path) -> Result<Value> {
    let reply = super::client::execute(json!({"url":path}), directory).await?;
    ensure!(
        reply["ok"] == true,
        "{}",
        reply.get("data").unwrap_or(&reply)
    );
    let full = reply["full_response_path"]
        .as_str()
        .context("Missing GitHub response")?;
    Ok(serde_json::from_slice(&tokio::fs::read(full).await?)?)
}

async fn rendered_readme(repo: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(35))
        .user_agent("socai-github-preview/0.1")
        .redirect(reqwest::redirect::Policy::custom(|a| {
            if a.previous().len() < 3
                && a.url().scheme() == "https"
                && a.url().host_str() == Some("api.github.com")
            {
                a.follow()
            } else {
                a.error("Unexpected README redirect")
            }
        }))
        .build()?;
    let mut request = client
        .get(format!("https://api.github.com/repos/{repo}/readme"))
        .header("Accept", "application/vnd.github.html+json");
    if let Some(token) = super::client::token().await {
        request = request.bearer_auth(token);
    }
    let mut response = request.send().await?.error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 4 * 1024 * 1024,
            "README exceeds preview size limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    let html = String::from_utf8(bytes)?;
    ensure!(
        html.trim_start().starts_with('<'),
        "GitHub did not return rendered README HTML"
    );
    Ok(html)
}

fn image_url(base: &Url, src: &str) -> Option<String> {
    let mut url = base.join(src).ok()?;
    if url.host_str() == Some("github.com") && url.path().contains("/blob/") {
        url.set_path(&url.path().replacen("/blob/", "/raw/", 1));
    }
    media_allowed(&url).then(|| url.to_string())
}
fn media_allowed(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && url.host_str().is_some_and(|h| {
            h == "github.com"
                || h.ends_with(".githubusercontent.com")
                || h.ends_with(".githubassets.com")
        })
}

fn prepare_html(html: &str, raw_base: &Url, web_base: &Url) -> (String, Vec<Value>, String) {
    let document = Document::from(html);
    document
        .select("script,style,iframe,object,embed,video,audio,source,link,meta,base,svg,form")
        .remove();
    let mut media = Vec::<Value>::new();
    let mut indexes = HashMap::new();
    for img in document.select("img").iter() {
        let src = img.attr("src").map(|s| s.to_string()).unwrap_or_default();
        img.remove_attrs(&["src", "srcset", "style", "data-canonical-src"]);
        let source = image_url(raw_base, &src);
        let key = source.clone().unwrap_or(src.clone());
        let index=*indexes.entry(key).or_insert_with(|| {
            let index=media.len();
            media.push(json!({"kind":"image","source_url":source.clone().unwrap_or(src),
                "alt":img.attr("alt").map(|s|s.to_string()).unwrap_or_default(),
                "download_state":if source.is_some(){"loading"}else{"failed"},
                "download_error":if source.is_none(){Some("Image URL is not on GitHub's media hosts")}else{None}}));
            index
        });
        img.set_attr("data-github-image", &index.to_string());
    }
    for anchor in document.select("a[href]").iter() {
        if let Some(href) = anchor.attr("href") {
            if href.starts_with('#') {
                continue;
            }
            if let Ok(url) = web_base.join(&href) {
                if matches!(url.scheme(), "https" | "http") {
                    anchor.set_attr("href", url.as_str());
                } else {
                    anchor.remove_attr("href");
                }
            }
        }
    }
    let text = document
        .select("body")
        .text()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (document.select("body").html().to_string(), media, text)
}

/// Local cache is authoritative until an explicit refresh. Failed refreshes leave
/// the prior snapshot usable offline. Image downloads continue independently.
pub async fn read(raw: &str, refresh: bool) -> Result<Value> {
    let (kind, identity, canonical) = target(raw)?;
    let key = format!("{:x}", Sha256::digest(canonical.to_lowercase().as_bytes()));
    let dir = cache_root()?.join(&key);
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join("preview.json");
    let lock = entry_lock(&key);
    let _guard = lock.lock().await;
    let mut note = if !refresh { cached(&path).await } else { None };
    if note.is_none() {
        static READS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(3);
        let _permit = READS.acquire().await?;
        let fetched = chrono::Utc::now().to_rfc3339();
        let mut value = json!({"site":"github","note_id":format!("github:{identity}"),"native_id":identity,
            "url":canonical,"title":identity,"archived":true,"level":"detail","media":[],
            "github":{"kind":kind,"fetched_at":fetched,"cache_dir":dir,"readme_available":false}});
        if kind == "repository" {
            let meta = api(&format!("/repos/{identity}"), &dir).await?;
            save(&dir.join("repository.json"), &meta).await?;
            value["title"] = meta["full_name"].clone();
            value["excerpt"] = meta["description"].clone();
            value["author"] =
                json!({"name":meta["owner"]["login"],"url":meta["owner"]["html_url"]});
            value["github"]["metadata"] = json!({"stars":meta["stargazers_count"],"forks":meta["forks_count"],
                "language":meta["language"],"created_at":meta["created_at"],"updated_at":meta["updated_at"],
                "pushed_at":meta["pushed_at"],"topics":meta["topics"],"license":meta["license"]["spdx_id"],
                "archived":meta["archived"],"default_branch":meta["default_branch"],"homepage":meta["homepage"],"open_issues":meta["open_issues_count"]});
            let readme = async {
                let data = api(&format!("/repos/{identity}/readme"), &dir).await?;
                let markdown = data["content"]
                    .as_str()
                    .context("README content unavailable")?;
                ensure!(data["encoding"] == "utf-8", "README is not UTF-8 text");
                tokio::fs::write(dir.join("readme.md"), markdown).await?;
                save(&dir.join("readme.json"), &data).await?;
                let html = rendered_readme(&identity).await?;
                let raw_base = Url::parse(
                    data["download_url"]
                        .as_str()
                        .context("README has no raw URL")?,
                )?;
                let web_base = Url::parse(
                    data["html_url"]
                        .as_str()
                        .context("README has no source URL")?,
                )?;
                Ok::<_, anyhow::Error>((
                    prepare_html(&html, &raw_base, &web_base),
                    data["html_url"].clone(),
                ))
            }
            .await;
            match readme {
                Ok(((html, images, text), url)) => {
                    tokio::fs::write(dir.join("readme.html"), &html).await?;
                    value["github"]["readme_html"] = json!(html);
                    value["github"]["readme_url"] = url;
                    value["github"]["readme_available"] = json!(true);
                    value["content"] = json!(text);
                    value["media"] = json!(images);
                }
                Err(error) => {
                    if let Some(previous) = cached(&path).await {
                        if previous["github"]["readme_available"] == true {
                            for field in ["readme_html", "readme_url", "readme_available"] {
                                value["github"][field] = previous["github"][field].clone();
                            }
                            value["content"] = previous["content"].clone();
                            value["media"] = previous["media"].clone();
                        }
                    }
                    value["github"]["readme_error"] = json!(format!("{error:#}"));
                }
            }
            if let Some(avatar) = meta["owner"]["avatar_url"].as_str() {
                value["github"]["avatar_source"] = json!(avatar);
            }
        } else if kind == "profile" {
            let meta = api(&format!("/users/{identity}"), &dir).await?;
            save(&dir.join("profile.json"), &meta).await?;
            value["title"] = json!(meta["name"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(&identity));
            value["excerpt"] = meta["bio"].clone();
            value["author"] = json!({"name":meta["login"],"url":canonical});
            value["github"]["metadata"] = json!({"created_at":meta["created_at"],"followers":meta["followers"],"public_repos":meta["public_repos"],"location":meta["location"],"company":meta["company"],"homepage":meta["blog"]});
            value["github"]["avatar_source"] = meta["avatar_url"].clone();
        } else {
            let data = api(&canonical, &dir).await?;
            value["title"] = data["title"].clone();
            value["content"] = json!(data["text"].as_str().unwrap_or(""));
            value["github"]["page_data"] = data;
        }
        save(&path, &value).await?;
        note = Some(value);
    }
    let mut note = note.unwrap();
    schedule_images(key, dir, note.clone());
    note["url"] = json!(raw);
    note["source_url"] = json!(canonical);
    Ok(note)
}

struct ActiveDownload(String);
impl Drop for ActiveDownload {
    fn drop(&mut self) {
        active().lock().unwrap().remove(&self.0);
    }
}

fn schedule_images(key: String, dir: PathBuf, snapshot: Value) {
    let mut jobs = Vec::new();
    for (i, image) in snapshot["media"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        if image["download_state"] == "loading" {
            if let Some(url) = image["source_url"].as_str() {
                jobs.push((Some(i), url.to_owned()));
            }
        }
    }
    if snapshot
        .pointer("/author/avatar")
        .and_then(Value::as_str)
        .is_none()
    {
        if let Some(url) = snapshot["github"]["avatar_source"].as_str() {
            if snapshot["github"]["avatar_state"] != "failed" {
                jobs.push((None, url.to_owned()));
            }
        }
    }
    if jobs.is_empty() || !active().lock().unwrap().insert(key.clone()) {
        return;
    }
    tokio::spawn(async move {
        let _active = ActiveDownload(key.clone());
        stream::iter(jobs)
            .map(|(index, url)| {
                let dir = dir.clone();
                let key = key.clone();
                let generation = snapshot["github"]["fetched_at"].clone();
                async move {
                    static LIMIT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
                    let _permit = LIMIT.acquire().await;
                    let outcome = download_image(&dir, &url).await;
                    let lock = entry_lock(&key);
                    let _guard = lock.lock().await;
                    let path = dir.join("preview.json");
                    if let Some(mut current) = cached(&path).await {
                        if current["github"]["fetched_at"] != generation {
                            return;
                        }
                        if let Some(i) = index {
                            let image = &mut current["media"][i];
                            match outcome {
                                Ok(path) => {
                                    image["src"] = json!(path);
                                    image["download_state"] = json!("ready");
                                }
                                Err(error) => {
                                    image["download_state"] = json!("failed");
                                    image["download_error"] = json!(error.to_string());
                                }
                            }
                        } else {
                            match outcome {
                                Ok(path) => {
                                    current["author"]["avatar"] = json!(path);
                                    current["github"]["avatar_state"] = json!("ready");
                                }
                                Err(_) => current["github"]["avatar_state"] = json!("failed"),
                            }
                        }
                        let _ = save(&path, &current).await;
                    }
                }
            })
            .buffer_unordered(4)
            .collect::<Vec<_>>()
            .await;
    });
}

async fn download_image(dir: &Path, raw: &str) -> Result<String> {
    let url = Url::parse(raw)?;
    ensure!(media_allowed(&url), "Image outside GitHub media hosts");
    let directory = dir.join("images");
    tokio::fs::create_dir_all(&directory).await?;
    let stem = format!("{:x}", Sha256::digest(raw.as_bytes()));
    for ext in ["png", "jpg", "gif", "webp", "svg", "img"] {
        let path = directory.join(format!("{stem}.{ext}"));
        if path.is_file() {
            return Ok(path.to_string_lossy().into_owned());
        }
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(25))
        .user_agent("socai-github-preview/0.1")
        .redirect(reqwest::redirect::Policy::custom(|a| {
            if a.previous().len() < 5 && media_allowed(a.url()) {
                a.follow()
            } else {
                a.error("Image redirect outside GitHub media hosts")
            }
        }))
        .build()?;
    // Public media only: never forward the API token to image hosts.
    let mut response = client.get(url).send().await?.error_for_status()?;
    let svg = response
        .headers()
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|s| s.starts_with("image/svg+xml"));
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 20 * 1024 * 1024,
            "Image exceeds 20 MiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    let ext = if svg {
        ensure!(std::str::from_utf8(&bytes)?.contains("<svg"), "Invalid SVG");
        "svg"
    } else {
        match image::guess_format(&bytes)? {
            image::ImageFormat::Png => "png",
            image::ImageFormat::Jpeg => "jpg",
            image::ImageFormat::Gif => "gif",
            image::ImageFormat::WebP => "webp",
            _ => "img",
        }
    };
    let path = directory.join(format!("{stem}.{ext}"));
    let temp = directory.join(format!("{}.part", uuid::Uuid::new_v4()));
    tokio::fs::write(&temp, bytes).await?;
    tokio::fs::rename(&temp, &path).await?;
    Ok(path.to_string_lossy().into_owned())
}

/// Capture lightweight repo records already returned to the model, without
/// fetching every README in a search. Desktop citations hydrate the full cache.
pub fn records(result: &Value) -> Vec<(String, Value)> {
    fn visit(value: &Value, out: &mut Vec<(String, Value)>, seen: &mut HashSet<String>) {
        if let Some(map) = value.as_object() {
            let name = map
                .get("full_name")
                .or_else(|| map.get("nameWithOwner"))
                .and_then(Value::as_str);
            if let Some(name) = name.filter(|n| n.split('/').count() == 2) {
                let url = format!("https://github.com/{name}");
                if target(&url).is_ok() {
                    let id = format!("github:{}", name.to_lowercase());
                    if seen.insert(id.clone()) {
                        out.push((id.clone(),json!({"note_id":id,"site":"github","native_id":name,"url":url,"title":name,
                        "excerpt":map.get("description"),"author":{"name":name.split('/').next()},"media":[],"archived":true,"level":"card",
                        "github":{"kind":"repository","metadata":{"stars":map.get("stargazers_count").or_else(||map.get("stargazerCount")),"language":map.get("language"),"created_at":map.get("created_at")}}})));
                    }
                }
            }
            for child in map.values() {
                visit(child, out, seen);
            }
        } else if let Some(items) = value.as_array() {
            for child in items {
                visit(child, out, seen);
            }
        }
    }
    let mut out = Vec::new();
    visit(&result["data"], &mut out, &mut HashSet::new());
    out
}
