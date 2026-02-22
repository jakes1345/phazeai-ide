use crate::error::PhazeError;
use crate::llm::provider::{ModelInfo, ProviderId};
use ollama_rs::Ollama;
use serde::Deserialize;

/// Discover locally available models from Ollama and LM Studio.
pub struct LocalDiscovery;

impl LocalDiscovery {
    /// Check if Ollama is running and list available models using ollama-rs.
    pub async fn ollama_models(base_url: &str) -> Result<Vec<ModelInfo>, PhazeError> {
        let ollama = Ollama::try_new(base_url)
            .map_err(|e| PhazeError::Llm(format!("Invalid Ollama URL: {e}")))?;

        let local_models = ollama
            .list_local_models()
            .await
            .map_err(|e| PhazeError::Llm(format!("Ollama not reachable: {e}")))?;

        let models = local_models
            .into_iter()
            .map(|m| {
                let context_window = estimate_context_window(&m.name);
                ModelInfo {
                    id: m.name.clone(),
                    name: format_model_name(&m.name, m.size),
                    context_window,
                    supports_tools: model_supports_tools(&m.name),
                    input_cost_per_m: 0.0,
                    output_cost_per_m: 0.0,
                }
            })
            .collect();

        Ok(models)
    }

    /// Check if Ollama is running.
    pub async fn ollama_available(base_url: &str) -> bool {
        if let Ok(ollama) = Ollama::try_new(base_url) {
            ollama.list_local_models().await.is_ok()
        } else {
            false
        }
    }

    /// Check if LM Studio is running and list models.
    pub async fn lm_studio_models(base_url: &str) -> Result<Vec<ModelInfo>, PhazeError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()?;

        let url = format!("{}/v1/models", base_url);
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| PhazeError::Llm(format!("LM Studio not reachable: {e}")))?;

        if !response.status().is_success() {
            return Err(PhazeError::Llm("LM Studio returned error".into()));
        }

        let body: OpenAIModelsResponse = response
            .json()
            .await
            .map_err(|e| PhazeError::Llm(format!("Failed to parse LM Studio response: {e}")))?;

        let models = body
            .data
            .into_iter()
            .map(|m| ModelInfo {
                name: m.id.clone(),
                id: m.id,
                context_window: 4096, // LM Studio doesn't expose this
                supports_tools: true,
                input_cost_per_m: 0.0,
                output_cost_per_m: 0.0,
            })
            .collect();

        Ok(models)
    }

    /// Check if LM Studio is running.
    pub async fn lm_studio_available(base_url: &str) -> bool {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .ok();

        if let Some(client) = client {
            let url = format!("{}/v1/models", base_url);
            client.get(&url).send().await.is_ok()
        } else {
            false
        }
    }

    /// Discover all available local providers and their models.
    pub async fn discover_all() -> Vec<(ProviderId, Vec<ModelInfo>)> {
        let mut results = Vec::new();

        // Check Ollama
        if let Ok(models) = Self::ollama_models("http://localhost:11434").await {
            if !models.is_empty() {
                results.push((ProviderId::Ollama, models));
            }
        }

        // Check LM Studio
        if let Ok(models) = Self::lm_studio_models("http://localhost:1234").await {
            if !models.is_empty() {
                results.push((ProviderId::LmStudio, models));
            }
        }

        results
    }
}

#[derive(Debug, Deserialize)]
struct OpenAIModelsResponse {
    data: Vec<OpenAIModelEntry>,
}

#[derive(Debug, Deserialize)]
struct OpenAIModelEntry {
    id: String,
}

fn format_model_name(name: &str, size: u64) -> String {
    if size > 0 {
        let gb = size as f64 / 1_073_741_824.0;
        format!("{} ({:.1}GB)", name, gb)
    } else {
        name.to_string()
    }
}

fn estimate_context_window(name: &str) -> usize {
    let lower = name.to_lowercase();
    if lower.contains("128k") {
        128_000
    } else if lower.contains("32k")
        || lower.contains("qwen")
        || lower.contains("deepseek")
        || lower.contains("codestral")
        || lower.contains("coder")
    {
        32_768
    } else if lower.contains("llama") || lower.contains("mistral") {
        8_192
    } else {
        4_096
    }
}

fn model_supports_tools(name: &str) -> bool {
    let lower = name.to_lowercase();
    // Most modern models support tools
    lower.contains("llama3")
        || lower.contains("mistral")
        || lower.contains("qwen")
        || lower.contains("codestral")
        || lower.contains("command-r")
        || lower.contains("firefunction")
        || lower.contains("hermes")
}
