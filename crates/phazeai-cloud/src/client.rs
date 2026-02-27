use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::{auth::CloudCredentials, cloud_api_url};

/// HTTP client for the PhazeAI Cloud API.
/// Implements the same streaming interface as phazeai-core LlmClient.
#[derive(Clone)]
pub struct CloudClient {
    http: Client,
    token: String,
    /// Which hosted model to use (e.g. "phaze-beast-70b", "phaze-fast").
    pub model: String,
}

impl CloudClient {
    pub fn new(creds: &CloudCredentials, model: impl Into<String>) -> Result<Self> {
        let token = creds.api_token.clone()
            .ok_or_else(|| anyhow::anyhow!("No PhazeAI Cloud API token configured"))?;
        Ok(Self {
            http: Client::new(),
            token,
            model: model.into(),
        })
    }

    /// Validate credentials against the cloud API and return account info.
    pub async fn validate(&self) -> Result<AccountInfo> {
        let url = format!("{}/account", cloud_api_url());
        let resp = self.http
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .await?
            .error_for_status()?
            .json::<AccountInfo>()
            .await?;
        Ok(resp)
    }

    /// OpenAI-compatible streaming chat endpoint (cloud-hosted).
    /// Returns raw SSE lines for consumption by phazeai-core's streaming parser.
    pub async fn stream_chat_url(&self) -> String {
        format!("{}/chat/completions", cloud_api_url())
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AccountInfo {
    pub email: String,
    pub tier: String,
    pub credits_remaining: u64,
    pub credits_limit: u64,
}
