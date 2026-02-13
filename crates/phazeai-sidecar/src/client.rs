use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Child;
use tokio::sync::Mutex;

/// JSON-RPC client that communicates with the Python sidecar over stdio.
pub struct SidecarClient {
    stdin: Mutex<tokio::process::ChildStdin>,
    stdout: Mutex<BufReader<tokio::process::ChildStdout>>,
    next_id: AtomicU64,
}

impl SidecarClient {
    pub fn from_process(mut process: Child) -> Result<Self, String> {
        let stdin = process
            .stdin
            .take()
            .ok_or("Failed to capture sidecar stdin")?;
        let stdout = process
            .stdout
            .take()
            .ok_or("Failed to capture sidecar stdout")?;

        Ok(Self {
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(BufReader::new(stdout)),
            next_id: AtomicU64::new(1),
        })
    }

    pub async fn call(&self, method: &str, params: Option<Value>) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let request = JsonRpcRequest::new(id, method, params);

        let mut request_line =
            serde_json::to_string(&request).map_err(|e| format!("Serialize error: {e}"))?;
        request_line.push('\n');

        // Write request
        {
            let mut stdin = self.stdin.lock().await;
            stdin
                .write_all(request_line.as_bytes())
                .await
                .map_err(|e| format!("Write error: {e}"))?;
            stdin.flush().await.map_err(|e| format!("Flush error: {e}"))?;
        }

        // Read response
        let mut line = String::new();
        {
            let mut stdout = self.stdout.lock().await;
            stdout
                .read_line(&mut line)
                .await
                .map_err(|e| format!("Read error: {e}"))?;
        }

        let response: JsonRpcResponse =
            serde_json::from_str(&line).map_err(|e| format!("Parse error: {e}"))?;

        response.into_result()
    }

    pub async fn search_embeddings(&self, query: &str, top_k: usize) -> Result<Value, String> {
        self.call(
            "search",
            Some(serde_json::json!({
                "query": query,
                "top_k": top_k,
            })),
        )
        .await
    }

    pub async fn build_index(&self, paths: &[String]) -> Result<Value, String> {
        self.call(
            "build_index",
            Some(serde_json::json!({
                "paths": paths,
            })),
        )
        .await
    }

    pub async fn analyze_file(&self, path: &str, content: &str) -> Result<Value, String> {
        self.call(
            "analyze",
            Some(serde_json::json!({
                "path": path,
                "content": content,
            })),
        )
        .await
    }

    pub async fn health_check(&self) -> bool {
        self.call("ping", None).await.is_ok()
    }
}
