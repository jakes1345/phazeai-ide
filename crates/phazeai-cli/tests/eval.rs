//! `phazeai eval` end to end: tasks are copied, agents run in their own
//! folder without the hidden tests, results are graded and summed, and the
//! tasks shipped in `evals/` are sound.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Stdio};

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A task: `answer.txt` starts as "wrong"; the hidden check wants `want`.
fn task(root: &Path, name: &str, want: &str) {
    let dir = root.join(name);
    write(
        &dir.join("task.toml"),
        "prompt = \"Put the right answer in answer.txt\"\ncheck = \"sh verify.sh\"\ntimeout_secs = 60\n",
    );
    write(&dir.join("repo/answer.txt"), "wrong\n");
    write(
        &dir.join("check/verify.sh"),
        &format!("grep -qx '{want}' answer.txt || {{ echo \"answer.txt says $(cat answer.txt)\"; exit 1; }}\n"),
    );
}

fn eval(args: &[&str]) -> (String, String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_phazeai"))
        .arg("eval")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
        out.status.success(),
    )
}

#[test]
fn a_shell_agent_is_graded_by_hidden_tests_and_its_tokens_are_summed() {
    let tasks = tempfile::tempdir().unwrap();
    task(tasks.path(), "gets-it-right", "right");
    task(tasks.path(), "wants-something-else", "other");
    let results = tasks.path().join("results.jsonl");

    // The agent can't see verify.sh, works in a git repository, gets the
    // prompt from the environment, and prints a Claude Code-style result.
    let agent = r#"test ! -e verify.sh && git rev-parse --git-dir >/dev/null && [ -n "$PHAZE_EVAL_PROMPT" ] && echo right > answer.txt; echo '{"type":"result","is_error":false,"num_turns":3,"usage":{"input_tokens":1500,"cache_read_input_tokens":20000,"cache_creation_input_tokens":0,"output_tokens":400},"total_cost_usd":0.01}'"#;
    let (stdout, stderr, ok) = eval(&[
        tasks.path().to_str().unwrap(),
        "--agent",
        agent,
        "--out",
        results.to_str().unwrap(),
    ]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("Solved 1 of 2 runs (50%)."), "{stdout}");
    assert!(
        stdout.contains("Spent $0.0200, $0.0200 per solved task."),
        "{stdout}"
    );
    assert!(
        stdout.contains("1,500") && stdout.contains("20,000"),
        "{stdout}"
    );

    let lines: Vec<serde_json::Value> = std::fs::read_to_string(&results)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["task"], "gets-it-right");
    assert_eq!(lines[0]["solved"], true);
    assert_eq!(lines[0]["turns"], 3);
    assert_eq!(lines[0]["cache_read_tokens"], 20000);
    assert_eq!(lines[1]["solved"], false);
    assert_eq!(lines[1]["check_output"], "answer.txt says right");
}

#[test]
fn only_the_named_tasks_run_as_often_as_asked() {
    let tasks = tempfile::tempdir().unwrap();
    task(tasks.path(), "alpha", "right");
    task(tasks.path(), "beta", "right");
    let (stdout, stderr, ok) = eval(&[
        tasks.path().to_str().unwrap(),
        "--task",
        "beta",
        "--runs",
        "2",
        "--agent",
        "echo right > answer.txt",
    ]);
    assert!(ok, "{stderr}");
    assert!(
        stdout.contains("beta") && !stdout.contains("alpha"),
        "{stdout}"
    );
    assert!(stdout.contains("2/2"), "{stdout}");
    // No JSON result: graded all the same, with tokens and cost unknown.
    assert!(stdout.contains("Solved 2 of 2 runs (100%).\n"), "{stdout}");
}

#[test]
fn a_task_that_already_passes_is_reported_broken() {
    let tasks = tempfile::tempdir().unwrap();
    task(tasks.path(), "too-easy", "wrong");
    let (stdout, _, ok) = eval(&[tasks.path().to_str().unwrap(), "--verify"]);
    assert!(!ok);
    assert!(
        stdout.contains("BROKEN  too-easy: the check already passes"),
        "{stdout}"
    );
}

/// A provider that answers every chat request with text only, so the
/// Phaze agent finishes without touching the task.
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
            let text = serde_json::json!({"choices": [{"delta": {"content": "I can't."}}]});
            let usage = serde_json::json!({
                "choices": [],
                "usage": {"prompt_tokens": 900, "completion_tokens": 4,
                          "prompt_tokens_details": {"cached_tokens": 600}}
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

#[test]
fn the_phaze_agent_runs_this_program_and_reports_its_tokens() {
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

    let tasks = tempfile::tempdir().unwrap();
    task(tasks.path(), "needs-work", "right");
    let results = tasks.path().join("results.jsonl");
    let out = Command::new(env!("CARGO_BIN_EXE_phazeai"))
        .args(["eval", tasks.path().to_str().unwrap()])
        .args(["--out", results.to_str().unwrap()])
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .env("XDG_CACHE_HOME", home.path().join("cache"))
        .env("XDG_DATA_HOME", home.path().join("data"))
        .env_remove("PHAZEAI_TELEMETRY")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("Solved 0 of 1 runs (0%)."), "{stdout}");
    let line: serde_json::Value =
        serde_json::from_str(std::fs::read_to_string(&results).unwrap().trim()).unwrap();
    assert_eq!(line["agent"], "phaze");
    assert_eq!(line["agent_exit"], 0);
    assert_eq!(line["turns"], 1);
    assert_eq!(line["input_tokens"], 300);
    assert_eq!(line["cache_read_tokens"], 600);
    assert_eq!(line["output_tokens"], 4);
}

/// Every task in `evals/` fails as given and passes with its solution, so a
/// low score is the agent's and not a broken task's. Needs git, python3,
/// node and cargo, as CI has.
#[test]
fn the_shipped_tasks_are_sound() {
    let evals = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals");
    let (stdout, stderr, ok) = eval(&[evals.to_str().unwrap(), "--verify"]);
    assert!(ok, "{stdout}{stderr}");
    assert_eq!(stdout.lines().filter(|l| l.starts_with("ok ")).count(), 6);
}
