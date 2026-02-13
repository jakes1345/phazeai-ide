use crate::error::PhazeError;
use crate::llm::traits::*;
use crate::tools::ToolDefinition;
use futures::channel::mpsc;
use serde::Deserialize;
use serde_json::Value;

/// Client for local Ollama models (localhost:11434).
pub struct OllamaClient {
    client: reqwest::Client,
    model: String,
    base_url: String,
}

impl OllamaClient {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            model: model.into(),
            base_url: "http://localhost:11434".to_string(),
        }
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }
}

#[derive(Debug, Deserialize)]
struct OllamaChatResponse {
    message: OllamaMessage,
}

#[derive(Debug, Deserialize)]
struct OllamaMessage {
    content: String,
}

#[derive(Debug, Deserialize)]
struct OllamaStreamChunk {
    message: OllamaMessage,
    done: bool,
}

#[async_trait::async_trait]
impl LlmClient for OllamaClient {
    async fn chat(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse, PhazeError> {
        let url = format!("{}/api/chat", self.base_url);

        let ollama_messages: Vec<Value> = messages
            .iter()
            .map(|m| {
                serde_json::json!({
                    "role": m.role,
                    "content": m.content,
                })
            })
            .collect();

        let body = serde_json::json!({
            "model": self.model,
            "messages": ollama_messages,
            "stream": false,
        });

        let response = self.client.post(&url).json(&body).send().await?;

        let status = response.status();
        let response_text = response.text().await?;

        if !status.is_success() {
            return Err(PhazeError::Llm(format!(
                "Ollama error ({}): {}",
                status, response_text
            )));
        }

        let api_response: OllamaChatResponse = serde_json::from_str(&response_text)
            .map_err(|e| PhazeError::Llm(format!("Failed to parse Ollama response: {e}")))?;

        Ok(LlmResponse {
            message: Message::assistant(api_response.message.content),
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> Result<mpsc::UnboundedReceiver<StreamEvent>, PhazeError> {
        let url = format!("{}/api/chat", self.base_url);

        let ollama_messages: Vec<Value> = messages
            .iter()
            .map(|m| {
                serde_json::json!({
                    "role": m.role,
                    "content": m.content,
                })
            })
            .collect();

        let body = serde_json::json!({
            "model": self.model,
            "messages": ollama_messages,
            "stream": true,
        });

        let response = self.client.post(&url).json(&body).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(PhazeError::Llm(format!(
                "Ollama error ({}): {}",
                status, text
            )));
        }

        let (tx, rx) = mpsc::unbounded();

        let mut stream = response.bytes_stream();
        tokio::spawn(async move {
            use futures::StreamExt;
            let mut buffer = String::new();

            while let Some(chunk) = stream.next().await {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx.unbounded_send(StreamEvent::Error(e.to_string()));
                        break;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    if line.is_empty() {
                        continue;
                    }

                    if let Ok(chunk) = serde_json::from_str::<OllamaStreamChunk>(&line) {
                        if !chunk.message.content.is_empty() {
                            let _ = tx.unbounded_send(StreamEvent::TextDelta(
                                chunk.message.content,
                            ));
                        }
                        if chunk.done {
                            let _ = tx.unbounded_send(StreamEvent::Done);
                            return;
                        }
                    }
                }
            }

            let _ = tx.unbounded_send(StreamEvent::Done);
        });

        Ok(rx)
    }
}
