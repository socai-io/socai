use std::{path::Path, time::Duration};

use anyhow::{ensure, Context, Result};
use base64::Engine;
use reqwest::{header::HeaderMap, Client, Url};
use serde_json::{json, Map, Value};

const MAX_BYTES: usize = 4 * 1024 * 1024;

fn validate_url(url: &Url) -> Result<()> {
    ensure!(
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port_or_known_default() == Some(443)
            && matches!(
                url.host_str(),
                Some("api.github.com" | "github.com" | "raw.githubusercontent.com")
            ),
        "Only HTTPS api.github.com, github.com and raw.githubusercontent.com URLs are supported"
    );
    Ok(())
}

/// Conservative lexical gate: mutation/subscription names outside literals/comments
/// are never accepted. GitHub performs full parsing; malformed documents cannot run.
/// Restrict the first token too so executable documents always start as queries.
fn read_only_query(query: &str) -> Result<()> {
    let b = query.as_bytes();
    let mut i = 0;
    let mut first = true;
    while i < b.len() {
        match b[i] {
            b'#' => {
                while i < b.len() && b[i] != b'\n' && b[i] != b'\r' {
                    i += 1;
                }
            }
            b'"' => {
                ensure!(!first, "GraphQL document must start with query or {{");
                let block = b[i..].starts_with(b"\"\"\"");
                i += if block { 3 } else { 1 };
                let mut closed = false;
                while i < b.len() {
                    if b[i] == b'\\' {
                        i = (i + 2).min(b.len());
                        continue;
                    }
                    if block && b[i..].starts_with(b"\"\"\"") {
                        i += 3;
                        closed = true;
                        break;
                    }
                    if !block && b[i] == b'"' {
                        i += 1;
                        closed = true;
                        break;
                    }
                    i += 1;
                }
                ensure!(closed, "Unterminated GraphQL string");
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                let name = &query[start..i];
                ensure!(
                    name != "mutation" && name != "subscription",
                    "Only read-only GraphQL queries are supported"
                );
                ensure!(
                    !first || name == "query",
                    "GraphQL document must start with query or {{"
                );
                first = false;
            }
            c if c.is_ascii_whitespace() || c == b',' => {
                i += 1;
            }
            c => {
                ensure!(
                    !first || c == b'{',
                    "GraphQL document must start with query or {{"
                );
                first = false;
                i += 1;
            }
        }
    }
    ensure!(!first, "GraphQL query is empty");
    Ok(())
}

pub(super) async fn token() -> Option<String> {
    for name in ["SOCAI_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(value) = std::env::var(name) {
            if !value.trim().is_empty() {
                return Some(value.trim().to_owned());
            }
        }
    }
    // Optional local credential helper. No shell, no token in args/logs/artifacts.
    let mut command = tokio::process::Command::new("gh");
    command
        .args(["auth", "token", "--hostname", "github.com"])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(3), command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    (!value.trim().is_empty()).then(|| value.trim().to_owned())
}

fn integer(input: &Value, key: &str, default: usize, min: usize, max: usize) -> Result<usize> {
    let Some(v) = input.get(key) else {
        return Ok(default);
    };
    let value = v
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .with_context(|| format!("{key} must be a nonnegative integer"))?;
    ensure!(
        (min..=max).contains(&value),
        "{key} must be between {min} and {max}"
    );
    Ok(value)
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

fn links(headers: &HeaderMap) -> Value {
    let mut result = Map::new();
    for part in header(headers, "link").unwrap_or_default().split(',') {
        if let Some((url, rel)) = part.trim().split_once("; rel=\"") {
            result.insert(
                rel.trim_end_matches('"').into(),
                json!(url.trim_matches(['<', '>'])),
            );
        }
    }
    Value::Object(result)
}

fn connection_pages(value: &Value, path: &str, pages: &mut Map<String, Value>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let p = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
                if key == "pageInfo" {
                    pages.insert(p, child.clone());
                } else {
                    connection_pages(child, &p, pages);
                }
            }
        }
        Value::Array(items) => {
            for (i, child) in items.iter().enumerate() {
                connection_pages(child, &format!("{path}/{i}"), pages);
            }
        }
        _ => (),
    }
}

fn project_record(value: Value, fields: Option<&Vec<String>>, compact_repos: bool) -> Value {
    let default_fields = [
        "full_name",
        "html_url",
        "description",
        "topics",
        "language",
        "stargazers_count",
        "forks_count",
        "created_at",
        "pushed_at",
        "updated_at",
        "archived",
        "fork",
        "homepage",
        "default_branch",
        "owner.login",
    ];
    let defaults = default_fields
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    let fields =
        fields.or_else(|| (compact_repos && value.get("full_name").is_some()).then_some(&defaults));
    let Some(fields) = fields else {
        return value;
    };
    if !value.is_object() {
        return value;
    }
    let mut result = json!({});
    for field in fields {
        let pointer = format!(
            "/{}",
            field
                .split('.')
                .map(|s| s.replace('~', "~0").replace('/', "~1"))
                .collect::<Vec<_>>()
                .join("/")
        );
        if let Some(v) = value.pointer(&pointer) {
            let mut target = &mut result;
            let parts: Vec<_> = field.split('.').collect();
            for part in &parts[..parts.len() - 1] {
                if !target[part].is_object() {
                    target[part] = json!({});
                }
                target = &mut target[part];
            }
            target[parts[parts.len() - 1]] = v.clone();
        }
    }
    result
}

fn project(value: Value, fields: Option<&Vec<String>>, compact_repos: bool) -> Value {
    match value {
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|v| project_record(v, fields, compact_repos))
                .collect(),
        ),
        Value::Object(mut map) if map.get("items").is_some_and(Value::is_array) => {
            let items = map.remove("items").unwrap();
            map.insert("items".into(), project(items, fields, compact_repos));
            Value::Object(map)
        }
        v => project_record(v, fields, compact_repos),
    }
}

fn clip(
    value: &mut Value,
    path: &str,
    limit: usize,
    budget: &mut usize,
    clipped: &mut Vec<String>,
    index_offset: usize,
) {
    match value {
        Value::String(text) => {
            let n = text.chars().count();
            let keep = (*budget).min(3000);
            if n > keep {
                *text = text.chars().take(keep).collect();
                clipped.push(path.to_owned());
            }
            *budget = budget.saturating_sub(n.min(keep));
        }
        Value::Array(items) => {
            if items.len() > limit {
                items.truncate(limit);
                clipped.push(path.to_owned());
            }
            for (i, child) in items.iter_mut().enumerate() {
                clip(
                    child,
                    &format!("{path}/{}", i + index_offset),
                    limit,
                    budget,
                    clipped,
                    0,
                );
            }
        }
        Value::Object(map) => {
            for (key, child) in map {
                clip(
                    child,
                    &format!("{path}/{}", key.replace('~', "~0").replace('/', "~1")),
                    limit,
                    budget,
                    clipped,
                    if key == "items" { index_offset } else { 0 },
                );
            }
        }
        _ => (),
    }
}

pub async fn execute(input: Value, output_dir: &Path) -> Result<Value> {
    let limit = integer(&input, "limit", 20, 1, 100)?;
    let max_chars = integer(&input, "max_chars", 18000, 100, 40000)?;
    let offset = integer(&input, "offset", 0, 0, usize::MAX)?;
    let query = input.get("query").and_then(Value::as_str);
    if let Some(q) = query {
        read_only_query(q)?;
    }
    let target = input
        .get("url")
        .and_then(Value::as_str)
        .or_else(|| query.map(|_| "/graphql"))
        .context("Supply a REST path, GitHub URL or GraphQL query")?;
    let mut url = if target.starts_with('/') {
        Url::parse(&format!("https://api.github.com{target}"))?
    } else {
        Url::parse(target)?
    };
    validate_url(&url)?;
    if let Some(params) = input.get("params") {
        let params = params
            .as_object()
            .context("params must be an object of string values")?;
        for (key, value) in params {
            url.query_pairs_mut().append_pair(
                key,
                value.as_str().context("params values must be strings")?,
            );
        }
    }
    let api = url.host_str() == Some("api.github.com");
    let graphql = query.is_some();
    ensure!(
        !graphql || (api && url.path() == "/graphql" && url.query().is_none()),
        "GraphQL queries must target https://api.github.com/graphql without URL parameters"
    );
    ensure!(
        graphql || url.path() != "/graphql",
        "Supply query for GraphQL"
    );
    let variables = match input.get("variables") {
        Some(Value::String(v)) => {
            serde_json::from_str::<Value>(v).context("variables must be a JSON object")?
        }
        Some(v) => v.clone(),
        None => json!({}),
    };
    ensure!(variables.is_object(), "variables must be a JSON object");
    let fields = input
        .get("fields")
        .map(|v| -> Result<Vec<String>> {
            v.as_array()
                .context("fields must be an array")?
                .iter()
                .map(|f| {
                    f.as_str()
                        .map(str::to_owned)
                        .context("fields must contain strings")
                })
                .collect()
        })
        .transpose()?;
    let credential = if api { token().await } else { None };
    if graphql && credential.is_none() {
        return Ok(
            json!({"ok":false,"reason":"authentication_required","message":"GraphQL needs host GitHub authentication (SOCAI_GITHUB_TOKEN, GH_TOKEN, GITHUB_TOKEN, or gh auth login). Public REST/HTML remain available."}),
        );
    }
    let client = Client::builder()
        .user_agent("socai-github/0.1")
        .timeout(Duration::from_secs(35))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let requested_url = url.to_string();
    let mut redirects = 0;
    let mut retries = 0;
    let response = loop {
        validate_url(&url)?;
        let is_api = url.host_str() == Some("api.github.com");
        let mut request = if graphql {
            client
                .post(url.clone())
                .json(&json!({"query":query,"variables":variables}))
        } else {
            client.get(url.clone())
        };
        if is_api {
            request = request.header("Accept", "application/vnd.github+json");
            if let Some(token) = &credential {
                request = request.bearer_auth(token);
            }
        }
        let response = request.send().await.context("GitHub HTTP request failed")?;
        if response.status().is_redirection() {
            ensure!(
                !graphql,
                "GraphQL redirect refused; use the canonical API endpoint"
            );
            ensure!(redirects < 3, "Too many GitHub redirects");
            let location =
                header(response.headers(), "location").context("Redirect has no Location")?;
            url = url.join(&location)?;
            redirects += 1;
            continue;
        }
        if response.status().is_server_error() && retries == 0 {
            retries += 1;
            tokio::time::sleep(Duration::from_millis(500)).await;
            continue;
        }
        break response;
    };
    let status = response.status();
    let headers = response.headers().clone();
    ensure!(
        response.content_length().unwrap_or(0) <= MAX_BYTES as u64,
        "GitHub response exceeds 4 MiB; request a smaller page or file"
    );
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= MAX_BYTES,
            "GitHub response exceeds 4 MiB; request a smaller page or file"
        );
        bytes.extend_from_slice(&chunk);
    }
    let body = String::from_utf8(bytes).context("Only UTF-8 text/JSON responses are supported")?;
    let content_type = header(&headers, "content-type").unwrap_or_default();
    let api = url.host_str() == Some("api.github.com");
    let is_html = !api && content_type.contains("text/html");
    let mut invalid_json = false;
    let mut full = if is_html {
        super::html::extract(&url, &body)
    } else if content_type.contains("json") || api {
        match serde_json::from_str(&body) {
            Ok(value) => value,
            Err(_) => {
                invalid_json = true;
                json!({"message":"Expected JSON but GitHub returned another response format", "text":body})
            }
        }
    } else {
        Value::String(body.clone())
    };
    if full.get("encoding").and_then(Value::as_str) == Some("base64") {
        if let Some(content) = full["content"].as_str() {
            let data = base64::engine::general_purpose::STANDARD.decode(
                content
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect::<String>(),
            )?;
            if let Ok(text) = String::from_utf8(data) {
                full["content"] = json!(text);
                full["encoding"] = json!("utf-8");
            }
        }
    }
    tokio::fs::create_dir_all(output_dir).await?;
    let stem = format!("github-{}", uuid::Uuid::new_v4());
    let raw_file = output_dir.join(format!("{stem}.{}", if is_html { "html" } else { "txt" }));
    let data_file = output_dir.join(format!("{stem}.json"));
    tokio::fs::write(&raw_file, &body).await?;
    tokio::fs::write(&data_file, serde_json::to_vec_pretty(&full)?).await?;
    let mut pages = Map::new();
    if graphql {
        connection_pages(&full, "", &mut pages);
    }
    let errors = full
        .get("errors")
        .filter(|e| e.as_array().is_some_and(|a| !a.is_empty()))
        .cloned();
    let ok = status.is_success() && errors.is_none() && !invalid_json;
    let mut result = json!({"ok":ok,"url":url.as_str(),"requested_url":requested_url,
        "fetched_at":chrono::Utc::now().to_rfc3339(),"status":status.as_u16(),
        "transport":if graphql {"graphql"} else if api {"rest"} else {"http"},
        "credential_used":credential.is_some() && api,
        "partial":!ok || (is_html && full.get("kind").and_then(Value::as_str) == Some("html_text")),
        "pagination":{"links":links(&headers),"connections":pages},
        "rate_limit":{"remaining":header(&headers,"x-ratelimit-remaining"),"reset":header(&headers,"x-ratelimit-reset"),"resource":header(&headers,"x-ratelimit-resource"),"retry_after":header(&headers,"retry-after")},
        "full_response_path":data_file,"raw_response_path":raw_file});
    if !ok {
        result["reason"] = json!(if status.as_u16() == 429
            || header(&headers, "x-ratelimit-remaining").as_deref() == Some("0")
        {
            "rate_limited"
        } else if status.as_u16() == 401 {
            "authentication_failed"
        } else if errors.is_some() {
            "graphql_errors"
        } else if invalid_json && status.is_success() {
            "unexpected_response"
        } else {
            "http_error"
        });
        result["message"] = json!("Request failed or returned partial GraphQL data; inspect data/errors. A permission failure or 404 does not establish that the resource does not exist.");
        if let Some(errors) = errors {
            result["errors"] = errors;
        }
    }
    let selection = if ok {
        input.get("select").and_then(Value::as_str).unwrap_or("")
    } else {
        ""
    };
    let selected = if ok {
        if let Some(pointer) = input.get("select").and_then(Value::as_str) {
            full.pointer(pointer)
                .with_context(|| {
                    format!(
                        "JSON Pointer {pointer:?} not found; full response: {}",
                        data_file.display()
                    )
                })?
                .clone()
        } else {
            full
        }
    } else {
        full
    };
    let mut data = project(
        selected,
        if ok { fields.as_ref() } else { None },
        api && !graphql && ok,
    );
    let offset = if ok { offset } else { 0 };
    result["selection"] = json!(selection);
    result["offset"] = json!(offset);
    let mut clipped = Vec::new();
    match &mut data {
        Value::String(text) => {
            let total = text.chars().count();
            ensure!(offset <= total, "offset exceeds text length ({total})");
            *text = text.chars().skip(offset).take(max_chars).collect();
            let end = offset.saturating_add(max_chars).min(total);
            result["next_offset"] = if end < total { json!(end) } else { Value::Null };
            result["total_chars"] = json!(total);
            if end < total || offset > 0 {
                clipped.push(selection.to_owned());
            }
        }
        _ => {
            let list = if data.is_array() {
                data.as_array_mut()
            } else {
                data.get_mut("items").and_then(Value::as_array_mut)
            };
            if let Some(list) = list {
                let total = list.len();
                ensure!(offset <= total, "offset exceeds list length ({total})");
                let end = offset.saturating_add(limit).min(total);
                *list = list[offset..end].to_vec();
                result["next_offset"] = if end < total { json!(end) } else { Value::Null };
                result["page_items"] = json!(total);
                if end < total || offset > 0 {
                    clipped.push(selection.to_owned());
                }
            } else {
                ensure!(offset == 0, "offset requires a selected text or list");
            }
            let mut budget = max_chars;
            clip(
                &mut data,
                selection,
                limit,
                &mut budget,
                &mut clipped,
                offset,
            );
        }
    }
    result["truncated"] = json!(!clipped.is_empty());
    result["truncated_paths"] = json!(clipped);
    result["data"] = data;
    Ok(result)
}
