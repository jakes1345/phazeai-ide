//! LM Studio's readiness probe and model list, against a server that answers
//! only at `/v1/models`, as LM Studio does.

use phazeai_core::llm::discovery::LocalDiscovery;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A server answering 200 with one model at `GET /v1/models` and 404 to
/// anything else (or to everything, when `models` is false).
async fn serve(models: bool) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let mut buf = vec![0u8; 4096];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]).to_string();
            let found = models && head.starts_with("GET /v1/models ");
            let (status, body) = if found {
                ("200 OK", r#"{"data":[{"id":"qwen-coder"}]}"#)
            } else {
                ("404 Not Found", "{}")
            };
            let reply = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(reply.as_bytes()).await;
        }
    });
    url
}

#[tokio::test]
async fn a_base_ending_in_v1_is_not_asked_for_v1_v1() {
    let base = serve(true).await;
    for url in [base.clone(), format!("{base}/v1"), format!("{base}/v1/")] {
        assert!(LocalDiscovery::lm_studio_available(&url).await, "{url}");
        let models = LocalDiscovery::lm_studio_models(&url).await.unwrap();
        assert_eq!(models.len(), 1, "{url}");
        assert_eq!(models[0].id, "qwen-coder");
    }
}

#[tokio::test]
async fn a_server_without_the_models_api_is_not_lm_studio() {
    let base = serve(false).await;
    assert!(!LocalDiscovery::lm_studio_available(&base).await);
}

/// A server answering Ollama's model list only under `/ollama/api/tags`, as
/// one behind a reverse proxy at that path does.
async fn serve_ollama_under_a_path() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let mut buf = vec![0u8; 4096];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]).to_string();
            let (status, body) = if head.starts_with("GET /ollama/api/tags ") {
                ("200 OK", r#"{"models":[]}"#)
            } else {
                ("404 Not Found", "{}")
            };
            let reply = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(reply.as_bytes()).await;
        }
    });
    url
}

#[tokio::test]
async fn an_ollama_address_with_a_path_keeps_it() {
    let base = serve_ollama_under_a_path().await;
    for url in [format!("{base}/ollama"), format!("{base}/ollama/")] {
        assert!(LocalDiscovery::ollama_available(&url).await, "{url}");
    }
}
