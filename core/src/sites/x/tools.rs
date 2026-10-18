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
use crate::sites::login_wait::{interactive_remote_login_enabled, login_resume_receiver};
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
const NOTIFICATIONS_URL: &str = "https://x.com/notifications";
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
        Arc::new(NotificationsTool { page: page.clone() }),
        Arc::new(FollowersTool { page: page.clone() }),
        Arc::new(SearchTool { page: page.clone() }),
        Arc::new(ProfileTool { page: page.clone() }),
        Arc::new(GetPostsTool { page: page.clone() }),
        Arc::new(ReplyTool { page: page.clone() }),
        Arc::new(PostTool { page: page.clone() }),
        Arc::new(LikeTool { page: page.clone() }),
        Arc::new(FollowTool { page: page.clone() }),
        Arc::new(HoverTool { page: page.clone() }),
        Arc::new(PageStateTool { page: page.clone() }),
        x_wait_for_login_tool(page),
    ]
}

pub fn x_wait_for_login_tool(page: Arc<PageSession>) -> Arc<dyn Tool> {
    Arc::new(WaitForXLoginTool { page })
}

pub static X_NATIVE_ADAPTER: NativeSiteAdapter = NativeSiteAdapter {
    browserless_tools: None,
    runtime_command: None,
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
            about: "Search X across Top, Latest, People, Media, or Lists and optionally click visible posts for details and replies.",
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
                    key: "deep",
                    long: Some("deep"),
                    value_name: "N",
                    help: "Open up to N collected posts by clicking the current Top or Latest search timeline. Defaults to 0.",
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
            name: "notifications",
            tool_name: "notifications",
            about: "Read the X notifications timeline. Clicks the sidebar Notifications link and scrolls. Opens the notifications URL only if that link is missing or the click does not land.",
            args: &[
                CommandArg {
                    key: "tab",
                    long: Some("tab"),
                    value_name: "TAB",
                    help: "Notifications tab: all or mentions. Defaults to all.",
                    required: false,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "num",
                    long: Some("num"),
                    value_name: "N",
                    help: "Number of notification rows to collect by scrolling. Defaults to 20.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for the notifications page to hydrate. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_notifications,
        },
        SiteCommand {
            name: "followers",
            tool_name: "followers",
            about: "Read people who follow the signed-in account and still show Follow back. Clicks Profile, then Followers. A profile URL is only used if those clicks do not land.",
            args: &[
                CommandArg {
                    key: "num",
                    long: Some("num"),
                    value_name: "N",
                    help: "Number of Follow back accounts to collect by scrolling. Defaults to 20.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for the followers page to hydrate. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_followers,
        },
        SiteCommand {
            name: "profile",
            tool_name: "profile",
            about: "Read an X profile across Posts, Replies, Reposts, Media, Highlights, Articles, or Likes and optionally click visible timeline posts for details and replies.",
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
            name: "post",
            tool_name: "post",
            about: "Publish one X post by typing into the home timeline composer and clicking Post.",
            args: &[
                CommandArg {
                    key: "text",
                    long: Some("text"),
                    value_name: "TEXT",
                    help: "Exact post text. The command refuses to replace an existing draft.",
                    required: true,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for the composer and post-submit reconciliation. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_post,
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

fn run_notifications(
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
        "notifications",
        "notifications",
    )
}

fn run_followers(
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
        "followers",
        "followers",
    )
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

fn run_post(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    run_named(page, args, debug_snapshot, progress, "post", "post")
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
        "Search X across top, latest, people, media, or lists. For top or latest results, set `deep` to open that many posts with trusted clicks in the current timeline, read details/replies, and restore the search URL and scroll position. Deep-read click or restoration failures fail closed and never fall back to direct post navigation. This tool is read-only."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "maxLength": 512 },
                "filter": { "type": "string", "enum": ["top", "latest", "people", "media", "lists"], "default": "top" },
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
        let filter = x_search_filter(input.get("filter").and_then(Value::as_str).unwrap_or("top"))?;
        let num = get_i64(&input, "num", DEFAULT_RESULT_COUNT).clamp(1, MAX_TOOL_ITEMS);
        let deep = get_i64(&input, "deep", 0).clamp(0, num);
        let num_comments =
            get_i64(&input, "num_comments", DEFAULT_COMMENT_COUNT).clamp(0, MAX_TOOL_ITEMS);
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        if deep > 0 && !x_search_deep_read_supported(filter) {
            return Ok(json_result(&failure_payload(
                "deep_read_unsupported_filter",
                json!({
                    "filter": filter,
                    "supported_filters": ["top", "latest"],
                    "navigation_policy": "card_click_only",
                }),
            )));
        }
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
        let mut payload = json!({
            "ok": true,
            "query": query,
            "filter": filter,
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
        "Read an X profile and one timeline tab: posts, replies, reposts, media, highlights, articles, or likes. Posts is the default. For the signed-in account's posts tab, clicks the profile sidebar first and opens the profile URL only if that click does not land. For post result tabs, set `deep` to open that many posts with trusted clicks in the current timeline, read details/replies, and restore the profile URL and scroll position. Deep-read click or restoration failures fail closed and never fall back to direct post navigation."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "profile": { "type": "string" },
                "tab": { "type": "string", "enum": ["posts", "replies", "reposts", "media", "highlights", "articles", "likes"], "default": "posts" },
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
        let parsed = parse_x_profile(&locator)?;
        let tab = match input.get("tab").and_then(Value::as_str) {
            Some(value) => x_profile_tab(value)?,
            None => parsed.tab.unwrap_or("posts"),
        };
        let url = x_profile_tab_url(&parsed.username, tab);
        let num = get_i64(&input, "num", DEFAULT_RESULT_COUNT).clamp(1, MAX_TOOL_ITEMS);
        let deep = get_i64(&input, "deep", 0).clamp(0, num);
        let num_comments =
            get_i64(&input, "num_comments", DEFAULT_COMMENT_COUNT).clamp(0, MAX_TOOL_ITEMS);
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        let mut state = json!({ "ok": false });
        let mut opened_by_click = false;
        if tab == "posts" && click_own_profile(&self.page, &parsed.username).await? {
            state = wait_for_browser_tool(&self.page, SITE_ID, "profileDetail", None, wait_seconds)
                .await?;
            let observed_user = state.get("username").and_then(Value::as_str).unwrap_or("");
            let observed_tab = state.get("tab").and_then(Value::as_str).unwrap_or("");
            opened_by_click = state.get("ok").and_then(Value::as_bool) == Some(true)
                && observed_user.eq_ignore_ascii_case(&parsed.username)
                && observed_tab == tab;
        }
        if !opened_by_click {
            navigate_https(&self.page, &url).await?;
            state = wait_for_browser_tool(&self.page, SITE_ID, "profileDetail", None, wait_seconds)
                .await?;
        }
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
        let mut payload = json!({
            "ok": true,
            "tab": tab,
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

struct WaitForXLoginTool {
    page: Arc<PageSession>,
}

const WAIT_FOR_X_LOGIN_DEFAULT_SECS: i64 = 180;
const WAIT_FOR_X_LOGIN_MAX_SECS: i64 = 600;

struct ReplyTool {
    page: Arc<PageSession>,
}

struct PostTool {
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

#[async_trait]
impl Tool for PostTool {
    fn name(&self) -> &str {
        "post"
    }

    fn description(&self) -> &str {
        "Publish one X post only when the user explicitly requests the exact text. Types into the home timeline composer and clicks its Post button once. Does not open the compose URL. Refuses an existing draft and does not retry commit_unknown."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "minLength": 1, "maxLength": 10000 },
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1, "maximum": 330 }
            },
            "required": ["text"]
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let raw_text = input
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing required argument: text"))?;
        if raw_text.trim().is_empty() || raw_text.trim() != raw_text {
            anyhow::bail!("post text must be non-empty and have no leading or trailing whitespace");
        }
        let text = raw_text.to_string();
        if text.chars().count() > 10_000 {
            anyhow::bail!("post text must contain at most 10000 characters");
        }
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        let composer = reveal_home_composer(&self.page).await?;
        let page_state =
            crate::sites::learning::run_site_browser_tool(&self.page, SITE_ID, "pageState", None)
                .await?;
        if let Some(reason) = gate_reason(&page_state).or_else(|| gate_reason(&composer)) {
            return Ok(json_result(&failure_payload(
                reason,
                json!({ "url": HOME_URL, "composer": composer, "submit_click_count": 0 }),
            )));
        }
        if composer.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(json_result(&failure_payload(
                composer
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("post_editor_unavailable"),
                json!({ "url": HOME_URL, "composer": composer, "submit_click_count": 0 }),
            )));
        }
        let actor_id = composer
            .get("actor")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if actor_id.is_empty() {
            return Ok(json_result(&failure_payload(
                "current_user_unknown",
                json!({ "url": HOME_URL, "composer": composer, "submit_click_count": 0 }),
            )));
        }
        let rendered_args = json!({ "text": text });
        let before = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "renderedPostState",
            Some(&rendered_args),
        )
        .await?;
        if before.get("status").and_then(Value::as_str) == Some("graduated_access") {
            return Ok(json_result(&failure_payload(
                "graduated_access",
                json!({ "url": HOME_URL, "reconcile": before, "submit_click_count": 0 }),
            )));
        }
        let actor = ActionActor {
            id: actor_id.clone(),
            display_name: format!("@{actor_id}"),
        };
        let idempotency_key = format!("x:post:{actor_id}:{text}");
        let store = ActionStore::open_default();
        let mut receipt = store.create_draft(
            &idempotency_key,
            "x",
            SocialActionKind::Publish,
            ActionTarget {
                id: actor_id.clone(),
                url: HOME_URL.to_string(),
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
                    "idempotent_replay": true, "url": HOME_URL,
                    "text": text, "submit_click_count": 0, "receipt": receipt,
                })));
            }
            SocialActionStatus::Committing | SocialActionStatus::CommitUnknown => {
                let prior = receipt.precommit_target_ids().unwrap_or(&[]);
                let observed = value_string_array(&before, "ids");
                if let Some(new_id) = observed.into_iter().find(|id| !prior.contains(id)) {
                    receipt = store.reconcile_committed(receipt.action_id(), &new_id)?;
                    return Ok(json_result(&json!({
                        "ok": true, "status": "reconciled", "action_id": receipt.action_id(),
                        "idempotent_replay": true, "url": HOME_URL,
                        "text": text, "post_id": new_id, "submit_click_count": 0, "receipt": receipt,
                    })));
                }
                return Ok(json_result(&json!({
                    "ok": false, "status": "commit_unknown",
                    "reason": "a submit attempt was already reserved; reconcile instead of retrying",
                    "action_id": receipt.action_id(), "url": HOME_URL,
                    "text": text, "submit_click_count": 0, "receipt": receipt,
                })));
            }
            SocialActionStatus::Prepared => {
                receipt = store.reset_prepared(receipt.action_id(), &actor.id, &actor_id)?;
            }
            SocialActionStatus::Draft => {}
        }
        let prior_ids = value_string_array(&before, "ids");
        if !prior_ids.is_empty() {
            return Ok(json_result(&json!({
                "ok": true,
                "status": "already_present",
                "url": HOME_URL,
                "text": text,
                "post_id": prior_ids.first().cloned().unwrap_or_default(),
                "submit_click_count": 0,
                "reconcile": before,
            })));
        }
        let Some((editor_x, editor_y)) = point_of(&composer) else {
            return Ok(json_result(&failure_payload(
                composer
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("post_editor_unavailable"),
                json!({ "url": HOME_URL, "composer": composer, "submit_click_count": 0 }),
            )));
        };
        self.page.click(editor_x, editor_y).await?;
        tokio::time::sleep(Duration::from_millis(150)).await;
        let draft = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "postDraftState",
            None,
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
            if draft_text.is_empty() || already_typed {
                close_post_composer(&self.page).await;
            }
            return Ok(json_result(&failure_payload(
                "post_editor_not_empty_or_focused",
                json!({ "url": HOME_URL, "draft": draft, "submit_click_count": 0 }),
            )));
        }
        if !already_typed {
            self.page.type_chars(&text).await?;
            let typed_deadline = Instant::now() + Duration::from_secs(5);
            let typed = loop {
                let state = crate::sites::learning::run_site_browser_tool(
                    &self.page,
                    SITE_ID,
                    "postDraftState",
                    None,
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
                close_post_composer(&self.page).await;
                return Ok(json_result(&failure_payload(
                    "post_draft_mismatch",
                    json!({ "url": HOME_URL, "draft": typed, "submit_click_count": 0 }),
                )));
            }
        }
        let submit =
            wait_for_browser_tool(&self.page, SITE_ID, "postSubmitTarget", None, 8.0).await?;
        let Some((submit_x, submit_y)) = point_of(&submit) else {
            close_post_composer(&self.page).await;
            return Ok(json_result(&failure_payload(
                submit
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("post_submit_unavailable"),
                json!({ "url": HOME_URL, "submit": submit, "submit_click_count": 0 }),
            )));
        };
        let final_rendered = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "renderedPostState",
            Some(&rendered_args),
        )
        .await?;
        if final_rendered.get("actor").and_then(Value::as_str) != Some(actor.id.as_str()) {
            close_post_composer(&self.page).await;
            return Ok(json_result(&failure_payload(
                "actor_changed_before_submit",
                json!({ "url": HOME_URL, "reconcile": final_rendered, "submit_click_count": 0 }),
            )));
        }
        let precommit_ids = value_string_array(&final_rendered, "ids");
        if precommit_ids.iter().any(|id| !prior_ids.contains(id)) {
            close_post_composer(&self.page).await;
            return Ok(json_result(&json!({
                "ok": true,
                "status": "already_present",
                "url": HOME_URL,
                "text": text,
                "submit_click_count": 0,
                "reconcile": final_rendered,
            })));
        }
        receipt = store.mark_prepared(receipt.action_id(), &actor.id, &actor_id, 300)?;
        receipt = store.begin_commit(
            receipt.action_id(),
            &actor.id,
            &actor_id,
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
                "renderedPostState",
                Some(&rendered_args),
            )
            .await
            {
                Ok(state) => {
                    reconciled = state;
                    let status = reconciled
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if status == "graduated_access"
                        || reconciled.get("actor").and_then(Value::as_str)
                            != Some(actor.id.as_str())
                    {
                        if reconciled.get("actor").and_then(Value::as_str)
                            != Some(actor.id.as_str())
                            && status != "graduated_access"
                        {
                            reconcile_error =
                                Some("signed-in actor changed after submit dispatch".to_string());
                        }
                        break;
                    }
                    let ids = value_string_array(&reconciled, "ids");
                    if ids.iter().any(|id| !precommit_ids.contains(id))
                        || Instant::now() >= deadline
                    {
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
        let posted_id = new_target_id.clone().unwrap_or_default();
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
            "url": HOME_URL,
            "text": text,
            "post_id": posted_id,
            "interaction": "trusted_pointer_and_keyboard",
            "platform_api_called": false,
            "submit_click_count": 1,
            "dispatch_error": dispatch_error,
            "reconcile_error": reconcile_error,
            "receipt_error": receipt_error,
            "receipt": persisted_receipt,
            "reconcile": reconciled,
        })))
    }
}

struct NotificationsTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for NotificationsTool {
    fn name(&self) -> &str {
        "notifications"
    }

    fn description(&self) -> &str {
        "Read the X notifications timeline. Clicks the sidebar Notifications link and scrolls. Opens the notifications URL only if that link is missing or the click does not land. `tab` is all or mentions. This tool is read-only."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "tab": { "type": "string", "enum": ["all", "mentions"], "default": "all" },
                "num": { "type": "integer", "default": 20, "minimum": 1, "maximum": 100 },
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1, "maximum": 330 }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let tab = x_notifications_tab(input.get("tab").and_then(Value::as_str).unwrap_or("all"))?;
        let num = get_i64(&input, "num", 20).clamp(1, MAX_TOOL_ITEMS);
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        let state = open_notifications(&self.page, tab, wait_seconds).await?;
        if let Some(reason) = gate_reason(&state) {
            return Ok(json_result(&failure_payload(
                reason,
                json!({ "tab": tab, "state": state, "count": 0, "results": [] }),
            )));
        }
        if state.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(json_result(&failure_payload(
                state
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("notifications_unavailable"),
                json!({ "tab": tab, "state": state, "count": 0, "results": [] }),
            )));
        }
        let results = invoke_browser_tool(
            &self.page,
            ctx,
            SITE_ID,
            "notificationItems",
            Some(&json!({ "limit": num })),
            true,
        )
        .await?;
        let final_state = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "notificationsState",
            Some(&json!({ "tab": tab })),
        )
        .await?;
        if final_state.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(json_result(&failure_payload(
                final_state
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("page_unavailable_after_scroll"),
                json!({
                    "tab": tab,
                    "state": final_state,
                    "count": results.as_array().map(Vec::len).unwrap_or(0),
                    "results": results,
                    "partial": true,
                }),
            )));
        }
        Ok(json_result(&json!({
            "ok": true,
            "tab": final_state.get("tab").cloned().unwrap_or(json!(tab)),
            "tabs": final_state.get("tabs").cloned().unwrap_or(json!([])),
            "actor": final_state.get("actor").cloned().unwrap_or(json!("")),
            "url": current_url(&self.page).await.unwrap_or_default(),
            "count": results.as_array().map(Vec::len).unwrap_or(0),
            "results": results,
            "state": final_state,
        })))
    }
}

struct FollowersTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for FollowersTool {
    fn name(&self) -> &str {
        "followers"
    }

    fn description(&self) -> &str {
        "Read people who follow the signed-in account and still show Follow back. Clicks Profile, then the Followers link, and scrolls. Opens the followers URL only if those clicks do not land. This tool is read-only."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "num": { "type": "integer", "default": 20, "minimum": 1, "maximum": 100 },
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1, "maximum": 330 }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let num = get_i64(&input, "num", 20).clamp(1, MAX_TOOL_ITEMS);
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        let state = open_followers(&self.page, wait_seconds).await?;
        if let Some(reason) = gate_reason(&state) {
            return Ok(json_result(&failure_payload(
                reason,
                json!({ "state": state, "count": 0, "results": [] }),
            )));
        }
        if state.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(json_result(&failure_payload(
                state
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("followers_unavailable"),
                json!({ "state": state, "count": 0, "results": [] }),
            )));
        }
        let results = invoke_browser_tool(
            &self.page,
            ctx,
            SITE_ID,
            "followBackCandidates",
            Some(&json!({ "limit": num })),
            true,
        )
        .await?;
        let final_state = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "followersState",
            None,
        )
        .await?;
        if final_state.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(json_result(&failure_payload(
                final_state
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("page_unavailable_after_scroll"),
                json!({
                    "state": final_state,
                    "count": results.as_array().map(Vec::len).unwrap_or(0),
                    "results": results,
                    "partial": true,
                }),
            )));
        }
        Ok(json_result(&json!({
            "ok": true,
            "username": final_state.get("username").cloned().unwrap_or(json!("")),
            "actor": final_state.get("actor").cloned().unwrap_or(json!("")),
            "url": current_url(&self.page).await.unwrap_or_default(),
            "count": results.as_array().map(Vec::len).unwrap_or(0),
            "results": results,
            "state": final_state,
        })))
    }
}

async fn click_named_nav(page: &PageSession, name: &str) -> anyhow::Result<(bool, String)> {
    let target = crate::sites::learning::run_site_browser_tool(
        page,
        SITE_ID,
        "navLinkTarget",
        Some(&json!({ "name": name })),
    )
    .await?;
    let username = target
        .get("username")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let Some((x, y)) = point_of(&target) else {
        return Ok((false, username));
    };
    page.click(x, y).await?;
    Ok((true, username))
}

async fn click_own_profile(page: &PageSession, username: &str) -> anyhow::Result<bool> {
    let url = current_url(page).await.unwrap_or_default();
    let (on_x, _) = on_x_home(&url);
    if !on_x {
        return Ok(false);
    }
    let target = crate::sites::learning::run_site_browser_tool(
        page,
        SITE_ID,
        "navLinkTarget",
        Some(&json!({ "name": "profile" })),
    )
    .await?;
    let actor = target.get("username").and_then(Value::as_str).unwrap_or("");
    if !actor.eq_ignore_ascii_case(username) {
        return Ok(false);
    }
    let Some((x, y)) = point_of(&target) else {
        return Ok(false);
    };
    page.click(x, y).await?;
    Ok(true)
}

async fn open_notifications(
    page: &PageSession,
    tab: &str,
    wait_seconds: f64,
) -> anyhow::Result<Value> {
    let url = current_url(page).await.unwrap_or_default();
    let (on_x, _) = on_x_home(&url);
    let mut used_url = false;
    if on_x {
        let (clicked, _) = click_named_nav(page, "notifications").await?;
        if !clicked {
            navigate_https(page, NOTIFICATIONS_URL).await?;
            used_url = true;
        }
    } else {
        navigate_https(page, NOTIFICATIONS_URL).await?;
        used_url = true;
    }
    let mut state = wait_for_browser_tool(
        page,
        SITE_ID,
        "notificationsState",
        Some(&json!({ "tab": "" })),
        wait_seconds,
    )
    .await?;
    if state.get("ok").and_then(Value::as_bool) != Some(true) && !used_url {
        navigate_https(page, NOTIFICATIONS_URL).await?;
        state = wait_for_browser_tool(
            page,
            SITE_ID,
            "notificationsState",
            Some(&json!({ "tab": "" })),
            wait_seconds,
        )
        .await?;
    }
    if state.get("ok").and_then(Value::as_bool) != Some(true) || gate_reason(&state).is_some() {
        return Ok(state);
    }
    let selected = state.get("tab").and_then(Value::as_str).unwrap_or("");
    if normalize_label(selected) == normalize_label(tab) {
        return Ok(state);
    }
    let target = crate::sites::learning::run_site_browser_tool(
        page,
        SITE_ID,
        "notificationsTabTarget",
        Some(&json!({ "tab": tab })),
    )
    .await?;
    if target.get("selected").and_then(Value::as_bool) == Some(true) {
        return wait_notifications(page, tab, wait_seconds).await;
    }
    let Some((x, y)) = point_of(&target) else {
        return Ok(target);
    };
    page.click(x, y).await?;
    wait_notifications(page, tab, wait_seconds).await
}

async fn wait_notifications(
    page: &PageSession,
    tab: &str,
    wait_seconds: f64,
) -> anyhow::Result<Value> {
    let deadline = Instant::now() + Duration::from_secs_f64(wait_seconds);
    let args = json!({ "tab": tab });
    let mut latest = json!({ "ok": false, "status": "waiting" });
    while Instant::now() < deadline {
        latest = crate::sites::learning::run_site_browser_tool(
            page,
            SITE_ID,
            "notificationsState",
            Some(&args),
        )
        .await?;
        if latest.get("ok").and_then(Value::as_bool) == Some(true) || gate_reason(&latest).is_some()
        {
            return Ok(latest);
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    Ok(latest)
}

async fn poll_owned_target(
    page: &PageSession,
    tool_name: &str,
    seconds: f64,
) -> anyhow::Result<Value> {
    let deadline = Instant::now() + Duration::from_secs_f64(seconds.clamp(1.0, 12.0));
    let mut latest = json!({ "ok": false, "status": "waiting" });
    while Instant::now() < deadline {
        latest =
            crate::sites::learning::run_site_browser_tool(page, SITE_ID, tool_name, None).await?;
        if point_of(&latest).is_some() || gate_reason(&latest).is_some() {
            return Ok(latest);
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    Ok(latest)
}

async fn open_followers_tab(
    page: &PageSession,
    wait_seconds: f64,
) -> anyhow::Result<Option<Value>> {
    let target = poll_owned_target(page, "followersTabTarget", 8.0).await?;
    if target.get("selected").and_then(Value::as_bool) == Some(true) {
        let state =
            wait_for_browser_tool(page, SITE_ID, "followersState", None, wait_seconds).await?;
        return Ok(Some(state));
    }
    let Some((x, y)) = point_of(&target) else {
        return Ok(None);
    };
    page.click(x, y).await?;
    let state = wait_for_browser_tool(page, SITE_ID, "followersState", None, wait_seconds).await?;
    if state.get("ok").and_then(Value::as_bool) == Some(true) || gate_reason(&state).is_some() {
        return Ok(Some(state));
    }
    Ok(None)
}

async fn open_followers(page: &PageSession, wait_seconds: f64) -> anyhow::Result<Value> {
    let mut url = current_url(page).await.unwrap_or_default();
    let (on_x, _) = on_x_home(&url);
    if !on_x {
        navigate_https(page, HOME_URL).await?;
        url = current_url(page).await.unwrap_or_default();
    } else if followers_path(&url) {
        return wait_for_browser_tool(page, SITE_ID, "followersState", None, wait_seconds).await;
    }
    if !followers_path(&url) && !url.contains("/verified_followers") {
        let _ = click_named_nav(page, "profile").await?;
    }
    let mut username = String::new();
    let count = poll_owned_target(page, "followersLinkTarget", 8.0).await?;
    if let Some(value) = count.get("username").and_then(Value::as_str) {
        username = value.to_string();
    }
    if let Some((x, y)) = point_of(&count) {
        page.click(x, y).await?;
        if let Some(state) = open_followers_tab(page, wait_seconds).await? {
            return Ok(state);
        }
    } else if url.contains("/verified_followers") {
        if let Some(state) = open_followers_tab(page, wait_seconds).await? {
            return Ok(state);
        }
    }
    if username.is_empty() {
        let target = crate::sites::learning::run_site_browser_tool(
            page,
            SITE_ID,
            "navLinkTarget",
            Some(&json!({ "name": "profile" })),
        )
        .await?;
        username = target
            .get("username")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
    }
    if !username
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        || username.is_empty()
    {
        return Ok(json!({ "ok": false, "status": "actor_not_found" }));
    }
    navigate_https(page, &format!("https://x.com/{username}/followers")).await?;
    wait_for_browser_tool(page, SITE_ID, "followersState", None, wait_seconds).await
}

fn followers_path(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    let host = parsed.host_str().unwrap_or("");
    if !(host == "x.com" || host.ends_with(".x.com")) {
        return false;
    }
    let parts: Vec<&str> = parsed
        .path()
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    parts.len() == 2 && parts[1].eq_ignore_ascii_case("followers")
}

fn normalize_label(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '_' && *ch != '-')
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn x_notifications_tab(raw: &str) -> anyhow::Result<&'static str> {
    match normalize_label(raw).as_str() {
        "" | "all" => Ok("all"),
        "mentions" => Ok("mentions"),
        _ => anyhow::bail!("tab must be all or mentions"),
    }
}

fn on_x_home(url: &str) -> (bool, bool) {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return (false, false);
    };
    let host = parsed.host_str().unwrap_or("");
    let on_x = host == "x.com" || host.ends_with(".x.com");
    let on_home = parsed.path() == "/home" || parsed.path() == "/home/";
    (on_x, on_home)
}

async fn click_home_link(page: &PageSession) -> anyhow::Result<bool> {
    let target =
        crate::sites::learning::run_site_browser_tool(page, SITE_ID, "homeLinkTarget", None)
            .await?;
    let Some((x, y)) = point_of(&target) else {
        return Ok(false);
    };
    page.click(x, y).await?;
    Ok(true)
}

/// Reach the home timeline composer by clicking Home when already on X.
/// A URL load is only used to enter the site.
async fn reveal_home_composer(page: &PageSession) -> anyhow::Result<Value> {
    let url = current_url(page).await.unwrap_or_default();
    let (on_x, on_home) = on_x_home(&url);
    if !on_x {
        navigate_https(page, HOME_URL).await?;
    } else if !on_home {
        let _ = click_home_link(page).await?;
    }
    let composer =
        crate::sites::learning::run_site_browser_tool(page, SITE_ID, "postComposerTarget", None)
            .await?;
    if composer.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(composer);
    }
    let status = composer.get("status").and_then(Value::as_str).unwrap_or("");
    if status == "post_editor_obscured" || status == "not_home" {
        let _ = click_home_link(page).await?;
    }
    wait_for_browser_tool(page, SITE_ID, "postComposerTarget", None, 8.0).await
}

async fn close_post_composer(page: &PageSession) {
    let Ok(close) =
        crate::sites::learning::run_site_browser_tool(page, SITE_ID, "postCloseTarget", None).await
    else {
        return;
    };
    let Some((x, y)) = point_of(&close) else {
        return;
    };
    let _ = page.click(x, y).await;
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
        Some("post_not_found" | "post_link_not_visible" | "post_link_obscured")
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
    true
}

fn x_source_surface_anchor_matches(source_state: &Value, current_state: &Value) -> bool {
    let source_ids = source_state
        .get("post_ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str);
    let current_ids = current_state
        .get("post_ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<std::collections::HashSet<_>>();
    !current_ids.is_empty() && source_ids.into_iter().any(|id| current_ids.contains(id))
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

    let expected_y = source_state
        .get("scroll_y")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0);
    let scroll_deadline = Instant::now() + Duration::from_secs_f64(wait_seconds.clamp(1.0, 15.0));
    let (final_url, final_state, final_y, scroll_restored, anchor_restored) = loop {
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
        let scroll_restored = expected_y.abs_diff(observed_y) <= 8;
        let anchor_restored = x_source_surface_anchor_matches(source_state, &observed_state);
        if (scroll_restored && anchor_restored) || Instant::now() >= scroll_deadline {
            break (
                observed_url,
                observed_state,
                observed_y,
                scroll_restored,
                anchor_restored,
            );
        }
        state = observed_state;
    };
    let restored = scroll_restored
        && anchor_restored
        && x_source_surface_identity_matches(source_url, source_state, &final_url, &final_state);
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
        "anchor_restored": anchor_restored,
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
        match crate::sites::learning::run_site_browser_tool(page, SITE_ID, "pageState", None).await
        {
            Ok(mut value) => {
                let observed_id = value.get("url").and_then(Value::as_str).and_then(x_post_id);
                let valid = value.get("ok").and_then(Value::as_bool) == Some(true)
                    && observed_id.as_deref() == Some(post_id);
                if let Some(object) = value.as_object_mut() {
                    object.insert("post_id".into(), json!(observed_id.unwrap_or_default()));
                }
                final_state = value;
                if !valid {
                    stage_errors.insert(
                        "final_state".into(),
                        json!("X post route changed or became unavailable during reply collection"),
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

#[async_trait]
impl Tool for WaitForXLoginTool {
    fn name(&self) -> &str {
        "wait_for_x_login"
    }

    fn description(&self) -> &str {
        "After an X tool reports login_required, keep the current X tab open and wait for the \
         user to sign in. Returns logged_in true only after the live page exposes the signed-in \
         account control; then retry the original read-only tool."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "timeout_seconds": {
                    "type": "integer",
                    "description": "Seconds to wait before returning (default 180, max 600)."
                }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        if self.page.is_remote_browser() && !interactive_remote_login_enabled() {
            return Ok(json_result(&json!({
                "logged_in": false,
                "remote_browser": true,
                "message": "Hosted X login is unavailable in this browser session.",
            })));
        }
        ensure_site_page(&self.page, HOST_ROOT, HOME_URL).await?;
        let timeout = get_i64(&input, "timeout_seconds", WAIT_FOR_X_LOGIN_DEFAULT_SECS)
            .clamp(10, WAIT_FOR_X_LOGIN_MAX_SECS);
        let deadline = Instant::now() + Duration::from_secs(timeout as u64);
        let mut human_ready = login_resume_receiver(&ctx.run_id);
        loop {
            let state = match crate::sites::learning::run_site_browser_tool(
                &self.page,
                SITE_ID,
                "pageState",
                None,
            )
            .await
            {
                Ok(state) => state,
                Err(error) => {
                    if self.page.transport_closed().await {
                        return Err(error);
                    }
                    Value::Null
                }
            };
            if state.get("authenticated").and_then(Value::as_bool) == Some(true)
                && state.get("login_required").and_then(Value::as_bool) != Some(true)
            {
                return Ok(json_result(&json!({
                    "logged_in": true,
                    "message": "X login detected. Re-run the original tool and continue.",
                })));
            }
            if Instant::now() >= deadline {
                return Ok(json_result(&json!({
                    "logged_in": false,
                    "timed_out": true,
                    "message": format!(
                        "X login was not detected within {timeout}s. Ask the user to sign in in \
                         the connected browser, then call wait_for_x_login again."
                    ),
                })));
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                changed = human_ready.changed() => {
                    if changed.is_err() {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        }
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

fn x_search_deep_read_supported(filter: &str) -> bool {
    matches!(filter, "top" | "latest")
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
