use std::sync::Arc;
use std::time::{Duration, Instant};

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
use crate::sites::runner::{get_f64, get_i64, get_str, json_result, ToolCommand};
use crate::sites::skill_cli::{
    current_url, ensure_site_page, failure_payload, gate_reason, invoke_browser_tool,
    navigate_https, percent_encode_query, run_skill_command, wait_for_browser_tool,
    DEFAULT_COMMENT_COUNT, DEFAULT_RESULT_COUNT, DEFAULT_WAIT_SECONDS, MAX_TOOL_ITEMS,
    MAX_TOOL_WAIT_SECONDS,
};

const SITE_ID: &str = "linkedin";
const HOME_URL: &str = "https://www.linkedin.com/";
const HOST_ROOT: &str = "linkedin.com";

pub async fn linkedin_agent_tools(
    page: Arc<PageSession>,
    _llm_provider: Arc<dyn LlmProvider>,
) -> anyhow::Result<Vec<Arc<dyn Tool>>> {
    Ok(linkedin_tools(page))
}

pub fn linkedin_agent_instructions(extra: &str) -> String {
    crate::sites::learning::site_agent_instructions(SITE_ID, extra)
}

fn linkedin_tools(page: Arc<PageSession>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(SearchTool { page: page.clone() }),
        Arc::new(ProfileTool { page: page.clone() }),
        Arc::new(HistoryTool { page: page.clone() }),
        Arc::new(CompanyTool { page: page.clone() }),
        Arc::new(CompanyPeopleTool { page: page.clone() }),
        Arc::new(RelatedPeopleTool { page: page.clone() }),
        Arc::new(GetPostsTool { page: page.clone() }),
        Arc::new(CommentTool { page: page.clone() }),
        Arc::new(PageStateTool { page }),
    ]
}

pub static LINKEDIN_NATIVE_ADAPTER: NativeSiteAdapter = NativeSiteAdapter {
    id: SITE_ID,
    about: "LinkedIn (linkedin.com)",
    home_url: "",
    agent_tools: |page, llm| Box::pin(linkedin_agent_tools(page, llm)),
    default_agent_tools: None,
    agent_instructions: linkedin_agent_instructions,
    default_agent_instructions: None,
    commands: &[
        SiteCommand {
            name: "search",
            tool_name: "search",
            about: "Search LinkedIn people, companies, or content and print result cards as JSON.",
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
                    key: "result_type",
                    long: Some("type"),
                    value_name: "TYPE",
                    help: "Result type: people, content, companies, or all. Defaults to people.",
                    required: false,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "num",
                    long: Some("num"),
                    value_name: "N",
                    help: "Number of results to collect by scrolling. Defaults to 10.",
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
            about: "Read a LinkedIn profile landing page.",
            args: &[
                CommandArg {
                    key: "profile",
                    long: None,
                    value_name: "ID_OR_URL",
                    help: "LinkedIn profile id or /in/ URL.",
                    required: true,
                    kind: ArgKind::Str,
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
            name: "history",
            tool_name: "history",
            about: "Read complete visible work or education history from a LinkedIn profile details route.",
            args: &[
                CommandArg {
                    key: "profile",
                    long: None,
                    value_name: "ID_OR_URL",
                    help: "LinkedIn profile id or /in/ URL.",
                    required: true,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "section",
                    long: Some("section"),
                    value_name: "SECTION",
                    help: "History section: experience or education. Defaults to experience.",
                    required: false,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for the details page to hydrate. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_history,
        },
        SiteCommand {
            name: "company",
            tool_name: "company",
            about: "Read a LinkedIn company or showcase landing page.",
            args: &[
                CommandArg {
                    key: "company",
                    long: None,
                    value_name: "ID_OR_URL",
                    help: "LinkedIn company id or /company/ or /showcase/ URL.",
                    required: true,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for the company page to hydrate. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_company,
        },
        SiteCommand {
            name: "company-people",
            tool_name: "company_people",
            about: "Read the visible People you may know suggestions on a LinkedIn company people page.",
            args: &[
                CommandArg {
                    key: "company",
                    long: None,
                    value_name: "ID_OR_URL",
                    help: "LinkedIn company id or company/people URL.",
                    required: true,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "num",
                    long: Some("num"),
                    value_name: "N",
                    help: "Maximum visible profile suggestions to return. Defaults to 10.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for the company people page to hydrate. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_company_people,
        },
        SiteCommand {
            name: "related-people",
            tool_name: "related_people",
            about: "Read explicitly labelled related-people sections on a LinkedIn profile or company page.",
            args: &[
                CommandArg {
                    key: "target",
                    long: None,
                    value_name: "ID_OR_URL",
                    help: "LinkedIn profile id, company id, or page URL.",
                    required: true,
                    kind: ArgKind::Str,
                },
                CommandArg {
                    key: "num",
                    long: Some("num"),
                    value_name: "N",
                    help: "Maximum visible related profiles to return. Defaults to 10.",
                    required: false,
                    kind: ArgKind::Int,
                },
                CommandArg {
                    key: "wait_seconds",
                    long: Some("wait-seconds"),
                    value_name: "SECONDS",
                    help: "Maximum wait for the page to hydrate. Defaults to 30.",
                    required: false,
                    kind: ArgKind::Int,
                },
            ],
            slow: SlowWhen::Always,
            run: run_related_people,
        },
        SiteCommand {
            name: "get-posts",
            tool_name: "get_posts",
            about: "Read LinkedIn posts by URL, including comments.",
            args: &[
                CommandArg {
                    key: "posts",
                    long: Some("post"),
                    value_name: "URL",
                    help: "LinkedIn post or feed-update URL. Repeat to read multiple posts.",
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
            about: "Comment on one LinkedIn post using verified pointer and keyboard events.",
            args: &[
                CommandArg {
                    key: "post",
                    long: None,
                    value_name: "URL",
                    help: "Target LinkedIn /posts/ or /feed/update/ URL.",
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
            about: "Open or reuse LinkedIn and print page state as JSON.",
            args: &[CommandArg {
                key: "wait_seconds",
                long: Some("wait-seconds"),
                value_name: "SECONDS",
                help: "Maximum wait for a LinkedIn page. Defaults to 30.",
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

fn run_history(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    run_named(page, args, debug_snapshot, progress, "history", "history")
}

fn run_company(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    run_named(page, args, debug_snapshot, progress, "company", "company")
}

fn run_company_people(
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
        "company-people",
        "company_people",
    )
}

fn run_related_people(
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
        "related-people",
        "related_people",
    )
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

fn run_comment(
    page: Arc<PageSession>,
    args: Value,
    debug_snapshot: bool,
    progress: Option<ToolProgressSender>,
) -> BoxFuture<Value> {
    run_named(page, args, debug_snapshot, progress, "comment", "comment")
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
        linkedin_tools(page),
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
        "Search LinkedIn people, companies, or content matching `query`."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "maxLength": 512 },
                "result_type": { "type": "string" },
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
        let result_type =
            normalize_result_type(get_str(&input, "result_type").unwrap_or("people"))?;
        let num = get_i64(&input, "num", DEFAULT_RESULT_COUNT).clamp(1, MAX_TOOL_ITEMS);
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        let target = format!(
            "https://www.linkedin.com/search/results/{result_type}/?keywords={}",
            percent_encode_query(&query)
        );
        navigate_https(&self.page, &target).await?;
        let state_args = json!({ "query": query, "result_type": result_type });
        let state = wait_for_browser_tool(
            &self.page,
            SITE_ID,
            "searchState",
            Some(&state_args),
            wait_seconds,
        )
        .await?;
        if let Some(reason) = gate_reason(&state) {
            return Ok(json_result(&failure_payload(
                reason,
                json!({
                    "query": query,
                    "result_type": result_type,
                    "state": state,
                    "count": 0,
                    "results": [],
                }),
            )));
        }
        if !state.get("ok").and_then(Value::as_bool).unwrap_or(false)
            && !state.get("empty").and_then(Value::as_bool).unwrap_or(false)
        {
            return Ok(json_result(&failure_payload(
                state
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("search_results_unavailable"),
                json!({
                    "query": query,
                    "result_type": result_type,
                    "state": state,
                    "count": 0,
                    "results": [],
                }),
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
                Some(&json!({ "limit": num, "result_type": result_type })),
                true,
            )
            .await?
        };
        Ok(json_result(&json!({
            "ok": true,
            "query": query,
            "result_type": result_type,
            "url": current_url(&self.page).await.unwrap_or_default(),
            "count": results.as_array().map(Vec::len).unwrap_or(0),
            "results": results,
            "state": state,
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
        "Read a LinkedIn profile landing page by id or URL."
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
        let url = linkedin_profile_url(&locator)?;
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        Ok(json_result(
            &read_object_page(&self.page, &url, "profileDetail", wait_seconds).await?,
        ))
    }
}

struct HistoryTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for HistoryTool {
    fn name(&self) -> &str {
        "history"
    }

    fn description(&self) -> &str {
        "Read complete visible work or education entries from a LinkedIn profile details route."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "profile": { "type": "string" },
                "section": { "type": "string" },
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1, "maximum": 330 }
            },
            "required": ["profile"]
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let locator = required_string(&input, "profile")?;
        let section =
            normalize_history_section(get_str(&input, "section").unwrap_or("experience"))?;
        let url = linkedin_history_url(&locator, section)?;
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        Ok(json_result(
            &read_object_page(&self.page, &url, "profileHistory", wait_seconds).await?,
        ))
    }
}

struct CompanyTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for CompanyTool {
    fn name(&self) -> &str {
        "company"
    }

    fn description(&self) -> &str {
        "Read a LinkedIn company or showcase landing page by id or URL."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "company": { "type": "string" },
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1, "maximum": 330 }
            },
            "required": ["company"]
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let locator = required_string(&input, "company")?;
        let url = linkedin_company_url(&locator, false)?;
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        Ok(json_result(
            &read_object_page(&self.page, &url, "companyDetail", wait_seconds).await?,
        ))
    }
}

struct CompanyPeopleTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for CompanyPeopleTool {
    fn name(&self) -> &str {
        "company_people"
    }

    fn description(&self) -> &str {
        "Read visible People you may know suggestions on a LinkedIn company people page."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "company": { "type": "string" },
                "num": { "type": "integer", "default": 10, "minimum": 1, "maximum": 100 },
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1, "maximum": 330 }
            },
            "required": ["company"]
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let locator = required_string(&input, "company")?;
        let url = linkedin_company_url(&locator, true)?;
        let num = get_i64(&input, "num", DEFAULT_RESULT_COUNT).clamp(1, MAX_TOOL_ITEMS);
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        navigate_https(&self.page, &url).await?;
        let state = wait_for_browser_tool(
            &self.page,
            SITE_ID,
            "companyPeople",
            Some(&json!({ "limit": num })),
            wait_seconds,
        )
        .await?;
        if let Some(reason) = gate_reason(&state) {
            return Ok(json_result(&failure_payload(
                reason,
                json!({ "company": locator, "url": url, "state": state }),
            )));
        }
        let people = invoke_browser_tool(
            &self.page,
            ctx,
            SITE_ID,
            "companyPeople",
            Some(&json!({ "limit": num })),
            false,
        )
        .await?;
        Ok(json_result(&people))
    }
}

struct RelatedPeopleTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for RelatedPeopleTool {
    fn name(&self) -> &str {
        "related_people"
    }

    fn description(&self) -> &str {
        "Read explicitly labelled related-people sections on a LinkedIn profile or company page."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target": { "type": "string" },
                "num": { "type": "integer", "default": 10, "minimum": 1, "maximum": 100 },
                "wait_seconds": { "type": "number", "default": 30, "minimum": 1, "maximum": 330 }
            },
            "required": ["target"]
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let locator = required_string(&input, "target")?;
        let url = linkedin_related_url(&locator)?;
        let num = get_i64(&input, "num", DEFAULT_RESULT_COUNT).clamp(1, MAX_TOOL_ITEMS);
        let wait_seconds =
            get_f64(&input, "wait_seconds", DEFAULT_WAIT_SECONDS).clamp(1.0, MAX_TOOL_WAIT_SECONDS);
        navigate_https(&self.page, &url).await?;
        let _ = wait_for_browser_tool(
            &self.page,
            SITE_ID,
            "relatedPeople",
            Some(&json!({ "limit": num })),
            wait_seconds,
        )
        .await?;
        let people = invoke_browser_tool(
            &self.page,
            ctx,
            SITE_ID,
            "relatedPeople",
            Some(&json!({ "limit": num })),
            false,
        )
        .await?;
        if let Some(reason) = gate_reason(&people) {
            return Ok(json_result(&failure_payload(
                reason,
                json!({ "target": locator, "url": url, "state": people }),
            )));
        }
        Ok(json_result(&people))
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
        "Read one or more LinkedIn posts by URL, including comments."
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
                .ok_or_else(|| anyhow::anyhow!("each --post must be a non-empty URL"))?;
            items.push(
                read_linkedin_post(&self.page, ctx, locator, num_comments, wait_seconds).await?,
            );
        }
        Ok(json_result(&json!({
            "ok": items.iter().all(|item| item.get("ok").and_then(Value::as_bool) == Some(true)),
            "count": items.len(),
            "posts": items,
        })))
    }
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
        "Comment on an explicitly selected LinkedIn post with real CDP pointer and keyboard events. Refuses login gates, ambiguous editors, existing drafts or exact comments, route changes, and submit retries."
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
        let url = linkedin_post_url(&locator)?;
        let expected_post_id = linkedin_post_id_from_url(&url)
            .ok_or_else(|| anyhow::anyhow!("LinkedIn post URL is missing an activity id"))?;
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
            || page_state
                .get("login_gate_present")
                .and_then(Value::as_bool)
                == Some(true)
        {
            return Ok(json_result(&failure_payload(
                "login_required",
                json!({ "post": locator, "url": url, "detail": detail, "page_state": page_state, "submit_click_count": 0 }),
            )));
        }
        let post_id = detail
            .get("post_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if detail.get("ok").and_then(Value::as_bool) != Some(true) || post_id != expected_post_id {
            return Ok(json_result(&failure_payload(
                if detail.get("ok").and_then(Value::as_bool) == Some(true) {
                    "wrong_post"
                } else {
                    detail
                        .get("status")
                        .or_else(|| detail.get("error"))
                        .and_then(Value::as_str)
                        .unwrap_or("post_unavailable")
                },
                json!({ "post": locator, "expected_post_id": expected_post_id, "url": url, "detail": detail, "page_state": page_state, "submit_click_count": 0 }),
            )));
        }
        let action_args = json!({ "post_id": post_id });
        let rendered_args = json!({ "post_id": post_id, "text": text });
        let before = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "renderedCommentState",
            Some(&rendered_args),
        )
        .await?;
        if before.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(json_result(&failure_payload(
                before
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("comment_preflight_failed"),
                json!({ "post_id": post_id, "url": url, "reconcile": before, "submit_click_count": 0 }),
            )));
        }
        let actor_value = before.get("actor").cloned().unwrap_or(Value::Null);
        let actor = ActionActor {
            id: actor_value
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            display_name: actor_value
                .get("display_name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        };
        if actor.id.is_empty() || actor.display_name.is_empty() {
            return Ok(json_result(&failure_payload(
                "current_user_unknown",
                json!({ "post_id": post_id, "url": url, "reconcile": before, "submit_click_count": 0 }),
            )));
        }
        let target_url = format!("https://www.linkedin.com/feed/update/urn:li:activity:{post_id}/");
        let idempotency_key = format!("linkedin:comment:{}:{post_id}:{text}", actor.id);
        let store = ActionStore::open_default();
        let mut receipt = store.create_draft(
            &idempotency_key,
            "linkedin",
            SocialActionKind::Comment,
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
                    "comment": text, "submit_click_count": 0, "receipt": receipt,
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
                        "comment": text, "submit_click_count": 0, "receipt": receipt,
                    })));
                }
                return Ok(json_result(&json!({
                    "ok": false, "status": "commit_unknown", "reason": "a submit attempt was already reserved; reconcile instead of retrying",
                    "action_id": receipt.action_id(), "post_id": post_id, "url": url,
                    "comment": text, "submit_click_count": 0, "receipt": receipt,
                })));
            }
            SocialActionStatus::Prepared => {
                receipt = store.reset_prepared(receipt.action_id(), &actor.id, post_id)?;
            }
            SocialActionStatus::Draft => {}
        }
        let baseline = before.get("count").and_then(Value::as_u64).unwrap_or(0);
        if before
            .get("total_exact_count")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0
        {
            return Ok(json_result(&failure_payload(
                "exact_comment_preexists",
                json!({
                    "post_id": post_id,
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
        let Some((editor_x, editor_y)) = verified_linkedin_write_target(&editor, post_id) else {
            return Ok(json_result(&failure_payload(
                editor
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("comment_editor_unavailable"),
                json!({ "post_id": post_id, "url": url, "editor": editor, "submit_click_count": 0 }),
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
                json!({ "post_id": post_id, "url": url, "draft": draft, "submit_click_count": 0 }),
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
                json!({ "post_id": post_id, "url": url, "draft": typed, "submit_click_count": 0 }),
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
            || final_page_state
                .get("login_gate_present")
                .and_then(Value::as_bool)
                == Some(true)
        {
            return Ok(json_result(&failure_payload(
                gate_reason(&final_page_state).unwrap_or("page_gate_before_submit"),
                json!({ "post_id": post_id, "url": url, "page_state": final_page_state, "submit_click_count": 0 }),
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
        let Some((submit_x, submit_y)) = verified_linkedin_write_target(&submit, post_id) else {
            return Ok(json_result(&failure_payload(
                submit
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("comment_submit_unavailable"),
                json!({ "post_id": post_id, "url": url, "submit": submit, "submit_click_count": 0 }),
            )));
        };
        if final_detail.get("ok").and_then(Value::as_bool) != Some(true)
            || final_detail.get("post_id").and_then(Value::as_str) != Some(post_id)
            || final_draft.get("value").and_then(Value::as_str) != Some(text.as_str())
        {
            return Ok(json_result(&failure_payload(
                "volatile_state_changed_before_submit",
                json!({ "post_id": post_id, "url": url, "page_state": final_page_state, "detail": final_detail, "draft": final_draft, "submit_click_count": 0 }),
            )));
        }

        let final_rendered = crate::sites::learning::run_site_browser_tool(
            &self.page,
            SITE_ID,
            "renderedCommentState",
            Some(&rendered_args),
        )
        .await?;
        if final_rendered.get("ok").and_then(Value::as_bool) != Some(true)
            || final_rendered
                .get("actor")
                .and_then(|value| value.get("id"))
                .and_then(Value::as_str)
                != Some(actor.id.as_str())
            || final_rendered
                .get("total_exact_count")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                > 0
        {
            return Ok(json_result(&failure_payload(
                "actor_or_comment_state_changed_before_submit",
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
                "renderedCommentState",
                Some(&rendered_args),
            )
            .await
            {
                Ok(state) => {
                    if state
                        .get("actor")
                        .and_then(|value| value.get("id"))
                        .and_then(Value::as_str)
                        != Some(actor.id.as_str())
                    {
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
            "comment": text,
            "interaction": "trusted_pointer_and_keyboard",
            "platform_api_called": false,
            "submit_click_count": 1,
            "dispatch_error": dispatch_error,
            "reconcile_error": reconcile_error,
            "receipt_error": receipt_error,
            "receipt": persisted_receipt,
            "baseline_exact_comment_count": baseline,
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

fn verified_linkedin_write_target(target: &Value, post_id: &str) -> Option<(f64, f64)> {
    if target.get("ok").and_then(Value::as_bool) != Some(true)
        || target.get("hit_owned").and_then(Value::as_bool) != Some(true)
        || target.get("post_id").and_then(Value::as_str) != Some(post_id)
    {
        return None;
    }
    Some((target.get("x")?.as_f64()?, target.get("y")?.as_f64()?))
}

struct PageStateTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for PageStateTool {
    fn name(&self) -> &str {
        "page_state"
    }

    fn description(&self) -> &str {
        "Open or reuse LinkedIn and return the current route, login gate, and hydration state."
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

async fn read_object_page(
    page: &PageSession,
    url: &str,
    tool_name: &str,
    wait_seconds: f64,
) -> anyhow::Result<Value> {
    navigate_https(page, url).await?;
    let state = wait_for_browser_tool(page, SITE_ID, tool_name, None, wait_seconds).await?;
    if let Some(reason) = gate_reason(&state) {
        return Ok(failure_payload(
            reason,
            json!({ "url": url, "state": state }),
        ));
    }
    if state.get("ok").and_then(Value::as_bool) != Some(true) {
        return Ok(failure_payload(
            state
                .get("status")
                .and_then(Value::as_str)
                .or_else(|| state.get("error").and_then(Value::as_str))
                .unwrap_or("page_unavailable"),
            json!({ "url": url, "state": state }),
        ));
    }
    Ok(state)
}

async fn read_linkedin_post(
    page: &PageSession,
    ctx: &ToolContext,
    locator: &str,
    num_comments: i64,
    wait_seconds: f64,
) -> anyhow::Result<Value> {
    let url = linkedin_post_url(locator)?;
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
    Ok(json!({
        "ok": true,
        "input": locator,
        "url": current_url(page).await.unwrap_or(url),
        "entity": entity,
        "comments": comments,
    }))
}

fn normalize_result_type(value: &str) -> anyhow::Result<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "people" => Ok("people"),
        "content" => Ok("content"),
        "companies" | "company" => Ok("companies"),
        "all" => Ok("all"),
        other => anyhow::bail!("unsupported LinkedIn search type: {other}"),
    }
}

fn normalize_history_section(value: &str) -> anyhow::Result<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "experience" | "work" => Ok("experience"),
        "education" => Ok("education"),
        other => anyhow::bail!("unsupported LinkedIn history section: {other}"),
    }
}

fn linkedin_https_url(locator: &str) -> Option<String> {
    reqwest::Url::parse(locator.trim())
        .ok()
        .filter(|url| {
            let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
            url.scheme() == "https"
                && url.username().is_empty()
                && url.password().is_none()
                && (host == "linkedin.com" || host.ends_with(".linkedin.com"))
        })
        .map(|url| url.to_string())
}

fn linkedin_slug(locator: &str) -> anyhow::Result<String> {
    let trimmed = locator.trim().trim_matches('/');
    if trimmed.is_empty()
        || trimmed.contains('/')
        || !trimmed.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '%')
        })
    {
        anyhow::bail!("invalid LinkedIn id: {locator}");
    }
    Ok(trimmed.to_string())
}

fn linkedin_profile_url(locator: &str) -> anyhow::Result<String> {
    if let Some(url) = linkedin_https_url(locator) {
        return Ok(url);
    }
    Ok(format!(
        "https://www.linkedin.com/in/{}/",
        linkedin_slug(locator)?
    ))
}

fn linkedin_history_url(locator: &str, section: &str) -> anyhow::Result<String> {
    if let Some(url) = linkedin_https_url(locator) {
        if url.contains("/details/") {
            return Ok(url);
        }
        let parsed = reqwest::Url::parse(&url)?;
        let id = parsed
            .path_segments()
            .and_then(|mut parts| {
                let mut found = None;
                while let Some(part) = parts.next() {
                    if part == "in" {
                        found = parts.next().map(ToOwned::to_owned);
                        break;
                    }
                }
                found
            })
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("LinkedIn history URL must include /in/<id>"))?;
        return Ok(format!(
            "https://www.linkedin.com/in/{id}/details/{section}/"
        ));
    }
    Ok(format!(
        "https://www.linkedin.com/in/{}/details/{section}/",
        linkedin_slug(locator)?
    ))
}

fn linkedin_company_url(locator: &str, people: bool) -> anyhow::Result<String> {
    if let Some(url) = linkedin_https_url(locator) {
        if !people {
            return Ok(url);
        }
        if url.contains("/people") {
            return Ok(url);
        }
        let trimmed = url.trim_end_matches('/');
        return Ok(format!("{trimmed}/people/"));
    }
    let slug = linkedin_slug(locator)?;
    if people {
        Ok(format!("https://www.linkedin.com/company/{slug}/people/"))
    } else {
        Ok(format!("https://www.linkedin.com/company/{slug}/"))
    }
}

fn linkedin_related_url(locator: &str) -> anyhow::Result<String> {
    if let Some(url) = linkedin_https_url(locator) {
        return Ok(url);
    }
    Ok(format!(
        "https://www.linkedin.com/in/{}/",
        linkedin_slug(locator)?
    ))
}

fn linkedin_post_url(locator: &str) -> anyhow::Result<String> {
    linkedin_https_url(locator)
        .filter(|url| {
            reqwest::Url::parse(url).ok().is_some_and(|parsed| {
                let path = parsed.path();
                path.starts_with("/posts/") || path.starts_with("/feed/update/")
            })
        })
        .ok_or_else(|| {
            anyhow::anyhow!("LinkedIn get-posts requires a /posts/ or /feed/update/ URL")
        })
}

fn linkedin_post_id_from_url(url: &str) -> Option<String> {
    [
        "activity-",
        "urn:li:activity:",
        "urn:li:ugcPost:",
        "urn:li:share:",
    ]
    .into_iter()
    .find_map(|marker| {
        let suffix = url.split(marker).nth(1)?;
        let id = suffix
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect::<String>();
        (!id.is_empty()).then_some(id)
    })
}
