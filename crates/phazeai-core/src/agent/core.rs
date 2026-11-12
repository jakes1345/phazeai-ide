use crate::agent_event::AgentEvent;
use crate::checkpoint::Checkpoints;
use crate::context::ConversationHistory;
use crate::error::PhazeError;
use crate::llm::{FunctionCall, LlmClient, Message, StreamEvent, ToolCall};
use crate::tools::{
    render, FileWrite, MoreToolsCall, ToolDefinition, ToolRegistry, ToolResult, MORE_TOOLS,
};
use futures::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;

/// How many model turns one run may take before it stops with
/// [`PhazeError::MaxIterations`], unless `[agent] max_iterations` in
/// `config.toml` (or [`Agent::with_max_iterations`]) says otherwise. A turn
/// is one model response plus the tool calls it asked for, so a real task
/// (read, search, edit, build, test, fix) takes dozens; the limit is a guard
/// against a model that loops, not a budget for ordinary work.
pub const DEFAULT_MAX_ITERATIONS: usize = 200;

#[derive(Debug, Clone)]
pub struct AgentResponse {
    pub content: String,
    pub tool_calls: Vec<ToolExecution>,
    pub iterations: usize,
    /// Cumulative token usage across all LLM calls in this run.
    /// `total_input_tokens` counts every prompt token sent, including those
    /// the provider served from its prompt cache.
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    /// Of `total_input_tokens`, the tokens the provider read from its prompt
    /// cache, and the tokens it wrote to it (see [`crate::llm::Usage`]).
    pub total_cache_read_tokens: u64,
    pub total_cache_write_tokens: u64,
}

/// Callback invoked before tool execution. Returns true to approve, false to deny.
pub type ApprovalFn = Box<
    dyn Fn(String, serde_json::Value) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync,
>;

/// Reviews a file write before it happens. With a diff hook, every call of a
/// tool that writes one file ([`crate::tools::Tool::plan_write`]: `write_file`,
/// `edit_file`) and would change it is shown here first, as (path, before,
/// after): the file that will be written (resolved), what it holds now and
/// exactly what it will hold. The answer ([`DiffAnswer`]) writes all of
/// `after`, none of it, or only some of its hunks ([`crate::review`]); what
/// is written is written only if the file still holds `before`. A call that
/// would fail, or would change nothing, is not shown (and writes nothing
/// different).
pub type DiffHookFn = Box<
    dyn Fn(String, String, String) -> Pin<Box<dyn Future<Output = DiffAnswer> + Send>>
        + Send
        + Sync,
>;

/// A reviewer's answer to a proposed file write ([`DiffHookFn`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffAnswer {
    /// Write the whole change.
    Approve,
    /// Write nothing; the call fails as rejected.
    Reject,
    /// Write only the hunks marked true, by index in
    /// [`crate::review::hunks`]`(before, after)`; the rest stay as they are.
    /// The model is told which parts were not written.
    Keep(Vec<bool>),
}

impl From<bool> for DiffAnswer {
    fn from(approved: bool) -> Self {
        if approved {
            Self::Approve
        } else {
            Self::Reject
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolExecution {
    pub tool_name: String,
    pub params: Value,
    pub success: bool,
    pub result_summary: String,
}

pub struct Agent {
    llm: Box<dyn LlmClient>,
    tools: ToolRegistry,
    conversation: Arc<Mutex<ConversationHistory>>,
    max_iterations: usize,
    max_context_tokens: usize,
    approval_fn: Option<ApprovalFn>,
    diff_hook: Option<DiffHookFn>,
    /// Optional cancellation token — set to `true` to abort the running loop.
    cancel_token: Option<Arc<AtomicBool>>,
    /// Filled for the duration of `run_with_events`; MCP bridges emit here.
    mcp_event_sink: Arc<StdMutex<Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>>>,
    /// Builder-provided system prompt, applied at run start under the async lock.
    system_prompt: Option<String>,
    /// Records each run's file writes so they can be undone.
    checkpoints: Option<Arc<StdMutex<Checkpoints>>>,
}

struct McpEventSinkGuard(Arc<StdMutex<Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>>>);

impl Drop for McpEventSinkGuard {
    fn drop(&mut self) {
        if let Ok(mut g) = self.0.lock() {
            *g = None;
        }
    }
}

impl Agent {
    pub fn new(llm: Box<dyn LlmClient>) -> Self {
        Self {
            llm,
            tools: ToolRegistry::default(),
            conversation: Arc::new(Mutex::new(ConversationHistory::new())),
            max_iterations: DEFAULT_MAX_ITERATIONS,
            max_context_tokens: 32768,
            approval_fn: None,
            diff_hook: None,
            cancel_token: None,
            mcp_event_sink: Arc::new(StdMutex::new(None)),
            system_prompt: None,
            checkpoints: None,
        }
    }

    /// Attach a cancellation token. Set the `AtomicBool` to `true` from any
    /// thread to stop the run within about 0.1 s, whatever it is waiting on:
    /// the provider (even one that sends nothing), a summary, a tool, or an
    /// approval or diff review. The run then returns
    /// [`PhazeError::Cancelled`], and every tool call the model asked for has
    /// a result in the conversation (an interrupted one says so, one that
    /// never started says it did not run), so a shared conversation stays
    /// valid to send.
    pub fn with_cancel_token(mut self, token: Arc<AtomicBool>) -> Self {
        self.cancel_token = Some(token);
        self
    }

    /// Returns a clone of the cancellation token, if one was attached.
    /// The caller can store this and call `.store(true, Ordering::Relaxed)` to cancel.
    pub fn cancel_token(&self) -> Option<Arc<AtomicBool>> {
        self.cancel_token.clone()
    }

    fn is_cancelled(&self) -> bool {
        self.cancel_token
            .as_ref()
            .map(|t| t.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    /// Resolves once the run is cancelled; never, without a cancel token.
    async fn stopped(&self) {
        match &self.cancel_token {
            Some(token) => cancelled(token).await,
            None => std::future::pending().await,
        }
    }

    /// `work`'s output, or `None` if the run is cancelled first. The token is
    /// read before `work` is first polled and then every 50 ms; once it is
    /// seen set, `work` is dropped, which stops what it started. (So `work`
    /// can still finish in the moments after Stop: callers that must not act
    /// on such a result check [`Self::is_cancelled`] again.)
    async fn unless_cancelled<F: Future>(&self, work: F) -> Option<F::Output> {
        tokio::select! {
            biased;
            _ = self.stopped() => None,
            out = work => Some(out),
        }
    }

    /// End a run stopped during a tool round. The calls in `not_run` get a
    /// result saying so first: providers reject a conversation with a tool
    /// call that has no result, and a shared conversation is sent again by
    /// the next run.
    async fn stop_round(
        &self,
        not_run: &[ToolCall],
        event_tx: &tokio::sync::mpsc::UnboundedSender<AgentEvent>,
    ) -> Result<AgentResponse, PhazeError> {
        {
            let mut conversation = self.conversation.lock().await;
            for call in not_run {
                conversation.add_tool_result(&call.id, NOT_RUN);
            }
        }
        cancelled_run(event_tx)
    }

    pub fn with_tools(mut self, tools: ToolRegistry) -> Self {
        self.tools = tools;
        self
    }

    /// The most model turns a run may take (at least 1).
    pub fn with_max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = max.max(1);
        self
    }

    pub fn with_context_budget(mut self, budget: usize) -> Self {
        self.max_context_tokens = budget;
        self
    }

    /// Change the context budget of a running agent (e.g. after a model switch).
    pub fn set_context_budget(&mut self, budget: usize) {
        self.max_context_tokens = budget;
    }

    pub fn with_approval(mut self, f: ApprovalFn) -> Self {
        self.approval_fn = Some(f);
        self
    }

    pub fn with_diff_hook(mut self, f: DiffHookFn) -> Self {
        self.diff_hook = Some(f);
        self
    }

    /// Replace the internal conversation with a shared one, enabling history persistence
    /// across multiple `run_with_events` calls from different Agent instances.
    /// Record what each run writes with `write_file` and `edit_file` in
    /// `checkpoints`, so [`Checkpoints::undo_last`] can put the files back.
    /// Share one across the agents of a conversation.
    pub fn with_checkpoints(mut self, checkpoints: Arc<StdMutex<Checkpoints>>) -> Self {
        self.checkpoints = Some(checkpoints);
        self
    }

    pub fn with_shared_conversation(mut self, conv: Arc<Mutex<ConversationHistory>>) -> Self {
        self.conversation = conv;
        self
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        // Store and apply at run start to avoid races with async lock acquisition.
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Change the system prompt of an agent between runs (e.g. rebuilt for a
    /// new model's context budget); the next run sends it.
    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) {
        self.system_prompt = Some(prompt.into());
    }

    pub fn register_tool(&mut self, tool: Box<dyn crate::tools::Tool>) {
        self.tools.register(tool);
    }

    /// Add a tool the model reaches through `more_tools` instead of seeing
    /// its schema on every request (see [`ToolRegistry`]).
    pub fn register_deferred_tool(&mut self, tool: Box<dyn crate::tools::Tool>) {
        self.tools.register_deferred(tool);
    }

    /// Offer the `subagent` tool, behind `more_tools`: the model can hand a
    /// self-contained question to a helper run as `config` says, and gets
    /// back only its answer (see [`crate::agent::SubagentTool`]). The
    /// helper's model calls come through this agent's events as
    /// [`AgentEvent::SubagentUsage`]. Calling it again replaces the config.
    pub fn register_subagents(&mut self, config: super::SubagentConfig) {
        self.tools
            .register_deferred(Box::new(super::SubagentTool::new(
                config,
                self.mcp_event_sink.clone(),
            )));
    }

    pub fn with_subagents(mut self, config: super::SubagentConfig) -> Self {
        self.register_subagents(config);
        self
    }

    pub fn register_mcp_tools(
        &mut self,
        manager: std::sync::Arc<std::sync::Mutex<crate::mcp::McpManager>>,
    ) {
        use crate::tools::mcp_bridge::create_mcp_tool_bridges;
        let bridges = create_mcp_tool_bridges(manager, Some(self.mcp_event_sink.clone()));
        let count = bridges.len();
        // Held back: a server can bring dozens of tools, each schema sent
        // with every request otherwise; a line apiece is enough to find them.
        for bridge in bridges {
            self.tools.register_deferred(bridge);
        }
        if count > 0 {
            tracing::info!("Registered {count} MCP tools into tool registry");
        }
    }

    pub fn swap_llm(&mut self, new_llm: Box<dyn LlmClient>) {
        self.llm = new_llm;
    }

    /// Run the agent loop, returning the final response.
    pub async fn run(&self, user_input: impl Into<String>) -> Result<AgentResponse, PhazeError> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.run_with_events(user_input, tx).await
    }

    /// Run the agent loop, emitting AgentEvents through the channel.
    ///
    /// Runs inside a privacy audit scope (unless the caller already opened one,
    /// e.g. with the files its @mentions attached), so files that tools read
    /// are attributed to the request that sends them.
    pub async fn run_with_events(
        &self,
        user_input: impl Into<String>,
        event_tx: tokio::sync::mpsc::UnboundedSender<AgentEvent>,
    ) -> Result<AgentResponse, PhazeError> {
        let user_input = user_input.into();
        if crate::privacy::audit::in_scope() {
            self.run_loop(user_input, event_tx).await
        } else {
            crate::privacy::audit::scope(Vec::new(), self.run_loop(user_input, event_tx)).await
        }
    }

    async fn run_loop(
        &self,
        user_input: String,
        event_tx: tokio::sync::mpsc::UnboundedSender<AgentEvent>,
    ) -> Result<AgentResponse, PhazeError> {
        let mut iterations = 0;
        let mut tool_executions = Vec::new();
        let mut total_input_tokens: u64 = 0;
        let mut total_output_tokens: u64 = 0;
        let mut total_cache_read_tokens: u64 = 0;
        let mut total_cache_write_tokens: u64 = 0;

        {
            let mut g = self.mcp_event_sink.lock().unwrap();
            *g = Some(event_tx.clone());
        }
        let _mcp_sink_guard = McpEventSinkGuard(self.mcp_event_sink.clone());

        {
            let mut conversation = self.conversation.lock().await;
            if let Some(prompt) = &self.system_prompt {
                conversation.set_system_prompt(prompt.clone());
            }
            conversation.add_user_message(&user_input);
        }
        if let Some(checkpoints) = &self.checkpoints {
            checkpoints.lock().unwrap().begin_run(&user_input);
        }

        loop {
            // Check cancellation at the start of every iteration.
            if self.is_cancelled() {
                return cancelled_run(&event_tx);
            }

            if iterations >= self.max_iterations {
                let _ = event_tx.send(AgentEvent::Error(format!(
                    "Exceeded maximum iterations ({})",
                    self.max_iterations
                )));
                return Err(PhazeError::MaxIterations(self.max_iterations));
            }

            iterations += 1;
            let _ = event_tx.send(AgentEvent::Thinking {
                iteration: iterations,
            });

            // Near the budget, fold the oldest messages into a summary rather
            // than forgetting them. If that fails, the trim below still keeps
            // the request within budget. Stop applies while the summary is
            // being written, like anywhere else in the loop.
            match crate::context::compaction::maybe_compact(
                &self.conversation,
                self.llm.as_ref(),
                self.max_context_tokens,
                &self.tools.definitions(),
                self.cancel_token.as_deref(),
            )
            .await
            {
                Ok(Some(done)) => {
                    let _ = event_tx.send(AgentEvent::ContextCompacted {
                        messages_folded: done.messages_folded,
                        tokens_before: done.tokens_before,
                        tokens_after: done.tokens_after,
                    });
                }
                Ok(None) => {}
                Err(e) if e == crate::context::compaction::CANCELLED => {
                    return cancelled_run(&event_tx);
                }
                Err(e) => tracing::warn!("context compaction failed, trimming instead: {e}"),
            }
            if self.is_cancelled() {
                return cancelled_run(&event_tx);
            }

            let messages = {
                let mut conversation = self.conversation.lock().await;
                // Hard limit: if still over budget, drop earlier turns, then
                // shrink this one (never its question; see the method).
                conversation.trim_to_token_budget(self.max_context_tokens);
                conversation.get_messages()
            };

            let tool_definitions: Vec<ToolDefinition> = self.tools.definitions();

            // Use streaming API to get real-time token deltas. Stop has to
            // work while the provider sends nothing, too: one watcher ends
            // the wait for the response to start and for each of its events.
            let stop = self.stopped();
            tokio::pin!(stop);
            let mut stream = tokio::select! {
                biased;
                _ = &mut stop => return cancelled_run(&event_tx),
                stream = self.llm.chat_stream(&messages, &tool_definitions) => {
                    stream.inspect_err(|e| {
                        let _ = event_tx.send(AgentEvent::Error(e.to_string()));
                    })?
                }
            };

            // Accumulate response content and tool calls from stream
            let mut content = String::new();
            // Kept on the assistant message for providers that must be sent
            // it back (`Message::reasoning`); not shown.
            let mut reasoning = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool_calls: HashMap<String, (String, String)> = HashMap::new(); // id -> (name, arguments)

            loop {
                let event = tokio::select! {
                    biased;
                    _ = &mut stop => return cancelled_run(&event_tx),
                    event = stream.next() => event,
                };
                let Some(event) = event else {
                    break;
                };
                if self.is_cancelled() {
                    return cancelled_run(&event_tx);
                }
                match event {
                    StreamEvent::TextDelta(delta) => {
                        content.push_str(&delta);
                        let _ = event_tx.send(AgentEvent::TextDelta(delta));
                    }
                    StreamEvent::ReasoningDelta(delta) => reasoning.push_str(&delta),
                    StreamEvent::ToolCallStart { id, name } => {
                        current_tool_calls.insert(id.clone(), (name, String::new()));
                    }
                    StreamEvent::ToolCallDelta {
                        id,
                        arguments_delta,
                    } => {
                        if let Some((_, args)) = current_tool_calls.get_mut(&id) {
                            args.push_str(&arguments_delta);
                        }
                    }
                    StreamEvent::ToolCallEnd { id } => {
                        if let Some((name, arguments)) = current_tool_calls.remove(&id) {
                            tool_calls.push(ToolCall {
                                id: id.clone(),
                                call_type: "function".to_string(),
                                function: FunctionCall { name, arguments },
                            });
                        }
                    }
                    StreamEvent::Usage(u) => {
                        let _ = event_tx.send(AgentEvent::CallUsage(u.clone()));
                        total_input_tokens += u.input_tokens as u64;
                        total_output_tokens += u.output_tokens as u64;
                        total_cache_read_tokens += u.cache_read_tokens as u64;
                        total_cache_write_tokens += u.cache_write_tokens as u64;
                    }
                    StreamEvent::Done => {
                        break;
                    }
                    StreamEvent::Error(err) => {
                        let _ = event_tx.send(AgentEvent::Error(err.clone()));
                        return Err(PhazeError::Llm(err));
                    }
                }
            }

            // Check if we have tool calls to execute
            if !tool_calls.is_empty() {
                // Add assistant message with tool calls to conversation
                {
                    let mut conversation = self.conversation.lock().await;
                    conversation.add_message(
                        Message::assistant_with_tools(content.clone(), tool_calls.clone())
                            .with_reasoning(std::mem::take(&mut reasoning)),
                    );
                }
                // A call that runs a held-back tool through `more_tools` is
                // from here on a call of that tool (same id, so its result
                // answers the model's call): approval, review, events and
                // execution all see the tool that really runs.
                let tool_calls: Vec<ToolCall> =
                    tool_calls.into_iter().map(resolve_more_tools).collect();

                // Execute the tool calls in the order the model gave them. A
                // run of two or more calls to tools that only read
                // (`Tool::is_concurrency_safe`) is asked about one call at a
                // time, then run together; every other call runs on its own.
                // A Stop from here on answers the calls that did not run
                // before the run ends (`stop_round`).
                let mut i = 0;
                while i < tool_calls.len() {
                    if self.is_cancelled() {
                        return self.stop_round(&tool_calls[i..], &event_tx).await;
                    }

                    let run_end = self.concurrent_run_end(&tool_calls, i);
                    if run_end - i >= 2 {
                        let mut approved: Vec<usize> = Vec::new();
                        for k in i..run_end {
                            match self
                                .approve(&tool_calls[k], &event_tx, &mut tool_executions)
                                .await
                            {
                                Gate::Allow => approved.push(k),
                                Gate::Deny => {}
                                Gate::Stop => {
                                    // Approved calls in this run have not
                                    // started yet: they did not run either.
                                    let mut not_run: Vec<ToolCall> =
                                        approved.iter().map(|&a| tool_calls[a].clone()).collect();
                                    not_run.extend_from_slice(&tool_calls[k..]);
                                    return self.stop_round(&not_run, &event_tx).await;
                                }
                            }
                        }
                        if self.is_cancelled() {
                            let mut not_run: Vec<ToolCall> =
                                approved.iter().map(|&a| tool_calls[a].clone()).collect();
                            not_run.extend_from_slice(&tool_calls[run_end..]);
                            return self.stop_round(&not_run, &event_tx).await;
                        }
                        for &k in &approved {
                            let _ = event_tx.send(AgentEvent::ToolStart {
                                name: tool_calls[k].function.name.clone(),
                            });
                        }
                        // One task, polled together: the privacy audit scope
                        // (task-local) covers every call. A Stop interrupts
                        // each call (`tool_outcome`), and the check at the top
                        // of the next pass answers the rest.
                        let outcomes = futures::future::join_all(
                            approved.iter().map(|&k| self.execute_tool(&tool_calls[k])),
                        )
                        .await;
                        for (&k, (success, result_str)) in approved.iter().zip(outcomes) {
                            self.record_outcome(
                                &tool_calls[k],
                                success,
                                result_str,
                                &event_tx,
                                &mut tool_executions,
                            )
                            .await;
                        }
                        i = run_end;
                        continue;
                    }

                    let tool_call = &tool_calls[i];
                    let tool_name = &tool_call.function.name;
                    i += 1;

                    match self
                        .approve(tool_call, &event_tx, &mut tool_executions)
                        .await
                    {
                        Gate::Allow => {}
                        Gate::Deny => continue,
                        Gate::Stop => {
                            return self.stop_round(&tool_calls[i - 1..], &event_tx).await;
                        }
                    }

                    // With a diff hook, a tool that writes one file first
                    // shows exactly what it will write, and then only that
                    // is written (see `DiffHookFn`).
                    // With checkpoints, it is planned the same way, so what
                    // it wrote is known and can be undone.
                    let mut action = Action::Run;
                    if self.diff_hook.is_some() || self.checkpoints.is_some() {
                        match self.compute_file_patch(tool_call).await {
                            None => {}
                            // The tool would fail this way, writing nothing:
                            // report it without running the tool, so no file
                            // is written that was not reviewed.
                            Some(Err(e)) => action = Action::Fail(e),
                            // A write that changes nothing has nothing to
                            // review, and without a hook nothing is reviewed.
                            Some(Ok(write))
                                if !write.changes_file() || self.diff_hook.is_none() =>
                            {
                                action = Action::Write(write);
                            }
                            Some(Ok(write)) => {
                                let Some(hook) = self.diff_hook.as_ref() else {
                                    unreachable!("checked by the arm above")
                                };
                                let path = write.target().display().to_string();
                                let before = write.before().into_owned();
                                let after = write.after().to_string();
                                let _ = event_tx.send(AgentEvent::FilePatch {
                                    path: path.clone(),
                                    before: before.clone(),
                                    after: after.clone(),
                                });
                                // As with approval: Stop ends the review.
                                let answer = self
                                    .unless_cancelled((hook)(path, before.clone(), after.clone()))
                                    .await;
                                let Some(answer) = answer.filter(|_| !self.is_cancelled()) else {
                                    return self.stop_round(&tool_calls[i - 1..], &event_tx).await;
                                };
                                // Some hunks kept: write those, and tell the
                                // model which parts were not written.
                                const REJECTED: &str = "File change rejected by user";
                                let (refused, write) = match answer {
                                    DiffAnswer::Approve => (None, write),
                                    DiffAnswer::Reject => (Some(REJECTED.to_string()), write),
                                    DiffAnswer::Keep(keep) => {
                                        let total = crate::review::hunks(&before, &after).len();
                                        let kept = keep.iter().take(total).filter(|k| **k).count();
                                        if kept == 0 {
                                            (Some(REJECTED.to_string()), write)
                                        } else if kept == total {
                                            (None, write)
                                        } else if !write.before_is_exact() {
                                            // The review saw the file as lossy
                                            // text; rebuilding the parts left
                                            // alone from it would rewrite bytes
                                            // nobody approved changing.
                                            (
                                                Some(format!(
                                                    "Only part of the change to '{path}' was \
                                                     kept, but the file is not valid UTF-8, so \
                                                     a partial write could not keep its other \
                                                     bytes exactly. Nothing was written: the \
                                                     user can approve or reject the whole change.",
                                                    path = write.target().display()
                                                )),
                                                write,
                                            )
                                        } else {
                                            let content =
                                                crate::review::apply(&before, &after, &keep);
                                            let note =
                                                crate::review::partial_note(&before, &after, &keep);
                                            (None, write.keep_only(content, note))
                                        }
                                    }
                                };
                                if let Some(why) = refused {
                                    let _ = event_tx.send(AgentEvent::ToolResult {
                                        name: tool_name.clone(),
                                        success: false,
                                        summary: why.clone(),
                                    });
                                    {
                                        let mut conversation = self.conversation.lock().await;
                                        conversation.add_tool_result(
                                            &tool_call.id,
                                            format!("Error: {why}"),
                                        );
                                    }
                                    tool_executions.push(ToolExecution {
                                        tool_name: tool_name.clone(),
                                        params: tool_call.parse_arguments().unwrap_or(Value::Null),
                                        success: false,
                                        result_summary: why,
                                    });
                                    continue;
                                }
                                action = Action::Write(write);
                            }
                        }
                    }

                    // A tool must not start after Stop (it could finish in
                    // its first poll, e.g. a file write).
                    if self.is_cancelled() {
                        return self.stop_round(&tool_calls[i - 1..], &event_tx).await;
                    }

                    let _ = event_tx.send(AgentEvent::ToolStart {
                        name: tool_name.clone(),
                    });

                    // If Stop interrupts the tool, the result says so and the
                    // check at the top of the next pass answers the rest.
                    let (success, result_str) = match action {
                        Action::Run => self.execute_tool(tool_call).await,
                        // What was reviewed, or nothing if the file changed since.
                        Action::Write(write) => {
                            let record = self
                                .checkpoints
                                .as_ref()
                                .filter(|_| write.changes_file())
                                .map(|cp| {
                                    (
                                        cp.clone(),
                                        write.target().to_path_buf(),
                                        write.current().map(<[u8]>::to_vec),
                                        write.after().to_string(),
                                    )
                                });
                            let outcome = self.tool_outcome(tool_name, write.commit()).await;
                            // Interrupted or not, what reached the disk counts.
                            if let Some((cp, path, before, after)) = record {
                                let written = outcome.0
                                    || std::fs::read(&path)
                                        .is_ok_and(|now| now == after.as_bytes());
                                if written {
                                    cp.lock().unwrap().record(
                                        &path,
                                        before.as_deref(),
                                        after.as_bytes(),
                                    );
                                }
                            }
                            outcome
                        }
                        Action::Fail(e) => (false, format!("Error: {e}")),
                    };

                    self.record_outcome(
                        tool_call,
                        success,
                        result_str,
                        &event_tx,
                        &mut tool_executions,
                    )
                    .await;
                }

                // Continue loop to get next LLM response
                continue;
            }

            // No tool calls - this is the final response
            let _ = event_tx.send(AgentEvent::TokenUsage {
                input_tokens: total_input_tokens,
                output_tokens: total_output_tokens,
                cache_read_tokens: total_cache_read_tokens,
                cache_write_tokens: total_cache_write_tokens,
            });
            let _ = event_tx.send(AgentEvent::Complete { iterations });

            {
                let mut conversation = self.conversation.lock().await;
                conversation.add_message(Message::assistant(&content).with_reasoning(reasoning));
            }

            return Ok(AgentResponse {
                content,
                tool_calls: tool_executions,
                iterations,
                total_input_tokens,
                total_output_tokens,
                total_cache_read_tokens,
                total_cache_write_tokens,
            });
        }
    }

    /// The end (exclusive) of the run of calls starting at `start` that may
    /// run together: consecutive calls to registered tools that are
    /// [`crate::tools::Tool::is_concurrency_safe`]. `start` itself when that
    /// call is not such a call.
    fn concurrent_run_end(&self, calls: &[ToolCall], start: usize) -> usize {
        calls[start..]
            .iter()
            .position(|call| {
                // A `more_tools` call left as one only reads a schema.
                !(call.function.name == MORE_TOOLS
                    || self
                        .tools
                        .get(&call.function.name)
                        .is_some_and(|tool| tool.is_concurrency_safe()))
            })
            .map_or(calls.len(), |offset| start + offset)
    }

    /// Ask the approval callback, if there is one, about `tool_call`. A
    /// denial is recorded (in the conversation, so the model knows, and in
    /// `executions`) before `Gate::Deny` is returned. `Gate::Stop` when the
    /// run was stopped while waiting, or before the answer could be used; the
    /// caller then ends the round without recording this call.
    async fn approve(
        &self,
        tool_call: &ToolCall,
        event_tx: &tokio::sync::mpsc::UnboundedSender<AgentEvent>,
        executions: &mut Vec<ToolExecution>,
    ) -> Gate {
        let Some(ref approval_fn) = self.approval_fn else {
            return Gate::Allow;
        };
        // Asking for a tool's parameters changes nothing and runs nothing.
        if tool_call.function.name == MORE_TOOLS {
            return Gate::Allow;
        }
        let tool_name = &tool_call.function.name;
        let params = tool_call.parse_arguments().unwrap_or(Value::Null);

        let _ = event_tx.send(AgentEvent::ToolApprovalRequest {
            name: tool_name.clone(),
            params: params.clone(),
        });

        // Stop ends the wait for an answer, and an answer given after Stop
        // (a prompt that Stop closed reads as "no") does not run the tool
        // either.
        let answer = self
            .unless_cancelled((approval_fn)(tool_name.clone(), params.clone()))
            .await;
        let Some(approved) = answer.filter(|_| !self.is_cancelled()) else {
            return Gate::Stop;
        };
        if approved {
            return Gate::Allow;
        }
        let _ = event_tx.send(AgentEvent::ToolResult {
            name: tool_name.clone(),
            success: false,
            summary: "Tool execution denied by user".to_string(),
        });
        {
            let mut conversation = self.conversation.lock().await;
            conversation.add_tool_result(&tool_call.id, "Error: Tool execution denied by user");
        }
        executions.push(ToolExecution {
            tool_name: tool_name.clone(),
            params,
            success: false,
            result_summary: "Tool execution denied by user".to_string(),
        });
        Gate::Deny
    }

    /// Report a finished call to the UI/CLI and record it: the event stream
    /// gets a summary, the conversation the full result (where very long
    /// output is cut, with a marker saying so, to fit the context budget).
    async fn record_outcome(
        &self,
        tool_call: &ToolCall,
        success: bool,
        result_str: String,
        event_tx: &tokio::sync::mpsc::UnboundedSender<AgentEvent>,
        executions: &mut Vec<ToolExecution>,
    ) {
        // Cap at 4 KiB and tell the user how much was elided.
        const SUMMARY_LIMIT: usize = 4096;
        let summary = if success {
            truncate_str_annotated(&result_str, SUMMARY_LIMIT)
        } else {
            result_str.clone()
        };
        let tool_name = &tool_call.function.name;

        let _ = event_tx.send(AgentEvent::ToolResult {
            name: tool_name.clone(),
            success,
            summary: summary.clone(),
        });

        executions.push(ToolExecution {
            tool_name: tool_name.clone(),
            params: tool_call.parse_arguments().unwrap_or(Value::Null),
            success,
            result_summary: summary,
        });

        let mut conversation = self.conversation.lock().await;
        conversation.add_tool_result(&tool_call.id, &result_str);
    }

    async fn execute_tool(&self, tool_call: &ToolCall) -> (bool, String) {
        let tool_name = &tool_call.function.name;

        let params = match tool_call.parse_arguments() {
            Ok(p) => p,
            Err(e) => {
                return (false, format!("Failed to parse tool arguments: {e}"));
            }
        };

        if tool_name == MORE_TOOLS {
            // Left as a `more_tools` call by `resolve_more_tools`: a request
            // for a tool's parameters, or arguments that did not parse.
            return match MoreToolsCall::parse(&params) {
                Ok(MoreToolsCall::Describe(name)) => match self.tools.describe(&name) {
                    Ok(schema) => (
                        true,
                        serde_json::to_string(&schema).unwrap_or_else(|_| schema.to_string()),
                    ),
                    Err(e) => (false, format!("Error: {e}")),
                },
                Ok(MoreToolsCall::Run { name, .. }) => (false, format!("Tool '{name}' not found")),
                Err(e) => (false, format!("Error: {e}")),
            };
        }

        if let Some(tool) = self.tools.get(tool_name) {
            self.tool_outcome(tool_name, tool.execute(params)).await
        } else {
            (false, format!("Tool '{}' not found", tool_name))
        }
    }

    /// A tool's work, awaited, as (success, what the model is told). Stop
    /// ends the wait at once: dropping the future stops what it started (a
    /// `bash` child is killed along with its process group, see `BashTool`),
    /// so Stop / Esc / Ctrl+C end a running `cargo build` instead of leaving
    /// it behind.
    ///
    /// A result is told as plain text ([`render::for_model`]), not as the
    /// tool's JSON: it stays in the conversation and is paid for again on
    /// every later request.
    async fn tool_outcome(
        &self,
        tool: &str,
        work: impl Future<Output = ToolResult>,
    ) -> (bool, String) {
        match self.unless_cancelled(work).await {
            Some(Ok(value)) => (true, render::for_model(tool, &value)),
            Some(Err(e)) => (false, format!("Error: {e}")),
            None => (false, INTERRUPTED.to_string()),
        }
    }

    /// The write a call of a file-writing tool would make, worked out by the
    /// tool itself ([`crate::tools::Tool::plan_write`]), or the error the call
    /// would fail with. `None` for any other call, including one to an
    /// unknown tool or with arguments that do not parse (`execute_tool`
    /// reports those without running anything).
    async fn compute_file_patch(
        &self,
        tool_call: &ToolCall,
    ) -> Option<Result<FileWrite, PhazeError>> {
        let params = tool_call.parse_arguments().ok()?;
        self.tools
            .get(&tool_call.function.name)?
            .plan_write(&params)
            .await
    }

    pub async fn clear_conversation(&self) {
        let mut conversation = self.conversation.lock().await;
        conversation.clear();
    }

    pub async fn get_conversation_history(&self) -> Vec<Message> {
        let conversation = self.conversation.lock().await;
        conversation.get_messages()
    }

    pub async fn estimated_tokens(&self) -> usize {
        let conversation = self.conversation.lock().await;
        conversation.estimate_tokens()
    }

    /// Pre-load conversation history (for resume/continue functionality)
    pub async fn load_history(&self, messages: Vec<(String, String)>) {
        let mut conversation = self.conversation.lock().await;
        for (role, content) in messages {
            match role.as_str() {
                "user" => conversation.add_user_message(content),
                "assistant" => conversation.add_assistant_message(content),
                _ => {}
            }
        }
    }
}

/// Resolves once `token` is set. Polled, because the token is a plain
/// `AtomicBool` shared with UI threads that have no async runtime.
async fn cancelled(token: &AtomicBool) {
    while !token.load(Ordering::Relaxed) {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// The result recorded for a tool call that Stop kept from starting.
const NOT_RUN: &str = "Cancelled by the user before this tool started; it did not run.";
/// The result recorded for a tool call that Stop interrupted.
const INTERRUPTED: &str = "Interrupted by the user before the tool finished.";

/// The approval callback's answer about one call.
enum Gate {
    /// Run it.
    Allow,
    /// Denied; the denial is already recorded.
    Deny,
    /// The run was stopped; nothing is recorded for this call.
    Stop,
}

/// How a tool call that was allowed is carried out.
enum Action {
    /// Run the tool.
    Run,
    /// Commit this write, which the diff hook approved (or which changes
    /// nothing), instead of running the tool.
    Write(FileWrite),
    /// Report the error the tool would fail with, instead of running it.
    Fail(PhazeError),
}

/// End a cancelled run the way every Stop path does: an `Error("Cancelled")`
/// event (which the UIs treat as a normal stop) and `PhazeError::Cancelled`.
fn cancelled_run<T>(
    event_tx: &tokio::sync::mpsc::UnboundedSender<AgentEvent>,
) -> Result<T, PhazeError> {
    let _ = event_tx.send(AgentEvent::Error("Cancelled".to_string()));
    Err(PhazeError::Cancelled)
}

/// Append an explicit "[truncated N chars]" marker
/// when truncation occurs, so the UI tells the user exactly how much output
/// was hidden. This only shortens the UI summary: what the model gets is the
/// result as `ConversationHistory::add_tool_result` stores it (capped there)
/// and as trimming later cuts it to fit the context budget.
/// `call` itself, or, when it runs a tool through `more_tools`, a call of
/// that tool with the given arguments and the same id. A `more_tools` call
/// that only asks for a tool's parameters, or whose arguments do not parse,
/// is left as it is (`execute_tool` answers it).
fn resolve_more_tools(call: ToolCall) -> ToolCall {
    if call.function.name != MORE_TOOLS {
        return call;
    }
    let Ok(params) = call.parse_arguments() else {
        return call;
    };
    match MoreToolsCall::parse(&params) {
        Ok(MoreToolsCall::Run { name, arguments }) => ToolCall {
            id: call.id,
            call_type: call.call_type,
            function: FunctionCall {
                name,
                arguments: arguments.to_string(),
            },
        },
        _ => call,
    }
}

fn truncate_str_annotated(s: &str, max_len: usize) -> String {
    let total = s.chars().count();
    if total <= max_len {
        return s.to_string();
    }
    let kept: String = s.chars().take(max_len).collect();
    let elided = total - max_len;
    format!("{kept}\n... [truncated, {elided} more chars not shown here]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::OpenAIClient;
    use std::time::Duration;

    /// Accept one connection and never answer on it.
    async fn serve_nothing() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (_sock, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn stop_during_compaction_cancels_the_run_promptly() {
        // ~1200 tokens of history against a 1000-token budget: the first
        // iteration asks the model for a summary before anything else.
        let conversation = Arc::new(Mutex::new(ConversationHistory::new()));
        {
            let mut c = conversation.lock().await;
            for n in 0..12 {
                let text = format!("turn {n:02} {}", "x".repeat(292));
                if n % 2 == 0 {
                    c.add_user_message(text);
                } else {
                    c.add_assistant_message(text);
                }
            }
        }
        let client = OpenAIClient::new("k").with_base_url(serve_nothing().await);
        let cancel = Arc::new(AtomicBool::new(false));
        let agent = Agent::new(Box::new(client))
            .with_context_budget(1000)
            .with_cancel_token(cancel.clone())
            .with_shared_conversation(conversation.clone());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let started = std::time::Instant::now();
        let (result, _) = tokio::join!(agent.run_with_events("one more", tx), async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            cancel.store(true, Ordering::Relaxed);
        });
        let elapsed = started.elapsed();

        assert!(matches!(result, Err(PhazeError::Cancelled)), "{result:?}");
        assert!(
            elapsed < Duration::from_secs(1),
            "returned after {elapsed:?}"
        );
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::Error(m) if m == "Cancelled")),
            "{events:?}"
        );
        // The user message was recorded; nothing was folded or dropped.
        let c = conversation.lock().await;
        assert_eq!(c.len(), 13);
        assert_eq!(c.summary(), None);
    }

    /// Calls `tool` once (no arguments), then says "done".
    struct OneCall {
        tool: &'static str,
        called: AtomicBool,
    }

    #[async_trait::async_trait]
    impl LlmClient for OneCall {
        async fn chat(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
        ) -> Result<crate::llm::LlmResponse, PhazeError> {
            Ok(crate::llm::LlmResponse {
                message: Message::assistant("summary"),
                usage: None,
            })
        }

        async fn chat_stream(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
        ) -> Result<futures::channel::mpsc::UnboundedReceiver<StreamEvent>, PhazeError> {
            let id = || "c1".to_string();
            let events = if self.called.swap(true, Ordering::SeqCst) {
                vec![StreamEvent::TextDelta("done".into()), StreamEvent::Done]
            } else {
                vec![
                    StreamEvent::ToolCallStart {
                        id: id(),
                        name: self.tool.into(),
                    },
                    StreamEvent::ToolCallDelta {
                        id: id(),
                        arguments_delta: "{}".into(),
                    },
                    StreamEvent::ToolCallEnd { id: id() },
                    StreamEvent::Done,
                ]
            };
            let (tx, rx) = futures::channel::mpsc::unbounded();
            for event in events {
                tx.unbounded_send(event).unwrap();
            }
            Ok(rx)
        }
    }

    /// A file-writing tool whose write cannot be planned (say `old_text` is
    /// missing) but whose `execute` would then succeed: the file changed in
    /// between. Counts its runs.
    struct ChangesUnderfoot {
        runs: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl crate::tools::Tool for ChangesUnderfoot {
        fn name(&self) -> &str {
            "changes_underfoot"
        }
        fn description(&self) -> &str {
            "writes a file"
        }
        fn parameters_schema(&self) -> Value {
            serde_json::json!({"type": "object"})
        }
        async fn execute(&self, _: Value) -> ToolResult {
            self.runs.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!({"success": true}))
        }
        async fn plan_write(&self, _: &Value) -> Option<Result<FileWrite, PhazeError>> {
            Some(Err(PhazeError::tool(self.name(), "old_text not found")))
        }
    }

    #[tokio::test]
    async fn a_write_that_cannot_be_planned_fails_without_running_the_tool() {
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reviews = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(ChangesUnderfoot { runs: runs.clone() }));
        let hook: DiffHookFn = {
            let reviews = reviews.clone();
            Box::new(move |_, _, _| {
                reviews.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { DiffAnswer::Approve })
            })
        };
        let llm = OneCall {
            tool: "changes_underfoot",
            called: AtomicBool::new(false),
        };
        let agent = Agent::new(Box::new(llm))
            .with_tools(tools)
            .with_diff_hook(hook);

        let response = agent.run("write it").await.unwrap();

        // Running it anyway could write what nobody reviewed.
        assert_eq!(runs.load(Ordering::SeqCst), 0, "the tool ran");
        assert_eq!(reviews.load(Ordering::SeqCst), 0);
        let call = &response.tool_calls[0];
        assert!(!call.success, "{call:?}");
        assert!(
            call.result_summary.contains("old_text not found"),
            "{call:?}"
        );
    }

    /// Answers each request with the next scripted round of tool calls (by
    /// tool name), then with plain text once the script runs out. With
    /// `forever`, every request asks for the same call again.
    struct Rounds {
        rounds: StdMutex<std::collections::VecDeque<Vec<&'static str>>>,
        forever: Option<&'static str>,
        requests: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Rounds {
        fn new(rounds: Vec<Vec<&'static str>>) -> Self {
            Self {
                rounds: StdMutex::new(rounds.into()),
                forever: None,
                requests: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for Rounds {
        async fn chat(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
        ) -> Result<crate::llm::LlmResponse, PhazeError> {
            Ok(crate::llm::LlmResponse {
                message: Message::assistant("summary"),
                usage: None,
            })
        }

        async fn chat_stream(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
        ) -> Result<futures::channel::mpsc::UnboundedReceiver<StreamEvent>, PhazeError> {
            let n = self.requests.fetch_add(1, Ordering::SeqCst);
            let round = match self.forever {
                Some(tool) => Some(vec![tool]),
                None => self.rounds.lock().unwrap().pop_front(),
            };
            let mut events = Vec::new();
            match round {
                Some(names) => {
                    for (k, name) in names.into_iter().enumerate() {
                        let id = format!("r{n}c{k}");
                        events.push(StreamEvent::ToolCallStart {
                            id: id.clone(),
                            name: name.into(),
                        });
                        events.push(StreamEvent::ToolCallDelta {
                            id: id.clone(),
                            arguments_delta: "{}".into(),
                        });
                        events.push(StreamEvent::ToolCallEnd { id });
                    }
                }
                None => events.push(StreamEvent::TextDelta("done".into())),
            }
            events.push(StreamEvent::Done);
            let (tx, rx) = futures::channel::mpsc::unbounded();
            for event in events {
                tx.unbounded_send(event).unwrap();
            }
            Ok(rx)
        }
    }

    /// Counts how many of its calls (and those of tools sharing its
    /// counters) are running at once. Each call takes `millis`.
    struct Overlap {
        name: &'static str,
        safe: bool,
        millis: u64,
        active: Arc<std::sync::atomic::AtomicUsize>,
        peak: Arc<std::sync::atomic::AtomicUsize>,
        finished: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl crate::tools::Tool for Overlap {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "takes a while"
        }
        fn is_concurrency_safe(&self) -> bool {
            self.safe
        }
        fn parameters_schema(&self) -> Value {
            serde_json::json!({"type": "object"})
        }
        async fn execute(&self, _: Value) -> ToolResult {
            let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(self.millis)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            self.finished.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!(self.name))
        }
    }

    struct Counters {
        active: Arc<std::sync::atomic::AtomicUsize>,
        peak: Arc<std::sync::atomic::AtomicUsize>,
        finished: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Counters {
        fn new() -> Self {
            Self {
                active: Arc::default(),
                peak: Arc::default(),
                finished: Arc::default(),
            }
        }
        fn tool(&self, name: &'static str, safe: bool, millis: u64) -> Box<Overlap> {
            Box::new(Overlap {
                name,
                safe,
                millis,
                active: self.active.clone(),
                peak: self.peak.clone(),
                finished: self.finished.clone(),
            })
        }
        fn peak(&self) -> usize {
            self.peak.load(Ordering::SeqCst)
        }
    }

    /// Every tool call in `history` has exactly one result, and results come
    /// in the order of the calls.
    fn assert_every_call_answered_in_order(history: &[Message]) {
        let mut pending: std::collections::VecDeque<String> = Default::default();
        for m in history {
            if let Some(calls) = &m.tool_calls {
                assert!(pending.is_empty(), "unanswered calls {pending:?}");
                pending.extend(calls.iter().map(|c| c.id.clone()));
            }
            if let Some(id) = &m.tool_call_id {
                assert_eq!(
                    pending.pop_front().as_deref(),
                    Some(id.as_str()),
                    "{history:#?}"
                );
            }
        }
        assert!(pending.is_empty(), "unanswered calls {pending:?}");
    }

    #[tokio::test]
    async fn a_task_longer_than_fifteen_turns_finishes_by_default() {
        // The old limit was 15 turns; a real task takes more.
        let counters = Counters::new();
        let mut tools = ToolRegistry::new();
        tools.register(counters.tool("step", false, 0));
        let llm = Rounds::new(vec![vec!["step"]; 40]);
        let agent = Agent::new(Box::new(llm)).with_tools(tools);

        let response = agent.run("do it").await.unwrap();

        assert_eq!(response.iterations, 41);
        assert_eq!(counters.finished.load(Ordering::SeqCst), 40);
        assert_eq!(response.content, "done");
    }

    #[tokio::test]
    async fn the_turn_limit_still_stops_a_model_that_loops() {
        let counters = Counters::new();
        let mut tools = ToolRegistry::new();
        tools.register(counters.tool("step", false, 0));
        let mut llm = Rounds::new(Vec::new());
        llm.forever = Some("step");
        let requests = llm.requests.clone();
        let agent = Agent::new(Box::new(llm))
            .with_tools(tools)
            .with_max_iterations(3);

        let err = agent.run("loop").await.unwrap_err();

        assert!(matches!(err, PhazeError::MaxIterations(3)), "{err:?}");
        assert_eq!(requests.load(Ordering::SeqCst), 3);
        assert_every_call_answered_in_order(&agent.get_conversation_history().await);
    }

    #[tokio::test]
    async fn a_zero_turn_limit_counts_as_one() {
        let llm = Rounds::new(Vec::new());
        let agent = Agent::new(Box::new(llm)).with_max_iterations(0);
        let response = agent.run("hi").await.unwrap();
        assert_eq!(response.content, "done");
    }

    #[tokio::test]
    async fn read_only_calls_in_one_round_run_together() {
        let counters = Counters::new();
        let mut tools = ToolRegistry::new();
        for name in ["read_a", "read_b", "read_c"] {
            tools.register(counters.tool(name, true, 300));
        }
        let llm = Rounds::new(vec![vec!["read_a", "read_b", "read_c"]]);
        let agent = Agent::new(Box::new(llm)).with_tools(tools);

        let started = std::time::Instant::now();
        let response = agent.run("read").await.unwrap();
        let took = started.elapsed();

        assert_eq!(counters.peak(), 3, "the three reads did not overlap");
        assert!(took < Duration::from_millis(800), "took {took:?}");
        let names: Vec<_> = response
            .tool_calls
            .iter()
            .map(|t| t.tool_name.as_str())
            .collect();
        assert_eq!(names, ["read_a", "read_b", "read_c"]);
        assert!(response.tool_calls.iter().all(|t| t.success));
        let history = agent.get_conversation_history().await;
        assert_every_call_answered_in_order(&history);
        let results: Vec<_> = history
            .iter()
            .filter(|m| m.tool_call_id.is_some())
            .map(|m| m.content.as_str())
            .collect();
        // A tool that returns a string is told as that string, not as JSON.
        assert_eq!(results, ["read_a", "read_b", "read_c"]);
    }

    #[tokio::test]
    async fn a_call_that_is_not_read_only_runs_alone_and_in_order() {
        let counters = Counters::new();
        let mut tools = ToolRegistry::new();
        tools.register(counters.tool("read_a", true, 100));
        tools.register(counters.tool("write", false, 100));
        tools.register(counters.tool("read_b", true, 100));
        let llm = Rounds::new(vec![vec!["read_a", "write", "read_b"]]);
        let agent = Agent::new(Box::new(llm)).with_tools(tools);

        let response = agent.run("go").await.unwrap();

        assert_eq!(counters.peak(), 1, "a write overlapped a read");
        let names: Vec<_> = response
            .tool_calls
            .iter()
            .map(|t| t.tool_name.as_str())
            .collect();
        assert_eq!(names, ["read_a", "write", "read_b"]);
        assert_every_call_answered_in_order(&agent.get_conversation_history().await);
    }

    #[tokio::test]
    async fn an_unknown_tool_in_a_run_of_reads_splits_it_and_is_reported() {
        let counters = Counters::new();
        let mut tools = ToolRegistry::new();
        tools.register(counters.tool("read_a", true, 100));
        tools.register(counters.tool("read_b", true, 100));
        let llm = Rounds::new(vec![vec!["read_a", "no_such_tool", "read_b"]]);
        let agent = Agent::new(Box::new(llm)).with_tools(tools);

        let response = agent.run("go").await.unwrap();

        assert_eq!(counters.peak(), 1);
        assert!(!response.tool_calls[1].success);
        assert!(response.tool_calls[1].result_summary.contains("not found"));
        assert_every_call_answered_in_order(&agent.get_conversation_history().await);
    }

    #[tokio::test]
    async fn each_read_in_a_run_is_asked_about_and_a_denied_one_does_not_run() {
        let counters = Counters::new();
        let mut tools = ToolRegistry::new();
        for name in ["read_a", "read_b", "read_c"] {
            tools.register(counters.tool(name, true, 200));
        }
        let asked = Arc::new(StdMutex::new(Vec::new()));
        let approval: ApprovalFn = {
            let asked = asked.clone();
            Box::new(move |name, _| {
                asked.lock().unwrap().push(name.clone());
                Box::pin(async move { name != "read_b" })
            })
        };
        let llm = Rounds::new(vec![vec!["read_a", "read_b", "read_c"]]);
        let agent = Agent::new(Box::new(llm))
            .with_tools(tools)
            .with_approval(approval);

        let response = agent.run("read").await.unwrap();

        assert_eq!(*asked.lock().unwrap(), ["read_a", "read_b", "read_c"]);
        assert_eq!(counters.finished.load(Ordering::SeqCst), 2);
        assert_eq!(counters.peak(), 2, "the two allowed reads did not overlap");
        let denied = response
            .tool_calls
            .iter()
            .find(|t| t.tool_name == "read_b")
            .unwrap();
        assert!(!denied.success);
        assert!(denied.result_summary.contains("denied"));
        // Every call has its result: the denial is recorded as one.
        let history = agent.get_conversation_history().await;
        let results = history.iter().filter(|m| m.tool_call_id.is_some()).count();
        assert_eq!(results, 3);
    }

    #[tokio::test]
    async fn stop_during_a_run_of_reads_ends_promptly_and_answers_every_call() {
        let counters = Counters::new();
        let mut tools = ToolRegistry::new();
        for name in ["read_a", "read_b"] {
            tools.register(counters.tool(name, true, 30_000));
        }
        tools.register(counters.tool("write", false, 0));
        let llm = Rounds::new(vec![vec!["read_a", "read_b", "write"]]);
        let cancel = Arc::new(AtomicBool::new(false));
        let agent = Agent::new(Box::new(llm))
            .with_tools(tools)
            .with_cancel_token(cancel.clone());

        let stopper = {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
                cancel.store(true, Ordering::SeqCst);
            })
        };
        let started = std::time::Instant::now();
        let err = agent.run("read").await.unwrap_err();
        stopper.await.unwrap();

        assert!(matches!(err, PhazeError::Cancelled), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(counters.finished.load(Ordering::SeqCst), 0);
        let history = agent.get_conversation_history().await;
        assert_every_call_answered_in_order(&history);
        let results: Vec<_> = history
            .iter()
            .filter(|m| m.tool_call_id.is_some())
            .map(|m| m.content.as_str())
            .collect();
        assert_eq!(results, [INTERRUPTED, INTERRUPTED, NOT_RUN]);
    }

    #[tokio::test]
    async fn stop_while_asking_about_a_read_in_a_run_answers_every_call() {
        let counters = Counters::new();
        let mut tools = ToolRegistry::new();
        for name in ["read_a", "read_b", "read_c"] {
            tools.register(counters.tool(name, true, 0));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        // Allows read_a at once, then never answers about read_b.
        let approval: ApprovalFn = Box::new(move |name, _| {
            Box::pin(async move {
                if name == "read_a" {
                    true
                } else {
                    std::future::pending().await
                }
            })
        });
        let llm = Rounds::new(vec![vec!["read_a", "read_b", "read_c"]]);
        let agent = Agent::new(Box::new(llm))
            .with_tools(tools)
            .with_approval(approval)
            .with_cancel_token(cancel.clone());

        let stopper = {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
                cancel.store(true, Ordering::SeqCst);
            })
        };
        let err = agent.run("read").await.unwrap_err();
        stopper.await.unwrap();

        assert!(matches!(err, PhazeError::Cancelled), "{err:?}");
        // read_a was allowed but had not started: it did not run either.
        assert_eq!(counters.finished.load(Ordering::SeqCst), 0);
        let history = agent.get_conversation_history().await;
        let results: Vec<_> = history
            .iter()
            .filter(|m| m.tool_call_id.is_some())
            .map(|m| m.content.as_str())
            .collect();
        assert_eq!(results, [NOT_RUN, NOT_RUN, NOT_RUN]);
    }

    /// Answers each request with the next scripted round of calls, each a
    /// tool name and its arguments, then with plain text; records the tool
    /// names each request offered.
    struct Script {
        rounds: StdMutex<std::collections::VecDeque<Vec<(&'static str, Value)>>>,
        offered: Arc<StdMutex<Vec<Vec<String>>>>,
    }

    impl Script {
        fn new(rounds: Vec<Vec<(&'static str, Value)>>) -> Self {
            Self {
                rounds: StdMutex::new(rounds.into()),
                offered: Arc::default(),
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for Script {
        async fn chat(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
        ) -> Result<crate::llm::LlmResponse, PhazeError> {
            Ok(crate::llm::LlmResponse {
                message: Message::assistant("summary"),
                usage: None,
            })
        }

        async fn chat_stream(
            &self,
            _: &[Message],
            tools: &[ToolDefinition],
        ) -> Result<futures::channel::mpsc::UnboundedReceiver<StreamEvent>, PhazeError> {
            let mut offered = self.offered.lock().unwrap();
            let n = offered.len();
            offered.push(tools.iter().map(|t| t.name.clone()).collect());
            let mut events = Vec::new();
            match self.rounds.lock().unwrap().pop_front() {
                Some(calls) => {
                    for (k, (name, args)) in calls.into_iter().enumerate() {
                        let id = format!("r{n}c{k}");
                        events.push(StreamEvent::ToolCallStart {
                            id: id.clone(),
                            name: name.into(),
                        });
                        events.push(StreamEvent::ToolCallDelta {
                            id: id.clone(),
                            arguments_delta: args.to_string(),
                        });
                        events.push(StreamEvent::ToolCallEnd { id });
                    }
                }
                None => events.push(StreamEvent::TextDelta("done".into())),
            }
            events.push(StreamEvent::Done);
            let (tx, rx) = futures::channel::mpsc::unbounded();
            for event in events {
                tx.unbounded_send(event).unwrap();
            }
            Ok(rx)
        }
    }

    #[tokio::test]
    async fn a_held_back_tool_is_described_then_run_under_its_own_name() {
        let counters = Counters::new();
        let mut tools = ToolRegistry::new();
        tools.register(counters.tool("read_a", true, 1));
        tools.register_deferred(counters.tool("rare", false, 1));
        let asked = Arc::new(StdMutex::new(Vec::new()));
        let approval: ApprovalFn = {
            let asked = asked.clone();
            Box::new(move |name, params| {
                asked.lock().unwrap().push((name, params));
                Box::pin(async { true })
            })
        };
        let llm = Script::new(vec![
            vec![(MORE_TOOLS, serde_json::json!({"name": "rare"}))],
            vec![(
                MORE_TOOLS,
                serde_json::json!({"name": "rare", "arguments": {"x": 1}}),
            )],
        ]);
        let offered = llm.offered.clone();
        let agent = Agent::new(Box::new(llm))
            .with_tools(tools)
            .with_approval(approval);

        let response = agent.run("use the rare tool").await.unwrap();

        // Only its line is sent, never its schema.
        for names in offered.lock().unwrap().iter() {
            assert_eq!(names, &["read_a", MORE_TOOLS]);
        }
        // Asking for the schema needs no approval; running the tool is
        // approved as that tool, with its own arguments.
        assert_eq!(
            *asked.lock().unwrap(),
            [("rare".to_string(), serde_json::json!({"x": 1}))]
        );
        assert_eq!(counters.finished.load(Ordering::SeqCst), 1);
        let names: Vec<&str> = response
            .tool_calls
            .iter()
            .map(|t| t.tool_name.as_str())
            .collect();
        assert_eq!(names, [MORE_TOOLS, "rare"]);
        assert!(response.tool_calls.iter().all(|t| t.success));
        assert!(response.tool_calls[0].result_summary.contains("parameters"));
        assert_every_call_answered_in_order(&agent.get_conversation_history().await);
    }

    #[tokio::test]
    async fn a_denied_held_back_tool_does_not_run() {
        let counters = Counters::new();
        let mut tools = ToolRegistry::new();
        tools.register_deferred(counters.tool("rare", false, 1));
        let approval: ApprovalFn = Box::new(|_, _| Box::pin(async { false }));
        let llm = Script::new(vec![vec![(
            MORE_TOOLS,
            serde_json::json!({"name": "rare", "arguments": {}}),
        )]]);
        let agent = Agent::new(Box::new(llm))
            .with_tools(tools)
            .with_approval(approval);

        let response = agent.run("go").await.unwrap();

        assert_eq!(counters.finished.load(Ordering::SeqCst), 0);
        assert_eq!(response.tool_calls[0].tool_name, "rare");
        assert!(!response.tool_calls[0].success);
    }

    #[tokio::test]
    async fn more_tools_reports_an_unknown_tool_or_bad_arguments() {
        let counters = Counters::new();
        let mut tools = ToolRegistry::new();
        tools.register_deferred(counters.tool("rare", false, 1));
        let llm = Script::new(vec![vec![
            (MORE_TOOLS, serde_json::json!({"name": "nope"})),
            (
                MORE_TOOLS,
                serde_json::json!({"name": "nope", "arguments": {}}),
            ),
            (
                MORE_TOOLS,
                serde_json::json!({"name": "rare", "arguments": "{bad"}),
            ),
        ]]);
        let agent = Agent::new(Box::new(llm)).with_tools(tools);

        let response = agent.run("go").await.unwrap();

        let calls = &response.tool_calls;
        assert_eq!(calls.len(), 3);
        assert!(calls.iter().all(|t| !t.success), "{calls:?}");
        assert!(calls[0].result_summary.contains("rare"), "{calls:?}");
        assert!(calls[1].result_summary.contains("not found"), "{calls:?}");
        assert!(
            calls[2].result_summary.contains("not valid JSON"),
            "{calls:?}"
        );
        assert_eq!(counters.finished.load(Ordering::SeqCst), 0);
        assert_every_call_answered_in_order(&agent.get_conversation_history().await);
    }
}
