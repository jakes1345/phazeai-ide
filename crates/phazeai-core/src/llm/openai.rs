use crate::error::PhazeError;
use crate::llm::traits::*;
use crate::tools::ToolDefinition;
use futures::channel::mpsc;
use serde::Deserialize;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct OpenAIClient {
    client: reqwest::Client,
    api_key: String,
    model: String,
    base_url: String,
    cache_hints: CacheHints,
    /// Ask for usage at the end of a stream (`stream_options.include_usage`).
    /// Cleared for good if the server rejects the field.
    stream_usage: AtomicBool,
}

/// What an OpenAI-compatible server is told so that requests sharing a
/// prefix reach the same cache. Each is sent only where the provider is known
/// to accept it: strict servers reject unknown body fields with a 400.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CacheHints {
    /// Body field `prompt_cache_key` (OpenAI and the providers that copy it).
    pub prompt_cache_key: Option<String>,
    /// Header `x-grok-conv-id` (xAI Chat Completions), which routes a
    /// conversation to the server holding its cache.
    pub grok_conv_id: Option<String>,
}

/// OpenAI caps `prompt_cache_key` at 64 characters.
const PROMPT_CACHE_KEY_MAX: usize = 64;

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

/// The token counts in an OpenAI-compatible response or final stream chunk,
/// or `None` when it carries none.
///
/// How much of the prompt came from the provider's cache is reported three
/// ways: `usage.prompt_tokens_details.cached_tokens` (OpenAI, xAI, and most
/// compatible servers), `usage.prompt_cache_hit_tokens` (DeepSeek, whose
/// `prompt_tokens` is the hit plus the miss count) and `timings` (llama.cpp's
/// llama-server: `cache_n` tokens reused and `prompt_n` tokens processed, which
/// together are the prompt). llama-server's `timings` is preferred because its
/// documentation calls `cache_n` the number reused from cache.
pub(crate) fn parse_usage(body: &Value) -> Option<Usage> {
    let count = |v: &Value, key: &str| v.get(key).and_then(Value::as_u64).map(|n| n as u32);
    let usage = body.get("usage").filter(|u| u.is_object());
    let timings = body.get("timings").filter(|t| t.is_object());
    let mut out = Usage::default();
    if let Some(u) = usage {
        out.input_tokens = count(u, "prompt_tokens").unwrap_or(0);
        out.output_tokens = count(u, "completion_tokens").unwrap_or(0);
        out.cache_read_tokens = u
            .get("prompt_tokens_details")
            .and_then(|d| count(d, "cached_tokens"))
            .or_else(|| count(u, "prompt_cache_hit_tokens"))
            .unwrap_or(0);
    }
    if let Some(t) = timings {
        if let (Some(processed), Some(reused)) = (count(t, "prompt_n"), count(t, "cache_n")) {
            out.input_tokens = processed + reused;
            out.cache_read_tokens = reused;
        }
    }
    (out.input_tokens > 0 || out.output_tokens > 0).then_some(out)
}

impl OpenAIClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
            model: crate::constants::models::DEFAULT_OPENAI_MODEL.to_string(),
            base_url: crate::constants::endpoints::OPENAI_BASE_URL.to_string(),
            cache_hints: CacheHints::default(),
            stream_usage: AtomicBool::new(true),
        }
    }

    /// Send these cache-routing hints with every request.
    pub fn with_cache_hints(mut self, mut hints: CacheHints) -> Self {
        if let Some(key) = hints.prompt_cache_key.as_mut() {
            if let Some((cut, _)) = key.char_indices().nth(PROMPT_CACHE_KEY_MAX) {
                key.truncate(cut);
            }
        }
        self.cache_hints = hints;
        self
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

/// One message in Chat Completions form.
fn chat_message(m: &Message) -> Value {
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
}

/// Whether an error body is a server refusing `stream_options`.
fn rejects_stream_options(status: reqwest::StatusCode, body: &str) -> bool {
    (status == reqwest::StatusCode::BAD_REQUEST
        || status == reqwest::StatusCode::UNPROCESSABLE_ENTITY)
        && body.contains("stream_options")
}

impl OpenAIClient {
    /// The request body. A stream asks for usage at its end while the server
    /// is not known to refuse that.
    fn request_body(&self, messages: &[Message], tools: &[ToolDefinition], stream: bool) -> Value {
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages.iter().map(chat_message).collect::<Vec<_>>(),
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(self.build_tool_defs(tools));
        }
        if stream {
            body["stream"] = Value::Bool(true);
            if self.stream_usage.load(Ordering::Relaxed) {
                body["stream_options"] = serde_json::json!({ "include_usage": true });
            }
        }
        if let Some(ref key) = self.cache_hints.prompt_cache_key {
            body["prompt_cache_key"] = Value::String(key.clone());
        }
        body
    }

    async fn post(&self, url: &str, body: &Value) -> Result<reqwest::Response, PhazeError> {
        crate::llm::retry::send_with_retry("openai", || {
            let mut request = self
                .client
                .post(url)
                .header("Authorization", format!("Bearer {}", self.api_key));
            if let Some(ref conv) = self.cache_hints.grok_conv_id {
                request = request.header("x-grok-conv-id", conv);
            }
            request.json(body).send()
        })
        .await
        .map_err(PhazeError::from)
    }
}

#[async_trait::async_trait]
impl LlmClient for OpenAIClient {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse, PhazeError> {
        let url = openai_compat_url(&self.base_url, "chat/completions");
        let request_body = self.request_body(messages, tools, false);
        let response = self.post(&url, &request_body).await?;

        let status = response.status();
        let response_text = response.text().await?;

        if !status.is_success() {
            return Err(PhazeError::Llm(format!(
                "OpenAI API error ({}): {}",
                status, response_text
            )));
        }

        let raw: Value = serde_json::from_str(&response_text)
            .map_err(|e| PhazeError::Llm(format!("Failed to parse response: {e}")))?;
        let usage = parse_usage(&raw);
        let api_response: OpenAIResponse = serde_json::from_value(raw)
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

        Ok(LlmResponse { message, usage })
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<mpsc::UnboundedReceiver<StreamEvent>, PhazeError> {
        let url = openai_compat_url(&self.base_url, "chat/completions");

        // Retry only the request submission; once the SSE stream begins we
        // can't safely re-issue without duplicating tokens to the consumer.
        let mut response = self
            .post(&url, &self.request_body(messages, tools, true))
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            if !(self.stream_usage.load(Ordering::Relaxed) && rejects_stream_options(status, &text))
            {
                return Err(PhazeError::Llm(format!(
                    "OpenAI API error ({}): {}",
                    status, text
                )));
            }
            // A server that refuses `stream_options` still streams; it just
            // reports no usage. Ask once more without it, and never again.
            tracing::debug!(
                "{} refused stream_options; streaming without usage",
                self.base_url
            );
            self.stream_usage.store(false, Ordering::Relaxed);
            response = self
                .post(&url, &self.request_body(messages, tools, true))
                .await?;
            if !response.status().is_success() {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                return Err(PhazeError::Llm(format!(
                    "OpenAI API error ({}): {}",
                    status, text
                )));
            }
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
            // Servers differ on where usage goes (its own last chunk, the
            // chunk with `finish_reason`, or every chunk with running
            // totals), so keep the latest and report it once, at the end.
            let mut usage: Option<Usage> = None;
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
                        if let Some(usage) = usage.take() {
                            let _ = tx.unbounded_send(StreamEvent::Usage(usage));
                        }
                        let _ = tx.unbounded_send(StreamEvent::Done);
                        return;
                    }

                    if let Ok(event) = serde_json::from_str::<Value>(data) {
                        if let Some(latest) = parse_usage(&event) {
                            usage = Some(latest);
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
            // tool calls or usage they streamed.
            finish_tool_calls(&mut tool_call_ids, &tx);
            if let Some(usage) = usage.take() {
                let _ = tx.unbounded_send(StreamEvent::Usage(usage));
            }
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

    /// One request as the server saw it: header lines and the JSON body.
    struct Seen {
        head: String,
        body: serde_json::Value,
    }

    /// Serve `replies` (status line, body) to successive connections,
    /// recording each request.
    async fn serve_seq(
        replies: Vec<(&'static str, String)>,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<Seen>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            for (status, body) in replies {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut raw = Vec::new();
                let mut buf = vec![0u8; 64 * 1024];
                // Read the head, then as much body as Content-Length says.
                let (head, body_start, len) = loop {
                    let n = sock.read(&mut buf).await.unwrap();
                    raw.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&raw).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let head = text[..end].to_string();
                        let len = head
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        break (head, end + 4, len);
                    }
                };
                while raw.len() < body_start + len {
                    let n = sock.read(&mut buf).await.unwrap();
                    raw.extend_from_slice(&buf[..n]);
                }
                let request = serde_json::from_slice(&raw[body_start..body_start + len])
                    .unwrap_or(serde_json::Value::Null);
                log.lock().unwrap().push(Seen {
                    head,
                    body: request,
                });
                let kind = if status.starts_with("200") {
                    "text/event-stream"
                } else {
                    "application/json"
                };
                let reply = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(reply.as_bytes()).await.unwrap();
                sock.shutdown().await.unwrap();
            }
        });
        (format!("http://{addr}"), seen)
    }

    async fn drain(mut rx: mpsc::UnboundedReceiver<StreamEvent>) -> (String, Vec<Usage>) {
        let (mut text, mut usage) = (String::new(), Vec::new());
        while let Some(ev) = rx.next().await {
            match ev {
                StreamEvent::TextDelta(t) => text.push_str(&t),
                StreamEvent::Usage(u) => usage.push(u),
                StreamEvent::Error(e) => panic!("{e}"),
                _ => {}
            }
        }
        (text, usage)
    }

    fn text_chunk(t: &str) -> String {
        chunk(serde_json::json!({"choices": [{"delta": {"content": t}}]}))
    }

    #[tokio::test]
    async fn a_stream_asks_for_usage_and_sends_the_cache_hints() {
        let body = [
            text_chunk("ok"),
            chunk(serde_json::json!({"choices": [], "usage": {
                "prompt_tokens": 2006, "completion_tokens": 3,
                "prompt_tokens_details": {"cached_tokens": 1920}}})),
            "data: [DONE]\n\n".to_string(),
        ]
        .concat();
        let (base, seen) = serve_seq(vec![("200 OK", body)]).await;
        let client = OpenAIClient::new("k")
            .with_base_url(base)
            .with_cache_hints(CacheHints {
                prompt_cache_key: Some("phaze-abc".into()),
                grok_conv_id: Some("phaze-abc".into()),
            });
        let rx = client
            .chat_stream(&[Message::user("hi")], &[])
            .await
            .unwrap();
        let (text, usage) = drain(rx).await;
        assert_eq!(text, "ok");
        assert_eq!(
            usage,
            vec![Usage {
                input_tokens: 2006,
                output_tokens: 3,
                cache_read_tokens: 1920,
                cache_write_tokens: 0,
            }]
        );
        let seen = seen.lock().unwrap();
        assert_eq!(
            seen[0].body["stream_options"],
            serde_json::json!({"include_usage": true})
        );
        assert_eq!(seen[0].body["prompt_cache_key"], "phaze-abc");
        assert!(seen[0]
            .head
            .lines()
            .any(|l| l.eq_ignore_ascii_case("x-grok-conv-id: phaze-abc")));
    }

    #[tokio::test]
    async fn a_request_without_hints_carries_neither() {
        let (base, seen) = serve_seq(vec![("200 OK", "data: [DONE]\n\n".into())]).await;
        let client = OpenAIClient::new("k").with_base_url(base);
        drain(
            client
                .chat_stream(&[Message::user("hi")], &[])
                .await
                .unwrap(),
        )
        .await;
        let seen = seen.lock().unwrap();
        assert!(seen[0].body.get("prompt_cache_key").is_none());
        assert!(!seen[0].head.to_ascii_lowercase().contains("x-grok-conv-id"));
    }

    #[tokio::test]
    async fn a_server_that_refuses_stream_options_is_asked_again_without_them() {
        let refusal =
            r#"{"error":{"message":"Unrecognized request argument supplied: stream_options"}}"#;
        let (base, seen) = serve_seq(vec![
            ("400 Bad Request", refusal.into()),
            (
                "200 OK",
                [text_chunk("first"), "data: [DONE]\n\n".into()].concat(),
            ),
            (
                "200 OK",
                [text_chunk("second"), "data: [DONE]\n\n".into()].concat(),
            ),
        ])
        .await;
        let client = OpenAIClient::new("k").with_base_url(base);
        let (text, _) = drain(
            client
                .chat_stream(&[Message::user("hi")], &[])
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(text, "first");
        let (text, _) = drain(
            client
                .chat_stream(&[Message::user("hi")], &[])
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(text, "second");
        let seen = seen.lock().unwrap();
        assert!(seen[0].body.get("stream_options").is_some());
        assert!(seen[1].body.get("stream_options").is_none());
        assert!(seen[2].body.get("stream_options").is_none(), "remembered");
    }

    #[tokio::test]
    async fn any_other_error_is_reported_not_retried() {
        let (base, seen) = serve_seq(vec![(
            "400 Bad Request",
            r#"{"error":{"message":"model not found"}}"#.into(),
        )])
        .await;
        let client = OpenAIClient::new("k").with_base_url(base);
        let Err(err) = client.chat_stream(&[Message::user("hi")], &[]).await else {
            panic!("a 400 that is not about stream_options must fail");
        };
        assert!(err.to_string().contains("model not found"));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn running_totals_are_reported_once_with_the_last_value() {
        let usage = |p: u32, c: u32| {
            chunk(serde_json::json!({"choices": [{"delta": {"content": "x"}}],
                "usage": {"prompt_tokens": p, "completion_tokens": c}}))
        };
        // Closes without [DONE].
        let body = [usage(500, 1), usage(500, 2), usage(500, 3)].concat();
        let (base, _) = serve_seq(vec![("200 OK", body)]).await;
        let client = OpenAIClient::new("k").with_base_url(base);
        let (_, usage) = drain(
            client
                .chat_stream(&[Message::user("hi")], &[])
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(usage.len(), 1);
        assert_eq!((usage[0].input_tokens, usage[0].output_tokens), (500, 3));
    }

    #[test]
    fn a_long_cache_key_is_cut_to_openais_limit() {
        let client = OpenAIClient::new("k").with_cache_hints(CacheHints {
            prompt_cache_key: Some("é".repeat(80)),
            grok_conv_id: None,
        });
        let body = client.request_body(&[Message::user("hi")], &[], false);
        assert_eq!(
            body["prompt_cache_key"].as_str().unwrap().chars().count(),
            64
        );
        assert!(
            body.get("stream_options").is_none(),
            "only streams ask for usage"
        );
        assert!(body.get("stream").is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn openai_and_xai_report_cached_tokens_in_the_prompt_details() {
        let body = json!({ "usage": {
            "prompt_tokens": 3000,
            "completion_tokens": 40,
            "prompt_tokens_details": { "cached_tokens": 2560 },
        }});
        let usage = parse_usage(&body).unwrap();
        assert_eq!(
            usage,
            Usage {
                input_tokens: 3000,
                output_tokens: 40,
                cache_read_tokens: 2560,
                cache_write_tokens: 0,
            }
        );
    }

    #[test]
    fn deepseek_reports_its_cache_hits_beside_the_prompt_total() {
        let body = json!({ "usage": {
            "prompt_tokens": 1200,
            "completion_tokens": 7,
            "prompt_cache_hit_tokens": 1024,
            "prompt_cache_miss_tokens": 176,
        }});
        let usage = parse_usage(&body).unwrap();
        assert_eq!((usage.input_tokens, usage.cache_read_tokens), (1200, 1024));
    }

    #[test]
    fn llama_server_timings_give_the_reused_and_processed_prompt_tokens() {
        // `usage` alone says nothing was cached; `timings` says 236 tokens
        // were reused and 12 processed, so the prompt was 248 tokens.
        let body = json!({
            "usage": {
                "prompt_tokens": 12,
                "completion_tokens": 30,
                "prompt_tokens_details": { "cached_tokens": 0 },
            },
            "timings": { "prompt_n": 12, "cache_n": 236 },
        });
        let usage = parse_usage(&body).unwrap();
        assert_eq!(
            (
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_tokens
            ),
            (248, 30, 236)
        );
    }

    #[test]
    fn a_body_without_counts_has_no_usage() {
        assert_eq!(parse_usage(&json!({ "choices": [] })), None);
        assert_eq!(parse_usage(&json!({ "usage": null })), None);
    }
}
