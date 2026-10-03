use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::tool::ToolProgressSender;
use crate::agent::{Backend as LlmProvider, Tool, ToolContext, ToolResult};
use crate::cdp::PageSession;
use crate::sites::actions::{
    ActionActor, ActionPreview, ActionStore, ActionTarget, SocialActionKind, SocialActionStatus,
};
use crate::sites::registry::{
    required_string, ArgKind, BoxFuture, CommandArg, NativeSiteAdapter, SiteCommand, SlowWhen,
};
use crate::sites::runner::{get_f64, get_i64, json_result, ToolCommand};
use crate::sites::skill_cli::{
    current_url, ensure_site_page, failure_payload, gate_reason, invoke_browser_tool,
    navigate_https, percent_encode_query, run_skill_command, wait_for_browser_tool,
    DEFAULT_COMMENT_COUNT, DEFAULT_RESULT_COUNT, DEFAULT_WAIT_SECONDS, MAX_TOOL_ITEMS,
    MAX_TOOL_WAIT_SECONDS,
};

const SITE_ID: &str = "x";
const HOME_URL: &str = "https://x.com/home";
const HOST_ROOT: &str = "x.com";
const RESERVED_PROFILE_NAMES: &[&str] = &[
    "compose",
    "explore",
    "home",
    "i",
    "intent",
    "login",
    "messages",
    "notifications",
    "search",
    "settings",
    "share",
    "signup",
];

pub async fn x_agent_tools(
    page: Arc<PageSession>,
    _llm_provider: Arc<dyn LlmProvider>,
) -> anyhow::Result<Vec<Arc<dyn Tool>>> {
    Ok(x_tools(page))
}

pub fn x_agent_instructions(extra: &str) -> String {
    crate::sites::learning::site_agent_instructions(SITE_ID, extra)
}

fn x_tools(page: Arc<PageSession>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(SearchTool { page: page.clone() }),
        Arc::new(ProfileTool { page: page.clone() }),
        Arc::new(GetPostsTool { page: page.clone() }),
        Arc::new(ReplyTool { page: page.clone() }),
        Arc::new(PageStateTool { page }),
    ]
}

pub static X_NATIVE_ADAPTER: NativeSiteAdapter = NativeSiteAdapter {
    id: SITE_ID,
    about: "X (x.com)",
    home_url: "",
    agent_tools: |page, llm| Box::pin(x_agent_tools(page, llm)),
    default_agent_tools: None,
    agent_instructions: x_agent_instructions,
    default_agent_instructions: None,
    commands: &[
        SiteCommand {
            name: "search",
            tool_name: "search",
            about: "Search X and optionally click visible posts for details and replies.",
            args: &[
                CommandArg {
                    key: "query",
                    long: None,
                    value_name: "QUERY",
                    help: "Search query",
                    required: true,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "num",
                    long: Some("num"),
                    value_name: "N",
                    help: "Number of posts to collect by scrolling. Defaults to 10.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "deep",
                    long: Some("deep"),
                    value_name: "N",
                    help: "Open up to N collected posts by clicking the current search timeline. Defaults to 0.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "num_comments",
                    long: Some("num-comments"),
                    value_name: "N",
                    help: "Visible replies to collect per deeply read post. Defaults to 8.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for the search page to hydrate. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_search,
        },
        SiteCommand {
            name: "profile",
            tool_name: "profile",
            about: "Read an X profile and optionally click visible timeline posts for details and replies.",
            args: &[
                CommandArg {
                    key: "profile",
                    long: None,
                    value_name: "HANDLE_OR_URL",
                    help: "X username or profile URL.",
                    required: true,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "num",
                    long: Some("num"),
                    value_name: "N",
                    help: "Number of visible posts to collect by scrolling. Defaults to 10.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "deep",
                    long: Some("deep"),
                    value_name: "N",
                    help: "Open up to N collected posts by clicking the current profile timeline. Defaults to 0.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "num_comments",
                    long: Some("num-comments"),
                    value_name: "N",
                    help: "Visible replies to collect per deeply read post. Defaults to 8.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for the profile to hydrate. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_profile,
        },
        SiteCommand {
            name: "get-posts",
            tool_name: "get_posts",
            about: "Read X posts by URL or numeric id, including visible replies and media.",
            args: &[
                CommandArg {
                    key: "posts",
                    long: Some("post"),
                    value_name: "URL_OR_ID",
                    help: "X post URL or numeric id. Repeat to read multiple posts.",
                    required: true,
                    kind: ArgKind::StrList,
                },
                CommandArg {
                    key: "num_comments",
                    long: Some("num-comments"),
                    value_name: "N",
                    help: "Visible replies to collect per post. Defaults to 8; 0 skips replies.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for each post to hydrate. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_get_posts,
        },
        SiteCommand {
            name: "reply",
            tool_name: "reply",
            about: "Reply to one X post using verified pointer and keyboard events.",
            args: &[
                CommandArg {
                    key: "post",
                    long: None,
                    value_name: "URL_OR_ID",
                    help: "Target X post URL or numeric id.",
                    required: true,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "text",
                    long: Some("text"),
                    value_name: "TEXT",
                    help: "Exact reply text. The command refuses to replace an existing draft.",
                    required: true,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help:
                        "Maximum wait for hydration and post-submit reconciliation. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_reply,
        },
        SiteCommand {
            name: "page_state",
            tool_name: "page_state",
            about: "Open or reuse X and print page, login, challenge, and rate-limit state.",
            args: &[CommandArg {
                key: "wait_seconds",
                long: Some("wait-seconds"),
                value_name: "SECONDS",
                help: "Maximum wait for X. Defaults to 30.",
                required: false,
                kind: ArgKind::Int,
            }],
            slow: SlowWhen::Always,
            run: run_page_state,
        },
    ],
};

fn run_search(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    run_named(page, args, debug_snapshot, progress, "search", "search")
}

fn run_profile(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    run_named(page, args, debug_snapshot, progress, "profile", "profile")
}

fn run_get_posts(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    run_named(
        page,
        args,
        debug_snapshot,
        progress,
        "get-posts",
        "get_posts",
    )
}

fn run_page_state(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    run_named(
        page,
        args,
        debug_snapshot,
        progress,
        "page_state",
        "page_state",
    )
}

fn run_reply(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    run_named(page, args, debug_snapshot, progress, "reply", "reply")
}

fn run_named(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
    command_name: &'static str,
    tool_name: &'static str,
) -> BoxFuture<Value> {
    run_skill_command(
        page.clone(),
        args,
        debug_snapshot,
        progress,
        ToolCommand {
            site_id: SITE_ID,
            command_name,
            tool_name,
            before: None,
            after: None,
            include_run_metadata: command_name == "get-posts",
        },
        x_tools(page),
    )
}

struct SearchTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }

    fn description(&self) -> &str {
        "Search X for posts matching `query`. Set `deep` to open that many posts with trusted clicks in the current search timeline, read details/replies, and restore the search URL and scroll position. Deep-read click or restoration failures fail closed and never fall back to direct post navigation."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "maxLength": 512 },
                "num": { "type": "integer", "default": 10, "minimum": 1, "maximum": 100 },
                "deep": { "type": "integer", "default": 0, "minimum": 0, "maximum": 100 },
                "num_comments": { "type": "integer", "default": 8, "minimum": 0, "maximum": 100 },
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1, "maximum": 330 }
            },
            "required": ["query"]
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let query = required_string(&input, "query")?;
        if query.chars().count() > 512 {
            anyhow::bail!("query must contain at most 512 characters");
        }
        let num = get_i64(&input, "num", DEFAULT_RESULT_COUNT).clamp(1, MAX_TOOL_ITEMS);
        let deep = get_i64(&input, "deep", 0).clamp(0, num);
        let num_comments =
            get_i64(&input, "num_comments", DEFAULT_COMMENT_COUNT).clamp(0, MAX_TOOL_ITEMS);
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        let target = format!(
            "https://x.com/search?q={}&src=typed_query&f=live",
            percent_encode_query(&query)
        );
        navigate_https(&self.page, &target).await?;
        let search_args = json!({ "query": query });
        let state = wait_for_browser_tool(
            &self.page,
            SITE_ID,
            "searchState",
            Some(&search_args),
            wait_seconds,
        )
        .await?;
        if let Some(reason) = gate_reason(&state) {
            return Ok(json_result(&failure_payload(
                reason,
                json!({ "query": query, "state": state, "count": 0, "results": [] }),
            )));
        }
        if state.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(json_result(&failure_payload(
                state
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("search_results_unavailable"),
                json!({ "query": query, "state": state, "count": 0, "results": [] }),
            )));
        }
        let results = invoke_browser_tool(
            &self.page,
            ctx,
            SITE_ID,
            "searchResults",
            Some(&json!({ "limit": num })),
            true,
        )
        .await?;
        let final_state = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "searchState",
            Some(&search_args),
        )
        .await?;
        if final_state.get("ok").and_then(Value::as_bool) != Some(true) {
            let reason = gate_reason(&final_state).unwrap_or_else(|| {
                final_state
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("page_unavailable_after_scroll")
            });
            return Ok(json_result(&failure_payload(
                reason,
                json!({
                    "query": query,
                    "state": final_state,
                    "count": results.as_array().map(Vec::len).unwrap_or(0),
                    "results": results,
                    "partial": true,
                }),
            )));
        }
        let mut payload = json!({
            "ok": true,
            "query": query,
            "url": current_url(&self.page).await.unwrap_or_default(),
            "count": results.as_array().map(Vec::len).unwrap_or(0),
            "results": results,
            "state": final_state,
        });
        if deep > 0 {
            let deep_posts = read_clicked_x_posts(
                &self.page,
                ctx,
                &payload["results"],
                deep,
                num_comments,
                wait_seconds,
            )
            .await?;
            let deep_status = x_deep_read_status(&payload["results"], &deep_posts, deep);
            payload["ok"] = json!(deep_status.get("ok").and_then(Value::as_bool) == Some(true));
            payload["deep_posts"] = deep_posts;
            payload["deep_status"] = deep_status;
            payload["navigation_policy"] = json!("card_click_only");
        }
        Ok(json_result(&payload))
    }
}

struct ProfileTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for ProfileTool {
    fn name(&self) -> &str {
        "profile"
    }

    fn description(&self) -> &str {
        "Read an X profile and its visible posts by @handle or URL. Set `deep` to open that many posts with trusted clicks in the current profile timeline, read details/replies, and restore the profile URL and scroll position. Deep-read click or restoration failures fail closed and never fall back to direct post navigation."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "profile": { "type": "string" },
                "num": { "type": "integer", "default": 10, "minimum": 1, "maximum": 100 },
                "deep": { "type": "integer", "default": 0, "minimum": 0, "maximum": 100 },
                "num_comments": { "type": "integer", "default": 8, "minimum": 0, "maximum": 100 },
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1, "maximum": 330 }
            },
            "required": ["profile"]
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let locator = required_string(&input, "profile")?;
        let url = x_profile_url(&locator)?;
        let num = get_i64(&input, "num", DEFAULT_RESULT_COUNT).clamp(1, MAX_TOOL_ITEMS);
        let deep = get_i64(&input, "deep", 0).clamp(0, num);
        let num_comments =
            get_i64(&input, "num_comments", DEFAULT_COMMENT_COUNT).clamp(0, MAX_TOOL_ITEMS);
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        navigate_https(&self.page, &url).await?;
        let state =
            wait_for_browser_tool(&self.page, SITE_ID, "profileDetail", None, wait_seconds).await?;
        if let Some(reason) = gate_reason(&state) {
            return Ok(json_result(&failure_payload(
                reason,
                json!({ "profile": locator, "url": url, "state": state }),
            )));
        }
        if state.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(json_result(&failure_payload(
                state
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("profile_unavailable"),
                json!({ "profile": locator, "url": url, "state": state }),
            )));
        }
        let posts = invoke_browser_tool(
            &self.page,
            ctx,
            SITE_ID,
            "profilePosts",
            Some(&json!({ "limit": num })),
            true,
        )
        .await?;
        let final_state = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "profileDetail",
            None,
        )
        .await?;
        let expected_username = state
            .get("username")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let final_username = final_state
            .get("username")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if final_state.get("ok").and_then(Value::as_bool) != Some(true)
            || expected_username.is_empty()
            || final_username != expected_username
        {
            let reason = if final_state.get("ok").and_then(Value::as_bool) == Some(true) {
                "profile_changed_during_read"
            } else {
                gate_reason(&final_state).unwrap_or_else(|| {
                    final_state
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("page_unavailable_after_scroll")
                })
            };
            return Ok(json_result(&failure_payload(
                reason,
                json!({
                    "profile": state,
                    "state": final_state,
                    "posts": posts,
                    "partial": true,
                }),
            )));
        }
        let mut payload = json!({
            "ok": true,
            "profile": state,
            "posts": posts,
            "count": posts.as_array().map(Vec::len).unwrap_or(0),
        });
        if deep > 0 {
            let deep_posts = read_clicked_x_posts(
                &self.page,
                ctx,
                &payload["posts"],
                deep,
                num_comments,
                wait_seconds,
            )
            .await?;
            let deep_status = x_deep_read_status(&payload["posts"], &deep_posts, deep);
            payload["ok"] = json!(deep_status.get("ok").and_then(Value::as_bool) == Some(true));
            payload["deep_posts"] = deep_posts;
            payload["deep_status"] = deep_status;
            payload["navigation_policy"] = json!("card_click_only");
        }
        Ok(json_result(&payload))
    }
}

struct GetPostsTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for GetPostsTool {
    fn name(&self) -> &str {
        "get_posts"
    }

    fn description(&self) -> &str {
        "Read one or more X posts by URL or numeric id, including visible replies and media."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "posts": { "type": "array", "items": { "type": "string" } },
                "num_comments": { "type": "integer", "default": 8, "minimum": 0, "maximum": 100 },
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1, "maximum": 330 }
            },
            "required": ["posts"]
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let posts = input
            .get("posts")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("missing required argument: posts"))?;
        if posts.is_empty() {
            anyhow::bail!("at least one --post is required");
        }
        let num_comments =
            get_i64(&input, "num_comments", DEFAULT_COMMENT_COUNT).clamp(0, MAX_TOOL_ITEMS);
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        let mut items = Vec::new();
        let mut stopped_on_gate = None;
        for (index, post) in posts.iter().enumerate() {
            let locator = post
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow::anyhow!("each --post must be a non-empty URL or id"))?;
            let item = read_x_post(&self.page, ctx, locator, num_comments, wait_seconds).await?;
            let gate = gate_reason(&item);
            items.push(item);
            if let Some(reason) = gate {
                stopped_on_gate = Some(reason);
                for remaining in &posts[index + 1..] {
                    items.push(json!({
                        "ok": false,
                        "status": "unprocessed_after_gate",
                        "reason": reason,
                        "input": remaining.as_str().unwrap_or_default(),
                    }));
                }
                break;
            }
        }
        Ok(json_result(&json!({
            "ok": items.iter().all(|item| item.get("ok").and_then(Value::as_bool) == Some(true)),
            "count": items.len(),
            "posts": items,
            "stopped_on_gate": stopped_on_gate,
        })))
    }
}

struct PageStateTool {
    page: Arc<PageSession>,
}

struct ReplyTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for ReplyTool {
    fn name(&self) -> &str {
        "reply"
    }

    fn description(&self) -> &str {
        "Reply only when the user explicitly requests the exact X post and exact text. Preserve both without inventing additional writes. Uses real CDP pointer and keyboard events and refuses ambiguous composers, existing drafts, route changes, and submit retries. Treat commit_unknown as unknown and never retry it automatically."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "post": { "type": "string" },
                "text": { "type": "string", "minLength": 1, "maxLength": 10000 },
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1, "maximum": 330 }
            },
            "required": ["post", "text"]
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let locator = required_string(&input, "post")?;
        let raw_text = input
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing required argument: text"))?;
        if raw_text.trim().is_empty() || raw_text.trim() != raw_text {
            anyhow::bail!(
                "reply text must be non-empty and have no leading or trailing whitespace"
            );
        }
        let text = raw_text.to_string();
        if text.chars().count() > 10_000 {
            anyhow::bail!("reply text must contain at most 10000 characters");
        }
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        let url = x_post_url(&locator)?;
        let expected_post_id = x_post_id(&url)
            .ok_or_else(|| anyhow::anyhow!("canonical X post URL is missing a status id"))?;
        navigate_https(&self.page, &url).await?;
        let detail =
            wait_for_browser_tool(&self.page, SITE_ID, "postDetail", None, wait_seconds).await?;
        if let Some(reason) = gate_reason(&detail) {
            return Ok(json_result(&failure_payload(
                reason,
                json!({ "post": locator, "url": url, "detail": detail, "submit_click_count": 0 }),
            )));
        }
        let post_id = detail.get("id").and_then(Value::as_str).unwrap_or_default();
        if detail.get("ok").and_then(Value::as_bool) != Some(true) || post_id != expected_post_id {
            return Ok(json_result(&failure_payload(
                if detail.get("ok").and_then(Value::as_bool) == Some(true) {
                    "wrong_post"
                } else {
                    detail
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("post_unavailable")
                },
                json!({ "post": locator, "expected_post_id": expected_post_id, "url": url, "detail": detail, "submit_click_count": 0 }),
            )));
        }
        let action_args = json!({ "post_id": post_id });
        let rendered_args = json!({ "post_id": post_id, "text": text });
        let before = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "renderedReplyState",
            Some(&rendered_args),
        )
        .await?;
        if before.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(json_result(&failure_payload(
                before
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("reply_preflight_failed"),
                json!({ "post_id": post_id, "url": url, "reconcile": before, "submit_click_count": 0 }),
            )));
        }
        let actor_id = before
            .get("author")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if actor_id.is_empty() {
            return Ok(json_result(&failure_payload(
                "current_user_unknown",
                json!({ "post_id": post_id, "url": url, "reconcile": before, "submit_click_count": 0 }),
            )));
        }
        let actor = ActionActor {
            id: actor_id.to_string(),
            display_name: format!("@{actor_id}"),
        };
        let target_url = format!("https://x.com/i/status/{post_id}");
        let idempotency_key = format!("x:reply:{actor_id}:{post_id}:{text}");
        let store = ActionStore::open_default();
        let mut receipt = store.create_draft(
            &idempotency_key,
            "x",
            SocialActionKind::Reply,
            ActionTarget {
                id: post_id.to_string(),
                url: target_url,
            },
            actor.clone(),
            ActionPreview {
                text: Some(text.clone()),
                evidence: Value::Null,
            },
        )?;
        match receipt.status() {
            SocialActionStatus::Committed | SocialActionStatus::Reconciled => {
                return Ok(json_result(&json!({
                    "ok": true, "status": receipt.status(), "action_id": receipt.action_id(),
                    "idempotent_replay": true, "post_id": post_id, "url": url,
                    "reply": text, "submit_click_count": 0, "receipt": receipt,
                })));
            }
            SocialActionStatus::Committing | SocialActionStatus::CommitUnknown => {
                let prior = receipt.precommit_target_ids().unwrap_or(&[]);
                let observed = value_string_array(&before, "ids");
                if let Some(new_id) = observed.iter().find(|id| !prior.contains(id)) {
                    receipt = store.reconcile_committed(receipt.action_id(), new_id)?;
                    return Ok(json_result(&json!({
                        "ok": true, "status": "reconciled", "action_id": receipt.action_id(),
                        "idempotent_replay": true, "post_id": post_id, "url": url,
                        "reply": text, "submit_click_count": 0, "receipt": receipt,
                    })));
                }
                return Ok(json_result(&json!({
                    "ok": false, "status": "commit_unknown", "reason": "a submit attempt was already reserved; reconcile instead of retrying",
                    "action_id": receipt.action_id(), "post_id": post_id, "url": url,
                    "reply": text, "submit_click_count": 0, "receipt": receipt,
                })));
            }
            SocialActionStatus::Prepared => {
                receipt = store.reset_prepared(receipt.action_id(), &actor.id, post_id)?;
            }
            SocialActionStatus::Draft => {}
        }
        let baseline = before.get("count").and_then(Value::as_u64).unwrap_or(0);
        if before.get("visible").and_then(Value::as_bool) == Some(true) {
            return Ok(json_result(&json!({
                "ok": true,
                "status": "already_present",
                "post_id": post_id,
                "url": url,
                "reply": text,
                "submit_click_count": 0,
                "reconcile": before,
            })));
        }

        let editor = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "replyEditorTarget",
            Some(&action_args),
        )
        .await?;
        let Some((editor_x, editor_y)) = verified_write_target(&editor, post_id) else {
            return Ok(json_result(&failure_payload(
                editor
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("reply_editor_unavailable"),
                json!({ "post_id": post_id, "url": url, "editor": editor, "submit_click_count": 0 }),
            )));
        };
        self.page.click(editor_x, editor_y).await?;
        tokio::time::sleep(Duration::from_millis(150)).await;
        let draft = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "replyDraftState",
            Some(&action_args),
        )
        .await?;
        if draft.get("ok").and_then(Value::as_bool) != Some(true)
            || draft.get("focused").and_then(Value::as_bool) != Some(true)
            || !draft
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .is_empty()
        {
            return Ok(json_result(&failure_payload(
                "reply_editor_not_empty_or_focused",
                json!({ "post_id": post_id, "url": url, "draft": draft, "submit_click_count": 0 }),
            )));
        }
        self.page.type_chars(&text).await?;
        let typed_deadline = Instant::now() + Duration::from_secs(5);
        let typed = loop {
            let state = crate::sites::learning::run_site_browser_tool(
                &self.page,
                SITE_ID,
                "replyDraftState",
                Some(&action_args),
            )
            .await?;
            if state.get("value").and_then(Value::as_str) == Some(text.as_str())
                || Instant::now() >= typed_deadline
            {
                break state;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        };
        if typed.get("value").and_then(Value::as_str) != Some(text.as_str()) {
            return Ok(json_result(&failure_payload(
                "reply_draft_mismatch",
                json!({ "post_id": post_id, "url": url, "draft": typed, "submit_click_count": 0 }),
            )));
        }

        let final_page_state =
            crate::sites::learning::run_site_browser_tool(&self.page, SITE_ID, "pageState", None)
                .await?;
        let final_detail =
            crate::sites::learning::run_site_browser_tool(&self.page, SITE_ID, "postDetail", None)
                .await?;
        let final_draft = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "replyDraftState",
            Some(&action_args),
        )
        .await?;
        if final_page_state.get("ok").and_then(Value::as_bool) != Some(true)
            || final_detail.get("ok").and_then(Value::as_bool) != Some(true)
            || final_detail.get("id").and_then(Value::as_str) != Some(post_id)
            || final_draft.get("value").and_then(Value::as_str) != Some(text.as_str())
        {
            return Ok(json_result(&failure_payload(
                "volatile_state_changed_before_submit",
                json!({ "post_id": post_id, "url": url, "page_state": final_page_state, "detail": final_detail, "draft": final_draft, "submit_click_count": 0 }),
            )));
        }
        let submit = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "replySubmitTarget",
            Some(&action_args),
        )
        .await?;
        let Some((submit_x, submit_y)) = verified_write_target(&submit, post_id) else {
            return Ok(json_result(&failure_payload(
                submit
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("reply_submit_unavailable"),
                json!({ "post_id": post_id, "url": url, "submit": submit, "submit_click_count": 0 }),
            )));
        };

        let final_rendered = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "renderedReplyState",
            Some(&rendered_args),
        )
        .await?;
        if final_rendered.get("ok").and_then(Value::as_bool) != Some(true)
            || final_rendered.get("author").and_then(Value::as_str) != Some(actor.id.as_str())
            || final_rendered.get("visible").and_then(Value::as_bool) == Some(true)
        {
            return Ok(json_result(&failure_payload(
                "actor_or_reply_state_changed_before_submit",
                json!({ "post_id": post_id, "url": url, "reconcile": final_rendered, "submit_click_count": 0 }),
            )));
        }
        receipt = store.mark_prepared(receipt.action_id(), &actor.id, post_id, 300)?;
        let precommit_ids = value_string_array(&final_rendered, "ids");
        receipt = store.begin_commit(
            receipt.action_id(),
            &actor.id,
            post_id,
            precommit_ids.clone(),
        )?;
        let action_id = receipt.action_id().to_string();
        let dispatch_error = self
            .page
            .click(submit_x, submit_y)
            .await
            .err()
            .map(|error| format!("{error:#}"));
        let deadline = Instant::now() + Duration::from_secs_f64(wait_seconds);
        let mut reconcile_error = None;
        let mut reconciled = json!({ "ok": false, "status": "not_observed", "ids": [] });
        loop {
            match crate::sites::learning::run_site_browser_tool(
                &self.page,
                SITE_ID,
                "renderedReplyState",
                Some(&rendered_args),
            )
            .await
            {
                Ok(state) => {
                    if state.get("author").and_then(Value::as_str) != Some(actor.id.as_str()) {
                        reconcile_error =
                            Some("signed-in actor changed after submit dispatch".to_string());
                        reconciled = state;
                        break;
                    }
                    let ids = value_string_array(&state, "ids");
                    let found = ids.iter().any(|id| !precommit_ids.contains(id));
                    reconciled = state;
                    if found || Instant::now() >= deadline {
                        break;
                    }
                }
                Err(error) => {
                    reconcile_error = Some(format!("{error:#}"));
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let new_target_id = value_string_array(&reconciled, "ids")
            .into_iter()
            .find(|id| !precommit_ids.contains(id));
        let (committed, persisted_receipt, receipt_error) = if let Some(new_id) = new_target_id {
            match store.reconcile_committed(&action_id, &new_id) {
                Ok(receipt) => (true, Some(receipt), None),
                Err(error) => (false, None, Some(format!("{error:#}"))),
            }
        } else {
            match store.finish_commit(&action_id, false) {
                Ok(receipt) => (false, Some(receipt), None),
                Err(error) => (false, None, Some(format!("{error:#}"))),
            }
        };
        Ok(json_result(&json!({
            "ok": committed,
            "status": if committed { "committed" } else { "commit_unknown" },
            "action_id": action_id,
            "post_id": post_id,
            "url": url,
            "reply": text,
            "interaction": "trusted_pointer_and_keyboard",
            "platform_api_called": false,
            "submit_click_count": 1,
            "dispatch_error": dispatch_error,
            "reconcile_error": reconcile_error,
            "receipt_error": receipt_error,
            "receipt": persisted_receipt,
            "baseline_exact_reply_count": baseline,
            "reconcile": reconciled,
        })))
    }
}

fn value_string_array(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn verified_write_target(target: &Value, post_id: &str) -> Option<(f64, f64)> {
    if target.get("ok").and_then(Value::as_bool) != Some(true)
        || target.get("hit_owned").and_then(Value::as_bool) != Some(true)
        || target.get("post_id").and_then(Value::as_str) != Some(post_id)
    {
        return None;
    }
    Some((target.get("x")?.as_f64()?, target.get("y")?.as_f64()?))
}

async fn read_clicked_x_posts(
    page: &PageSession,
    ctx: &ToolContext,
    candidates: &Value,
    deep: i64,
    num_comments: i64,
    wait_seconds: f64,
) -> anyhow::Result<Value> {
    if deep <= 0 {
        return Ok(Value::Array(Vec::new()));
    }
    let Some(items) = candidates.as_array() else {
        return Ok(Value::Array(Vec::new()));
    };
    let mut output = Vec::new();
    for candidate in items {
        if output.len() >= deep as usize {
            break;
        }
        let Some(post_id) = candidate
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()))
        else {
            continue;
        };
        let result = match read_clicked_x_post(page, ctx, post_id, num_comments, wait_seconds).await
        {
            Ok(result) => result,
            Err(error) => failure_payload(
                "deep_read_error",
                json!({
                    "post_id": post_id,
                    "navigation_policy": "card_click_only",
                    "origin_preserved": false,
                    "error": format!("{error:#}"),
                }),
            ),
        };
        let restored = result
            .get("close")
            .and_then(|close| close.get("ok"))
            .and_then(Value::as_bool)
            .or_else(|| result.get("origin_preserved").and_then(Value::as_bool))
            .unwrap_or(false);
        output.push(result);
        if !restored {
            break;
        }
    }
    Ok(Value::Array(output))
}

fn x_deep_read_status(candidates: &Value, deep_posts: &Value, deep: i64) -> Value {
    let available = candidates.as_array().map(Vec::len).unwrap_or(0);
    let requested = (deep.max(0) as usize).min(available);
    let attempted = deep_posts.as_array().map(Vec::len).unwrap_or(0);
    let completed = deep_posts
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item.get("ok").and_then(Value::as_bool) == Some(true))
        .count();
    json!({
        "ok": deep <= 0 || (attempted == requested && completed == requested),
        "requested": requested,
        "attempted": attempted,
        "completed": completed,
    })
}

fn relocatable_x_post_target(target: &Value) -> bool {
    matches!(
        target.get("status").and_then(Value::as_str),
        Some("post_not_found" | "post_link_not_visible")
    )
}

async fn locate_x_post_card(page: &PageSession, post_id: &str) -> anyhow::Result<Value> {
    let args = json!({ "id": post_id });
    let mut target =
        crate::sites::learning::run_site_browser_tool(page, SITE_ID, "postCardTarget", Some(&args))
            .await?;
    if target.get("ok").and_then(Value::as_bool) == Some(true)
        || !relocatable_x_post_target(&target)
    {
        return Ok(target);
    }

    let state =
        crate::sites::learning::run_site_browser_tool(page, SITE_ID, "sourceSurfaceState", None)
            .await?;
    let scroll_tool = match state.get("page_type").and_then(Value::as_str) {
        Some("search") => "scrollResults",
        Some("profile") => "scrollPosts",
        _ => return Ok(target),
    };
    let reset = crate::sites::learning::run_site_browser_tool(
        page,
        SITE_ID,
        scroll_tool,
        Some(&json!({ "to_top": true })),
    )
    .await?;
    if reset.get("ok").and_then(Value::as_bool) != Some(true) {
        return Ok(target);
    }

    tokio::time::sleep(Duration::from_millis(350)).await;
    for _ in 0..40 {
        target = crate::sites::learning::run_site_browser_tool(
            page,
            SITE_ID,
            "postCardTarget",
            Some(&args),
        )
        .await?;
        if target.get("ok").and_then(Value::as_bool) == Some(true)
            || !relocatable_x_post_target(&target)
        {
            return Ok(target);
        }
        let scroll = crate::sites::learning::run_site_browser_tool(
            page,
            SITE_ID,
            scroll_tool,
            Some(&json!({})),
        )
        .await?;
        if scroll.get("ok").and_then(Value::as_bool) != Some(true) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(350)).await;
        target = crate::sites::learning::run_site_browser_tool(
            page,
            SITE_ID,
            "postCardTarget",
            Some(&args),
        )
        .await?;
        if target.get("ok").and_then(Value::as_bool) == Some(true)
            || !relocatable_x_post_target(&target)
        {
            return Ok(target);
        }
        let observed = crate::sites::learning::run_site_browser_tool(
            page,
            SITE_ID,
            "sourceSurfaceState",
            None,
        )
        .await?;
        if observed.get("at_end").and_then(Value::as_bool) == Some(true) {
            tokio::time::sleep(Duration::from_millis(600)).await;
            target = crate::sites::learning::run_site_browser_tool(
                page,
                SITE_ID,
                "postCardTarget",
                Some(&args),
            )
            .await?;
            if target.get("ok").and_then(Value::as_bool) == Some(true)
                || !relocatable_x_post_target(&target)
            {
                return Ok(target);
            }
            let confirmed = crate::sites::learning::run_site_browser_tool(
                page,
                SITE_ID,
                "sourceSurfaceState",
                None,
            )
            .await?;
            let no_result_growth = confirmed
                .get("result_count")
                .and_then(Value::as_u64)
                .zip(observed.get("result_count").and_then(Value::as_u64))
                .is_some_and(|(confirmed, observed)| confirmed <= observed);
            let no_height_growth = confirmed
                .get("document_height")
                .and_then(Value::as_u64)
                .zip(observed.get("document_height").and_then(Value::as_u64))
                .is_some_and(|(confirmed, observed)| confirmed <= observed);
            if confirmed.get("at_end").and_then(Value::as_bool) == Some(true)
                && no_result_growth
                && no_height_growth
            {
                break;
            }
        }
    }
    Ok(target)
}

fn validated_x_post_click_target(target: &Value, expected_id: &str) -> anyhow::Result<(f64, f64)> {
    if target.get("ok").and_then(Value::as_bool) != Some(true)
        || target.get("hit_owned").and_then(Value::as_bool) != Some(true)
        || target.get("id").and_then(Value::as_str) != Some(expected_id)
    {
        anyhow::bail!("X post click target is not owned by the expected timeline post");
    }
    let target_url = target
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if x_post_id(target_url).as_deref() != Some(expected_id) {
        anyhow::bail!("X post identity changed before click");
    }
    let x = target
        .get("x")
        .and_then(Value::as_f64)
        .ok_or_else(|| anyhow::anyhow!("X post target is missing x"))?;
    let y = target
        .get("y")
        .and_then(Value::as_f64)
        .ok_or_else(|| anyhow::anyhow!("X post target is missing y"))?;
    Ok((x, y))
}

fn x_source_surface_identity_matches(
    source_url: &str,
    source_state: &Value,
    current_url: &str,
    current_state: &Value,
) -> bool {
    if source_url != current_url
        || gate_reason(current_state).is_some()
        || current_state.get("ok").and_then(Value::as_bool) != Some(true)
        || current_state.get("hydrated").and_then(Value::as_bool) != Some(true)
    {
        return false;
    }
    let expected_type = source_state
        .get("page_type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let actual_type = current_state
        .get("page_type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(expected_type, "search" | "profile") || expected_type != actual_type {
        return false;
    }
    if expected_type == "search" {
        let expected_query = source_state
            .get("search_query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let actual_query = current_state
            .get("search_query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if expected_query.is_empty() || expected_query != actual_query {
            return false;
        }
    }
    if expected_type == "profile" {
        let expected_profile = source_state
            .get("profile_username")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let actual_profile = current_state
            .get("profile_username")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if expected_profile.is_empty() || expected_profile != actual_profile {
            return false;
        }
    }
    current_state
        .get("result_count")
        .and_then(Value::as_u64)
        .is_some_and(|count| count > 0)
}

fn x_source_surface_extent_matches(source_state: &Value, current_state: &Value) -> bool {
    let expected_count = source_state
        .get("result_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let actual_count = current_state
        .get("result_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let expected_height = source_state
        .get("document_height")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let actual_height = current_state
        .get("document_height")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    actual_count >= expected_count && actual_height.saturating_add(8) >= expected_height
}

fn x_source_surface_matches(
    source_url: &str,
    source_state: &Value,
    current_url: &str,
    current_state: &Value,
) -> bool {
    x_source_surface_identity_matches(source_url, source_state, current_url, current_state)
        && x_source_surface_extent_matches(source_state, current_state)
}

async fn restore_x_source_surface(
    page: &PageSession,
    source_url: &str,
    source_state: &Value,
    wait_seconds: f64,
) -> anyhow::Result<Value> {
    let mut url = current_url(page).await.unwrap_or_default();
    let mut state =
        crate::sites::learning::run_site_browser_tool(page, SITE_ID, "sourceSurfaceState", None)
            .await?;
    let mut back_error = None;
    let mut probe_error = None;
    let mut used_history = false;
    if !x_source_surface_identity_matches(source_url, source_state, &url, &state) {
        let can_history_return = state.get("page_type").and_then(Value::as_str) == Some("post")
            || (gate_reason(&state).is_some() && url != source_url);
        if !can_history_return {
            return Ok(json!({
                "ok": false,
                "strategy": "refused_wrong_surface",
                "source_url": source_url,
                "url": url,
                "state": state,
                "reason": "originating_list_not_restored",
            }));
        }
        used_history = true;
        back_error = page
            .evaluate_json("history.back(); return {ok: true};")
            .await
            .err()
            .map(|error| format!("{error:#}"));
        let deadline = Instant::now() + Duration::from_secs_f64(wait_seconds.clamp(1.0, 15.0));
        while Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(250)).await;
            url = current_url(page).await.unwrap_or_default();
            state = match crate::sites::learning::run_site_browser_tool(
                page,
                SITE_ID,
                "sourceSurfaceState",
                None,
            )
            .await
            {
                Ok(state) => state,
                Err(error) => {
                    probe_error = Some(format!("{error:#}"));
                    continue;
                }
            };
            if x_source_surface_identity_matches(source_url, source_state, &url, &state) {
                break;
            }
        }
    }
    if !x_source_surface_identity_matches(source_url, source_state, &url, &state) {
        return Ok(json!({
            "ok": false,
            "strategy": "history_back_failed",
            "source_url": source_url,
            "url": url,
            "state": state,
            "back_error": back_error,
            "probe_error": probe_error,
            "reason": "originating_list_not_restored",
        }));
    }

    let extent_deadline = Instant::now() + Duration::from_secs_f64(wait_seconds.clamp(1.0, 15.0));
    let extent_scroll_tool = match source_state.get("page_type").and_then(Value::as_str) {
        Some("search") => "scrollResults",
        Some("profile") => "scrollPosts",
        _ => "",
    };
    while !x_source_surface_extent_matches(source_state, &state)
        && !extent_scroll_tool.is_empty()
        && Instant::now() < extent_deadline
    {
        let scroll = crate::sites::learning::run_site_browser_tool(
            page,
            SITE_ID,
            extent_scroll_tool,
            Some(&json!({})),
        )
        .await?;
        if scroll.get("ok").and_then(Value::as_bool) != Some(true) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(350)).await;
        url = current_url(page).await.unwrap_or_default();
        state = crate::sites::learning::run_site_browser_tool(
            page,
            SITE_ID,
            "sourceSurfaceState",
            None,
        )
        .await?;
        if !x_source_surface_identity_matches(source_url, source_state, &url, &state) {
            break;
        }
        // Being at the current document bottom does not prove exhaustion:
        // X can append the next virtualized batch after more than one second.
        // Keep probing until the captured extent is restored or the explicit
        // restoration deadline expires.
    }
    if !x_source_surface_matches(source_url, source_state, &url, &state) {
        return Ok(json!({
            "ok": false,
            "strategy": "lazy_extent_restore_failed",
            "source_url": source_url,
            "url": url,
            "state": state,
            "expected_result_count": source_state.get("result_count"),
            "expected_document_height": source_state.get("document_height"),
            "reason": "originating_list_not_restored",
        }));
    }

    let expected_y = source_state
        .get("scroll_y")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0);
    let scroll_deadline = Instant::now() + Duration::from_secs_f64(wait_seconds.clamp(1.0, 5.0));
    let (final_url, final_state, final_y, scroll_restored) = loop {
        let current_y = state
            .get("scroll_y")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .max(0);
        let delta = expected_y.saturating_sub(current_y);
        if delta != 0 {
            page.scroll(delta).await?;
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let observed_url = current_url(page).await.unwrap_or_default();
        let observed_state = crate::sites::learning::run_site_browser_tool(
            page,
            SITE_ID,
            "sourceSurfaceState",
            None,
        )
        .await?;
        let observed_y = observed_state
            .get("scroll_y")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .max(0);
        let restored = expected_y.abs_diff(observed_y) <= 8;
        if restored || Instant::now() >= scroll_deadline {
            break (observed_url, observed_state, observed_y, restored);
        }
        state = observed_state;
    };
    let restored = scroll_restored
        && x_source_surface_matches(source_url, source_state, &final_url, &final_state);
    Ok(json!({
        "ok": restored,
        "strategy": if used_history && back_error.is_some() {
            "history_back_after_context_change"
        } else if used_history {
            "history_back"
        } else {
            "scroll_restore"
        },
        "source_url": source_url,
        "url": final_url,
        "expected_scroll_y": expected_y,
        "scroll_y": final_y,
        "scroll_restored": scroll_restored,
        "state": final_state,
        "back_error": back_error,
        "probe_error": probe_error,
        "reason": if restored { Value::Null } else { json!("originating_list_not_restored") },
    }))
}

async fn x_preclick_failure(
    page: &PageSession,
    post_id: &str,
    source_url: &str,
    source_state: &Value,
    wait_seconds: f64,
    reason: &str,
    evidence: Value,
) -> Value {
    let close = restore_x_source_surface(page, source_url, source_state, wait_seconds)
        .await
        .unwrap_or_else(|error| json!({ "ok": false, "error": format!("{error:#}") }));
    failure_payload(
        reason,
        json!({
            "post_id": post_id,
            "navigation_policy": "card_click_only",
            "source_url": source_url,
            "evidence": evidence,
            "close": close,
        }),
    )
}

fn x_clicked_post_result(
    post_id: &str,
    source_url: &str,
    entity: Value,
    comments: Value,
    state: Value,
    stage_errors: Value,
    close: Value,
) -> Value {
    let read_ok = stage_errors
        .as_object()
        .is_some_and(serde_json::Map::is_empty);
    let close_ok = close.get("ok").and_then(Value::as_bool) == Some(true);
    json!({
        "ok": read_ok && close_ok,
        "reason": if !read_ok {
            json!("post_read_failed")
        } else if !close_ok {
            json!("originating_list_not_restored")
        } else {
            Value::Null
        },
        "post_id": post_id,
        "navigation_policy": "card_click_only",
        "source_url": source_url,
        "open_strategy": "trusted_cdp_timeline_click",
        "entity": entity,
        "comments": comments,
        "state": state,
        "stage_errors": stage_errors,
        "close": close,
    })
}

async fn read_clicked_x_post(
    page: &PageSession,
    ctx: &ToolContext,
    post_id: &str,
    num_comments: i64,
    wait_seconds: f64,
) -> anyhow::Result<Value> {
    let source_url = current_url(page).await.unwrap_or_default();
    let source_state =
        crate::sites::learning::run_site_browser_tool(page, SITE_ID, "sourceSurfaceState", None)
            .await?;
    if !matches!(
        source_state.get("page_type").and_then(Value::as_str),
        Some("search" | "profile")
    ) {
        return Ok(failure_payload(
            "unsupported_source_surface",
            json!({
                "post_id": post_id,
                "navigation_policy": "card_click_only",
                "source_url": source_url,
                "origin_preserved": true,
                "source_state": source_state,
            }),
        ));
    }

    let initial = match locate_x_post_card(page, post_id).await {
        Ok(target) => target,
        Err(error) => {
            return Ok(x_preclick_failure(
                page,
                post_id,
                &source_url,
                &source_state,
                wait_seconds,
                "post_card_relocation_error",
                json!({ "error": format!("{error:#}") }),
            )
            .await)
        }
    };
    if initial.get("ok").and_then(Value::as_bool) != Some(true) {
        let reason = initial
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("post_card_not_found")
            .to_string();
        return Ok(x_preclick_failure(
            page,
            post_id,
            &source_url,
            &source_state,
            wait_seconds,
            &reason,
            json!({ "open": initial }),
        )
        .await);
    }

    tokio::time::sleep(Duration::from_millis(180)).await;
    let args = json!({ "id": post_id });
    let fresh = match crate::sites::learning::run_site_browser_tool(
        page,
        SITE_ID,
        "postCardTarget",
        Some(&args),
    )
    .await
    {
        Ok(target) => target,
        Err(error) => {
            return Ok(x_preclick_failure(
                page,
                post_id,
                &source_url,
                &source_state,
                wait_seconds,
                "post_target_refresh_error",
                json!({ "initial_target": initial, "error": format!("{error:#}") }),
            )
            .await)
        }
    };
    if fresh.get("ok").and_then(Value::as_bool) != Some(true)
        || initial.get("url").and_then(Value::as_str) != fresh.get("url").and_then(Value::as_str)
    {
        return Ok(x_preclick_failure(
            page,
            post_id,
            &source_url,
            &source_state,
            wait_seconds,
            "post_card_changed_before_click",
            json!({
                "initial_target": initial,
                "fresh_target": fresh,
            }),
        )
        .await);
    }
    let (x, y) = match validated_x_post_click_target(&fresh, post_id) {
        Ok(point) => point,
        Err(error) => {
            return Ok(x_preclick_failure(
                page,
                post_id,
                &source_url,
                &source_state,
                wait_seconds,
                "post_target_validation_error",
                json!({ "target": fresh, "error": format!("{error:#}") }),
            )
            .await)
        }
    };
    if let Err(error) = page.click(x, y).await {
        let close = restore_x_source_surface(page, &source_url, &source_state, wait_seconds)
            .await
            .unwrap_or_else(
                |close_error| json!({ "ok": false, "error": format!("{close_error:#}") }),
            );
        return Ok(failure_payload(
            "post_click_error",
            json!({
                "post_id": post_id,
                "navigation_policy": "card_click_only",
                "source_url": source_url,
                "error": format!("{error:#}"),
                "close": close,
            }),
        ));
    }

    let deadline = Instant::now() + Duration::from_secs_f64(wait_seconds.clamp(1.0, 330.0));
    let mut open = json!({ "ok": false, "status": "waiting" });
    let mut open_probe_error = None;
    while Instant::now() < deadline {
        match crate::sites::learning::run_site_browser_tool(
            page,
            SITE_ID,
            "postOpenState",
            Some(&args),
        )
        .await
        {
            Ok(state) => open = state,
            Err(error) => {
                open_probe_error = Some(format!("{error:#}"));
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
        }
        if open.get("ok").and_then(Value::as_bool) == Some(true)
            || gate_reason(&open).is_some()
            || open.get("status").and_then(Value::as_str) == Some("wrong_post")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    if open.get("ok").and_then(Value::as_bool) != Some(true) {
        let close = restore_x_source_surface(page, &source_url, &source_state, wait_seconds)
            .await
            .unwrap_or_else(|error| json!({ "ok": false, "error": format!("{error:#}") }));
        let reason = gate_reason(&open).unwrap_or_else(|| {
            open.get("status")
                .and_then(Value::as_str)
                .unwrap_or("post_click_failed")
        });
        return Ok(failure_payload(
            reason,
            json!({
                "post_id": post_id,
                "navigation_policy": "card_click_only",
                "source_url": source_url,
                "open": open,
                "probe_error": open_probe_error,
                "close": close,
            }),
        ));
    }

    let mut entity = Value::Null;
    let mut comments = if num_comments > 0 {
        Value::Null
    } else {
        Value::Array(Vec::new())
    };
    let mut final_state = Value::Null;
    let mut stage_errors = serde_json::Map::new();
    match invoke_browser_tool(page, ctx, SITE_ID, "postDetail", None, false).await {
        Ok(value) => {
            let valid = value.get("ok").and_then(Value::as_bool) == Some(true)
                && value.get("id").and_then(Value::as_str) == Some(post_id);
            entity = value;
            if !valid {
                stage_errors.insert(
                    "entity".into(),
                    json!("X post identity changed during click-first read"),
                );
            }
        }
        Err(error) => {
            stage_errors.insert("entity".into(), json!(format!("{error:#}")));
        }
    }
    if !stage_errors.contains_key("entity") && num_comments > 0 {
        match invoke_browser_tool(
            page,
            ctx,
            SITE_ID,
            "comments",
            Some(&json!({ "limit": num_comments })),
            true,
        )
        .await
        {
            Ok(value) => comments = value,
            Err(error) => {
                stage_errors.insert("comments".into(), json!(format!("{error:#}")));
            }
        }
    }
    if !stage_errors.contains_key("entity") {
        match crate::sites::learning::run_site_browser_tool(
            page,
            SITE_ID,
            "postOpenState",
            Some(&args),
        )
        .await
        {
            Ok(value) => {
                let valid = value.get("ok").and_then(Value::as_bool) == Some(true)
                    && value.get("post_id").and_then(Value::as_str) == Some(post_id);
                final_state = value;
                if !valid {
                    stage_errors.insert(
                        "final_state".into(),
                        json!("X post became unavailable during reply collection"),
                    );
                }
            }
            Err(error) => {
                stage_errors.insert("final_state".into(), json!(format!("{error:#}")));
            }
        }
    }
    let close = restore_x_source_surface(page, &source_url, &source_state, wait_seconds)
        .await
        .unwrap_or_else(|error| json!({ "ok": false, "error": format!("{error:#}") }));
    Ok(x_clicked_post_result(
        post_id,
        &source_url,
        entity,
        comments,
        final_state,
        Value::Object(stage_errors),
        close,
    ))
}

#[async_trait]
impl Tool for PageStateTool {
    fn name(&self) -> &str {
        "page_state"
    }

    fn description(&self) -> &str {
        "Open or reuse X and return route, login, challenge, rate-limit, and hydration state."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1, "maximum": 330 }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        ensure_site_page(&self.page, HOST_ROOT, HOME_URL).await?;
        let _ = wait_for_browser_tool(&self.page, SITE_ID, "pageState", None, wait_seconds).await?;
        let state = invoke_browser_tool(&self.page, ctx, SITE_ID, "pageState", None, false).await?;
        Ok(json_result(&state))
    }
}

async fn read_x_post(
    page: &PageSession,
    ctx: &ToolContext,
    locator: &str,
    num_comments: i64,
    wait_seconds: f64,
) -> anyhow::Result<Value> {
    let url = x_post_url(locator)?;
    let expected_id = x_post_id(&url)
        .ok_or_else(|| anyhow::anyhow!("canonical X post URL is missing a status id"))?;
    navigate_https(page, &url).await?;
    let detail = wait_for_browser_tool(page, SITE_ID, "postDetail", None, wait_seconds).await?;
    if let Some(reason) = gate_reason(&detail) {
        return Ok(failure_payload(
            reason,
            json!({ "input": locator, "url": url, "entity": detail }),
        ));
    }
    if detail.get("ok").and_then(Value::as_bool) != Some(true)
        || detail.get("id").and_then(Value::as_str) != Some(expected_id.as_str())
    {
        return Ok(failure_payload(
            if detail.get("ok").and_then(Value::as_bool) == Some(true) {
                "wrong_post"
            } else {
                detail
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("post_unavailable")
            },
            json!({ "input": locator, "expected_id": expected_id, "url": url, "entity": detail }),
        ));
    }
    let entity = invoke_browser_tool(page, ctx, SITE_ID, "postDetail", None, false).await?;
    if let Some(reason) = gate_reason(&entity) {
        return Ok(failure_payload(
            reason,
            json!({ "input": locator, "url": url, "entity": entity }),
        ));
    }
    if entity.get("ok").and_then(Value::as_bool) != Some(true)
        || entity.get("id").and_then(Value::as_str) != Some(expected_id.as_str())
    {
        return Ok(failure_payload(
            if entity.get("ok").and_then(Value::as_bool) == Some(true) {
                "post_changed_during_read"
            } else {
                entity
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("post_changed_during_read")
            },
            json!({ "input": locator, "expected_id": expected_id, "url": url, "entity": entity }),
        ));
    }
    let comments = if num_comments > 0 {
        match invoke_browser_tool(
            page,
            ctx,
            SITE_ID,
            "comments",
            Some(&json!({ "limit": num_comments })),
            true,
        )
        .await
        {
            Ok(comments) => comments,
            Err(error) => {
                let state = crate::sites::learning::run_site_browser_tool(
                    page,
                    SITE_ID,
                    "pageState",
                    None,
                )
                .await
                .unwrap_or_else(|state_error| {
                    json!({ "ok": false, "status": "state_check_failed", "error": format!("{state_error:#}") })
                });
                let reason = gate_reason(&state).unwrap_or("comment_collection_failed");
                return Ok(failure_payload(
                    reason,
                    json!({
                        "input": locator,
                        "url": url,
                        "entity": entity,
                        "state": state,
                        "error": format!("{error:#}"),
                    }),
                ));
            }
        }
    } else {
        Value::Array(Vec::new())
    };
    let final_state =
        crate::sites::learning::run_site_browser_tool(page, SITE_ID, "postDetail", None).await?;
    let final_id = final_state
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if final_state.get("ok").and_then(Value::as_bool) != Some(true) || final_id != expected_id {
        let reason = if final_state.get("ok").and_then(Value::as_bool) == Some(true) {
            "post_changed_during_read"
        } else {
            gate_reason(&final_state).unwrap_or_else(|| {
                final_state
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("page_changed_during_read")
            })
        };
        return Ok(failure_payload(
            reason,
            json!({
                "input": locator,
                "url": url,
                "entity": entity,
                "comments": comments,
                "state": final_state,
                "partial": true,
            }),
        ));
    }
    Ok(json!({
        "ok": true,
        "input": locator,
        "url": current_url(page).await.unwrap_or(url),
        "entity": entity,
        "comments": comments,
        "state": final_state,
    }))
}

fn x_profile_url(locator: &str) -> anyhow::Result<String> {
    let trimmed = locator.trim();
    if let Ok(mut url) = reqwest::Url::parse(trimmed) {
        validate_x_url(&url, "profile")?;
        let parts = url
            .path_segments()
            .into_iter()
            .flatten()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        if parts.len() != 1 {
            anyhow::bail!("X profile URL must identify exactly one profile");
        }
        let username = parts[0].to_ascii_lowercase();
        validate_username(&username, locator)?;
        url.set_host(Some("x.com"))?;
        url.set_path(&format!("/{username}"));
        url.set_query(None);
        url.set_fragment(None);
        return Ok(url.to_string());
    }
    let username = trimmed.trim_start_matches('@').to_ascii_lowercase();
    validate_username(&username, locator)?;
    Ok(format!("https://x.com/{username}"))
}

fn x_post_url(locator: &str) -> anyhow::Result<String> {
    let trimmed = locator.trim();
    if let Ok(mut url) = reqwest::Url::parse(trimmed) {
        validate_x_url(&url, "post")?;
        let identity = url
            .path_segments()
            .map(|segments| segments.collect::<Vec<_>>())
            .and_then(|parts| match parts.as_slice() {
                [username, "status", id]
                    if valid_username_segment(username)
                        && !id.is_empty()
                        && id.chars().all(|character| character.is_ascii_digit()) =>
                {
                    Some((username.to_ascii_lowercase(), (*id).to_string()))
                }
                ["i", "web", "status", id]
                    if !id.is_empty() && id.chars().all(|character| character.is_ascii_digit()) =>
                {
                    Some(("i/web".to_string(), (*id).to_string()))
                }
                _ => None,
            });
        let (owner, id) =
            identity.ok_or_else(|| anyhow::anyhow!("invalid X post URL: {locator}"))?;
        url.set_host(Some("x.com"))?;
        url.set_path(&format!("/{owner}/status/{id}"));
        url.set_query(None);
        url.set_fragment(None);
        return Ok(url.to_string());
    }
    if trimmed.len() < 5 || !trimmed.chars().all(|character| character.is_ascii_digit()) {
        anyhow::bail!("invalid X post URL or numeric id: {locator}");
    }
    Ok(format!("https://x.com/i/web/status/{trimmed}"))
}

fn x_post_id(raw_url: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw_url).ok()?;
    let parts = url
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let id = match parts.as_slice() {
        [_username, "status", id] => *id,
        ["i", "web", "status", id] => *id,
        _ => return None,
    };
    (!id.is_empty() && id.chars().all(|character| character.is_ascii_digit()))
        .then(|| id.to_string())
}

fn validate_x_url(url: &reqwest::Url, kind: &str) -> anyhow::Result<()> {
    if url.scheme() != "https" {
        anyhow::bail!("X {kind} URL must be https");
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if !matches!(
        host.as_str(),
        "x.com" | "www.x.com" | "twitter.com" | "www.twitter.com"
    ) {
        anyhow::bail!("X {kind} URL must use x.com or twitter.com");
    }
    if !url.username().is_empty() || url.password().is_some() {
        anyhow::bail!("X {kind} URL must not contain credentials");
    }
    Ok(())
}

fn validate_username(username: &str, locator: &str) -> anyhow::Result<()> {
    if username.is_empty()
        || username.len() > 15
        || !username
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
        || RESERVED_PROFILE_NAMES.contains(&username)
    {
        anyhow::bail!("invalid X username or profile URL: {locator}");
    }
    Ok(())
}

fn valid_username_segment(username: &str) -> bool {
    !username.is_empty()
        && username.len() <= 15
        && username
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
        && !RESERVED_PROFILE_NAMES.contains(&username.to_ascii_lowercase().as_str())
}

#[cfg(test)]
mod deep_read_tests {
    use super::*;

    #[test]
    fn source_surface_match_requires_the_original_lazy_extent() {
        let source = json!({
            "ok": true,
            "hydrated": true,
            "page_type": "search",
            "search_query": "agents",
            "result_count": 10,
            "document_height": 5000,
        });
        let truncated = json!({
            "ok": true,
            "hydrated": true,
            "page_type": "search",
            "search_query": "agents",
            "result_count": 3,
            "document_height": 1800,
        });

        assert!(!x_source_surface_matches(
            "https://x.com/search?q=agents&f=live",
            &source,
            "https://x.com/search?q=agents&f=live",
            &truncated,
        ));
    }

    #[test]
    fn deep_read_failure_preserves_completed_entity_and_stage_error() {
        let result = x_clicked_post_result(
            "123",
            "https://x.com/search?q=agents&f=live",
            json!({ "ok": true, "id": "123", "text": "evidence" }),
            Value::Null,
            json!({ "ok": true, "post_id": "123" }),
            json!({ "comments": "comment collection failed" }),
            json!({ "ok": true }),
        );

        assert_eq!(result.get("ok").and_then(Value::as_bool), Some(false));
        assert_eq!(
            result.get("reason").and_then(Value::as_str),
            Some("post_read_failed")
        );
        assert_eq!(
            result
                .get("entity")
                .and_then(|entity| entity.get("id"))
                .and_then(Value::as_str),
            Some("123")
        );
        assert_eq!(
            result
                .get("stage_errors")
                .and_then(|errors| errors.get("comments"))
                .and_then(Value::as_str),
            Some("comment collection failed")
        );
    }
}
