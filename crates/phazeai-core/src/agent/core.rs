use crate::agent_event::AgentEvent;
use crate::context::ConversationHistory;
use crate::error::PhazeError;
use crate::llm::{FunctionCall, LlmClient, Message, StreamEvent, ToolCall};
use crate::tools::{ToolDefinition, ToolRegistry};
use futures::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct AgentResponse {
    pub content: String,
    pub tool_calls: Vec<ToolExecution>,
    pub iterations: usize,
    /// Cumulative token usage across all LLM calls in this run.
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
}

/// Callback invoked before tool execution. Returns true to approve, false to deny.
pub type ApprovalFn = Box<
    dyn Fn(String, serde_json::Value) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync,
>;

/// Called before a write_file/edit_file executes.
/// Arguments: (path, before_content, after_content). Returns true to proceed.
pub type DiffHookFn = Box<
    dyn Fn(String, String, String) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync,
>;

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
            max_iterations: 15,
            max_context_tokens: 32768,
            approval_fn: None,
            diff_hook: None,
            cancel_token: None,
            mcp_event_sink: Arc::new(StdMutex::new(None)),
            system_prompt: None,
        }
    }

    /// Attach a cancellation token. Set the `AtomicBool` to `true` from any
    /// thread to abort the agent loop after the current LLM/tool step.
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

    pub fn with_tools(mut self, tools: ToolRegistry) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = max;
        self
    }

    pub fn with_context_budget(mut self, budget: usize) -> Self {
        self.max_context_tokens = budget;
        self
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
    pub fn with_shared_conversation(mut self, conv: Arc<Mutex<ConversationHistory>>) -> Self {
        self.conversation = conv;
        self
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        // Store and apply at run start to avoid races with async lock acquisition.
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn register_tool(&mut self, tool: Box<dyn crate::tools::Tool>) {
        self.tools.register(tool);
    }

    pub fn register_mcp_tools(
        &mut self,
        manager: std::sync::Arc<std::sync::Mutex<crate::mcp::McpManager>>,
    ) {
        use crate::tools::mcp_bridge::create_mcp_tool_bridges;
        let bridges = create_mcp_tool_bridges(manager, Some(self.mcp_event_sink.clone()));
        let count = bridges.len();
        for bridge in bridges {
            self.tools.register(bridge);
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
    pub async fn run_with_events(
        &self,
        user_input: impl Into<String>,
        event_tx: tokio::sync::mpsc::UnboundedSender<AgentEvent>,
    ) -> Result<AgentResponse, PhazeError> {
        let user_input = user_input.into();
        let mut iterations = 0;
        let mut tool_executions = Vec::new();
        let mut total_input_tokens: u64 = 0;
        let mut total_output_tokens: u64 = 0;

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

        loop {
            // Check cancellation at the start of every iteration.
            if self.is_cancelled() {
                let _ = event_tx.send(AgentEvent::Error("Cancelled".to_string()));
                return Err(PhazeError::Cancelled);
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

            let messages = {
                let mut conversation = self.conversation.lock().await;
                // Trim conversation to stay within the configured token budget.
                // Keeps the most recent messages; old ones are evicted from the front.
                conversation.trim_to_token_budget(self.max_context_tokens);
                conversation.get_messages()
            };

            let tool_definitions: Vec<ToolDefinition> = self.tools.definitions();

            // Use streaming API to get real-time token deltas
            let mut stream = self
                .llm
                .chat_stream(&messages, &tool_definitions)
                .await
                .inspect_err(|e| {
                    let _ = event_tx.send(AgentEvent::Error(e.to_string()));
                })?;

            // Accumulate response content and tool calls from stream
            let mut content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool_calls: HashMap<String, (String, String)> = HashMap::new(); // id -> (name, arguments)

            while let Some(event) = stream.next().await {
                if self.is_cancelled() {
                    let _ = event_tx.send(AgentEvent::Error("Cancelled".to_string()));
                    return Err(PhazeError::Cancelled);
                }
                match event {
                    StreamEvent::TextDelta(delta) => {
                        content.push_str(&delta);
                        let _ = event_tx.send(AgentEvent::TextDelta(delta));
                    }
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
                        total_input_tokens += u.input_tokens as u64;
                        total_output_tokens += u.output_tokens as u64;
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
                    conversation.add_message(Message::assistant_with_tools(
                        content.clone(),
                        tool_calls.clone(),
                    ));
                }

                // Execute each tool call
                for tool_call in &tool_calls {
                    if self.is_cancelled() {
                        let _ = event_tx.send(AgentEvent::Error("Cancelled".to_string()));
                        return Err(PhazeError::Cancelled);
                    }
                    let tool_name = &tool_call.function.name;

                    // Check if approval is needed
                    if let Some(ref approval_fn) = self.approval_fn {
                        let params = tool_call.parse_arguments().unwrap_or(Value::Null);

                        // Emit approval request event
                        let _ = event_tx.send(AgentEvent::ToolApprovalRequest {
                            name: tool_name.clone(),
                            params: params.clone(),
                        });

                        let approved = (approval_fn)(tool_name.clone(), params.clone()).await;
                        if !approved {
                            let _ = event_tx.send(AgentEvent::ToolResult {
                                name: tool_name.clone(),
                                success: false,
                                summary: "Tool execution denied by user".to_string(),
                            });
                            // Add denial to conversation so LLM knows
                            {
                                let mut conversation = self.conversation.lock().await;
                                conversation.add_tool_result(
                                    &tool_call.id,
                                    "Error: Tool execution denied by user",
                                );
                            }
                            tool_executions.push(ToolExecution {
                                tool_name: tool_name.clone(),
                                params,
                                success: false,
                                result_summary: "Tool execution denied by user".to_string(),
                            });
                            continue; // Skip to next tool call
                        }
                    }

                    // For file-writing tools, compute before/after and emit FilePatch.
                    // If a diff_hook is registered it blocks until the user approves.
                    if let Some(ref hook) = self.diff_hook {
                        let params_val = tool_call.parse_arguments().unwrap_or(Value::Null);
                        if let Some((fp, before, after)) =
                            compute_file_patch(tool_name, &params_val).await
                        {
                            let _ = event_tx.send(AgentEvent::FilePatch {
                                path: fp.clone(),
                                before: before.clone(),
                                after: after.clone(),
                            });
                            let approved = (hook)(fp, before, after).await;
                            if !approved {
                                let _ = event_tx.send(AgentEvent::ToolResult {
                                    name: tool_name.clone(),
                                    success: false,
                                    summary: "File change rejected by user".to_string(),
                                });
                                {
                                    let mut conversation = self.conversation.lock().await;
                                    conversation.add_tool_result(
                                        &tool_call.id,
                                        "Error: File change rejected by user",
                                    );
                                }
                                tool_executions.push(ToolExecution {
                                    tool_name: tool_name.clone(),
                                    params: params_val,
                                    success: false,
                                    result_summary: "File change rejected by user".to_string(),
                                });
                                continue;
                            }
                        }
                    }

                    let _ = event_tx.send(AgentEvent::ToolStart {
                        name: tool_name.clone(),
                    });

                    let (success, result_str) = self.execute_tool(tool_call).await;

                    // Tool result summary sent to the UI/CLI event stream. The
                    // full untruncated result is still appended to the
                    // conversation history below so the LLM sees everything.
                    // Cap at 4 KiB and tell the user how much was elided —
                    // 200 chars (the previous limit) silently hid most output.
                    const SUMMARY_LIMIT: usize = 4096;
                    let summary = if success {
                        truncate_str_annotated(&result_str, SUMMARY_LIMIT)
                    } else {
                        result_str.clone()
                    };

                    let _ = event_tx.send(AgentEvent::ToolResult {
                        name: tool_name.clone(),
                        success,
                        summary: summary.clone(),
                    });

                    tool_executions.push(ToolExecution {
                        tool_name: tool_name.clone(),
                        params: tool_call.parse_arguments().unwrap_or(Value::Null),
                        success,
                        result_summary: summary,
                    });

                    {
                        let mut conversation = self.conversation.lock().await;
                        conversation.add_tool_result(&tool_call.id, &result_str);
                    }
                }

                // Continue loop to get next LLM response
                continue;
            }

            // No tool calls - this is the final response
            let _ = event_tx.send(AgentEvent::TokenUsage {
                input_tokens: total_input_tokens,
                output_tokens: total_output_tokens,
            });
            let _ = event_tx.send(AgentEvent::Complete { iterations });

            {
                let mut conversation = self.conversation.lock().await;
                conversation.add_assistant_message(&content);
            }

            return Ok(AgentResponse {
                content,
                tool_calls: tool_executions,
                iterations,
                total_input_tokens,
                total_output_tokens,
            });
        }
    }

    async fn execute_tool(&self, tool_call: &ToolCall) -> (bool, String) {
        let tool_name = &tool_call.function.name;

        let params = match tool_call.parse_arguments() {
            Ok(p) => p,
            Err(e) => {
                return (false, format!("Failed to parse tool arguments: {e}"));
            }
        };

        if let Some(tool) = self.tools.get(tool_name) {
            match tool.execute(params).await {
                Ok(value) => {
                    let result_str =
                        serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
                    (true, result_str)
                }
                Err(e) => (false, format!("Error: {e}")),
            }
        } else {
            (false, format!("Tool '{}' not found", tool_name))
        }
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

/// Compute (path, before, after) for write_file and edit_file tools so the UI
/// can show a diff before the write happens. Returns None for all other tools.
async fn compute_file_patch(
    tool_name: &str,
    params: &Value,
) -> Option<(String, String, String)> {
    match tool_name {
        "write_file" => {
            let path = params.get("path")?.as_str()?;
            let after = params.get("content")?.as_str()?;
            let before = tokio::fs::read_to_string(path).await.unwrap_or_default();
            if before == after {
                return None; // no change — don't bother the user
            }
            Some((path.to_string(), before, after.to_string()))
        }
        "edit_file" => {
            let path = params.get("path")?.as_str()?;
            let old_text = params.get("old_text")?.as_str()?;
            let new_text = params.get("new_text")?.as_str()?;
            let before = tokio::fs::read_to_string(path).await.ok()?;
            let after = before.replacen(old_text, new_text, 1);
            if before == after {
                return None;
            }
            Some((path.to_string(), before, after))
        }
        _ => None,
    }
}

/// Append an explicit "[truncated N chars]" marker
/// when truncation occurs, so the UI tells the user (and downstream LLM if the
/// summary ever gets routed back to a model) exactly how much output was
/// hidden. The full result is still written to the conversation history;
/// this is purely a UI summary transformer.
fn truncate_str_annotated(s: &str, max_len: usize) -> String {
    let total = s.chars().count();
    if total <= max_len {
        return s.to_string();
    }
    let kept: String = s.chars().take(max_len).collect();
    let elided = total - max_len;
    format!("{kept}\n... [truncated, {elided} more chars in tool output sent to model]")
}
