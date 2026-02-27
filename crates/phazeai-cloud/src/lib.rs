//! PhazeAI Cloud — optional paid-tier client.
//!
//! The core IDE (`phazeai-ui`, `phazeai-core`) is MIT open source.
//! This crate adds cloud account auth, hosted model access, and team features.
//!
//! ## Tiers
//! - **Free / Self-Hosted**: Use your own API keys (OpenAI, Anthropic, Ollama).
//! - **PhazeAI Cloud** ($20/mo): Hosted models, no API key needed, cloud sync.
//! - **Team** ($50/seat/mo): Pair programming, shared agent context, audit logs.
//! - **Enterprise**: On-premise, SSO, SLA.

pub mod auth;
pub mod client;
pub mod subscription;

pub use auth::{CloudCredentials, CloudSession};
pub use client::CloudClient;
pub use subscription::Tier;

/// Cloud API base URL. Points to our hosted backend.
/// Override with PHAZEAI_CLOUD_URL env var for self-hosted enterprise deployments.
pub fn cloud_api_url() -> String {
    std::env::var("PHAZEAI_CLOUD_URL")
        .unwrap_or_else(|_| "https://api.phazeai.com/v1".to_string())
}
