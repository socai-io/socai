pub(crate) mod media;
mod retry;
mod search;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::{Backend, Tool, ToolContext, ToolResult};
use crate::cdp::PageSession;
use crate::runtime::SocaiRuntime;
use crate::sites::learning::run_site_browser_tool;
use crate::sites::post_archive::persist_site_tool_result;
use crate::sites::registry::{
    required_string, ArgKind, CommandArg, NativeSiteAdapter, SiteCommand, SlowWhen,
};
use crate::sites::runner::{json_result, ToolCommand};
use crate::sites::skill_cli::run_skill_command;

const SITE: &str = "weixin";

pub fn weixin_tools(page: Arc<PageSession>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(WeixinTool {
            page: Some(page.clone()),
            runtime: None,
            debug_snapshot: false,
            read: false,
        }),
        Arc::new(WeixinTool {
            page: Some(page),
            runtime: None,
            debug_snapshot: false,
            read: true,
        }),
    ]
}

async fn agent_tools(
    page: Arc<PageSession>,
    _provider: Arc<dyn Backend>,
) -> anyhow::Result<Vec<Arc<dyn Tool>>> {
    Ok(weixin_tools(page))
}

pub static WEIXIN_NATIVE_ADAPTER: NativeSiteAdapter = NativeSiteAdapter {
    runtime_command: Some(run_runtime_command),
    id: SITE,
    about: "WeChat Official Account articles via Sogou (微信公众号)",
    home_url: "",
    agent_tools: |page, provider| Box::pin(agent_tools(page, provider)),
    default_agent_tools: None,
    agent_instructions: |extra| crate::sites::learning::site_agent_instructions(SITE, extra),
    default_agent_instructions: None,
    commands: &[
        SiteCommand {
            name: "search",
            tool_name: "search_weixin_articles",
            about: "Search WeChat articles over HTTP; optionally read each article body.",
            args: &[
                CommandArg {
                    key: "query",
                    long: None,
                    value_name: "QUERY",
                    help: "Search keywords",
                    required: true,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "num",
                    long: Some("num"),
                    value_name: "N",
                    help: "Maximum articles (1–30, default 10)",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "page",
                    long: Some("page"),
                    value_name: "PAGE",
                    help: "Starting search page (1–100, default 1)",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "read",
                    long: Some("read"),
                    value_name: "READ",
                    help: "Open each result in Chrome and include article text",
                    required: false,
                    kind: ArgKind::Flag,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Content wait per page (1–120, default 30)",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: |page, args, debug, progress| {
                run_skill_command(
                    page.clone(),
                    args,
                    debug,
                    progress,
                    ToolCommand {
                        site_id: SITE,
                        command_name: "search",
                        tool_name: "search_weixin_articles",
                        before: None,
                        after: None,
                        include_run_metadata: false,
                    },
                    weixin_tools(page),
                )
            },
        },
        SiteCommand {
            name: "read",
            tool_name: "read_weixin_article",
            about: "Read a WeChat article through Chrome, following a Sogou result link if needed.",
            args: &[
                CommandArg {
                    key: "url",
                    long: None,
                    value_name: "URL",
                    help: "HTTPS Sogou result link or mp.weixin.qq.com/s article URL",
                    required: true,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Content wait (1–120, default 30)",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: |page, args, debug, progress| {
                run_skill_command(
                    page.clone(),
                    args,
                    debug,
                    progress,
                    ToolCommand {
                        site_id: SITE,
                        command_name: "read",
                        tool_name: "read_weixin_article",
                        before: None,
                        after: None,
                        include_run_metadata: false,
                    },
                    weixin_tools(page),
                )
            },
        },
    ],
};

struct WeixinTool {
    page: Option<Arc<PageSession>>,
    runtime: Option<SocaiRuntime>,
    debug_snapshot: bool,
    read: bool,
}

#[async_trait]
impl Tool for WeixinTool {
    fn name(&self) -> &str {
        if self.read {
            "read_weixin_article"
        } else {
            "search_weixin_articles"
        }
    }
    fn description(&self) -> &str {
        if self.read {
            "Read a WeChat Official Account article in Chrome from an HTTPS Sogou search-result link or mp.weixin.qq.com/s URL. Returns full rendered article text, publisher and source URL; reports access failures. Never fetches article HTML over HTTP."
        } else {
            "Search WeChat Official Account articles through Sogou over HTTP without opening a search tab. Returns titles, publishers, snippets and links. Set read=true to also read full article text in fresh Chrome tabs; otherwise use read_weixin_article on selected links. Result links can expire; read them promptly."
        }
    }
    fn input_schema(&self) -> Value {
        let mut properties = json!({ "wait_seconds": { "type": "integer", "minimum": 1, "maximum": 120, "default": 30 } });
        if self.read {
            properties["url"] = json!({ "type": "string", "description": "Sogou /link result URL or WeChat /s article URL." });
        } else {
            properties["query"] = json!({ "type": "string", "minLength": 1, "maxLength": 512 });
            properties["num"] =
                json!({ "type": "integer", "minimum": 1, "maximum": 30, "default": 10 });
            properties["page"] =
                json!({ "type": "integer", "minimum": 1, "maximum": 100, "default": 1 });
            properties["read"] = json!({ "type": "boolean", "default": false });
        }
        json!({ "type": "object", "properties": properties, "required": [if self.read { "url" } else { "query" }] })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let wait = integer(&input, "wait_seconds", 30, 120)? as u64;
        let result = if self.read {
            let url = required_string(&input, "url")?;
            validate_article_url(&url)?;
            self.read_article(&url, wait, ctx).await?
        } else {
            let query = required_string(&input, "query")?;
            anyhow::ensure!(
                query.chars().count() <= 512,
                "query must contain at most 512 characters"
            );
            let num = integer(&input, "num", 10, 30)? as usize;
            let page = integer(&input, "page", 1, 100)?;
            let read = match input.get("read") {
                None => false,
                Some(v) => v
                    .as_bool()
                    .ok_or_else(|| anyhow::anyhow!("read must be boolean"))?,
            };
            search(self, &query, num, page, read, wait, ctx).await?
        };
        Ok(json_result(&result))
    }
}

fn integer(input: &Value, key: &str, default: i64, max: i64) -> anyhow::Result<i64> {
    let value = match input.get(key) {
        None => default,
        Some(v) => v
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("{key} must be an integer"))?,
    };
    anyhow::ensure!(
        (1..=max).contains(&value),
        "{key} must be between 1 and {max}"
    );
    Ok(value)
}

fn validate_article_url(value: &str) -> anyhow::Result<()> {
    let url = reqwest::Url::parse(value)?;
    anyhow::ensure!(
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port_or_known_default() == Some(443)
            && match url.host_str() {
                Some("weixin.sogou.com") =>
                    url.path() == "/link"
                        && url.query_pairs().any(|(k, v)| k == "url" && !v.is_empty()),
                Some("mp.weixin.qq.com") => url.path() == "/s" || url.path().starts_with("/s/"),
                _ => false,
            },
        "expected an HTTPS Sogou /link or WeChat /s article URL"
    );
    Ok(())
}

async fn wait_state(
    page: &PageSession,
    tool: &str,
    args: &Value,
    wait: u64,
) -> anyhow::Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(wait);
    loop {
        let state = run_site_browser_tool(page, SITE, tool, Some(args)).await?;
        let pending = matches!(
            state["reason"].as_str(),
            Some("search_not_ready" | "article_not_ready" | "redirect_pending")
        );
        if state["ok"] == true || !pending {
            return Ok(state);
        }
        if Instant::now() >= deadline {
            return Ok(json!({ "ok": false, "reason": "timeout", "state": state }));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn search(
    tool: &WeixinTool,
    query: &str,
    num: usize,
    start: i64,
    read: bool,
    wait: u64,
    ctx: &ToolContext,
) -> anyhow::Result<Value> {
    let client = search::client(wait)?;
    let mut results = Vec::new();
    let mut failure = None;
    let mut has_more = false;
    let mut last_page = start;
    for number in start..=(start + 2).min(100) {
        last_page = number;
        let cards = match retry::run(|| {
            search::search_page(&client, query, number, (num - results.len()).min(10))
        })
        .await
        {
            Ok(cards) => cards,
            Err(error) => {
                failure =
                    Some(json!({"reason":"search_request_failed","message":error.to_string()}));
                break;
            }
        };
        if cards["ok"] == false {
            failure = Some(cards);
            break;
        }
        has_more = cards["has_more"].as_bool().unwrap_or(false);
        for card in cards["results"].as_array().into_iter().flatten() {
            if !results
                .iter()
                .any(|existing: &Value| existing["url"] == card["url"])
            {
                results.push(card.clone());
            }
        }
        if results.len() >= num || !has_more {
            break;
        }
    }
    if read && failure.is_none() {
        let mut blocked = false;
        for card in &mut results {
            if blocked {
                card["article"] = json!({"ok":false,"reason":"not_attempted_after_access_block"});
                continue;
            }
            let url = card["url"].as_str().unwrap_or_default().to_string();
            let article = match tool.read_article(&url, wait, ctx).await {
                Ok(value) => value,
                Err(error) => {
                    json!({"ok":false,"reason":"article_read_failed","message":error.to_string(),"url":url})
                }
            };
            if article["ok"] == false && failure.is_none() {
                failure = Some(json!({"reason":"partial_article_failure"}));
            }
            blocked = retry::blocked(&article);
            if blocked {
                failure = Some(article.clone());
            }
            card["article"] = article;
        }
    }
    let mut result = json!({"query":query,"page":start,"last_page":last_page,"has_more":has_more,"results":results});
    if let Some(failure) = failure {
        result["ok"] = json!(false);
        result["reason"] = failure["reason"].clone();
        result["failure"] = failure;
    }
    persist_site_tool_result(ctx, SITE, "searchResults", &result, None);
    Ok(result)
}

impl WeixinTool {
    async fn read_article(&self, url: &str, wait: u64, ctx: &ToolContext) -> anyhow::Result<Value> {
        let page = if let Some(page) = &self.page {
            page.clone()
        } else {
            self.runtime
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("browser runtime unavailable"))?
                .ensure_site_page(SITE, "")
                .await?
        };
        let capture_dir = ctx
            .output_dir()
            .join(format!("weixin-article-{}", uuid::Uuid::new_v4()));
        crate::cdp::with_snapshot_recording(
            &page,
            &capture_dir,
            self.debug_snapshot,
            retry::run(|| read_article(&page, url, wait, ctx)),
        )
        .await
    }
}

fn run_runtime_command(
    runtime: SocaiRuntime,
    command: &'static str,
    args: Value,
    debug: bool,
    progress: Option<crate::agent::tool::ToolProgressSender>,
) -> crate::sites::registry::BoxFuture<Value> {
    Box::pin(async move {
        let read = command == "read";
        let tool: Arc<dyn Tool> = Arc::new(WeixinTool {
            page: None,
            runtime: Some(runtime),
            debug_snapshot: debug,
            read,
        });
        crate::sites::runner::run_tool_command_without_browser(
            ToolCommand {
                site_id: SITE,
                command_name: command,
                tool_name: if read {
                    "read_weixin_article"
                } else {
                    "search_weixin_articles"
                },
                before: None,
                after: None,
                include_run_metadata: false,
            },
            &[tool],
            args,
            progress,
        )
        .await
    })
}

async fn read_article(
    parent: &PageSession,
    url: &str,
    wait: u64,
    ctx: &ToolContext,
) -> anyhow::Result<Value> {
    validate_article_url(url)?;
    let page = parent.create_sibling("about:blank").await?;
    if let Some(recorder) = parent.recorder() {
        page.set_recorder(recorder);
    }
    let outcome = async {
        let (session_id, mut responses, frame_id) = page.document_responses().await?;
        let session = search::session_for(url);
        if let Some(session) = &session {
            page.set_cookies(&session.cookies).await?;
        }
        page.navigate_with_referrer(
            url,
            session.as_ref().map(|s| s.referrer.as_str()),
            wait as f64,
        )
        .await?;
        let state = wait_state(&page, "articleState", &json!({}), wait).await?;
        while let Ok((event_session, response)) = responses.try_recv() {
            if event_session == session_id && response["frame_id"] == frame_id {
                if let Some(gate) = retry::response_gate(
                    response["status"].as_u64().unwrap_or(0) as u16,
                    response["retry_after"].as_str(),
                ) {
                    return Ok(gate);
                }
            }
        }
        if state["ok"] != true {
            return Ok(state);
        }
        let deadline = Instant::now() + Duration::from_secs(wait);
        let mut previous = String::new();
        let mut stable = 0;
        loop {
            let article = run_site_browser_tool(&page, SITE, "articleDetail", None).await?;
            if article["ok"] == false {
                return Ok(article);
            }
            let text = article["text"].as_str().unwrap_or_default();
            stable = if !text.is_empty() && text == previous {
                stable + 1
            } else {
                0
            };
            if stable >= 2 {
                persist_site_tool_result(ctx, SITE, "articleDetail", &article, Some(url));
                return Ok(article);
            }
            if Instant::now() >= deadline {
                return Ok(json!({ "ok": false, "reason": "article_not_stable", "url": url }));
            }
            previous = text.to_string();
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    .await;
    let _ = page.close().await;
    outcome
}
