//! OpenAI-compatible backend.
//!
//! OpenAI proper always goes through the Responses API — the official
//! `/v1/responses` endpoint with an API key, or the ChatGPT Codex endpoint
//! with Codex OAuth — because chat completions never returns reasoning
//! content. Kimi (Moonshot) and Qwen (DashScope) use chat completions with
//! their own `base_url` via `ProviderConfig`; their quirks (the
//! `reasoning_content` field, thinking toggles) live here too.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::agent::api_errors::format_http_error;
use crate::agent::llm::{
    Backend, Block, LLMResponse, Message, StopReason, ToolCall, ToolResultContent, ToolSchema,
};
use crate::agent::provider::{
    config_for, load_api_key, load_openai_credential, Credential, Provider, ProviderConfig,
};

#[derive(Debug, Clone)]
pub struct OpenAICompatBackend {
    provider: Provider,
    model: String,
    credential: Credential,
    base_url: String,
    client: reqwest::Client,
}

impl OpenAICompatBackend {
    pub fn new(provider: Provider, model: impl Into<String>) -> anyhow::Result<Self> {
        let cfg: &'static ProviderConfig = config_for(provider);
        let credential = if provider == Provider::OpenAI {
            match load_openai_credential() {
                Some(credential) => credential,
                None => {
                    anyhow::bail!(
                        "no OpenAI credential found. Set OPENAI_API_KEY, save openai.api_key to ~/.socai/auth.json, or run `codex login`."
                    );
                }
            }
        } else {
            let api_key = load_api_key(provider).ok_or_else(|| {
                anyhow::anyhow!(
                    "no {} API key found. Set {} or add {}.api_key to ~/.socai/auth.json.",
                    cfg.display_name,
                    cfg.env_keys.join(" or "),
                    provider.as_str(),
                )
            })?;
            Credential::ApiKey(api_key)
        };
        let base_url = cfg
            .base_url
            .ok_or_else(|| anyhow::anyhow!("provider {:?} is not OpenAI-compatible", provider))?
            .trim_end_matches('/')
            .to_string();
        let model = model.into();
        let resolved_model = if model.trim().is_empty() {
            cfg.default_model.to_string()
        } else {
            model
        };
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(180))
            .build()?;
        Ok(Self {
            provider,
            model: resolved_model,
            credential,
            base_url,
            client,
        })
    }

    fn url(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    fn responses_url(&self) -> String {
        match &self.credential {
            Credential::CodexOAuth { .. } => {
                "https://chatgpt.com/backend-api/codex/responses".to_string()
            }
            Credential::ApiKey(_) => format!("{}/responses", self.base_url),
        }
    }

    /// Provider-specific extra fields merged into the request body.
    fn extra_body(&self, has_tools: bool) -> Map<String, Value> {
        let mut extra = Map::new();
        if !has_tools {
            return extra;
        }
        match self.provider {
            Provider::Kimi if self.model.starts_with("kimi-k2.6") => {
                extra.insert("thinking".into(), json!({"type": "disabled"}));
            }
            Provider::Qwen | Provider::QwenIntl => {
                extra.insert("enable_thinking".into(), Value::Bool(false));
            }
            _ => {}
        }
        extra
    }

    /// Some providers (Kimi, Qwen, Doubao) want `reasoning_content` round-tripped
    /// in the assistant message when tool_calls are present. Others
    /// (OpenAI proper) ignore it. Toggle is per-provider.
    fn preserve_reasoning_content(&self) -> bool {
        matches!(
            self.provider,
            Provider::Kimi | Provider::Qwen | Provider::QwenIntl | Provider::Doubao
        )
    }

    fn build_chat_request(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolSchema],
        max_tokens: u32,
    ) -> OutgoingRequest {
        let chat_tools = tools_to_wire(tools);
        let has_tools = !chat_tools.is_empty();
        OutgoingRequest {
            model: self.model.clone(),
            messages: build_chat_messages(system, messages, self.preserve_reasoning_content()),
            max_tokens,
            tools: chat_tools,
            tool_choice: if has_tools { Some("auto") } else { None },
            extra: self.extra_body(has_tools),
        }
    }

    fn build_responses_request(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolSchema],
        max_tokens: u32,
    ) -> ResponsesRequest {
        // Reasoning on by default — newer GPT-5.x releases default the
        // effort to "none". summary: "auto" surfaces the summary text;
        // encrypted_content is what store:false replay needs. Non-reasoning
        // models (gpt-4o, the -chat variants) reject the parameter.
        let reasoning = is_openai_reasoning_model(&self.model)
            .then(|| json!({"effort": "medium", "summary": "auto"}));
        let include = if reasoning.is_some() {
            vec!["reasoning.encrypted_content"]
        } else {
            Vec::new()
        };
        ResponsesRequest {
            model: self.model.clone(),
            instructions: system.to_string(),
            input: build_responses_input(messages),
            tools: responses_tools_to_wire(tools),
            tool_choice: if tools.is_empty() { None } else { Some("auto") },
            parallel_tool_calls: true,
            stream: true,
            store: false,
            // The Codex endpoint manages output limits itself; only the
            // official API gets an explicit cap.
            max_output_tokens: match self.credential {
                Credential::CodexOAuth { .. } => None,
                Credential::ApiKey(_) => Some(max_tokens),
            },
            reasoning,
            include,
        }
    }
}

/// OpenAI models that accept the Responses API `reasoning` parameter.
/// The `-chat` variants (gpt-5-chat-latest, …) are the non-reasoning
/// conversational builds and reject it, as do gpt-4o/gpt-4.x.
fn is_openai_reasoning_model(model: &str) -> bool {
    if model.contains("-chat") {
        return false;
    }
    model.starts_with("gpt-5")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::llm::{Message, MessageContent, MessageRole};

    #[test]
    fn parses_responses_sse_text_and_tool_call() {
        let body = r#"event: response.output_text.delta
data: {"type":"response.output_text.delta","delta":"hello"}

event: response.output_item.done
data: {"type":"response.output_item.done","item":{"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{\"key\":\"abc\"}"}}

event: response.completed
data: {"type":"response.completed","response":{"usage":{"input_tokens":3,"output_tokens":5}}}
"#;

        let parsed = parse_responses_sse(body).expect("sse should parse");
        assert_eq!(parsed.text_blocks, vec!["hello"]);
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].id, "call_1");
        assert_eq!(parsed.tool_calls[0].name, "lookup");
        assert_eq!(parsed.tool_calls[0].input["key"], "abc");
        assert_eq!(parsed.stop_reason, StopReason::ToolUse);
        assert_eq!(parsed.input_tokens, 3);
        assert_eq!(parsed.output_tokens, 5);
    }

    #[test]
    fn responses_input_preserves_function_call_outputs() {
        let input = build_responses_input(&[
            Message::user("lookup abc"),
            Message::assistant_blocks(vec![Block::ToolUse {
                id: "call_1".into(),
                name: "lookup".into(),
                input: json!({"key": "abc"}),
            }]),
            Message {
                role: MessageRole::User,
                content: MessageContent::Blocks(vec![Block::ToolResult {
                    tool_use_id: "call_1".into(),
                    content: vec![ToolResultContent::Text { text: "42".into() }],
                }]),
            },
        ]);

        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "call_1");
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["output"], "42");
    }
}

fn flatten_tool_result_content(blocks: &[ToolResultContent]) -> Value {
    // OpenAI Chat Completions tool messages support content as a string or
    // as an array of {type: text} / {type: image_url} parts when the model
    // can read vision. We emit an array whenever there's at least one image;
    // otherwise fall back to a plain string for older models.
    let has_image = blocks
        .iter()
        .any(|b| matches!(b, ToolResultContent::Image { .. }));
    if has_image {
        let parts: Vec<Value> = blocks
            .iter()
            .map(|b| match b {
                ToolResultContent::Text { text } => json!({"type": "text", "text": text}),
                ToolResultContent::Image { data, media_type } => json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:{media_type};base64,{data}"),
                    },
                }),
            })
            .collect();
        Value::Array(parts)
    } else {
        let combined = blocks
            .iter()
            .filter_map(|b| match b {
                ToolResultContent::Text { text } => Some(text.clone()),
                ToolResultContent::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        Value::String(combined)
    }
}

/// Translate Anthropic-shaped history into chat-completion messages.
fn build_chat_messages(system: &str, messages: &[Message], preserve_reasoning: bool) -> Vec<Value> {
    let mut out = vec![json!({"role": "system", "content": system})];

    for msg in messages {
        let blocks = msg.content.as_blocks();
        match msg.role {
            crate::agent::llm::MessageRole::Assistant => {
                let mut text_parts: Vec<String> = Vec::new();
                let mut tool_calls: Vec<Value> = Vec::new();
                let mut reasoning: Option<String> = None;
                for block in blocks {
                    match block {
                        Block::Text { text } => text_parts.push(text),
                        Block::Image { .. } => {}
                        Block::ReasoningContent { text } => {
                            reasoning = Some(text);
                        }
                        Block::ToolUse { id, name, input } => {
                            tool_calls.push(json!({
                                "id": id,
                                "type": "function",
                                "function": {
                                    "name": name,
                                    "arguments": serde_json::to_string(&input).unwrap_or_else(|_| "{}".into()),
                                },
                            }));
                        }
                        Block::ToolResult { .. }
                        | Block::Thinking { .. }
                        | Block::OpenAIReasoning { .. } => {}
                    }
                }
                let content_str = text_parts.join("\n").trim().to_string();
                let mut assistant_msg = Map::new();
                assistant_msg.insert("role".into(), json!("assistant"));
                if content_str.is_empty() {
                    assistant_msg.insert("content".into(), Value::Null);
                } else {
                    assistant_msg.insert("content".into(), json!(content_str));
                }
                if !tool_calls.is_empty() {
                    assistant_msg.insert("tool_calls".into(), Value::Array(tool_calls.clone()));
                }
                if preserve_reasoning && !tool_calls.is_empty() {
                    assistant_msg.insert(
                        "reasoning_content".into(),
                        json!(reasoning.unwrap_or_default()),
                    );
                }
                out.push(Value::Object(assistant_msg));
            }
            crate::agent::llm::MessageRole::User => {
                let mut user_text_parts: Vec<String> = Vec::new();
                let mut user_image_parts: Vec<Value> = Vec::new();
                for block in blocks {
                    match block {
                        Block::Text { text } => user_text_parts.push(text),
                        Block::Image { data, media_type } => {
                            user_image_parts.push(json!({
                                "type": "image_url",
                                "image_url": {
                                    "url": format!("data:{media_type};base64,{data}"),
                                },
                            }));
                        }
                        Block::ToolResult {
                            tool_use_id,
                            content,
                        } => {
                            out.push(json!({
                                "role": "tool",
                                "tool_call_id": tool_use_id,
                                "content": flatten_tool_result_content(&content),
                            }));
                        }
                        Block::ReasoningContent { .. }
                        | Block::ToolUse { .. }
                        | Block::Thinking { .. }
                        | Block::OpenAIReasoning { .. } => {}
                    }
                }
                let joined = user_text_parts.join("\n").trim().to_string();
                if !user_image_parts.is_empty() {
                    let mut content = Vec::new();
                    if !joined.is_empty() {
                        content.push(json!({"type": "text", "text": joined}));
                    }
                    content.extend(user_image_parts);
                    out.push(json!({"role": "user", "content": content}));
                } else if !joined.is_empty() {
                    out.push(json!({"role": "user", "content": joined}));
                }
            }
        }
    }

    out
}

fn tools_to_wire(tools: &[ToolSchema]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.input_schema,
                }
            })
        })
        .collect()
}

fn responses_tools_to_wire(tools: &[ToolSchema]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "name": t.name,
                "description": t.description,
                "parameters": t.input_schema,
                "strict": false,
            })
        })
        .collect()
}

fn tool_result_to_text(blocks: &[ToolResultContent]) -> String {
    blocks
        .iter()
        .map(|b| match b {
            ToolResultContent::Text { text } => text.clone(),
            ToolResultContent::Image { data, media_type } => {
                format!("data:{media_type};base64,{data}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn responses_content_parts(blocks: Vec<Block>, output: bool) -> Vec<Value> {
    let mut parts = Vec::new();
    for block in blocks {
        match block {
            Block::Text { text } if output => {
                parts.push(json!({"type": "output_text", "text": text}));
            }
            Block::Text { text } => {
                parts.push(json!({"type": "input_text", "text": text}));
            }
            Block::Image { data, media_type } if !output => {
                parts.push(json!({
                    "type": "input_image",
                    "image_url": format!("data:{media_type};base64,{data}"),
                }));
            }
            Block::ReasoningContent { .. }
            | Block::ToolUse { .. }
            | Block::ToolResult { .. }
            | Block::Thinking { .. }
            | Block::OpenAIReasoning { .. } => {}
            Block::Image { .. } => {}
        }
    }
    parts
}

fn build_responses_input(messages: &[Message]) -> Vec<Value> {
    let mut out = Vec::new();
    for msg in messages {
        let blocks = msg.content.as_blocks();
        match msg.role {
            crate::agent::llm::MessageRole::Assistant => {
                let mut text_blocks = Vec::new();
                for block in blocks {
                    match block {
                        Block::Text { .. } => text_blocks.push(block),
                        // Replay the reasoning item exactly as received —
                        // required with store:false to keep the model's chain
                        // of thought across tool calls.
                        Block::OpenAIReasoning { item } => out.push(item),
                        Block::ToolUse { id, name, input } => {
                            out.push(json!({
                                "type": "function_call",
                                "call_id": id,
                                "name": name,
                                "arguments": serde_json::to_string(&input).unwrap_or_else(|_| "{}".into()),
                            }));
                        }
                        Block::Image { .. }
                        | Block::ReasoningContent { .. }
                        | Block::ToolResult { .. }
                        | Block::Thinking { .. } => {}
                    }
                }
                let content = responses_content_parts(text_blocks, true);
                if !content.is_empty() {
                    out.push(json!({"role": "assistant", "content": content}));
                }
            }
            crate::agent::llm::MessageRole::User => {
                let mut message_blocks = Vec::new();
                for block in blocks {
                    match block {
                        Block::ToolResult {
                            tool_use_id,
                            content,
                        } => {
                            out.push(json!({
                                "type": "function_call_output",
                                "call_id": tool_use_id,
                                "output": tool_result_to_text(&content),
                            }));
                        }
                        Block::Text { .. } | Block::Image { .. } => message_blocks.push(block),
                        Block::ReasoningContent { .. }
                        | Block::ToolUse { .. }
                        | Block::Thinking { .. }
                        | Block::OpenAIReasoning { .. } => {}
                    }
                }
                let content = responses_content_parts(message_blocks, false);
                if !content.is_empty() {
                    out.push(json!({"role": "user", "content": content}));
                }
            }
        }
    }
    out
}

#[derive(Deserialize)]
struct WireResponse {
    choices: Vec<WireChoice>,
    #[serde(default)]
    usage: WireUsage,
}

#[derive(Deserialize, Default)]
struct WireUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

#[derive(Deserialize)]
struct WireChoice {
    message: WireMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct WireMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<WireToolCall>>,
}

#[derive(Deserialize)]
struct WireToolCall {
    id: String,
    function: WireFunction,
}

#[derive(Deserialize)]
struct WireFunction {
    name: String,
    #[serde(default)]
    arguments: Option<String>,
}

fn parse_stop_reason(s: Option<&str>) -> StopReason {
    match s {
        Some("stop") => StopReason::EndTurn,
        Some("tool_calls") => StopReason::ToolUse,
        Some("length") => StopReason::MaxTokens,
        _ => StopReason::Other,
    }
}

#[derive(Serialize)]
struct OutgoingRequest {
    model: String,
    messages: Vec<Value>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Serialize)]
struct ResponsesRequest {
    model: String,
    instructions: String,
    input: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
    parallel_tool_calls: bool,
    stream: bool,
    store: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    include: Vec<&'static str>,
}

fn parse_responses_sse(body: &str) -> anyhow::Result<LLMResponse> {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut reasoning_items: Vec<Value> = Vec::new();
    let mut reasoning_texts: Vec<String> = Vec::new();
    let mut input_tokens = 0;
    let mut output_tokens = 0;
    let mut completed = false;

    for line in body.lines() {
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        if data.trim() == "[DONE]" {
            continue;
        }
        let value: Value = serde_json::from_str(data)?;
        match value.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => {
                if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                    text.push_str(delta);
                }
            }
            Some("response.output_item.done") => {
                if let Some(item) = value.get("item").and_then(Value::as_object) {
                    if item.get("type").and_then(Value::as_str) == Some("reasoning") {
                        for part in item
                            .get("summary")
                            .and_then(Value::as_array)
                            .map(Vec::as_slice)
                            .unwrap_or_default()
                        {
                            if let Some(t) = part.get("text").and_then(Value::as_str) {
                                if !t.trim().is_empty() {
                                    reasoning_texts.push(t.trim().to_string());
                                }
                            }
                        }
                        reasoning_items.push(Value::Object(item.clone()));
                    }
                    if item.get("type").and_then(Value::as_str) == Some("function_call") {
                        let id = item
                            .get("call_id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let name = item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let args = item
                            .get("arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("{}");
                        let input = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
                        if !id.is_empty() && !name.is_empty() {
                            tool_calls.push(ToolCall { id, name, input });
                        }
                    }
                }
            }
            Some("response.completed") => {
                completed = true;
                if let Some(usage) = value
                    .get("response")
                    .and_then(|r| r.get("usage"))
                    .and_then(Value::as_object)
                {
                    input_tokens = usage
                        .get("input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or_default();
                    output_tokens = usage
                        .get("output_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or_default();
                }
            }
            Some("response.failed") | Some("response.incomplete") => {
                anyhow::bail!("OpenAI Codex Responses request failed: {value}");
            }
            _ => {}
        }
    }

    if !completed {
        anyhow::bail!("OpenAI Codex Responses stream ended without response.completed");
    }

    let text_blocks = if text.trim().is_empty() {
        Vec::new()
    } else {
        vec![text]
    };
    let stop_reason = if tool_calls.is_empty() {
        StopReason::EndTurn
    } else {
        StopReason::ToolUse
    };
    Ok(LLMResponse {
        text_blocks,
        tool_calls,
        stop_reason,
        input_tokens,
        output_tokens,
        reasoning_content: reasoning_texts.join("\n\n"),
        thinking_blocks: Vec::new(),
        reasoning_items,
    })
}

#[async_trait]
impl Backend for OpenAICompatBackend {
    fn label(&self) -> String {
        format!("{}:{}", self.provider.as_str(), self.model)
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
        if self.provider == Provider::OpenAI {
            serde_json::to_value(self.build_responses_request(system, messages, tools, max_tokens))
                .map_err(Into::into)
        } else {
            serde_json::to_value(self.build_chat_request(system, messages, tools, max_tokens))
                .map_err(Into::into)
        }
    }

    async fn send(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolSchema],
        max_tokens: u32,
    ) -> anyhow::Result<LLMResponse> {
        if self.provider == Provider::OpenAI {
            return self.send_responses(system, messages, tools, max_tokens).await;
        }

        let body = self.build_chat_request(system, messages, tools, max_tokens);

        let response = self
            .client
            .post(self.url())
            .bearer_auth(self.api_key()?)
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!(format_http_error(
                self.provider.as_str(),
                status.as_u16(),
                &text
            ));
        }

        let parsed: WireResponse = response.json().await?;
        let choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("response had no choices"))?;

        let text = choice.message.content.unwrap_or_default();
        let text_blocks = if text.trim().is_empty() {
            Vec::new()
        } else {
            vec![text]
        };

        let mut tool_calls = Vec::new();
        for tc in choice.message.tool_calls.unwrap_or_default() {
            let args_raw = tc.function.arguments.unwrap_or_else(|| "{}".into());
            let input: Value = serde_json::from_str(&args_raw).unwrap_or(Value::Object(Map::new()));
            tool_calls.push(ToolCall {
                id: tc.id,
                name: tc.function.name,
                input,
            });
        }

        Ok(LLMResponse {
            text_blocks,
            tool_calls,
            stop_reason: parse_stop_reason(choice.finish_reason.as_deref()),
            input_tokens: parsed.usage.prompt_tokens,
            output_tokens: parsed.usage.completion_tokens,
            reasoning_content: choice.message.reasoning_content.unwrap_or_default(),
            thinking_blocks: Vec::new(),
            reasoning_items: Vec::new(),
        })
    }
}

impl OpenAICompatBackend {
    fn api_key(&self) -> anyhow::Result<&str> {
        match &self.credential {
            Credential::ApiKey(api_key) => Ok(api_key),
            Credential::CodexOAuth { .. } => {
                anyhow::bail!("Codex OAuth credentials require the Codex Responses backend")
            }
        }
    }

    async fn send_responses(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolSchema],
        max_tokens: u32,
    ) -> anyhow::Result<LLMResponse> {
        let body = self.build_responses_request(system, messages, tools, max_tokens);

        let response = self.send_responses_once(&body).await?;
        self.parse_responses_response(response).await
    }

    async fn send_responses_once(&self, body: &ResponsesRequest) -> anyhow::Result<reqwest::Response> {
        let credential = &self.credential;
        let request = self.client.post(self.responses_url());
        let request = match credential {
            Credential::CodexOAuth {
                access_token,
                account_id,
                ..
            } => request
                .bearer_auth(access_token)
                .header("ChatGPT-Account-ID", account_id),
            Credential::ApiKey(api_key) => request.bearer_auth(api_key),
        };
        Ok(request.json(body).send().await?)
    }

    async fn parse_responses_response(
        &self,
        response: reqwest::Response,
    ) -> anyhow::Result<LLMResponse> {
        let is_codex = matches!(self.credential, Credential::CodexOAuth { .. });
        let label = if is_codex { "openai-codex" } else { "openai" };
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED && is_codex {
                anyhow::bail!(
                    "{}\nHint: run `codex login`, then retry socai.",
                    format_http_error(label, status.as_u16(), &text)
                );
            }
            anyhow::bail!(format_http_error(label, status.as_u16(), &text));
        }
        parse_responses_sse(&text)
    }
}
