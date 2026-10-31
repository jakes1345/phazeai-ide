//! Stress tests for the agent loop: a long session under context pressure,
//! many tool rounds in one run, Stop pressed in the middle of a busy run,
//! and several agents at once.
//!
//! Everything here runs the production code: `Agent::run_with_events`, the
//! real tools (`ToolRegistry::default()`, `BashTool`), context compaction
//! (`context::compaction`) and the trim behind it, the budget from
//! `Settings::context_budget()` and `GuardedClient`'s audit log. Only the
//! model is scripted. [`ScriptedModel`] plays it the way a provider would:
//! it reads each request the agent sends, finds the current user turn, and
//! makes that turn's next tool calls or answers with the markers it found in
//! the turn's tool results; when compaction asks for a summary, it writes one
//! that merges the previous summary. It records every request, so the tests
//! check what the model was actually sent, not only what the agent returned.
//!
//! No test waits a fixed time for something to happen: Stop is pressed from
//! the event stream (or when compaction's summary request arrives), and the
//! only timing assertions are upper bounds on how long a stopped run takes to
//! return.

use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex as StdMutex};
use std::time::{Duration, Instant};

use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use phazeai_core::config::LlmProvider;
use phazeai_core::context::compaction;
use phazeai_core::llm::Usage;
use phazeai_core::privacy::audit::{self, AuditKind};
use phazeai_core::privacy::GuardedClient;
use phazeai_core::{
    Agent, AgentEvent, AgentResponse, ConversationHistory, LlmClient, LlmResponse, Message,
    PhazeError, ProviderId, Role, Settings, StreamEvent, SystemPromptBuilder, ToolDefinition,
    ToolRegistry,
};
use serde_json::{json, Value};
use tokio::sync::Mutex;

/// Upper bound for a stopped run to return. The agent polls its cancel flag
/// every 50 ms while a tool runs and every 100 ms while compaction waits for
/// the summary; the rest is room for a loaded CI machine.
const STOP_BOUND: Duration = Duration::from_secs(2);

/// A run that takes longer than this is stuck: fail instead of hanging CI.
const RUN_LIMIT: Duration = Duration::from_secs(30);

/// Share of the budget that one generated file's `read_file` result takes.
const FILE_SHARE: f64 = 0.085;

/// Words a slow answer streams before its provider waits for Stop.
const SLOW_HEAD: usize = 40;

// ── Generated workspace ──────────────────────────────────────────────────────

/// One padding line of the generated source files (59 bytes).
const LINE: &str = "    let value = compute(index, &table) + offset; // padding";

/// A temp directory of generated source files. The first line of each file
/// is a marker unique to the workspace and the file, so a tool result shows
/// which file was really read.
struct Workspace {
    dir: tempfile::TempDir,
    tag: String,
}

impl Workspace {
    fn new(tag: &str, files: usize, lines: usize) -> Self {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..files {
            let mut text = format!("// {}\n", marker(tag, i));
            for n in 1..lines {
                text.push_str(&format!("{LINE} {n:04}\n"));
            }
            std::fs::write(dir.path().join(file_name(i)), text).unwrap();
        }
        Self {
            dir,
            tag: tag.to_string(),
        }
    }

    fn path(&self, i: usize) -> String {
        self.dir
            .path()
            .join(file_name(i))
            .to_string_lossy()
            .into_owned()
    }

    fn marker(&self, i: usize) -> String {
        marker(&self.tag, i)
    }

    fn read(&self, i: usize) -> Call {
        ("read_file".to_string(), json!({ "path": self.path(i) }))
    }
}

fn file_name(i: usize) -> String {
    format!("module_{i:03}.rs")
}

fn marker(tag: &str, i: usize) -> String {
    format!("MARK-{tag}-{i:03}")
}

/// Lines per file for a `read_file` result of about `share` of `budget`
/// tokens: each line comes back numbered and JSON-escaped (~74 bytes), and
/// the agent counts ~3 bytes per token.
fn lines_for(budget: usize, share: f64) -> usize {
    (budget as f64 * share * 3.0 / (LINE.len() + 15) as f64) as usize
}

// ── The script a session follows ─────────────────────────────────────────────

/// One tool call: tool name and arguments.
type Call = (String, Value);

/// For each user turn, the tool calls of each assistant message in order.
type Plan = HashMap<u32, Vec<Vec<Call>>>;

/// What the model does in each turn, and which markers each answer lists.
#[derive(Default)]
struct Script {
    plan: Plan,
    expected: HashMap<u32, Vec<String>>,
}

impl Script {
    /// Turn `turn` reads `files` from `ws`, `per_message[k]` of them in its
    /// k-th assistant message, and answers with their markers.
    fn reading_turn(
        &mut self,
        turn: u32,
        ws: &Workspace,
        files: Range<usize>,
        per_message: &[usize],
    ) {
        let mut reads = files.clone().map(|i| ws.read(i));
        let batches = per_message
            .iter()
            .map(|&n| reads.by_ref().take(n).collect())
            .collect();
        assert!(
            reads.next().is_none(),
            "per_message does not cover the files"
        );
        self.plan.insert(turn, batches);
        self.expected
            .insert(turn, files.map(|i| ws.marker(i)).collect());
    }

    fn answer(&self, turn: u32) -> String {
        answer_text(turn, &self.expected[&turn])
    }
}

/// A long reading session over `ws`: each turn reads three new files, two in
/// one message and one in the next. Turn `heavy` reads six in one message,
/// more than compaction's keep-recent allowance; turn `chat_only` uses no tool.
fn reading_session(
    ws: &Workspace,
    turns: u32,
    heavy: Option<u32>,
    chat_only: Option<u32>,
) -> Script {
    let mut script = Script::default();
    let mut next = 0;
    for turn in 1..=turns {
        let per_message: &[usize] = if Some(turn) == heavy {
            &[6]
        } else if Some(turn) == chat_only {
            &[]
        } else {
            &[2, 1]
        };
        let count: usize = per_message.iter().sum();
        script.reading_turn(turn, ws, next..next + count, per_message);
        next += count;
    }
    script
}

fn prompt(turn: u32) -> String {
    format!("turn {turn:02}: read the files planned for this turn and list their markers")
}

fn answer_text(turn: u32, markers: &[String]) -> String {
    format!("turn {turn:02} done: {}", markers.join(" "))
}

/// The number of a "turn NN: ..." message.
fn turn_of(text: &str) -> Option<u32> {
    text.strip_prefix("turn ")?.split(':').next()?.parse().ok()
}

/// Every `MARK-<tag>-<nnn>` in `text`, in order. (A bare `MARK-`, such as
/// the pattern `grep` echoes back, is not a marker.)
fn markers_in(text: &str) -> Vec<String> {
    text.match_indices("MARK-")
        .map(|(at, _)| {
            text[at..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect::<String>()
        })
        .filter(|m| {
            m.rsplit_once('-').is_some_and(|(head, n)| {
                head.len() > "MARK".len() && n.len() == 3 && n.bytes().all(|b| b.is_ascii_digit())
            })
        })
        .collect()
}

fn summary_text(turns: &BTreeSet<u32>) -> String {
    let list: Vec<String> = turns.iter().map(|t| format!("{t:02}")).collect();
    format!("Folded turns: {}", list.join(" "))
}

fn summarised_turns(summary: &str) -> Vec<u32> {
    summary
        .strip_prefix("Folded turns: ")
        .map(|rest| {
            rest.split_whitespace()
                .filter_map(|t| t.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = start + text[start..].find(close)?;
    Some(&text[start..end])
}

// ── The scripted model ───────────────────────────────────────────────────────

/// A request the scripted model received.
#[derive(Debug, Clone)]
enum Seen {
    /// A streamed request from the agent loop, as sent.
    Turn(Vec<Message>),
    /// Compaction asked for a summary and got `reply`.
    Summary {
        /// Turns whose user message was in the transcript to fold.
        folded: Vec<u32>,
        /// The previous summary, as handed back to be merged.
        previous: Option<String>,
        reply: String,
    },
}

type Log = Arc<StdMutex<Vec<Seen>>>;

/// Press Stop when compaction's `nth` summary request arrives (1-based), and
/// never answer that request.
struct StopDuringSummary {
    nth: usize,
    flag: Arc<AtomicBool>,
    pressed: Arc<StdMutex<Option<Instant>>>,
}

/// Deterministic stand-in for a provider; see the module docs.
struct ScriptedModel {
    plan: Plan,
    log: Log,
    summaries: AtomicUsize,
    /// Stream this turn's answer like a provider that is still streaming
    /// when Stop is pressed (see [`slow_stream`]).
    slow_answer: Option<(u32, Arc<AtomicBool>)>,
    stop_during_summary: Option<StopDuringSummary>,
}

enum Step {
    Call {
        turn: u32,
        batch: usize,
        calls: Vec<Call>,
    },
    Answer {
        turn: Option<u32>,
        text: String,
    },
}

impl ScriptedModel {
    fn new(plan: Plan) -> (Self, Log) {
        let log = Log::default();
        let model = Self {
            plan,
            log: log.clone(),
            summaries: AtomicUsize::new(0),
            slow_answer: None,
            stop_during_summary: None,
        };
        (model, log)
    }

    /// What a model reading `messages` does next: the next batch of calls
    /// planned for the current turn, or the answer once they are answered.
    fn next_step(&self, messages: &[Message]) -> Step {
        let Some(start) = messages.iter().rposition(is_plain_user) else {
            return Step::Answer {
                turn: None,
                text: "I was sent no user request".to_string(),
            };
        };
        let Some(turn) = turn_of(&messages[start].content) else {
            return Step::Answer {
                turn: None,
                text: format!("not a turn: {}", messages[start].content),
            };
        };
        let current = &messages[start..];
        let made = current.iter().filter(|m| m.tool_calls.is_some()).count();
        let batches = self.plan.get(&turn).map(Vec::as_slice).unwrap_or_default();
        if let Some(calls) = batches.get(made) {
            return Step::Call {
                turn,
                batch: made,
                calls: calls.clone(),
            };
        }
        let markers: Vec<String> = current
            .iter()
            .filter(|m| m.tool_call_id.is_some())
            .flat_map(|m| markers_in(&m.content))
            .collect();
        Step::Answer {
            turn: Some(turn),
            text: answer_text(turn, &markers),
        }
    }
}

fn send(tx: &UnboundedSender<StreamEvent>, event: StreamEvent) {
    tx.unbounded_send(event).unwrap();
}

#[async_trait::async_trait]
impl LlmClient for ScriptedModel {
    /// The agent calls `chat` only to have compaction's summary written.
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse, PhazeError> {
        let request = match messages {
            [system, user]
                if system.role == Role::System
                    && system.content == compaction::SYSTEM_PROMPT
                    && tools.is_empty() =>
            {
                user.content.as_str()
            }
            _ => {
                return Err(PhazeError::Llm(format!(
                    "unexpected chat request: {messages:?}"
                )))
            }
        };
        let previous =
            between(request, "<previous_summary>\n", "\n</previous_summary>").map(str::to_string);
        let transcript = between(request, "<transcript>\n", "</transcript>").unwrap_or_default();
        let folded: Vec<u32> = transcript
            .split("[User]\n")
            .skip(1)
            .filter_map(turn_of)
            .collect();
        // Merge as a model would: the old summary's turns plus the new ones.
        let mut turns: BTreeSet<u32> = previous
            .as_deref()
            .map(summarised_turns)
            .unwrap_or_default()
            .into_iter()
            .collect();
        turns.extend(&folded);
        let reply = summary_text(&turns);

        let nth = self.summaries.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(stop) = self.stop_during_summary.as_ref().filter(|s| s.nth == nth) {
            *stop.pressed.lock().unwrap() = Some(Instant::now());
            stop.flag.store(true, Ordering::SeqCst);
            std::future::pending::<()>().await;
        }
        self.log.lock().unwrap().push(Seen::Summary {
            folded,
            previous,
            reply: reply.clone(),
        });
        Ok(LlmResponse {
            message: Message::assistant(reply),
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> Result<UnboundedReceiver<StreamEvent>, PhazeError> {
        self.log.lock().unwrap().push(Seen::Turn(messages.to_vec()));
        let (tx, rx) = unbounded();
        match self.next_step(messages) {
            Step::Call { turn, batch, calls } => {
                for (i, (name, args)) in calls.into_iter().enumerate() {
                    let id = format!("t{turn:02}-{batch}-{i}");
                    // Arguments arrive in two pieces, as providers stream them.
                    let args = args.to_string();
                    let mid = (0..=args.len() / 2)
                        .rev()
                        .find(|&at| args.is_char_boundary(at))
                        .unwrap_or(0);
                    send(
                        &tx,
                        StreamEvent::ToolCallStart {
                            id: id.clone(),
                            name,
                        },
                    );
                    for part in [&args[..mid], &args[mid..]] {
                        send(
                            &tx,
                            StreamEvent::ToolCallDelta {
                                id: id.clone(),
                                arguments_delta: part.to_string(),
                            },
                        );
                    }
                    send(&tx, StreamEvent::ToolCallEnd { id });
                }
            }
            Step::Answer { turn, text } => {
                if let Some((_, stop)) = self.slow_answer.as_ref().filter(|(t, _)| Some(*t) == turn)
                {
                    tokio::spawn(slow_stream(tx, stop.clone()));
                    return Ok(rx);
                }
                let chars: Vec<char> = text.chars().collect();
                for chunk in chars.chunks(7) {
                    send(&tx, StreamEvent::TextDelta(chunk.iter().collect()));
                }
            }
        }
        send(
            &tx,
            StreamEvent::Usage(Usage {
                input_tokens: messages.len() as u32,
                output_tokens: 1,
            }),
        );
        send(&tx, StreamEvent::Done);
        Ok(rx)
    }
}

/// A provider still streaming when Stop is pressed: the first words of an
/// answer, then the rest only once `stop` is raised. Ends quietly when the
/// agent drops the stream.
async fn slow_stream(tx: UnboundedSender<StreamEvent>, stop: Arc<AtomicBool>) {
    for i in 0..SLOW_HEAD {
        if tx
            .unbounded_send(StreamEvent::TextDelta(format!("word{i} ")))
            .is_err()
        {
            return;
        }
    }
    // The flag is a plain AtomicBool, as in the IDE, so poll it; nothing
    // depends on the interval.
    while !stop.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    for i in SLOW_HEAD..SLOW_HEAD * 2 {
        if tx
            .unbounded_send(StreamEvent::TextDelta(format!("word{i} ")))
            .is_err()
        {
            return;
        }
    }
    let _ = tx.unbounded_send(StreamEvent::Done);
}

// ── Checks on what the model was sent ────────────────────────────────────────

/// A plain user message: what a request must start with after the system
/// prompt, and how each turn begins.
fn is_plain_user(m: &Message) -> bool {
    m.role == Role::User && m.tool_call_id.is_none() && m.tool_calls.is_none()
}

/// The provider contract for a request (or a history): the system prompt
/// first, then a plain user message; every tool call answered by exactly one
/// result before the next message; no result without its call.
fn check_shape(messages: &[Message]) -> Result<(), String> {
    let Some((system, rest)) = messages.split_first() else {
        return Err("empty request".to_string());
    };
    if system.role != Role::System {
        return Err(format!("does not start with the system prompt: {system:?}"));
    }
    if !rest.first().is_some_and(is_plain_user) {
        return Err(format!(
            "the first message after the system prompt is not a plain user message: {:?}",
            rest.first()
        ));
    }
    let mut waiting: Vec<&str> = Vec::new();
    for (i, m) in rest.iter().enumerate() {
        if let Some(id) = &m.tool_call_id {
            let Some(at) = waiting.iter().position(|w| w == id) else {
                return Err(format!("message {i}: result {id} answers no pending call"));
            };
            waiting.remove(at);
            continue;
        }
        if !waiting.is_empty() {
            return Err(format!(
                "message {i}: calls {waiting:?} were never answered"
            ));
        }
        if m.role == Role::System {
            return Err(format!("message {i}: a second system message"));
        }
        if let Some(calls) = &m.tool_calls {
            waiting = calls.iter().map(|c| c.id.as_str()).collect();
        }
    }
    if waiting.is_empty() {
        Ok(())
    } else {
        Err(format!("calls {waiting:?} were never answered"))
    }
}

/// The agent's own estimate of `messages` (a request: system message first),
/// rebuilt with the production `ConversationHistory`.
fn request_tokens(messages: &[Message], system_prompt: &str, summary: Option<&str>) -> usize {
    let mut history = ConversationHistory::new()
        .with_max_messages(usize::MAX)
        .with_system_prompt(system_prompt);
    if let Some(summary) = summary {
        history.set_summary(summary);
    }
    for m in &messages[1..] {
        history.add_message(m.clone());
    }
    history.estimate_tokens()
}

/// The estimate of `messages` alone, without system prompt or summary.
fn verbatim_tokens(messages: &[Message]) -> usize {
    let mut history = ConversationHistory::new().with_max_messages(usize::MAX);
    for m in messages {
        history.add_message(m.clone());
    }
    history.estimate_tokens()
}

/// Checks every request a session's model received, in order, and returns how
/// many summaries compaction asked for.
///
/// Each streamed request must meet the provider contract, carry the system
/// prompt with the latest summary appended, fit the budget by the agent's own
/// estimate, and hold every turn since that summary verbatim, so summary and
/// verbatim turns cover each turn so far exactly once: nothing was dropped
/// without being summarised. Each summary request must fold the oldest turns
/// not yet folded, whole and in order, and be handed the previous summary.
fn check_session(log: &[Seen], system_prompt: &str, budget: usize) -> usize {
    let mut latest: Option<&str> = None;
    let mut folded_upto = 0;
    let mut summaries = 0;
    for (n, seen) in log.iter().enumerate() {
        match seen {
            Seen::Turn(messages) => {
                if let Err(e) = check_shape(messages) {
                    panic!("request {n} breaks the provider contract: {e}");
                }
                let system = &messages[0].content;
                match latest {
                    None => assert_eq!(system, system_prompt, "request {n}: system message"),
                    Some(summary) => assert!(
                        system.starts_with(system_prompt)
                            && system.ends_with(&format!("\n{summary}")),
                        "request {n}: the system message does not carry {summary:?}"
                    ),
                }
                let tokens = request_tokens(messages, system_prompt, latest);
                assert!(
                    tokens <= budget,
                    "request {n}: {tokens} tokens, over the {budget}-token budget"
                );
                let verbatim: Vec<u32> = messages
                    .iter()
                    .filter(|m| is_plain_user(m))
                    .map(|m| turn_of(&m.content).expect("a turn message"))
                    .collect();
                let current = *verbatim.last().expect("check_shape found a user turn");
                assert_eq!(
                    verbatim,
                    (folded_upto + 1..=current).collect::<Vec<_>>(),
                    "request {n}: turns kept verbatim next to a summary of turns 1-{folded_upto}"
                );
            }
            Seen::Summary {
                folded,
                previous,
                reply,
            } => {
                summaries += 1;
                assert_eq!(
                    previous.as_deref(),
                    latest,
                    "summary request {summaries} was not handed the previous summary"
                );
                let upto = *folded.last().expect("compaction folded no turn");
                assert_eq!(
                    folded,
                    &(folded_upto + 1..=upto).collect::<Vec<_>>(),
                    "summary request {summaries} did not fold the oldest turns, whole and in order"
                );
                assert_eq!(summarised_turns(reply), (1..=upto).collect::<Vec<_>>());
                latest = Some(reply);
                folded_upto = upto;
            }
        }
    }
    summaries
}

fn turn_requests(log: &Log) -> usize {
    log.lock()
        .unwrap()
        .iter()
        .filter(|s| matches!(s, Seen::Turn(_)))
        .count()
}

// ── Running the agent ────────────────────────────────────────────────────────

/// What production gives the built-in local model at its default 16k
/// context: the window minus the reply and tool-schema reserves.
fn local_model_budget() -> usize {
    let mut settings = Settings::default();
    settings.llm.provider = LlmProvider::BuiltIn;
    settings.local_ai.ctx_size = Some(16_384);
    settings.context_budget()
}

/// What production gives a Claude model: a 200k window minus the reserves.
fn cloud_budget() -> usize {
    let mut settings = Settings::default();
    settings.llm.provider = LlmProvider::Claude;
    settings.context_budget()
}

/// The production system prompt for the default tool set.
fn system_prompt() -> String {
    let mut names: Vec<String> = ToolRegistry::default()
        .list()
        .iter()
        .map(|t| t.name().to_string())
        .collect();
    names.sort();
    SystemPromptBuilder::new().with_tools(names).build()
}

/// An agent wired the way the IDE chat and the CLI wire theirs: a tool set, a
/// system prompt, a context budget, a shared conversation and a cancel flag.
fn agent(
    model: Box<dyn LlmClient>,
    tools: ToolRegistry,
    budget: usize,
    conversation: &Arc<Mutex<ConversationHistory>>,
    cancel: &Arc<AtomicBool>,
) -> Agent {
    Agent::new(model)
        .with_tools(tools)
        .with_system_prompt(system_prompt())
        .with_context_budget(budget)
        .with_shared_conversation(conversation.clone())
        .with_cancel_token(cancel.clone())
}

/// Everything one `run_with_events` call produced.
struct TurnRun {
    result: Result<AgentResponse, PhazeError>,
    events: Vec<AgentEvent>,
    /// Index in `events` of the event on which Stop was pressed, and when.
    stopped_at: Option<(usize, Instant)>,
    /// When `run_with_events` returned.
    returned_at: Instant,
}

/// Runs one turn and collects its events. The first event `stop_when`
/// matches raises `flag` at that moment, as the Stop button would.
async fn run_turn(
    agent: &Agent,
    prompt: String,
    flag: Option<&AtomicBool>,
    mut stop_when: impl FnMut(&AgentEvent) -> bool,
) -> TurnRun {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let run = async {
        let result = agent.run_with_events(prompt, tx).await;
        (result, Instant::now())
    };
    let watch = async {
        let mut events = Vec::new();
        let mut stopped_at = None;
        while let Some(event) = rx.recv().await {
            if stopped_at.is_none() && stop_when(&event) {
                if let Some(flag) = flag {
                    flag.store(true, Ordering::SeqCst);
                }
                stopped_at = Some((events.len(), Instant::now()));
            }
            events.push(event);
        }
        (events, stopped_at)
    };
    let ((result, returned_at), (events, stopped_at)) =
        tokio::time::timeout(RUN_LIMIT, async { tokio::join!(run, watch) })
            .await
            .expect("the run never finished");
    TurnRun {
        result,
        events,
        stopped_at,
        returned_at,
    }
}

/// One `AgentEvent::ContextCompacted`.
#[derive(Debug, Clone, Copy)]
struct Compaction {
    folded: usize,
    before: usize,
    after: usize,
}

fn compactions_in(events: &[AgentEvent]) -> Vec<Compaction> {
    events
        .iter()
        .filter_map(|e| match *e {
            AgentEvent::ContextCompacted {
                messages_folded,
                tokens_before,
                tokens_after,
            } => Some(Compaction {
                folded: messages_folded,
                before: tokens_before,
                after: tokens_after,
            }),
            _ => None,
        })
        .collect()
}

/// Runs turn `turn` to its answer and checks the answer and the run; returns
/// the compactions that happened during it.
async fn finish(agent: &Agent, script: &Script, turn: u32) -> Vec<Compaction> {
    let run = run_turn(agent, prompt(turn), None, |_| false).await;
    let response = run
        .result
        .unwrap_or_else(|e| panic!("turn {turn} failed: {e}"));
    assert_eq!(
        response.content,
        script.answer(turn),
        "turn {turn}'s answer"
    );
    let batches = &script.plan[&turn];
    assert_eq!(
        response.iterations,
        batches.len() + 1,
        "turn {turn}'s model calls"
    );
    assert_eq!(
        response.tool_calls.len(),
        batches.iter().map(Vec::len).sum::<usize>()
    );
    assert!(
        response.tool_calls.iter().all(|c| c.success),
        "turn {turn}: {:?}",
        response.tool_calls
    );
    assert!(
        matches!(run.events.last(), Some(AgentEvent::Complete { .. })),
        "turn {turn} ended with {:?}",
        run.events.last()
    );
    assert!(
        !run.events.iter().any(|e| matches!(e, AgentEvent::Error(_))),
        "turn {turn}: {:?}",
        run.events
    );
    compactions_in(&run.events)
}

fn assert_stopped(run: &TurnRun, pressed: Instant) {
    assert!(
        matches!(run.result, Err(PhazeError::Cancelled)),
        "expected the run to stop, got {:?}",
        run.result
    );
    let reaction = run.returned_at.saturating_duration_since(pressed);
    assert!(reaction < STOP_BOUND, "the run took {reaction:?} to stop");
    assert!(
        matches!(run.events.last(), Some(AgentEvent::Error(m)) if m == "Cancelled"),
        "last event: {:?}",
        run.events.last()
    );
    assert!(
        !run.events
            .iter()
            .any(|e| matches!(e, AgentEvent::Complete { .. })),
        "a stopped run reported Complete"
    );
}

fn summaries_in(log: &Log) -> usize {
    log.lock()
        .unwrap()
        .iter()
        .filter(|s| matches!(s, Seen::Summary { .. }))
        .count()
}

// ── (a) Context pressure ─────────────────────────────────────────────────────

/// Sixteen turns whose tool output adds up to several budgets: compaction runs
/// again and again, every request stays within the budget with every call
/// answered, the summary rides in the system prompt, no turn is lost, and
/// every answer still lists exactly the files its turn read.
#[tokio::test]
async fn long_session_under_context_pressure_compacts_repeatedly_and_loses_no_turn() {
    const TURNS: u32 = 16;
    const HEAVY: u32 = 9;
    const CHAT_ONLY: u32 = 5;
    let budget = local_model_budget();
    let system = system_prompt();
    let system_tokens = ConversationHistory::new()
        .with_system_prompt(system.clone())
        .estimate_tokens();
    assert!(
        system_tokens * 4 < budget,
        "the system prompt ({system_tokens} tokens) no longer leaves room for the heavy turn"
    );

    let ws = Workspace::new("long", 50, lines_for(budget, FILE_SHARE));
    let script = reading_session(&ws, TURNS, Some(HEAVY), Some(CHAT_ONLY));
    let read_bytes: u64 = (0..script.expected.values().map(Vec::len).sum())
        .map(|i| std::fs::metadata(ws.path(i)).unwrap().len())
        .sum();
    assert!(
        read_bytes / 3 > 3 * budget as u64,
        "the session must read several budgets' worth of files"
    );

    let (model, log) = ScriptedModel::new(script.plan.clone());
    let conversation = Arc::new(Mutex::new(ConversationHistory::new()));
    let cancel = Arc::new(AtomicBool::new(false));
    let agent = agent(
        Box::new(model),
        ToolRegistry::default(),
        budget,
        &conversation,
        &cancel,
    );

    let mut compactions = Vec::new();
    for turn in 1..=TURNS {
        compactions.extend(finish(&agent, &script, turn).await);
    }

    let log = log.lock().unwrap().clone();
    let summaries = check_session(&log, &system, budget);
    assert!(summaries >= 3, "compaction ran {summaries} times");
    assert_eq!(
        compactions.len(),
        summaries,
        "a compaction failed: {compactions:?}"
    );
    for c in &compactions {
        assert!(c.folded > 0 && c.after < c.before, "{c:?}");
        assert!(c.after <= budget, "over budget after compacting: {c:?}");
    }

    // The heavy turn alone is over compaction's keep-recent allowance. The
    // request that answered it still held the whole turn, and nothing older.
    let heavy_answer = log
        .iter()
        .filter_map(|seen| match seen {
            Seen::Turn(m) => Some(m),
            Seen::Summary { .. } => None,
        })
        .find(|m| {
            let start = m.iter().rposition(is_plain_user).unwrap();
            turn_of(&m[start].content) == Some(HEAVY)
                && m[start..]
                    .iter()
                    .filter(|x| x.tool_call_id.is_some())
                    .count()
                    == 6
        })
        .expect("no request carried the heavy turn's six results");
    assert_eq!(turn_of(&heavy_answer[1].content), Some(HEAVY));
    let kept = verbatim_tokens(&heavy_answer[1..]);
    assert!(
        kept as f64 > budget as f64 * compaction::KEEP_RECENT_RATIO,
        "the heavy turn ({kept} tokens) is not over the keep-recent allowance"
    );

    // The session ends with the last summary in place, every folded message
    // counted, and the summary plus the verbatim turns covering all turns.
    let conv = conversation.lock().await;
    let last = log
        .iter()
        .rev()
        .find_map(|seen| match seen {
            Seen::Summary { reply, .. } => Some(reply.as_str()),
            Seen::Turn(_) => None,
        })
        .unwrap();
    assert_eq!(conv.summary(), Some(last));
    assert_eq!(
        conv.folded_count(),
        compactions.iter().map(|c| c.folded).sum::<usize>()
    );
    let history = conv.get_messages();
    check_shape(&history).unwrap();
    let mut covered = summarised_turns(last);
    covered.extend(
        history
            .iter()
            .filter(|m| is_plain_user(m))
            .filter_map(|m| turn_of(&m.content)),
    );
    assert_eq!(covered, (1..=TURNS).collect::<Vec<_>>());
    assert!(conv.estimate_tokens() <= budget);
}

// ── (b) Many iterations ──────────────────────────────────────────────────────

/// One run of forty-five tool rounds (three times the default cap, which this
/// test raises) through three real tools: every result reaches the model, the
/// loop's events, history and token accounting stay exact, and the answer
/// arrives. Forty-five rounds (93 messages) stay under `ConversationHistory`'s
/// 100-message cap.
#[tokio::test]
async fn forty_five_rounds_in_one_run_keep_exact_bookkeeping() {
    const ROUNDS: usize = 45;
    let ws = Workspace::new("loop", ROUNDS, 4);
    let dir = ws.dir.path().to_string_lossy().into_owned();
    // Rotate read_file, grep and list_files; the first two return the marker.
    let mut batches = Vec::new();
    let mut markers = Vec::new();
    for i in 0..ROUNDS {
        let call = match i % 3 {
            0 => ws.read(i),
            1 => (
                "grep".to_string(),
                json!({ "pattern": "MARK-", "path": ws.path(i) }),
            ),
            _ => ("list_files".to_string(), json!({ "path": dir })),
        };
        if i % 3 != 2 {
            markers.push(ws.marker(i));
        }
        batches.push(vec![call]);
    }
    let names: Vec<String> = batches.iter().map(|b| b[0].0.clone()).collect();
    let mut script = Script::default();
    script.plan.insert(1, batches);
    script.expected.insert(1, markers);

    let (model, log) = ScriptedModel::new(script.plan.clone());
    let conversation = Arc::new(Mutex::new(ConversationHistory::new()));
    let cancel = Arc::new(AtomicBool::new(false));
    let agent = agent(
        Box::new(model),
        ToolRegistry::default(),
        cloud_budget(),
        &conversation,
        &cancel,
    )
    .with_max_iterations(ROUNDS + 1);

    let run = run_turn(&agent, prompt(1), None, |_| false).await;
    let response = run.result.expect("the run failed");
    assert_eq!(response.content, script.answer(1));
    assert_eq!(response.iterations, ROUNDS + 1);
    let called: Vec<&str> = response
        .tool_calls
        .iter()
        .map(|c| c.tool_name.as_str())
        .collect();
    assert_eq!(called, names);
    assert!(
        response.tool_calls.iter().all(|c| c.success),
        "{:?}",
        response.tool_calls
    );

    // Each round: Thinking, ToolStart, a successful ToolResult; then the
    // answer's Thinking, the usage report and Complete.
    let mut shape = Vec::new();
    let mut iterations = Vec::new();
    for event in &run.events {
        match event {
            AgentEvent::Thinking { iteration } => {
                iterations.push(*iteration);
                shape.push("thinking");
            }
            AgentEvent::ToolStart { .. } => shape.push("start"),
            AgentEvent::ToolResult { success: true, .. } => shape.push("result"),
            AgentEvent::TextDelta(_) => {}
            AgentEvent::TokenUsage { .. } => shape.push("usage"),
            AgentEvent::Complete { .. } => shape.push("complete"),
            other => panic!("unexpected event {other:?}"),
        }
    }
    let mut expected = Vec::new();
    for _ in 0..ROUNDS {
        expected.extend(["thinking", "start", "result"]);
    }
    expected.extend(["thinking", "usage", "complete"]);
    assert_eq!(shape, expected);
    assert_eq!(iterations, (1..=ROUNDS + 1).collect::<Vec<_>>());

    // The model was asked ROUNDS + 1 times, each request one call and one
    // result longer than the one before: nothing dropped, nothing repeated.
    let requests: Vec<Vec<Message>> = log
        .lock()
        .unwrap()
        .iter()
        .map(|s| match s {
            Seen::Turn(m) => m.clone(),
            Seen::Summary { .. } => panic!("nothing should be compacted here"),
        })
        .collect();
    assert_eq!(requests.len(), ROUNDS + 1);
    for (k, request) in requests.iter().enumerate() {
        check_shape(request).unwrap_or_else(|e| panic!("request {k}: {e}"));
        assert_eq!(request.len(), 2 + 2 * k, "request {k}");
    }

    // The totals are the sums of what the provider reported on each call.
    let input: u64 = requests.iter().map(|r| r.len() as u64).sum();
    assert_eq!(response.total_input_tokens, input);
    assert_eq!(response.total_output_tokens, (ROUNDS + 1) as u64);
    assert!(run.events.iter().any(|e| matches!(
        e,
        AgentEvent::TokenUsage { input_tokens, output_tokens }
            if *input_tokens == input && *output_tokens == (ROUNDS + 1) as u64
    )));

    let history = agent.get_conversation_history().await;
    assert_eq!(history.len(), 2 + 2 * ROUNDS + 1);
    check_shape(&history).unwrap();
}

/// A model that never stops calling tools is cut off at the production
/// default of 15 iterations, with every call it made answered.
#[tokio::test]
async fn a_model_that_never_stops_calling_tools_is_cut_off_at_the_default_cap() {
    let ws = Workspace::new("runaway", 40, 4);
    let mut script = Script::default();
    script.reading_turn(1, &ws, 0..40, &[1; 40]);
    let (model, log) = ScriptedModel::new(script.plan.clone());
    let conversation = Arc::new(Mutex::new(ConversationHistory::new()));
    let cancel = Arc::new(AtomicBool::new(false));
    let agent = agent(
        Box::new(model),
        ToolRegistry::default(),
        cloud_budget(),
        &conversation,
        &cancel,
    );

    let run = run_turn(&agent, prompt(1), None, |_| false).await;
    assert!(
        matches!(run.result, Err(PhazeError::MaxIterations(15))),
        "{:?}",
        run.result
    );
    assert_eq!(turn_requests(&log), 15);
    let results = run
        .events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolResult { success: true, .. }))
        .count();
    assert_eq!(results, 15);
    assert!(matches!(
        run.events.last(),
        Some(AgentEvent::Error(m)) if m == "Exceeded maximum iterations (15)"
    ));
    let history = agent.get_conversation_history().await;
    assert_eq!(history.len(), 2 + 2 * 15);
    check_shape(&history).unwrap();
}

// ── (c) Stop under load ──────────────────────────────────────────────────────

/// Stop while a real shell command runs, eleven model calls into a long turn
/// of a session that has been compacting: the run returns within the bound,
/// the command's call is answered as interrupted, nothing runs after Stop, and
/// the next turn carries on from a history the provider accepts.
#[cfg(unix)]
#[tokio::test]
async fn stop_while_a_shell_command_runs_deep_in_a_long_turn() {
    let budget = local_model_budget();
    let ws = Workspace::new("files", 9, lines_for(budget, FILE_SHARE));
    let small = Workspace::new("small", 26, 3);
    let mut script = reading_session(&ws, 2, None, None);
    // Turn 3: twenty reads, two per message, then `sleep 30`, then three more
    // messages of reads: fifteen model calls in all, the default cap.
    let mut batches: Vec<Vec<Call>> = (0..13)
        .map(|b| vec![small.read(2 * b), small.read(2 * b + 1)])
        .collect();
    batches.insert(
        10,
        vec![("bash".to_string(), json!({ "command": "sleep 30" }))],
    );
    script.plan.insert(3, batches);
    script.reading_turn(4, &ws, 6..9, &[2, 1]);

    let (model, log) = ScriptedModel::new(script.plan.clone());
    let conversation = Arc::new(Mutex::new(ConversationHistory::new()));
    let cancel = Arc::new(AtomicBool::new(false));
    let mut tools = ToolRegistry::default();
    tools.register(Box::new(phazeai_core::tools::BashTool::new(
        small.dir.path().to_path_buf(),
    )));
    let agent = agent(Box::new(model), tools, budget, &conversation, &cancel);

    finish(&agent, &script, 1).await;
    finish(&agent, &script, 2).await;
    let asked_before = turn_requests(&log);

    let run = run_turn(
        &agent,
        prompt(3),
        Some(&cancel),
        |e| matches!(e, AgentEvent::ToolStart { name } if name == "bash"),
    )
    .await;
    let (stop_index, pressed) = run.stopped_at.expect("the shell command never started");
    assert_stopped(&run, pressed);
    assert!(summaries_in(&log) >= 1, "nothing was compacted before Stop");
    // Nothing started after Stop, and the model was not asked again: ten
    // messages of reads and the command made 11 requests this turn.
    let after = &run.events[stop_index + 1..];
    assert!(
        !after.iter().any(|e| matches!(
            e,
            AgentEvent::Thinking { .. } | AgentEvent::ToolStart { .. }
        )),
        "{after:?}"
    );
    assert_eq!(turn_requests(&log) - asked_before, 11);
    let reads = run
        .events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolResult { name, success: true, .. } if name == "read_file"))
        .count();
    assert_eq!(reads, 20);
    {
        let conv = conversation.lock().await;
        let history = conv.get_messages();
        check_shape(&history).unwrap_or_else(|e| panic!("history after Stop: {e}"));
        let last = history.last().unwrap();
        assert_eq!(last.tool_call_id.as_deref(), Some("t03-10-0"));
        assert_eq!(
            last.content,
            "Interrupted by the user before the tool finished."
        );
    }

    // As the IDE and the CLI do: clear the flag and send the next message.
    cancel.store(false, Ordering::SeqCst);
    finish(&agent, &script, 4).await;
    let log = log.lock().unwrap().clone();
    check_session(&log, &system_prompt(), budget);
}

/// Stop while the answer streams: the run returns at the next delta, no
/// half-written answer lands in the history, and the next turn works.
#[tokio::test]
async fn stop_while_the_answer_streams_leaves_no_partial_answer() {
    let budget = local_model_budget();
    let ws = Workspace::new("stream", 12, lines_for(budget, FILE_SHARE));
    let script = reading_session(&ws, 4, None, None);
    let cancel = Arc::new(AtomicBool::new(false));
    let (mut model, log) = ScriptedModel::new(script.plan.clone());
    model.slow_answer = Some((3, cancel.clone()));
    let conversation = Arc::new(Mutex::new(ConversationHistory::new()));
    let agent = agent(
        Box::new(model),
        ToolRegistry::default(),
        budget,
        &conversation,
        &cancel,
    );

    finish(&agent, &script, 1).await;
    finish(&agent, &script, 2).await;
    let mut words = 0;
    let run = run_turn(&agent, prompt(3), Some(&cancel), |e| {
        words += usize::from(matches!(e, AgentEvent::TextDelta(_)));
        words == SLOW_HEAD
    })
    .await;
    let (stop_index, pressed) = run.stopped_at.expect("the answer never streamed");
    assert_stopped(&run, pressed);
    assert!(
        !run.events[stop_index + 1..]
            .iter()
            .any(|e| matches!(e, AgentEvent::TextDelta(_))),
        "words were shown after Stop"
    );
    {
        let conv = conversation.lock().await;
        let history = conv.get_messages();
        check_shape(&history).unwrap_or_else(|e| panic!("history after Stop: {e}"));
        let last = history.last().unwrap();
        assert_eq!(last.tool_call_id.as_deref(), Some("t03-1-0"), "{last:?}");
        assert!(!history.iter().any(|m| m.content.contains("word0 ")));
    }

    cancel.store(false, Ordering::SeqCst);
    finish(&agent, &script, 4).await;
    let log = log.lock().unwrap().clone();
    check_session(&log, &system_prompt(), budget);
}

/// Stop while compaction waits for the model's summary: the run returns within
/// the bound, the half-done compaction changes nothing, and the next turns
/// compact and answer as if it had never started.
#[tokio::test]
async fn stop_while_compaction_writes_the_summary_changes_nothing() {
    let budget = local_model_budget();
    let ws = Workspace::new("compact", 45, lines_for(budget, FILE_SHARE));
    let script = reading_session(&ws, 14, None, None);
    let cancel = Arc::new(AtomicBool::new(false));
    let pressed = Arc::new(StdMutex::new(None));
    let (mut model, log) = ScriptedModel::new(script.plan.clone());
    model.stop_during_summary = Some(StopDuringSummary {
        nth: 2,
        flag: cancel.clone(),
        pressed: pressed.clone(),
    });
    let conversation = Arc::new(Mutex::new(ConversationHistory::new()));
    let agent = agent(
        Box::new(model),
        ToolRegistry::default(),
        budget,
        &conversation,
        &cancel,
    );

    // Turns run normally until compaction asks for its second summary.
    let mut first = Vec::new();
    let mut stopped = None;
    for turn in 1..=12 {
        let run = run_turn(&agent, prompt(turn), None, |_| false).await;
        if run.result.is_err() {
            stopped = Some((turn, run));
            break;
        }
        let response = run.result.as_ref().unwrap();
        assert_eq!(
            response.content,
            script.answer(turn),
            "turn {turn}'s answer"
        );
        first.extend(compactions_in(&run.events));
    }
    let (turn, run) = stopped.expect("compaction never asked for a second summary");
    let pressed = pressed.lock().unwrap().expect("Stop was never pressed");
    assert_stopped(&run, pressed);
    assert!(compactions_in(&run.events).is_empty());
    assert_eq!(first.len(), 1, "{first:?}");
    {
        // Still the first summary and the first fold, and a history the
        // provider accepts that ends with what the stopped turn had done.
        let conv = conversation.lock().await;
        let summary = log.lock().unwrap().iter().find_map(|s| match s {
            Seen::Summary { reply, .. } => Some(reply.clone()),
            Seen::Turn(_) => None,
        });
        assert_eq!(conv.summary(), summary.as_deref());
        assert_eq!(conv.folded_count(), first[0].folded);
        let history = conv.get_messages();
        check_shape(&history).unwrap_or_else(|e| panic!("history after Stop: {e}"));
        let start = history.iter().rposition(is_plain_user).unwrap();
        assert_eq!(turn_of(&history[start].content), Some(turn));
    }

    cancel.store(false, Ordering::SeqCst);
    let mut later = Vec::new();
    for next in turn + 1..=turn + 2 {
        later.extend(finish(&agent, &script, next).await);
    }
    assert!(!later.is_empty(), "the next turns never compacted");
    let log = log.lock().unwrap().clone();
    assert_eq!(
        check_session(&log, &system_prompt(), budget),
        1 + later.len()
    );
}

// ── (d) Agents at once ───────────────────────────────────────────────────────

/// What one agent's thread reports back.
struct Threaded {
    model: String,
    log: Vec<Seen>,
    files: BTreeSet<String>,
    _workspace: Workspace,
}

/// One agent, ready to run its session on the thread that built it.
struct Prepared {
    ws: Workspace,
    script: Script,
    log: Log,
    name: String,
    cancel: Arc<AtomicBool>,
    agent: Agent,
    rt: tokio::runtime::Runtime,
}

fn prepare_session(k: usize, turns: u32, stop_at: Option<u32>) -> Prepared {
    let budget = local_model_budget();
    let ws = Workspace::new(
        &format!("agent{k}"),
        3 * turns as usize,
        lines_for(budget, FILE_SHARE),
    );
    let script = reading_session(&ws, turns, None, None);
    let cancel = Arc::new(AtomicBool::new(false));
    let (mut model, log) = ScriptedModel::new(script.plan.clone());
    if let Some(turn) = stop_at {
        model.slow_answer = Some((turn, cancel.clone()));
    }
    // Production wraps every client in GuardedClient, which writes the audit log.
    let name = format!("stress-agent-{k}");
    let guarded = GuardedClient::new(
        Box::new(model),
        ProviderId::Ollama,
        name.clone(),
        "http://127.0.0.1:11434",
    );
    let conversation = Arc::new(Mutex::new(ConversationHistory::new()));
    let agent = agent(
        Box::new(guarded),
        ToolRegistry::default(),
        budget,
        &conversation,
        &cancel,
    );
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    Prepared {
        ws,
        script,
        log,
        name,
        cancel,
        agent,
        rt,
    }
}

/// One agent's session on its own thread and runtime, as the IDE runs each
/// chat request. With `stop_at`, Stop is pressed while that turn's answer
/// streams, and the session carries on with the next turn.
fn threaded_session(k: usize, turns: u32, stop_at: Option<u32>, start: Arc<Barrier>) -> Threaded {
    // Every thread reaches the barrier, even one whose setup failed, so that
    // failure cannot leave the others waiting forever.
    let setup = std::panic::catch_unwind(|| prepare_session(k, turns, stop_at));
    start.wait();
    let Prepared {
        ws,
        script,
        log,
        name,
        cancel,
        agent,
        rt,
    } = setup.unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    let budget = local_model_budget();
    let compactions = rt.block_on(async {
        let mut compactions = 0;
        for turn in 1..=turns {
            if Some(turn) == stop_at {
                let mut words = 0;
                let run = run_turn(&agent, prompt(turn), Some(&cancel), |e| {
                    words += usize::from(matches!(e, AgentEvent::TextDelta(_)));
                    words == SLOW_HEAD
                })
                .await;
                let (_, pressed) = run.stopped_at.expect("the answer never streamed");
                assert_stopped(&run, pressed);
                compactions += compactions_in(&run.events).len();
                cancel.store(false, Ordering::SeqCst);
                continue;
            }
            compactions += finish(&agent, &script, turn).await.len();
        }
        compactions
    });

    let log = log.lock().unwrap().clone();
    let summaries = check_session(&log, &system_prompt(), budget);
    assert_eq!(summaries, compactions, "agent {k}: a compaction failed");
    assert!(summaries >= 2, "agent {k} compacted {summaries} times");
    // Its model never saw another agent's files.
    let own = format!("MARK-{}-", ws.tag);
    for seen in &log {
        if let Seen::Turn(messages) = seen {
            for m in messages {
                for found in markers_in(&m.content) {
                    assert!(found.starts_with(&own), "agent {k} was sent {found}");
                }
            }
        }
    }
    let files = (0..3 * turns as usize)
        .map(|i| canonical(ws.path(i)))
        .collect();
    Threaded {
        model: name,
        log,
        files,
        _workspace: ws,
    }
}

/// `path` resolved the way the file tools resolve it before noting it.
fn canonical(path: impl AsRef<std::path::Path>) -> String {
    std::fs::canonicalize(path)
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

/// Five agents on their own threads run long, compacting sessions at once,
/// and one of them is stopped mid-session. Each gets every answer right, sees
/// only its own files and compacts only its own history; the stopped one
/// returns within the bound and carries on. The audit log records every
/// request under the agent that sent it, with exactly that agent's files.
#[test]
fn concurrent_sessions_on_separate_threads_stay_separate() {
    const AGENTS: usize = 5;
    const TURNS: u32 = 8;
    let start = Arc::new(Barrier::new(AGENTS));
    let threads: Vec<_> = (0..AGENTS)
        .map(|k| {
            let start = start.clone();
            let stop_at = (k == AGENTS - 1).then_some(3);
            std::thread::spawn(move || threaded_session(k, TURNS, stop_at, start))
        })
        .collect();
    let sessions: Vec<Threaded> = threads
        .into_iter()
        .map(|t| t.join().expect("a session thread panicked"))
        .collect();

    // Only this test writes audit entries in this binary, and the log keeps
    // the last CAPACITY of them.
    let requests: usize = sessions.iter().map(|s| s.log.len()).sum();
    assert!(
        requests <= audit::CAPACITY,
        "{requests} requests overflow the audit log"
    );
    let entries = audit::recent(audit::CAPACITY);
    for s in &sessions {
        let mine: Vec<&Vec<String>> = entries
            .iter()
            .filter_map(|e| match &e.kind {
                AuditKind::Request { model, files, .. } if *model == s.model => Some(files),
                _ => None,
            })
            .collect();
        assert_eq!(mine.len(), s.log.len(), "{}: requests recorded", s.model);
        let attributed: BTreeSet<String> = mine.into_iter().flatten().map(canonical).collect();
        assert_eq!(attributed, s.files, "{}: files attributed", s.model);
    }
}
