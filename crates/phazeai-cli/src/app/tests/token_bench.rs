//! Token bench: what one fixed, representative coding task costs in input
//! tokens, measured on the requests the CLI agent really sends.
//!
//! The task is a two-message session on a small Rust service (the fixture in
//! `tests/fixtures/token_bench`): find and fix a startup crash, add tests,
//! then a follow-up change. The model's side is scripted, so every run makes
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
    root
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "report, not a check; sets process-wide state (run with --ignored)"]
async fn token_bench() {
    let root = fresh_fixture();
    let _home = EnvGuard::set("HOME", &root.parent().unwrap().join("home"));
    // The CLI runs in the project it works on: shell commands and code search
    // start from the working directory.
    let _cwd = CwdGuard::enter(&root);
    phazeai_core::tools::sandbox::set_workspace_root(Some(root.clone()));

    let turns = script(&root);
    let mut offset = 0;
    let replies = turns
        .iter()
        .map(|t| {
            let r = reply(t, offset);
            offset += t.calls.len();
            r
        })
        .collect();
    let (url, bodies) = fake_provider(replies).await;

    let mut settings = Settings::default();
    settings.llm.provider = phazeai_core::config::LlmProvider::OpenAI;
    settings.llm.model = "gpt-4.1".into();
    let agent = single_prompt_agent(&settings, fake_client(&url), &root, None);

    let first = agent.run(FIRST_ASK).await.unwrap();
    let second = agent.run(FOLLOW_UP).await.unwrap();

    // A second session started the same way (a new `phazeai -p`, or the IDE
    // chat, which builds a new agent for every message). What its first
    // request shares with the first session's is what a provider can still
    // serve from cache across sessions.
    let (url2, bodies2) = fake_provider(vec![]).await;
    std::fs::write(root.join("src/config.rs"), FIXTURE[4].1).unwrap();
    let again = single_prompt_agent(&settings, fake_client(&url2), &root, None);
    again.run(FIRST_ASK).await.unwrap();
    phazeai_core::tools::sandbox::set_workspace_root(None);

    // The script ran as written: every call succeeded and the fix landed.
    for exec in first.tool_calls.iter().chain(&second.tool_calls) {
        assert!(
            exec.success,
            "{} failed: {}",
            exec.tool_name, exec.result_summary
        );
    }
    let fixed = second.tool_calls.last().unwrap();
    assert_eq!(fixed.tool_name, "edit_file");

    let sent = bodies.lock().unwrap().clone();
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
    let new_session = bpe.encode_ordinary(&render(&bodies2.lock().unwrap()[0]));
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
    println!("- **Tools offered:** {}", tool_names.join(", "));
}
