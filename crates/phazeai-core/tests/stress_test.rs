use phazeai_core::agent::multi_agent::{MultiAgentOrchestrator, AgentTask};
use phazeai_core::Settings;
use phazeai_core::error::PhazeError;

#[tokio::main]
async fn main() -> Result<(), PhazeError> {
    println!("🚀 Starting PhazeAI Multi-Agent Stress Test...");
    
    let settings = Settings::load();
    let orchestrator = MultiAgentOrchestrator::new(settings, true); // true = full pipeline (Planner -> Coder -> Reviewer)
    
    let task = AgentTask {
        prompt: "Create a new module `analysis/benchmark.rs` that can run a sample prompt against all `phaze-*` local models and measure tokens per second (TPS) and total latency. Wire it into the CLI as a `/benchmark` command.".to_string(),
        context_files: vec![
            "crates/phazeai-core/src/llm/ollama_manager.rs".into(),
            "crates/phazeai-cli/src/commands.rs".into(),
        ],
        project_root: std::env::current_dir().unwrap(),
        repo_map: None,
    };

    println!("🤖 Executing full pipeline (Planner -> Coder -> Reviewer)...");
    
    // We'll use a channel to see real-time events from the agents
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    
    let handle = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                phazeai_core::agent::multi_agent::MultiAgentEvent::AgentStarted { role, model } => {
                    println!("\n▶️  Agent Started: {:?} (Model: {})", role, model);
                }
                phazeai_core::agent::multi_agent::MultiAgentEvent::AgentFinished { role, result } => {
                    println!("\n✅ Agent Finished: {:?}", role);
                    println!("--- Output Snippet ---\n{}\n---", result.output.chars().take(200).collect::<String>());
                }
                phazeai_core::agent::multi_agent::MultiAgentEvent::StepProgress { step, total, description } => {
                    println!("  [{}/{}] {}", step, total, description);
                }
                _ => {}
            }
        }
    });

    match orchestrator.execute(task, Some(tx)).await {
        Ok(result) => {
            println!("\n🏁 STRESS TEST COMPLETE!");
            println!("Status: {:?}", result.status);
            println!("Final Coder Output: {} chars", result.coder_output.len());
        }
        Err(e) => {
            println!("\n❌ STRESS TEST FAILED: {}", e);
        }
    }

    handle.await.unwrap();
    Ok(())
}
