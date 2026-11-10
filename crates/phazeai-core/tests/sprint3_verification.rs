use futures::channel::mpsc;
use phazeai_core::context::SystemPromptBuilder;
use phazeai_core::error::PhazeError;
use phazeai_core::llm::provider::ProviderRegistry;
use phazeai_core::llm::{LlmClient, LlmResponse, Message, ModelRouter, StreamEvent, TaskType};
use phazeai_core::tools::{ToolDefinition, ToolRegistry};
use std::collections::HashMap;

#[test]
fn test_task_classification_heuristics() {
    // Reasoning - Keywords should win over length
    assert_eq!(
        TaskType::classify("Explain the trade-offs between REST and GraphQL", false),
        TaskType::Reasoning
    );
    assert_eq!(
        TaskType::classify("Design a scalable chat architecture", false),
        TaskType::Reasoning
    );
    assert_eq!(
        TaskType::classify("Compare React vs Vue", false),
        TaskType::Reasoning
    );
    assert_eq!(
        TaskType::classify("Why is my code slow?", false),
        TaskType::Reasoning
    );

    // Tool Orchestration - Triggered by has_tools flag
    assert_eq!(
        TaskType::classify("Search for files matching *.rs", true),
        TaskType::ToolOrchestration
    );

    // Code Generation
    assert_eq!(
        TaskType::classify("Write a Rust function to parse JSON", false),
        TaskType::CodeGeneration
    );
    assert_eq!(
        TaskType::classify("Implement a toggle component", false),
        TaskType::CodeGeneration
    );

    // Code Review
    assert_eq!(
        TaskType::classify("Review this diff for bugs", false),
        TaskType::CodeReview
    );
    assert_eq!(
        TaskType::classify("Fix this null pointer error", false),
        TaskType::CodeReview
    );

    // Quick Answer - Short & No Keywords
    assert_eq!(
        TaskType::classify("How do I list files?", false),
        TaskType::QuickAnswer
    );
    assert_eq!(
        TaskType::classify("What time is it?", false),
        TaskType::QuickAnswer
    );
}

/// A mock LLM client for verification tests.
#[derive(Debug)]
struct MockClient {
    name: String,
}

#[async_trait::async_trait]
impl LlmClient for MockClient {
    async fn chat(&self, _m: &[Message], _t: &[ToolDefinition]) -> Result<LlmResponse, PhazeError> {
        Ok(LlmResponse {
            message: Message::assistant(format!("Response from {}", self.name)),
            usage: None,
        })
    }
    async fn chat_stream(
        &self,
        _m: &[Message],
        _t: &[ToolDefinition],
    ) -> Result<mpsc::UnboundedReceiver<StreamEvent>, PhazeError> {
        unimplemented!()
    }
}

#[tokio::test]
async fn test_model_router_drop_in_llm_client() {
    let default_client = Box::new(MockClient {
        name: "Default".to_string(),
    });

    // Create router with no specific routes (everything goes to default)
    let router = ModelRouter::new(&HashMap::new(), &ProviderRegistry::new(), default_client);

    let messages = vec![Message::user("Hello")];
    let response = router.chat(&messages, &[]).await.unwrap();

    assert_eq!(response.message.content, "Response from Default");
}

#[test]
fn test_every_tool_is_offered_or_listed_in_more_tools() {
    // Every tool the agent can call must reach the model: its definition is
    // sent, or it has a line in `more_tools` (a hard-coded count went stale
    // as tools were added).
    let registry = ToolRegistry::default();
    let defs = registry.definitions();
    let more = defs
        .iter()
        .find(|d| d.name == phazeai_core::tools::MORE_TOOLS)
        .expect("held-back tools are listed in more_tools");
    for tool in registry.list() {
        let name = tool.name();
        let offered = defs.iter().any(|d| d.name == name);
        let listed = more.description.contains(&format!("\n- {name}: "));
        assert!(
            offered != listed,
            "`{name}` must be either offered or listed, once"
        );
    }
    for core in [
        "read_file",
        "edit_file",
        "write_file",
        "grep",
        "glob",
        "bash",
    ] {
        assert!(defs.iter().any(|d| d.name == core), "{core} is not offered");
    }

    let prompt = SystemPromptBuilder::new().build();
    assert!(
        prompt.contains("## Safety"),
        "Should contain safety section"
    );
}
