//! Unified podcast discovery and progressive reading, without a browser/login.
mod client;
mod preview;
pub use preview::read as read_preview;

use crate::agent::{Tool, ToolContext, ToolResult};
use crate::sites::registry::{ArgKind, CommandArg, NativeSiteAdapter, SiteCommand, SlowWhen};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub const INSTRUCTIONS: &str = "Podcasts: use search_podcast_shows to resolve a show name, list_podcast_episodes for that show's episodes, and search_podcast_episodes for content keywords. These tools use public BidClub HTTP, without Chrome or login, across Chinese and international podcasts. Results contain metadata and TL;DR by default. Only request read_podcast_episode section=digest, transcript or full when the user's request needs that depth. Follow next_offset until null when complete text is required; offsets count Unicode characters. Cite only the returned BidClub url once per source; the preview contains the original source_url as a separate listening link. Do not append Spotify/YouTube/publisher URLs after the preview citation unless explicitly requested. Set language=ZH for Chinese requests or EN for English requests, including when query keywords are in another language. This selects translated summaries, not an original-language search filter; leave lang unset unless asked to filter sources. Preserve language_fallback; transcripts remain in the original language. This is BidClub's indexed library, not all podcasts. Always report partial/searched_recent and detail errors; zero search results may only cover a recent subset. Summaries and transcripts are processed text, not guaranteed verbatim or timestamp aligned.";

fn instructions(preamble: &str) -> String {
    format!("{preamble}\n\n{INSTRUCTIONS}")
}

struct PodcastTool {
    command: &'static str,
    language: Option<String>,
}
impl PodcastTool {
    fn spec(&self) -> &'static SiteCommand {
        COMMANDS
            .iter()
            .find(|c| c.name == self.command)
            .expect("registered podcast command")
    }
}
#[async_trait]
impl Tool for PodcastTool {
    fn name(&self) -> &str {
        self.spec().tool_name
    }
    fn description(&self) -> &str {
        self.spec().about
    }
    fn defer_until_site(&self) -> &str {
        "podcast"
    }
    fn input_schema(&self) -> Value {
        let mut properties = json!({});
        let mut required = Vec::new();
        for arg in self.spec().args {
            let kind = match arg.kind {
                ArgKind::Int => "integer",
                _ => "string",
            };
            properties[arg.key] = json!({"type":kind,"description":arg.help});
            if arg.required {
                required.push(arg.key);
            }
        }
        if properties.get("section").is_some() {
            properties["section"]["enum"] =
                json!(["metadata", "tldr", "digest", "transcript", "full"]);
        }
        json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
    }
    fn effective_input(&self, input: &Value) -> Value {
        let mut input = input.clone();
        if input.get("language").is_none() && self.command != "shows" {
            if let Some(language) = &self.language {
                input["language"] = json!(language);
            }
        }
        input
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        match client::execute(self.command, self.effective_input(&input)).await {
            Ok(value) => {
                crate::sites::post_archive::persist_site_tool_result(
                    ctx,
                    "podcast",
                    self.name(),
                    &value,
                    None,
                );
                Ok(ToolResult::text(serde_json::to_string(&value)?))
            }
            Err(error) => Ok(ToolResult::failure(
                json!({
                    "ok": false,
                    "reason": "podcast_request_failed",
                    "message": format!("Podcast {} failed: {error:#}", self.command)
                })
                .to_string(),
            )),
        }
    }
}

pub fn podcast_tools() -> Vec<Arc<dyn Tool>> {
    podcast_tools_for_language(None)
}

pub fn preferred_language(task: &str, fallback: Option<&str>) -> String {
    if task.chars().any(|c| ('\u{3400}'..='\u{9fff}').contains(&c)) {
        "ZH".into()
    } else if task.split_whitespace().count() >= 3 && task.chars().any(|c| c.is_ascii_alphabetic())
    {
        "EN".into()
    } else {
        fallback.unwrap_or("EN").to_uppercase()
    }
}

pub fn podcast_tools_for_language(language: Option<&str>) -> Vec<Arc<dyn Tool>> {
    COMMANDS
        .iter()
        .map(|spec| {
            Arc::new(PodcastTool {
                command: spec.name,
                language: language.map(str::to_owned),
            }) as Arc<dyn Tool>
        })
        .collect()
}

async fn run(
    command: &'static str,
    input: Value,
    progress: Option<crate::agent::tool::ToolProgressSender>,
) -> anyhow::Result<Value> {
    let spec = COMMANDS
        .iter()
        .find(|spec| spec.name == command)
        .ok_or_else(|| anyhow::anyhow!("Unknown podcast command: {command}"))?;
    crate::sites::runner::run_tool_command_without_browser(
        crate::sites::runner::ToolCommand {
            site_id: "podcast",
            command_name: command,
            tool_name: spec.tool_name,
            before: None,
            after: None,
            include_run_metadata: false,
        },
        &podcast_tools(),
        input,
        progress,
    )
    .await
}

macro_rules! arg {
    ($key:literal, $long:expr, $kind:ident, $required:literal, $help:literal) => {
        CommandArg {
            key: $key,
            long: $long,
            value_name: $key,
            help: $help,
            required: $required,
            kind: ArgKind::$kind,
        }
    };
}
const SHOWS_ARGS: &[CommandArg] = &[arg!(
    "query",
    None,
    Str,
    false,
    "Show name, host or id (optional; otherwise list all indexed shows)"
)];
const SEARCH_ARGS: &[CommandArg] = &[
    arg!("language", Some("language"), Str, false, "Preferred summary language EN or ZH, independent of original-language filter. Defaults to the user request language in the app; keep the same language across pages."),
    arg!(
        "query",
        None,
        Str,
        true,
        "Content keyword, 2–100 characters; may match title, summary or transcript"
    ),
    arg!(
        "show",
        Some("show"),
        Str,
        false,
        "Show id from podcast shows; narrows search coverage"
    ),
    arg!(
        "lang",
        Some("lang"),
        Str,
        false,
        "Original language EN or ZH; omit to include both"
    ),
    arg!(
        "limit",
        Some("limit"),
        Int,
        false,
        "Maximum results, 1–20, default 5; metadata + TL;DR only"
    ),
];
const EPISODE_ARGS: &[CommandArg] = &[
    arg!("language", Some("language"), Str, false, "Preferred summary language EN or ZH, independent of original-language filter. Defaults to the user request language in the app; keep the same language across pages."),
    arg!(
        "show",
        Some("show"),
        Str,
        false,
        "Show id from podcast shows; omit for the whole indexed library"
    ),
    arg!(
        "lang",
        Some("lang"),
        Str,
        false,
        "Original language EN or ZH; omit to include both"
    ),
    arg!(
        "limit",
        Some("limit"),
        Int,
        false,
        "Page size, 1–20, default 5; metadata + TL;DR only"
    ),
    arg!(
        "offset",
        Some("offset"),
        Int,
        false,
        "Episode offset, default 0; follow pagination.next_offset"
    ),
];
const READ_ARGS: &[CommandArg] = &[
    arg!("language", Some("language"), Str, false, "Preferred summary language EN or ZH, independent of original-language filter. Defaults to the user request language in the app; keep the same language across pages."),
    arg!(
        "slug",
        None,
        Str,
        true,
        "Episode slug returned by search or episodes"
    ),
    arg!(
        "section",
        Some("section"),
        Str,
        false,
        "metadata, tldr (default), digest, transcript, or full; deeper content only on demand"
    ),
    arg!(
        "offset",
        Some("offset"),
        Int,
        false,
        "Unicode character offset, default 0; follow next_offset until null"
    ),
    arg!(
        "max_chars",
        Some("max-chars"),
        Int,
        false,
        "Page length, 1–20000 characters, default 12000; applies to the chosen section"
    ),
];
static COMMANDS: &[SiteCommand] = &[
    SiteCommand { name:"shows", tool_name:"search_podcast_shows", about:"Find indexed Chinese/international podcast shows by name or host; returns metadata, no browser or login.", args:SHOWS_ARGS, slow:SlowWhen::Always, run:|_,a,_,p| Box::pin(run("shows",a,p)) },
    SiteCommand { name:"search", tool_name:"search_podcast_episodes", about:"Search podcast episode content via BidClub. Returns metadata + TL;DR (default 5). Preserve partial/searched_recent: search may only cover recent episodes, no pagination. No browser/login.", args:SEARCH_ARGS, slow:SlowWhen::Always, run:|_,a,_,p| Box::pin(run("search",a,p)) },
    SiteCommand { name:"episodes", tool_name:"list_podcast_episodes", about:"List indexed podcast episodes, optionally by show, with metadata + TL;DR; follow pagination.next_offset. Default 5. No browser/login.", args:EPISODE_ARGS, slow:SlowWhen::Always, run:|_,a,_,p| Box::pin(run("episodes",a,p)) },
    SiteCommand { name:"read", tool_name:"read_podcast_episode", about:"Read one podcast: metadata + TL;DR by default. Request section=digest, transcript or full only for deeper reading. Follow next_offset for complete text; missing content has available=false. No browser/login.", args:READ_ARGS, slow:SlowWhen::Always, run:|_,a,_,p| Box::pin(run("read",a,p)) },
];
pub static PODCAST_NATIVE_ADAPTER: NativeSiteAdapter = NativeSiteAdapter {
    id:"podcast", about:"Search Chinese and international podcasts and read summaries/transcripts via BidClub (no browser)", home_url:"https://bidclub.ai",
    runtime_command:Some(|_,command,args,_,progress| Box::pin(run(command,args,progress))),
    agent_tools:|_,_| Box::pin(async { Ok(podcast_tools()) }), default_agent_tools:None,
    agent_instructions:instructions, default_agent_instructions:None, commands:COMMANDS,
};
