//! The Streamable HTTP transport against a small MCP server on localhost,
//! once answering with JSON bodies and once with event streams.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use phazeai_core::mcp::{McpClient, McpServerConfig};
use serde_json::{json, Value};

const SESSION: &str = "session-7f3a";
const TOKEN_VAR: &str = "PHAZE_TEST_MCP_HTTP_TOKEN";

/// What the server saw.
#[derive(Default)]
struct Seen {
    /// Each POSTed message's method, with the session header it carried.
    posts: Vec<(String, Option<String>)>,
    authorization: Vec<Option<String>>,
    protocol_versions: Vec<Option<String>>,
    deleted: Vec<Option<String>>,
}

struct Request {
    method: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let method = line.split_whitespace().next()?.to_string();
    let mut headers = HashMap::new();
    loop {
        line.clear();
        reader.read_line(&mut line).ok()?;
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    reader.read_exact(&mut body).ok()?;
    Some(Request {
        method,
        headers,
        body,
    })
}

fn respond(stream: &mut TcpStream, status: &str, extra: &[(&str, &str)], body: &str) {
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

/// Starts the server; `sse` picks how it answers requests. With `forget`,
/// it forgets the session before any tool call, as a restarted server does.
fn serve(sse: bool) -> (String, Arc<Mutex<Seen>>) {
    serve_with(sse, false)
}

fn serve_with(sse: bool, forget: bool) -> (String, Arc<Mutex<Seen>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Seen::default()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some(req) = read_request(&mut stream) else {
                continue;
            };
            let session = req.headers.get("mcp-session-id").cloned();
            if req.method == "DELETE" {
                log.lock().unwrap().deleted.push(session);
                respond(&mut stream, "200 OK", &[], "");
                continue;
            }
            let msg: Value = serde_json::from_slice(&req.body).unwrap();
            let method = msg["method"].as_str().unwrap_or_default().to_string();
            {
                let mut s = log.lock().unwrap();
                s.posts.push((method.clone(), session.clone()));
                s.authorization
                    .push(req.headers.get("authorization").cloned());
                s.protocol_versions
                    .push(req.headers.get("mcp-protocol-version").cloned());
            }
            if msg.get("id").is_none() {
                respond(&mut stream, "202 Accepted", &[], "");
                continue;
            }
            if method != "initialize" && session.as_deref() != Some(SESSION) {
                respond(&mut stream, "400 Bad Request", &[], "missing session");
                continue;
            }
            if forget && method == "tools/call" {
                respond(&mut stream, "404 Not Found", &[], "");
                continue;
            }
            let id = msg["id"].clone();
            let result = match method.as_str() {
                "initialize" => json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "fake-http", "version": "1.0"},
                }),
                "tools/list" => json!({"tools": [{
                    "name": "echo",
                    "description": "Says the text back",
                    "inputSchema": {"type": "object"},
                }]}),
                "tools/call" => json!({"content": [{
                    "type": "text",
                    "text": msg["params"]["arguments"]["text"],
                }]}),
                _ => {
                    let error = json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32601, "message": "no such method"}});
                    respond(
                        &mut stream,
                        "200 OK",
                        &[("Content-Type", "application/json")],
                        &error.to_string(),
                    );
                    continue;
                }
            };
            let reply = json!({"jsonrpc": "2.0", "id": id, "result": result});
            let mut extra = vec![];
            if method == "initialize" {
                extra.push(("Mcp-Session-Id", SESSION));
            }
            if sse {
                // A progress notification first, split over two data lines'
                // worth of events, then the answer.
                let progress =
                    json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {}});
                let body = format!(
                    ": keep-alive\n\nevent: message\ndata: {progress}\n\nevent: message\nid: 1\ndata: {reply}\n\n"
                );
                extra.push(("Content-Type", "text/event-stream"));
                respond(&mut stream, "200 OK", &extra, &body);
            } else {
                // A batch, as a server may answer.
                extra.push(("Content-Type", "application/json"));
                respond(&mut stream, "200 OK", &extra, &json!([reply]).to_string());
            }
        }
    });
    (url, seen)
}

fn config(url: &str) -> McpServerConfig {
    McpServerConfig {
        name: "remote".into(),
        url: Some(url.to_string()),
        headers: HashMap::from([(
            "Authorization".to_string(),
            format!("Bearer ${{{TOKEN_VAR}}}"),
        )]),
        ..Default::default()
    }
}

fn exercise(sse: bool) {
    std::env::set_var(TOKEN_VAR, "tok-123");
    let (url, seen) = serve(sse);
    let client = McpClient::connect(&config(&url)).expect("connect over HTTP");
    assert_eq!(client.server_info().unwrap().name, "fake-http");
    assert_eq!(client.tools().len(), 1);
    assert_eq!(client.tools()[0].name, "echo");

    let out = client
        .call_tool("echo", json!({"text": "over the wire"}))
        .expect("tool call");
    assert_eq!(out.content[0].text.as_deref(), Some("over the wire"));
    assert!(client.is_alive());
    drop(client);

    // The session is ended once the client goes.
    let deadline = Instant::now() + Duration::from_secs(5);
    while seen.lock().unwrap().deleted.is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let seen = seen.lock().unwrap();
    assert_eq!(seen.deleted, vec![Some(SESSION.to_string())]);

    let methods: Vec<&str> = seen.posts.iter().map(|(m, _)| m.as_str()).collect();
    assert_eq!(
        methods,
        [
            "initialize",
            "notifications/initialized",
            "tools/list",
            "tools/call"
        ]
    );
    // Every message after `initialize` carried the session back.
    assert_eq!(seen.posts[0].1, None);
    assert!(seen.posts[1..]
        .iter()
        .all(|(_, s)| s.as_deref() == Some(SESSION)));
    // The header's ${VAR} was filled in from the environment.
    assert!(seen
        .authorization
        .iter()
        .all(|a| a.as_deref() == Some("Bearer tok-123")));
    // The negotiated version goes with every message after `initialize`.
    assert_eq!(seen.protocol_versions[0], None);
    assert!(seen.protocol_versions[1..]
        .iter()
        .all(|v| v.as_deref() == Some("2025-06-18")));
}

#[test]
fn a_server_that_answers_with_json() {
    exercise(false);
}

#[test]
fn a_server_that_answers_with_an_event_stream() {
    exercise(true);
}

#[test]
fn a_forgotten_session_marks_the_client_for_reconnecting() {
    std::env::set_var(TOKEN_VAR, "tok-123");
    let (url, _seen) = serve_with(false, true);
    let client = McpClient::connect(&config(&url)).expect("connect over HTTP");
    assert!(client.is_alive());
    let err = client.call_tool("echo", json!({"text": "x"})).unwrap_err();
    assert!(err.contains("ended the session"), "{err}");
    assert!(!client.is_alive());
}

#[test]
fn a_header_variable_that_is_not_set_stops_the_connection() {
    let mut cfg = config("http://127.0.0.1:9/mcp");
    cfg.headers = HashMap::from([(
        "X-Api-Key".to_string(),
        "${PHAZE_TEST_MCP_HTTP_NEVER_SET}".to_string(),
    )]);
    let err = McpClient::connect(&cfg).err().expect("must fail");
    assert!(err.contains("PHAZE_TEST_MCP_HTTP_NEVER_SET"), "{err}");
}

#[test]
fn a_config_needs_a_command_or_a_url() {
    let err = McpClient::connect(&McpServerConfig {
        name: "empty".into(),
        ..Default::default()
    })
    .err()
    .expect("must fail");
    assert!(err.contains("`command`") && err.contains("`url`"), "{err}");
}

#[test]
fn headers_are_not_sent_unencrypted_to_another_machine() {
    let err = McpClient::connect(&McpServerConfig {
        name: "plain".into(),
        url: Some("http://mcp.example.com/mcp".into()),
        headers: HashMap::from([("X-Api-Key".to_string(), "k".to_string())]),
        ..Default::default()
    })
    .err()
    .expect("must fail");
    assert!(err.contains("use https"), "{err}");
}
