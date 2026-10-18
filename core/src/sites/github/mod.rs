//! Read-only GitHub research over HTTP. One tool, no browser session.
mod client;
mod html;
pub mod preview;

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::{Tool, ToolContext, ToolResult};
use crate::sites::registry::{ArgKind, CommandArg, NativeSiteAdapter, SiteCommand, SlowWhen};

struct GithubRequest;

#[async_trait]
impl Tool for GithubRequest {
    fn name(&self) -> &str {
        "github_request"
    }
    fn description(&self) -> &str {
        "Read GitHub using REST GET, GraphQL queries, or public HTML/text. Relative URLs use api.github.com; full GitHub URLs read pages, including Trending and Topics. Supply query for GraphQL. No Chrome. Credentials are resolved by the host, never supplied in tool input. Returns source/time, pagination, rate limits, explicit truncation and a full local response. Repository lists are compact by default; request fields for extra metadata."
    }
    fn defer_until_site(&self) -> &str {
        "github"
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","additionalProperties":false,"properties":{
            "url":{"type":"string","description":"REST path such as /search/repositories or full https://github.com/trending?since=weekly; defaults to /graphql when query is supplied. Also accepts raw.githubusercontent.com."},
            "query":{"type":"string","description":"Read-only GraphQL query. Requires host GitHub authentication; mutations and subscriptions are rejected."},
            "params":{"type":"object","additionalProperties":{"type":"string"},"description":"URL query parameters; put search syntax in q. Example: {q:'personal agent in:name,description,readme fork:false',sort:'updated',per_page:'10'}."},
            "variables":{"type":"object","description":"GraphQL variables as a JSON object."},
            "select":{"type":"string","description":"Optional JSON Pointer into the full response, before projection/paging. Example /data/repository/discussion/body or /items. README/contents base64 is decoded as content before selection."},
            "fields":{"type":"array","items":{"type":"string"},"description":"Optional top-level record fields (dot paths allowed, e.g. owner.login). Applied to REST items/array records or the selected object. Omit for compact repository metadata; GraphQL already selects fields."},
            "limit":{"type":"integer","minimum":1,"maximum":100,"description":"Maximum returned items per array, default 20. Does not fetch subsequent server pages."},
            "offset":{"type":"integer","minimum":0,"description":"Local offset in a selected text (Unicode characters) or selected/top-level list; default 0. Follow next_offset or server pagination as appropriate."},
            "max_chars":{"type":"integer","minimum":100,"maximum":40000,"description":"Text page/output string budget, default 18000. JSON truncation lists paths; select a clipped field to page through it."}
        }})
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        match client::execute(input, ctx.output_dir()).await {
            Ok(value) if value["ok"] == false => Ok(ToolResult::failure(value.to_string())),
            Ok(value) => {
                crate::sites::post_archive::persist_site_tool_result(
                    ctx,
                    "github",
                    self.name(),
                    &value,
                    None,
                );
                Ok(ToolResult::text(value.to_string()))
            }
            Err(error) => Ok(ToolResult::failure(
                json!({"ok":false,"reason":"github_request_failed","message":format!("{error:#}")})
                    .to_string(),
            )),
        }
    }
}

pub fn github_tools() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(GithubRequest)]
}

async fn run(
    input: Value,
    progress: Option<crate::agent::tool::ToolProgressSender>,
) -> anyhow::Result<Value> {
    crate::sites::runner::run_tool_command_without_browser(
        crate::sites::runner::ToolCommand {
            site_id: "github",
            command_name: "request",
            tool_name: "github_request",
            before: None,
            after: None,
            include_run_metadata: false,
        },
        &github_tools(),
        input,
        progress,
    )
    .await
}

macro_rules! arg {
    ($key:literal, $long:expr, $kind:ident, $help:literal) => {
        CommandArg {
            key: $key,
            long: $long,
            value_name: $key,
            help: $help,
            required: false,
            kind: ArgKind::$kind,
        }
    };
}

static COMMANDS: &[SiteCommand] = &[SiteCommand {
    name: "request",
    tool_name: "github_request",
    about: "Read GitHub REST, GraphQL or public pages without Chrome",
    args: &[
        arg!(
            "url",
            None,
            Str,
            "REST path or full GitHub URL; optional for GraphQL"
        ),
        arg!("query", Some("query"), Str, "Read-only GraphQL query"),
        arg!(
            "params",
            Some("param"),
            KeyValueMap,
            "URL parameter key=value (repeatable)"
        ),
        arg!(
            "variables",
            Some("variables"),
            Str,
            "GraphQL variables as a JSON object"
        ),
        arg!("select", Some("select"), Str, "JSON Pointer into response"),
        arg!(
            "fields",
            Some("field"),
            StrList,
            "Record field or dot path (repeatable)"
        ),
        arg!(
            "limit",
            Some("limit"),
            Int,
            "Maximum items per array, 1–100 (default 20)"
        ),
        arg!(
            "offset",
            Some("offset"),
            Int,
            "Local list/text offset (default 0)"
        ),
        arg!(
            "max_chars",
            Some("max-chars"),
            Int,
            "Output character budget, 100–40000 (default 18000)"
        ),
    ],
    slow: SlowWhen::Always,
    run: |_, input, _, progress| Box::pin(run(input, progress)),
}];

pub static GITHUB_NATIVE_ADAPTER: NativeSiteAdapter = NativeSiteAdapter {
    id: "github",
    about: "Discover GitHub projects, trends, people and discussions via HTTP",
    home_url: "https://github.com",
    browserless_tools: Some(|_, _| github_tools()),
    runtime_command: Some(|_, _, input, _, progress| Box::pin(run(input, progress))),
    agent_tools: |_, _| Box::pin(async { Ok(github_tools()) }),
    default_agent_tools: None,
    agent_instructions: |preamble| {
        crate::sites::learning::site_agent_instructions("github", preamble)
    },
    default_agent_instructions: None,
    commands: COMMANDS,
};
