//! The system prompt is sized to the model's context budget.
//!
//! A local Ollama or LM Studio model gets a budget of about 4,100 tokens. An
//! instructions file of the size this repo's CLAUDE.md files add up to (about
//! 20 KB, ~7,000 tokens) used to go into the system prompt whole, so every
//! request was over budget before the conversation started, and the trim cut
//! every tool result down to a bare marker: the model never saw what its tools
//! returned.
//!
//! This drives the production loop (`Agent::run_with_events`) with the
//! production system prompt (`SystemPromptBuilder`), tool (`read_file`) and
//! budget (`Settings::context_budget()`). Only the model is scripted.

use std::sync::{Arc, Mutex};

use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use phazeai_core::config::LlmProvider;
use phazeai_core::{
    Agent, ConversationHistory, LlmClient, LlmResponse, Message, PhazeError, Role, Settings,
    StreamEvent, SystemPromptBuilder, ToolDefinition, ToolRegistry,
};
use serde_json::{json, Value};

/// What the agent's trim leaves in a tool result it had to cut.
const CUT_SUFFIX: &str = " bytes of this tool output were cut to fit the context budget ...]";

/// How the scripted model answers: given the number of requests before this
/// one and the request itself, the events of its reply.
type Script = Box<dyn Fn(usize, &[Message]) -> Vec<StreamEvent> + Send + Sync>;

/// Answers each request with `script` and records every request.
struct ScriptedModel {
    script: Script,
    requests: Arc<Mutex<Vec<Vec<Message>>>>,
}

#[async_trait::async_trait]
impl LlmClient for ScriptedModel {
    async fn chat(&self, _: &[Message], _: &[ToolDefinition]) -> Result<LlmResponse, PhazeError> {
        // Only compaction calls this; nothing here should need it.
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
        let turn = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(messages.to_vec());
            requests.len() - 1
        };
        let (tx, rx) = unbounded();
        for event in (self.script)(turn, messages) {
            tx.unbounded_send(event).unwrap();
        }
        Ok(rx)
    }
}

/// The arguments of the `read_file` call that a shortened instructions file's
/// marker (at the end of the system prompt) says reads the rest of it.
fn rest_of_instructions(request: &[Message]) -> Option<Value> {
    let system = request.iter().find(|m| m.role == Role::System)?;
    let (_, hint) = system.content.rsplit_once("call read_file with ")?;
    serde_json::from_str(hint.trim_end_matches(".]")).ok()
}

fn text(s: &str) -> Vec<StreamEvent> {
    vec![StreamEvent::TextDelta(s.into()), StreamEvent::Done]
}

/// A reply that calls `read_file` with `args`.
fn read_file(id: &str, args: Value) -> Vec<StreamEvent> {
    vec![
        StreamEvent::ToolCallStart {
            id: id.into(),
            name: "read_file".into(),
        },
        StreamEvent::ToolCallDelta {
            id: id.into(),
            arguments_delta: args.to_string(),
        },
        StreamEvent::ToolCallEnd { id: id.into() },
        StreamEvent::Done,
    ]
}

/// Estimated tokens of a request, counted by the production estimator.
fn estimated_tokens(request: &[Message]) -> usize {
    let mut h = ConversationHistory::new().with_max_messages(usize::MAX);
    for m in request {
        if m.role == Role::System {
            h.set_system_prompt(m.content.clone());
        } else {
            h.add_message(m.clone());
        }
    }
    h.estimate_tokens()
}

/// The result answering tool call `id` in `request`.
fn result_for<'a>(request: &'a [Message], id: &str) -> &'a str {
    request
        .iter()
        .find(|m| m.tool_call_id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("no result for {id}"))
        .content
        .as_str()
}

/// `line` as `read_file` numbers it, escaped the way it appears inside the
/// tool result's JSON.
fn numbered(n: usize, line: &str) -> String {
    let quoted = serde_json::to_string(&format!("{n:>6}\t{line}")).unwrap();
    quoted[1..quoted.len() - 1].to_string()
}

#[tokio::test]
async fn a_small_budget_with_a_long_instructions_file_still_delivers_tool_results() {
    // A project with a ~20 KB CLAUDE.md and a short file to read.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut instructions = String::new();
    let mut n = 0;
    while instructions.len() < 20 * 1024 {
        n += 1;
        instructions.push_str(&format!(
            "- Rule {n:04}: keep each change small and run the tests before saying it works.\n"
        ));
    }
    std::fs::write(root.join("CLAUDE.md"), &instructions).unwrap();
    let mut notes = String::new();
    for i in 0..30 {
        notes.push_str(&format!(
            "note {i:02}: the release is blocked on the audit\n"
        ));
    }
    notes.push_str("NOTES-END: ship on Friday\n");
    std::fs::write(root.join("notes.md"), &notes).unwrap();

    // The budget production gives a local Ollama model.
    let mut settings = Settings::default();
    settings.llm.provider = LlmProvider::Ollama;
    settings.llm.model = "qwen2.5-coder:7b".into();
    let budget = settings.context_budget();
    assert!(
        budget < 4_500,
        "expected a small local budget, got {budget}"
    );

    let mut tool_names: Vec<String> = ToolRegistry::default()
        .list()
        .iter()
        .map(|t| t.name().to_string())
        .collect();
    tool_names.sort();
    let system_prompt = SystemPromptBuilder::new()
        .with_project_root(root.clone())
        .with_tools(tool_names)
        .load_project_instructions()
        .with_context_budget(budget)
        .build();

    // The model reads notes.md, then does what the marker at the end of the
    // shortened instructions says, to read the rest of them.
    let notes = root.join("notes.md");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = ScriptedModel {
        script: Box::new(move |turn, request| match turn {
            0 => read_file("c1", json!({ "path": notes })),
            1 => match rest_of_instructions(request) {
                Some(args) => read_file("c2", args),
                None => text("I could not read the rest of the instructions."),
            },
            _ => text("Blocked on the audit; it ships on Friday."),
        }),
        requests: requests.clone(),
    };
    let agent = Agent::new(Box::new(model))
        .with_system_prompt(system_prompt)
        .with_context_budget(budget);

    let response = agent.run("What is the release blocked on?").await;

    let requests = requests.lock().unwrap();
    assert!(requests.len() >= 2, "{response:?}");
    // The tool result reaches the model whole, not as a bare marker.
    let notes_result = result_for(&requests[1], "c1");
    assert!(
        !notes_result.contains(CUT_SUFFIX),
        "the model got a cut tool result: {notes_result:?}"
    );
    assert!(notes_result.contains("NOTES-END: ship on Friday"));
    for (i, request) in requests.iter().enumerate() {
        let tokens = estimated_tokens(request);
        assert!(
            tokens <= budget,
            "request {i} is ~{tokens} tokens, over the {budget}-token budget"
        );
    }

    // The marker's read_file call reads exactly the first line the system
    // prompt left out.
    let rest_args = rest_of_instructions(&requests[0]).expect("a marker with a read_file call");
    assert_eq!(rest_args["path"], json!(root.join("CLAUDE.md")));
    let first_left_out = rest_args["offset"].as_u64().unwrap() as usize;
    let lines: Vec<&str> = instructions.lines().collect();
    let system = &requests[0][0];
    assert_eq!(system.role, Role::System);
    assert!(system.content.contains(lines[first_left_out - 2]));
    assert!(!system.content.contains(lines[first_left_out - 1]));
    assert_eq!(requests.len(), 3, "{response:?}");
    let rest_result = result_for(&requests[2], "c2");
    assert!(
        rest_result.contains(&numbered(first_left_out, lines[first_left_out - 1])),
        "{}",
        &rest_result[..rest_result.len().min(300)]
    );
    assert_eq!(
        response.unwrap().content,
        "Blocked on the audit; it ships on Friday."
    );
}
