//! Stop always stops and leaves a conversation the next request can send;
//! trimming to the context budget never drops the question being answered.
//!
//! The tests drive the production loop (`Agent::run_with_events`) and
//! `ConversationHistory` with scripted models: no network, no real provider.
//! The scripted model checks every request the way OpenAI and Anthropic do
//! and rejects an invalid conversation, as they answer with HTTP 400.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use phazeai_core::llm::{FunctionCall, ToolCall};
use phazeai_core::tools::WriteFileTool;
use phazeai_core::{
    Agent, AgentEvent, AgentResponse, ApprovalFn, ConversationHistory, DiffHookFn, LlmClient,
    LlmResponse, Message, PhazeError, Role, StreamEvent, Tool, ToolDefinition, ToolRegistry,
    ToolResult,
};
use serde_json::{json, Value};
use tokio::sync::Mutex as AsyncMutex;

/// How long a run may take after Stop before the test gives up on it.
const STOP_BOUND: Duration = Duration::from_secs(2);
/// What the model is told when a tool output was cut.
const CUT_SUFFIX: &str = " bytes of this tool output were cut to fit the context budget ...]";

// ── Checks ───────────────────────────────────────────────────────────────────

/// What OpenAI and Anthropic check before accepting a conversation: it opens
/// with a plain user message, every tool call is answered exactly once by the
/// results that follow its message, and every result belongs to such a call.
fn check_valid(messages: &[Message]) -> Result<(), String> {
    let conv: Vec<&Message> = messages.iter().filter(|m| m.role != Role::System).collect();
    match conv.first() {
        Some(m) if m.role == Role::User && m.tool_call_id.is_none() => {}
        other => {
            return Err(format!(
                "the conversation opens with {:?}",
                other.map(brief)
            ))
        }
    }
    let mut i = 0;
    while i < conv.len() {
        let m = conv[i];
        i += 1;
        if let Some(id) = &m.tool_call_id {
            return Err(format!("tool result {id} has no tool call before it"));
        }
        let Some(calls) = &m.tool_calls else {
            continue;
        };
        let mut answers: HashMap<&str, usize> = calls.iter().map(|c| (c.id.as_str(), 0)).collect();
        while let Some(id) = conv.get(i).and_then(|m| m.tool_call_id.as_deref()) {
            *answers
                .get_mut(id)
                .ok_or_else(|| format!("tool result {id} answers no call of its message"))? += 1;
            i += 1;
        }
        for call in calls {
            let n = answers[call.id.as_str()];
            if n != 1 {
                return Err(format!(
                    "tool call {} ({}) has {n} results",
                    call.id, call.function.name
                ));
            }
        }
    }
    Ok(())
}

/// A message in a line, for assertion messages (contents can be huge).
fn brief(m: &&Message) -> String {
    let head: String = m.content.chars().take(40).collect();
    format!(
        "{:?} call_id={:?} calls={} len={} {head:?}",
        m.role,
        m.tool_call_id,
        m.tool_calls.as_ref().map_or(0, Vec::len),
        m.content.len()
    )
}

fn describe(messages: &[Message]) -> Vec<String> {
    messages.iter().map(|m| brief(&m)).collect()
}

/// The contents of every tool result answering `id`.
fn results_for<'a>(messages: &'a [Message], id: &str) -> Vec<&'a str> {
    messages
        .iter()
        .filter(|m| m.tool_call_id.as_deref() == Some(id))
        .map(|m| m.content.as_str())
        .collect()
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

/// (bytes the marker says were cut, bytes of the output still shown).
fn cut_counts(output: &str) -> (usize, usize) {
    let end = output
        .find(CUT_SUFFIX)
        .unwrap_or_else(|| panic!("no cut marker in {output:?}"));
    let start = output[..end].rfind("[... ").expect("marker start");
    let cut = output[start + 5..end].parse().expect("bytes cut");
    let head = &output[..start];
    let tail = &output[end + CUT_SUFFIX.len()..];
    let head = head.strip_suffix('\n').unwrap_or(head);
    let tail = tail.strip_prefix('\n').unwrap_or(tail);
    (cut, head.len() + tail.len())
}

fn assert_cancelled(result: &Result<AgentResponse, PhazeError>, events: &[AgentEvent]) {
    assert!(matches!(result, Err(PhazeError::Cancelled)), "{result:?}");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Error(m) if m == "Cancelled")),
        "no Cancelled event: {events:?}"
    );
}

// ── Scripted models ──────────────────────────────────────────────────────────

/// One scripted reply.
enum Reply {
    /// Stream these events, then end the stream.
    Stream(Vec<StreamEvent>),
    /// Stream these events and leave the stream open: the sender is parked
    /// in [`ScriptedLlm::open`].
    StreamThenHold(Vec<StreamEvent>),
}

/// Answers from a script (then "done"), records every request, and rejects
/// an invalid conversation.
struct ScriptedLlm {
    script: Mutex<VecDeque<Reply>>,
    requests: Arc<Mutex<Vec<Vec<Message>>>>,
    open: Arc<Mutex<Option<UnboundedSender<StreamEvent>>>>,
}

impl ScriptedLlm {
    fn new(script: Vec<Reply>) -> Self {
        Self {
            script: Mutex::new(script.into()),
            requests: Arc::default(),
            open: Arc::default(),
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for ScriptedLlm {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse, PhazeError> {
        Ok(LlmResponse {
            message: Message::assistant("summary"),
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> Result<UnboundedReceiver<StreamEvent>, PhazeError> {
        self.requests.lock().unwrap().push(messages.to_vec());
        check_valid(messages).map_err(|e| PhazeError::Llm(format!("400 Bad Request: {e}")))?;
        let reply = self.script.lock().unwrap().pop_front();
        let (tx, rx) = unbounded();
        match reply.unwrap_or_else(|| Reply::Stream(text("done"))) {
            Reply::Stream(events) => events
                .into_iter()
                .for_each(|e| tx.unbounded_send(e).unwrap()),
            Reply::StreamThenHold(events) => {
                events
                    .into_iter()
                    .for_each(|e| tx.unbounded_send(e).unwrap());
                *self.open.lock().unwrap() = Some(tx);
            }
        }
        Ok(rx)
    }
}

/// A provider that takes the request and never answers.
struct NoResponse;

#[async_trait::async_trait]
impl LlmClient for NoResponse {
    async fn chat(&self, _: &[Message], _: &[ToolDefinition]) -> Result<LlmResponse, PhazeError> {
        std::future::pending().await
    }

    async fn chat_stream(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
    ) -> Result<UnboundedReceiver<StreamEvent>, PhazeError> {
        std::future::pending().await
    }
}

fn text(s: &str) -> Vec<StreamEvent> {
    vec![StreamEvent::TextDelta(s.into()), StreamEvent::Done]
}

/// A reply asking for these tool calls, (id, tool, arguments), in one message.
fn calls<I: AsRef<str>>(list: &[(I, &str, Value)]) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    for (id, name, args) in list {
        let id = id.as_ref().to_string();
        events.push(StreamEvent::ToolCallStart {
            id: id.clone(),
            name: name.to_string(),
        });
        events.push(StreamEvent::ToolCallDelta {
            id: id.clone(),
            arguments_delta: args.to_string(),
        });
        events.push(StreamEvent::ToolCallEnd { id });
    }
    events.push(StreamEvent::Done);
    events
}

fn tool_call(id: &str, name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: name.to_string(),
            arguments: args.to_string(),
        },
    }
}

// ── Tools ────────────────────────────────────────────────────────────────────

/// Echoes its text and counts its runs.
struct Echo {
    runs: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Tool for Echo {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "Echoes its text"
    }
    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {"text": {"type": "string"}}})
    }
    async fn execute(&self, params: Value) -> ToolResult {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(json!({ "echoed": params["text"] }))
    }
}

/// Runs for a minute unless Stop ends it.
struct Slow;

#[async_trait::async_trait]
impl Tool for Slow {
    fn name(&self) -> &str {
        "slow"
    }
    fn description(&self) -> &str {
        "Takes a minute"
    }
    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }
    async fn execute(&self, _params: Value) -> ToolResult {
        tokio::time::sleep(Duration::from_secs(60)).await;
        Ok(json!("finished"))
    }
}

/// Returns `size` bytes of output between BEGIN and END.
struct Big {
    size: usize,
}

#[async_trait::async_trait]
impl Tool for Big {
    fn name(&self) -> &str {
        "big"
    }
    fn description(&self) -> &str {
        "Returns a lot of output"
    }
    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }
    async fn execute(&self, _params: Value) -> ToolResult {
        Ok(Value::String(format!(
            "BEGIN {} END",
            "x".repeat(self.size)
        )))
    }
}

fn echo_tools(runs: &Arc<AtomicUsize>) -> ToolRegistry {
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(Echo { runs: runs.clone() }));
    tools.register(Box::new(Slow));
    tools
}

// ── Running and stopping ─────────────────────────────────────────────────────

type EventRx = tokio::sync::mpsc::UnboundedReceiver<AgentEvent>;

/// Run `agent` on `prompt` while `stopper` (given the run's events) decides
/// when to press Stop. Fails the test if the run outlives [`STOP_BOUND`].
async fn run_with_stop<Fut>(
    agent: &Agent,
    prompt: &str,
    stopper: impl FnOnce(EventRx) -> Fut,
) -> (Result<AgentResponse, PhazeError>, Vec<AgentEvent>)
where
    Fut: Future<Output = Vec<AgentEvent>>,
{
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let both = async { tokio::join!(agent.run_with_events(prompt, tx), stopper(rx)) };
    tokio::time::timeout(STOP_BOUND, both)
        .await
        .unwrap_or_else(|_| {
            panic!("the run was still going {STOP_BOUND:?} in: Stop did not end it")
        })
}

async fn drain(mut rx: EventRx) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

/// Press Stop after `delay`, then collect the run's events.
async fn stop_after(cancel: Arc<AtomicBool>, delay: Duration, rx: EventRx) -> Vec<AgentEvent> {
    tokio::time::sleep(delay).await;
    cancel.store(true, Ordering::SeqCst);
    drain(rx).await
}

/// Press Stop as soon as an event matches `when`, then collect the rest.
async fn stop_on(
    cancel: Arc<AtomicBool>,
    mut rx: EventRx,
    when: impl Fn(&AgentEvent) -> bool,
) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        let hit = when(&event);
        events.push(event);
        if hit {
            cancel.store(true, Ordering::SeqCst);
            break;
        }
    }
    events.extend(drain(rx).await);
    events
}

/// The next send on the same conversation, as the CLI (one history for the
/// session) or any caller of `with_shared_conversation` makes it.
async fn next_run_is_accepted(conv: &Arc<AsyncMutex<ConversationHistory>>) {
    let agent = Agent::new(Box::new(ScriptedLlm::new(vec![Reply::Stream(text("ok"))])))
        .with_shared_conversation(conv.clone());
    let result = agent.run("and now?").await;
    assert!(
        matches!(&result, Ok(r) if r.content == "ok"),
        "the next run on the same conversation failed: {result:?}"
    );
}

/// The three calls the D4 tests start from: `first`, then two echoes.
fn three_calls(first: (&str, Value)) -> Vec<StreamEvent> {
    calls(&[
        ("c1", first.0, first.1),
        ("c2", "echo", json!({"text": "two"})),
        ("c3", "echo", json!({"text": "three"})),
    ])
}

// ── D3: Stop while the provider is silent ───────────────────────────────────

async fn assert_stop_ends_a_silent_run(llm: Box<dyn LlmClient>) {
    let cancel = Arc::new(AtomicBool::new(false));
    let agent = Agent::new(llm).with_cancel_token(cancel.clone());
    let stop_at = Duration::from_millis(100);
    let started = Instant::now();
    let (result, events) =
        run_with_stop(&agent, "hello?", |rx| stop_after(cancel, stop_at, rx)).await;
    let elapsed = started.elapsed();
    assert_cancelled(&result, &events);
    assert!(
        elapsed < stop_at + Duration::from_millis(500),
        "the run ended {elapsed:?} after it started; Stop came at {stop_at:?}"
    );
}

#[tokio::test]
async fn stop_ends_a_run_whose_provider_never_starts_answering() {
    assert_stop_ends_a_silent_run(Box::new(NoResponse)).await;
}

#[tokio::test]
async fn stop_ends_a_run_whose_stream_goes_quiet() {
    let quiet = ScriptedLlm::new(vec![Reply::StreamThenHold(vec![StreamEvent::TextDelta(
        "Let me look".into(),
    )])]);
    assert_stop_ends_a_silent_run(Box::new(quiet)).await;
}

// ── D4: Stop in a tool round leaves every call answered ─────────────────────

#[tokio::test]
async fn stop_while_a_tool_runs_answers_every_call_and_the_next_run_is_accepted() {
    let conv = Arc::new(AsyncMutex::new(ConversationHistory::new()));
    let cancel = Arc::new(AtomicBool::new(false));
    let runs = Arc::new(AtomicUsize::new(0));
    let agent = Agent::new(Box::new(ScriptedLlm::new(vec![Reply::Stream(
        three_calls(("slow", json!({}))),
    )])))
    .with_tools(echo_tools(&runs))
    .with_cancel_token(cancel.clone())
    .with_shared_conversation(conv.clone());

    let (result, events) = run_with_stop(&agent, "run three tools", |rx| {
        stop_on(
            cancel,
            rx,
            |e| matches!(e, AgentEvent::ToolStart { name } if name == "slow"),
        )
    })
    .await;
    assert_cancelled(&result, &events);

    let history = conv.lock().await.get_messages();
    check_valid(&history).unwrap_or_else(|e| panic!("{e}: {:#?}", describe(&history)));
    let c1 = results_for(&history, "c1");
    assert!(
        c1.len() == 1 && c1[0].contains("Interrupted by the user"),
        "{c1:?}"
    );
    for id in ["c2", "c3"] {
        let r = results_for(&history, id);
        assert!(r.len() == 1 && r[0].contains("did not run"), "{id}: {r:?}");
    }
    assert_eq!(runs.load(Ordering::SeqCst), 0, "no echo may run after Stop");
    next_run_is_accepted(&conv).await;
}

#[tokio::test]
async fn stop_after_the_calls_arrive_but_before_any_runs_leaves_a_valid_history() {
    let conv = Arc::new(AsyncMutex::new(ConversationHistory::new()));
    let cancel = Arc::new(AtomicBool::new(false));
    let runs = Arc::new(AtomicUsize::new(0));
    // The model lists its three calls, then a word, and the stream stays
    // open; once the agent has read it all, Stop, and the provider closes the
    // stream without a Done event.
    let mut reply = three_calls(("echo", json!({"text": "one"})));
    reply.pop();
    reply.push(StreamEvent::TextDelta("Running them now.".into()));
    let llm = ScriptedLlm::new(vec![Reply::StreamThenHold(reply)]);
    let open = llm.open.clone();
    let agent = Agent::new(Box::new(llm))
        .with_tools(echo_tools(&runs))
        .with_cancel_token(cancel.clone())
        .with_shared_conversation(conv.clone());

    let (result, events) = run_with_stop(&agent, "run three tools", |mut rx| async move {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            let read_all = matches!(event, AgentEvent::TextDelta(_));
            events.push(event);
            if read_all {
                break;
            }
        }
        cancel.store(true, Ordering::SeqCst);
        drop(open.lock().unwrap().take());
        events.extend(drain(rx).await);
        events
    })
    .await;
    assert_cancelled(&result, &events);

    let history = conv.lock().await.get_messages();
    check_valid(&history).unwrap_or_else(|e| panic!("{e}: {:#?}", describe(&history)));
    for id in ["c1", "c2", "c3"] {
        let r = results_for(&history, id);
        assert!(r.iter().all(|r| r.contains("did not run")), "{id}: {r:?}");
    }
    assert_eq!(runs.load(Ordering::SeqCst), 0, "no tool may run after Stop");
    next_run_is_accepted(&conv).await;
}

#[tokio::test]
async fn stop_while_waiting_for_approval_ends_the_run_and_answers_every_call() {
    let conv = Arc::new(AsyncMutex::new(ConversationHistory::new()));
    let cancel = Arc::new(AtomicBool::new(false));
    let runs = Arc::new(AtomicUsize::new(0));
    // Nobody ever answers the approval prompt.
    let approval: ApprovalFn = Box::new(|_, _| Box::pin(std::future::pending()));
    let agent = Agent::new(Box::new(ScriptedLlm::new(vec![Reply::Stream(
        three_calls(("echo", json!({"text": "one"}))),
    )])))
    .with_tools(echo_tools(&runs))
    .with_approval(approval)
    .with_cancel_token(cancel.clone())
    .with_shared_conversation(conv.clone());

    let (result, events) = run_with_stop(&agent, "run three tools", |rx| {
        stop_on(cancel, rx, |e| {
            matches!(e, AgentEvent::ToolApprovalRequest { .. })
        })
    })
    .await;
    assert_cancelled(&result, &events);

    let history = conv.lock().await.get_messages();
    check_valid(&history).unwrap_or_else(|e| panic!("{e}: {:#?}", describe(&history)));
    for id in ["c1", "c2", "c3"] {
        let r = results_for(&history, id);
        assert!(r.len() == 1 && r[0].contains("did not run"), "{id}: {r:?}");
    }
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    next_run_is_accepted(&conv).await;
}

#[tokio::test]
async fn stop_while_waiting_for_diff_review_ends_the_run_and_answers_every_call() {
    let conv = Arc::new(AsyncMutex::new(ConversationHistory::new()));
    let cancel = Arc::new(AtomicBool::new(false));
    let runs = Arc::new(AtomicUsize::new(0));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    // Nobody ever answers the diff review. (The agent reviews what the
    // registered write_file would write, so it is registered, as in the IDE.)
    let hook: DiffHookFn = Box::new(|_, _, _| Box::pin(std::future::pending()));
    let mut tools = echo_tools(&runs);
    tools.register(Box::new(WriteFileTool));
    let agent = Agent::new(Box::new(ScriptedLlm::new(vec![Reply::Stream(
        three_calls(("write_file", json!({"path": path, "content": "new text"}))),
    )])))
    .with_tools(tools)
    .with_diff_hook(hook)
    .with_cancel_token(cancel.clone())
    .with_shared_conversation(conv.clone());

    let (result, events) = run_with_stop(&agent, "write a file", |rx| {
        stop_on(cancel, rx, |e| matches!(e, AgentEvent::FilePatch { .. }))
    })
    .await;
    assert_cancelled(&result, &events);

    let history = conv.lock().await.get_messages();
    check_valid(&history).unwrap_or_else(|e| panic!("{e}: {:#?}", describe(&history)));
    for id in ["c1", "c2", "c3"] {
        let r = results_for(&history, id);
        assert!(r.len() == 1 && r[0].contains("did not run"), "{id}: {r:?}");
    }
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    next_run_is_accepted(&conv).await;
}

#[tokio::test]
async fn a_call_approved_after_stop_does_not_run() {
    let conv = Arc::new(AsyncMutex::new(ConversationHistory::new()));
    let cancel = Arc::new(AtomicBool::new(false));
    let runs = Arc::new(AtomicUsize::new(0));
    // The user presses Stop, then answers "allow" on the prompt still shown.
    let approval: ApprovalFn = {
        let cancel = cancel.clone();
        Box::new(move |_, _| {
            let cancel = cancel.clone();
            Box::pin(async move {
                cancel.store(true, Ordering::SeqCst);
                true
            })
        })
    };
    let agent = Agent::new(Box::new(ScriptedLlm::new(vec![Reply::Stream(
        three_calls(("echo", json!({"text": "one"}))),
    )])))
    .with_tools(echo_tools(&runs))
    .with_approval(approval)
    .with_cancel_token(cancel.clone())
    .with_shared_conversation(conv.clone());

    let (result, events) = run_with_stop(&agent, "run three tools", drain).await;
    assert_cancelled(&result, &events);

    let history = conv.lock().await.get_messages();
    check_valid(&history).unwrap_or_else(|e| panic!("{e}: {:#?}", describe(&history)));
    for id in ["c1", "c2", "c3"] {
        let r = results_for(&history, id);
        assert!(r.len() == 1 && r[0].contains("did not run"), "{id}: {r:?}");
    }
    assert_eq!(
        runs.load(Ordering::SeqCst),
        0,
        "the approved echo ran after Stop"
    );
    next_run_is_accepted(&conv).await;
}

// ── D1: an oversized result is cut, the question stays ──────────────────────

#[tokio::test]
async fn oversized_tool_results_are_cut_to_fit_and_the_question_stays() {
    // The smallest budget PhazeAI gives a model, and three outputs that are
    // each about as big as all of it.
    const BUDGET: usize = 4096;
    let llm = ScriptedLlm::new(vec![
        Reply::Stream(calls(&[
            ("c1", "big", json!({})),
            ("c2", "big", json!({})),
            ("c3", "big", json!({})),
        ])),
        Reply::Stream(text("the build failed in step 2")),
    ]);
    let requests = llm.requests.clone();
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(Big { size: 100_000 }));
    let agent = Agent::new(Box::new(llm))
        .with_tools(tools)
        .with_context_budget(BUDGET)
        .with_system_prompt("You are a coding agent.");
    let question = "Read the three build logs and tell me what failed.";

    let response = agent.run(question).await;

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2, "{response:?}");
    let sent = &requests[1];
    let first = sent.iter().find(|m| m.role != Role::System);
    assert!(
        first.is_some_and(|m| m.role == Role::User && m.content == question),
        "the request after the tool round lost the question: {:#?}",
        describe(sent)
    );
    let tokens = estimated_tokens(sent);
    assert!(
        tokens <= BUDGET,
        "the request is ~{tokens} tokens, over the {BUDGET}-token budget"
    );
    for id in ["c1", "c2", "c3"] {
        let out = results_for(sent, id);
        assert_eq!(out.len(), 1, "{id}");
        // The model is told, and still sees how the output starts.
        assert!(out[0].contains(CUT_SUFFIX), "{id} was cut silently");
        assert!(out[0].starts_with("\"BEGIN"), "{id}: {:?}", &out[0][..40]);
    }
    assert_eq!(response.unwrap().content, "the build failed in step 2");
}

#[test]
fn trimming_cuts_the_longest_output_of_the_last_turn_and_counts_every_byte_cut() {
    let mut h = ConversationHistory::new().with_system_prompt("SYS");
    h.add_user_message("earlier question");
    h.add_assistant_message("earlier answer");
    h.add_user_message("current question");
    h.add_message(Message::assistant_with_tools(
        "",
        vec![
            tool_call("a", "read_file", json!({})),
            tool_call("b", "read_file", json!({})),
        ],
    ));
    let long = format!("HEAD{}TAIL", "x".repeat(9000));
    h.add_tool_result("a", long.clone());
    h.add_tool_result("b", "short output");

    h.trim_to_token_budget(1500);
    let kept = h.get_conversation_messages();
    // The earlier turn went first; the question and both results stay.
    assert_eq!(kept.len(), 4, "{:#?}", describe(&kept));
    assert_eq!(kept[0].content, "current question");
    assert!(h.estimate_tokens() <= 1500, "{}", h.estimate_tokens());
    check_valid(&h.get_messages()).unwrap();
    // The short output is whole; the long one keeps its start and end around
    // a marker that counts what was cut.
    assert_eq!(kept[3].content, "short output");
    let cut = &kept[2].content;
    assert!(cut.starts_with("HEAD") && cut.ends_with("TAIL"), "{cut:?}");
    let (bytes_cut, shown) = cut_counts(cut);
    assert_eq!(bytes_cut + shown, long.len());

    // Cut again under a smaller budget: the count still covers every byte.
    h.trim_to_token_budget(600);
    assert!(h.estimate_tokens() <= 600, "{}", h.estimate_tokens());
    let kept = h.get_conversation_messages();
    assert_eq!(kept[0].content, "current question");
    let (bytes_cut_again, shown_again) = cut_counts(&kept[2].content);
    assert!(shown_again < shown);
    assert_eq!(bytes_cut_again + shown_again, long.len());
    check_valid(&h.get_messages()).unwrap();
}

#[test]
fn trimming_drops_the_oldest_rounds_of_the_last_turn_but_never_the_question_or_newest_round() {
    // Six rounds whose calls alone (about 1000 tokens each) cannot all fit in
    // 2500 tokens, however much output is cut.
    let mut h = ConversationHistory::new();
    let question = "Write the six files.";
    h.add_user_message(question);
    for n in 0..6 {
        let id = format!("w{n}");
        let args = json!({"path": format!("f{n}.rs"), "content": "y".repeat(3000)});
        h.add_message(Message::assistant_with_tools(
            "",
            vec![tool_call(&id, "write_file", args)],
        ));
        h.add_tool_result(&id, format!("wrote f{n}.rs"));
    }

    h.trim_to_token_budget(2500);
    assert!(h.estimate_tokens() <= 2500, "{}", h.estimate_tokens());
    let kept = h.get_conversation_messages();
    check_valid(&h.get_messages()).unwrap_or_else(|e| panic!("{e}: {:#?}", describe(&kept)));
    // The question is kept word for word, with a note that steps were removed.
    assert!(
        kept[0].content.starts_with(question),
        "{:?}",
        kept[0].content
    );
    assert!(
        kept[0]
            .content
            .contains("removed to fit the context budget"),
        "the model was not told: {:?}",
        kept[0].content
    );
    // The newest round stays with its result; the oldest went first.
    let ids: Vec<&str> = kept
        .iter()
        .filter_map(|m| m.tool_calls.as_ref())
        .flatten()
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(ids.last(), Some(&"w5"), "{ids:?}");
    assert!(!ids.contains(&"w0"), "{ids:?}");
    assert_eq!(results_for(&kept, "w5"), vec!["wrote f5.rs"]);
}

// ── D2: the message cap never drops the last turn ───────────────────────────

#[tokio::test]
async fn a_run_longer_than_the_message_cap_keeps_its_question_in_every_request() {
    // 12 rounds of 10 parallel calls: 133 messages in one turn, past the
    // history's cap of 100.
    const ROUNDS: usize = 12;
    const PARALLEL: usize = 10;
    let mut script: Vec<Reply> = (0..ROUNDS)
        .map(|r| {
            let round: Vec<(String, &str, Value)> = (0..PARALLEL)
                .map(|c| {
                    (
                        format!("r{r}c{c}"),
                        "echo",
                        json!({"text": format!("{r}.{c}")}),
                    )
                })
                .collect();
            Reply::Stream(calls(&round))
        })
        .collect();
    script.push(Reply::Stream(text("all checked")));
    let llm = ScriptedLlm::new(script);
    let requests = llm.requests.clone();
    let runs = Arc::new(AtomicUsize::new(0));
    let agent = Agent::new(Box::new(llm))
        .with_tools(echo_tools(&runs))
        .with_max_iterations(ROUNDS + 2);
    let question = "Check every file, one echo per file.";

    let response = agent.run(question).await;

    let requests = requests.lock().unwrap();
    for (n, request) in requests.iter().enumerate() {
        let first = request.iter().find(|m| m.role != Role::System);
        assert!(
            first.is_some_and(|m| m.role == Role::User && m.content == question),
            "request {n} ({} messages) lost the question; it opens with {:?}",
            request.len(),
            first.as_ref().map(brief)
        );
    }
    let response = response.expect("the run should finish");
    assert_eq!(response.content, "all checked");
    assert_eq!(requests.len(), ROUNDS + 1);
    assert!(requests[ROUNDS].len() > 100, "{}", requests[ROUNDS].len());
    assert_eq!(runs.load(Ordering::SeqCst), ROUNDS * PARALLEL);
}

#[test]
fn message_cap_drops_earlier_turns_but_never_cuts_into_the_last_one() {
    let mut h = ConversationHistory::new().with_max_messages(5);
    h.add_user_message("earlier");
    h.add_assistant_message("earlier answer");
    h.add_user_message("the question");
    for n in 0..4 {
        let id = format!("c{n}");
        h.add_message(Message::assistant_with_tools(
            "",
            vec![tool_call(&id, "echo", json!({}))],
        ));
        h.add_tool_result(&id, "ok");
    }
    // The last turn alone has 9 messages, over the cap of 5: the earlier turn
    // went, the last one is whole.
    let kept = h.get_conversation_messages();
    assert_eq!(kept.len(), 9, "{:#?}", describe(&kept));
    assert_eq!(kept[0].content, "the question");
    check_valid(&h.get_messages()).unwrap();
}

#[test]
fn dropping_an_earlier_turn_keeps_results_whose_ids_repeat_in_the_last_one() {
    // Ollama numbers tool call ids per reply, so every turn has
    // "ollama_tool_0". Dropping an earlier tool call (here the history opens
    // with one) must not take the last turn's result, which has the same id.
    let mut h = ConversationHistory::new().with_max_messages(4);
    h.add_message(Message::assistant_with_tools(
        "",
        vec![tool_call("ollama_tool_0", "echo", json!({}))],
    ));
    h.add_tool_result("ollama_tool_0", "first result");
    h.add_user_message("second question");
    h.add_message(Message::assistant_with_tools(
        "",
        vec![tool_call("ollama_tool_0", "echo", json!({}))],
    ));
    h.add_tool_result("ollama_tool_0", "second result"); // 5 > 4: trims

    let kept = h.get_conversation_messages();
    check_valid(&h.get_messages()).unwrap_or_else(|e| panic!("{e}: {:#?}", describe(&kept)));
    assert_eq!(kept[0].content, "second question");
    assert_eq!(results_for(&kept, "ollama_tool_0"), vec!["second result"]);
}
