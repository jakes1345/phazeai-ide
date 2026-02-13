use crate::context::ConversationHistory;
use crate::error::PhazeError;
use crate::llm::{LlmClient, LlmResponse, Message, ToolCall};
use crate::tools::{ToolDefinition, ToolRegistry};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Events emitted during agent execution - the shared CLI/IDE interface.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    Thinking { iteration: usize },
    TextDelta(String),
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
}

impl Agent {
    pub fn new(llm: Box<dyn LlmClient>) -> Self {
        Self {
            llm,
            tools: ToolRegistry::default(),
            conversation: Arc::new(Mutex::new(ConversationHistory::new())),
            max_iterations: 15,
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

            let llm_response: LlmResponse = self
                .llm
                .chat(&messages, &tool_definitions)
                .await
                .map_err(|e| {
                    let _ = event_tx.send(AgentEvent::Error(e.to_string()));
                    e
                })?;

            if let Some(ref tool_calls) = llm_response.message.tool_calls {
                if !tool_calls.is_empty() {
                    // Add assistant message with tool calls
                    {
                        let mut conversation = self.conversation.lock().await;
                        conversation.add_message(llm_response.message.clone());
                    }

                    // Send any text content as delta
                    if !llm_response.message.content.is_empty() {
                        let _ = event_tx.send(AgentEvent::TextDelta(
                            llm_response.message.content.clone(),
                        ));
                    }

                    for tool_call in tool_calls {
                        let tool_name = &tool_call.function.name;
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
                            params: tool_call
                                .parse_arguments()
                                .unwrap_or(Value::Null),
                            success,
                            result_summary: summary,
                        });

                        {
                            let mut conversation = self.conversation.lock().await;
                            conversation.add_tool_result(&tool_call.id, &result_str);
                        }
                    }

                    continue;
                }
            }

            // No tool calls - this is the final response
            let final_content = llm_response.message.content;
            let _ = event_tx.send(AgentEvent::TextDelta(final_content.clone()));
            let _ = event_tx.send(AgentEvent::Complete { iterations });

            {
                let mut conversation = self.conversation.lock().await;
                conversation.add_assistant_message(&final_content);
            }

            return Ok(AgentResponse {
                content: final_content,
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
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}
