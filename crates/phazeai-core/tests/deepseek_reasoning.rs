//! DeepSeek's models think by default, and once a request carries tools its
//! API refuses (HTTP 400) any request whose earlier assistant turns leave out
//! the `reasoning_content` it sent. So an agent run on DeepSeek has to keep
//! each turn's reasoning and send it back on every later request.
//!
//! The model is a scripted DeepSeek on 127.0.0.1 that streams reasoning,
//! answers each request from a script and records what it was sent.
//! Everything else is production code: `Settings::build_llm_client` with
//! `provider = "deepseek"`, the privacy guard, the OpenAI-compatible client
//! and the agent loop.

use std::sync::{Arc, Mutex};

use phazeai_core::config::LlmProvider;
use phazeai_core::{Agent, Settings, Tool, ToolRegistry, ToolResult};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

type Log = Arc<Mutex<Vec<Value>>>;

struct EchoTool;

#[async_trait::async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echoes input"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        Ok(json!({"echoed": params["text"]}))
    }
}

/// The streamed reply to the `n`th request (from 0): reasoning, then a call
/// to `echo` for the first request and text for every other.
fn reply(n: usize) -> String {
    let chunk = |delta: Value| format!("data: {}\n\n", json!({"choices": [{"delta": delta}]}));
    let mut body = chunk(json!({"reasoning_content": format!("thought {n}, ")}));
    body += &chunk(json!({"reasoning_content": "part two"}));
    if n == 0 {
        body += &chunk(json!({"tool_calls": [{
            "index": 0, "id": "call_1",
            "function": {"name": "echo", "arguments": "{\"text\":\"hi\"}"}
        }]}));
    } else {
        body += &chunk(json!({"content": format!("answer {n}")}));
    }
    body + "data: [DONE]\n\n"
}

/// A DeepSeek stand-in. Like the real one, it refuses a request with tools
/// whose assistant turns are missing their `reasoning_content`.
async fn deepseek() -> (String, Log) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let log = Log::default();
    let seen = log.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let Some(body) = read_body(&mut sock).await else {
                continue;
            };
            let n = {
                let mut seen = seen.lock().unwrap();
                seen.push(body.clone());
                seen.len() - 1
            };
            let unthought = body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["role"] == "assistant" && m["reasoning_content"].is_null());
            let response = if body["tools"].is_array() && unthought {
                let error = r#"{"error":{"message":"The reasoning_content in the thinking mode must be passed back to the API."}}"#;
                format!(
                    "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{error}",
                    error.len()
                )
            } else {
                let body = reply(n);
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
            };
            let _ = sock.write_all(response.as_bytes()).await;
            let _ = sock.shutdown().await;
        }
    });
    (address, log)
}

async fn read_body(sock: &mut TcpStream) -> Option<Value> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        let Some(head_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
        let len: usize = head
            .lines()
            .find_map(|l| l.strip_prefix("content-length:"))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        if buf.len() < head_end + 4 + len {
            continue;
        }
        return serde_json::from_slice(&buf[head_end + 4..head_end + 4 + len]).ok();
    }
}

/// The `reasoning_content` of each assistant message a request carried.
fn sent_reasoning(request: &Value) -> Vec<String> {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "assistant")
        .map(|m| {
            m["reasoning_content"]
                .as_str()
                .unwrap_or("<missing>")
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn an_agent_on_deepseek_sends_every_turns_reasoning_back() {
    let (server, log) = deepseek().await;
    std::env::set_var("DEEPSEEK_API_KEY", "test-key");
    let mut settings = Settings::default();
    settings.set_provider(LlmProvider::DeepSeek);
    settings.llm.model = "deepseek-flash".into();
    settings.llm.base_url = Some(format!("{server}/v1"));

    let mut tools = ToolRegistry::new();
    tools.register(Box::new(EchoTool));
    let agent = Agent::new(settings.build_llm_client().unwrap()).with_tools(tools);

    // A tool round, then the answer; then a second question.
    let first = agent.run("echo hi").await.expect("first question");
    assert_eq!(first.content, "answer 1");
    let second = agent.run("and again").await.expect("second question");
    assert_eq!(second.content, "answer 2");

    let requests = log.lock().unwrap().clone();
    assert_eq!(requests.len(), 3);
    assert!(sent_reasoning(&requests[0]).is_empty());
    // The call's turn, with the reasoning streamed before it.
    assert_eq!(sent_reasoning(&requests[1]), ["thought 0, part two"]);
    // Both turns of the first question, the final answer included.
    assert_eq!(
        sent_reasoning(&requests[2]),
        ["thought 0, part two", "thought 1, part two"]
    );
    // The reasoning goes back to the model and nowhere else.
    assert!(!first.content.contains("thought"));
}
