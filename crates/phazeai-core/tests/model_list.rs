//! Reading a provider's model list over HTTP: every page is read, the key
//! goes in the provider's own header, and errors say what went wrong.

use std::sync::{Arc, Mutex};

use phazeai_core::llm::model_list::fetch_models;
use phazeai_core::ProviderId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A server that answers each request with the next of `pages` (status,
/// body) and keeps every request's head.
async fn serve(pages: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let heads = seen.clone();
    tokio::spawn(async move {
        for (status, body) in pages {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let mut request = Vec::new();
            let mut chunk = [0u8; 4096];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = sock.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..n]);
            }
            heads
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&request).to_ascii_lowercase());
            let reply = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(reply.as_bytes()).await.unwrap();
            let _ = sock.shutdown().await;
        }
    });
    (url, seen)
}

#[tokio::test]
async fn every_page_of_claudes_list_is_read() {
    let first = serde_json::json!({
        "data": [{"id": "claude-a", "display_name": "A", "max_input_tokens": 1_000_000}],
        "has_more": true, "last_id": "claude-a"
    });
    let second = serde_json::json!({
        "data": [{"id": "claude-b", "display_name": "B", "max_input_tokens": 200_000}],
        "has_more": false, "last_id": "claude-b"
    });
    let (url, seen) = serve(vec![(200, first.to_string()), (200, second.to_string())]).await;
    let models = fetch_models(&ProviderId::Claude, &url, Some("sk-test"))
        .await
        .unwrap();
    let got: Vec<_> = models
        .iter()
        .map(|m| (m.id.as_str(), m.context_window))
        .collect();
    assert_eq!(
        got,
        [("claude-a", Some(1_000_000)), ("claude-b", Some(200_000))]
    );
    let heads = seen.lock().unwrap();
    assert!(heads[0].starts_with("get /v1/models?limit=1000 "));
    assert!(heads[0].contains("x-api-key: sk-test"));
    assert!(heads[1].starts_with("get /v1/models?limit=1000&after_id=claude-a "));
}

#[tokio::test]
async fn a_refused_key_is_named_as_the_problem() {
    let (url, _) = serve(vec![(401, r#"{"error":"bad key"}"#.into())]).await;
    let err = fetch_models(&ProviderId::OpenAI, &url, Some("sk-bad"))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("turned the API key down"), "{err}");
    let err = fetch_models(&ProviderId::OpenAI, &url, None)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("needs an API key"), "{err}");
}

#[tokio::test]
async fn a_local_server_needs_no_key() {
    let body = r#"{"object":"list","data":[{"id":"qwen3-coder:30b"},{"id":"nomic-embed-text"}]}"#;
    let (url, seen) = serve(vec![(200, body.into())]).await;
    let models = fetch_models(&ProviderId::Ollama, &url, None).await.unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "qwen3-coder:30b");
    assert!(!seen.lock().unwrap()[0].contains("authorization"));
}

#[tokio::test]
async fn the_key_is_not_sent_unencrypted_to_another_machine() {
    // As for chat: a cloud provider pointed at a plain-http address on
    // another machine would get the key in clear text. Nothing is sent.
    std::env::set_var("OPENAI_API_KEY", "sk-must-not-leave");
    let mut settings = phazeai_core::Settings::default();
    settings.llm.provider = phazeai_core::config::LlmProvider::OpenAI;
    settings.llm.base_url = Some("http://192.0.2.1:9/v1".into());
    let started = std::time::Instant::now();
    let err = settings.live_models(true).await.unwrap_err().to_string();
    assert!(err.contains("unencrypted"), "{err}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "refused before any request, not after a timeout"
    );
}
