//! `phazeai review` end to end: the binary reviews a branch against a local
//! OpenAI-compatible model, checks its findings against the diff, fails CI
//! on request, and posts one review with inline comments to a local stand-in
//! for GitHub's API.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// A tiny HTTP server: records (request line, body) and answers with
/// `reply(request line, body)` as (content type, body).
fn serve(reply: fn(&str, &str) -> (String, String), seen: Seen) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for sock in listener.incoming() {
            let Ok(mut sock) = sock else { return };
            let mut request = Vec::new();
            let mut chunk = [0u8; 16 * 1024];
            let mut body_start = 0;
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
                    body_start = end + 4;
                    if request.len() >= body_start + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&request).into_owned();
            let first = text.lines().next().unwrap_or("").to_string();
            let body =
                String::from_utf8_lossy(&request[body_start.min(request.len())..]).into_owned();
            seen.lock().unwrap().push((first.clone(), body.clone()));
            let (ctype, out) = reply(&first, &body);
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: {ctype}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{out}",
                out.len()
            );
            let _ = sock.write_all(resp.as_bytes());
        }
    });
    url
}

/// The model: one finding on the division by zero (line 3 of calc.py), one
/// on a line far outside the diff (dropped) and one on a file it was not
/// shown (dropped).
fn model(_first: &str, _body: &str) -> (String, String) {
    let answer = serde_json::json!({"findings": [
        {"file": "calc.py", "line": 3, "severity": "high", "title": "Division by zero",
         "detail": "ratio() divides by zero for every input and raises ZeroDivisionError.",
         "fix": "divide by b"},
        {"file": "calc.py", "line": 90, "severity": "low", "title": "Far away", "detail": "x"},
        {"file": "elsewhere.py", "line": 1, "severity": "high", "title": "Not shown", "detail": "x"}
    ]})
    .to_string();
    let chunk = serde_json::json!({"choices": [{"delta": {"content": answer}}]});
    let usage = serde_json::json!({"choices": [], "usage": {"prompt_tokens": 300, "completion_tokens": 40}});
    (
        "text/event-stream".into(),
        format!("data: {chunk}\n\ndata: {usage}\n\ndata: [DONE]\n\n"),
    )
}

fn github(_first: &str, _body: &str) -> (String, String) {
    (
        "application/json".into(),
        r#"{"id":1,"html_url":"https://github.example/o/r/pull/7#pullrequestreview-1"}"#.into(),
    )
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

struct Fixture {
    home: tempfile::TempDir,
    repo: tempfile::TempDir,
    model_seen: Seen,
}

fn fixture() -> Fixture {
    let model_seen: Seen = Arc::default();
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config").join("phazeai");
    std::fs::create_dir_all(&config).unwrap();
    let mut settings = phazeai_core::Settings::default();
    settings.llm.provider = phazeai_core::config::LlmProvider::LmStudio;
    settings.llm.model = "review-model".into();
    settings.llm.base_url = Some(serve(model, model_seen.clone()));
    settings.search.enabled = false;
    std::fs::write(
        config.join("config.toml"),
        toml::to_string_pretty(&settings).unwrap(),
    )
    .unwrap();

    let repo = tempfile::tempdir().unwrap();
    let r = repo.path();
    git(r, &["init", "-q", "-b", "main"]);
    git(r, &["config", "user.email", "dev@example.com"]);
    git(r, &["config", "user.name", "Dev"]);
    std::fs::write(r.join("calc.py"), "def add(a, b):\n    return a + b\n").unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-q", "-m", "first"]);
    git(r, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(
        r.join("calc.py"),
        "def add(a, b):\n    return a + b\ndef ratio(a, b):\n    return a / 0\n",
    )
    .unwrap();
    std::fs::write(r.join("package-lock.json"), "{}\n").unwrap();
    std::fs::write(r.join(".env"), "SECRET=hunter2\n").unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-q", "-m", "add ratio"]);
    Fixture {
        home,
        repo,
        model_seen,
    }
}

fn phazeai(f: &Fixture, args: &[&str], env: &[(&str, &str)]) -> Output {
    let home = f.home.path();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_phazeai"));
    cmd.args(args)
        .current_dir(f.repo.path())
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env_remove("PHAZEAI_TELEMETRY")
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_REPOSITORY")
        .env_remove("GITHUB_API_URL")
        .stdin(Stdio::null());
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

fn show(out: &Output) -> String {
    format!(
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn a_branch_review_reports_only_findings_on_the_diff() {
    let f = fixture();
    let out = phazeai(
        &f,
        &["review", "--base", "main", "--output-format", "json"],
        &[],
    );
    assert!(out.status.success(), "{}", show(&out));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let findings = json["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "{}", show(&out));
    assert_eq!(findings[0]["file"], "calc.py");
    assert_eq!(findings[0]["line"], 3);
    assert_eq!(findings[0]["severity"], "high");
    assert_eq!(json["files_reviewed"], serde_json::json!(["calc.py"]));
    let skipped: Vec<&str> = json["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["path"].as_str().unwrap())
        .collect();
    assert!(
        skipped.contains(&"package-lock.json") && skipped.contains(&".env"),
        "{skipped:?}"
    );
    assert_eq!(json["usage"]["input_tokens"], 300);

    // One request, holding the numbered diff and never the secret or the lock file.
    let seen = f.model_seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let body = &seen[0].1;
    assert!(body.contains("### calc.py"), "{body}");
    assert!(body.contains("4+     return a / 0"), "{body}");
    assert!(
        !body.contains("hunter2") && !body.contains("package-lock"),
        "{body}"
    );
    assert!(
        !body.contains("\"tools\":[{"),
        "the review offers the model no tools: {body}"
    );
}

#[test]
fn fail_on_turns_a_finding_into_a_failing_exit() {
    let f = fixture();
    let out = phazeai(&f, &["review", "--base", "main", "--fail-on", "high"], &[]);
    assert!(!out.status.success(), "{}", show(&out));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("calc.py:3 [high] Division by zero"),
        "{}",
        show(&out)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("high severity"),
        "{}",
        show(&out)
    );
}

#[test]
fn findings_are_posted_to_a_pull_request_as_one_review() {
    let f = fixture();
    let gh_seen: Seen = Arc::default();
    let api = serve(github, gh_seen.clone());
    let out = phazeai(
        &f,
        &[
            "review",
            "--base",
            "main",
            "--github-pr",
            "7",
            "--github-repo",
            "o/r",
        ],
        &[("GITHUB_TOKEN", "test-token"), ("GITHUB_API_URL", &api)],
    );
    assert!(out.status.success(), "{}", show(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("pullrequestreview-1"),
        "{}",
        show(&out)
    );
    let seen = gh_seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "POST /repos/o/r/pulls/7/reviews HTTP/1.1");
    let payload: serde_json::Value = serde_json::from_str(&seen[0].1).unwrap();
    assert_eq!(payload["event"], "COMMENT");
    assert_eq!(payload["comments"].as_array().unwrap().len(), 1);
    assert_eq!(payload["comments"][0]["path"], "calc.py");
    assert_eq!(payload["comments"][0]["line"], 3);
    assert_eq!(payload["comments"][0]["side"], "RIGHT");
    assert_eq!(payload["commit_id"].as_str().unwrap().len(), 40);
    drop(seen);

    // Without a token nothing is posted and the run fails.
    let out = phazeai(
        &f,
        &[
            "review",
            "--base",
            "main",
            "--github-pr",
            "7",
            "--github-repo",
            "o/r",
        ],
        &[],
    );
    assert!(!out.status.success(), "{}", show(&out));
    assert_eq!(gh_seen.lock().unwrap().len(), 1);
}

#[test]
fn uncommitted_changes_are_reviewed_when_asked() {
    let f = fixture();
    std::fs::write(
        f.repo.path().join("calc.py"),
        "def add(a, b):\n    return a - b\ndef ratio(a, b):\n    return a / 0\n",
    )
    .unwrap();
    let out = phazeai(
        &f,
        &["review", "--uncommitted", "--output-format", "json"],
        &[],
    );
    assert!(out.status.success(), "{}", show(&out));
    let body = f.model_seen.lock().unwrap()[0].1.clone();
    assert!(body.contains("2+     return a - b"), "{body}");
    assert!(
        !body.contains("4+ "),
        "only the uncommitted line is new: {body}"
    );
}
