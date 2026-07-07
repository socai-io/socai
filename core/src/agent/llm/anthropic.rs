//! Anthropic Messages API client.
//!
//! Hits POST https://api.anthropic.com/v1/messages directly via reqwest.
//! Supports:
//! - mixed content blocks (text + image) in tool_result messages
//! - prompt caching via `cache_control: { type: "ephemeral" }` on the
//!   system prompt and the last tool definition

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::api_errors::format_http_error;
use crate::agent::llm::{
    Backend, Block, LLMResponse, Message, StopReason, ThinkingBlock, ToolCall, ToolResultContent,
    ToolSchema,
};
use crate::agent::provider::{config_for, load_api_key, Provider};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone)]
pub struct AnthropicBackend {
    model: String,
    api_key: String,
    client: reqwest::Client,
    /// When true, add `cache_control: ephemeral` to (a) the system prompt
    /// and (b) the last tool definition. Lets Anthropic cache them across
    /// steps and cuts input-token cost.
    enable_prompt_caching: bool,
}

impl AnthropicBackend {
    pub fn new(model: impl Into<String>) -> anyhow::Result<Self> {
        let api_key = load_api_key(Provider::Anthropic).ok_or_else(|| {
            anyhow::anyhow!(
                "no Anthropic API key found. Set ANTHROPIC_API_KEY or add anthropic.api_key \
                 to ~/.socai/auth.json."
            )
        })?;
        let model = model.into();
        let resolved_model = if model.trim().is_empty() {
            config_for(Provider::Anthropic).default_model.to_string()
        } else {
            model
        };
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()?;
        Ok(Self {
            model: resolved_model,
            api_key,
            client,
            enable_prompt_caching: true,
        })
    }

    pub fn with_prompt_caching(mut self, enable: bool) -> Self {
        self.enable_prompt_caching = enable;
        self
    }

    fn build_request_payload(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolSchema],
        max_tokens: u32,
    ) -> Value {
        let wire_messages: Vec<WireMessage> = messages.iter().map(message_to_wire).collect();
        let system_value = if self.enable_prompt_caching && !system.is_empty() {
            json!([{
                "type": "text",
                "text": system,
                "cache_control": {"type": "ephemeral"},
            }])
        } else {
            json!(system)
        };
        let mut wire_tools: Vec<Value> = tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.input_schema,
                })
            })
            .collect();
        if self.enable_prompt_caching {
            if let Some(Value::Object(map)) = wire_tools.last_mut() {
                map.insert("cache_control".into(), json!({"type": "ephemeral"}));
            }
        }
        let mut body = json!({
            "model": self.model,
            "max_tokens": max_tokens,
            "system": system_value,
            "messages": wire_messages.iter().map(|message| json!({
                "role": message.role,
                "content": message.content,
            })).collect::<Vec<_>>(),
            "tools": wire_tools,
        });
        if supports_adaptive_thinking(&self.model) {
            // display: "summarized" — the default ("omitted") returns thinking
            // blocks with empty text, so nothing could be surfaced in the UI.
            body["thinking"] = json!({"type": "adaptive", "display": "summarized"});
        }
        body
    }
}

/// Models that accept `thinking: {type: "adaptive"}`. Older models
/// (Haiku 4.5, Sonnet 4.5, Claude 3.x, …) reject it with a 400, so the
/// parameter is only sent where it's supported. Note that Sonnet 5 and
/// Fable 5 run adaptive thinking even when the parameter is omitted — the
/// explicit config is still needed there for `display: "summarized"`.
fn supports_adaptive_thinking(model: &str) -> bool {
    const PREFIXES: [&str; 7] = [
        "claude-fable-5",
        "claude-mythos",
        "claude-opus-4-6",
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-sonnet-4-6",
        "claude-sonnet-5",
    ];
    PREFIXES.iter().any(|p| model.starts_with(p))
}

fn block_to_wire(block: &Block) -> Option<Value> {
    match block {
        Block::Text { text } => Some(json!({"type": "text", "text": text})),
        Block::Image { data, media_type } => Some(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": media_type,
                "data": data,
            },
        })),
        Block::ReasoningContent { .. } => {
            // Kimi/Qwen-style reasoning trace — has no signature, so it can't
            // be faked as an Anthropic thinking block. Drop it on this wire.
            None
        }
        Block::Thinking {
            thinking,
            signature,
        } => Some(json!({
            "type": "thinking",
            "thinking": thinking,
            "signature": signature,
        })),
        Block::OpenAIReasoning { .. } => None,
        Block::ToolUse { id, name, input } => Some(json!({
            "type": "tool_use",
            "id": id,
            "name": name,
            "input": input,
        })),
        Block::ToolResult {
            tool_use_id,
            content,
        } => {
            let wire_content: Vec<Value> = content
                .iter()
                .map(|c| match c {
                    ToolResultContent::Text { text } => json!({"type": "text", "text": text}),
                    ToolResultContent::Image { data, media_type } => json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": media_type,
                            "data": data,
                        },
                    }),
                })
                .collect();
            Some(json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": wire_content,
            }))
        }
    }
}

#[derive(Serialize)]
struct WireMessage {
    role: &'static str,
    content: Vec<Value>,
}

fn message_to_wire(msg: &Message) -> WireMessage {
    let role = match msg.role {
        crate::agent::llm::MessageRole::User => "user",
        crate::agent::llm::MessageRole::Assistant => "assistant",
    };
    let content: Vec<Value> = msg
        .content
        .as_blocks()
        .iter()
        .filter_map(block_to_wire)
        .collect();
    WireMessage { role, content }
}

#[derive(Deserialize, Debug)]
struct WireResponse {
    #[serde(default)]
    content: Vec<WireResponseBlock>,
    stop_reason: Option<String>,
    #[serde(default)]
    usage: WireUsage,
}

#[derive(Deserialize, Debug, Default)]
struct WireUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireResponseBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    Thinking {
        #[serde(default)]
        thinking: String,
        #[serde(default)]
        signature: String,
    },
    #[serde(other)]
    Other,
}

fn parse_stop_reason(s: Option<&str>) -> StopReason {
    match s {
        Some("end_turn") => StopReason::EndTurn,
        Some("tool_use") => StopReason::ToolUse,
        Some("max_tokens") => StopReason::MaxTokens,
        _ => StopReason::Other,
    }
}

#[async_trait]
impl Backend for AnthropicBackend {
    fn label(&self) -> String {
        format!("anthropic:{}", self.model)
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn request_payload(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolSchema],
        max_tokens: u32,
    ) -> anyhow::Result<Value> {
        Ok(self.build_request_payload(system, messages, tools, max_tokens))
    }

    async fn send(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolSchema],
        max_tokens: u32,
    ) -> anyhow::Result<LLMResponse> {
        let body = self.build_request_payload(system, messages, tools, max_tokens);

        let response = self
            .client
            .post(API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!(format_http_error("anthropic", status.as_u16(), &text));
        }
        let parsed: WireResponse = response.json().await?;

        let mut text_blocks = Vec::new();
        let mut tool_calls = Vec::new();
        let mut thinking_blocks: Vec<ThinkingBlock> = Vec::new();
        for block in parsed.content {
            match block {
                WireResponseBlock::Text { text } => text_blocks.push(text),
                WireResponseBlock::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCall { id, name, input })
                }
                WireResponseBlock::Thinking {
                    thinking,
                    signature,
                } => thinking_blocks.push(ThinkingBlock {
                    thinking,
                    signature,
                }),
                WireResponseBlock::Other => {}
            }
        }
        // Surface the thinking summary through the same channel Kimi/Qwen
        // use, so the UI reasoning stream lights up identically.
        let reasoning_content = thinking_blocks
            .iter()
            .map(|tb| tb.thinking.trim())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");

        let input_total = parsed.usage.input_tokens
            + parsed.usage.cache_creation_input_tokens
            + parsed.usage.cache_read_input_tokens;

        Ok(LLMResponse {
            text_blocks,
            tool_calls,
            stop_reason: parse_stop_reason(parsed.stop_reason.as_deref()),
            input_tokens: input_total,
            output_tokens: parsed.usage.output_tokens,
            reasoning_content,
            thinking_blocks,
            reasoning_items: Vec::new(),
        })
    }
}
