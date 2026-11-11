//! Inline completions ask a provider's fill-in-the-middle API where it has
//! one: the code before and after the cursor go as they are, with no chat
//! prompt around them, and the model writes what belongs between.
//!
//! The model is a scripted server on 127.0.0.1 that speaks Ollama's
//! `/api/generate`, DeepSeek's `/beta/completions`, Mistral's
//! `/v1/fim/completions` and the OpenAI chat API, and records every request.
//! Everything else is production code: `fim_complete_for` with `Settings`,
//! the privacy guard and the clients. The tests change process-wide state
//! (API key variables, the active privacy project), so they take turns.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use phazeai_core::config::LlmProvider;
use phazeai_core::fim::fim_complete_for;
use phazeai_core::{privacy, Settings};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

static TURN: Mutex<()> = Mutex::new(());

/// Held for a whole test: the state these tests change is process-wide.
struct Turn(#[allow(dead_code)] MutexGuard<'static, ()>);

fn turn() -> Turn {
    Turn(TURN.lock().unwrap_or_else(|e| e.into_inner()))
}

/// One request the scripted model received.
#[derive(Debug, Clone)]
struct Seen {
    /// The request line, e.g. `POST /api/generate HTTP/1.1`.
    line: String,
    body: Value,
    /// The `authorization` header, if one came.
    auth: Option<String>,
}

type Log = Arc<Mutex<Vec<Seen>>>;

/// The text every API answers with: a completion that ends with the `)`
/// already after the cursor.
const COMPLETION: &str = "a, b)";

/// A model server on 127.0.0.1. `fim` is false for a model with no
/// fill-in-the-middle: its FIM routes answer as such a server does.
async fn model_server(fim: bool) -> (String, Log) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let log = Log::default();
    let seen = log.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let seen = seen.clone();
            tokio::spawn(async move {
                let Some(request) = read_request(&mut sock).await else {
                    return;
                };
                let response = answer(&request, fim);
                seen.lock().unwrap().push(request);
                let _ = sock.write_all(response.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (address, log)
}

async fn read_request(sock: &mut TcpStream) -> Option<Seen> {
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
        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
        let header = |name: &str| {
            head.lines().find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.eq_ignore_ascii_case(name).then(|| v.trim().to_string())
            })
        };
        let len: usize = header("content-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let body_start = head_end + 4;
        if buf.len() < body_start + len {
            continue;
        }
        return Some(Seen {
            line: head.lines().next().unwrap_or_default().to_string(),
            body: serde_json::from_slice(&buf[body_start..body_start + len]).unwrap_or(Value::Null),
            auth: header("authorization"),
        });
    }
}

fn answer(request: &Seen, fim: bool) -> String {
    let path = request.line.split_whitespace().nth(1).unwrap_or_default();
    let reply = |status: &str, body: Value| {
        let body = body.to_string();
        format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
             connection: close\r\n\r\n{body}",
            body.len()
        )
    };
    match path {
        "/api/generate" if fim => reply("200 OK", json!({"response": COMPLETION, "done": true})),
        // What Ollama answers for a model whose template has no suffix.
        "/api/generate" => reply(
            "400 Bad Request",
            json!({"error": "registry.ollama.ai/library/llama3:latest does not support insert"}),
        ),
        "/beta/completions" if fim => reply(
            "200 OK",
            json!({"choices": [{"text": COMPLETION, "index": 0, "finish_reason": "stop"}]}),
        ),
        "/v1/fim/completions" if fim => reply(
            "200 OK",
            json!({"choices": [{"index": 0, "message": {"role": "assistant", "content": COMPLETION}}]}),
        ),
        p if p.ends_with("/chat/completions") => reply(
            "200 OK",
            json!({"choices": [{"message": {"role": "assistant", "content": "a, b"}}]}),
        ),
        "/api/chat" => reply(
            "200 OK",
            json!({
                "model": request.body["model"],
                "created_at": "2026-01-01T00:00:00Z",
                "message": {"role": "assistant", "content": "a, b"},
                "done": true,
            }),
        ),
        _ => "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".into(),
    }
}

fn requests(log: &Log) -> Vec<Seen> {
    log.lock().unwrap().clone()
}

fn settings(provider: LlmProvider, model: &str, base_url: &str) -> Settings {
    let mut s = Settings::default();
    s.llm.provider = provider;
    s.llm.model = model.to_string();
    s.llm.base_url = Some(base_url.to_string());
    s
}

const PREFIX: &str = "fn add(a: i32, b: i32) -> i32 { a + b }\n\nfn main() {\n    add(";
const SUFFIX: &str = ");\n}\n";

async fn complete(s: &Settings) -> Result<String, String> {
    fim_complete_for(
        s,
        &PathBuf::from("/project/src/main.rs"),
        PREFIX,
        SUFFIX,
        "rust",
    )
    .await
}

/// The request carries the code as it is: no `<CURSOR>` marker, no
/// instructions.
fn assert_plain_code(seen: &Seen, prompt: &str, suffix: &str) {
    assert_eq!(seen.body[prompt], PREFIX);
    assert_eq!(seen.body[suffix], SUFFIX);
    let text = seen.body.to_string();
    assert!(
        !text.contains("CURSOR") && !text.contains("completion engine"),
        "{text}"
    );
}

#[tokio::test]
async fn ollama_fills_in_the_middle_with_the_small_model() {
    let _turn = turn();
    let (server, log) = model_server(true).await;
    let mut s = settings(LlmProvider::Ollama, "qwen2.5-coder:32b", &server);
    s.llm.small_model = Some("qwen2.5-coder:1.5b".into());

    // The `)` the model wrote is already after the cursor, so it isn't suggested.
    assert_eq!(complete(&s).await, Ok("a, b".to_string()));

    let seen = requests(&log);
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].line, "POST /api/generate HTTP/1.1");
    assert_eq!(seen[0].body["model"], "qwen2.5-coder:1.5b");
    assert_eq!(seen[0].body["stream"], false);
    assert_eq!(seen[0].body["options"]["stop"], json!(["\n\n"]));
    assert!(seen[0].body["options"]["num_predict"].as_u64().unwrap() <= 96);
    assert_plain_code(&seen[0], "prompt", "suffix");
    assert_eq!(seen[0].auth, None, "a local server gets no key");
}

#[tokio::test]
async fn an_ollama_model_without_insert_is_asked_by_chat_instead() {
    let _turn = turn();
    let (server, log) = model_server(false).await;
    let s = settings(LlmProvider::Ollama, "llama3:latest", &server);

    assert_eq!(complete(&s).await, Ok("a, b".to_string()));
    assert_eq!(complete(&s).await, Ok("a, b".to_string()));

    let lines: Vec<String> = requests(&log).into_iter().map(|r| r.line).collect();
    assert_eq!(
        lines,
        [
            "POST /api/generate HTTP/1.1",
            "POST /api/chat HTTP/1.1",
            // Remembered: the second completion goes straight to chat.
            "POST /api/chat HTTP/1.1",
        ]
    );
}

#[tokio::test]
async fn deepseek_fills_in_the_middle_on_flash_through_its_beta_api() {
    let _turn = turn();
    std::env::set_var("DEEPSEEK_API_KEY", "test-key");
    let (server, log) = model_server(true).await;
    // Even when the agent runs on Pro, completions go to the cheaper Flash.
    let s = settings(
        LlmProvider::DeepSeek,
        "deepseek-v4-pro",
        &format!("{server}/v1"),
    );

    assert_eq!(complete(&s).await, Ok("a, b".to_string()));

    let seen = requests(&log);
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].line, "POST /beta/completions HTTP/1.1");
    assert_eq!(seen[0].body["model"], "deepseek-flash");
    assert_eq!(seen[0].auth.as_deref(), Some("Bearer test-key"));
    assert_eq!(seen[0].body["max_tokens"], 96);
    assert_plain_code(&seen[0], "prompt", "suffix");
}

#[tokio::test]
async fn mistral_completes_with_codestral_unless_told_otherwise() {
    let _turn = turn();
    std::env::set_var("MISTRAL_API_KEY", "test-key");
    let (server, log) = model_server(true).await;
    let mut s = settings(
        LlmProvider::Mistral,
        "mistral-large-latest",
        &format!("{server}/v1"),
    );
    s.llm.small_model = Some("mistral-small-latest".into());

    assert_eq!(complete(&s).await, Ok("a, b".to_string()));
    s.editor.completion_model = Some("codestral-2508".into());
    assert_eq!(complete(&s).await, Ok("a, b".to_string()));

    let seen = requests(&log);
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].line, "POST /v1/fim/completions HTTP/1.1");
    // A chat model has no fill-in-the-middle, so not the small model.
    assert_eq!(seen[0].body["model"], "codestral-latest");
    assert_eq!(seen[1].body["model"], "codestral-2508");
    assert_eq!(seen[0].auth.as_deref(), Some("Bearer test-key"));
    assert_plain_code(&seen[0], "prompt", "suffix");
}

#[tokio::test]
async fn a_provider_without_fill_in_the_middle_keeps_the_chat_prompt() {
    let _turn = turn();
    let (server, log) = model_server(true).await;
    let s = settings(LlmProvider::LmStudio, "qwen2.5-coder-7b", &server);

    assert_eq!(complete(&s).await, Ok("a, b".to_string()));

    let seen = requests(&log);
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].line, "POST /v1/chat/completions HTTP/1.1");
    assert!(seen[0].body.to_string().contains("<CURSOR>"));
}

/// A fresh directory made the active privacy project; forgotten on drop.
struct Project {
    _dir: tempfile::TempDir,
}

impl Project {
    fn local_only() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        privacy::set_active_project(&root);
        privacy::write_local_only(&root, true).unwrap();
        assert!(privacy::active().local_only(), "policy not applied");
        Self { _dir: dir }
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        privacy::clear_active_project();
    }
}

#[tokio::test]
async fn local_only_refuses_a_remote_completion_and_allows_a_local_one() {
    let _turn = turn();
    let _project = Project::local_only();
    std::env::set_var("DEEPSEEK_API_KEY", "test-key");
    // Nothing listens here: a refused request never gets as far as connecting.
    let remote = settings(
        LlmProvider::DeepSeek,
        "deepseek-flash",
        "https://fim.invalid/v1",
    );
    let refused = complete(&remote).await.unwrap_err();
    assert!(refused.to_lowercase().contains("local"), "{refused}");

    let (server, log) = model_server(true).await;
    let local = settings(LlmProvider::Ollama, "qwen2.5-coder:1.5b", &server);
    assert_eq!(complete(&local).await, Ok("a, b".to_string()));
    assert_eq!(requests(&log).len(), 1);
}
