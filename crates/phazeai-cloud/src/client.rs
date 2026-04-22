use anyhow::Result;
use futures::channel::mpsc;
use phazeai_core::{
    error::PhazeError, llm::OpenAIClient, tools::ToolDefinition, LlmClient, LlmResponse, Message,
    StreamEvent,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::{auth::CloudCredentials, cloud_api_url};

/// HTTP client for the PhazeAI Cloud API.
/// Delegates LLM calls to an `OpenAIClient` pointed at the cloud backend.
#[derive(Clone)]
pub struct CloudClient {
    http: Client,
    token: String,
    /// Which hosted model to use (e.g. "phaze-beast-70b", "phaze-fast").
    pub model: String,
}

impl CloudClient {
    pub fn new(creds: &CloudCredentials, model: impl Into<String>) -> Result<Self> {
        let token = creds
            .api_token
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No PhazeAI Cloud API token configured"))?;
        Ok(Self {
            http: Client::new(),
            token,
            model: model.into(),
        })
    }

    /// Build an `OpenAIClient` delegating to the cloud backend.
    fn openai_client(&self) -> OpenAIClient {
        // cloud_api_url() returns e.g. "https://api.phazeai.com/v1".
        // OpenAIClient appends "/v1/chat/completions", so strip the trailing "/v1".
        let base = cloud_api_url();
        let base = base.trim_end_matches("/v1").to_string();
        OpenAIClient::new(&self.token)
            .with_base_url(base)
            .with_model(&self.model)
    }

    /// Validate credentials against the cloud API and return account info.
    pub async fn validate(&self) -> Result<AccountInfo> {
        let url = format!("{}/account", cloud_api_url());
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .await?
            .error_for_status()?
            .json::<AccountInfo>()
            .await?;
        Ok(resp)
    }
}

#[async_trait::async_trait]
impl LlmClient for CloudClient {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse, PhazeError> {
        self.openai_client().chat(messages, tools).await
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<mpsc::UnboundedReceiver<StreamEvent>, PhazeError> {
        self.openai_client().chat_stream(messages, tools).await
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AccountInfo {
    pub email: String,
    pub tier: String,
    pub credits_remaining: u64,
    pub credits_limit: u64,
}
