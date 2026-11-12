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
            model: crate::constants::models::DEFAULT_CLAUDE_MODEL.to_string(),
            base_url: crate::constants::endpoints::CLAUDE_BASE_URL.to_string(),
            max_tokens: 8192,
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Connect directly, ignoring `HTTP(S)_PROXY` (for a server on this machine).
    pub fn without_proxy(mut self) -> Self {
        if let Ok(client) = reqwest::Client::builder().no_proxy().build() {
            self.client = client;
        }
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

    /// `<base_url>/v1/messages`. A trailing slash on the base URL is
    /// tolerated, as `openai_compat_url` tolerates it for the other clients.
    fn messages_url(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }

    fn build_request_body(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        stream: bool,
    ) -> Value {
        let system: Vec<Value> = messages
            .iter()
            .filter(|m| m.role == Role::System)
            .flat_map(|m| system_blocks(&m.content))
            .collect();
        let conversation = conversation_messages(messages);

        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "messages": conversation,
        });

        if !system.is_empty() {
            body["system"] = Value::Array(system);
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

        place_cache_breakpoints(&mut body);
        body
    }
}

/// Anthropic caches a prompt prefix only up to a `cache_control` breakpoint,
/// and takes at most four per request.
const MAX_BREAKPOINTS: usize = 4;

fn ephemeral() -> Value {
    serde_json::json!({ "type": "ephemeral" })
}

/// A system message as content blocks: the part every session shares (the
/// base prompt and the project's instructions), then the part that is this
/// session's own (its git state and the summary of compacted history), each in
/// a block of its own so a breakpoint can sit between them. Empty blocks are
/// left out, as the API refuses them.
fn system_blocks(text: &str) -> Vec<Value> {
    let summary = format!("\n\n{}", crate::context::SUMMARY_HEADING);
    let split = if text.starts_with(crate::context::SUMMARY_HEADING) {
        0
    } else {
        [
            text.find(crate::context::system_prompt::GIT_HEADING),
            text.find(&summary),
        ]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(text.len())
    };
    let (shared, own) = text.split_at(split);
    [shared, own.trim_start_matches('\n')]
        .into_iter()
        .filter(|t| !t.trim().is_empty())
        .map(|t| serde_json::json!({ "type": "text", "text": t }))
        .collect()
}

/// The conversation in Anthropic's shape. The results of one turn's tool calls
/// go back in a single user message, as the API documents for parallel calls
/// (sending each in a message of its own teaches the model to stop calling
/// tools in parallel).
fn conversation_messages(messages: &[Message]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for m in messages.iter().filter(|m| m.role != Role::System) {
        if let Some(ref tool_call_id) = m.tool_call_id {
            let result = serde_json::json!({
                "type": "tool_result",
                "tool_use_id": tool_call_id,
                "content": m.content,
            });
            let previous_is_results = out.last().is_some_and(|prev: &Value| {
                prev["role"] == "user"
                    && prev["content"]
                        .as_array()
                        .is_some_and(|c| c.iter().all(|b| b["type"] == "tool_result"))
            });
            if previous_is_results {
                if let Some(content) = out
                    .last_mut()
                    .and_then(|prev| prev["content"].as_array_mut())
                {
                    content.push(result);
                }
            } else {
                out.push(serde_json::json!({ "role": "user", "content": [result] }));
            }
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
            out.push(serde_json::json!({
                "role": "assistant",
                "content": content,
            }));
        } else {
            out.push(serde_json::json!({
                "role": m.role,
                "content": m.content,
            }));
        }
    }
    out
}

/// Mark the prefixes worth caching, in render order (tools, then system, then
/// messages): the last tool definition (the tool schemas, the same for every
/// session), the first and last system blocks (the shared base prompt, then
/// this session's own part) and the conversation tail, which moves forward
/// after each tool result so every request reads what the one before it
/// wrote. This is the placement opencode uses by default
/// (`packages/ai/src/cache-policy.ts`).
fn place_cache_breakpoints(body: &mut Value) {
    let mut placed = 0;
    if let Some(last_tool) = body
        .get_mut("tools")
        .and_then(Value::as_array_mut)
        .and_then(|tools| tools.last_mut())
    {
        last_tool["cache_control"] = ephemeral();
        placed += 1;
    }
    if let Some(system) = body.get_mut("system").and_then(Value::as_array_mut) {
        let last = system.len().saturating_sub(1);
        for i in [0, last] {
            if let Some(block) = system.get_mut(i) {
                if block.get("cache_control").is_none() {
                    block["cache_control"] = ephemeral();
                    placed += 1;
                }
            }
        }
    }
    if placed < MAX_BREAKPOINTS {
        if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) {
            mark_conversation_tail(messages);
        }
    }
}

/// Put a breakpoint on the last block of the conversation that can carry one.
/// A plain string content becomes a text block, since only blocks take
/// `cache_control`; an empty text (which the API refuses as a block) is passed
/// over for the message before it.
fn mark_conversation_tail(messages: &mut [Value]) {
    for message in messages.iter_mut().rev() {
        let content = &mut message["content"];
        if let Some(text) = content.as_str() {
            if text.is_empty() {
                continue;
            }
            *content = serde_json::json!([{
                "type": "text",
                "text": text,
                "cache_control": ephemeral(),
            }]);
            return;
        }
        let block = content.as_array_mut().and_then(|blocks| {
            blocks
                .iter_mut()
                .rev()
                .find(|b| b["type"] != "text" || b["text"].as_str().is_some_and(|t| !t.is_empty()))
        });
        if let Some(block) = block {
            block["cache_control"] = ephemeral();
            return;
        }
    }
}

/// Anthropic's token counts as a [`Usage`]. Its `input_tokens` is only the
/// part of the prompt after the last cache breakpoint that was neither read
/// from nor written to the cache, so the whole prompt is the sum of the three.
///
/// A stream reports the counts in two events: `message_start` carries the
/// input side and `message_delta` the output (and, from newer API versions,
/// the input side again, cumulatively). Fields in `delta` win over `start`.
fn claude_usage(start: Option<&Value>, delta: Option<&Value>) -> Option<Usage> {
    let field = |key: &str| -> u32 {
        delta
            .and_then(|d| d.get(key))
            .and_then(Value::as_u64)
            .or_else(|| start.and_then(|s| s.get(key)).and_then(Value::as_u64))
            .unwrap_or(0) as u32
    };
    let uncached = field("input_tokens");
    let read = field("cache_read_input_tokens");
    let written = field("cache_creation_input_tokens");
    let usage = Usage {
        input_tokens: uncached + read + written,
        output_tokens: field("output_tokens"),
        cache_read_tokens: read,
        cache_write_tokens: written,
    };
    (usage.input_tokens > 0 || usage.output_tokens > 0).then_some(usage)
}

#[derive(Debug, Deserialize)]
struct ClaudeApiResponse {
    content: Vec<ClaudeContent>,
    usage: Option<Value>,
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

#[async_trait::async_trait]
impl LlmClient for ClaudeClient {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse, PhazeError> {
        let url = self.messages_url();
        let request_body = self.build_request_body(messages, tools, false);

        let response = crate::llm::retry::send_with_retry("claude", || {
            self.client
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&request_body)
                .send()
        })
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
            usage: claude_usage(None, api_response.usage.as_ref()),
        })
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<mpsc::UnboundedReceiver<StreamEvent>, PhazeError> {
        let url = self.messages_url();
        let request_body = self.build_request_body(messages, tools, true);

        // Retry only the request submission; once the SSE stream starts we
        // can't safely re-issue without duplicating tokens.
        let response = crate::llm::retry::send_with_retry("claude", || {
            self.client
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&request_body)
                .send()
        })
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
            let mut buffer = String::new();
            // Maps content_block index → tool_use id, so delta events can find their tool call
            let mut tool_block_ids: std::collections::HashMap<u64, String> =
                std::collections::HashMap::new();
            // The input-side counts arrive in `message_start`, the output in
            // `message_delta`; one Usage event is sent from the two.
            let mut start_usage: Option<Value> = None;
            let mut usage_sent = false;

            while let Some(chunk) =
                super::stream_cancel::next_unless_dropped(&mut stream, &tx).await
            {
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
                            Some("message_start") => {
                                start_usage = event.pointer("/message/usage").cloned();
                            }
                            Some("message_delta") => {
                                if let Some(usage) =
                                    claude_usage(start_usage.as_ref(), event.get("usage"))
                                {
                                    usage_sent = true;
                                    let _ = tx.unbounded_send(StreamEvent::Usage(usage));
                                }
                            }
                            Some("message_stop") => {
                                if !usage_sent {
                                    if let Some(usage) = claude_usage(start_usage.as_ref(), None) {
                                        let _ = tx.unbounded_send(StreamEvent::Usage(usage));
                                    }
                                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::system_prompt::GIT_HEADING;
    use crate::context::SUMMARY_HEADING;
    use serde_json::json;

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.into(),
            description: format!("{name} things"),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: name.into(),
                arguments: "{}".into(),
            },
        }
    }

    fn breakpoints(body: &Value) -> Vec<String> {
        let mut found = Vec::new();
        for (i, t) in body["tools"].as_array().into_iter().flatten().enumerate() {
            if t.get("cache_control").is_some() {
                found.push(format!("tools[{i}]"));
            }
        }
        for (i, b) in body["system"].as_array().into_iter().flatten().enumerate() {
            if b.get("cache_control").is_some() {
                found.push(format!("system[{i}]"));
            }
        }
        for (i, m) in body["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            for (j, b) in m["content"].as_array().into_iter().flatten().enumerate() {
                if b.get("cache_control").is_some() {
                    found.push(format!("messages[{i}][{j}]"));
                }
            }
        }
        found
    }

    #[test]
    fn the_shared_prompt_the_sessions_own_part_and_the_tail_are_cached() {
        let base = "You are Phaze.\n\n## Project-Specific Instructions\nUse tabs.";
        let system =
            format!("{base}{GIT_HEADING}- Git branch: main\n\n{SUMMARY_HEADING}\nFixed the port.");
        let messages = [
            Message::system(system),
            Message::user("Add a test"),
            Message::assistant_with_tools("", vec![call("c1", "read_file"), call("c2", "grep")]),
            Message::tool_result("c1", "fn main() {}"),
            Message::tool_result("c2", "src/main.rs:1"),
        ];
        let client = ClaudeClient::new("k");
        let body = client.build_request_body(&messages, &[tool("grep"), tool("read_file")], true);

        // The base prompt is a block of its own, so its breakpoint is shared
        // by every session; the git state and summary follow it.
        assert_eq!(body["system"][0]["text"], base);
        assert_eq!(
            body["system"][1]["text"],
            format!("## Git Status\n- Git branch: main\n\n{SUMMARY_HEADING}\nFixed the port.")
        );
        // Both tool results go back in one user message.
        assert_eq!(body["messages"].as_array().unwrap().len(), 3);
        assert_eq!(body["messages"][2]["role"], "user");
        assert_eq!(body["messages"][2]["content"][1]["tool_use_id"], "c2");
        assert_eq!(
            breakpoints(&body),
            ["tools[1]", "system[0]", "system[1]", "messages[2][1]"]
        );
    }

    #[test]
    fn a_prompt_with_nothing_session_specific_has_one_system_block() {
        let messages = [Message::system("You are Phaze."), Message::user("hi")];
        let body = ClaudeClient::new("k").build_request_body(&messages, &[], false);
        assert_eq!(body["system"].as_array().unwrap().len(), 1);
        // A string message becomes a text block so it can carry the marker.
        assert_eq!(
            body["messages"][0]["content"],
            json!([{ "type": "text", "text": "hi", "cache_control": { "type": "ephemeral" } }])
        );
        assert_eq!(breakpoints(&body), ["system[0]", "messages[0][0]"]);
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn a_summary_without_a_system_prompt_is_the_only_block() {
        let summary = format!("{SUMMARY_HEADING}\nEarlier work.");
        let blocks = system_blocks(&summary);
        assert_eq!(blocks, vec![json!({ "type": "text", "text": summary })]);
    }

    #[test]
    fn the_tail_marker_skips_an_empty_reply() {
        let messages = [Message::user("hi"), Message::assistant("")];
        let body = ClaudeClient::new("k").build_request_body(&messages, &[], false);
        assert_eq!(breakpoints(&body), ["messages[0][0]"]);
        assert_eq!(body["messages"][1]["content"], "");
    }

    #[test]
    fn no_request_has_more_than_four_breakpoints() {
        let messages = [
            Message::system(format!("Base{GIT_HEADING}- Git branch: x")),
            Message::user("go"),
        ];
        let tools: Vec<ToolDefinition> = (0..30).map(|i| tool(&format!("t{i:02}"))).collect();
        let body = ClaudeClient::new("k").build_request_body(&messages, &tools, true);
        assert_eq!(breakpoints(&body).len(), MAX_BREAKPOINTS);
    }

    #[test]
    fn usage_counts_the_whole_prompt_with_its_cached_parts() {
        let start = json!({
            "input_tokens": 12,
            "cache_creation_input_tokens": 300,
            "cache_read_input_tokens": 4000,
            "output_tokens": 1,
        });
        let delta = json!({ "output_tokens": 57 });
        assert_eq!(
            claude_usage(Some(&start), Some(&delta)),
            Some(Usage {
                input_tokens: 4312,
                output_tokens: 57,
                cache_read_tokens: 4000,
                cache_write_tokens: 300,
            })
        );
        // A delta that repeats the input side (newer API versions) wins.
        let delta =
            json!({ "input_tokens": 12, "cache_read_input_tokens": 4100, "output_tokens": 9 });
        let usage = claude_usage(Some(&start), Some(&delta)).unwrap();
        assert_eq!((usage.input_tokens, usage.cache_read_tokens), (4412, 4100));
        assert_eq!(claude_usage(None, None), None);
    }

    /// Serve one SSE response on a local port and return its base URL.
    async fn serve_once(body: String) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                let n = sock.read(&mut chunk).await.unwrap();
                request.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&request).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let len: usize = text[..end]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().to_string())
                        })
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(response.as_bytes()).await.unwrap();
        });
        url
    }

    #[tokio::test]
    async fn a_stream_reports_one_usage_from_its_start_and_delta_events() {
        let events = [
            json!({ "type": "message_start", "message": { "usage": {
                "input_tokens": 20, "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 3700, "output_tokens": 1 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Done." } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 3 } }),
            json!({ "type": "message_stop" }),
        ];
        let body: String = events
            .iter()
            .map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap()))
            .collect();
        let url = serve_once(body).await;
        let client = ClaudeClient::new("k").with_base_url(url).without_proxy();
        let mut rx = client
            .chat_stream(&[Message::user("hi")], &[])
            .await
            .unwrap();
        use futures::StreamExt;
        let mut usages = Vec::new();
        let mut text = String::new();
        while let Some(event) = rx.next().await {
            match event {
                StreamEvent::Usage(u) => usages.push(u),
                StreamEvent::TextDelta(t) => text.push_str(&t),
                StreamEvent::Done => break,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(text, "Done.");
        assert_eq!(
            usages,
            [Usage {
                input_tokens: 3720,
                output_tokens: 3,
                cache_read_tokens: 3700,
                cache_write_tokens: 0,
            }]
        );
    }

    #[tokio::test]
    async fn dropping_the_stream_closes_the_provider_connection() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (closed_tx, closed) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 64 * 1024];
            let _ = sock.read(&mut buf).await;
            let first = json!({ "type": "content_block_delta", "index": 0,
                "delta": { "type": "text_delta", "text": "Hel" } });
            let first = format!("event: content_block_delta\ndata: {first}\n\n");
            let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n";
            sock.write_all(head.as_bytes()).await.unwrap();
            let part = format!("{:x}\r\n{first}\r\n", first.len());
            sock.write_all(part.as_bytes()).await.unwrap();
            // Still generating: send nothing more until the client hangs up.
            loop {
                match sock.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            let _ = closed_tx.send(());
        });
        let client = ClaudeClient::new("k").with_base_url(url).without_proxy();
        let mut rx = client
            .chat_stream(&[Message::user("hi")], &[])
            .await
            .unwrap();
        use futures::StreamExt;
        match rx.next().await {
            Some(StreamEvent::TextDelta(t)) => assert_eq!(t, "Hel"),
            other => panic!("expected the first text, got {other:?}"),
        }
        // What the agent does on Stop.
        drop(rx);
        tokio::time::timeout(std::time::Duration::from_secs(3), closed)
            .await
            .expect("the request was still open 3 s after Stop")
            .unwrap();
    }
}
