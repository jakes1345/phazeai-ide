use crate::error::PhazeError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// What a reasoning model thought before this assistant message, kept
    /// for providers that must be sent it back (DeepSeek refuses a request
    /// with tools whose earlier assistant turns leave it out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            reasoning: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            reasoning: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            reasoning: None,
        }
    }

    pub fn assistant_with_tools(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            reasoning: None,
        }
    }

    /// The same message carrying the model's `reasoning`, when there is any.
    pub fn with_reasoning(mut self, reasoning: impl Into<String>) -> Self {
        let reasoning = reasoning.into();
        self.reasoning = (!reasoning.is_empty()).then_some(reasoning);
        self
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            reasoning: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

impl ToolCall {
    pub fn parse_arguments(&self) -> Result<Value, serde_json::Error> {
        serde_json::from_str(&self.function.arguments)
    }
}

#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub message: Message,
    pub usage: Option<Usage>,
}

/// Token counts for one request.
///
/// `input_tokens` is the whole prompt, whether or not the provider served part
/// of it from its prompt cache. `cache_read_tokens` is the part it read from
/// that cache (billed at a fraction of the input price) and
/// `cache_write_tokens` the part it wrote to it (Anthropic bills a write above
/// the input price; the other providers cache for free and report none).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_read_tokens: u32,
    #[serde(default)]
    pub cache_write_tokens: u32,
}

/// Events emitted during streaming LLM responses.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    TextDelta(String),
    /// Part of what a reasoning model thinks before it answers.
    ReasoningDelta(String),
    ToolCallStart {
        id: String,
        name: String,
    },
    ToolCallDelta {
        id: String,
        arguments_delta: String,
    },
    ToolCallEnd {
        id: String,
    },
    /// Emitted by providers that report usage in the stream (before Done).
    Usage(Usage),
    Done,
    Error(String),
}

/// The LLM client trait. Implementations provide both blocking and streaming chat.
#[async_trait::async_trait]
pub trait LlmClient: Send + Sync {
    /// Send a chat request and get the full response.
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[crate::tools::ToolDefinition],
    ) -> Result<LlmResponse, PhazeError>;

    /// Send a chat request and get a stream of events.
    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[crate::tools::ToolDefinition],
    ) -> Result<futures::channel::mpsc::UnboundedReceiver<StreamEvent>, PhazeError>;
}
