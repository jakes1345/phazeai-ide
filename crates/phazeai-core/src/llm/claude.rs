use crate::error::PhazeError;
use crate::llm::traits::*;
use crate::tools::ToolDefinition;
use futures::channel::mpsc;
use serde::Deserialize;
use serde_json::Value;

pub struct ClaudeClient {
    client: reqwest::Client,
    api_key: String,
    model: String,
    base_url: String,
    max_tokens: u32,
}

impl ClaudeClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
            model: "claude-sonnet-4-5-20250929".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            max_tokens: 8192,
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    pub fn with_max_tokens(mut self, max: u32) -> Self {
        self.max_tokens = max;
        self
    }

    fn build_request_body(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        stream: bool,
    ) -> Value {
        // Separate system messages from conversation messages
        let system_prompt: String = messages
            .iter()
            .filter(|m| m.role == Role::System)
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");

        let conv_messages: Vec<Value> = messages
            .iter()
            .filter(|m| m.role != Role::System)
            .map(|m| {
                if let Some(ref tool_call_id) = m.tool_call_id {
                    serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": tool_call_id,
                            "content": m.content,
                        }]
                    })
                } else if let Some(ref tool_calls) = m.tool_calls {
                    let mut content: Vec<Value> = Vec::new();
                    if !m.content.is_empty() {
                        content.push(serde_json::json!({
                            "type": "text",
                            "text": m.content,
                        }));
                    }
                    for tc in tool_calls {
                        let args: Value = serde_json::from_str(&tc.function.arguments)
                            .unwrap_or(Value::Object(Default::default()));
                        content.push(serde_json::json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.function.name,
                            "input": args,
                        }));
                    }
                    serde_json::json!({
                        "role": "assistant",
                        "content": content,
                    })
                } else {
                    serde_json::json!({
                        "role": m.role,
                        "content": m.content,
                    })
                }
            })
            .collect();

        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "messages": conv_messages,
        });

        if !system_prompt.is_empty() {
            body["system"] = Value::String(system_prompt);
        }

        if !tools.is_empty() {
            let tool_defs: Vec<Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect();
            body["tools"] = Value::Array(tool_defs);
        }

        if stream {
            body["stream"] = Value::Bool(true);
        }

        body
    }
}

#[derive(Debug, Deserialize)]
struct ClaudeApiResponse {
    content: Vec<ClaudeContent>,
    usage: Option<ClaudeUsage>,
}

#[derive(Debug, Deserialize)]
struct ClaudeContent {
    #[serde(rename = "type")]
    content_type: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Value,
}

#[derive(Debug, Deserialize)]
struct ClaudeUsage {
    input_tokens: u32,
    output_tokens: u32,
}

#[async_trait::async_trait]
impl LlmClient for ClaudeClient {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse, PhazeError> {
        let url = format!("{}/v1/messages", self.base_url);
        let request_body = self.build_request_body(messages, tools, false);

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request_body)
            .send()
            .await?;

        let status = response.status();
        let response_text = response.text().await?;

        if !status.is_success() {
            return Err(PhazeError::Llm(format!(
                "Claude API error ({}): {}",
                status, response_text
            )));
        }

        let api_response: ClaudeApiResponse = serde_json::from_str(&response_text)
            .map_err(|e| PhazeError::Llm(format!("Failed to parse response: {e}")))?;

        let content = api_response
            .content
            .iter()
            .find(|c| c.content_type == "text")
            .map(|c| c.text.clone())
            .unwrap_or_default();

        let tool_calls: Vec<ToolCall> = api_response
            .content
            .iter()
            .filter(|c| c.content_type == "tool_use")
            .map(|c| ToolCall {
                id: c.id.clone().unwrap_or_default(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: c.name.clone().unwrap_or_default(),
                    arguments: serde_json::to_string(&c.input).unwrap_or_default(),
                },
            })
            .collect();

        let message = if tool_calls.is_empty() {
            Message::assistant(content)
        } else {
            Message::assistant_with_tools(content, tool_calls)
        };

        Ok(LlmResponse {
            message,
            usage: api_response.usage.map(|u| Usage {
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
            }),
        })
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<mpsc::UnboundedReceiver<StreamEvent>, PhazeError> {
        let url = format!("{}/v1/messages", self.base_url);
        let request_body = self.build_request_body(messages, tools, true);

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request_body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(PhazeError::Llm(format!(
                "Claude API error ({}): {}",
                status, text
            )));
        }

        let (tx, rx) = mpsc::unbounded();

        let mut stream = response.bytes_stream();
        tokio::spawn(async move {
            use futures::StreamExt;
            let mut buffer = String::new();
            // Maps content_block index → tool_use id, so delta events can find their tool call
            let mut tool_block_ids: std::collections::HashMap<u64, String> =
                std::collections::HashMap::new();

            while let Some(chunk) = stream.next().await {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx.unbounded_send(StreamEvent::Error(e.to_string()));
                        break;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Process SSE lines
                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    if line.is_empty() || !line.starts_with("data: ") {
                        continue;
                    }

                    let data = &line[6..];
                    if data == "[DONE]" {
                        let _ = tx.unbounded_send(StreamEvent::Done);
                        return;
                    }

                    if let Ok(event) = serde_json::from_str::<Value>(data) {
                        let event_type = event.get("type").and_then(|t| t.as_str());
                        match event_type {
                            Some("content_block_delta") => {
                                if let Some(delta) = event.get("delta") {
                                    let delta_type = delta.get("type").and_then(|t| t.as_str());
                                    match delta_type {
                                        Some("text_delta") => {
                                            if let Some(text) =
                                                delta.get("text").and_then(|t| t.as_str())
                                            {
                                                let _ = tx.unbounded_send(StreamEvent::TextDelta(
                                                    text.to_string(),
                                                ));
                                            }
                                        }
                                        Some("input_json_delta") => {
                                            if let Some(partial) =
                                                delta.get("partial_json").and_then(|t| t.as_str())
                                            {
                                                let index = event
                                                    .get("index")
                                                    .and_then(|i| i.as_u64())
                                                    .unwrap_or(0);
                                                if let Some(id) = tool_block_ids.get(&index) {
                                                    let _ = tx.unbounded_send(
                                                        StreamEvent::ToolCallDelta {
                                                            id: id.clone(),
                                                            arguments_delta: partial.to_string(),
                                                        },
                                                    );
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            Some("content_block_start") => {
                                if let Some(cb) = event.get("content_block") {
                                    if cb.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                                        let index = event
                                            .get("index")
                                            .and_then(|i| i.as_u64())
                                            .unwrap_or(0);
                                        let id = cb
                                            .get("id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        let name = cb
                                            .get("name")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        tool_block_ids.insert(index, id.clone());
                                        let _ = tx.unbounded_send(StreamEvent::ToolCallStart {
                                            id,
                                            name,
                                        });
                                    }
                                }
                            }
                            Some("content_block_stop") => {
                                let index =
                                    event.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                                if let Some(id) = tool_block_ids.remove(&index) {
                                    let _ = tx.unbounded_send(StreamEvent::ToolCallEnd { id });
                                }
                            }
                            Some("message_stop") => {
                                let _ = tx.unbounded_send(StreamEvent::Done);
                                return;
                            }
                            _ => {}
                        }
                    }
                }
            }

            let _ = tx.unbounded_send(StreamEvent::Done);
        });

        Ok(rx)
    }
}
