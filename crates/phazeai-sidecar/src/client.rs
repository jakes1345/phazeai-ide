use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Child;
use tokio::sync::Mutex;
use tracing::{debug, warn};

const SIDECAR_QUICK_TIMEOUT: Duration = Duration::from_secs(30);
/// Indexing large trees can exceed `SIDECAR_QUICK_TIMEOUT`; keep this generous but bounded.
const SIDECAR_BUILD_INDEX_TIMEOUT: Duration = Duration::from_secs(600);

/// JSON-RPC client that communicates with the Python sidecar over stdio.
pub struct SidecarClient {
    call_lock: Mutex<()>,
    stdin: Mutex<tokio::process::ChildStdin>,
    stdout: Mutex<BufReader<tokio::process::ChildStdout>>,
    process: Mutex<Child>,
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
            call_lock: Mutex::new(()),
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(BufReader::new(stdout)),
            process: Mutex::new(process),
            next_id: AtomicU64::new(1),
        })
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        let mut process = self.process.lock().await;
        match process.kill().await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => Ok(()),
            Err(e) => Err(format!("Failed to stop sidecar: {e}")),
        }
    }

    pub async fn call(&self, method: &str, params: Option<Value>) -> Result<Value, String> {
        // Keep at most one request in-flight over stdio.
        // Without this, concurrent callers can consume and drop each other's responses.
        let _call_guard = self.call_lock.lock().await;
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
            stdin
                .flush()
                .await
                .map_err(|e| format!("Flush error: {e}"))?;
        }

        // Read and correlate response by id, skipping unrelated lines.
        let mut line = String::new();
        let timeout = match method {
            "build_index" => SIDECAR_BUILD_INDEX_TIMEOUT,
            _ => SIDECAR_QUICK_TIMEOUT,
        };
        let deadline = tokio::time::Instant::now() + timeout;
        {
            let mut stdout = self.stdout.lock().await;
            loop {
                line.clear();
                let bytes_read = tokio::time::timeout_at(deadline, stdout.read_line(&mut line))
                    .await
                    .map_err(|_| {
                        format!(
                            "Timed out waiting for sidecar response to '{}' after {}s",
                            method,
                            timeout.as_secs()
                        )
                    })?
                    .map_err(|e| format!("Read error: {e}"))?;
                if bytes_read == 0 {
                    return Err("Sidecar closed stdout unexpectedly".to_string());
                }

                let parsed: JsonRpcResponse = match serde_json::from_str(&line) {
                    Ok(response) => response,
                    Err(_) => {
                        debug!("Skipping non-JSON sidecar stdout line");
                        continue;
                    }
                };

                if parsed.id != id {
                    debug!(
                        "Skipping sidecar response id {} while waiting for {}",
                        parsed.id, id
                    );
                    continue;
                }
                return parsed.into_result();
            }
        }
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

impl Drop for SidecarClient {
    fn drop(&mut self) {
        if let Ok(mut process) = self.process.try_lock() {
            let _ = process.start_kill();
        } else {
            warn!("Sidecar client dropped while process lock was held");
        }
    }
}
