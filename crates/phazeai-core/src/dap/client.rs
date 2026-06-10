//! DAP client — spawns a debug adapter process and speaks the protocol.

use super::protocol::*;
use crate::error::PhazeError;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};

/// A running DAP session with a debug adapter process.
pub struct DapClient {
    child: Mutex<Child>,
    stdin: Mutex<Box<dyn Write + Send>>,
    seq: AtomicU64,
    alive: Arc<AtomicBool>,
    event_tx: mpsc::Sender<DebugEvent>,
    pending: Mutex<HashMap<u64, mpsc::SyncSender<DapResponse>>>,
    pub capabilities: Mutex<Capabilities>,
}

impl DapClient {
    /// Spawn a debug adapter process. Returns the client + an event receiver
    /// that the UI polls for stopped/output/terminated events.
    pub fn spawn(
        adapter_cmd: &str,
        adapter_args: &[&str],
    ) -> Result<(Arc<Self>, mpsc::Receiver<DebugEvent>), PhazeError> {
        let mut child = Command::new(adapter_cmd)
            .args(adapter_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| PhazeError::Config(format!("Failed to spawn DAP adapter: {e}")))?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (event_tx, event_rx) = mpsc::channel();
        let alive = Arc::new(AtomicBool::new(true));

        let client = Arc::new(Self {
            child: Mutex::new(child),
            stdin: Mutex::new(Box::new(stdin)),
            seq: AtomicU64::new(1),
            alive: alive.clone(),
            event_tx,
            pending: Mutex::new(HashMap::new()),
            capabilities: Mutex::new(Capabilities::default()),
        });

        // Reader thread — parses Content-Length framed JSON messages.
        let client_weak = Arc::downgrade(&client);
        let alive2 = alive.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while alive2.load(Ordering::Relaxed) {
                match read_message(&mut reader) {
                    Ok(msg) => {
                        let Some(client) = client_weak.upgrade() else {
                            break;
                        };
                        client.handle_message(msg);
                    }
                    Err(_) => {
                        alive2.store(false, Ordering::Relaxed);
                        break;
                    }
                }
            }
        });

        Ok((client, event_rx))
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// Send a request and wait for its response (blocking).
    pub fn request(
        &self,
        command: &str,
        arguments: Option<Value>,
    ) -> Result<DapResponse, PhazeError> {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let req = DapRequest::new(seq, command, arguments);
        let body = serde_json::to_string(&req).map_err(|e| PhazeError::Config(e.to_string()))?;

        let (tx, rx) = mpsc::sync_channel(1);
        self.pending.lock().unwrap().insert(seq, tx);

        self.send_raw(&body)?;

        rx.recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| PhazeError::Config("DAP request timed out".into()))
    }

    /// Initialize the debug adapter.
    pub fn initialize(&self) -> Result<(), PhazeError> {
        let args = serde_json::json!({
            "clientID": "phazeai",
            "clientName": "PhazeAI IDE",
            "adapterID": "generic",
            "linesStartAt1": true,
            "columnsStartAt1": true,
            "pathFormat": "path",
            "supportsRunInTerminalRequest": false,
        });
        let resp = self.request("initialize", Some(args))?;
        if let Some(body) = resp.body {
            if let Ok(caps) = serde_json::from_value::<Capabilities>(body) {
                *self.capabilities.lock().unwrap() = caps;
            }
        }
        Ok(())
    }

    /// Send launch request.
    pub fn launch(&self, config: &LaunchConfig) -> Result<DapResponse, PhazeError> {
        let args = serde_json::to_value(config).map_err(|e| PhazeError::Config(e.to_string()))?;
        self.request("launch", Some(args))
    }

    /// Set breakpoints for a source file.
    pub fn set_breakpoints(
        &self,
        path: &std::path::Path,
        breakpoints: &[SourceBreakpoint],
    ) -> Result<DapResponse, PhazeError> {
        let args = serde_json::json!({
            "source": { "path": path.to_string_lossy() },
            "breakpoints": breakpoints,
        });
        self.request("setBreakpoints", Some(args))
    }

    /// Send configurationDone (required after setting initial breakpoints).
    pub fn configuration_done(&self) -> Result<DapResponse, PhazeError> {
        self.request("configurationDone", None)
    }

    /// Continue execution.
    pub fn continue_execution(&self, thread_id: u64) -> Result<DapResponse, PhazeError> {
        let args = serde_json::json!({ "threadId": thread_id });
        self.request("continue", Some(args))
    }

    /// Step over (next line).
    pub fn next(&self, thread_id: u64) -> Result<DapResponse, PhazeError> {
        let args = serde_json::json!({ "threadId": thread_id });
        self.request("next", Some(args))
    }

    /// Step into.
    pub fn step_in(&self, thread_id: u64) -> Result<DapResponse, PhazeError> {
        let args = serde_json::json!({ "threadId": thread_id });
        self.request("stepIn", Some(args))
    }

    /// Step out.
    pub fn step_out(&self, thread_id: u64) -> Result<DapResponse, PhazeError> {
        let args = serde_json::json!({ "threadId": thread_id });
        self.request("stepOut", Some(args))
    }

    /// Get stack trace for a thread.
    pub fn stack_trace(&self, thread_id: u64) -> Result<Vec<StackFrame>, PhazeError> {
        let args = serde_json::json!({ "threadId": thread_id });
        let resp = self.request("stackTrace", Some(args))?;
        let frames = resp
            .body
            .and_then(|b| b.get("stackFrames").cloned())
            .and_then(|v| serde_json::from_value::<Vec<StackFrame>>(v).ok())
            .unwrap_or_default();
        Ok(frames)
    }

    /// Get scopes for a frame.
    pub fn scopes(&self, frame_id: u64) -> Result<Vec<Scope>, PhazeError> {
        let args = serde_json::json!({ "frameId": frame_id });
        let resp = self.request("scopes", Some(args))?;
        let scopes = resp
            .body
            .and_then(|b| b.get("scopes").cloned())
            .and_then(|v| serde_json::from_value::<Vec<Scope>>(v).ok())
            .unwrap_or_default();
        Ok(scopes)
    }

    /// Get variables for a variables reference.
    pub fn variables(&self, reference: u64) -> Result<Vec<Variable>, PhazeError> {
        let args = serde_json::json!({ "variablesReference": reference });
        let resp = self.request("variables", Some(args))?;
        let vars = resp
            .body
            .and_then(|b| b.get("variables").cloned())
            .and_then(|v| serde_json::from_value::<Vec<Variable>>(v).ok())
            .unwrap_or_default();
        Ok(vars)
    }

    /// Disconnect and kill the adapter.
    pub fn disconnect(&self) {
        let _ = self.request(
            "disconnect",
            Some(serde_json::json!({ "terminateDebuggee": true })),
        );
        self.alive.store(false, Ordering::Relaxed);
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }

    fn send_raw(&self, body: &str) -> Result<(), PhazeError> {
        let msg = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut stdin = self.stdin.lock().unwrap();
        stdin
            .write_all(msg.as_bytes())
            .map_err(|e| PhazeError::Config(format!("DAP write error: {e}")))?;
        stdin
            .flush()
            .map_err(|e| PhazeError::Config(format!("DAP flush error: {e}")))
    }

    fn handle_message(&self, msg: DapMessage) {
        match msg {
            DapMessage::Response(resp) => {
                if let Some(tx) = self.pending.lock().unwrap().remove(&resp.request_seq) {
                    let _ = tx.send(resp);
                }
            }
            DapMessage::Event(event) => {
                let debug_event = match event.event.as_str() {
                    "initialized" => Some(DebugEvent::Initialized),
                    "stopped" => {
                        let body = event.body.unwrap_or_default();
                        let reason = body
                            .get("reason")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let thread_id = body.get("threadId").and_then(|v| v.as_u64()).unwrap_or(1);
                        Some(DebugEvent::Stopped {
                            reason: StopReason::parse(reason),
                            thread_id,
                        })
                    }
                    "continued" => {
                        let thread_id = event
                            .body
                            .as_ref()
                            .and_then(|b| b.get("threadId"))
                            .and_then(|v| v.as_u64())
                            .unwrap_or(1);
                        Some(DebugEvent::Continued { thread_id })
                    }
                    "exited" => {
                        let code = event
                            .body
                            .as_ref()
                            .and_then(|b| b.get("exitCode"))
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0);
                        Some(DebugEvent::Exited { exit_code: code })
                    }
                    "terminated" => Some(DebugEvent::Terminated),
                    "output" => {
                        let body = event.body.unwrap_or_default();
                        let category = body
                            .get("category")
                            .and_then(|v| v.as_str())
                            .unwrap_or("console")
                            .to_string();
                        let output = body
                            .get("output")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        Some(DebugEvent::Output { category, output })
                    }
                    "breakpoint" => {
                        let body = event.body.unwrap_or_default();
                        let reason = body
                            .get("reason")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let bp = body.get("breakpoint").cloned().unwrap_or_default();
                        let id = bp.get("id").and_then(|v| v.as_u64());
                        let verified = bp
                            .get("verified")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        Some(DebugEvent::Breakpoint {
                            reason,
                            id,
                            verified,
                        })
                    }
                    _ => None,
                };
                if let Some(evt) = debug_event {
                    let _ = self.event_tx.send(evt);
                }
            }
        }
    }
}

/// Read one Content-Length framed message from the reader.
fn read_message(reader: &mut BufReader<impl std::io::Read>) -> Result<DapMessage, PhazeError> {
    let mut content_length: usize = 0;
    loop {
        let mut header = String::new();
        reader
            .read_line(&mut header)
            .map_err(|e| PhazeError::Config(format!("DAP read header: {e}")))?;
        let header = header.trim();
        if header.is_empty() {
            break;
        }
        if let Some(len_str) = header.strip_prefix("Content-Length: ") {
            content_length = len_str
                .parse()
                .map_err(|e| PhazeError::Config(format!("Bad Content-Length: {e}")))?;
        }
    }
    if content_length == 0 {
        return Err(PhazeError::Config("Empty DAP message".into()));
    }
    let mut buf = vec![0u8; content_length];
    reader
        .read_exact(&mut buf)
        .map_err(|e| PhazeError::Config(format!("DAP read body: {e}")))?;
    serde_json::from_slice(&buf).map_err(|e| PhazeError::Config(format!("DAP parse: {e}")))
}

impl Drop for DapClient {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}
