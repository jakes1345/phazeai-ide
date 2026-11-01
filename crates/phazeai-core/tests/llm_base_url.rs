//! `[llm] base_url` decides where every client built from `Settings` sends
//! its requests, and `local_only` judges that same address.
//!
//! The setting moves the active provider off its default address: an Ollama
//! or LM Studio server on another machine or port, or an OpenAI-compatible
//! proxy. It used to reach only the IDE's readiness probe. The clients were
//! built for the provider's default URL, so the probe asked the configured
//! server while the chat went to localhost:11434, and a cloud provider
//! pointed at a local server or a proxy sent the code to the provider's own
//! API instead.
//!
//! The model is a scripted server on 127.0.0.1 that speaks the OpenAI, Ollama
//! and Anthropic chat APIs and records every request. Everything else is the
//! production code: `Settings::build_llm_client`, the clients, the privacy
//! guard, the model router and FIM. The tests change process-wide state (the
//! active privacy project, API key variables, the config directory), which is
//! why they are a test binary of their own and take turns ([`turn`]).

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use futures::StreamExt;
use phazeai_core::config::{LlmProvider, ProviderEntry};
use phazeai_core::llm::{Message, ModelRoute, StreamEvent, TaskType};
use phazeai_core::{privacy, Settings};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// What the scripted model answers to every request.
const REPLY: &str = "scripted reply";

static TURN: Mutex<()> = Mutex::new(());

/// Held for a whole test: the state these tests change is process-wide.
struct Turn(#[allow(dead_code)] MutexGuard<'static, ()>);

fn turn() -> Turn {
    Turn(TURN.lock().unwrap_or_else(|e| e.into_inner()))
}

/// One request the scripted model received.
#[derive(Debug, Clone)]
struct Seen {
    /// The request line, e.g. `POST /v1/chat/completions HTTP/1.1`.
    line: String,
    body: Value,
    /// Whether an `authorization` or `x-api-key` header came with it. The
    /// values are not kept, so a failing assertion never prints a key.
    sent_key: bool,
}

type Log = Arc<Mutex<Vec<Seen>>>;

/// A model server on 127.0.0.1 for the OpenAI (`…/chat/completions`, plain
/// and streamed), Ollama (`/api/chat`) and Anthropic (`…/messages`) chat
/// APIs. Returns its `http://127.0.0.1:<port>` address and the requests it
/// received.
async fn model_server() -> (String, Log) {
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
                let response = answer(&request);
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
        let lower = head.to_ascii_lowercase();
        let len: usize = lower
            .lines()
            .find_map(|l| l.strip_prefix("content-length:"))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        let body_start = head_end + 4;
        if buf.len() < body_start + len {
            continue;
        }
        return Some(Seen {
            line: head.lines().next().unwrap_or_default().to_string(),
            body: serde_json::from_slice(&buf[body_start..body_start + len]).unwrap_or(Value::Null),
            sent_key: lower
                .lines()
                .any(|l| l.starts_with("authorization:") || l.starts_with("x-api-key:")),
        });
    }
}

fn answer(request: &Seen) -> String {
    let path = request.line.split_whitespace().nth(1).unwrap_or_default();
    let streamed = request.body["stream"].as_bool().unwrap_or(false);
    let (content_type, body) = if path.ends_with("/chat/completions") && streamed {
        let delta = json!({"choices": [{"delta": {"content": REPLY}}]});
        (
            "text/event-stream",
            format!("data: {delta}\n\ndata: [DONE]\n\n"),
        )
    } else if path.ends_with("/chat/completions") {
        let message = json!({"choices": [{"message": {"role": "assistant", "content": REPLY}}]});
        ("application/json", message.to_string())
    } else if path == "/api/chat" {
        let message = json!({
            "model": request.body["model"],
            "created_at": "2026-01-01T00:00:00Z",
            "message": {"role": "assistant", "content": REPLY},
            "done": true,
        });
        ("application/json", message.to_string())
    } else if path.ends_with("/messages") {
        let message = json!({
            "content": [{"type": "text", "text": REPLY}],
            "usage": {"input_tokens": 1, "output_tokens": 1},
        });
        ("application/json", message.to_string())
    } else {
        return "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".into();
    };
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{body}",
        body.len()
    )
}

fn requests(log: &Log) -> Vec<Seen> {
    log.lock().unwrap().clone()
}

fn lines(seen: &[Seen]) -> Vec<&str> {
    seen.iter().map(|r| r.line.as_str()).collect()
}

fn settings(provider: LlmProvider, model: &str, base_url: Option<&str>) -> Settings {
    let mut s = Settings::default();
    s.llm.provider = provider;
    s.llm.model = model.to_string();
    s.llm.base_url = base_url.map(str::to_string);
    s
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

/// A fresh directory made the active privacy project; forgotten on drop.
struct Project {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Project {
    fn new(local_only: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        privacy::set_active_project(&root);
        let project = Self { _dir: dir, root };
        project.set_local_only(local_only);
        project
    }

    fn set_local_only(&self, on: bool) {
        privacy::write_local_only(&self.root, on).unwrap();
        assert_eq!(privacy::active().local_only(), on, "policy not applied");
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        privacy::clear_active_project();
    }
}

#[tokio::test]
async fn ollama_chat_goes_to_the_configured_server() {
    let _turn = turn();
    let (server, log) = model_server().await;
    let s = settings(LlmProvider::Ollama, "llama3.2:3b", Some(&server));

    let client = s.build_llm_client().expect("client builds");
    let reply = client
        .chat(&[Message::user("what does main do?")], &[])
        .await
        .expect("the configured Ollama answers");

    assert_eq!(reply.message.content, REPLY);
    let seen = requests(&log);
    assert_eq!(lines(&seen), ["POST /api/chat HTTP/1.1"]);
    assert_eq!(seen[0].body["model"], "llama3.2:3b");
    assert!(
        seen[0].body.to_string().contains("what does main do?"),
        "{:?}",
        seen[0].body
    );
}

#[tokio::test]
async fn openai_compatible_chat_and_stream_go_to_the_configured_server() {
    let _turn = turn();
    let (server, log) = model_server().await;
    // Padded with spaces, with a version segment and a trailing slash: the
    // requests go to <server>/v1/chat/completions (not …/v1/v1/…), the way
    // the probe and the OpenAI client already read base URLs.
    let s = settings(
        LlmProvider::LmStudio,
        "qwen2.5-coder-7b",
        Some(&format!("  {server}/v1/  ")),
    );

    let client = s.build_llm_client().expect("client builds");
    let reply = client
        .chat(&[Message::user("hello")], &[])
        .await
        .expect("the configured server answers");
    assert_eq!(reply.message.content, REPLY);
    // The IDE chat and the agent stream.
    let mut events = client
        .chat_stream(&[Message::user("hello again")], &[])
        .await
        .expect("the stream starts");
    let mut streamed = String::new();
    while let Some(event) = events.next().await {
        match event {
            StreamEvent::TextDelta(text) => streamed.push_str(&text),
            StreamEvent::Error(e) => panic!("{e}"),
            _ => {}
        }
    }
    assert_eq!(streamed, REPLY);

    let seen = requests(&log);
    assert_eq!(lines(&seen), ["POST /v1/chat/completions HTTP/1.1"; 2]);
    assert!(seen.iter().all(|r| r.body["model"] == "qwen2.5-coder-7b"));
}

#[tokio::test]
async fn a_cloud_provider_pointed_at_a_local_server_only_talks_to_it() {
    let _turn = turn();
    // local_only is the safety net here: a client built for the providers'
    // own APIs is refused before anything is sent.
    let _project = Project::new(true);
    let _keys = EnvVars::set(&[
        ("OPENAI_API_KEY", "phazeai-test-key"),
        ("ANTHROPIC_API_KEY", "phazeai-test-key"),
    ]);
    let (server, log) = model_server().await;

    let openai = settings(LlmProvider::OpenAI, "gpt-4o", Some(&server))
        .build_llm_client()
        .expect("an address on this machine is allowed under local_only");
    let reply = openai.chat(&[Message::user("hi")], &[]).await.unwrap();
    assert_eq!(reply.message.content, REPLY);

    // A trailing slash is tolerated here too.
    let claude = settings(
        LlmProvider::Claude,
        "claude-sonnet-4-5",
        Some(&format!("{server}/")),
    )
    .build_llm_client()
    .expect("an address on this machine is allowed under local_only");
    let reply = claude.chat(&[Message::user("hi")], &[]).await.unwrap();
    assert_eq!(reply.message.content, REPLY);

    let seen = requests(&log);
    assert_eq!(
        lines(&seen),
        [
            "POST /v1/chat/completions HTTP/1.1",
            "POST /v1/messages HTTP/1.1"
        ]
    );
    assert!(
        seen.iter().all(|r| r.sent_key),
        "each provider's key goes with its request"
    );
}

#[tokio::test]
async fn local_only_refuses_a_server_on_another_machine() {
    let _turn = turn();
    let _project = Project::new(true);
    for (provider, url) in [
        (LlmProvider::Ollama, "http://gpu-box.invalid:11434"),
        (LlmProvider::LmStudio, "http://lan-box.invalid:1234/v1"),
    ] {
        let err = match settings(provider, "m", Some(url)).build_llm_client() {
            Ok(_) => panic!("{url} is not on this machine: the client must be refused"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("local-only"), "{err}");
        assert!(
            err.contains(".invalid"),
            "the refusal names the host: {err}"
        );
    }
}

#[tokio::test]
async fn a_client_for_another_machine_is_refused_once_local_only_is_on() {
    let _turn = turn();
    let project = Project::new(false);
    let client = settings(
        LlmProvider::Ollama,
        "m",
        Some("http://gpu-box.invalid:11434"),
    )
    .build_llm_client()
    .expect("allowed while local_only is off");

    project.set_local_only(true);
    let err = match client.chat(&[Message::user("hi")], &[]).await {
        Ok(_) => panic!("a remote server must be refused under local_only"),
        Err(e) => e.to_string(),
    };
    assert!(err.contains("local-only"), "refused before sending: {err}");
}

#[tokio::test]
async fn a_routed_provider_entry_keeps_its_own_address() {
    let _turn = turn();
    let _key = EnvVars::set(&[("PHAZEAI_TEST_PROXY_KEY", "phazeai-test-key")]);
    let (main, main_log) = model_server().await;
    let (proxy, proxy_log) = model_server().await;
    let mut s = settings(LlmProvider::LmStudio, "main-model", Some(&main));
    s.providers.push(ProviderEntry {
        name: "myproxy".into(),
        enabled: true,
        api_key_env: "PHAZEAI_TEST_PROXY_KEY".into(),
        base_url: proxy,
        default_model: "proxy-model".into(),
    });
    s.model_routes.insert(
        TaskType::QuickAnswer,
        ModelRoute {
            provider: "myproxy".into(),
            model: "quick-model".into(),
        },
    );
    let client = s.build_llm_client().expect("client builds");

    // A short question is a quick answer: the [[providers]] entry's server.
    client.chat(&[Message::user("hi")], &[]).await.unwrap();
    // Anything else: the active provider at [llm] base_url.
    client
        .chat(&[Message::user("explain why the build is slow")], &[])
        .await
        .unwrap();

    let routed = requests(&proxy_log);
    assert_eq!(lines(&routed), ["POST /v1/chat/completions HTTP/1.1"]);
    assert_eq!(routed[0].body["model"], "quick-model");
    let default = requests(&main_log);
    assert_eq!(lines(&default), ["POST /v1/chat/completions HTTP/1.1"]);
    assert_eq!(default[0].body["model"], "main-model");
}

/// Background completions (`fim_complete`) read `config.toml` and build their
/// own client, so they are checked through the file, as the editor calls them.
///
/// Unix only: the config directory is moved to a temp dir through
/// `XDG_CONFIG_HOME` and `HOME`, which does not move it on Windows.
#[cfg(unix)]
#[tokio::test]
async fn fim_completions_go_to_the_configured_server() {
    let _turn = turn();
    let home = tempfile::tempdir().unwrap();
    let xdg = home.path().join("xdg");
    let _env = EnvVars::set(&[
        ("HOME", home.path().to_str().unwrap()),
        ("XDG_CONFIG_HOME", xdg.to_str().unwrap()),
    ]);
    let config = Settings::config_path();
    assert!(
        config.starts_with(home.path()),
        "never touch the real config.toml: {}",
        config.display()
    );
    let (server, log) = model_server().await;
    settings(LlmProvider::LmStudio, "qwen2.5-coder-7b", Some(&server))
        .save()
        .expect("config.toml written");

    let source = home.path().join("project/src/lib.rs");
    let completion = phazeai_core::fim::fim_complete(
        &source,
        "fn add(a: i32, b: i32) -> i32 {\n    ",
        "\n}\n",
        "rust",
    )
    .await
    .expect("the configured server completes");

    assert_eq!(completion, REPLY);
    let seen = requests(&log);
    assert_eq!(lines(&seen), ["POST /v1/chat/completions HTTP/1.1"]);
    assert_eq!(seen[0].body["model"], "qwen2.5-coder-7b");
    assert!(
        seen[0].body.to_string().contains("<CURSOR>"),
        "the completion prompt: {:?}",
        seen[0].body
    );
}

/// A proxy on 127.0.0.1 that counts the connections it gets and refuses them.
async fn counting_proxy() -> (String, Arc<Mutex<usize>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    let count = Arc::new(Mutex::new(0usize));
    let seen = count.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            *seen.lock().unwrap() += 1;
            let _ = sock
                .write_all(
                    b"HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await;
        }
    });
    (addr, count)
}

#[tokio::test]
async fn a_server_on_this_machine_is_reached_without_any_proxy() {
    // With a proxy in the environment, a request to a local model must still
    // go straight to it: through the proxy it would leave the machine while
    // local_only counts it as on-device.
    let _turn = turn();
    let (proxy, proxied) = counting_proxy().await;
    let _env = EnvVars::set(&[
        ("HTTP_PROXY", &proxy),
        ("http_proxy", &proxy),
        ("ALL_PROXY", &proxy),
        ("all_proxy", &proxy),
        ("NO_PROXY", ""),
        ("no_proxy", ""),
    ]);
    let (server, log) = model_server().await;
    for (provider, model, base) in [
        (LlmProvider::Ollama, "llama3.2:3b", server.clone()),
        (
            LlmProvider::LmStudio,
            "qwen2.5-coder-7b",
            format!("{server}/v1"),
        ),
    ] {
        let s = settings(provider, model, Some(&base));
        let client = s.build_llm_client().expect("client builds");
        let reply = client
            .chat(&[Message::user("hello")], &[])
            .await
            .unwrap_or_else(|e| panic!("{model}: {e}"));
        assert_eq!(reply.message.content, REPLY);
    }
    assert_eq!(
        requests(&log).len(),
        2,
        "the local server got both requests"
    );
    assert_eq!(
        *proxied.lock().unwrap(),
        0,
        "a local request went through the proxy"
    );
}
