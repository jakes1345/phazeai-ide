//! The LLM client wrapper that enforces `local_only` and feeds the audit log.

use super::audit;
use crate::error::PhazeError;
use crate::llm::{LlmClient, LlmResponse, Message, ProviderId, StreamEvent};
use crate::tools::ToolDefinition;

/// Wraps every client built by [`crate::llm::ProviderRegistry::build_client_for`].
///
/// Before each `chat` / `chat_stream` call it re-checks the active privacy
/// policy (so flipping `local_only` takes effect for already-built clients)
/// and records the request in [`audit`].
pub struct GuardedClient {
    inner: Box<dyn LlmClient>,
    provider: ProviderId,
    model: String,
    base_url: String,
}

impl GuardedClient {
    pub fn new(
        inner: Box<dyn LlmClient>,
        provider: ProviderId,
        model: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            provider,
            model: model.into(),
            base_url: base_url.into(),
        }
    }

    fn before_request(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<(), PhazeError> {
        super::check_provider(&self.provider, &self.base_url, &self.model)?;
        let name = self.provider.name().to_string();
        audit::record_request(
            &name,
            &self.model,
            request_chars(messages, tools),
            audit::take_files(),
        );
        Ok(())
    }
}

/// Approximate size of a request: message text, tool-call arguments and tool schemas.
pub(crate) fn request_chars(messages: &[Message], tools: &[ToolDefinition]) -> usize {
    let msgs: usize = messages
        .iter()
        .map(|m| {
            m.content.chars().count()
                + m.tool_calls
                    .iter()
                    .flatten()
                    .map(|c| c.function.name.len() + c.function.arguments.chars().count())
                    .sum::<usize>()
        })
        .sum();
    let schemas: usize = tools
        .iter()
        .map(|t| serde_json::to_string(t).map(|s| s.len()).unwrap_or(0))
        .sum();
    msgs + schemas
}

#[async_trait::async_trait]
impl LlmClient for GuardedClient {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse, PhazeError> {
        self.before_request(messages, tools)?;
        self.inner.chat(messages, tools).await
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<futures::channel::mpsc::UnboundedReceiver<StreamEvent>, PhazeError> {
        self.before_request(messages, tools)?;
        self.inner.chat_stream(messages, tools).await
    }
}
