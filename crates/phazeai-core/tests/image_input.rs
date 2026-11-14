//! A picture sent with a question reaches the model in each provider's own
//! format, and stays in the conversation for the turns after it.
//!
//! The model is a scripted server on 127.0.0.1 that speaks the Ollama,
//! OpenAI and Anthropic chat APIs and records what it is sent. Everything
//! else is the production code: `Settings::build_llm_client`, the clients and
//! the agent loop.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::sync::{Arc, Mutex, MutexGuard};

use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use phazeai_core::config::LlmProvider;
use phazeai_core::llm::ImageInput;
use phazeai_core::{
    Agent, LlmClient, LlmResponse, Message, PhazeError, Settings, StreamEvent, ToolDefinition,
};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// A real 1x1 PNG.
const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

const QUESTION: &str = "why is the button cut off?";
const REPLY: &str = "the container has a fixed height";

static TURN: Mutex<()> = Mutex::new(());

/// Held for a whole test: the tests set API key variables, which are
/// process-wide.
struct Turn(#[allow(dead_code)] MutexGuard<'static, ()>);

fn turn() -> Turn {
    Turn(TURN.lock().unwrap_or_else(|e| e.into_inner()))
}

fn picture() -> ImageInput {
    ImageInput::from_bytes(PNG_1X1).expect("a real PNG")
}

type Log = Arc<Mutex<Vec<(String, Value)>>>;

/// A model server for the Ollama (`/api/chat`), OpenAI (`…/chat/completions`)
/// and Anthropic (`…/messages`) APIs, answering [`REPLY`] and recording each
/// request line and body.
async fn model_server() -> (String, Log) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let log = Log::default();
    let seen = log.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let seen = seen.clone();
            tokio::spawn(async move {
                let Some((line, body)) = read_request(&mut sock).await else {
                    return;
                };
                let response = answer(&line, &body);
                seen.lock().unwrap().push((line, body));
                let _ = sock.write_all(response.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (address, log)
}

async fn read_request(sock: &mut TcpStream) -> Option<(String, Value)> {
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
        let body_start = head_end + 4;
        if buf.len() < body_start + len {
            continue;
        }
        let line = head.lines().next().unwrap_or_default().to_string();
        let body = serde_json::from_slice(&buf[body_start..body_start + len]).ok()?;
        return Some((line, body));
    }
}

fn answer(line: &str, body: &Value) -> String {
    let path = line.split_whitespace().nth(1).unwrap_or_default();
    let reply = if path == "/api/chat" {
        json!({
            "model": body["model"],
            "created_at": "2026-01-01T00:00:00Z",
            "message": {"role": "assistant", "content": REPLY},
            "done": true,
        })
    } else if path.ends_with("/chat/completions") {
        json!({"choices": [{"message": {"role": "assistant", "content": REPLY}}]})
    } else if path.ends_with("/messages") {
        json!({
            "content": [{"type": "text", "text": REPLY}],
            "usage": {"input_tokens": 1, "output_tokens": 1},
        })
    } else {
        return "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".into();
    };
    let reply = reply.to_string();
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{reply}",
        reply.len()
    )
}

/// Sets environment variables for its lifetime; the old values come back on
/// drop.
struct EnvVars(Vec<(&'static str, Option<OsString>)>);

impl EnvVars {
    fn set(vars: &[(&'static str, &str)]) -> Self {
        let saved = vars
            .iter()
            .map(|&(name, value)| {
                let old = std::env::var_os(name);
                std::env::set_var(name, value);
                (name, old)
            })
            .collect();
        Self(saved)
    }
}

impl Drop for EnvVars {
    fn drop(&mut self) {
        for (name, old) in self.0.drain(..).rev() {
            match old {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

/// Ask `provider` at the scripted server [`QUESTION`] with the picture, and
/// return the one request body it sent.
async fn ask_with_picture(provider: LlmProvider, model: &str) -> Value {
    let (server, log) = model_server().await;
    let mut s = Settings::default();
    s.llm.provider = provider;
    s.llm.model = model.to_string();
    s.llm.base_url = Some(server);
    let client = s.build_llm_client().expect("client builds");
    let reply = client
        .chat(&[Message::user_with_images(QUESTION, vec![picture()])], &[])
        .await
        .expect("the scripted model answers");
    assert_eq!(reply.message.content, REPLY);
    let seen = log.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "{seen:?}");
    seen[0].1.clone()
}

/// The last message of a request body.
fn last_message(body: &Value) -> &Value {
    body["messages"]
        .as_array()
        .and_then(|m| m.last())
        .expect("the request has messages")
}

#[tokio::test]
async fn ollama_gets_the_picture_in_the_message_images() {
    let _turn = turn();
    let body = ask_with_picture(LlmProvider::Ollama, "llava:7b").await;
    let message = last_message(&body);
    assert_eq!(message["role"], "user");
    assert_eq!(message["content"], QUESTION);
    assert_eq!(message["images"], json!([picture().data]));
}

#[tokio::test]
async fn openai_compatible_servers_get_a_data_url_part() {
    let _turn = turn();
    let _key = EnvVars::set(&[("OPENAI_API_KEY", "phazeai-test-key")]);
    let body = ask_with_picture(LlmProvider::OpenAI, "gpt-4o").await;
    let message = last_message(&body);
    assert_eq!(message["role"], "user");
    assert_eq!(
        message["content"],
        json!([
            {"type": "text", "text": QUESTION},
            {"type": "image_url", "image_url": {"url": picture().data_url()}},
        ])
    );
}

#[tokio::test]
async fn claude_gets_a_base64_image_block_before_the_question() {
    let _turn = turn();
    let _key = EnvVars::set(&[("ANTHROPIC_API_KEY", "phazeai-test-key")]);
    let body = ask_with_picture(LlmProvider::Claude, "claude-sonnet-4-5").await;
    let message = last_message(&body);
    assert_eq!(message["role"], "user");
    let content = message["content"].as_array().expect("content blocks");
    assert_eq!(content.len(), 2, "{content:?}");
    assert_eq!(content[0]["type"], "image");
    assert_eq!(
        content[0]["source"],
        json!({"type": "base64", "media_type": "image/png", "data": picture().data})
    );
    assert_eq!(content[1]["type"], "text");
    assert_eq!(content[1]["text"], QUESTION);
    // The conversation tail is still marked for the prompt cache, on the
    // last block, so a follow-up question reads the picture from the cache.
    assert!(content[1].get("cache_control").is_some(), "{content:?}");
}

#[tokio::test]
async fn a_message_without_pictures_is_sent_as_before() {
    let _turn = turn();
    let _key = EnvVars::set(&[("OPENAI_API_KEY", "phazeai-test-key")]);
    let (server, log) = model_server().await;
    let mut s = Settings::default();
    s.llm.provider = LlmProvider::OpenAI;
    s.llm.model = "gpt-4o".into();
    s.llm.base_url = Some(server);
    let client = s.build_llm_client().unwrap();
    client.chat(&[Message::user(QUESTION)], &[]).await.unwrap();
    let body = log.lock().unwrap()[0].1.clone();
    assert_eq!(last_message(&body)["content"], QUESTION);
}

/// A model that records the messages of every call and answers in text.
struct Recorder {
    calls: Arc<Mutex<Vec<Vec<Message>>>>,
    replies: Mutex<VecDeque<&'static str>>,
}

#[async_trait::async_trait]
impl LlmClient for Recorder {
    async fn chat(&self, _: &[Message], _: &[ToolDefinition]) -> Result<LlmResponse, PhazeError> {
        Ok(LlmResponse {
            message: Message::assistant("summary"),
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> Result<UnboundedReceiver<StreamEvent>, PhazeError> {
        self.calls.lock().unwrap().push(messages.to_vec());
        let reply = self.replies.lock().unwrap().pop_front().unwrap_or("ok");
        let (tx, rx) = unbounded();
        tx.unbounded_send(StreamEvent::TextDelta(reply.into()))
            .unwrap();
        tx.unbounded_send(StreamEvent::Done).unwrap();
        Ok(rx)
    }
}

#[tokio::test]
async fn the_agent_sends_the_picture_and_keeps_it_for_the_next_turn() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let agent = Agent::new(Box::new(Recorder {
        calls: calls.clone(),
        replies: Mutex::new(VecDeque::from([REPLY, "make it auto"])),
    }));

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let first = agent
        .run_with_images_and_events(QUESTION, vec![picture()], tx)
        .await
        .expect("the run finishes");
    assert_eq!(first.content, REPLY);

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    agent
        .run_with_events("how do I fix it?", tx)
        .await
        .expect("the follow-up finishes");

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    let asked = |call: &[Message], text: &str| -> Vec<ImageInput> {
        call.iter()
            .find(|m| m.content == text)
            .unwrap_or_else(|| panic!("{text:?} was not sent"))
            .images
            .clone()
    };
    assert_eq!(asked(&calls[0], QUESTION), vec![picture()]);
    // The follow-up still carries the picture in the history, and the new
    // question has none of its own.
    assert_eq!(asked(&calls[1], QUESTION), vec![picture()]);
    assert!(asked(&calls[1], "how do I fix it?").is_empty());
}
