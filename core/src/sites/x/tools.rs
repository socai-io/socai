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
use crate::sites::runner::{get_bool, get_f64, get_i64, json_result, ToolCommand};
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
        Arc::new(HomeTool { page: page.clone() }),
        Arc::new(SearchTool { page: page.clone() }),
        Arc::new(ProfileTool { page: page.clone() }),
        Arc::new(GetPostsTool { page: page.clone() }),
        Arc::new(ReplyTool { page: page.clone() }),
        Arc::new(LikeTool { page: page.clone() }),
        Arc::new(FollowTool { page: page.clone() }),
        Arc::new(HoverTool { page: page.clone() }),
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
            about: "Search X. filter selects Top, Latest, People, Media, or Lists. Top is the default.",
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
                    key: "filter",
                    long: Some("filter"),
                    value_name: "TAB",
                    help: "Result tab: top, latest, people, media, or lists. Defaults to top.",
                    required: false,
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
            name: "home",
            tool_name: "home",
            about: "Read the X home timeline. tab selects For you, Following, or another tab visible on the account. For you is the default.",
            args: &[
                CommandArg {
                    key: "tab",
                    long: Some("tab"),
                    value_name: "TAB",
                    help: "Home tab label: for you, following, or another visible tab such as News. Defaults to for you.",
                    required: false,
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
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for the timeline to hydrate. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_home,
        },
        SiteCommand {
            name: "profile",
            tool_name: "profile",
            about: "Read an X profile and collect the selected timeline. Posts is the default; replies, reposts, media, highlights, articles, and likes are the other tabs.",
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
                    key: "tab",
                    long: Some("tab"),
                    value_name: "TAB",
                    help: "Timeline tab: posts, replies, reposts, media, highlights, articles, or likes. Defaults to posts, or the tab in a profile URL.",
                    required: false,
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
            about: "Reply to one X post using verified pointer and keyboard events. --inline clicks the reply icon on a post already visible in the home, search, or profile timeline, then returns to that timeline.",
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
                    key: "inline",
                    long: Some("inline"),
                    value_name: "",
                    help: "Reply from the timeline reply icon. Does not open the post URL. The post must be in the current home, search, or profile timeline.",
                    required: false,
                    kind: ArgKind::Flag,
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
            name: "like",
            tool_name: "like",
            about: "Like one X post. Does nothing when that post is already liked.",
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
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for hydration and post-click reconciliation. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_like,
        },
        SiteCommand {
            name: "follow",
            tool_name: "follow",
            about: "Follow one X account from its profile header. --hover uses the card that appears over a name in the current timeline, then moves the pointer away. Does nothing when that account is already followed.",
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
                    key: "hover",
                    long: Some("hover"),
                    value_name: "",
                    help: "Follow from the hover card on a name in the current timeline, then move the pointer away. Does not open the profile URL.",
                    required: false,
                    kind: ArgKind::Flag,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for hydration and post-click reconciliation. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_follow,
        },
        SiteCommand {
            name: "hover",
            tool_name: "hover",
            about: "Hover a person's name in the current home, search, or profile timeline, read the card, then move the pointer away so the card closes.",
            args: &[
                CommandArg {
                    key: "profile",
                    long: None,
                    value_name: "HANDLE",
                    help: "X username visible in the current timeline.",
                    required: true,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait to find the name and the hover card. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_hover,
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

fn run_home(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    run_named(page, args, debug_snapshot, progress, "home", "home")
}

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

fn run_like(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    run_named(page, args, debug_snapshot, progress, "like", "like")
}

fn run_hover(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    run_named(page, args, debug_snapshot, progress, "hover", "hover")
}

fn run_follow(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    run_named(page, args, debug_snapshot, progress, "follow", "follow")
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

struct HomeTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for HomeTool {
    fn name(&self) -> &str {
        "home"
    }

    fn description(&self) -> &str {
        "Read the X home timeline. `tab` is the visible label, such as For you, Following, or another tab on the account. For you is the default. This tool is read-only."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "tab": { "type": "string", "default": "for you" },
                "num": { "type": "integer", "default": 10, "minimum": 1, "maximum": 100 },
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1, "maximum": 330 }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let tab = x_home_tab(
            input
                .get("tab")
                .and_then(Value::as_str)
                .unwrap_or("for you"),
        )?;
        let num = get_i64(&input, "num", DEFAULT_RESULT_COUNT).clamp(1, MAX_TOOL_ITEMS);
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        navigate_https(&self.page, HOME_URL).await?;
        let opened = wait_for_browser_tool(
            &self.page,
            SITE_ID,
            "feedState",
            Some(&json!({ "tab": "" })),
            wait_seconds,
        )
        .await?;
        if let Some(reason) = gate_reason(&opened) {
            return Ok(json_result(&failure_payload(
                reason,
                json!({ "tab": tab, "state": opened, "count": 0, "results": [] }),
            )));
        }
        if opened.get("ok").and_then(Value::as_bool) != Some(true)
            && opened.get("status").and_then(Value::as_str) != Some("tab_mismatch")
        {
            return Ok(json_result(&failure_payload(
                opened
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("home_unavailable"),
                json!({ "tab": tab, "state": opened, "count": 0, "results": [] }),
            )));
        }
        let target = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "feedTabTarget",
            Some(&json!({ "tab": tab })),
        )
        .await?;
        if target.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(json_result(&failure_payload(
                target
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("home_tab_unavailable"),
                json!({ "tab": tab, "state": target, "count": 0, "results": [] }),
            )));
        }
        let selected_label = target
            .get("tab")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if target.get("selected").and_then(Value::as_bool) != Some(true) {
            let previous = opened
                .get("first_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let Some((x, y)) = point_of(&target) else {
                return Ok(json_result(&failure_payload(
                    "home_tab_unavailable",
                    json!({ "tab": tab, "state": target, "count": 0, "results": [] }),
                )));
            };
            self.page.click(x, y).await?;
            let settled = wait_for_home_tab(&self.page, &tab, &previous, wait_seconds).await?;
            if settled.get("ok").and_then(Value::as_bool) != Some(true) {
                return Ok(json_result(&failure_payload(
                    settled
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("home_tab_not_settled"),
                    json!({ "tab": tab, "state": settled, "count": 0, "results": [] }),
                )));
            }
        }
        let tab_args = json!({ "tab": tab });
        let state = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "feedState",
            Some(&tab_args),
        )
        .await?;
        if state.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(json_result(&failure_payload(
                state
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("home_tab_mismatch"),
                json!({ "tab": selected_label, "state": state, "count": 0, "results": [] }),
            )));
        }
        let results = invoke_browser_tool(
            &self.page,
            ctx,
            SITE_ID,
            "feedPosts",
            Some(&json!({ "limit": num })),
            true,
        )
        .await?;
        if results.as_array().is_none_or(|items| items.is_empty()) {
            return Ok(json_result(&failure_payload(
                "home_results_unparsed",
                json!({ "tab": state.get("tab").cloned().unwrap_or(json!(tab)), "state": state, "count": 0, "results": results }),
            )));
        }
        let final_state = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "feedState",
            Some(&tab_args),
        )
        .await?;
        if final_state.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(json_result(&failure_payload(
                final_state
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("page_unavailable_after_scroll"),
                json!({
                    "tab": final_state.get("tab").cloned().unwrap_or(json!(tab)),
                    "state": final_state,
                    "count": results.as_array().map(Vec::len).unwrap_or(0),
                    "results": results,
                    "partial": true,
                }),
            )));
        }
        Ok(json_result(&json!({
            "ok": true,
            "tab": final_state.get("tab").cloned().unwrap_or(json!(selected_label)),
            "tabs": final_state.get("tabs").cloned().unwrap_or(json!([])),
            "url": current_url(&self.page).await.unwrap_or_default(),
            "count": results.as_array().map(Vec::len).unwrap_or(0),
            "results": results,
            "state": final_state,
        })))
    }
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
        "Search X. `filter` is top, latest, people, media, or lists. Top is the default. This tool is read-only."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "maxLength": 512 },
                "filter": { "type": "string", "enum": ["top", "latest", "people", "media", "lists"], "default": "top" },
                "num": { "type": "integer", "default": 10, "minimum": 1, "maximum": 100 },
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
        let filter = x_search_filter(input.get("filter").and_then(Value::as_str).unwrap_or("top"))?;
        let num = get_i64(&input, "num", DEFAULT_RESULT_COUNT).clamp(1, MAX_TOOL_ITEMS);
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        let target = x_search_url(&query, filter);
        navigate_https(&self.page, &target).await?;
        let search_args = json!({ "query": query, "filter": filter });
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
                json!({ "query": query, "filter": filter, "state": state, "count": 0, "results": [] }),
            )));
        }
        if state.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(json_result(&failure_payload(
                state
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("search_results_unavailable"),
                json!({ "query": query, "filter": filter, "state": state, "count": 0, "results": [] }),
            )));
        }
        let collector = match filter {
            "people" => "searchPeople",
            "lists" => "searchLists",
            "media" => "searchMedia",
            _ => "searchResults",
        };
        let results = invoke_browser_tool(
            &self.page,
            ctx,
            SITE_ID,
            collector,
            Some(&json!({ "limit": num })),
            true,
        )
        .await?;
        if results.as_array().is_none_or(|items| items.is_empty()) {
            return Ok(json_result(&failure_payload(
                "search_results_unparsed",
                json!({
                    "query": query,
                    "filter": filter,
                    "state": state,
                    "count": 0,
                    "results": results,
                }),
            )));
        }
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
                    "filter": filter,
                    "state": final_state,
                    "count": results.as_array().map(Vec::len).unwrap_or(0),
                    "results": results,
                    "partial": true,
                }),
            )));
        }
        Ok(json_result(&json!({
            "ok": true,
            "query": query,
            "filter": filter,
            "url": current_url(&self.page).await.unwrap_or_default(),
            "count": results.as_array().map(Vec::len).unwrap_or(0),
            "results": results,
            "state": final_state,
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
        "Read an X profile and one timeline tab. `tab` is posts, replies, reposts, media, highlights, articles, or likes. Posts is the default."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "profile": { "type": "string" },
                "tab": { "type": "string", "enum": ["posts", "replies", "reposts", "media", "highlights", "articles", "likes"], "default": "posts" },
                "num": { "type": "integer", "default": 10, "minimum": 1, "maximum": 100 },
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1, "maximum": 330 }
            },
            "required": ["profile"]
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let locator = required_string(&input, "profile")?;
        let parsed = parse_x_profile(&locator)?;
        let tab = match input.get("tab").and_then(Value::as_str) {
            Some(value) => x_profile_tab(value)?,
            None => parsed.tab.unwrap_or("posts"),
        };
        let url = x_profile_tab_url(&parsed.username, tab);
        let num = get_i64(&input, "num", DEFAULT_RESULT_COUNT).clamp(1, MAX_TOOL_ITEMS);
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
        let observed_tab = state.get("tab").and_then(Value::as_str).unwrap_or_default();
        let observed_username = state
            .get("username")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if state.get("ok").and_then(Value::as_bool) != Some(true)
            || observed_username != parsed.username
            || observed_tab != tab
        {
            return Ok(json_result(&failure_payload(
                if state.get("ok").and_then(Value::as_bool) != Some(true) {
                    state
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("profile_unavailable")
                } else if observed_username != parsed.username {
                    "wrong_profile"
                } else {
                    "profile_tab_mismatch"
                },
                json!({ "profile": locator, "tab": tab, "url": url, "state": state }),
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
        let final_tab = final_state
            .get("tab")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if final_state.get("ok").and_then(Value::as_bool) != Some(true)
            || expected_username.is_empty()
            || final_username != expected_username
            || final_tab != tab
        {
            let reason = if final_state.get("ok").and_then(Value::as_bool) == Some(true)
                && final_username == expected_username
                && final_tab != tab
            {
                "profile_tab_mismatch"
            } else if final_state.get("ok").and_then(Value::as_bool) == Some(true) {
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
        Ok(json_result(&json!({
            "ok": true,
            "tab": tab,
            "profile": state,
            "posts": posts,
            "count": posts.as_array().map(Vec::len).unwrap_or(0),
        })))
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

struct LikeTool {
    page: Arc<PageSession>,
}

struct FollowTool {
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
                "inline": { "type": "boolean", "default": false },
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
        if get_bool(&input, "inline", false) {
            return reply_from_timeline(&self.page, &locator, &text, &input).await;
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

        // The inline composer mounts after the post article. A single read
        // races that and reports reply_editor_not_found on a page that is fine.
        let editor = wait_for_browser_tool(
            &self.page,
            SITE_ID,
            "replyEditorTarget",
            Some(&action_args),
            8.0,
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
        // Draft.js renders an empty block as a single newline. That is an
        // empty composer, not a draft the user already started. If this same
        // reply is already in the box from a submit that never clicked, keep
        // it and continue instead of typing it a second time.
        let draft_text = draft
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let already_typed = draft_text == text;
        if draft.get("ok").and_then(Value::as_bool) != Some(true)
            || draft.get("focused").and_then(Value::as_bool) != Some(true)
            || (!draft_text.is_empty() && !already_typed)
        {
            return Ok(json_result(&failure_payload(
                "reply_editor_not_empty_or_focused",
                json!({ "post_id": post_id, "url": url, "draft": draft, "submit_click_count": 0 }),
            )));
        }
        if !already_typed {
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
                if state.get("value").and_then(Value::as_str).map(str::trim) == Some(text.as_str())
                    || Instant::now() >= typed_deadline
                {
                    break state;
                }
                tokio::time::sleep(Duration::from_millis(150)).await;
            };
            if typed.get("value").and_then(Value::as_str).map(str::trim) != Some(text.as_str()) {
                return Ok(json_result(&failure_payload(
                    "reply_draft_mismatch",
                    json!({ "post_id": post_id, "url": url, "draft": typed, "submit_click_count": 0 }),
                )));
            }
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
            || final_draft
                .get("value")
                .and_then(Value::as_str)
                .map(str::trim)
                != Some(text.as_str())
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

#[async_trait]
impl Tool for LikeTool {
    fn name(&self) -> &str {
        "like"
    }

    fn description(&self) -> &str {
        "Like one X post only when the user explicitly requests that post. Uses one real CDP pointer click on the post's own Like control. If it is already liked, do not click. Treat commit_unknown as unknown and never retry it automatically."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "post": { "type": "string" },
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1, "maximum": 330 }
            },
            "required": ["post"]
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let locator = required_string(&input, "post")?;
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        let url = x_post_url(&locator)?;
        let post_id = x_post_id(&url)
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
        if detail.get("ok").and_then(Value::as_bool) != Some(true)
            || detail.get("id").and_then(Value::as_str) != Some(post_id.as_str())
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
                json!({ "post": locator, "url": url, "detail": detail, "submit_click_count": 0 }),
            )));
        }
        let args = json!({ "post_id": post_id });
        let observed = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "likeTarget",
            Some(&args),
        )
        .await?;
        Ok(json_result(
            &commit_toggle(
                &self.page,
                SocialActionKind::Like,
                &format!(
                    "x:like:{}:{post_id}",
                    observed
                        .get("actor")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                ),
                &post_id,
                &format!("https://x.com/i/status/{post_id}"),
                "likeTarget",
                &args,
                "post_id",
                &post_id,
                "liked",
                wait_seconds,
                observed,
            )
            .await?,
        ))
    }
}

#[async_trait]
impl Tool for FollowTool {
    fn name(&self) -> &str {
        "follow"
    }

    fn description(&self) -> &str {
        "Follow one X account only when the user explicitly requests that account. Uses one real CDP pointer click on the profile header Follow control. If it is already followed, do not click. Treat commit_unknown as unknown and never retry it automatically."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "profile": { "type": "string" },
                "hover": { "type": "boolean", "default": false },
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1, "maximum": 330 }
            },
            "required": ["profile"]
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let locator = required_string(&input, "profile")?;
        let parsed = parse_x_profile(&locator)?;
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        if get_bool(&input, "hover", false) {
            return follow_from_hover(&self.page, &parsed.username, wait_seconds).await;
        }
        let url = x_profile_tab_url(&parsed.username, "posts");
        navigate_https(&self.page, &url).await?;
        let detail =
            wait_for_browser_tool(&self.page, SITE_ID, "profileDetail", None, wait_seconds).await?;
        if let Some(reason) = gate_reason(&detail) {
            return Ok(json_result(&failure_payload(
                reason,
                json!({ "profile": locator, "url": url, "detail": detail, "submit_click_count": 0 }),
            )));
        }
        if detail.get("ok").and_then(Value::as_bool) != Some(true)
            || detail.get("username").and_then(Value::as_str) != Some(parsed.username.as_str())
        {
            return Ok(json_result(&failure_payload(
                if detail.get("ok").and_then(Value::as_bool) == Some(true) {
                    "wrong_profile"
                } else {
                    detail
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("profile_unavailable")
                },
                json!({ "profile": locator, "url": url, "detail": detail, "submit_click_count": 0 }),
            )));
        }
        let args = json!({ "username": parsed.username });
        let observed = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "followTarget",
            Some(&args),
        )
        .await?;
        Ok(json_result(
            &commit_toggle(
                &self.page,
                SocialActionKind::Follow,
                &format!(
                    "x:follow:{}:{}",
                    observed
                        .get("actor")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    parsed.username
                ),
                &parsed.username,
                &url,
                "followTarget",
                &args,
                "username",
                &parsed.username,
                "following",
                wait_seconds,
                observed,
            )
            .await?,
        ))
    }
}

struct HoverTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for HoverTool {
    fn name(&self) -> &str {
        "hover"
    }

    fn description(&self) -> &str {
        "Hover a person's name in the current X home, search, or profile timeline, read the card, then move the pointer away so the card closes. This tool does not follow or open the profile."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "profile": { "type": "string" },
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1, "maximum": 330 }
            },
            "required": ["profile"]
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let locator = required_string(&input, "profile")?;
        let parsed = parse_x_profile(&locator)?;
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        let card = read_hover_card(&self.page, &parsed.username, wait_seconds).await?;
        if card.get("ok").and_then(Value::as_bool) != Some(true) {
            if card.get("moved").and_then(Value::as_bool) == Some(true) {
                let _ = dismiss_hover_card(&self.page).await;
            }
            return Ok(json_result(&failure_payload(
                card.get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("hover_card_not_found"),
                json!({ "username": parsed.username, "card": card }),
            )));
        }
        let dismissed = dismiss_hover_card(&self.page).await?;
        if dismissed.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(json_result(&failure_payload(
                "hover_card_still_open",
                json!({ "username": parsed.username, "card": card, "dismiss": dismissed }),
            )));
        }
        Ok(json_result(&json!({
            "ok": true,
            "username": parsed.username,
            "display_name": card.get("display_name").cloned().unwrap_or(Value::Null),
            "bio": card.get("bio").cloned().unwrap_or(json!("")),
            "following_count": card.get("following_count").cloned().unwrap_or(Value::Null),
            "followers_count": card.get("followers_count").cloned().unwrap_or(Value::Null),
            "following": card.get("following").cloned().unwrap_or(json!(false)),
            "url": current_url(&self.page).await.unwrap_or_default(),
            "dismissed": true,
        })))
    }
}

fn verified_write_target(target: &Value, post_id: &str) -> Option<(f64, f64)> {
    verified_identity_target(target, "post_id", post_id)
}

fn verified_identity_target(target: &Value, key: &str, expected: &str) -> Option<(f64, f64)> {
    if target.get("ok").and_then(Value::as_bool) != Some(true)
        || target.get("hit_owned").and_then(Value::as_bool) != Some(true)
        || target.get(key).and_then(Value::as_str) != Some(expected)
    {
        return None;
    }
    Some((target.get("x")?.as_f64()?, target.get("y")?.as_f64()?))
}

fn toggle_active(state: &Value, key: &str) -> bool {
    state.get(key).and_then(Value::as_bool) == Some(true)
}

async fn commit_toggle(
    page: &PageSession,
    kind: SocialActionKind,
    idempotency_key: &str,
    target_id: &str,
    target_url: &str,
    tool_name: &str,
    tool_args: &Value,
    identity_key: &str,
    identity: &str,
    active_key: &str,
    wait_seconds: f64,
    observed: Value,
) -> anyhow::Result<Value> {
    let actor_id = observed
        .get("actor")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if observed.get("ok").and_then(Value::as_bool) != Some(true) {
        return Ok(failure_payload(
            observed
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("control_unavailable"),
            identity_payload(identity_key, identity, target_url, &observed, 0),
        ));
    }
    if actor_id.is_empty() {
        return Ok(failure_payload(
            "current_user_unknown",
            identity_payload(identity_key, identity, target_url, &observed, 0),
        ));
    }
    let actor = ActionActor {
        id: actor_id.to_string(),
        display_name: format!("@{actor_id}"),
    };
    let store = ActionStore::open_default();
    let mut receipt = store.create_draft(
        idempotency_key,
        "x",
        kind,
        ActionTarget {
            id: target_id.to_string(),
            url: target_url.to_string(),
        },
        actor.clone(),
        ActionPreview {
            text: None,
            evidence: Value::Null,
        },
    )?;
    match receipt.status() {
        SocialActionStatus::Committed | SocialActionStatus::Reconciled => {
            return Ok(identity_fields(
                identity_key,
                identity,
                json!({
                    "ok": true, "status": receipt.status(), "action_id": receipt.action_id(),
                    "idempotent_replay": true, "url": target_url,
                    "submit_click_count": 0, "receipt": receipt,
                }),
            ));
        }
        SocialActionStatus::Committing | SocialActionStatus::CommitUnknown => {
            if toggle_active(&observed, active_key) {
                receipt = store.reconcile_committed(receipt.action_id(), target_id)?;
                return Ok(identity_fields(
                    identity_key,
                    identity,
                    json!({
                        "ok": true, "status": "reconciled", "action_id": receipt.action_id(),
                        "idempotent_replay": true, "url": target_url,
                        "submit_click_count": 0, "receipt": receipt,
                    }),
                ));
            }
            return Ok(identity_fields(
                identity_key,
                identity,
                json!({
                    "ok": false, "status": "commit_unknown",
                    "reason": "a click attempt was already reserved; reconcile instead of retrying",
                    "action_id": receipt.action_id(), "url": target_url,
                    "submit_click_count": 0, "receipt": receipt,
                }),
            ));
        }
        SocialActionStatus::Prepared => {
            receipt = store.reset_prepared(receipt.action_id(), &actor.id, target_id)?;
        }
        SocialActionStatus::Draft => {}
    }
    if toggle_active(&observed, active_key) {
        return Ok(identity_fields(
            identity_key,
            identity,
            json!({
                "ok": true,
                "status": "already_present",
                "url": target_url,
                "submit_click_count": 0,
                "state": observed,
            }),
        ));
    }
    let fresh =
        crate::sites::learning::run_site_browser_tool(page, SITE_ID, tool_name, Some(tool_args))
            .await?;
    if fresh.get("actor").and_then(Value::as_str) != Some(actor.id.as_str())
        || fresh.get(identity_key).and_then(Value::as_str) != Some(identity)
    {
        return Ok(failure_payload(
            "actor_or_target_changed_before_click",
            identity_payload(identity_key, identity, target_url, &fresh, 0),
        ));
    }
    if toggle_active(&fresh, active_key) {
        return Ok(identity_fields(
            identity_key,
            identity,
            json!({
                "ok": true, "status": "already_present", "url": target_url,
                "submit_click_count": 0, "state": fresh,
            }),
        ));
    }
    let Some((x, y)) = verified_identity_target(&fresh, identity_key, identity) else {
        return Ok(failure_payload(
            fresh
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("control_unavailable"),
            identity_payload(identity_key, identity, target_url, &fresh, 0),
        ));
    };
    receipt = store.mark_prepared(receipt.action_id(), &actor.id, target_id, 300)?;
    receipt = store.begin_commit(receipt.action_id(), &actor.id, target_id, Vec::new())?;
    let action_id = receipt.action_id().to_string();
    let dispatch_error = page
        .click(x, y)
        .await
        .err()
        .map(|error| format!("{error:#}"));
    let deadline = Instant::now() + Duration::from_secs_f64(wait_seconds);
    let mut reconcile_error = None;
    let mut reconciled = json!({ "ok": false, active_key: false });
    loop {
        match crate::sites::learning::run_site_browser_tool(
            page,
            SITE_ID,
            tool_name,
            Some(tool_args),
        )
        .await
        {
            Ok(state) => {
                if state.get("actor").and_then(Value::as_str) != Some(actor.id.as_str()) {
                    reconcile_error = Some("signed-in actor changed after click".to_string());
                    reconciled = state;
                    break;
                }
                reconciled = state;
                if toggle_active(&reconciled, active_key) || Instant::now() >= deadline {
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
    let active = toggle_active(&reconciled, active_key)
        && reconciled.get(identity_key).and_then(Value::as_str) == Some(identity);
    let (committed, persisted_receipt, receipt_error) = if active {
        match store.reconcile_committed(&action_id, target_id) {
            Ok(receipt) => (true, Some(receipt), None),
            Err(error) => (false, None, Some(format!("{error:#}"))),
        }
    } else {
        match store.finish_commit(&action_id, false) {
            Ok(receipt) => (false, Some(receipt), None),
            Err(error) => (false, None, Some(format!("{error:#}"))),
        }
    };
    Ok(identity_fields(
        identity_key,
        identity,
        json!({
            "ok": committed,
            "status": if committed { "committed" } else { "commit_unknown" },
            "action_id": action_id,
            "url": target_url,
            "interaction": "trusted_pointer",
            "platform_api_called": false,
            "submit_click_count": 1,
            "dispatch_error": dispatch_error,
            "reconcile_error": reconcile_error,
            "receipt_error": receipt_error,
            "receipt": persisted_receipt,
            "state": reconciled,
        }),
    ))
}

fn identity_fields(key: &str, identity: &str, mut payload: Value) -> Value {
    if let Some(object) = payload.as_object_mut() {
        object.insert(key.to_string(), json!(identity));
    }
    payload
}

fn identity_payload(key: &str, identity: &str, url: &str, state: &Value, clicks: u64) -> Value {
    identity_fields(
        key,
        identity,
        json!({ "url": url, "state": state, "submit_click_count": clicks }),
    )
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

struct ParsedProfile {
    username: String,
    tab: Option<&'static str>,
}

fn point_of(target: &Value) -> Option<(f64, f64)> {
    if target.get("ok").and_then(Value::as_bool) != Some(true)
        || target.get("hit_owned").and_then(Value::as_bool) != Some(true)
    {
        return None;
    }
    Some((target.get("x")?.as_f64()?, target.get("y")?.as_f64()?))
}

fn x_home_tab(raw: &str) -> anyhow::Result<String> {
    let tab = raw.trim();
    if tab.is_empty() || tab.chars().count() > 40 {
        anyhow::bail!("tab must be a non-empty home tab label of at most 40 characters");
    }
    Ok(tab.to_string())
}

fn stream_target_miss(status: &str) -> bool {
    matches!(
        status,
        "post_not_found" | "reply_button_not_visible" | "name_not_found" | "name_not_visible"
    )
}

async fn reveal_stream_target(
    page: &PageSession,
    tool_name: &str,
    args: &Value,
    wait_seconds: f64,
) -> anyhow::Result<Value> {
    let deadline = Instant::now() + Duration::from_secs_f64(wait_seconds);
    let mut latest = json!({ "ok": false, "status": "not_found" });
    loop {
        latest =
            crate::sites::learning::run_site_browser_tool(page, SITE_ID, tool_name, Some(args))
                .await?;
        if latest.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(latest);
        }
        let status = latest.get("status").and_then(Value::as_str).unwrap_or("");
        if !stream_target_miss(status) || Instant::now() >= deadline {
            return Ok(latest);
        }
        let _ = crate::sites::learning::run_site_browser_tool(
            page,
            SITE_ID,
            "scrollResults",
            Some(&json!({})),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

async fn wait_for_home_tab(
    page: &PageSession,
    tab: &str,
    previous_first_id: &str,
    wait_seconds: f64,
) -> anyhow::Result<Value> {
    let deadline = Instant::now() + Duration::from_secs_f64(wait_seconds);
    let args = json!({ "tab": tab });
    let mut cleared = false;
    let mut latest = json!({ "ok": false, "status": "home_tab_not_settled" });
    while Instant::now() < deadline {
        latest =
            crate::sites::learning::run_site_browser_tool(page, SITE_ID, "feedState", Some(&args))
                .await?;
        if gate_reason(&latest).is_some() {
            return Ok(latest);
        }
        let count = latest
            .get("result_count")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let first = latest.get("first_id").and_then(Value::as_str).unwrap_or("");
        if count == 0 {
            cleared = true;
        }
        let switched = cleared || (!previous_first_id.is_empty() && first != previous_first_id);
        if latest.get("ok").and_then(Value::as_bool) == Some(true) && switched {
            return Ok(latest);
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    if latest.get("status").and_then(Value::as_str).is_none() {
        latest["status"] = json!("home_tab_not_settled");
    }
    latest["ok"] = json!(false);
    Ok(latest)
}

async fn dismiss_hover_card(page: &PageSession) -> anyhow::Result<Value> {
    let target =
        crate::sites::learning::run_site_browser_tool(page, SITE_ID, "hoverDismissTarget", None)
            .await?;
    let Some((x, y)) = point_of(&target) else {
        return Ok(target);
    };
    page.mouse_move(x, y).await?;
    crate::sites::learning::run_site_browser_tool(page, SITE_ID, "hoverCardPresence", None).await
}

async fn read_hover_card(
    page: &PageSession,
    username: &str,
    wait_seconds: f64,
) -> anyhow::Result<Value> {
    let args = json!({ "username": username });
    let name = reveal_stream_target(page, "streamNameTarget", &args, wait_seconds).await?;
    if name.get("ok").and_then(Value::as_bool) != Some(true) {
        return Ok(name);
    }
    let Some((x, y)) = point_of(&name) else {
        return Ok(failure_payload(
            "name_obscured",
            json!({ "username": username, "target": name }),
        ));
    };
    page.mouse_move(x, y).await?;
    let mut card = wait_for_browser_tool(page, SITE_ID, "hoverCardState", Some(&args), 8.0).await?;
    if let Some(object) = card.as_object_mut() {
        object.insert("moved".into(), json!(true));
    }
    Ok(card)
}

async fn follow_from_hover(
    page: &PageSession,
    username: &str,
    wait_seconds: f64,
) -> anyhow::Result<ToolResult> {
    let card = read_hover_card(page, username, wait_seconds).await?;
    let moved = card.get("moved").and_then(Value::as_bool) == Some(true);
    if card.get("ok").and_then(Value::as_bool) != Some(true) {
        if moved {
            let _ = dismiss_hover_card(page).await;
        }
        return Ok(json_result(&failure_payload(
            card.get("status")
                .and_then(Value::as_str)
                .unwrap_or("hover_card_not_found"),
            json!({ "username": username, "card": card, "submit_click_count": 0 }),
        )));
    }
    let args = json!({ "username": username });
    let observed = crate::sites::learning::run_site_browser_tool(
        page,
        SITE_ID,
        "hoverFollowTarget",
        Some(&args),
    )
    .await?;
    let actor = observed
        .get("actor")
        .and_then(Value::as_str)
        .or_else(|| card.get("actor").and_then(Value::as_str))
        .unwrap_or_default();
    let mut result = commit_toggle(
        page,
        SocialActionKind::Follow,
        &format!("x:follow:{actor}:{username}"),
        username,
        &format!("https://x.com/{username}"),
        "hoverFollowTarget",
        &args,
        "username",
        username,
        "following",
        wait_seconds,
        observed,
    )
    .await?;
    let dismissed = dismiss_hover_card(page).await?;
    let closed = dismissed.get("ok").and_then(Value::as_bool) == Some(true);
    if let Some(object) = result.as_object_mut() {
        object.insert("dismissed".into(), json!(closed));
        object.insert("hover".into(), json!(true));
        if !closed {
            object.insert("ok".into(), json!(false));
            object.insert("reason".into(), json!("hover_card_still_open"));
        }
    }
    Ok(json_result(&result))
}

async fn close_reply_overlay(page: &PageSession) {
    let target =
        crate::sites::learning::run_site_browser_tool(page, SITE_ID, "overlayCloseTarget", None)
            .await
            .unwrap_or_else(|_| json!({ "ok": false }));
    if let Some((x, y)) = point_of(&target) {
        let _ = page.click(x, y).await;
    }
}

async fn reply_from_timeline(
    page: &PageSession,
    locator: &str,
    text: &str,
    input: &Value,
) -> anyhow::Result<ToolResult> {
    let url = x_post_url(locator)?;
    let post_id = x_post_id(&url)
        .ok_or_else(|| anyhow::anyhow!("canonical X post URL is missing a status id"))?;
    let wait_seconds =
        get_f64(input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
    let timeline =
        crate::sites::learning::run_site_browser_tool(page, SITE_ID, "timelineState", None).await?;
    if let Some(reason) = gate_reason(&timeline) {
        return Ok(json_result(&failure_payload(
            reason,
            json!({ "post_id": post_id, "url": url, "state": timeline, "submit_click_count": 0 }),
        )));
    }
    if timeline.get("ok").and_then(Value::as_bool) != Some(true) {
        return Ok(json_result(&failure_payload(
            timeline
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("not_timeline"),
            json!({ "post_id": post_id, "url": url, "state": timeline, "submit_click_count": 0 }),
        )));
    }
    let return_path = timeline
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let return_url = timeline
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let target = reveal_stream_target(
        page,
        "streamReplyTarget",
        &json!({ "post_id": post_id }),
        wait_seconds,
    )
    .await?;
    if target.get("ok").and_then(Value::as_bool) != Some(true) {
        return Ok(json_result(&failure_payload(
            target
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("reply_button_unavailable"),
            json!({ "post_id": post_id, "url": return_url, "target": target, "submit_click_count": 0 }),
        )));
    }
    let actor_id = target
        .get("actor")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if actor_id.is_empty() {
        return Ok(json_result(&failure_payload(
            "current_user_unknown",
            json!({ "post_id": post_id, "url": return_url, "submit_click_count": 0 }),
        )));
    }
    let overlay_args = json!({
        "username": target.get("username").and_then(Value::as_str).unwrap_or_default(),
        "text": target.get("text").and_then(Value::as_str).unwrap_or_default(),
        "published_at": target.get("published_at").and_then(Value::as_str).unwrap_or_default(),
    });
    let actor = ActionActor {
        id: actor_id.clone(),
        display_name: format!("@{actor_id}"),
    };
    let store = ActionStore::open_default();
    let mut receipt = store.create_draft(
        &format!("x:reply:{actor_id}:{post_id}:{text}"),
        "x",
        SocialActionKind::Reply,
        ActionTarget {
            id: post_id.clone(),
            url: url.clone(),
        },
        actor.clone(),
        ActionPreview {
            text: Some(text.to_string()),
            evidence: Value::Null,
        },
    )?;
    match receipt.status() {
        SocialActionStatus::Committed | SocialActionStatus::Reconciled => {
            return Ok(json_result(&json!({
                "ok": true, "status": receipt.status(), "action_id": receipt.action_id(),
                "idempotent_replay": true, "post_id": post_id, "url": return_url,
                "reply": text, "inline": true, "submit_click_count": 0, "receipt": receipt,
            })));
        }
        SocialActionStatus::Committing | SocialActionStatus::CommitUnknown => {
            return Ok(json_result(&json!({
                "ok": false, "status": "commit_unknown",
                "reason": "a submit attempt was already reserved; reconcile instead of retrying",
                "action_id": receipt.action_id(), "post_id": post_id, "url": return_url,
                "reply": text, "inline": true, "submit_click_count": 0, "receipt": receipt,
            })));
        }
        SocialActionStatus::Prepared => {
            receipt = store.reset_prepared(receipt.action_id(), &actor.id, &post_id)?;
        }
        SocialActionStatus::Draft => {}
    }
    let Some((reply_x, reply_y)) = point_of(&target) else {
        return Ok(json_result(&failure_payload(
            "reply_button_obscured",
            json!({ "post_id": post_id, "url": return_url, "target": target, "submit_click_count": 0 }),
        )));
    };
    page.click(reply_x, reply_y).await?;
    let editor = wait_for_browser_tool(
        page,
        SITE_ID,
        "overlayReplyEditorTarget",
        Some(&overlay_args),
        8.0,
    )
    .await?;
    let Some((editor_x, editor_y)) = point_of(&editor) else {
        close_reply_overlay(page).await;
        return Ok(json_result(&failure_payload(
            editor
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("reply_overlay_not_found"),
            json!({ "post_id": post_id, "url": return_url, "editor": editor, "submit_click_count": 0 }),
        )));
    };
    page.click(editor_x, editor_y).await?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let draft = crate::sites::learning::run_site_browser_tool(
        page,
        SITE_ID,
        "overlayReplyDraftState",
        Some(&overlay_args),
    )
    .await?;
    let draft_text = draft
        .get("value")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let already_typed = draft_text == text;
    if draft.get("ok").and_then(Value::as_bool) != Some(true)
        || draft.get("focused").and_then(Value::as_bool) != Some(true)
        || (!draft_text.is_empty() && !already_typed)
    {
        close_reply_overlay(page).await;
        return Ok(json_result(&failure_payload(
            "reply_editor_not_empty_or_focused",
            json!({ "post_id": post_id, "url": return_url, "draft": draft, "submit_click_count": 0 }),
        )));
    }
    if !already_typed {
        page.type_chars(text).await?;
        let typed_deadline = Instant::now() + Duration::from_secs(5);
        let typed = loop {
            let state = crate::sites::learning::run_site_browser_tool(
                page,
                SITE_ID,
                "overlayReplyDraftState",
                Some(&overlay_args),
            )
            .await?;
            if state.get("value").and_then(Value::as_str).map(str::trim) == Some(text)
                || Instant::now() >= typed_deadline
            {
                break state;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        };
        if typed.get("value").and_then(Value::as_str).map(str::trim) != Some(text) {
            close_reply_overlay(page).await;
            return Ok(json_result(&failure_payload(
                "reply_draft_mismatch",
                json!({ "post_id": post_id, "url": return_url, "draft": typed, "submit_click_count": 0 }),
            )));
        }
    }
    let submit = wait_for_browser_tool(
        page,
        SITE_ID,
        "overlayReplySubmitTarget",
        Some(&overlay_args),
        8.0,
    )
    .await?;
    let Some((submit_x, submit_y)) = point_of(&submit) else {
        close_reply_overlay(page).await;
        return Ok(json_result(&failure_payload(
            submit
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("reply_submit_unavailable"),
            json!({ "post_id": post_id, "url": return_url, "submit": submit, "submit_click_count": 0 }),
        )));
    };
    receipt = store.mark_prepared(receipt.action_id(), &actor.id, &post_id, 300)?;
    receipt = store.begin_commit(receipt.action_id(), &actor.id, &post_id, Vec::new())?;
    let action_id = receipt.action_id().to_string();
    let dispatch_error = page
        .click(submit_x, submit_y)
        .await
        .err()
        .map(|error| format!("{error:#}"));
    let deadline = Instant::now() + Duration::from_secs_f64(wait_seconds);
    let closed_args = json!({ "path": return_path });
    let mut returned = json!({ "ok": false, "status": "overlay_open" });
    loop {
        returned = crate::sites::learning::run_site_browser_tool(
            page,
            SITE_ID,
            "overlayClosed",
            Some(&closed_args),
        )
        .await?;
        let status = returned.get("status").and_then(Value::as_str).unwrap_or("");
        if returned.get("ok").and_then(Value::as_bool) == Some(true)
            || matches!(status, "graduated_access" | "left_timeline")
            || Instant::now() >= deadline
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    let verified = returned.get("ok").and_then(Value::as_bool) == Some(true);
    let (committed, persisted_receipt, receipt_error) =
        match store.finish_commit(&action_id, verified) {
            Ok(receipt) => (verified, Some(receipt), None),
            Err(error) => (false, None, Some(format!("{error:#}"))),
        };
    Ok(json_result(&json!({
        "ok": committed,
        "status": if committed { "committed" } else { "commit_unknown" },
        "action_id": action_id,
        "post_id": post_id,
        "url": current_url(page).await.unwrap_or_else(|_| return_url.clone()),
        "return_url": return_url,
        "reply": text,
        "inline": true,
        "interaction": "timeline_reply_overlay",
        "submit_click_count": 1,
        "dispatch_error": dispatch_error,
        "receipt_error": receipt_error,
        "receipt": persisted_receipt,
        "state": returned,
    })))
}

fn x_search_filter(raw: &str) -> anyhow::Result<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "top" => Ok("top"),
        "latest" | "live" => Ok("latest"),
        "people" | "user" | "users" => Ok("people"),
        "media" => Ok("media"),
        "lists" | "list" => Ok("lists"),
        _ => anyhow::bail!("filter must be top, latest, people, media, or lists"),
    }
}

fn x_search_url(query: &str, filter: &str) -> String {
    let mut url = format!(
        "https://x.com/search?q={}&src=typed_query",
        percent_encode_query(query)
    );
    let param = match filter {
        "latest" => Some("live"),
        "people" => Some("user"),
        "media" => Some("media"),
        "lists" => Some("list"),
        _ => None,
    };
    if let Some(param) = param {
        url.push_str("&f=");
        url.push_str(param);
    }
    url
}

fn x_profile_tab(raw: &str) -> anyhow::Result<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "posts" | "post" => Ok("posts"),
        "replies" | "reply" | "with_replies" => Ok("replies"),
        "reposts" | "repost" => Ok("reposts"),
        "media" => Ok("media"),
        "highlights" | "highlight" => Ok("highlights"),
        "articles" | "article" => Ok("articles"),
        "likes" | "like" => Ok("likes"),
        _ => anyhow::bail!(
            "tab must be posts, replies, reposts, media, highlights, articles, or likes"
        ),
    }
}

fn x_profile_tab_suffix(tab: &str) -> &'static str {
    match tab {
        "replies" => "/with_replies",
        "reposts" => "/reposts",
        "media" => "/media",
        "highlights" => "/highlights",
        "articles" => "/articles",
        "likes" => "/likes",
        _ => "",
    }
}

fn x_profile_tab_url(username: &str, tab: &str) -> String {
    format!("https://x.com/{username}{}", x_profile_tab_suffix(tab))
}

fn parse_x_profile(locator: &str) -> anyhow::Result<ParsedProfile> {
    let trimmed = locator.trim();
    if let Ok(url) = reqwest::Url::parse(trimmed) {
        validate_x_url(&url, "profile")?;
        let parts = url
            .path_segments()
            .into_iter()
            .flatten()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let (username, tab) = match parts.as_slice() {
            [username] => (username.to_ascii_lowercase(), None),
            [username, suffix] => (username.to_ascii_lowercase(), Some(x_profile_tab(suffix)?)),
            _ => anyhow::bail!("X profile URL must identify exactly one profile"),
        };
        validate_username(&username, locator)?;
        return Ok(ParsedProfile { username, tab });
    }
    let username = trimmed.trim_start_matches('@').to_ascii_lowercase();
    validate_username(&username, locator)?;
    Ok(ParsedProfile {
        username,
        tab: None,
    })
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
