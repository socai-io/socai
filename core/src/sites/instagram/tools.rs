use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::tool::ToolProgressSender;
use crate::agent::{Backend as LlmProvider, Tool, ToolContext, ToolResult};
use crate::cdp::PageSession;
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

const SITE_ID: &str = "instagram";
const HOME_URL: &str = "https://www.instagram.com/";
const HOST_ROOT: &str = "instagram.com";
const RESERVED_PROFILE_NAMES: &[&str] = &[
    "accounts",
    "about",
    "api",
    "challenge",
    "developer",
    "direct",
    "emails",
    "download",
    "explore",
    "graphql",
    "legal",
    "oauth",
    "p",
    "privacy",
    "reel",
    "reels",
    "settings",
    "stories",
    "terms",
    "web",
];

pub async fn instagram_agent_tools(
    page: Arc<PageSession>,
    _llm_provider: Arc<dyn LlmProvider>,
) -> anyhow::Result<Vec<Arc<dyn Tool>>> {
    Ok(instagram_tools(page))
}

pub fn instagram_agent_instructions(extra: &str) -> String {
    crate::sites::learning::site_agent_instructions(SITE_ID, extra)
}

fn instagram_tools(page: Arc<PageSession>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(SearchTool { page: page.clone() }),
        Arc::new(AccountsTool { page: page.clone() }),
        Arc::new(ProfileTool { page: page.clone() }),
        Arc::new(GetPostsTool { page: page.clone() }),
        Arc::new(CommentTool { page: page.clone() }),
        Arc::new(PageStateTool { page }),
    ]
}

pub static INSTAGRAM_NATIVE_ADAPTER: NativeSiteAdapter = NativeSiteAdapter {
    id: SITE_ID,
    about: "Instagram (instagram.com)",
    home_url: "",
    agent_tools: |page, llm| Box::pin(instagram_agent_tools(page, llm)),
    default_agent_tools: None,
    agent_instructions: instagram_agent_instructions,
    default_agent_instructions: None,
    commands: &[
        SiteCommand {
            name: "search",
            tool_name: "search",
            about: "Search Instagram posts and Reels. Default opens each result and returns its caption, media, and comments. --preview returns grid cards only. Use search_accounts to find people.",
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
                    help: "Number of posts or Reels to open. Defaults to 10. With --preview, number of grid cards.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "num_comments",
                    long: Some("num-comments"),
                    value_name: "N",
                    help: "Comments to collect per opened post. Defaults to 8; 0 skips comments. Ignored with --preview.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "preview",
                    long: Some("preview"),
                    value_name: "PREVIEW",
                    help: "Return search-grid cards only, without opening posts.",
                    required: false,
                    kind: ArgKind::Flag,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for each page to hydrate. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_search,
        },
        SiteCommand {
            name: "search_accounts",
            tool_name: "search_accounts",
            about: "Find Instagram accounts from the homepage search dropdown, in the order shown.",
            args: &[
                CommandArg {
                    key: "query",
                    long: None,
                    value_name: "QUERY",
                    help: "Name or username to look up",
                    required: true,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for the suggestion dropdown. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_accounts,
        },
        SiteCommand {
            name: "profile",
            tool_name: "profile",
            about: "Read an Instagram profile and collect visible post and reel cards.",
            args: &[
                CommandArg {
                    key: "profile",
                    long: None,
                    value_name: "HANDLE_OR_URL",
                    help: "Instagram username or profile URL.",
                    required: true,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "num",
                    long: Some("num"),
                    value_name: "N",
                    help: "Number of post or reel cards to collect by scrolling. Defaults to 10.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "deep",
                    long: Some("deep"),
                    value_name: "N",
                    help: "Open up to N grid cards by trusted page click and return full details. Defaults to 0.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "num_comments",
                    long: Some("num-comments"),
                    value_name: "N",
                    help: "Comments to collect for each deeply read post. Defaults to 8.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for the profile page to hydrate. Defaults to 30.",
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
            about: "Read Instagram posts or Reels by URL or shortcode, including comments and playable video.",
            args: &[
                CommandArg {
                    key: "posts",
                    long: Some("post"),
                    value_name: "URL_OR_SHORTCODE",
                    help: "Instagram post/reel URL or shortcode. Repeat to read multiple items.",
                    required: true,
                    kind: ArgKind::StrList,
                },
                CommandArg {
                    key: "num_comments",
                    long: Some("num-comments"),
                    value_name: "N",
                    help: "Comments to collect per post. Defaults to 8; 0 skips comments.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for each post page to hydrate. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_get_posts,
        },
        SiteCommand {
            name: "comment",
            tool_name: "comment",
            about: "Comment on one Instagram post or Reel using verified pointer and keyboard events.",
            args: &[
                CommandArg {
                    key: "post",
                    long: None,
                    value_name: "URL_OR_SHORTCODE",
                    help: "Target Instagram post/Reel URL or shortcode.",
                    required: true,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "text",
                    long: Some("text"),
                    value_name: "TEXT",
                    help: "Exact comment text. The command refuses to replace an existing draft.",
                    required: true,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for hydration and post-submit reconciliation. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_comment,
        },
        SiteCommand {
            name: "page_state",
            tool_name: "page_state",
            about: "Open or reuse Instagram and print page state as JSON.",
            args: &[CommandArg {
                key: "wait_seconds",
                long: Some("wait-seconds"),
                value_name: "SECONDS",
                help: "Maximum wait for an Instagram page. Defaults to 30.",
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

fn run_accounts(
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
        "search_accounts",
        "search_accounts",
    )
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

fn run_comment(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    run_named(page, args, debug_snapshot, progress, "comment", "comment")
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
        instagram_tools(page),
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
        "Search Instagram posts and Reels. Default: collect up to `num` post/Reel cards, open each one, and return caption, author, likes, comments, the short post URL, and the playable video URL together. `preview=true` returns grid cards only and does not open posts. Use search_accounts to find people."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "maxLength": 512 },
                "num": { "type": "integer", "default": 10, "minimum": 1, "maximum": 100 },
                "num_comments": { "type": "integer", "default": 8, "minimum": 0, "maximum": 100 },
                "preview": { "type": "boolean", "default": false },
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
        let preview = input
            .get("preview")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let num_comments = if preview {
            0
        } else {
            get_i64(&input, "num_comments", DEFAULT_COMMENT_COUNT).clamp(0, MAX_TOOL_ITEMS)
        };
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        let target = format!(
            "https://www.instagram.com/explore/search/keyword/?q={}",
            percent_encode_query(&query)
        );
        navigate_https(&self.page, &target).await?;
        let search_args = json!({ "query": query });
        let mut state = wait_for_browser_tool(
            &self.page,
            SITE_ID,
            "searchState",
            Some(&search_args),
            wait_seconds,
        )
        .await?;
        if !state.get("ok").and_then(Value::as_bool).unwrap_or(false)
            && !state.get("empty").and_then(Value::as_bool).unwrap_or(false)
            && gate_reason(&state).is_none()
        {
            navigate_https(&self.page, HOME_URL).await?;
            let _ = wait_for_browser_tool(
                &self.page,
                SITE_ID,
                "pageState",
                None,
                wait_seconds.min(15.0),
            )
            .await?;
            let _ = invoke_browser_tool(
                &self.page,
                ctx,
                SITE_ID,
                "setSearchQuery",
                Some(&search_args),
                false,
            )
            .await?;
            state = wait_for_browser_tool(
                &self.page,
                SITE_ID,
                "searchState",
                Some(&search_args),
                wait_seconds,
            )
            .await?;
        }
        if let Some(reason) = gate_reason(&state) {
            return Ok(json_result(&failure_payload(
                reason,
                json!({ "query": query, "state": state, "count": 0, "results": [] }),
            )));
        }
        if !state.get("ok").and_then(Value::as_bool).unwrap_or(false)
            && !state.get("empty").and_then(Value::as_bool).unwrap_or(false)
        {
            return Ok(json_result(&failure_payload(
                state
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("search_results_unavailable"),
                json!({ "query": query, "state": state, "count": 0, "results": [] }),
            )));
        }
        let results = if state.get("empty").and_then(Value::as_bool).unwrap_or(false) {
            Value::Array(Vec::new())
        } else {
            invoke_browser_tool(
                &self.page,
                ctx,
                SITE_ID,
                "searchResults",
                Some(&json!({ "limit": num })),
                true,
            )
            .await?
        };
        let count = results.as_array().map(Vec::len).unwrap_or(0);
        if preview {
            return Ok(json_result(&json!({
                "ok": true,
                "preview": true,
                "query": query,
                "url": current_url(&self.page).await.unwrap_or_default(),
                "count": count,
                "results": results,
                "state": state,
            })));
        }
        let mut posts = Vec::new();
        if let Some(cards) = results.as_array() {
            for card in cards {
                let kind = card.get("kind").and_then(Value::as_str).unwrap_or("");
                if !matches!(kind, "post" | "reel") {
                    posts.push(card.clone());
                    continue;
                }
                let id = card.get("id").and_then(Value::as_str).unwrap_or("");
                if id.is_empty() {
                    continue;
                }
                let card_video = card
                    .get("video_url")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                posts.push(
                    open_search_card(&self.page, ctx, id, num_comments, wait_seconds, &card_video)
                        .await?,
                );
            }
        }
        let failed = posts
            .iter()
            .any(|item| item.get("ok").and_then(Value::as_bool) == Some(false));
        let mut payload = json!({
            "query": query,
            "count": posts.len(),
            "posts": posts,
        });
        if failed {
            payload["ok"] = json!(false);
        }
        Ok(json_result(&payload))
    }
}

struct AccountsTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for AccountsTool {
    fn name(&self) -> &str {
        "search_accounts"
    }

    fn description(&self) -> &str {
        "Find Instagram accounts. Opens the homepage Search control, types the query, and returns the suggested accounts in dropdown order. Does not search posts."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "maxLength": 512 },
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
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        navigate_https(&self.page, HOME_URL).await?;
        let deadline = Instant::now() + Duration::from_secs_f64(wait_seconds.min(15.0));
        let mut opened = json!({ "ok": false, "status": "search_control_not_found" });
        loop {
            opened =
                invoke_browser_tool(&self.page, ctx, SITE_ID, "openSearch", None, false).await?;
            if gate_reason(&opened).is_some()
                || opened.get("already_open").and_then(Value::as_bool) == Some(true)
                || opened.get("x").and_then(Value::as_f64).is_some()
                || Instant::now() >= deadline
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        if let Some(reason) = gate_reason(&opened) {
            return Ok(json_result(&failure_payload(
                reason,
                json!({ "query": query, "open": opened, "count": 0, "accounts": [] }),
            )));
        }
        if opened.get("already_open").and_then(Value::as_bool) != Some(true)
            && opened.get("x").and_then(Value::as_f64).is_none()
        {
            return Ok(json_result(&failure_payload(
                opened
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("search_control_not_found"),
                json!({ "query": query, "open": opened, "count": 0, "accounts": [] }),
            )));
        }
        if opened.get("already_open").and_then(Value::as_bool) != Some(true) {
            let x = opened.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let y = opened.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            self.page.click(x, y).await?;
            let ready = wait_for_browser_tool(
                &self.page,
                SITE_ID,
                "openSearch",
                None,
                wait_seconds.min(10.0),
            )
            .await?;
            if ready.get("already_open").and_then(Value::as_bool) != Some(true) {
                return Ok(json_result(&failure_payload(
                    "search_input_not_found",
                    json!({ "query": query, "open": ready, "count": 0, "accounts": [] }),
                )));
            }
        }
        let search_args = json!({ "query": query });
        let typed = invoke_browser_tool(
            &self.page,
            ctx,
            SITE_ID,
            "setSearchQuery",
            Some(&search_args),
            false,
        )
        .await?;
        if typed.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(json_result(&failure_payload(
                typed
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("query_rejected"),
                json!({ "query": query, "typed": typed, "count": 0, "accounts": [] }),
            )));
        }
        let suggestions = wait_for_browser_tool(
            &self.page,
            SITE_ID,
            "accountSuggestions",
            Some(&search_args),
            wait_seconds,
        )
        .await?;
        if let Some(reason) = gate_reason(&suggestions) {
            return Ok(json_result(&failure_payload(
                reason,
                json!({ "query": query, "state": suggestions, "count": 0, "accounts": [] }),
            )));
        }
        if suggestions.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(json_result(&failure_payload(
                suggestions
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("account_suggestions_unavailable"),
                json!({ "query": query, "state": suggestions, "count": 0, "accounts": [] }),
            )));
        }
        let accounts = suggestions
            .get("accounts")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        Ok(json_result(&json!({
            "query": query,
            "accounts": compact_accounts(&accounts),
        })))
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
        "Read an Instagram profile by @handle or URL and collect visible post and reel cards."
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
        let url = instagram_profile_url(&locator)?;
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
        let declared = state.get("post_count").and_then(Value::as_i64).unwrap_or(0);
        let found = posts.as_array().map(Vec::len).unwrap_or(0);
        if declared > 0 && found == 0 {
            return Ok(json_result(&failure_payload(
                "profile_posts_unavailable",
                json!({ "profile": locator, "url": url }),
            )));
        }
        let mut payload = compact_profile(&state, &posts);
        if deep > 0 {
            let deep_posts =
                read_clicked_candidates(&self.page, ctx, &posts, deep, num_comments, wait_seconds)
                    .await?;
            let deep_status = deep_read_status(&posts, &deep_posts, deep);
            if deep_status.get("ok").and_then(Value::as_bool) != Some(true) {
                payload["ok"] = json!(false);
            }
            payload["deep_posts"] = deep_posts;
            payload["deep_status"] = deep_status;
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
        "Read one or more Instagram posts or Reels by URL or shortcode, including comments and playable video."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "posts": {
                    "type": "array",
                    "items": { "type": "string" }
                },
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
        for post in posts {
            let locator = post
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("each --post must be a non-empty URL or shortcode")
                })?;
            items.push(
                read_instagram_post(&self.page, ctx, locator, num_comments, wait_seconds).await?,
            );
        }
        Ok(json_result(&json!({
            "ok": items.iter().all(|item| item.get("ok").and_then(Value::as_bool) == Some(true)),
            "count": items.len(),
            "posts": items,
        })))
    }
}

struct PageStateTool {
    page: Arc<PageSession>,
}

struct CommentTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for CommentTool {
    fn name(&self) -> &str {
        "comment"
    }

    fn description(&self) -> &str {
        "Comment on an explicitly selected Instagram post or Reel with real CDP pointer and keyboard events. Refuses login gates, ambiguous editors, existing drafts or exact comments, route changes, and submit retries."
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
                "comment text must be non-empty and have no leading or trailing whitespace"
            );
        }
        let text = raw_text.to_string();
        if text.chars().count() > 10_000 {
            anyhow::bail!("comment text must contain at most 10000 characters");
        }
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        let url = instagram_post_url(&locator)?;
        let expected_shortcode = instagram_post_shortcode(&url)
            .ok_or_else(|| anyhow::anyhow!("canonical Instagram URL is missing a shortcode"))?;
        navigate_https(&self.page, &url).await?;
        let detail =
            wait_for_browser_tool(&self.page, SITE_ID, "postDetail", None, wait_seconds).await?;
        let page_state =
            crate::sites::learning::run_site_browser_tool(&self.page, SITE_ID, "pageState", None)
                .await?;
        if let Some(reason) = gate_reason(&page_state) {
            return Ok(json_result(&failure_payload(
                reason,
                json!({ "post": locator, "url": url, "detail": detail, "page_state": page_state, "submit_click_count": 0 }),
            )));
        }
        if page_state.get("ok").and_then(Value::as_bool) != Some(true)
            || page_state.get("authenticated").and_then(Value::as_bool) != Some(true)
        {
            return Ok(json_result(&failure_payload(
                "login_required",
                json!({ "post": locator, "url": url, "detail": detail, "page_state": page_state, "submit_click_count": 0 }),
            )));
        }
        let shortcode = detail
            .get("shortcode")
            .or_else(|| detail.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if detail.get("ok").and_then(Value::as_bool) != Some(true)
            || shortcode != expected_shortcode
        {
            return Ok(json_result(&failure_payload(
                if detail.get("ok").and_then(Value::as_bool) == Some(true) {
                    "wrong_post"
                } else {
                    detail
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("post_unavailable")
                },
                json!({ "post": locator, "expected_shortcode": expected_shortcode, "url": url, "detail": detail, "page_state": page_state, "submit_click_count": 0 }),
            )));
        }
        let action_args = json!({ "shortcode": shortcode });
        let rendered_args = json!({ "shortcode": shortcode, "text": text });
        let before = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "renderedCommentState",
            Some(&rendered_args),
        )
        .await?;
        let baseline = before.get("count").and_then(Value::as_u64).unwrap_or(0);
        if baseline > 0 {
            return Ok(json_result(&failure_payload(
                "exact_comment_preexists",
                json!({
                    "shortcode": shortcode,
                    "url": url,
                    "comment": text,
                    "submit_click_count": 0,
                    "reconcile": before,
                }),
            )));
        }

        let editor = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "commentEditorTarget",
            Some(&action_args),
        )
        .await?;
        let Some((editor_x, editor_y)) = verified_instagram_write_target(&editor, shortcode) else {
            return Ok(json_result(&failure_payload(
                editor
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("comment_editor_unavailable"),
                json!({ "shortcode": shortcode, "url": url, "editor": editor, "submit_click_count": 0 }),
            )));
        };
        self.page.click(editor_x, editor_y).await?;
        tokio::time::sleep(Duration::from_millis(150)).await;
        let draft = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "commentDraftState",
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
                "comment_editor_not_empty_or_focused",
                json!({ "shortcode": shortcode, "url": url, "draft": draft, "submit_click_count": 0 }),
            )));
        }
        self.page.type_chars(&text).await?;
        let typed_deadline = Instant::now() + Duration::from_secs(5);
        let typed = loop {
            let state = crate::sites::learning::run_site_browser_tool(
                &self.page,
                SITE_ID,
                "commentDraftState",
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
                "comment_draft_mismatch",
                json!({ "shortcode": shortcode, "url": url, "draft": typed, "submit_click_count": 0 }),
            )));
        }

        let final_page_state =
            crate::sites::learning::run_site_browser_tool(&self.page, SITE_ID, "pageState", None)
                .await?;
        if final_page_state.get("ok").and_then(Value::as_bool) != Some(true)
            || final_page_state
                .get("authenticated")
                .and_then(Value::as_bool)
                != Some(true)
        {
            return Ok(json_result(&failure_payload(
                gate_reason(&final_page_state).unwrap_or("page_gate_before_submit"),
                json!({ "shortcode": shortcode, "url": url, "page_state": final_page_state, "submit_click_count": 0 }),
            )));
        }
        let final_detail =
            crate::sites::learning::run_site_browser_tool(&self.page, SITE_ID, "postDetail", None)
                .await?;
        let final_draft = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "commentDraftState",
            Some(&action_args),
        )
        .await?;
        let submit = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "commentSubmitTarget",
            Some(&action_args),
        )
        .await?;
        let Some((submit_x, submit_y)) = verified_instagram_write_target(&submit, shortcode) else {
            return Ok(json_result(&failure_payload(
                submit
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("comment_submit_unavailable"),
                json!({ "shortcode": shortcode, "url": url, "submit": submit, "submit_click_count": 0 }),
            )));
        };
        if final_detail.get("ok").and_then(Value::as_bool) != Some(true)
            || final_detail
                .get("shortcode")
                .or_else(|| final_detail.get("id"))
                .and_then(Value::as_str)
                != Some(shortcode)
            || final_draft.get("value").and_then(Value::as_str) != Some(text.as_str())
        {
            return Ok(json_result(&failure_payload(
                "volatile_state_changed_before_submit",
                json!({ "shortcode": shortcode, "url": url, "page_state": final_page_state, "detail": final_detail, "draft": final_draft, "submit_click_count": 0 }),
            )));
        }

        let dispatch_error = self
            .page
            .click(submit_x, submit_y)
            .await
            .err()
            .map(|error| format!("{error:#}"));
        let deadline = Instant::now() + Duration::from_secs_f64(wait_seconds);
        let reconciled = loop {
            let state = crate::sites::learning::run_site_browser_tool(
                &self.page,
                SITE_ID,
                "renderedCommentState",
                Some(&rendered_args),
            )
            .await?;
            if state.get("count").and_then(Value::as_u64).unwrap_or(0) > baseline
                || Instant::now() >= deadline
            {
                break state;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        };
        let committed = reconciled.get("count").and_then(Value::as_u64).unwrap_or(0) > baseline;
        Ok(json_result(&json!({
            "ok": committed,
            "status": if committed { "committed" } else { "commit_unknown" },
            "shortcode": shortcode,
            "url": url,
            "comment": text,
            "interaction": "trusted_pointer_and_keyboard",
            "platform_api_called": false,
            "submit_click_count": 1,
            "dispatch_error": dispatch_error,
            "baseline_exact_comment_count": baseline,
            "reconcile": reconciled,
        })))
    }
}

fn verified_instagram_write_target(target: &Value, shortcode: &str) -> Option<(f64, f64)> {
    if target.get("ok").and_then(Value::as_bool) != Some(true)
        || target.get("hit_owned").and_then(Value::as_bool) != Some(true)
        || target.get("shortcode").and_then(Value::as_str) != Some(shortcode)
    {
        return None;
    }
    Some((target.get("x")?.as_f64()?, target.get("y")?.as_f64()?))
}

#[async_trait]
impl Tool for PageStateTool {
    fn name(&self) -> &str {
        "page_state"
    }

    fn description(&self) -> &str {
        "Open or reuse Instagram and return the current route, login gate, and hydration state."
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

async fn read_clicked_candidates(
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
        let kind = candidate.get("kind").and_then(Value::as_str).unwrap_or("");
        if !matches!(kind, "post" | "reel") {
            continue;
        }
        let Some(shortcode) = candidate
            .get("shortcode")
            .or_else(|| candidate.get("id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let result =
            match read_clicked_instagram_post(page, ctx, shortcode, num_comments, wait_seconds)
                .await
            {
                Ok(result) => result,
                Err(error) => failure_payload(
                    "deep_read_error",
                    json!({
                        "shortcode": shortcode,
                        "navigation_policy": "card_click_only",
                        "origin_preserved": true,
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

fn deep_read_status(candidates: &Value, deep_posts: &Value, deep: i64) -> Value {
    let available = candidates
        .as_array()
        .into_iter()
        .flatten()
        .filter(|candidate| {
            matches!(
                candidate.get("kind").and_then(Value::as_str),
                Some("post" | "reel")
            )
        })
        .count();
    let requested = (deep.max(0) as usize).min(available);
    let attempted = deep_posts.as_array().map(Vec::len).unwrap_or(0);
    let completed = deep_posts
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item.get("ok").and_then(Value::as_bool) == Some(true))
        .count();
    let ok = deep <= 0 || (attempted == requested && completed == requested);
    json!({
        "ok": ok,
        "requested": requested,
        "attempted": attempted,
        "completed": completed,
    })
}

async fn locate_instagram_post_card(page: &PageSession, args: &Value) -> anyhow::Result<Value> {
    let mut target =
        crate::sites::learning::run_site_browser_tool(page, SITE_ID, "postCardTarget", Some(args))
            .await?;
    if target.get("ok").and_then(Value::as_bool) == Some(true)
        || target.get("status").and_then(Value::as_str) != Some("post_card_not_found")
    {
        return Ok(target);
    }

    let mut active_scroll_tool = None;
    for tool_name in ["scrollResults", "scrollPosts"] {
        let scroll = crate::sites::learning::run_site_browser_tool(
            page,
            SITE_ID,
            tool_name,
            Some(&json!({ "to_top": true })),
        )
        .await?;
        if scroll.get("ok").and_then(Value::as_bool) == Some(true) {
            active_scroll_tool = Some(tool_name);
            break;
        }
    }
    let Some(scroll_tool) = active_scroll_tool else {
        return Ok(target);
    };

    tokio::time::sleep(Duration::from_millis(350)).await;
    for _ in 0..16 {
        target = crate::sites::learning::run_site_browser_tool(
            page,
            SITE_ID,
            "postCardTarget",
            Some(args),
        )
        .await?;
        if target.get("ok").and_then(Value::as_bool) == Some(true)
            || target.get("status").and_then(Value::as_str) != Some("post_card_not_found")
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
            Some(args),
        )
        .await?;
        if target.get("ok").and_then(Value::as_bool) == Some(true)
            || target.get("status").and_then(Value::as_str) != Some("post_card_not_found")
        {
            return Ok(target);
        }
        if scroll.get("at_end").and_then(Value::as_bool) == Some(true) {
            break;
        }
    }
    Ok(target)
}

fn validated_instagram_click_target(
    target: &Value,
    expected_shortcode: &str,
) -> anyhow::Result<(f64, f64)> {
    if target.get("ok").and_then(Value::as_bool) != Some(true)
        || target.get("hit_owned").and_then(Value::as_bool) != Some(true)
    {
        anyhow::bail!("Instagram post card click target is not owned by the expected anchor");
    }
    let actual_shortcode = target
        .get("shortcode")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let source_url = target
        .get("source_url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let validated_url = instagram_post_url(source_url)?;
    if actual_shortcode != expected_shortcode
        || instagram_post_shortcode(&validated_url).as_deref() != Some(expected_shortcode)
    {
        anyhow::bail!("Instagram post card identity changed before click");
    }
    let x = target
        .get("x")
        .and_then(Value::as_f64)
        .ok_or_else(|| anyhow::anyhow!("Instagram post card target is missing x"))?;
    let y = target
        .get("y")
        .and_then(Value::as_f64)
        .ok_or_else(|| anyhow::anyhow!("Instagram post card target is missing y"))?;
    Ok((x, y))
}

fn source_surface_restored(
    source_url: &str,
    source_state: &Value,
    current_url: &str,
    current_state: &Value,
) -> bool {
    if source_url != current_url
        || gate_reason(current_state).is_some()
        || current_state.get("ok").and_then(Value::as_bool) != Some(true)
        || current_state
            .get("login_gate_present")
            .and_then(Value::as_bool)
            == Some(true)
        || current_state.get("hydrated").and_then(Value::as_bool) != Some(true)
        || current_state
            .get("content_available")
            .and_then(Value::as_bool)
            != Some(true)
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
    if expected_type != actual_type {
        return false;
    }
    let expected_query = source_state
        .get("search_query")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let actual_query = current_state
        .get("search_query")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !expected_query.is_empty() && expected_query != actual_query {
        return false;
    }
    let expected_profile = source_state
        .get("profile_username")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let actual_profile = current_state
        .get("profile_username")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if expected_type == "profile"
        && (expected_profile.is_empty() || expected_profile != actual_profile)
    {
        return false;
    }
    let expected_results = source_state
        .get("result_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let actual_results = current_state
        .get("result_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    (expected_results == 0 && expected_type != "search") || actual_results > 0
}

async fn read_clicked_instagram_post(
    page: &PageSession,
    ctx: &ToolContext,
    shortcode: &str,
    num_comments: i64,
    wait_seconds: f64,
) -> anyhow::Result<Value> {
    let source_url = current_url(page).await.unwrap_or_default();
    let source_state =
        crate::sites::learning::run_site_browser_tool(page, SITE_ID, "pageState", None).await?;
    let args = json!({ "shortcode": shortcode });
    let initial = locate_instagram_post_card(page, &args).await?;
    if initial.get("ok").and_then(Value::as_bool) != Some(true) {
        return Ok(failure_payload(
            initial
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("post_card_not_found"),
            json!({
                "shortcode": shortcode,
                "navigation_policy": "card_click_only",
                "source_url": source_url,
                "origin_preserved": true,
                "open": initial,
            }),
        ));
    }

    tokio::time::sleep(Duration::from_millis(180)).await;
    let fresh =
        crate::sites::learning::run_site_browser_tool(page, SITE_ID, "postCardTarget", Some(&args))
            .await?;
    if fresh.get("ok").and_then(Value::as_bool) != Some(true) {
        return Ok(failure_payload(
            fresh
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("post_card_changed_before_click"),
            json!({
                "shortcode": shortcode,
                "navigation_policy": "card_click_only",
                "source_url": source_url,
                "origin_preserved": true,
                "initial_target": initial,
                "fresh_target": fresh,
            }),
        ));
    }
    if initial.get("source_url").and_then(Value::as_str)
        != fresh.get("source_url").and_then(Value::as_str)
    {
        return Ok(failure_payload(
            "post_card_changed_before_click",
            json!({
                "shortcode": shortcode,
                "navigation_policy": "card_click_only",
                "source_url": source_url,
                "origin_preserved": true,
                "initial_target": initial,
                "fresh_target": fresh,
            }),
        ));
    }
    let (x, y) = validated_instagram_click_target(&fresh, shortcode)?;
    if let Err(error) = page.click(x, y).await {
        let close = close_clicked_instagram_post(page, shortcode, &source_url, &source_state)
            .await
            .unwrap_or_else(
                |close_error| json!({ "ok": false, "error": format!("{close_error:#}") }),
            );
        return Ok(failure_payload(
            "post_click_error",
            json!({
                "shortcode": shortcode,
                "navigation_policy": "card_click_only",
                "source_url": source_url,
                "error": format!("{error:#}"),
                "close": close,
            }),
        ));
    }

    let deadline = Instant::now() + Duration::from_secs_f64(wait_seconds.clamp(1.0, 330.0));
    let open_result = async {
        let mut open = json!({ "ok": false, "status": "waiting" });
        while Instant::now() < deadline {
            open = crate::sites::learning::run_site_browser_tool(
                page,
                SITE_ID,
                "postOpenState",
                Some(&args),
            )
            .await?;
            if open.get("ok").and_then(Value::as_bool) == Some(true)
                || gate_reason(&open).is_some()
                || matches!(
                    open.get("status").and_then(Value::as_str),
                    Some("wrong_post" | "full_page_navigation")
                )
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Ok::<_, anyhow::Error>(open)
    }
    .await;
    let open = match open_result {
        Ok(open) => open,
        Err(error) => {
            let close = close_clicked_instagram_post(page, shortcode, &source_url, &source_state)
                .await
                .unwrap_or_else(
                    |close_error| json!({ "ok": false, "error": format!("{close_error:#}") }),
                );
            return Ok(failure_payload(
                "post_open_state_error",
                json!({
                    "shortcode": shortcode,
                    "navigation_policy": "card_click_only",
                    "source_url": source_url,
                    "error": format!("{error:#}"),
                    "close": close,
                }),
            ));
        }
    };

    if open.get("ok").and_then(Value::as_bool) != Some(true) {
        let close = close_clicked_instagram_post(page, shortcode, &source_url, &source_state).await;
        let reason = gate_reason(&open).unwrap_or_else(|| {
            open.get("status")
                .and_then(Value::as_str)
                .unwrap_or("post_click_failed")
        });
        return Ok(failure_payload(
            reason,
            json!({
                "shortcode": shortcode,
                "navigation_policy": "card_click_only",
                "source_url": source_url,
                "open": open,
                "close": close.unwrap_or_else(|error| json!({ "ok": false, "error": format!("{error:#}") })),
            }),
        ));
    }

    let read = async {
        let entity = invoke_browser_tool(page, ctx, SITE_ID, "postDetail", None, false).await?;
        let comments = if num_comments > 0 {
            invoke_browser_tool(
                page,
                ctx,
                SITE_ID,
                "comments",
                Some(&json!({ "limit": num_comments })),
                true,
            )
            .await?
        } else {
            Value::Array(Vec::new())
        };
        Ok::<_, anyhow::Error>((entity, comments))
    }
    .await;
    let close = close_clicked_instagram_post(page, shortcode, &source_url, &source_state)
        .await
        .unwrap_or_else(|error| json!({ "ok": false, "error": format!("{error:#}") }));

    match read {
        Ok((entity, comments)) => {
            let entity_ok = entity.get("ok").and_then(Value::as_bool) == Some(true);
            Ok(json!({
                "ok": entity_ok && close.get("ok").and_then(Value::as_bool) == Some(true),
                "shortcode": shortcode,
                "navigation_policy": "card_click_only",
                "source_url": source_url,
                "open_strategy": "trusted_cdp_card_click",
                "entity": entity,
                "comments": comments,
                "close": close,
            }))
        }
        Err(error) => Ok(failure_payload(
            "post_read_failed",
            json!({
                "shortcode": shortcode,
                "navigation_policy": "card_click_only",
                "source_url": source_url,
                "error": format!("{error:#}"),
                "close": close,
            }),
        )),
    }
}

async fn close_clicked_instagram_post(
    page: &PageSession,
    shortcode: &str,
    source_url: &str,
    source_state: &Value,
) -> anyhow::Result<Value> {
    let before_close = current_url(page).await.unwrap_or_default();
    let before_state =
        crate::sites::learning::run_site_browser_tool(page, SITE_ID, "pageState", None).await?;
    if source_surface_restored(source_url, source_state, &before_close, &before_state) {
        return Ok(
            json!({ "ok": true, "strategy": "already_restored", "url": before_close, "state": before_state }),
        );
    }

    page.press_key("Escape").await?;
    tokio::time::sleep(Duration::from_millis(350)).await;
    let after_escape = current_url(page).await.unwrap_or_default();
    let after_escape_state =
        crate::sites::learning::run_site_browser_tool(page, SITE_ID, "pageState", None).await?;
    if source_surface_restored(source_url, source_state, &after_escape, &after_escape_state) {
        return Ok(
            json!({ "ok": true, "strategy": "escape", "url": after_escape, "state": after_escape_state }),
        );
    }

    let close =
        crate::sites::learning::run_site_browser_tool(page, SITE_ID, "closePostTarget", None)
            .await?;
    if close.get("ok").and_then(Value::as_bool) == Some(true)
        && close.get("hit_owned").and_then(Value::as_bool) == Some(true)
        && close.get("shortcode").and_then(Value::as_str) == Some(shortcode)
    {
        let x = close
            .get("x")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow::anyhow!("Instagram close target is missing x"))?;
        let y = close
            .get("y")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow::anyhow!("Instagram close target is missing y"))?;
        page.click(x, y).await?;
        tokio::time::sleep(Duration::from_millis(350)).await;
        let after_close = current_url(page).await.unwrap_or_default();
        let after_close_state =
            crate::sites::learning::run_site_browser_tool(page, SITE_ID, "pageState", None).await?;
        if source_surface_restored(source_url, source_state, &after_close, &after_close_state) {
            return Ok(
                json!({ "ok": true, "strategy": "close_button", "url": after_close, "state": after_close_state }),
            );
        }
    }

    let state = crate::sites::learning::run_site_browser_tool(
        page,
        SITE_ID,
        "postOpenState",
        Some(&json!({ "shortcode": shortcode })),
    )
    .await?;
    Ok(json!({
        "ok": false,
        "strategy": "close_failed",
        "source_url": source_url,
        "url": current_url(page).await.unwrap_or_default(),
        "state": state,
        "reason": "originating_list_not_restored",
    }))
}

async fn open_search_card(
    page: &PageSession,
    ctx: &ToolContext,
    id: &str,
    num_comments: i64,
    wait_seconds: f64,
    card_video_url: &str,
) -> anyhow::Result<Value> {
    let click = invoke_browser_tool(
        page,
        ctx,
        SITE_ID,
        "clickResult",
        Some(&json!({ "id": id })),
        false,
    )
    .await?;
    if click.get("ok").and_then(Value::as_bool) != Some(true) {
        return Ok(failure_payload(
            click
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("card_not_found"),
            json!({ "input": id, "click": click }),
        ));
    }
    let x = click.get("x").and_then(Value::as_f64).unwrap_or(0.0);
    let y = click.get("y").and_then(Value::as_f64).unwrap_or(0.0);
    page.click(x, y).await?;
    let detail = wait_for_browser_tool(page, SITE_ID, "postDetail", None, wait_seconds).await?;
    if let Some(reason) = gate_reason(&detail) {
        let _ = close_search_overlay(page, ctx).await;
        return Ok(failure_payload(
            reason,
            json!({ "input": id, "entity": detail }),
        ));
    }
    if detail.get("ok").and_then(Value::as_bool) != Some(true) {
        let _ = close_search_overlay(page, ctx).await;
        return Ok(failure_payload(
            detail
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("post_unavailable"),
            json!({ "input": id, "entity": detail }),
        ));
    }
    let entity = wait_for_post_media(page, ctx, wait_seconds, card_video_url).await?;
    let comments = if num_comments > 0 {
        wait_for_overlay_comments(page, ctx, num_comments, wait_seconds).await?
    } else {
        Value::Array(Vec::new())
    };
    let _ = close_search_overlay(page, ctx).await;
    Ok(compact_opened_post(id, &entity, &comments, card_video_url))
}

/// Caption can be ready while likes, the playable video URL, and comments are
/// still mounting. Closing or reading at the first non-empty caption records
/// those regions as empty.
async fn wait_for_post_media(
    page: &PageSession,
    ctx: &ToolContext,
    wait_seconds: f64,
    card_video_url: &str,
) -> anyhow::Result<Value> {
    let deadline = Instant::now() + Duration::from_secs_f64(wait_seconds.clamp(1.0, 12.0));
    let mut latest = invoke_browser_tool(page, ctx, SITE_ID, "postDetail", None, false).await?;
    while Instant::now() < deadline {
        let overlay_video = latest
            .get("video_url")
            .and_then(Value::as_str)
            .unwrap_or("");
        let video_url = if overlay_video.is_empty() {
            card_video_url
        } else {
            overlay_video
        };
        let has_video = latest.get("kind").and_then(Value::as_str) == Some("reel");
        let author_ready = latest
            .pointer("/author/username")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty());
        let date_ready = latest
            .get("published_at")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty());
        if (!has_video || !video_url.is_empty()) && author_ready && date_ready {
            if latest
                .pointer("/engagement/likes")
                .and_then(Value::as_i64)
                .is_some()
                || latest
                    .pointer("/engagement/provenance/likes/source")
                    .and_then(Value::as_str)
                    == Some("hidden")
                || !has_video
            {
                return Ok(latest);
            }
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
        latest = invoke_browser_tool(page, ctx, SITE_ID, "postDetail", None, false).await?;
    }
    Ok(latest)
}

async fn wait_for_overlay_comments(
    page: &PageSession,
    ctx: &ToolContext,
    num_comments: i64,
    wait_seconds: f64,
) -> anyhow::Result<Value> {
    let deadline = Instant::now() + Duration::from_secs_f64(wait_seconds.clamp(1.0, 12.0));
    loop {
        let state = invoke_browser_tool(page, ctx, SITE_ID, "commentState", None, false).await?;
        let count = state.get("count").and_then(Value::as_i64).unwrap_or(0);
        let empty = state.get("empty").and_then(Value::as_bool).unwrap_or(false);
        if count > 0 || empty || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    invoke_browser_tool(
        page,
        ctx,
        SITE_ID,
        "comments",
        Some(&json!({ "limit": num_comments })),
        true,
    )
    .await
}

fn compact_accounts(accounts: &Value) -> Value {
    let Some(items) = accounts.as_array() else {
        return Value::Array(Vec::new());
    };
    Value::Array(
        items
            .iter()
            .map(|account| {
                let mut item = json!({
                    "username": account.get("username").and_then(Value::as_str).unwrap_or(""),
                    "name": account.get("name").and_then(Value::as_str).unwrap_or(""),
                    "url": account.get("url").and_then(Value::as_str).unwrap_or(""),
                });
                if let Some(subtitle) = account
                    .get("subtitle")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    item["subtitle"] = json!(subtitle);
                }
                if let Some(avatar) = account
                    .get("avatar_url")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    item["avatar_url"] = json!(avatar);
                }
                item
            })
            .collect(),
    )
}

fn compact_profile(state: &Value, posts: &Value) -> Value {
    let mut profile = json!({
        "username": state.get("username").and_then(Value::as_str).unwrap_or(""),
        "display_name": state.get("display_name").and_then(Value::as_str).unwrap_or(""),
        "url": state.get("url").and_then(Value::as_str).unwrap_or(""),
        "bio": state.get("bio").and_then(Value::as_str).unwrap_or(""),
        "avatar_url": state.get("avatar_url").and_then(Value::as_str).unwrap_or(""),
        "posts": compact_profile_posts(posts),
    });
    if let Some(url) = state
        .get("external_url")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        profile["external_url"] = json!(url);
    }
    for key in ["followers", "following", "post_count"] {
        if let Some(value) = state.get(key).and_then(Value::as_i64) {
            profile[key] = json!(value);
        }
    }
    profile
}

fn compact_profile_posts(posts: &Value) -> Value {
    let Some(items) = posts.as_array() else {
        return Value::Array(Vec::new());
    };
    Value::Array(
        items
            .iter()
            .map(|post| {
                let mut item = json!({
                    "id": post.get("id").and_then(Value::as_str).unwrap_or(""),
                    "kind": post.get("kind").and_then(Value::as_str).unwrap_or("post"),
                    "url": post.get("url").and_then(Value::as_str).unwrap_or(""),
                });
                if let Some(thumb) = post
                    .get("thumbnail_url")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    item["thumbnail_url"] = json!(thumb);
                }
                item
            })
            .collect(),
    )
}

fn compact_opened_post(id: &str, entity: &Value, comments: &Value, card_video_url: &str) -> Value {
    if entity.get("ok").and_then(Value::as_bool) == Some(false) {
        return json!({
            "ok": false,
            "id": id,
            "reason": entity.get("status").and_then(Value::as_str).unwrap_or("post_unavailable"),
        });
    }
    let mut post = json!({
        "ok": true,
        "id": entity.get("id").and_then(Value::as_str).unwrap_or(id),
        "kind": entity.get("kind").and_then(Value::as_str).unwrap_or("post"),
        "url": entity.get("url").and_then(Value::as_str).unwrap_or(""),
        "author": entity.pointer("/author/username").and_then(Value::as_str).unwrap_or(""),
        "caption": entity.get("caption").and_then(Value::as_str).unwrap_or(""),
        "published_at": entity.get("published_at").and_then(Value::as_str).unwrap_or(""),
        "comments": compact_comments(comments),
        "likes": entity.pointer("/engagement/likes").cloned().unwrap_or(Value::Null),
        "comment_count": entity.pointer("/engagement/comments").cloned().unwrap_or(Value::Null),
        "comment_count_source": entity.pointer("/engagement/provenance/comments/source").and_then(Value::as_str).unwrap_or("unavailable"),
        "comment_count_approximate": entity.pointer("/engagement/provenance/comments/approximate").and_then(Value::as_bool).unwrap_or(false),
        "likes_source": entity.pointer("/engagement/provenance/likes/source").and_then(Value::as_str).unwrap_or("unavailable"),
        "likes_approximate": entity.pointer("/engagement/provenance/likes/approximate").and_then(Value::as_bool).unwrap_or(false),
        "complete": entity.get("complete").and_then(Value::as_bool).unwrap_or(false),
        "missing_fields": entity.get("missing_fields").cloned().unwrap_or_else(|| json!([])),
    });
    if let Some(media) = entity
        .get("media")
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
    {
        post["media"] = json!(media);
    }
    let video_url = entity
        .get("video_url")
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty())
        .unwrap_or(card_video_url);
    if !video_url.is_empty() {
        post["video_url"] = json!(video_url);
    }
    post
}

fn compact_comments(comments: &Value) -> Value {
    let Some(items) = comments.as_array() else {
        return Value::Array(Vec::new());
    };
    Value::Array(items.iter().map(compact_comment).collect())
}

fn compact_comment(comment: &Value) -> Value {
    let replies = comment
        .get("replies")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let mut item = json!({
        "text": comment_body(comment.get("text").and_then(Value::as_str).unwrap_or("")),
        "replies": replies,
        "author": comment.pointer("/author/username").and_then(Value::as_str).unwrap_or(""),
        "url": comment.get("url").and_then(Value::as_str).unwrap_or(""),
    });
    if let Some(likes) = comment.get("likes").and_then(Value::as_i64) {
        item["likes"] = json!(likes);
    }
    item
}

fn comment_body(raw: &str) -> String {
    let trimmed = raw.trim();
    let Some((head, tail)) = trimmed.rsplit_once('\n') else {
        return trimmed.to_string();
    };
    let chrome: String = tail.chars().filter(|c| !c.is_whitespace()).collect();
    let chrome = chrome.to_ascii_lowercase();
    if chrome.contains("reply") {
        return head.trim().to_string();
    }
    trimmed.to_string()
}

async fn close_search_overlay(page: &PageSession, ctx: &ToolContext) -> anyhow::Result<()> {
    let close = invoke_browser_tool(page, ctx, SITE_ID, "closeOverlay", None, false).await?;
    if close.get("ok").and_then(Value::as_bool) == Some(true) {
        let x = close.get("x").and_then(Value::as_f64).unwrap_or(0.0);
        let y = close.get("y").and_then(Value::as_f64).unwrap_or(0.0);
        page.click(x, y).await?;
    } else {
        page.press_key("Escape").await?;
    }
    let _ = wait_for_browser_tool(page, SITE_ID, "searchState", None, 8.0).await;
    Ok(())
}

async fn read_instagram_post(
    page: &PageSession,
    ctx: &ToolContext,
    locator: &str,
    num_comments: i64,
    wait_seconds: f64,
) -> anyhow::Result<Value> {
    let url = instagram_post_url(locator)?;
    navigate_https(page, &url).await?;
    let detail = wait_for_browser_tool(page, SITE_ID, "postDetail", None, wait_seconds).await?;
    if let Some(reason) = gate_reason(&detail) {
        return Ok(failure_payload(
            reason,
            json!({ "input": locator, "url": url, "entity": detail }),
        ));
    }
    if detail.get("ok").and_then(Value::as_bool) != Some(true) {
        return Ok(failure_payload(
            detail
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("post_unavailable"),
            json!({ "input": locator, "url": url, "entity": detail }),
        ));
    }
    let entity = wait_for_post_media(page, ctx, wait_seconds, "").await?;
    let comments = if num_comments > 0 {
        invoke_browser_tool(
            page,
            ctx,
            SITE_ID,
            "comments",
            Some(&json!({ "limit": num_comments })),
            true,
        )
        .await?
    } else {
        Value::Array(Vec::new())
    };
    Ok(compact_opened_post(locator, &entity, &comments, ""))
}

fn instagram_profile_url(locator: &str) -> anyhow::Result<String> {
    let trimmed = locator.trim();
    if let Ok(mut url) = reqwest::Url::parse(trimmed) {
        validate_instagram_origin(&url)?;
        let parts = url
            .path_segments()
            .into_iter()
            .flatten()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        if parts.len() != 1 || !valid_instagram_username(parts[0]) {
            anyhow::bail!("Instagram profile URL must identify exactly one profile");
        }
        url.set_query(None);
        url.set_fragment(None);
        return Ok(url.to_string());
    }
    let username = trimmed.trim_start_matches('@').to_ascii_lowercase();
    if !valid_instagram_username(&username) {
        anyhow::bail!("invalid Instagram username or profile URL: {locator}");
    }
    Ok(format!("https://www.instagram.com/{username}/"))
}

fn instagram_post_url(locator: &str) -> anyhow::Result<String> {
    let trimmed = locator.trim();
    if let Ok(mut url) = reqwest::Url::parse(trimmed) {
        validate_instagram_origin(&url)?;
        if instagram_post_shortcode(url.as_str()).is_none() {
            anyhow::bail!("Instagram post URL must identify a post or Reel");
        }
        url.set_query(None);
        url.set_fragment(None);
        return Ok(url.to_string());
    }
    if !trimmed
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        || trimmed.len() < 5
    {
        anyhow::bail!("invalid Instagram post URL or shortcode: {locator}");
    }
    Ok(format!("https://www.instagram.com/p/{trimmed}/"))
}
fn validate_instagram_origin(url: &reqwest::Url) -> anyhow::Result<()> {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if url.scheme() != "https"
        || !(host == "instagram.com" || host.ends_with(".instagram.com"))
        || !url.username().is_empty()
        || url.password().is_some()
    {
        anyhow::bail!("Instagram URL must use HTTPS on instagram.com without credentials");
    }
    Ok(())
}

fn valid_instagram_username(username: &str) -> bool {
    !username.is_empty()
        && username
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_'))
        && !RESERVED_PROFILE_NAMES.contains(&username.to_ascii_lowercase().as_str())
}

fn instagram_post_shortcode(raw_url: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw_url).ok()?;
    let parts = url
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let shortcode = match parts.as_slice() {
        [kind, shortcode] if matches!(*kind, "p" | "reel") => *shortcode,
        [_owner, kind, shortcode] if matches!(*kind, "p" | "reel") => *shortcode,
        _ => return None,
    };
    if shortcode.len() < 5
        || !shortcode
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return None;
    }
    Some(shortcode.to_string())
}
