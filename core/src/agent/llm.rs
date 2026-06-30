//! LLM backend abstraction and message types.
//!
//! Messages are modeled in Anthropic's shape (mixed content blocks).
//! `OpenAICompatBackend` translates to chat-completions on the wire.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod anthropic;
pub mod openai;

pub use self::anthropic::AnthropicBackend;
pub use self::openai::OpenAICompatBackend;

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
    /// Echoed back to those providers on subsequent turns when tool_calls
    /// are present (some providers reject the request otherwise).
    ReasoningContent {
        text: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMResponse {
    pub text_blocks: Vec<String>,
    pub tool_calls: Vec<ToolCall>,
    pub stop_reason: StopReason,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Reasoning trace surfaced by Kimi K2.6 / Qwen. Empty for providers
    /// that don't expose it.
    pub reasoning_content: String,
}

impl LLMResponse {
    /// Reconstruct the assistant content we'll append to history.
    /// reasoning_content first (if any, only when there are tool calls —
    /// some providers reject it alone), text, then tool_use blocks.
    pub fn to_assistant_blocks(&self) -> Vec<Block> {
        let mut blocks: Vec<Block> = Vec::new();
        if !self.reasoning_content.trim().is_empty() && !self.tool_calls.is_empty() {
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
