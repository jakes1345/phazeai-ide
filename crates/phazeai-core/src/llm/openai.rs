use crate::error::PhazeError;
use crate::llm::traits::*;
use crate::tools::ToolDefinition;
use futures::channel::mpsc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub struct OpenAIClient {
    client: reqwest::Client,
    api_key: String,
    model: String,
    base_url: String,
}

/// Build `<base_url>/<path>` for an OpenAI-compatible API.
///
/// Providers document their base URL differently: some stop at the host
/// (`https://api.openai.com`) and some already carry the version segment
/// (`https://api.mistral.ai/v1`, `.../v1beta/openai/`). Appending `/v1`
/// unconditionally produced `.../v1/v1/chat/completions` for the latter, so only
/// add it when the base has no version segment of its own.
pub(crate) fn openai_compat_url(base_url: &str, path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    if base.split('/').any(is_version_segment) {
        format!("{base}/{path}")
    } else {
        format!("{base}/v1/{path}")
    }
}

/// `v1`, `v2`, `v1beta`, `v1alpha`, ... but not hosts like `v1.example.com`.
fn is_version_segment(seg: &str) -> bool {
    let Some(rest) = seg.strip_prefix('v') else {
        return false;
    };
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    digits > 0 && matches!(&rest[digits..], "" | "beta" | "alpha")
}

impl OpenAIClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
            model: crate::constants::models::DEFAULT_OPENAI_MODEL.to_string(),
            base_url: crate::constants::endpoints::OPENAI_BASE_URL.to_string(),
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

    /// Connect directly, ignoring `HTTP(S)_PROXY`. For servers on 127.0.0.1:
    /// a corporate proxy can't reach the user's loopback interface.
    pub fn without_proxy(mut self) -> Self {
        if let Ok(client) = reqwest::Client::builder().no_proxy().build() {
            self.client = client;
        }
        self
    }

    fn build_tool_defs(&self, tools: &[ToolDefinition]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect()
    }
}

#[derive(Debug, Deserialize)]
struct OpenAIResponse {
    choices: Vec<OpenAIChoice>,
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    message: OpenAIMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAIMessage {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OpenAIToolCall>,
}

#[derive(Debug, Deserialize)]
struct OpenAIToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: OpenAIFunction,
}

#[derive(Debug, Deserialize)]
struct OpenAIFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct OpenAIUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

#[derive(Debug, Serialize)]
struct OpenAIRequest {
    model: String,
    messages: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[async_trait::async_trait]
impl LlmClient for OpenAIClient {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse, PhazeError> {
        let url = openai_compat_url(&self.base_url, "chat/completions");

        let oai_messages: Vec<Value> = messages
            .iter()
            .map(|m| {
                if let Some(ref tool_call_id) = m.tool_call_id {
                    serde_json::json!({
                        "role": "tool",
                        "tool_call_id": tool_call_id,
                        "content": m.content,
                    })
                } else if let Some(ref tool_calls) = m.tool_calls {
                    let tcs: Vec<Value> = tool_calls
                        .iter()
                        .map(|tc| {
                            serde_json::json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.function.name,
                                    "arguments": tc.function.arguments,
                                }
                            })
                        })
                        .collect();
                    serde_json::json!({
                        "role": "assistant",
                        "content": m.content,
                        "tool_calls": tcs,
                    })
                } else {
                    serde_json::json!({
                        "role": m.role,
                        "content": m.content,
                    })
                }
            })
            .collect();

        let request_body = OpenAIRequest {
            model: self.model.clone(),
            messages: oai_messages,
            tools: if tools.is_empty() {
                None
            } else {
                Some(self.build_tool_defs(tools))
            },
            stream: None,
        };

        let response = crate::llm::retry::send_with_retry("openai", || {
            self.client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .json(&request_body)
                .send()
        })
        .await?;

        let status = response.status();
        let response_text = response.text().await?;

        if !status.is_success() {
            return Err(PhazeError::Llm(format!(
                "OpenAI API error ({}): {}",
                status, response_text
            )));
        }

        let api_response: OpenAIResponse = serde_json::from_str(&response_text)
            .map_err(|e| PhazeError::Llm(format!("Failed to parse response: {e}")))?;

        let choice = api_response
            .choices
            .first()
            .ok_or_else(|| PhazeError::Llm("No response from API".into()))?;

        let content = choice.message.content.clone().unwrap_or_default();

        let tool_calls: Vec<ToolCall> = choice
            .message
            .tool_calls
            .iter()
            .map(|tc| ToolCall {
                id: tc.id.clone(),
                call_type: tc.call_type.clone(),
                function: FunctionCall {
                    name: tc.function.name.clone(),
                    arguments: tc.function.arguments.clone(),
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
                input_tokens: u.prompt_tokens,
                output_tokens: u.completion_tokens,
            }),
        })
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<mpsc::UnboundedReceiver<StreamEvent>, PhazeError> {
        let url = openai_compat_url(&self.base_url, "chat/completions");

        let oai_messages: Vec<Value> = messages
            .iter()
            .map(|m| {
                if let Some(ref tool_call_id) = m.tool_call_id {
                    serde_json::json!({
                        "role": "tool",
                        "tool_call_id": tool_call_id,
                        "content": m.content,
                    })
                } else if let Some(ref tool_calls) = m.tool_calls {
                    let tcs: Vec<Value> = tool_calls
                        .iter()
                        .map(|tc| {
                            serde_json::json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.function.name,
                                    "arguments": tc.function.arguments,
                                }
                            })
                        })
                        .collect();
                    serde_json::json!({
                        "role": "assistant",
                        "content": m.content,
                        "tool_calls": tcs,
                    })
                } else {
                    serde_json::json!({
                        "role": m.role,
                        "content": m.content,
                    })
                }
            })
            .collect();

        let request_body = OpenAIRequest {
            model: self.model.clone(),
            messages: oai_messages,
            tools: if tools.is_empty() {
                None
            } else {
                Some(self.build_tool_defs(tools))
            },
            stream: Some(true),
        };

        // Retry only the request submission; once the SSE stream begins we
        // can't safely re-issue without duplicating tokens to the consumer.
        let response = crate::llm::retry::send_with_retry("openai", || {
            self.client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .json(&request_body)
                .send()
        })
        .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(PhazeError::Llm(format!(
                "OpenAI API error ({}): {}",
                status, text
            )));
        }

        let (tx, rx) = mpsc::unbounded();

        let mut stream = response.bytes_stream();
        tokio::spawn(async move {
            use futures::StreamExt;
            let mut buffer = String::new();
            // Maps tool_call index → id, since OpenAI only sends id on the first
            // delta chunk. Ordered by index so parallel calls finish (and run)
            // in the order the model wrote them.
            let mut tool_call_ids: std::collections::BTreeMap<u64, String> =
                std::collections::BTreeMap::new();
            let finish_tool_calls =
                |ids: &mut std::collections::BTreeMap<u64, String>,
                 tx: &mpsc::UnboundedSender<StreamEvent>| {
                    for (_, id) in std::mem::take(ids) {
                        let _ = tx.unbounded_send(StreamEvent::ToolCallEnd { id });
                    }
                };

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

                    if line.is_empty() || !line.starts_with("data: ") {
                        continue;
                    }

                    let data = &line[6..];
                    if data == "[DONE]" {
                        // Finalize any tool calls that didn't get an explicit end event
                        finish_tool_calls(&mut tool_call_ids, &tx);
                        let _ = tx.unbounded_send(StreamEvent::Done);
                        return;
                    }

                    if let Ok(event) = serde_json::from_str::<Value>(data) {
                        // OpenAI stream_options: { include_usage: true } emits usage
                        // on the final chunk (choices=[]) — capture it here.
                        if let Some(usage) = event.get("usage") {
                            let input = usage
                                .get("prompt_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0) as u32;
                            let output = usage
                                .get("completion_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0) as u32;
                            if input > 0 || output > 0 {
                                let _ = tx.unbounded_send(StreamEvent::Usage(crate::llm::Usage {
                                    input_tokens: input,
                                    output_tokens: output,
                                }));
                            }
                        }

                        if let Some(choices) = event.get("choices").and_then(|c| c.as_array()) {
                            if let Some(delta) = choices.first().and_then(|c| c.get("delta")) {
                                if let Some(content) = delta.get("content").and_then(|c| c.as_str())
                                {
                                    let _ = tx.unbounded_send(StreamEvent::TextDelta(
                                        content.to_string(),
                                    ));
                                }

                                if let Some(tool_calls) =
                                    delta.get("tool_calls").and_then(|t| t.as_array())
                                {
                                    for tc in tool_calls {
                                        let index =
                                            tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                                        // Store id by index on first chunk (id absent on subsequent chunks)
                                        if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                                            if !id.is_empty() {
                                                tool_call_ids.insert(index, id.to_string());
                                            }
                                        }
                                        let id =
                                            tool_call_ids.get(&index).cloned().unwrap_or_default();
                                        if let Some(func) = tc.get("function") {
                                            if let Some(name) =
                                                func.get("name").and_then(|n| n.as_str())
                                            {
                                                if !name.is_empty() {
                                                    let _ = tx.unbounded_send(
                                                        StreamEvent::ToolCallStart {
                                                            id: id.clone(),
                                                            name: name.to_string(),
                                                        },
                                                    );
                                                }
                                            }
                                            if let Some(args) =
                                                func.get("arguments").and_then(|a| a.as_str())
                                            {
                                                if !args.is_empty() {
                                                    let _ = tx.unbounded_send(
                                                        StreamEvent::ToolCallDelta {
                                                            id: id.clone(),
                                                            arguments_delta: args.to_string(),
                                                        },
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Some servers close the stream without `[DONE]`; don't lose the
            // tool calls they streamed.
            finish_tool_calls(&mut tool_call_ids, &tx);
            let _ = tx.unbounded_send(StreamEvent::Done);
        });

        Ok(rx)
    }
}

#[cfg(test)]
mod url_tests {
    use super::openai_compat_url;

    fn chat(base: &str) -> String {
        openai_compat_url(base, "chat/completions")
    }

    #[test]
    fn host_only_bases_get_v1_added() {
        assert_eq!(
            chat("https://api.openai.com"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            chat("https://api.together.xyz"),
            "https://api.together.xyz/v1/chat/completions"
        );
        assert_eq!(
            chat("http://localhost:1234"),
            "http://localhost:1234/v1/chat/completions"
        );
        // Groq and OpenRouter keep a path prefix but no version segment.
        assert_eq!(
            chat("https://api.groq.com/openai"),
            "https://api.groq.com/openai/v1/chat/completions"
        );
        assert_eq!(
            chat("https://openrouter.ai/api"),
            "https://openrouter.ai/api/v1/chat/completions"
        );
    }

    #[test]
    fn versioned_bases_are_not_double_versioned() {
        assert_eq!(
            chat("https://api.mistral.ai/v1"),
            "https://api.mistral.ai/v1/chat/completions"
        );
        assert_eq!(
            chat("https://api.cerebras.ai/v1"),
            "https://api.cerebras.ai/v1/chat/completions"
        );
        assert_eq!(
            chat("https://api.fireworks.ai/inference/v1"),
            "https://api.fireworks.ai/inference/v1/chat/completions"
        );
        assert_eq!(
            chat("https://api.cohere.com/compatibility/v1"),
            "https://api.cohere.com/compatibility/v1/chat/completions"
        );
        // Gemini's OpenAI-compat base: trailing slash, v1beta segment.
        assert_eq!(
            chat("https://generativelanguage.googleapis.com/v1beta/openai/"),
            "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
        );
    }

    #[test]
    fn trailing_slashes_and_leading_path_slashes_are_tolerated() {
        assert_eq!(
            openai_compat_url("https://api.mistral.ai/v1//", "/models"),
            "https://api.mistral.ai/v1/models"
        );
        assert_eq!(
            openai_compat_url("https://api.openai.com/", "models"),
            "https://api.openai.com/v1/models"
        );
    }

    #[test]
    fn a_version_like_hostname_is_not_a_version_segment() {
        assert_eq!(
            chat("https://v1.example.com"),
            "https://v1.example.com/v1/chat/completions"
        );
        assert_eq!(
            chat("https://example.com/v2"),
            "https://example.com/v2/chat/completions"
        );
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;
    use futures::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Serve one streaming chat response from a real local socket.
    async fn serve_once(body: String) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 64 * 1024];
            let _ = sock.read(&mut buf).await;
            let head =
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n";
            sock.write_all(head.as_bytes()).await.unwrap();
            sock.write_all(body.as_bytes()).await.unwrap();
            sock.shutdown().await.unwrap();
        });
        format!("http://{addr}")
    }

    fn chunk(v: serde_json::Value) -> String {
        format!("data: {v}\n\n")
    }

    #[tokio::test]
    async fn parallel_tool_calls_end_in_index_order_even_without_done() {
        let tc = |index: u64, id: Option<&str>, name: Option<&str>, args: &str| {
            let mut call = serde_json::json!({"index": index, "function": {"arguments": args}});
            if let Some(id) = id {
                call["id"] = id.into();
            }
            if let Some(name) = name {
                call["function"]["name"] = name.into();
            }
            chunk(serde_json::json!({"choices": [{"delta": {"tool_calls": [call]}}]}))
        };
        // Calls interleaved, and the stream closes with no `[DONE]`.
        let body = [
            tc(0, Some("call_a"), Some("read_file"), ""),
            tc(1, Some("call_b"), Some("grep"), ""),
            tc(1, None, None, "{\"pattern\":\"x\"}"),
            tc(0, None, None, "{\"path\":\"a\"}"),
        ]
        .concat();
        let base = serve_once(body).await;
        let client = OpenAIClient::new("test-key").with_base_url(base);
        let mut rx = client
            .chat_stream(&[Message::user("hi")], &[])
            .await
            .unwrap();
        let mut ended = Vec::new();
        let mut done = false;
        while let Some(ev) = rx.next().await {
            match ev {
                StreamEvent::ToolCallEnd { id } => ended.push(id),
                StreamEvent::Done => done = true,
                StreamEvent::Error(e) => panic!("{e}"),
                _ => {}
            }
        }
        assert_eq!(ended, vec!["call_a", "call_b"]);
        assert!(done);
    }
}
