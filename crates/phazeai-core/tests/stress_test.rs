use phazeai_core::agent::multi_agent::{AgentTask, MultiAgentOrchestrator};
use phazeai_core::llm::OllamaClient;
use std::sync::Arc;

/// Smoke test: verify the orchestrator can be constructed and the task struct works.
/// Does not actually call Ollama (no server required in CI).
#[test]
fn test_orchestrator_construction() {
    let client = Arc::new(OllamaClient::new("qwen2.5-coder:14b"));
    let orchestrator = MultiAgentOrchestrator::new(client).with_full_pipeline(false);
    let _task = AgentTask {
        user_request: "test request".to_string(),
        repo_map: None,
        relevant_files: vec![],
        conversation_context: vec![],
    };
    drop(orchestrator);
}

#[test]
fn test_agent_task_fields() {
    let task = AgentTask {
        user_request: "Create a benchmark module".to_string(),
        repo_map: Some("phazeai-ide/\n  src/\n    main.rs".to_string()),
        relevant_files: vec![("src/main.rs".to_string(), "fn main() {}".to_string())],
        conversation_context: vec!["previous message".to_string()],
    };
    assert_eq!(task.user_request, "Create a benchmark module");
    assert!(task.repo_map.is_some());
    assert_eq!(task.relevant_files.len(), 1);
    assert_eq!(task.conversation_context.len(), 1);
}
