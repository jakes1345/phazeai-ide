//! `phazeai worktree run` end to end: two real agents (this binary, talking
//! to a local OpenAI-compatible server) each write a file in their own git
//! worktree at the same time, the user's checkout stays untouched, and
//! `phazeai worktree merge` brings one agent's work in.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// A model that, asked to "Write ALPHA" or "Write BETA", calls `write_file`
/// once and then says it is done.
fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for sock in listener.incoming() {
            let Ok(mut sock) = sock else { return };
            std::thread::spawn(move || {
                let mut request = Vec::new();
                let mut chunk = [0u8; 16 * 1024];
                let body_start;
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
                            body_start = end + 4;
                            break;
                        }
                    }
                    if n == 0 {
                        body_start = request.len();
                        break;
                    }
                }
                let body = String::from_utf8_lossy(&request[body_start..]).into_owned();
                let events: Vec<serde_json::Value> = if body.contains("\"role\":\"tool\"") {
                    vec![serde_json::json!({"choices": [{"delta": {"content": "Done."}}]})]
                } else {
                    let (path, content) = if body.contains("ALPHA") {
                        ("alpha.txt", "from agent alpha\n")
                    } else {
                        ("beta.txt", "from agent beta\n")
                    };
                    let args = serde_json::json!({"path": path, "content": content}).to_string();
                    vec![
                        serde_json::json!({"choices": [{"delta": {"tool_calls": [{
                            "index": 0, "id": "call_1", "type": "function",
                            "function": {"name": "write_file", "arguments": args}
                        }]}}]}),
                        serde_json::json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
                    ]
                };
                let mut text = String::new();
                for e in events {
                    text.push_str(&format!("data: {e}\n\n"));
                }
                text.push_str("data: [DONE]\n\n");
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                    text.len()
                );
                let _ = sock.write_all(reply.as_bytes());
            });
        }
    });
    url
}

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8(out.stdout).unwrap()
}

struct Env {
    home: tempfile::TempDir,
    worktrees: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
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
        Env {
            home,
            worktrees: tempfile::tempdir().unwrap(),
        }
    }

    fn phazeai(&self, dir: &Path, args: &[&str]) -> Output {
        let home = self.home.path();
        Command::new(env!("CARGO_BIN_EXE_phazeai"))
            .args(args)
            .current_dir(dir)
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_CACHE_HOME", home.join("cache"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("PHAZEAI_WORKTREES_DIR", self.worktrees.path())
            .env_remove("PHAZEAI_TELEMETRY")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
}

fn text(out: &Output) -> String {
    format!(
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn parallel_agents_work_in_their_own_worktrees_and_merge_back() {
    let env = Env::new();
    let project = tempfile::tempdir().unwrap();
    let repo = project.path().canonicalize().unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "dev@example.com"]);
    git(&repo, &["config", "user.name", "Dev"]);
    std::fs::write(repo.join("README.md"), "# demo\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "first"]);

    let out = env.phazeai(
        &repo,
        &[
            "worktree",
            "run",
            "--task",
            "Write ALPHA",
            "--task",
            "Write BETA",
            "--jobs",
            "2",
        ],
    );
    assert!(out.status.success(), "{}", text(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("write-alpha [done"), "{}", text(&out));
    assert!(stdout.contains("alpha.txt"), "{}", text(&out));
    assert!(stdout.contains("beta.txt"), "{}", text(&out));

    // Each agent's file is on its own branch only; the checkout is untouched.
    assert!(!repo.join("alpha.txt").exists());
    assert!(git(&repo, &["status", "--porcelain"]).is_empty());
    let alpha = git(&repo, &["show", "phaze/write-alpha:alpha.txt"]);
    assert_eq!(alpha, "from agent alpha\n");
    let alpha_files = git(&repo, &["diff", "--name-only", "HEAD...phaze/write-alpha"]);
    assert_eq!(alpha_files.trim(), "alpha.txt");
    let beta_files = git(&repo, &["diff", "--name-only", "HEAD...phaze/write-beta"]);
    assert_eq!(beta_files.trim(), "beta.txt");

    let list = env.phazeai(&repo, &["worktree", "list"]);
    assert!(list.status.success(), "{}", text(&list));
    let listed = String::from_utf8_lossy(&list.stdout);
    assert!(
        listed.contains("write-alpha") && listed.contains("write-beta"),
        "{listed}"
    );

    let merge = env.phazeai(&repo, &["worktree", "merge", "write-alpha"]);
    assert!(merge.status.success(), "{}", text(&merge));
    assert_eq!(
        std::fs::read_to_string(repo.join("alpha.txt")).unwrap(),
        "from agent alpha\n"
    );
    assert!(!repo.join("beta.txt").exists());
    let branches = git(
        &repo,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/phaze/",
        ],
    );
    assert_eq!(branches.trim(), "phaze/write-beta");

    // Unmerged work isn't dropped without --force.
    let keep = env.phazeai(&repo, &["worktree", "remove", "write-beta"]);
    assert!(!keep.status.success(), "{}", text(&keep));
    let drop = env.phazeai(&repo, &["worktree", "remove", "write-beta", "--force"]);
    assert!(drop.status.success(), "{}", text(&drop));
    assert!(git(
        &repo,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/phaze/"
        ]
    )
    .trim()
    .is_empty());
}

#[test]
fn a_single_prompt_can_run_in_a_worktree() {
    let env = Env::new();
    let project = tempfile::tempdir().unwrap();
    let repo = project.path().canonicalize().unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "dev@example.com"]);
    git(&repo, &["config", "user.name", "Dev"]);
    std::fs::write(repo.join("README.md"), "# demo\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "first"]);

    let out = env.phazeai(
        &repo,
        &["-p", "Write ALPHA", "--allow", "edit", "--worktree", "solo"],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert!(!repo.join("alpha.txt").exists());
    assert_eq!(
        git(&repo, &["show", "phaze/solo:alpha.txt"]),
        "from agent alpha\n"
    );
    assert_eq!(
        git(&repo, &["log", "-1", "--format=%s", "phaze/solo"]).trim(),
        "Write ALPHA"
    );
}
