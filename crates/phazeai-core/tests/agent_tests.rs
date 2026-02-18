use phazeai_core::{
    Agent, AgentEvent, LlmClient, LlmResponse, Message, Role, StreamEvent,
    Tool, ToolDefinition, ToolRegistry, ToolResult, PhazeError,
};
use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::unbounded_channel;

/// Mock LLM that returns pre-programmed stream event sequences.
struct MockLlm {
    responses: Arc<Mutex<Vec<Vec<StreamEvent>>>>,
}

impl MockLlm {
    fn new(responses: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses)),
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for MockLlm {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse, PhazeError> {
        // Not used by Agent, but required by trait
        Ok(LlmResponse {
            message: Message::assistant("Mock response"),
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> Result<UnboundedReceiver<StreamEvent>, PhazeError> {
        let mut responses = self.responses.lock().unwrap();
        let events = responses.pop().unwrap_or_else(|| vec![StreamEvent::Done]);

        let (mut tx, rx) = unbounded();

        // Send all events through the channel
        for event in events {
            tx.start_send(event).unwrap();
        }

        Ok(rx)
    }
}

/// Simple echo tool for testing.
struct EchoTool;

#[async_trait::async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echoes input"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {"type": "string"}
            },
            "required": ["text"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        Ok(serde_json::json!({"echoed": params["text"]}))
    }
}

/// Tool that returns an error.
struct ErrorTool;

#[async_trait::async_trait]
impl Tool for ErrorTool {
    fn name(&self) -> &str {
        "error"
    }

    fn description(&self) -> &str {
        "Always returns an error"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn execute(&self, _params: Value) -> ToolResult {
        Err(PhazeError::tool("error", "Intentional error"))
    }
}

#[tokio::test]
async fn test_simple_text_response() {
    let mock = MockLlm::new(vec![
        vec![
            StreamEvent::TextDelta("Hello".to_string()),
            StreamEvent::Done,
        ],
    ]);

    let agent = Agent::new(Box::new(mock));
    let response = agent.run("Test").await.unwrap();

    assert_eq!(response.content, "Hello");
    assert_eq!(response.iterations, 1);
    assert!(response.tool_calls.is_empty());
}

#[tokio::test]
async fn test_multi_token_streaming() {
    let mock = MockLlm::new(vec![
        vec![
            StreamEvent::TextDelta("Hello".to_string()),
            StreamEvent::TextDelta(" ".to_string()),
            StreamEvent::TextDelta("world".to_string()),
            StreamEvent::TextDelta("!".to_string()),
            StreamEvent::Done,
        ],
    ]);

    let agent = Agent::new(Box::new(mock));
    let response = agent.run("Test").await.unwrap();

    assert_eq!(response.content, "Hello world!");
    assert_eq!(response.iterations, 1);
}

#[tokio::test]
async fn test_tool_call_execution() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(EchoTool));

    // First response: tool call
    // Second response: text response after tool execution
    let mock = MockLlm::new(vec![
        // Response 2 (popped first - stack order)
        vec![
            StreamEvent::TextDelta("Done!".to_string()),
            StreamEvent::Done,
        ],
        // Response 1 (popped second)
        vec![
            StreamEvent::ToolCallStart {
                id: "call_1".to_string(),
                name: "echo".to_string(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_1".to_string(),
                arguments_delta: r#"{"text": "hello"}"#.to_string(),
            },
            StreamEvent::ToolCallEnd {
                id: "call_1".to_string(),
            },
            StreamEvent::Done,
        ],
    ]);

    let agent = Agent::new(Box::new(mock)).with_tools(registry);
    let response = agent.run("Echo hello").await.unwrap();

    assert_eq!(response.content, "Done!");
    assert_eq!(response.iterations, 2);
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].tool_name, "echo");
    assert!(response.tool_calls[0].success);
    assert!(response.tool_calls[0].result_summary.contains("hello"));
}

#[tokio::test]
async fn test_max_iterations_reached() {
    // Always return a tool call, causing infinite loop
    let mock = MockLlm::new(vec![
        vec![
            StreamEvent::ToolCallStart {
                id: "call_1".to_string(),
                name: "echo".to_string(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_1".to_string(),
                arguments_delta: r#"{"text": "test"}"#.to_string(),
            },
            StreamEvent::ToolCallEnd {
                id: "call_1".to_string(),
            },
            StreamEvent::Done,
        ],
        vec![
            StreamEvent::ToolCallStart {
                id: "call_2".to_string(),
                name: "echo".to_string(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_2".to_string(),
                arguments_delta: r#"{"text": "test"}"#.to_string(),
            },
            StreamEvent::ToolCallEnd {
                id: "call_2".to_string(),
            },
            StreamEvent::Done,
        ],
    ]);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(EchoTool));

    let agent = Agent::new(Box::new(mock))
        .with_tools(registry)
        .with_max_iterations(1);

    let result = agent.run("Test").await;

    assert!(result.is_err());
    match result.unwrap_err() {
        PhazeError::MaxIterations(max) => assert_eq!(max, 1),
        _ => panic!("Expected MaxIterations error"),
    }
}

#[tokio::test]
async fn test_system_prompt_is_set() {
    let mock = MockLlm::new(vec![
        vec![
            StreamEvent::TextDelta("OK".to_string()),
            StreamEvent::Done,
        ],
    ]);

    let agent = Agent::new(Box::new(mock))
        .with_system_prompt("You are a helpful assistant.");

    let _ = agent.run("Test").await.unwrap();

    let history = agent.get_conversation_history().await;

    // First message should be system prompt
    assert!(!history.is_empty());
    assert_eq!(history[0].role, Role::System);
    assert_eq!(history[0].content, "You are a helpful assistant.");
}

#[tokio::test]
async fn test_clear_conversation() {
    let mock = MockLlm::new(vec![
        vec![
            StreamEvent::TextDelta("Response".to_string()),
            StreamEvent::Done,
        ],
    ]);

    let agent = Agent::new(Box::new(mock))
        .with_system_prompt("System prompt");

    let _ = agent.run("Test message").await.unwrap();

    let history_before = agent.get_conversation_history().await;
    // Should have: System, User, Assistant
    assert_eq!(history_before.len(), 3);

    agent.clear_conversation().await;

    let history_after = agent.get_conversation_history().await;
    // After clear, only system prompt remains (it's not part of the conversation history)
    assert_eq!(history_after.len(), 1);
    assert_eq!(history_after[0].role, Role::System);
}

#[tokio::test]
async fn test_event_channel_receives_all_events() {
    let mock = MockLlm::new(vec![
        vec![
            StreamEvent::TextDelta("Hello".to_string()),
            StreamEvent::TextDelta(" world".to_string()),
            StreamEvent::Done,
        ],
    ]);

    let agent = Agent::new(Box::new(mock));
    let (tx, mut rx) = unbounded_channel();

    let handle = tokio::spawn(async move {
        let _ = agent.run_with_events("Test", tx).await;
    });

    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
        if matches!(events.last(), Some(AgentEvent::Complete { .. })) {
            break;
        }
    }

    handle.await.unwrap();

    // Check we got: Thinking, TextDelta, TextDelta, Complete
    assert!(matches!(events[0], AgentEvent::Thinking { iteration: 1 }));
    assert!(matches!(events[1], AgentEvent::TextDelta(_)));
    assert!(matches!(events[2], AgentEvent::TextDelta(_)));
    assert!(matches!(events[3], AgentEvent::Complete { iterations: 1 }));
}

#[tokio::test]
async fn test_llm_error_propagation() {
    let mock = MockLlm::new(vec![
        vec![
            StreamEvent::Error("LLM connection failed".to_string()),
        ],
    ]);

    let agent = Agent::new(Box::new(mock));
    let result = agent.run("Test").await;

    assert!(result.is_err());
    match result.unwrap_err() {
        PhazeError::Llm(msg) => assert_eq!(msg, "LLM connection failed"),
        _ => panic!("Expected Llm error"),
    }
}

#[tokio::test]
async fn test_tool_not_found() {
    // Return a tool call for a tool that doesn't exist
    let mock = MockLlm::new(vec![
        vec![
            StreamEvent::TextDelta("Tool failed".to_string()),
            StreamEvent::Done,
        ],
        vec![
            StreamEvent::ToolCallStart {
                id: "call_1".to_string(),
                name: "nonexistent_tool".to_string(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_1".to_string(),
                arguments_delta: r#"{"param": "value"}"#.to_string(),
            },
            StreamEvent::ToolCallEnd {
                id: "call_1".to_string(),
            },
            StreamEvent::Done,
        ],
    ]);

    let agent = Agent::new(Box::new(mock));
    let response = agent.run("Test").await.unwrap();

    // Agent should continue and return the final response
    assert_eq!(response.content, "Tool failed");
    assert_eq!(response.tool_calls.len(), 1);
    assert!(!response.tool_calls[0].success);
    assert!(response.tool_calls[0].result_summary.contains("not found"));
}

#[tokio::test]
async fn test_multiple_tool_calls_in_one_response() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(EchoTool));

    let mock = MockLlm::new(vec![
        // Final response
        vec![
            StreamEvent::TextDelta("All tools executed".to_string()),
            StreamEvent::Done,
        ],
        // Multiple tool calls in one response
        vec![
            StreamEvent::ToolCallStart {
                id: "call_1".to_string(),
                name: "echo".to_string(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_1".to_string(),
                arguments_delta: r#"{"text": "first"}"#.to_string(),
            },
            StreamEvent::ToolCallEnd {
                id: "call_1".to_string(),
            },
            StreamEvent::ToolCallStart {
                id: "call_2".to_string(),
                name: "echo".to_string(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_2".to_string(),
                arguments_delta: r#"{"text": "second"}"#.to_string(),
            },
            StreamEvent::ToolCallEnd {
                id: "call_2".to_string(),
            },
            StreamEvent::ToolCallStart {
                id: "call_3".to_string(),
                name: "echo".to_string(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_3".to_string(),
                arguments_delta: r#"{"text": "third"}"#.to_string(),
            },
            StreamEvent::ToolCallEnd {
                id: "call_3".to_string(),
            },
            StreamEvent::Done,
        ],
    ]);

    let agent = Agent::new(Box::new(mock)).with_tools(registry);
    let response = agent.run("Run multiple tools").await.unwrap();

    assert_eq!(response.content, "All tools executed");
    assert_eq!(response.tool_calls.len(), 3);
    assert_eq!(response.iterations, 2);

    // All should have succeeded
    assert!(response.tool_calls.iter().all(|tc| tc.success));

    // Verify they were called in order
    assert_eq!(response.tool_calls[0].params["text"], "first");
    assert_eq!(response.tool_calls[1].params["text"], "second");
    assert_eq!(response.tool_calls[2].params["text"], "third");
}

#[tokio::test]
async fn test_tool_execution_error() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ErrorTool));

    let mock = MockLlm::new(vec![
        // Final response after tool error
        vec![
            StreamEvent::TextDelta("Handled error".to_string()),
            StreamEvent::Done,
        ],
        // Tool call that will error
        vec![
            StreamEvent::ToolCallStart {
                id: "call_1".to_string(),
                name: "error".to_string(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_1".to_string(),
                arguments_delta: "{}".to_string(),
            },
            StreamEvent::ToolCallEnd {
                id: "call_1".to_string(),
            },
            StreamEvent::Done,
        ],
    ]);

    let agent = Agent::new(Box::new(mock)).with_tools(registry);
    let response = agent.run("Test error tool").await.unwrap();

    assert_eq!(response.content, "Handled error");
    assert_eq!(response.tool_calls.len(), 1);
    assert!(!response.tool_calls[0].success);
    assert!(response.tool_calls[0].result_summary.contains("Intentional error"));
}

#[tokio::test]
async fn test_invalid_tool_arguments() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(EchoTool));

    let mock = MockLlm::new(vec![
        // Final response
        vec![
            StreamEvent::TextDelta("Invalid args handled".to_string()),
            StreamEvent::Done,
        ],
        // Tool call with invalid JSON
        vec![
            StreamEvent::ToolCallStart {
                id: "call_1".to_string(),
                name: "echo".to_string(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_1".to_string(),
                arguments_delta: "not valid json{".to_string(),
            },
            StreamEvent::ToolCallEnd {
                id: "call_1".to_string(),
            },
            StreamEvent::Done,
        ],
    ]);

    let agent = Agent::new(Box::new(mock)).with_tools(registry);
    let response = agent.run("Test").await.unwrap();

    assert_eq!(response.content, "Invalid args handled");
    assert_eq!(response.tool_calls.len(), 1);
    assert!(!response.tool_calls[0].success);
    assert!(response.tool_calls[0].result_summary.contains("Failed to parse"));
}

#[tokio::test]
async fn test_tool_event_emissions() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(EchoTool));

    let mock = MockLlm::new(vec![
        vec![
            StreamEvent::TextDelta("Done".to_string()),
            StreamEvent::Done,
        ],
        vec![
            StreamEvent::ToolCallStart {
                id: "call_1".to_string(),
                name: "echo".to_string(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_1".to_string(),
                arguments_delta: r#"{"text": "test"}"#.to_string(),
            },
            StreamEvent::ToolCallEnd {
                id: "call_1".to_string(),
            },
            StreamEvent::Done,
        ],
    ]);

    let agent = Agent::new(Box::new(mock)).with_tools(registry);
    let (tx, mut rx) = unbounded_channel();

    let handle = tokio::spawn(async move {
        let _ = agent.run_with_events("Test", tx).await;
    });

    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
        if matches!(events.last(), Some(AgentEvent::Complete { .. })) {
            break;
        }
    }

    handle.await.unwrap();

    // Find the tool events
    let has_tool_start = events.iter().any(|e| matches!(e, AgentEvent::ToolStart { name } if name == "echo"));
    let has_tool_result = events.iter().any(|e| matches!(e, AgentEvent::ToolResult { name, success, .. } if name == "echo" && *success));

    assert!(has_tool_start, "Should have ToolStart event");
    assert!(has_tool_result, "Should have ToolResult event");
}

#[tokio::test]
async fn test_estimated_tokens() {
    let mock = MockLlm::new(vec![
        vec![
            StreamEvent::TextDelta("Response".to_string()),
            StreamEvent::Done,
        ],
    ]);

    let agent = Agent::new(Box::new(mock))
        .with_system_prompt("System prompt");

    let _ = agent.run("User message").await.unwrap();

    let tokens = agent.estimated_tokens().await;

    // Should have some tokens from system prompt + user message + assistant response
    assert!(tokens > 0);
}

#[tokio::test]
async fn test_conversation_history_structure() {
    let mock = MockLlm::new(vec![
        vec![
            StreamEvent::TextDelta("Assistant response".to_string()),
            StreamEvent::Done,
        ],
    ]);

    let agent = Agent::new(Box::new(mock))
        .with_system_prompt("You are helpful");

    let _ = agent.run("User question").await.unwrap();

    let history = agent.get_conversation_history().await;

    // Should have: System, User, Assistant
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].role, Role::System);
    assert_eq!(history[1].role, Role::User);
    assert_eq!(history[2].role, Role::Assistant);

    assert_eq!(history[0].content, "You are helpful");
    assert_eq!(history[1].content, "User question");
    assert_eq!(history[2].content, "Assistant response");
}

#[tokio::test]
async fn test_mixed_content_and_tool_calls() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(EchoTool));

    // Response with both text content AND tool calls
    let mock = MockLlm::new(vec![
        vec![
            StreamEvent::TextDelta("Final answer".to_string()),
            StreamEvent::Done,
        ],
        vec![
            StreamEvent::TextDelta("Let me use a tool: ".to_string()),
            StreamEvent::ToolCallStart {
                id: "call_1".to_string(),
                name: "echo".to_string(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_1".to_string(),
                arguments_delta: r#"{"text": "mixed"}"#.to_string(),
            },
            StreamEvent::ToolCallEnd {
                id: "call_1".to_string(),
            },
            StreamEvent::Done,
        ],
    ]);

    let agent = Agent::new(Box::new(mock)).with_tools(registry);
    let response = agent.run("Test").await.unwrap();

    assert_eq!(response.content, "Final answer");
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.iterations, 2);
}

#[tokio::test]
async fn test_approval_callback_denies_tool() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(EchoTool));

    let mock = MockLlm::new(vec![
        // Final response after tool denial
        vec![
            StreamEvent::TextDelta("Tool was denied".to_string()),
            StreamEvent::Done,
        ],
        // Tool call
        vec![
            StreamEvent::ToolCallStart {
                id: "call_1".to_string(),
                name: "echo".to_string(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_1".to_string(),
                arguments_delta: r#"{"text": "test"}"#.to_string(),
            },
            StreamEvent::ToolCallEnd {
                id: "call_1".to_string(),
            },
            StreamEvent::Done,
        ],
    ]);

    // Create approval callback that denies all tools
    let approval_fn: phazeai_core::agent::ApprovalFn = Box::new(|_tool_name, _params| {
        Box::pin(async move { false })
    });

    let agent = Agent::new(Box::new(mock))
        .with_tools(registry)
        .with_approval(approval_fn);

    let (tx, mut rx) = unbounded_channel();
    let handle = tokio::spawn(async move {
        agent.run_with_events("Test", tx).await
    });

    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
        if matches!(events.last(), Some(AgentEvent::Complete { .. })) {
            break;
        }
    }

    let response = handle.await.unwrap().unwrap();

    // Should have approval request event
    let has_approval_request = events.iter().any(|e| {
        matches!(e, AgentEvent::ToolApprovalRequest { name, .. } if name == "echo")
    });
    assert!(has_approval_request, "Should emit ToolApprovalRequest event");

    // Tool should have been denied
    assert_eq!(response.tool_calls.len(), 1);
    assert!(!response.tool_calls[0].success);
    assert!(response.tool_calls[0].result_summary.contains("denied by user"));
}

#[tokio::test]
async fn test_approval_callback_approves_tool() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(EchoTool));

    let mock = MockLlm::new(vec![
        // Final response
        vec![
            StreamEvent::TextDelta("Tool executed".to_string()),
            StreamEvent::Done,
        ],
        // Tool call
        vec![
            StreamEvent::ToolCallStart {
                id: "call_1".to_string(),
                name: "echo".to_string(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_1".to_string(),
                arguments_delta: r#"{"text": "approved"}"#.to_string(),
            },
            StreamEvent::ToolCallEnd {
                id: "call_1".to_string(),
            },
            StreamEvent::Done,
        ],
    ]);

    // Create approval callback that approves all tools
    let approval_fn: phazeai_core::agent::ApprovalFn = Box::new(|_tool_name, _params| {
        Box::pin(async move { true })
    });

    let agent = Agent::new(Box::new(mock))
        .with_tools(registry)
        .with_approval(approval_fn);

    let response = agent.run("Test").await.unwrap();

    // Tool should have been approved and executed successfully
    assert_eq!(response.tool_calls.len(), 1);
    assert!(response.tool_calls[0].success);
    assert!(response.tool_calls[0].result_summary.contains("approved"));
}

#[tokio::test]
async fn test_no_approval_callback_executes_normally() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(EchoTool));

    let mock = MockLlm::new(vec![
        vec![
            StreamEvent::TextDelta("Done".to_string()),
            StreamEvent::Done,
        ],
        vec![
            StreamEvent::ToolCallStart {
                id: "call_1".to_string(),
                name: "echo".to_string(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_1".to_string(),
                arguments_delta: r#"{"text": "no approval"}"#.to_string(),
            },
            StreamEvent::ToolCallEnd {
                id: "call_1".to_string(),
            },
            StreamEvent::Done,
        ],
    ]);

    // No approval callback - tools should execute without approval
    let agent = Agent::new(Box::new(mock)).with_tools(registry);
    let response = agent.run("Test").await.unwrap();

    // Tool should have executed successfully without any approval
    assert_eq!(response.tool_calls.len(), 1);
    assert!(response.tool_calls[0].success);
}
