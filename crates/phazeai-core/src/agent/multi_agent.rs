use crate::error::PhazeError;
use crate::llm::{LlmClient, Message, Role};
/// Multi-agent orchestrator for PhazeAI.
/// Runs planner, coder, and reviewer agents ALL locally through Ollama.
/// Inspired by goose's subagent system but fully local — zero cloud dependency.
use std::sync::Arc;

/// Roles in the multi-agent system
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AgentRole {
    /// Analyzes the request, breaks it into steps, decides what files to touch
    Planner,
    /// Writes the actual code changes
    Coder,
    /// Reviews code for bugs, security, style issues
    Reviewer,
    /// Orchestrates the other agents
    Orchestrator,
}

impl AgentRole {
    pub fn name(&self) -> &str {
        match self {
            AgentRole::Planner => "Planner",
            AgentRole::Coder => "Coder",
            AgentRole::Reviewer => "Reviewer",
            AgentRole::Orchestrator => "Orchestrator",
        }
    }

    /// System prompt for each role
    pub fn system_prompt(&self) -> &str {
        match self {
            AgentRole::Planner => PLANNER_PROMPT,
            AgentRole::Coder => CODER_PROMPT,
            AgentRole::Reviewer => REVIEWER_PROMPT,
            AgentRole::Orchestrator => ORCHESTRATOR_PROMPT,
        }
    }
}

/// Result from a single agent's execution
#[derive(Debug, Clone)]
pub struct AgentRoleResult {
    pub role: AgentRole,
    pub output: String,
    pub confidence: f32,
    pub suggestions: Vec<String>,
}

/// A task that flows through the multi-agent pipeline
#[derive(Debug, Clone)]
pub struct AgentTask {
    pub user_request: String,
    pub repo_map: Option<String>,
    pub relevant_files: Vec<(String, String)>, // (path, content)
    pub conversation_context: Vec<String>,
}

/// Events emitted during multi-agent execution
#[derive(Debug, Clone)]
pub enum MultiAgentEvent {
    /// An agent started working
    AgentStarted(AgentRole),
    /// An agent produced output
    AgentOutput { role: AgentRole, text: String },
    /// An agent finished
    AgentFinished(AgentRoleResult),
    /// The full pipeline completed
    PipelineComplete {
        plan: String,
        code: String,
        review: String,
    },
    /// Something went wrong
    Error(String),
}

/// The multi-agent orchestrator.
/// All agents run through the SAME local Ollama instance.
pub struct MultiAgentOrchestrator {
    llm: Arc<dyn LlmClient>,
    /// Whether to run the full pipeline (plan → code → review) or just single-shot
    full_pipeline: bool,
    /// Optional model override per role (e.g. use a smaller model for planning)
    role_models: std::collections::HashMap<AgentRole, String>,
}

impl MultiAgentOrchestrator {
    pub fn new(llm: Arc<dyn LlmClient>) -> Self {
        Self {
            llm,
            full_pipeline: true,
            role_models: std::collections::HashMap::new(),
        }
    }

    /// Set whether to run the full pipeline or just coding
    pub fn with_full_pipeline(mut self, full: bool) -> Self {
        self.full_pipeline = full;
        self
    }

    /// Override which Ollama model to use for a specific role
    /// (e.g. use qwen2.5-coder:7b for coding, llama3.2:3b for planning)
    pub fn with_role_model(mut self, role: AgentRole, model: String) -> Self {
        self.role_models.insert(role, model);
        self
    }

    /// Run the full multi-agent pipeline on a task.
    /// All execution happens locally through Ollama.
    pub async fn execute(
        &self,
        task: AgentTask,
        event_tx: Option<tokio::sync::mpsc::UnboundedSender<MultiAgentEvent>>,
    ) -> Result<PipelineResult, PhazeError> {
        if self.full_pipeline {
            self.execute_full_pipeline(task, event_tx).await
        } else {
            // Single-shot: just run the coder directly
            let output = self.run_role(AgentRole::Coder, &task, None).await?;
            Ok(PipelineResult {
                plan: String::new(),
                code: output.output.clone(),
                review: String::new(),
                final_output: output.output,
            })
        }
    }

    /// Full pipeline: Planner → Coder → Reviewer
    async fn execute_full_pipeline(
        &self,
        task: AgentTask,
        event_tx: Option<tokio::sync::mpsc::UnboundedSender<MultiAgentEvent>>,
    ) -> Result<PipelineResult, PhazeError> {
        // Step 1: Planner analyzes the request
        Self::emit(&event_tx, MultiAgentEvent::AgentStarted(AgentRole::Planner));

        let plan_result = self.run_role(AgentRole::Planner, &task, None).await?;

        Self::emit(
            &event_tx,
            MultiAgentEvent::AgentFinished(plan_result.clone()),
        );

        // Step 2: Coder implements based on the plan
        Self::emit(&event_tx, MultiAgentEvent::AgentStarted(AgentRole::Coder));

        let code_result = self
            .run_role(AgentRole::Coder, &task, Some(&plan_result.output))
            .await?;

        Self::emit(
            &event_tx,
            MultiAgentEvent::AgentFinished(code_result.clone()),
        );

        // Step 3: Reviewer checks the code
        Self::emit(
            &event_tx,
            MultiAgentEvent::AgentStarted(AgentRole::Reviewer),
        );

        let review_context = format!(
            "## Plan\n{}\n\n## Implementation\n{}",
            plan_result.output, code_result.output
        );
        let review_result = self
            .run_role(AgentRole::Reviewer, &task, Some(&review_context))
            .await?;

        Self::emit(
            &event_tx,
            MultiAgentEvent::AgentFinished(review_result.clone()),
        );

        let pipeline_result = PipelineResult {
            plan: plan_result.output,
            code: code_result.output.clone(),
            review: review_result.output,
            final_output: code_result.output,
        };

        Self::emit(
            &event_tx,
            MultiAgentEvent::PipelineComplete {
                plan: pipeline_result.plan.clone(),
                code: pipeline_result.code.clone(),
                review: pipeline_result.review.clone(),
            },
        );

        Ok(pipeline_result)
    }

    /// Run a single agent role
    async fn run_role(
        &self,
        role: AgentRole,
        task: &AgentTask,
        previous_output: Option<&str>,
    ) -> Result<AgentRoleResult, PhazeError> {
        let system_prompt = role.system_prompt().to_string();

        // Build the user message with all context
        let mut user_msg = String::new();

        // Add repo map if available
        if let Some(ref repo_map) = task.repo_map {
            user_msg.push_str("## Repository Structure\n");
            user_msg.push_str(repo_map);
            user_msg.push_str("\n\n");
        }

        // Add relevant files
        if !task.relevant_files.is_empty() {
            user_msg.push_str("## Relevant Files\n");
            for (path, content) in &task.relevant_files {
                user_msg.push_str(&format!("### {}\n```\n{}\n```\n\n", path, content));
            }
        }

        // Add previous agent output if this is a follow-up step
        if let Some(prev) = previous_output {
            user_msg.push_str("## Previous Agent Output\n");
            user_msg.push_str(prev);
            user_msg.push_str("\n\n");
        }

        // Add the actual request
        user_msg.push_str("## User Request\n");
        user_msg.push_str(&task.user_request);

        let messages = vec![
            Message {
                role: Role::System,
                content: system_prompt,
                tool_calls: None,
                tool_call_id: None,
            },
            Message {
                role: Role::User,
                content: user_msg,
                tool_calls: None,
                tool_call_id: None,
            },
        ];

        let response = self
            .llm
            .chat(&messages, &[])
            .await
            .map_err(|e| PhazeError::Other(format!("Agent {} failed: {}", role.name(), e)))?;

        Ok(AgentRoleResult {
            role,
            output: response.message.content,
            confidence: 0.8,
            suggestions: vec![],
        })
    }

    fn emit(
        tx: &Option<tokio::sync::mpsc::UnboundedSender<MultiAgentEvent>>,
        event: MultiAgentEvent,
    ) {
        if let Some(ref tx) = tx {
            let _ = tx.send(event);
        }
    }
}

/// Result of the full multi-agent pipeline
#[derive(Debug, Clone)]
pub struct PipelineResult {
    pub plan: String,
    pub code: String,
    pub review: String,
    pub final_output: String,
}

// ── Agent Role Prompts ──────────────────────────────────────

const PLANNER_PROMPT: &str = r#"You are the PLANNER agent in PhazeAI's multi-agent system.
Your job is to analyze a coding request and produce a clear, step-by-step plan.

You will receive:
- A repo map showing the project structure (functions, classes, modules)
- Relevant source files
- The user's request

Your output should be:
1. A brief analysis of what needs to change
2. A numbered list of specific steps
3. Which files need to be created, modified, or deleted
4. Any potential risks or edge cases

Be concise. The CODER agent will implement your plan.
Do NOT write code — just plan."#;

const CODER_PROMPT: &str = r#"You are the CODER agent in PhazeAI's multi-agent system.
Your job is to write the actual code changes.

You will receive:
- The PLANNER's step-by-step plan
- The repo map and relevant source files
- The user's original request

Your output should be:
- Complete code changes with file paths
- Use diff format when modifying existing files
- Use full file content when creating new files
- Include ALL necessary changes — don't leave TODOs

Write production-quality code. The REVIEWER agent will check your work."#;

const REVIEWER_PROMPT: &str = r#"You are the REVIEWER agent in PhazeAI's multi-agent system.
Your job is to review the CODER's implementation for issues.

You will receive:
- The original plan
- The code implementation
- The repo map and relevant files

Check for:
1. Correctness: Does the code implement the plan correctly?
2. Bugs: Are there any logical errors, off-by-one, null checks missing?
3. Security: Any injection vectors, unsafe operations, secret leaks?
4. Style: Does it match the existing codebase style?
5. Performance: Any obvious inefficiencies?

Output a brief review:
- ✅ APPROVED if the code looks good
- ⚠️ CONCERNS if there are minor issues (list them)
- ❌ REJECTED if there are critical bugs (explain what needs fixing)"#;

const ORCHESTRATOR_PROMPT: &str = r#"You are the ORCHESTRATOR agent in PhazeAI.
You coordinate between planner, coder, and reviewer agents.
Analyze the task complexity and decide which agents to involve."#;
