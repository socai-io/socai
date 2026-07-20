//! LLM backend abstraction and message types.
//!
//! Messages are modeled in Anthropic's shape (mixed content blocks).
//! `OpenAICompatBackend` translates to chat-completions on the wire.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod anthropic;
pub mod openai;
pub mod usage;

pub use self::anthropic::AnthropicBackend;
pub use self::openai::OpenAICompatBackend;
pub use self::usage::{TokenUsage, UsageCost};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
}

/// One block within a tool_result. Plain text + image are the two we
/// support today. Mirrors `ToolResultBlock` in `tool.rs` but lives in the
/// LLM-side type tree because tool_result content must be transportable
/// over the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultContent {
    Text { text: String },
    Image { data: String, media_type: String },
}

/// One block within a message. Tool requests + tool results are represented
/// natively (the OpenAI translation happens at the wire-format boundary).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    Image {
        data: String,
        media_type: String,
    },
    /// Reasoning trace surfaced by Kimi K2.6 / Qwen / o1-style models.
    /// Echoed back to those providers on subsequent steps when tool_calls
    /// are present (some providers reject the request otherwise).
    ReasoningContent {
        text: String,
    },
    /// Anthropic native thinking block (adaptive thinking). Must be echoed
    /// back verbatim — text and signature untouched — when continuing the
    /// conversation on the same Anthropic model; the API rejects modified
    /// blocks. Other backends drop it on the wire.
    Thinking {
        thinking: String,
        signature: String,
    },
    /// OpenAI Responses API reasoning item, kept as raw JSON. With
    /// `store: false` the encrypted reasoning must be replayed verbatim in
    /// the next request's `input`, or the model loses its chain of thought
    /// between tool calls. Other backends drop it on the wire.
    #[serde(rename = "openai_reasoning")]
    OpenAIReasoning {
        item: Value,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        /// Mixed content (text + images). Anthropic accepts this natively;
        /// OpenAI-compatible backends flatten images to data URIs.
        content: Vec<ToolResultContent>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<Block>),
}

impl MessageContent {
    pub fn as_blocks(&self) -> Vec<Block> {
        match self {
            MessageContent::Text(t) => vec![Block::Text { text: t.clone() }],
            MessageContent::Blocks(b) => b.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: MessageContent,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: MessageContent::Text(text.into()),
        }
    }

    pub fn assistant_blocks(blocks: Vec<Block>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: MessageContent::Blocks(blocks),
        }
    }

    pub fn user_blocks(blocks: Vec<Block>) -> Self {
        Self {
            role: MessageRole::User,
            content: MessageContent::Blocks(blocks),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Other,
}

/// One Anthropic thinking block from a response, kept verbatim (text +
/// signature) so it can be replayed on the next step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingBlock {
    pub thinking: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMResponse {
    pub text_blocks: Vec<String>,
    pub tool_calls: Vec<ToolCall>,
    pub stop_reason: StopReason,
    pub usage: TokenUsage,
    /// The provider's usage object before normalization. This keeps new or
    /// provider-specific counters available in persisted step responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_usage: Option<Value>,
    /// Reasoning trace surfaced by Kimi K2.6 / Qwen. Empty for providers
    /// that don't expose it.
    pub reasoning_content: String,
    /// Anthropic thinking blocks, in response order, for verbatim replay.
    /// Empty for non-Anthropic providers.
    #[serde(default)]
    pub thinking_blocks: Vec<ThinkingBlock>,
    /// OpenAI Responses API reasoning items (raw JSON), in response order,
    /// for verbatim replay. Empty for other providers.
    #[serde(default)]
    pub reasoning_items: Vec<Value>,
}

impl LLMResponse {
    /// Reconstruct the assistant content we'll append to history.
    /// Thinking / reasoning first, text, then tool_use blocks.
    pub fn to_assistant_blocks(&self) -> Vec<Block> {
        let mut blocks: Vec<Block> = Vec::new();
        for item in &self.reasoning_items {
            blocks.push(Block::OpenAIReasoning { item: item.clone() });
        }
        for tb in &self.thinking_blocks {
            blocks.push(Block::Thinking {
                thinking: tb.thinking.clone(),
                signature: tb.signature.clone(),
            });
        }
        if self.thinking_blocks.is_empty()
            && self.reasoning_items.is_empty()
            && !self.reasoning_content.trim().is_empty()
            && !self.tool_calls.is_empty()
        {
            blocks.push(Block::ReasoningContent {
                text: self.reasoning_content.clone(),
            });
        }
        for text in &self.text_blocks {
            if !text.trim().is_empty() {
                blocks.push(Block::Text { text: text.clone() });
            }
        }
        for tc in &self.tool_calls {
            blocks.push(Block::ToolUse {
                id: tc.id.clone(),
                name: tc.name.clone(),
                input: tc.input.clone(),
            });
        }
        blocks
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[async_trait]
pub trait Backend: Send + Sync {
    fn label(&self) -> String;
    fn provider(&self) -> &str;
    fn model(&self) -> &str;

    /// JSON body sent to the provider, excluding authentication headers.
    /// Production backends override this with their wire-format payload.
    fn request_payload(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolSchema],
        max_tokens: u32,
    ) -> anyhow::Result<Value> {
        Ok(serde_json::json!({
            "model": self.model(),
            "system": system,
            "messages": messages,
            "tools": tools,
            "max_tokens": max_tokens,
        }))
    }

    async fn send(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolSchema],
        max_tokens: u32,
    ) -> anyhow::Result<LLMResponse>;
}
