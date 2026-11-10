//! Token bench: what one fixed, representative coding task costs in input
//! tokens, measured on the requests the CLI agent really sends.
//!
//! The task is a two-message session on a small Rust service (the fixture in
//! `tests/fixtures/token_bench`, set up as a git repository): find and fix a
//! startup crash, add tests, then a follow-up change. The model's side is scripted, so every run makes
//! the same tool calls with the same arguments; everything Phaze controls
//! (system prompt, tool schemas, how history and tool results are sent) is
//! the real code. A local server stands in for an OpenAI-compatible provider
//! and keeps the body of every request.
//!
//! Each request is rendered the way a provider lays out a prompt (tool
//! schemas first, then the messages in order) and counted with the `o200k`
//! tokenizer. For each request it also counts the longest prefix shared, token
//! for token, with the request before it: the part a provider's prefix cache
//! can reuse. Totals are reported for both, plus an effective input cost that
//! bills the reusable part at a tenth of the price, which is what Anthropic,
//! OpenAI and DeepSeek charge for a cache read.
//!
//! The model's own replies are fixed by the script, so output tokens are the
//! same before and after any change here and are not reported.
//!
//! Run it with:
//!
//! ```sh
//! cargo test -p phazeai-cli --lib token_bench -- --ignored --nocapture
//! ```
//!
//! It is ignored by default because it sets process-wide state (the tools'
//! workspace root, the working directory and `HOME`, so no personal instructions file is read) and
//! because its output is a report, not a pass or fail.

use super::*;
use std::path::{Path, PathBuf};

const FIXTURE: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        include_str!("../../../tests/fixtures/token_bench/manifest.toml"),
    ),
    (
        "README.md",
        include_str!("../../../tests/fixtures/token_bench/README.md"),
    ),
    (
        "src/lib.rs",
        include_str!("../../../tests/fixtures/token_bench/src/lib.rs"),
    ),
    (
        "src/main.rs",
        include_str!("../../../tests/fixtures/token_bench/src/main.rs"),
    ),
    (
        "src/config.rs",
        include_str!("../../../tests/fixtures/token_bench/src/config.rs"),
    ),
    (
        "src/log.rs",
        include_str!("../../../tests/fixtures/token_bench/src/log.rs"),
    ),
    (
        "src/store.rs",
        include_str!("../../../tests/fixtures/token_bench/src/store.rs"),
    ),
    (
        "src/routes.rs",
        include_str!("../../../tests/fixtures/token_bench/src/routes.rs"),
    ),
    (
        "src/server.rs",
        include_str!("../../../tests/fixtures/token_bench/src/server.rs"),
    ),
    (
        "tests/config.rs",
        include_str!("../../../tests/fixtures/token_bench/tests/config.rs"),
    ),
];

const FIRST_ASK: &str = "The server crashes on startup when PORT is set but empty. \
                         Find the cause, fix it, and add a test for it.";
const FOLLOW_UP: &str = "Also log a warning when PORT is set but empty, so people know \
                         the default port was used.";

/// One scripted model turn: optional text, then tool calls (name, arguments).
struct Turn {
    text: &'static str,
    calls: Vec<(&'static str, Value)>,
}

fn turn(text: &'static str, calls: Vec<(&'static str, Value)>) -> Turn {
    Turn { text, calls }
}

/// The model's side of the session, in order. Paths are absolute inside the
/// fixture, as models write them once they have seen the project root.
fn script(root: &Path) -> Vec<Turn> {
    let p = |rel: &str| root.join(rel).to_string_lossy().into_owned();
    let old_port = "        let port = match lookup(\"PORT\") {\n            \
                    Some(raw) => raw.trim().parse::<u16>().unwrap(),\n            \
                    None => DEFAULT_PORT,\n        };\n";
    let new_port = "        let port = match lookup(\"PORT\") {\n            \
                    Some(raw) if !raw.trim().is_empty() => raw\n                \
                    .trim()\n                .parse::<u16>()\n                \
                    .map_err(|_| ConfigError::BadPort(raw.clone()))?,\n            \
                    _ => DEFAULT_PORT,\n        };\n";
    let old_test =
        "    let config = Config::from_lookup(env(&[(\"PORT\", \"9090\")])).unwrap();\n    \
                    assert_eq!(config.port, 9090);\n}\n";
    let new_test = format!(
        "{old_test}\n#[test]\nfn an_empty_port_uses_the_default() {{\n    \
         let config = Config::from_lookup(env(&[(\"PORT\", \"\")])).unwrap();\n    \
         assert_eq!(config.port, DEFAULT_PORT);\n}}\n\n#[test]\n\
         fn a_port_that_is_not_a_number_is_an_error() {{\n    \
         let err = Config::from_lookup(env(&[(\"PORT\", \"eighty\")])).unwrap_err();\n    \
         assert_eq!(err, ConfigError::BadPort(\"eighty\".into()));\n}}\n"
    );
    vec![
        // First ask.
        turn(
            "I'll look at the project layout first.",
            vec![
                ("list_files", json!({ "path": p(""), "recursive": true })),
                ("read_file", json!({ "path": p("README.md") })),
            ],
        ),
        turn(
            "",
            vec![("grep", json!({ "pattern": "PORT", "path": p("src") }))],
        ),
        turn(
            "",
            vec![
                ("read_file", json!({ "path": p("src/config.rs") })),
                ("read_file", json!({ "path": p("src/main.rs") })),
            ],
        ),
        turn(
            "`PORT` is parsed with `unwrap()`, so an empty value panics. I'll treat an \
             empty value as unset and report a bad one as a config error.",
            vec![(
                "edit_file",
                json!({ "path": p("src/config.rs"), "old_text": old_port, "new_text": new_port }),
            )],
        ),
        turn(
            "",
            vec![(
                "edit_file",
                json!({
                    "path": p("src/config.rs"),
                    "old_text": "    BadHost(String),\n",
                    "new_text": "    BadHost(String),\n    BadPort(String),\n",
                }),
            )],
        ),
        turn(
            "",
            vec![(
                "edit_file",
                json!({
                    "path": p("src/config.rs"),
                    "old_text": "            ConfigError::BadHost(v) => write!(f, \"HOST is not an IP address: {v:?}\"),\n",
                    "new_text": "            ConfigError::BadHost(v) => write!(f, \"HOST is not an IP address: {v:?}\"),\n            ConfigError::BadPort(v) => write!(f, \"PORT is not a port number: {v:?}\"),\n",
                }),
            )],
        ),
        turn(
            "",
            vec![("read_file", json!({ "path": p("tests/config.rs") }))],
        ),
        turn(
            "",
            vec![(
                "edit_file",
                json!({ "path": p("tests/config.rs"), "old_text": old_test, "new_text": new_test }),
            )],
        ),
        turn(
            "",
            vec![(
                "bash",
                json!({ "command": format!("grep -rn BadPort {} {}", p("src"), p("tests")) }),
            )],
        ),
        turn(
            "Fixed. An empty `PORT` now falls back to 8080, and a value that isn't a port \
             number is reported as a config error instead of a panic. Two tests cover both.",
            vec![],
        ),
        // Follow-up.
        turn(
            "",
            vec![("read_file", json!({ "path": p("src/config.rs") }))],
        ),
        turn(
            "",
            vec![(
                "grep",
                json!({ "pattern": "pub fn warn", "path": p("src") }),
            )],
        ),
        turn(
            "",
            vec![(
                "edit_file",
                json!({
                    "path": p("src/config.rs"),
                    "old_text": "            _ => DEFAULT_PORT,\n",
                    "new_text": "            Some(_) => {\n                crate::log::warn(&format!(\"PORT is empty; using the default {DEFAULT_PORT}\"));\n                DEFAULT_PORT\n            }\n            None => DEFAULT_PORT,\n",
                }),
            )],
        ),
        turn(
            "Done: an empty `PORT` now logs a warning naming the default port.",
            vec![],
        ),
    ]
}

/// The streamed reply for one scripted turn.
fn reply(turn: &Turn, call_offset: usize) -> String {
    let mut out = String::new();
    if !turn.text.is_empty() {
        out.push_str(&format!(
            "data: {}\n\n",
            json!({ "choices": [{ "delta": { "content": turn.text } }] })
        ));
    }
    for (i, (name, args)) in turn.calls.iter().enumerate() {
        let call = json!({
            "index": i,
            "id": format!("call_{}", call_offset + i + 1),
            "type": "function",
            "function": { "name": name, "arguments": args.to_string() },
        });
        out.push_str(&format!(
            "data: {}\n\n",
            json!({ "choices": [{ "delta": { "tool_calls": [call] } }] })
        ));
    }
    out.push_str("data: [DONE]\n\n");
    out
}

/// A request as a provider lays it out before tokenizing: every tool schema,
/// then each message with its role, text and tool calls.
fn render(body: &Value) -> String {
    let mut out = String::new();
    for tool in body["tools"].as_array().into_iter().flatten() {
        out.push_str("<tool>");
        out.push_str(&tool["function"].to_string());
        out.push('\n');
    }
    for message in body["messages"].as_array().into_iter().flatten() {
        out.push_str("<|");
        out.push_str(message["role"].as_str().unwrap_or_default());
        out.push_str("|>\n");
        if let Some(text) = message["content"].as_str() {
            out.push_str(text);
        }
        for call in message["tool_calls"].as_array().into_iter().flatten() {
            out.push_str("\n<call>");
            out.push_str(call["function"]["name"].as_str().unwrap_or_default());
            out.push(' ');
            out.push_str(call["function"]["arguments"].as_str().unwrap_or_default());
        }
        out.push('\n');
    }
    out
}

/// Restores an environment variable when dropped.
struct EnvGuard(&'static str, Option<std::ffi::OsString>);

impl EnvGuard {
    fn set(key: &'static str, value: &Path) -> Self {
        let old = std::env::var_os(key);
        std::env::set_var(key, value);
        EnvGuard(key, old)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.1 {
            Some(v) => std::env::set_var(self.0, v),
            None => std::env::remove_var(self.0),
        }
    }
}

/// Restores the working directory when dropped.
struct CwdGuard(PathBuf);

impl CwdGuard {
    fn enter(dir: &Path) -> Self {
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir).unwrap();
        CwdGuard(old)
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.0);
    }
}

/// A fresh copy of the fixture at a fixed path, so file paths (which appear
/// in the system prompt and in tool calls) and so token counts are the same
/// on every run on the same OS.
fn fresh_fixture() -> PathBuf {
    let base = std::env::temp_dir().join("phaze-token-bench");
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("tally");
    for (rel, text) in FIXTURE {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    std::fs::create_dir_all(base.join("home")).unwrap();
    // A git repository with everything committed, as a real project is: the
    // system prompt reports the branch and any uncommitted files.
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=bench", "-c", "user.email=bench@localhost"])
            .args([
                "-c",
                "init.defaultBranch=main",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            // A fixed date gives the same commit, so the same `.git`, every run.
            .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
            .current_dir(&root)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "tally 0.3.1"]);
    root
}

/// The same scripted turn as Anthropic streams it: a text block, then one
/// `tool_use` block per call.
fn claude_reply(turn: &Turn, call_offset: usize) -> String {
    let mut events = vec![json!({
        "type": "message_start",
        "message": { "usage": { "input_tokens": 0, "output_tokens": 0 } },
    })];
    let mut index = 0;
    if !turn.text.is_empty() {
        events.push(json!({ "type": "content_block_start", "index": index,
            "content_block": { "type": "text", "text": "" } }));
        events.push(json!({ "type": "content_block_delta", "index": index,
            "delta": { "type": "text_delta", "text": turn.text } }));
        events.push(json!({ "type": "content_block_stop", "index": index }));
        index += 1;
    }
    for (i, (name, args)) in turn.calls.iter().enumerate() {
        events.push(json!({ "type": "content_block_start", "index": index,
            "content_block": { "type": "tool_use", "id": format!("toolu_{}", call_offset + i + 1),
                "name": name, "input": {} } }));
        events.push(json!({ "type": "content_block_delta", "index": index,
            "delta": { "type": "input_json_delta", "partial_json": args.to_string() } }));
        events.push(json!({ "type": "content_block_stop", "index": index }));
        index += 1;
    }
    events.push(
        json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" },
        "usage": { "output_tokens": 0 } }),
    );
    events.push(json!({ "type": "message_stop" }));
    events
        .iter()
        .map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap()))
        .collect()
}

fn claude_client(url: &str) -> Box<dyn LlmClient> {
    Box::new(
        phazeai_core::llm::ClaudeClient::new("k")
            .with_base_url(url)
            .without_proxy(),
    )
}

/// An Anthropic request in render order (tools, system, messages) as a list
/// of blocks: the text each contributes, and whether it carries a
/// `cache_control` breakpoint.
fn claude_blocks(body: &Value) -> Vec<(String, bool)> {
    let marked = |v: &Value| v.get("cache_control").is_some();
    let mut out = Vec::new();
    for tool in body["tools"].as_array().into_iter().flatten() {
        let schema = json!({
            "name": tool["name"],
            "description": tool["description"],
            "input_schema": tool["input_schema"],
        });
        out.push((format!("<tool>{schema}\n"), marked(tool)));
    }
    for block in body["system"].as_array().into_iter().flatten() {
        out.push((
            format!(
                "<|system|>\n{}\n",
                block["text"].as_str().unwrap_or_default()
            ),
            marked(block),
        ));
    }
    if let Some(text) = body["system"].as_str() {
        out.push((format!("<|system|>\n{text}\n"), false));
    }
    for message in body["messages"].as_array().into_iter().flatten() {
        let role = message["role"].as_str().unwrap_or_default();
        if let Some(text) = message["content"].as_str() {
            out.push((format!("<|{role}|>\n{text}\n"), false));
            continue;
        }
        for (i, block) in message["content"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let head = if i == 0 {
                format!("<|{role}|>\n")
            } else {
                String::new()
            };
            let text = match block["type"].as_str() {
                Some("text") => block["text"].as_str().unwrap_or_default().to_string(),
                Some("tool_use") => format!(
                    "<call>{} {}",
                    block["name"].as_str().unwrap_or_default(),
                    block["input"]
                ),
                Some("tool_result") => block["content"].as_str().unwrap_or_default().to_string(),
                _ => block.to_string(),
            };
            out.push((format!("{head}{text}\n"), marked(block)));
        }
    }
    out
}

/// What Anthropic bills for one request, in input tokens.
#[derive(Default, Clone, Copy)]
struct Billed {
    total: usize,
    read: usize,
    written: usize,
    uncached: usize,
}

impl Billed {
    /// Input price units: a cache read costs 0.1x and a 5-minute write 1.25x.
    fn equivalent(&self) -> f64 {
        self.uncached as f64 + self.written as f64 * 1.25 + self.read as f64 * 0.1
    }
}

/// Anthropic's prompt cache, as its documentation describes it: an entry is
/// written at each breakpoint, holding the exact prefix up to it; a request
/// reads the longest stored prefix that ends at one of its block boundaries
/// within 20 blocks before one of its breakpoints. Tokens from there to the
/// last breakpoint are written, and tokens after it are plain input.
struct PromptCache {
    entries: std::collections::HashSet<Vec<u32>>,
}

impl PromptCache {
    fn bill(&mut self, bpe: &tiktoken_rs::CoreBPE, body: &Value) -> Billed {
        let mut tokens: Vec<u32> = Vec::new();
        let mut ends = Vec::new();
        let mut marks = Vec::new();
        for (i, (text, marked)) in claude_blocks(body).into_iter().enumerate() {
            tokens.extend(bpe.encode_ordinary(&text));
            ends.push(tokens.len());
            if marked {
                marks.push(i);
            }
        }
        let mut read = 0;
        for &mark in &marks {
            for k in (mark.saturating_sub(19)..=mark).rev() {
                if self.entries.contains(&tokens[..ends[k]]) {
                    read = read.max(ends[k]);
                    break;
                }
            }
        }
        let last_mark = marks.last().map(|&m| ends[m]).unwrap_or(0);
        for &mark in &marks {
            self.entries.insert(tokens[..ends[mark]].to_vec());
        }
        Billed {
            total: tokens.len(),
            read,
            written: last_mark.saturating_sub(read),
            uncached: tokens.len() - last_mark.max(read),
        }
    }
}

/// One measured session pair on a fresh copy of the fixture: the two-message
/// session, then a new session's first message on the project as the first
/// left it apart from `src/config.rs`. Returns the first session's runs and
/// the request bodies of both sessions.
async fn capture(
    settings: &Settings,
    make_client: fn(&str) -> Box<dyn LlmClient>,
    replies: Vec<String>,
    next_session_replies: Vec<String>,
) -> (Vec<phazeai_core::AgentResponse>, Vec<Value>, Vec<Value>) {
    let root = fresh_fixture();
    // The CLI runs in the project it works on: shell commands and code search
    // start from the working directory.
    let _cwd = CwdGuard::enter(&root);
    phazeai_core::tools::sandbox::set_workspace_root(Some(root.clone()));

    let (url, bodies) = fake_provider(replies).await;
    let agent = single_prompt_agent(settings, make_client(&url), &root, None);
    let first = agent.run(FIRST_ASK).await.unwrap();
    let second = agent.run(FOLLOW_UP).await.unwrap();

    // A second session started the same way (a new `phazeai -p`, or the IDE
    // chat, which builds a new agent for every message), on the project as the
    // first session left it except for `src/config.rs`, so its first request
    // is the first session's apart from what changed in the project. What
    // the two share is what a provider can still serve from cache across
    // sessions.
    let (url2, bodies2) = fake_provider(next_session_replies).await;
    std::fs::write(root.join("src/config.rs"), FIXTURE[4].1).unwrap();
    let again = single_prompt_agent(settings, make_client(&url2), &root, None);
    again.run(FIRST_ASK).await.unwrap();
    phazeai_core::tools::sandbox::set_workspace_root(None);

    let sent = bodies.lock().unwrap().clone();
    let next = bodies2.lock().unwrap().clone();
    (vec![first, second], sent, next)
}

/// The script ran as written: every call succeeded and the fix landed.
fn check_script_ran(runs: &[phazeai_core::AgentResponse]) {
    for exec in runs.iter().flat_map(|r| &r.tool_calls) {
        assert!(
            exec.success,
            "{} failed: {}",
            exec.tool_name, exec.result_summary
        );
    }
    let fixed = runs.last().unwrap().tool_calls.last().unwrap();
    assert_eq!(fixed.tool_name, "edit_file");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "report, not a check; sets process-wide state (run with --ignored)"]
async fn token_bench() {
    // No personal instructions file is read: `HOME` is an empty directory
    // beside the fixture (which `fresh_fixture` recreates).
    let _home = EnvGuard::set(
        "HOME",
        &std::env::temp_dir().join("phaze-token-bench").join("home"),
    );

    let turns = script(&fresh_fixture());
    let scripted = |render: fn(&Turn, usize) -> String| -> Vec<String> {
        let mut offset = 0;
        turns
            .iter()
            .map(|t| {
                let r = render(t, offset);
                offset += t.calls.len();
                r
            })
            .collect()
    };

    let mut settings = Settings::default();
    settings.llm.provider = phazeai_core::config::LlmProvider::OpenAI;
    settings.llm.model = "gpt-4.1".into();
    let (runs, sent, next) = capture(&settings, fake_client, scripted(reply), vec![]).await;
    check_script_ran(&runs);
    assert_eq!(sent.len(), turns.len(), "one request per scripted turn");

    let bpe = tiktoken_rs::o200k_base().unwrap();
    let first_body = &sent[0];
    let tools_tokens: usize = first_body["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            bpe.encode_ordinary(&format!("<tool>{}\n", t["function"]))
                .len()
        })
        .sum();
    let system_tokens = bpe
        .encode_ordinary(
            first_body["messages"][0]["content"]
                .as_str()
                .unwrap_or_default(),
        )
        .len();
    let tool_names: Vec<&str> = first_body["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();

    println!("\n## Token bench\n");
    println!(
        "{} requests, {} tools offered ({tools_tokens} tokens of schemas), \
         system prompt {system_tokens} tokens.\n",
        sent.len(),
        tool_names.len()
    );
    println!("| # | input tokens | reusable prefix | new tokens |");
    println!("|--:|--:|--:|--:|");
    let mut previous: Vec<u32> = Vec::new();
    let (mut total, mut reusable) = (0usize, 0usize);
    for (i, body) in sent.iter().enumerate() {
        let tokens: Vec<u32> = bpe.encode_ordinary(&render(body)).into_iter().collect();
        let shared = tokens
            .iter()
            .zip(&previous)
            .take_while(|(a, b)| a == b)
            .count();
        total += tokens.len();
        reusable += shared;
        println!(
            "| {} | {} | {} | {} |",
            i + 1,
            tokens.len(),
            shared,
            tokens.len() - shared
        );
        previous = tokens;
    }
    let effective = (total - reusable) as f64 + reusable as f64 * 0.1;
    println!("\n- **Input tokens sent:** {total}");
    println!(
        "- **Reusable by a prefix cache:** {reusable} ({:.1}%)",
        100.0 * reusable as f64 / total as f64
    );
    println!(
        "- **Effective input, reusable part at 0.1x:** {effective:.0} ({:.1}% of sent)",
        100.0 * effective / total as f64
    );
    let new_session = bpe.encode_ordinary(&render(&next[0]));
    let first_tokens = bpe.encode_ordinary(&render(&sent[0]));
    let across = new_session
        .iter()
        .zip(&first_tokens)
        .take_while(|(a, b)| a == b)
        .count();
    println!(
        "- **A new session's first request shares:** {across} of {} tokens with this one's",
        new_session.len()
    );
    let mut per_tool: Vec<(usize, &str)> = first_body["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            (
                bpe.encode_ordinary(&format!("<tool>{}\n", t["function"]))
                    .len(),
                t["function"]["name"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    per_tool.sort_by(|a, b| b.cmp(a));
    let per_tool: Vec<String> = per_tool
        .iter()
        .map(|(n, name)| format!("{name} {n}"))
        .collect();
    println!("- **Tools offered, tokens each:** {}", per_tool.join(", "));

    // The same task with Claude: what Anthropic bills, given the cache
    // breakpoints the requests really carry. Without breakpoints it bills
    // every input token at full price.
    let mut settings = Settings::default();
    settings.llm.provider = phazeai_core::config::LlmProvider::Claude;
    settings.llm.model = "claude-sonnet-4-6".into();
    let closing = vec![claude_reply(&turn("Done.", vec![]), 0)];
    let (runs, sent, next) =
        capture(&settings, claude_client, scripted(claude_reply), closing).await;
    check_script_ran(&runs);
    assert_eq!(sent.len(), turns.len(), "one request per scripted turn");

    let mut cache = PromptCache {
        entries: Default::default(),
    };
    println!("\n### Claude, as Anthropic bills it\n");
    println!("| # | input tokens | cache read | cache write | uncached |");
    println!("|--:|--:|--:|--:|--:|");
    let mut sum = Billed::default();
    for (i, body) in sent.iter().enumerate() {
        let b = cache.bill(&bpe, body);
        println!(
            "| {} | {} | {} | {} | {} |",
            i + 1,
            b.total,
            b.read,
            b.written,
            b.uncached
        );
        sum.total += b.total;
        sum.read += b.read;
        sum.written += b.written;
        sum.uncached += b.uncached;
    }
    println!("\n- **Input tokens sent:** {}", sum.total);
    println!(
        "- **Read from cache:** {} · **written:** {} · **uncached:** {}",
        sum.read, sum.written, sum.uncached
    );
    println!(
        "- **Billed, in input-token units (read 0.1x, write 1.25x):** {:.0} ({:.1}% of sent)",
        sum.equivalent(),
        100.0 * sum.equivalent() / sum.total as f64
    );
    let b = cache.bill(&bpe, &next[0]);
    println!(
        "- **A new session's first request reads:** {} of {} tokens from cache",
        b.read, b.total
    );
}

/// The questions of the long session, one per user message.
const LONG_ASKS: &[&str] = &[
    "Walk me through how a request reaches the store.",
    "Where is the port read, and what happens when it is missing?",
    "How does logging work, and where is it called from?",
    "Which routes exist, and which of them write to the store?",
    "What do the tests cover today, and what is missing?",
    "How does the server shut down, and could it lose writes?",
    "Summarise the main types and who owns them.",
    "Which functions are public, and which of them have no test?",
];

/// The model's side of the long session: for each question, two files
/// read together, a search, a third file, then the answer. The files rotate
/// through the project, so the conversation keeps growing until it has to be
/// compacted.
fn long_script(root: &Path) -> Vec<Turn> {
    let p = |rel: &str| root.join(rel).display().to_string();
    let files = [
        "src/config.rs",
        "src/server.rs",
        "src/routes.rs",
        "src/store.rs",
        "src/log.rs",
        "src/main.rs",
        "tests/config.rs",
        "README.md",
        "src/lib.rs",
    ];
    let searches = [
        "pub fn",
        "impl ",
        "use ",
        "struct ",
        "#\\[test\\]",
        "fn main",
        "Store",
        "warn",
    ];
    let mut turns = Vec::new();
    for (k, _) in LONG_ASKS.iter().enumerate() {
        let f = |i: usize| p(files[(k * 3 + i) % files.len()]);
        turns.push(turn(
            "",
            vec![
                ("read_file", json!({ "path": f(0) })),
                ("read_file", json!({ "path": f(1) })),
            ],
        ));
        turns.push(turn(
            "",
            vec![(
                "grep",
                json!({ "pattern": searches[k % searches.len()], "path": p("src") }),
            )],
        ));
        turns.push(turn("", vec![("read_file", json!({ "path": f(2) }))]));
        turns.push(turn(
            "Here is what I found: the code paths are as described in the files above.",
            vec![],
        ));
    }
    turns
}

/// The summary the model writes when the conversation is compacted, as a
/// whole (not streamed) Anthropic response. Its length is fixed so every
/// run bills the same.
fn claude_summary() -> String {
    let text = "**Goal**: answer questions about the tally service.\n\
                **Work done**: read the config, server, routes, store and log modules; \
                searched for public functions, impls, structs and tests.\n\
                **Findings**: requests go from routes to the store; the port comes \
                from PORT with a default; logging is a thin wrapper used by config.\n\
                **Open items**: none.";
    json!({
        "id": "msg_summary",
        "type": "message",
        "role": "assistant",
        "model": "claude-sonnet-4-6",
        "content": [{ "type": "text", "text": text }],
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 0, "output_tokens": 0 },
    })
    .to_string()
}

/// A long session with a small context budget, so it has to be compacted
/// more than once: what it sends, and what Anthropic bills for it given the
/// cache breakpoints the requests carry. Compaction and trimming are where a
/// long session loses its cache, which the short bench never reaches.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "report, not a check; sets process-wide state (run with --ignored)"]
async fn long_session_bench() {
    let _home = EnvGuard::set(
        "HOME",
        &std::env::temp_dir().join("phaze-token-bench").join("home"),
    );
    let root = fresh_fixture();
    let turns = long_script(&root);
    let mut offset = 0;
    let replies: Vec<String> = turns
        .iter()
        .map(|t| {
            let r = claude_reply(t, offset);
            offset += t.calls.len();
            r
        })
        .collect();

    let mut settings = Settings::default();
    settings.llm.provider = phazeai_core::config::LlmProvider::Claude;
    settings.llm.model = "claude-sonnet-4-6".into();
    settings.llm.context_tokens = Some(BUDGET);
    const BUDGET: usize = 8_000;

    let _cwd = CwdGuard::enter(&root);
    phazeai_core::tools::sandbox::set_workspace_root(Some(root.clone()));
    let (url, bodies) = fake_provider_answering(replies, Some(claude_summary())).await;
    let agent = single_prompt_agent(&settings, claude_client(&url), &root, None);
    let mut runs = Vec::new();
    for ask in LONG_ASKS {
        runs.push(agent.run(*ask).await.unwrap());
    }
    phazeai_core::tools::sandbox::set_workspace_root(None);
    check_long_script_ran(&runs);

    let sent = bodies.lock().unwrap().clone();
    let compactions = sent.iter().filter(|b| b["stream"] != json!(true)).count();
    let bpe = tiktoken_rs::o200k_base().unwrap();
    let mut cache = PromptCache {
        entries: Default::default(),
    };
    println!("\n## Long session bench\n");
    println!(
        "{} questions, {} requests ({} of them compaction summaries), context budget {BUDGET} tokens.\n",
        LONG_ASKS.len(),
        sent.len(),
        compactions
    );
    println!("| # | kind | input tokens | cache read | cache write | uncached |");
    println!("|--:|--|--:|--:|--:|--:|");
    let mut sum = Billed::default();
    let mut summaries = Billed::default();
    for (i, body) in sent.iter().enumerate() {
        let b = cache.bill(&bpe, body);
        let kind = if body["stream"] == json!(true) {
            "turn"
        } else {
            "summary"
        };
        println!(
            "| {} | {kind} | {} | {} | {} | {} |",
            i + 1,
            b.total,
            b.read,
            b.written,
            b.uncached
        );
        for s in [&mut sum]
            .into_iter()
            .chain((kind == "summary").then_some(&mut summaries))
        {
            s.total += b.total;
            s.read += b.read;
            s.written += b.written;
            s.uncached += b.uncached;
        }
    }
    println!("\n- **Input tokens sent:** {}", sum.total);
    println!(
        "- **Read from cache:** {} · **written:** {} · **uncached:** {}",
        sum.read, sum.written, sum.uncached
    );
    println!(
        "- **Billed, in input-token units (read 0.1x, write 1.25x):** {:.0} ({:.1}% of sent)",
        sum.equivalent(),
        100.0 * sum.equivalent() / sum.total as f64
    );
    println!(
        "- **Of which compaction summaries:** {} sent, billed {:.0}",
        summaries.total,
        summaries.equivalent()
    );
}

/// Every scripted call of the long session succeeded.
fn check_long_script_ran(runs: &[phazeai_core::AgentResponse]) {
    for exec in runs.iter().flat_map(|r| &r.tool_calls) {
        assert!(
            exec.success,
            "{} failed: {}",
            exec.tool_name, exec.result_summary
        );
    }
    assert_eq!(runs.len(), LONG_ASKS.len());
}
