//! Permission rules and hooks around the agent's tool calls: a denied call
//! never reaches the approval callback, an allowed one skips it, an `ask` one
//! reaches it flagged, and the user's hooks run before and after calls.
//! Helpers follow the same rules.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};

use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use phazeai_core::hooks::{HookEntry, HookSettings};
use phazeai_core::permissions::{rule_says_ask, PermissionSettings};
use phazeai_core::{
    Agent, ApprovalFn, LlmClient, LlmResponse, Message, Permissions, PhazeError, StreamEvent,
    SubagentConfig, ToolDefinition,
};
use serde_json::{json, Value};

fn call(id: &str, name: &str, args: Value) -> Vec<StreamEvent> {
    vec![
        StreamEvent::ToolCallStart {
            id: id.into(),
            name: name.into(),
        },
        StreamEvent::ToolCallDelta {
            id: id.into(),
            arguments_delta: args.to_string(),
        },
        StreamEvent::ToolCallEnd { id: id.into() },
    ]
}

fn send(events: Vec<StreamEvent>) -> UnboundedReceiver<StreamEvent> {
    let (tx, rx) = unbounded();
    for event in events {
        tx.unbounded_send(event).unwrap();
    }
    rx
}

/// Plays its replies in order and keeps every request.
struct Scripted {
    replies: Mutex<VecDeque<Vec<StreamEvent>>>,
    seen: Arc<Mutex<Vec<Vec<Message>>>>,
}

impl Scripted {
    fn new(replies: Vec<Vec<StreamEvent>>) -> (Self, Arc<Mutex<Vec<Vec<Message>>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                replies: Mutex::new(replies.into()),
                seen: seen.clone(),
            },
            seen,
        )
    }
}

#[async_trait::async_trait]
impl LlmClient for Scripted {
    async fn chat(&self, _: &[Message], _: &[ToolDefinition]) -> Result<LlmResponse, PhazeError> {
        Ok(LlmResponse {
            message: Message::assistant("summary"),
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> Result<UnboundedReceiver<StreamEvent>, PhazeError> {
        self.seen.lock().unwrap().push(messages.to_vec());
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| vec![StreamEvent::TextDelta("done".into()), StreamEvent::Done]);
        Ok(send(reply))
    }
}

/// One model turn asking for these calls, then a final answer.
fn turns(calls: Vec<(&str, &str, Value)>) -> Vec<Vec<StreamEvent>> {
    let mut first = Vec::new();
    for (id, name, args) in calls {
        first.extend(call(id, name, args));
    }
    first.push(StreamEvent::Done);
    vec![
        first,
        vec![StreamEvent::TextDelta("done".into()), StreamEvent::Done],
    ]
}

/// The tool results the model got back, in call order.
fn results(seen: &Arc<Mutex<Vec<Vec<Message>>>>) -> Vec<String> {
    seen.lock().unwrap()[1]
        .iter()
        .filter(|m| m.tool_call_id.is_some())
        .map(|m| m.content.clone())
        .collect()
}

fn rules(root: &Path, allow: &[&str], ask: &[&str], deny: &[&str]) -> Arc<Permissions> {
    let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect();
    Arc::new(Permissions::from_rules(
        &PermissionSettings {
            allow: s(allow),
            ask: s(ask),
            deny: s(deny),
        },
        Some(root),
    ))
}

/// An approval callback that says yes and records each call it was asked
/// about, with whether a rule said to ask.
/// Each call an approval callback was asked about, and whether a rule said
/// to ask.
type Asked = Arc<Mutex<Vec<(String, bool)>>>;

fn recording_approval() -> (ApprovalFn, Asked) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let log = asked.clone();
    let approval: ApprovalFn = Box::new(move |name, _| {
        log.lock().unwrap().push((name, rule_says_ask()));
        Box::pin(async { true })
    });
    (approval, asked)
}

#[tokio::test]
async fn deny_skips_the_question_allow_skips_it_too_and_ask_is_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("notes.md"), "hello\n").unwrap();
    std::fs::write(root.join("server.pem"), "key\n").unwrap();
    let (llm, seen) = Scripted::new(turns(vec![
        ("c1", "read_file", json!({"path": root.join("server.pem")})),
        ("c2", "read_file", json!({"path": root.join("notes.md")})),
        ("c3", "list_files", json!({"path": root})),
        ("c4", "glob", json!({"pattern": "*.md", "path": root})),
    ]));
    let (approval, asked) = recording_approval();
    let agent = Agent::new(Box::new(llm))
        .with_approval(approval)
        .with_permissions(rules(
            &root,
            &["read_file(*.md)"],
            &["list_files"],
            &["read_file(*.pem)"],
        ));
    let response = agent.run("look around").await.unwrap();
    assert_eq!(response.content, "done");

    let results = results(&seen);
    assert!(
        results[0].starts_with("Error: Blocked by the permission rule `read_file(*.pem)`"),
        "{}",
        results[0]
    );
    assert!(!results[0].contains("key"));
    assert!(results[1].contains("hello"), "{}", results[1]);
    // Only the calls no deny or allow rule settled were asked about, and the
    // one an ask rule matched was flagged.
    assert_eq!(
        *asked.lock().unwrap(),
        [
            ("list_files".to_string(), true),
            ("glob".to_string(), false)
        ]
    );
}

#[tokio::test]
async fn bash_runs_unasked_only_when_every_part_is_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let made = root.join("made");
    let sneaky = root.join("sneaky");
    let second = format!("echo three; touch {}", made.display());
    let (llm, seen) = Scripted::new(turns(vec![
        ("c1", "bash", json!({"command": "echo one && echo two"})),
        ("c2", "bash", json!({ "command": second })),
        (
            "c3",
            "bash",
            json!({"command": format!("echo $(touch {})", sneaky.display())}),
        ),
    ]));
    let asked = Arc::new(Mutex::new(Vec::new()));
    let log = asked.clone();
    // Says no to everything it is asked.
    let approval: ApprovalFn = Box::new(move |_, params| {
        log.lock()
            .unwrap()
            .push(params["command"].as_str().unwrap().to_string());
        Box::pin(async { false })
    });
    let agent = Agent::new(Box::new(llm))
        .with_approval(approval)
        .with_permissions(rules(
            &root,
            &["bash(echo *)"],
            &[],
            &["bash(touch *sneaky)"],
        ));
    agent.run("go").await.unwrap();
    let results = results(&seen);
    assert!(
        results[0].contains("one") && results[0].contains("two"),
        "{}",
        results[0]
    );
    assert_eq!(results[1], "Error: Tool execution denied by user");
    assert!(
        results[2].contains("Blocked by the permission rule `bash(touch *sneaky)`"),
        "{}",
        results[2]
    );
    assert_eq!(*asked.lock().unwrap(), [second]);
    assert!(!made.exists() && !sneaky.exists());
}

#[tokio::test]
async fn an_ask_rule_with_no_one_to_ask_does_not_run() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("a.txt"), "secret-ish\n").unwrap();
    let (llm, seen) = Scripted::new(turns(vec![(
        "c1",
        "read_file",
        json!({"path": root.join("a.txt")}),
    )]));
    let agent =
        Agent::new(Box::new(llm)).with_permissions(rules(&root, &[], &["read_file(a.txt)"], &[]));
    agent.run("go").await.unwrap();
    let results = results(&seen);
    assert!(
        results[0].contains("needs the user's approval") && !results[0].contains("secret-ish"),
        "{}",
        results[0]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn hooks_can_stop_a_call_and_speak_after_one() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("a.rs"), "fn main() {}\n").unwrap();
    std::fs::write(root.join("b.md"), "# b\n").unwrap();
    let hooks = HookSettings {
        pre_tool: vec![
            HookEntry {
                tools: vec!["read_file(*.md)".into()],
                command: "echo 'markdown is off limits today' >&2; exit 2".into(),
                timeout_secs: Some(10),
            },
            HookEntry {
                tools: vec!["read_file".into()],
                command: "exit 5".into(),
                timeout_secs: Some(10),
            },
        ],
        post_tool: vec![HookEntry {
            tools: vec!["read_file(*.rs)".into()],
            command: "grep -q 'fn main' && echo 'lint: add docs to main' >&2; exit 2".into(),
            timeout_secs: Some(10),
        }],
    };
    let permissions = Arc::new(Permissions::load(
        &PermissionSettings::default(),
        &hooks,
        Some(&root),
    ));
    assert!(
        permissions.warnings().is_empty(),
        "{:?}",
        permissions.warnings()
    );
    let (llm, seen) = Scripted::new(turns(vec![
        ("c1", "read_file", json!({"path": root.join("b.md")})),
        ("c2", "read_file", json!({"path": root.join("a.rs")})),
    ]));
    let agent = Agent::new(Box::new(llm)).with_permissions(permissions);
    agent.run("go").await.unwrap();
    let results = results(&seen);
    assert!(
        results[0].starts_with("Error: Blocked by the user's hook")
            && results[0].ends_with("The hook says: markdown is off limits today"),
        "{}",
        results[0]
    );
    let read = &results[1];
    assert!(read.contains("fn main() {}"), "{read}");
    assert!(read.contains("[the pre_tool hook `exit 5` exited 5; the call ran anyway]"));
    assert!(read.contains("says:]\nlint: add docs to main"), "{read}");
}

/// A helper model that reads the path it is asked about and answers with
/// what the read returned.
struct Reader;

#[async_trait::async_trait]
impl LlmClient for Reader {
    async fn chat(&self, _: &[Message], _: &[ToolDefinition]) -> Result<LlmResponse, PhazeError> {
        Ok(LlmResponse {
            message: Message::assistant("summary"),
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> Result<UnboundedReceiver<StreamEvent>, PhazeError> {
        if let Some(result) = messages.iter().rev().find(|m| m.tool_call_id.is_some()) {
            return Ok(send(vec![
                StreamEvent::TextDelta(format!("read gave: {}", result.content)),
                StreamEvent::Done,
            ]));
        }
        let path = messages.last().unwrap().content.clone();
        let mut reply = call("r1", "read_file", json!({ "path": path }));
        reply.push(StreamEvent::Done);
        Ok(send(reply))
    }
}

#[tokio::test]
async fn helpers_follow_the_same_rules() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("private")).unwrap();
    std::fs::write(root.join("private/plan.txt"), "the plan\n").unwrap();
    let (llm, seen) = Scripted::new(turns(vec![(
        "m1",
        "more_tools",
        json!({"name": "subagent", "arguments": {
            "question": root.join("private/plan.txt").display().to_string()
        }}),
    )]));
    let config = SubagentConfig::new(
        Arc::new(|| Ok(Box::new(Reader) as Box<dyn LlmClient>)),
        "helper-model",
        60_000,
    );
    // Rules set after the helpers were registered still reach them.
    let agent = Agent::new(Box::new(llm)).with_subagents(config);
    agent.set_permissions(rules(&root, &[], &[], &["read_file(private)"]));
    agent.run("go").await.unwrap();
    let answer = &results(&seen)[0];
    assert!(
        answer.contains("Blocked by the permission rule `read_file(private)`")
            && !answer.contains("the plan"),
        "{answer}"
    );
}
