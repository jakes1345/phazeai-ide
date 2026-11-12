//! MCP's Streamable HTTP transport (spec 2025-03-26 and later), for servers
//! reached by URL rather than run as a child process.
//!
//! Every client message is a `POST` to the server's one endpoint. A request's
//! answer comes back either as a JSON body or as a `text/event-stream` whose
//! events carry JSON-RPC messages, the answer among them. A notification is
//! answered `202 Accepted`. The server may hand out a session in an
//! `Mcp-Session-Id` header on its answer to `initialize`; every later message
//! carries it back, and the client ends the session with a `DELETE` when it
//! is done. A `404` for a message that carries a session means the server
//! forgot the session, so the client is marked dead and reconnected by
//! [`super::McpManager::health_check`].
//!
//! `reqwest::blocking` must not run on a tokio worker (it starts a runtime of
//! its own and panics there), and [`super::McpClient`] is called from both
//! sync and async code, so all HTTP happens on one thread per server that
//! owns the client and takes jobs over a channel.

use std::io::{BufRead, BufReader, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE};
use serde_json::Value;

use super::REQUEST_TIMEOUT;

/// The protocol version offered to HTTP servers.
pub(super) const PROTOCOL_VERSION: &str = "2025-06-18";
const SESSION_HEADER: &str = "mcp-session-id";
const VERSION_HEADER: &str = "mcp-protocol-version";
/// The most of a response body read, in bytes: a tool's output, not a feed.
const MAX_BODY: u64 = 32 * 1024 * 1024;
/// The most of an error body quoted back, in bytes.
const MAX_ERROR_TEXT: usize = 300;
/// How long ending the session may take when the client goes away.
const DELETE_TIMEOUT: Duration = Duration::from_secs(5);

/// Replaces each `${VAR}` in `value` with that environment variable. An unset
/// variable is an error naming it, so a key left out of the environment
/// fails at connect time instead of being sent as the literal `${VAR}`.
pub(super) fn expand_env(value: &str) -> Result<String, String> {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            // No closing brace: not a reference, keep it as written.
            out.push_str(&rest[start..]);
            return Ok(out);
        };
        let var = &after[..end];
        match std::env::var(var) {
            Ok(v) => out.push_str(&v),
            Err(_) => return Err(var.to_string()),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Work for the HTTP thread.
enum Job {
    /// Send a request and wait for the response with this id.
    Request {
        message: Value,
        id: u64,
        reply: mpsc::Sender<Result<Value, String>>,
    },
    /// Send a notification (or a response); nothing comes back.
    Notify {
        message: Value,
        reply: mpsc::Sender<Result<(), String>>,
    },
    /// The version `initialize` settled on, sent with every later message.
    Version(String),
}

/// The client side of one HTTP server: a channel to its thread.
pub(super) struct HttpTransport {
    jobs: mpsc::Sender<Job>,
    alive: Arc<AtomicBool>,
}

impl HttpTransport {
    /// Starts the thread for the server at `url`. `headers` are already
    /// expanded; their values are never logged or put in an error.
    pub(super) fn start(
        name: &str,
        url: reqwest::Url,
        headers: Vec<(String, String)>,
        local: bool,
        alive: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let mut map = HeaderMap::new();
        for (key, value) in headers {
            let header = HeaderName::from_bytes(key.as_bytes())
                .map_err(|_| format!("MCP server '{name}': `{key}` is not a header name"))?;
            let mut value = HeaderValue::from_str(&value).map_err(|_| {
                format!("MCP server '{name}': header `{key}` has characters a header can't carry")
            })?;
            value.set_sensitive(true);
            map.insert(header, value);
        }
        let mut builder = reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(Duration::from_secs(10));
        if local {
            builder = builder.no_proxy();
        }
        let (jobs, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread_alive = alive.clone();
        std::thread::Builder::new()
            .name(format!("mcp-http-{name}"))
            .spawn(move || {
                // The client is built and dropped on this thread: a blocking
                // client may not be dropped inside an async context either.
                let client = match builder.build() {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("could not start an HTTP client: {e}")));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                Worker {
                    client,
                    url,
                    headers: map,
                    session: None,
                    version: None,
                    alive: thread_alive,
                }
                .run(rx);
            })
            .map_err(|e| format!("MCP server '{name}': could not start a thread: {e}"))?;
        ready_rx
            .recv()
            .map_err(|_| format!("MCP server '{name}': the HTTP thread stopped"))?
            .map_err(|e| format!("MCP server '{name}': {e}"))?;
        Ok(Self { jobs, alive })
    }

    /// Sends request `message` (whose id is `id`) and returns the response.
    pub(super) fn request(&self, message: Value, id: u64) -> Result<Value, String> {
        let (reply, rx) = mpsc::channel();
        self.jobs
            .send(Job::Request { message, id, reply })
            .map_err(|_| "the connection is closed".to_string())?;
        self.wait(rx)
    }

    /// Sends a message that gets no response.
    pub(super) fn notify(&self, message: Value) -> Result<(), String> {
        let (reply, rx) = mpsc::channel();
        self.jobs
            .send(Job::Notify { message, reply })
            .map_err(|_| "the connection is closed".to_string())?;
        self.wait(rx)
    }

    pub(super) fn set_protocol_version(&self, version: &str) {
        let _ = self.jobs.send(Job::Version(version.to_string()));
    }

    fn wait<T>(&self, rx: mpsc::Receiver<Result<T, String>>) -> Result<T, String> {
        // The worker's own client times out first; this only catches a
        // worker that is stuck for some other reason.
        match rx.recv_timeout(REQUEST_TIMEOUT + Duration::from_secs(10)) {
            Ok(result) => result,
            Err(_) => {
                self.alive.store(false, Ordering::SeqCst);
                Err(format!(
                    "no response within {} s",
                    REQUEST_TIMEOUT.as_secs()
                ))
            }
        }
    }
}

/// The HTTP thread's state.
struct Worker {
    client: reqwest::blocking::Client,
    url: reqwest::Url,
    headers: HeaderMap,
    session: Option<HeaderValue>,
    version: Option<HeaderValue>,
    alive: Arc<AtomicBool>,
}

impl Worker {
    fn run(mut self, jobs: mpsc::Receiver<Job>) {
        for job in jobs {
            match job {
                Job::Request { message, id, reply } => {
                    let _ = reply.send(self.request(&message, id));
                }
                Job::Notify { message, reply } => {
                    let _ = reply.send(self.post(&message).map(|_| ()));
                }
                Job::Version(v) => self.version = HeaderValue::from_str(&v).ok(),
            }
        }
        // Every sender is gone: the client was dropped. End the session so
        // the server can free it; a server that doesn't allow that answers
        // 405, which is fine.
        if let Some(session) = self.session.take() {
            let _ = self
                .client
                .delete(self.url.clone())
                .headers(self.headers.clone())
                .header(SESSION_HEADER, session)
                .timeout(DELETE_TIMEOUT)
                .send();
        }
    }

    fn request(&mut self, message: &Value, id: u64) -> Result<Value, String> {
        let response = self
            .post(message)?
            .ok_or_else(|| "the server accepted the request but sent no response".to_string())?;
        let is_stream = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.trim_start().starts_with("text/event-stream"));
        if is_stream {
            return self.read_stream(response, id);
        }
        let mut body = Vec::new();
        response
            .take(MAX_BODY)
            .read_to_end(&mut body)
            .map_err(|e| self.transport_error(e))?;
        let value: Value = serde_json::from_slice(&body)
            .map_err(|e| format!("the server's response is not JSON: {e}"))?;
        // A JSON body is one message or a batch of them.
        let found = match value {
            Value::Array(items) => items.into_iter().find(|m| is_response_to(m, id)),
            one if is_response_to(&one, id) => Some(one),
            _ => None,
        };
        found.ok_or_else(|| format!("the server's response did not answer request {id}"))
    }

    /// Reads server-sent events until the response to `id` arrives. A request
    /// from the server is answered; other messages on the stream (progress,
    /// log notifications) are passed over.
    fn read_stream(
        &mut self,
        response: reqwest::blocking::Response,
        id: u64,
    ) -> Result<Value, String> {
        let mut reader = BufReader::new(response.take(MAX_BODY));
        let mut data = String::new();
        let mut line = String::new();
        loop {
            line.clear();
            let read = reader
                .read_line(&mut line)
                .map_err(|e| self.transport_error(e))?;
            let text = line.trim_end_matches(['\n', '\r']);
            if read == 0 || text.is_empty() {
                // End of an event (or of the stream): dispatch what it held.
                if !data.is_empty() {
                    if let Ok(message) = serde_json::from_str::<Value>(&data) {
                        if is_response_to(&message, id) {
                            return Ok(message);
                        }
                        // A request from the server (a `ping` while it
                        // works): answered by its own POST, as the spec has it.
                        if let Some(answer) = super::answer_server_request(&message) {
                            if let Err(e) = self.post(&answer) {
                                tracing::warn!("MCP: could not answer the server's request: {e}");
                            }
                        }
                    }
                    data.clear();
                }
                if read == 0 {
                    return Err(format!(
                        "the server's event stream ended without answering request {id}"
                    ));
                }
                continue;
            }
            if let Some(rest) = text.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            }
            // `event:`, `id:`, `retry:` and `:` comments need nothing here.
        }
    }

    /// POSTs one message. `None` for `202 Accepted` (or `204`), which is how
    /// a server answers a notification.
    fn post(&mut self, message: &Value) -> Result<Option<reqwest::blocking::Response>, String> {
        let mut request = self
            .client
            .post(self.url.clone())
            .headers(self.headers.clone())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .body(serde_json::to_vec(message).map_err(|e| e.to_string())?);
        if let Some(session) = &self.session {
            request = request.header(SESSION_HEADER, session.clone());
        }
        if let Some(version) = &self.version {
            request = request.header(VERSION_HEADER, version.clone());
        }
        let response = request.send().map_err(|e| self.transport_error(e))?;
        let status = response.status();
        if status.as_u16() == 404 && self.session.is_some() {
            self.session = None;
            self.alive.store(false, Ordering::SeqCst);
            return Err("the server ended the session (HTTP 404); reconnecting".into());
        }
        if !status.is_success() {
            let mut text = String::new();
            let _ = response
                .take(MAX_ERROR_TEXT as u64)
                .read_to_string(&mut text);
            let text = text.trim();
            let what = match status.as_u16() {
                401 | 403 => "turned the request down; check the server's `headers`".to_string(),
                _ => format!("answered HTTP {}", status.as_u16()),
            };
            return Err(if text.is_empty() {
                format!("the server {what} (HTTP {})", status.as_u16())
            } else {
                format!("the server {what} (HTTP {}): {text}", status.as_u16())
            });
        }
        if self.session.is_none() {
            if let Some(session) = response.headers().get(SESSION_HEADER) {
                self.session = Some(session.clone());
            }
        }
        if matches!(status.as_u16(), 202 | 204) {
            return Ok(None);
        }
        Ok(Some(response))
    }

    /// A failure to reach the server or read its answer: the connection is
    /// no good, so [`super::McpManager::health_check`] should replace it.
    fn transport_error(&self, e: impl std::fmt::Display) -> String {
        self.alive.store(false, Ordering::SeqCst);
        format!("could not reach the server: {e}")
    }
}

/// Whether `message` is the JSON-RPC response to request `id`.
fn is_response_to(message: &Value, id: u64) -> bool {
    message.get("id").and_then(Value::as_u64) == Some(id)
        && (message.get("result").is_some() || message.get("error").is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_values_take_variables_from_the_environment() {
        std::env::set_var("PHAZE_TEST_MCP_TOKEN", "s3cret");
        assert_eq!(
            expand_env("Bearer ${PHAZE_TEST_MCP_TOKEN}").unwrap(),
            "Bearer s3cret"
        );
        assert_eq!(expand_env("plain").unwrap(), "plain");
        assert_eq!(expand_env("a ${ b").unwrap(), "a ${ b");
        assert_eq!(
            expand_env("${PHAZE_TEST_MCP_UNSET_VAR}").unwrap_err(),
            "PHAZE_TEST_MCP_UNSET_VAR"
        );
    }

    #[test]
    fn only_a_response_with_the_same_id_answers_a_request() {
        let ok = serde_json::json!({"jsonrpc": "2.0", "id": 3, "result": {}});
        let note = serde_json::json!({"jsonrpc": "2.0", "method": "notifications/progress"});
        assert!(is_response_to(&ok, 3));
        assert!(!is_response_to(&ok, 4));
        assert!(!is_response_to(&note, 3));
    }
}
