//! `phazeai -p … --output-format json|stream-json` end to end: the binary
//! runs against a local OpenAI-compatible server and prints JSON a script can
//! read, with the answer, the turns and the token counts the server reported.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};

/// A server answering every chat request with a streamed "Hello there." and
/// a usage chunk. Returns its address.
fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for sock in listener.incoming() {
            let Ok(mut sock) = sock else { return };
            let mut request = Vec::new();
            let mut chunk = [0u8; 16 * 1024];
            loop {
                let n = sock.read(&mut chunk).unwrap_or(0);
                request.extend_from_slice(&chunk[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    let len: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let text = serde_json::json!({"choices": [{"delta": {"content": "Hello there."}}]});
            let usage = serde_json::json!({
                "choices": [],
                "usage": {"prompt_tokens": 1200, "completion_tokens": 5,
                          "prompt_tokens_details": {"cached_tokens": 1000}}
            });
            let body = format!("data: {text}\n\ndata: {usage}\n\ndata: [DONE]\n\n");
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(reply.as_bytes());
        }
    });
    url
}

fn run(format: &str) -> (String, bool) {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config").join("phazeai");
    std::fs::create_dir_all(&config).unwrap();
    let mut settings = phazeai_core::Settings::default();
    settings.llm.provider = phazeai_core::config::LlmProvider::LmStudio;
    settings.llm.model = "test-model".into();
    settings.llm.base_url = Some(serve());
    settings.search.enabled = false;
    std::fs::write(
        config.join("config.toml"),
        toml::to_string_pretty(&settings).unwrap(),
    )
    .unwrap();
    let project = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_phazeai"))
        .args(["-p", "Say hello", "--output-format", format])
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .env("XDG_CACHE_HOME", home.path().join("cache"))
        .env("XDG_DATA_HOME", home.path().join("data"))
        .env_remove("PHAZEAI_TELEMETRY")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (String::from_utf8(out.stdout).unwrap(), out.status.success())
}

#[test]
fn json_prints_one_result_object() {
    let (stdout, ok) = run("json");
    assert!(ok, "{stdout}");
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "{stdout}");
    let result: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(result["type"], "result");
    assert_eq!(result["is_error"], false);
    assert_eq!(result["result"], "Hello there.");
    assert_eq!(result["num_turns"], 1);
    assert_eq!(result["provider"], "lmstudio");
    assert_eq!(result["usage"]["output_tokens"], 5);
    assert_eq!(result["usage"]["cache_read_input_tokens"], 1000);
    assert_eq!(result["usage"]["input_tokens"], 200);
}

#[test]
fn stream_json_prints_a_line_per_event_and_the_result_last() {
    let (stdout, ok) = run("stream-json");
    assert!(ok, "{stdout}");
    let lines: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines[0]["type"], "system");
    assert_eq!(lines[0]["model"], "test-model");
    assert!(lines
        .iter()
        .any(|l| l["type"] == "text" && l["text"] == "Hello there."));
    assert!(lines.iter().any(|l| l["type"] == "usage"));
    let last = lines.last().unwrap();
    assert_eq!(last["type"], "result");
    assert_eq!(last["result"], "Hello there.");
}
