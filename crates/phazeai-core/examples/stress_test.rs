use phazeai_core::agent::multi_agent::{
    AgentRole, AgentTask, MultiAgentEvent, MultiAgentOrchestrator,
};
use phazeai_core::error::PhazeError;
use phazeai_core::llm::OllamaClient;
use phazeai_core::Settings;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), PhazeError> {
    println!("🚀 Starting PhazeAI Multi-Agent Stress Test...");

    let settings = Settings::load();
    let base_url = settings
        .llm
        .base_url
        .unwrap_or_else(|| "http://localhost:11434".to_string());

    // Initialize Ollama Client
    println!("🔌 Connecting to Ollama at {}...", base_url);
    let ollama_client = Arc::new(OllamaClient::new("qwen2.5-coder:14b").with_base_url(base_url));

    let planner_client  = Arc::clone(&ollama_client);
    let coder_client    = Arc::clone(&ollama_client);
    let reviewer_client = Arc::clone(&ollama_client);

    let orchestrator = MultiAgentOrchestrator::new(ollama_client)
        .with_role_client(AgentRole::Planner,  planner_client)
        .with_role_client(AgentRole::Coder,    coder_client)
        .with_role_client(AgentRole::Reviewer, reviewer_client);

    let task = AgentTask {
        user_request: "Create a new module `analysis/benchmark.rs` that can run a sample prompt against all `phaze-*` local models and measure tokens per second (TPS) and total latency. Wire it into the CLI as a `/benchmark` command.".to_string(),
        repo_map: None,
        relevant_files: vec![
            ("crates/phazeai-core/src/llm/ollama_manager.rs".to_string(), "".to_string()),
            ("crates/phazeai-cli/src/commands.rs".to_string(), "".to_string()),
        ],
        conversation_context: vec![],
    };

    println!("🤖 Executing full pipeline (Planner -> Coder -> Reviewer)...");

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    let handle = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                MultiAgentEvent::AgentStarted(role) => {
                    println!("\n▶️  Agent Started: {:?}", role);
                }
                MultiAgentEvent::AgentOutput { role, text } => {
                    // Just print a dot for animation
                    print!(".");
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                }
                MultiAgentEvent::AgentFinished(result) => {
                    println!("\n\n✅ Agent Finished: {:?}", result.role);
                    println!(
                        "--- Output Snippet ---\n{}\n---",
                        result.output.chars().take(200).collect::<String>()
                    );
                }
                MultiAgentEvent::PipelineComplete { plan, code, review } => {
                    println!("\n🏁 PIPELINE COMPLETE!");
                }
                MultiAgentEvent::Error(e) => {
                    println!("\n❌ Agent Error: {}", e);
                }
            }
        }
    });

    match orchestrator.execute(task, Some(tx)).await {
        Ok(result) => {
            println!("\n🏁 STRESS TEST SUCCESS!");
            println!("Final Coder Output: {} chars", result.code.len());
        }
        Err(e) => {
            println!("\n❌ STRESS TEST FAILED: {}", e);
        }
    }

    handle.await.unwrap();
    Ok(())
}
