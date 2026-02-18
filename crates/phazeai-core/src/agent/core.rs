use crate::context::ConversationHistory;
use crate::error::PhazeError;
use crate::llm::{LlmClient, Message, StreamEvent, ToolCall, FunctionCall};
use crate::tools::{ToolDefinition, ToolRegistry};
use futures::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::pin::Pin;
use std::future::Future;

/// Events emitted during agent execution - the shared CLI/IDE interface.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    Thinking { iteration: usize },
    TextDelta(String),
    ToolApprovalRequest { name: String, params: Value },
    ToolStart { name: String },
    ToolResult { name: String, success: bool, summary: String },
    Complete { iterations: usize },
    Error(String),
}

#[derive(Debug, Clone)]
pub struct AgentResponse {
    pub content: String,
    pub tool_calls: Vec<ToolExecution>,
    pub iterations: usize,
}

/// Callback invoked before tool execution. Returns true to approve, false to deny.
pub type ApprovalFn = Box<dyn Fn(String, serde_json::Value) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

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
}

impl Agent {
    pub fn new(llm: Box<dyn LlmClient>) -> Self {
        Self {
            llm,
            tools: ToolRegistry::default(),
            conversation: Arc::new(Mutex::new(ConversationHistory::new())),
            max_iterations: 15,
            max_context_tokens: 32768, // Default budget
            approval_fn: None,
        }
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

    pub fn with_system_prompt(self, prompt: impl Into<String>) -> Self {
        // Set system prompt synchronously by accessing the inner mutex
        // This avoids the race condition of the old tokio::spawn approach
        let conversation = self.conversation.clone();
        let prompt = prompt.into();
        // Use blocking lock since this is called during construction
        let mut conv = conversation.try_lock().expect("Agent not yet shared during construction");
        conv.set_system_prompt(prompt);
        drop(conv);
        self
    }

    pub fn register_tool(&mut self, tool: Box<dyn crate::tools::Tool>) {
        self.tools.register(tool);
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

        {
            let mut conversation = self.conversation.lock().await;
            conversation.add_user_message(&user_input);
        }

        loop {
            if iterations >= self.max_iterations {
                let _ = event_tx.send(AgentEvent::Error(format!(
                    "Exceeded maximum iterations ({})",
                    self.max_iterations
                )));
                return Err(PhazeError::MaxIterations(self.max_iterations));
            }

            iterations += 1;
            let _ = event_tx.send(AgentEvent::Thinking { iteration: iterations });

            let messages = {
                let conversation = self.conversation.lock().await;
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
                match event {
                    StreamEvent::TextDelta(delta) => {
                        content.push_str(&delta);
                        let _ = event_tx.send(AgentEvent::TextDelta(delta));
                    }
                    StreamEvent::ToolCallStart { id, name } => {
                        current_tool_calls.insert(id.clone(), (name, String::new()));
                    }
                    StreamEvent::ToolCallDelta { id, arguments_delta } => {
                        if let Some((_, args)) = current_tool_calls.get_mut(&id) {
                            args.push_str(&arguments_delta);
                        }
                    }
                    StreamEvent::ToolCallEnd { id } => {
                        if let Some((name, arguments)) = current_tool_calls.remove(&id) {
                            tool_calls.push(ToolCall {
                                id: id.clone(),
                                call_type: "function".to_string(),
                                function: FunctionCall {
                                    name,
                                    arguments,
                                },
                            });
                        }
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
                                conversation.add_tool_result(&tool_call.id, "Error: Tool execution denied by user");
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

                    let _ = event_tx.send(AgentEvent::ToolStart {
                        name: tool_name.clone(),
                    });

                    let (success, result_str) = self.execute_tool(tool_call).await;

                    let summary = if success {
                        truncate_str(&result_str, 200)
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
            let _ = event_tx.send(AgentEvent::Complete { iterations });

            {
                let mut conversation = self.conversation.lock().await;
                conversation.add_assistant_message(&content);
            }

            return Ok(AgentResponse {
                content,
                tool_calls: tool_executions,
                iterations,
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
                    let result_str = serde_json::to_string_pretty(&value)
                        .unwrap_or_else(|_| value.to_string());
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

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}
