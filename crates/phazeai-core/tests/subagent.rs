//! `subagent`: a helper on its own model reads files for a question, and only
//! its answer reaches the main conversation. Helpers asked for in one turn
//! run together, their model calls are reported at their own model's price,
//! and `todo` gives its list back as short lines.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use phazeai_core::cost::Meter;
use phazeai_core::llm::Usage;
use phazeai_core::{
    Agent, AgentEvent, LlmClient, LlmResponse, Message, PhazeError, ProviderId, StreamEvent,
    SubagentConfig, ToolDefinition,
};
use serde_json::json;

fn usage(input: u32, output: u32) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
    }
}

fn call(id: &str, name: &str, args: serde_json::Value) -> Vec<StreamEvent> {
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

/// The main model: plays its replies in order and keeps every request.
struct Main {
    replies: Mutex<VecDeque<Vec<StreamEvent>>>,
    seen: Arc<Mutex<Vec<Vec<Message>>>>,
}

#[async_trait::async_trait]
impl LlmClient for Main {
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
        Ok(send(self.replies.lock().unwrap().pop_front().unwrap()))
    }
}

/// A helper model: asked "first line of <path>", it reads the file, then
/// answers with the file's first line. It records which tools it was offered
/// and how many helpers were mid-question at once.
struct Helper {
    tools_offered: Arc<Mutex<Vec<String>>>,
    running: Arc<Mutex<(usize, usize)>>,
}

#[async_trait::async_trait]
impl LlmClient for Helper {
    async fn chat(&self, _: &[Message], _: &[ToolDefinition]) -> Result<LlmResponse, PhazeError> {
        Ok(LlmResponse {
            message: Message::assistant("summary"),
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<UnboundedReceiver<StreamEvent>, PhazeError> {
        *self.tools_offered.lock().unwrap() = tools.iter().map(|t| t.name.clone()).collect();
        let question = messages
            .iter()
            .rev()
            .find(|m| m.tool_call_id.is_none() && m.content.starts_with("first line of "))
            .map(|m| m.content["first line of ".len()..].to_string())
            .unwrap();
        match messages.iter().rev().find(|m| m.tool_call_id.is_some()) {
            None => {
                {
                    let mut running = self.running.lock().unwrap();
                    running.0 += 1;
                    running.1 = running.1.max(running.0);
                }
                // Long enough for a second helper to start meanwhile.
                tokio::time::sleep(Duration::from_millis(150)).await;
                let mut reply = call("r1", "read_file", json!({ "path": question }));
                reply.push(StreamEvent::Usage(usage(1_000, 20)));
                reply.push(StreamEvent::Done);
                Ok(send(reply))
            }
            Some(result) => {
                self.running.lock().unwrap().0 -= 1;
                let first = result
                    .content
                    .lines()
                    .find_map(|l| l.find("TITLE").map(|i| &l[i..]))
                    .unwrap_or("not found")
                    .to_string();
                Ok(send(vec![
                    StreamEvent::TextDelta(format!("It says: {first}")),
                    StreamEvent::Usage(usage(3_000, 10)),
                    StreamEvent::Done,
                ]))
            }
        }
    }
}

struct Project {
    _dir: tempfile::TempDir,
    a: PathBuf,
    b: PathBuf,
}

fn project() -> Project {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let body: String = (0..400)
        .map(|i| format!("filler line {i} that only the helper should read\n"))
        .collect();
    let a = root.join("a.md");
    let b = root.join("b.md");
    std::fs::write(&a, format!("TITLE alpha\n{body}")).unwrap();
    std::fs::write(&b, format!("TITLE beta\n{body}")).unwrap();
    Project { _dir: dir, a, b }
}

#[tokio::test]
async fn helpers_read_for_the_agent_and_only_their_answers_come_back() {
    let p = project();
    let ask = |id: &str, path: &PathBuf| {
        call(
            id,
            "more_tools",
            json!({
                "name": "subagent",
                "arguments": { "question": format!("first line of {}", path.display()) }
            }),
        )
    };
    let mut first = ask("m1", &p.a);
    first.extend(ask("m2", &p.b));
    first.push(StreamEvent::Usage(usage(5_000, 40)));
    first.push(StreamEvent::Done);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let main = Main {
        replies: Mutex::new(
            vec![
                first,
                vec![StreamEvent::TextDelta("Done.".into()), StreamEvent::Done],
            ]
            .into(),
        ),
        seen: seen.clone(),
    };
    let tools_offered = Arc::new(Mutex::new(Vec::new()));
    let running = Arc::new(Mutex::new((0, 0)));
    let (offered, live) = (tools_offered.clone(), running.clone());
    let config = SubagentConfig::new(
        Arc::new(move || {
            Ok(Box::new(Helper {
                tools_offered: offered.clone(),
                running: live.clone(),
            }) as Box<dyn LlmClient>)
        }),
        "claude-haiku-4-5",
        60_000,
    );
    let agent = Agent::new(Box::new(main)).with_subagents(config);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let response = agent
        .run_with_events("Compare the titles", tx)
        .await
        .unwrap();
    assert_eq!(response.content, "Done.");

    // The main model's second request holds the two answers, not the files.
    let requests = seen.lock().unwrap();
    let results: Vec<&str> = requests[1]
        .iter()
        .filter(|m| m.tool_call_id.is_some())
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(
        results,
        [
            "It says: TITLE alpha\n\n(helper: 2 model turns on claude-haiku-4-5)",
            "It says: TITLE beta\n\n(helper: 2 model turns on claude-haiku-4-5)",
        ]
    );
    assert!(!format!("{:?}", requests[1]).contains("filler line"));

    // The two ran at the same time, with read-only tools only.
    assert_eq!(running.lock().unwrap().1, 2);
    assert_eq!(
        *tools_offered.lock().unwrap(),
        ["find_path", "glob", "grep", "list_files", "read_file"]
    );

    // Each helper call is reported at the helper's model, apart from the
    // main model's own calls, and a meter prices it as that model.
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    let helper_calls: Vec<(String, Usage)> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::SubagentUsage { model, usage } => Some((model.clone(), usage.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(helper_calls.len(), 4);
    assert!(helper_calls.iter().all(|(m, _)| m == "claude-haiku-4-5"));
    let main_calls = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::CallUsage(_)))
        .count();
    assert_eq!(main_calls, 1);
    let mut meter = Meter::default();
    for (model, u) in &helper_calls {
        meter.add(Some((&ProviderId::Claude, model)), u);
    }
    // Haiku 4.5: $1/M in, $5/M out. 2 × (1,000 + 3,000) in, 2 × 30 out.
    let expected = (8_000.0 * 1.0 + 60.0 * 5.0) / 1_000_000.0;
    assert!((meter.cost - expected).abs() < 1e-12, "{}", meter.cost);
}

#[tokio::test]
async fn a_helper_that_cannot_start_reports_why() {
    let main = Main {
        replies: Mutex::new(
            vec![
                {
                    let mut r = call(
                        "m1",
                        "more_tools",
                        json!({"name": "subagent", "arguments": {"question": "where is main?"}}),
                    );
                    r.push(StreamEvent::Done);
                    r
                },
                vec![StreamEvent::TextDelta("ok".into()), StreamEvent::Done],
            ]
            .into(),
        ),
        seen: Arc::new(Mutex::new(Vec::new())),
    };
    let seen = main.seen.clone();
    let config = SubagentConfig::new(
        Arc::new(|| Err(PhazeError::Config("no key for the small model".into()))),
        "small",
        8_000,
    );
    let agent = Agent::new(Box::new(main)).with_subagents(config);
    agent.run("go").await.unwrap();
    let requests = seen.lock().unwrap();
    let result = requests[1]
        .iter()
        .find(|m| m.tool_call_id.is_some())
        .unwrap();
    assert!(
        result.content.contains("no key for the small model"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn todo_gives_the_list_back_as_short_lines() {
    let mut first = call(
        "t1",
        "more_tools",
        json!({"name": "todo", "arguments": {"items": [
            {"task": "Find the parser", "status": "done"},
            {"task": "Fix the bug", "status": "doing"},
            {"task": "Add a test", "status": "todo"},
        ]}}),
    );
    first.push(StreamEvent::Done);
    let main = Main {
        replies: Mutex::new(
            vec![
                first,
                vec![StreamEvent::TextDelta("ok".into()), StreamEvent::Done],
            ]
            .into(),
        ),
        seen: Arc::new(Mutex::new(Vec::new())),
    };
    let seen = main.seen.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    Agent::new(Box::new(main))
        .run_with_events("go", tx)
        .await
        .unwrap();
    let list = "[x] Find the parser\n[>] Fix the bug\n[ ] Add a test\n1 of 3 done";
    let requests = seen.lock().unwrap();
    let result = requests[1]
        .iter()
        .find(|m| m.tool_call_id.is_some())
        .unwrap();
    assert_eq!(result.content, list);
    // The user sees the same list.
    let mut shown = None;
    while let Ok(e) = rx.try_recv() {
        if let AgentEvent::ToolResult { name, summary, .. } = e {
            shown = Some((name, summary));
        }
    }
    assert_eq!(shown, Some(("todo".to_string(), list.to_string())));
}
