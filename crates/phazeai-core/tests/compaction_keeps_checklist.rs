//! After compaction the agent still has its checklist and knows which files
//! it changed, whatever the model's summary says: both are taken from the
//! tool calls and kept word for word under the summary.
//!
//! Drives the production loop (`Agent::run_with_events`) with a scripted
//! model, the real `todo`, `edit_file` and `read_file` tools and real
//! compaction, and checks what the model was sent after it.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use phazeai_core::context::carried::HEADING;
use phazeai_core::tools::sandbox;
use phazeai_core::{
    Agent, AgentEvent, LlmClient, LlmResponse, Message, PhazeError, StreamEvent, ToolDefinition,
};
use serde_json::{json, Value};

fn workspace() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root =
            std::env::temp_dir().join(format!("phazeai-keep-checklist-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        sandbox::set_workspace_root(Some(root.clone()));
        root
    })
}

/// Replies from a script; summaries are a fixed text that mentions neither
/// the checklist nor the files. Keeps every turn request.
struct Script {
    replies: Mutex<VecDeque<Vec<StreamEvent>>>,
    sent: Arc<Mutex<Vec<Vec<Message>>>>,
}

#[async_trait::async_trait]
impl LlmClient for Script {
    async fn chat(&self, _: &[Message], _: &[ToolDefinition]) -> Result<LlmResponse, PhazeError> {
        Ok(LlmResponse {
            message: Message::assistant("Goal: tidy the module."),
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> Result<UnboundedReceiver<StreamEvent>, PhazeError> {
        self.sent.lock().unwrap().push(messages.to_vec());
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| vec![StreamEvent::TextDelta("ok".into()), StreamEvent::Done]);
        let (tx, rx) = unbounded();
        for event in reply {
            tx.unbounded_send(event).unwrap();
        }
        Ok(rx)
    }
}

fn tool_call(id: &str, tool: &str, args: Value) -> Vec<StreamEvent> {
    vec![
        StreamEvent::ToolCallStart {
            id: id.into(),
            name: tool.into(),
        },
        StreamEvent::ToolCallDelta {
            id: id.into(),
            arguments_delta: args.to_string(),
        },
        StreamEvent::ToolCallEnd { id: id.into() },
        StreamEvent::Done,
    ]
}

fn answer(text: &str) -> Vec<StreamEvent> {
    vec![StreamEvent::TextDelta(text.into()), StreamEvent::Done]
}

#[tokio::test]
async fn the_checklist_and_changed_files_survive_compaction() {
    let dir = tempfile::tempdir_in(workspace()).unwrap();
    let module = dir.path().join("module.rs");
    std::fs::write(&module, "fn old_name() {}\n").unwrap();
    let big: Vec<PathBuf> = (0..5)
        .map(|n| {
            let path = dir.path().join(format!("big{n}.rs"));
            let text: String = (0..150)
                .map(|l| format!("pub fn big{n}_line{l:03}() -> u32 {{ {l} }}\n"))
                .collect();
            std::fs::write(&path, text).unwrap();
            path
        })
        .collect();

    // Turn 1: plan, edit, update the plan. Turns 2-6: one big read each.
    let mut replies = vec![
        tool_call(
            "t1",
            "todo",
            json!({"items": [
                {"task": "rename old_name", "status": "doing"},
                {"task": "update the callers", "status": "todo"},
            ]}),
        ),
        tool_call(
            "e1",
            "edit_file",
            json!({"path": module, "old_text": "old_name", "new_text": "new_name"}),
        ),
        tool_call(
            "t2",
            "todo",
            json!({"items": [
                {"task": "rename old_name", "status": "done"},
                {"task": "update the callers", "status": "doing"},
            ]}),
        ),
        answer("renamed"),
    ];
    for (n, path) in big.iter().enumerate() {
        replies.push(tool_call(
            &format!("r{n}"),
            "read_file",
            json!({"path": path}),
        ));
        replies.push(answer(&format!("read big{n}")));
    }
    let sent = Arc::new(Mutex::new(Vec::new()));
    let agent = Agent::new(Box::new(Script {
        replies: Mutex::new(replies.into()),
        sent: sent.clone(),
    }))
    .with_context_budget(6_000)
    .with_approval(Box::new(|_, _| Box::pin(async { true })));

    let mut compacted = 0;
    let prompts = ["tidy module.rs".to_string()]
        .into_iter()
        .chain((0..big.len()).map(|n| format!("look at big{n}.rs")));
    for prompt in prompts {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        agent.run_with_events(prompt, tx).await.unwrap();
        while let Ok(e) = rx.try_recv() {
            compacted += usize::from(matches!(e, AgentEvent::ContextCompacted { .. }));
        }
    }
    assert_eq!(
        std::fs::read_to_string(&module).unwrap(),
        "fn new_name() {}\n"
    );
    assert!(compacted >= 2, "compacted {compacted} times");

    // The last request: turn 1 is long gone from the messages, yet its newest
    // checklist and the edited file ride under the model's summary.
    let sent = sent.lock().unwrap();
    let last = sent.last().unwrap();
    assert!(
        !last.iter().any(|m| m.content.contains("tidy module.rs")),
        "turn 1 was not folded"
    );
    let system = &last[0].content;
    assert!(system.contains("Goal: tidy the module."), "{system}");
    let kept = &system[system.find(HEADING).expect("no kept section")..];
    assert!(kept.contains("[x] rename old_name"), "{kept}");
    assert!(kept.contains("[>] update the callers"), "{kept}");
    assert!(kept.contains("1 of 2 done"), "{kept}");
    assert!(
        !kept.contains("[>] rename old_name"),
        "the older list: {kept}"
    );
    assert!(kept.contains(&format!("- {}", module.display())), "{kept}");
    assert!(!kept.contains("big0.rs"), "a read is not a change: {kept}");
    // Kept once, however many times the session compacted.
    assert_eq!(system.matches(HEADING).count(), 1, "{system}");
}
