use crate::error::PhazeError;
use crate::llm::{LlmClient, Message, Role};
/// Multi-agent orchestrator for PhazeAI.
/// Runs planner, coder, and reviewer agents ALL locally through Ollama.
/// Features a self-healing iterative refinement loop: after the Coder writes
/// code, the system runs build/check commands, feeds any errors back to the
/// Coder, and loops until the build is clean — zero human intervention.
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
    /// Self-healing refinement loop started
    RefinementStarted { max_iterations: usize },
    /// Build/check command was run
    BuildCheck {
        iteration: usize,
        success: bool,
        error_count: usize,
        warning_count: usize,
        raw_output: String,
    },
    /// A refinement iteration completed (coder fixed errors)
    RefinementIteration {
        iteration: usize,
        errors_remaining: usize,
        fix_output: String,
    },
    /// Refinement loop completed
    RefinementComplete {
        iterations_used: usize,
        clean_build: bool,
    },
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
/// All agents run through the SAME local Ollama instance by default, but each
/// role can be given its own `LlmClient` for model specialisation.
pub struct MultiAgentOrchestrator {
    llm: Arc<dyn LlmClient>,
    /// Whether to run the full pipeline (plan → code → review) or just single-shot
    full_pipeline: bool,
    /// Maximum number of build→fix iterations before giving up
    max_refinement_iterations: usize,
    /// Optional per-role LLM client overrides.
    /// If a role has no entry the default `self.llm` is used.
    role_clients: std::collections::HashMap<AgentRole, Arc<dyn LlmClient>>,
    /// Project root path for running build checks
    project_root: Option<String>,
}

impl MultiAgentOrchestrator {
    pub fn new(llm: Arc<dyn LlmClient>) -> Self {
        Self {
            llm,
            full_pipeline: true,
            max_refinement_iterations: 5,
            role_clients: std::collections::HashMap::new(),
            project_root: None,
        }
    }

    /// Set whether to run the full pipeline or just coding
    pub fn with_full_pipeline(mut self, full: bool) -> Self {
        self.full_pipeline = full;
        self
    }

    /// Override the LLM client used for a specific role.
    /// E.g. use a fast coding-focused model for the Coder role.
    pub fn with_role_client(mut self, role: AgentRole, client: Arc<dyn LlmClient>) -> Self {
        self.role_clients.insert(role, client);
        self
    }

    /// Set the maximum number of refinement iterations (default: 5)
    pub fn with_max_refinements(mut self, max: usize) -> Self {
        self.max_refinement_iterations = max;
        self
    }

    /// Set the project root path for build checks
    pub fn with_project_root(mut self, root: impl Into<String>) -> Self {
        self.project_root = Some(root.into());
        self
    }

    /// Convenience: get the appropriate client for a role (falls back to default).
    fn client_for_role(&self, role: &AgentRole) -> &Arc<dyn LlmClient> {
        self.role_clients.get(role).unwrap_or(&self.llm)
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
                refinement_iterations: 0,
                clean_build: false,
            })
        }
    }

    /// Full pipeline: Planner → Coder → Build Check → Fix Loop → Reviewer
    ///
    /// The self-healing refinement loop runs after the Coder produces code:
    /// 1. Run `cargo check` (or language-appropriate checker)
    /// 2. If errors/warnings found, feed them back to the Coder with instructions to fix
    /// 3. Repeat until build is clean or max iterations exhausted
    /// 4. Only THEN run the Reviewer on the final clean code
    async fn execute_full_pipeline(
        &self,
        mut task: AgentTask,
        event_tx: Option<tokio::sync::mpsc::UnboundedSender<MultiAgentEvent>>,
    ) -> Result<PipelineResult, PhazeError> {
        // Auto-generate repo map if project root is set and no repo map provided
        if task.repo_map.is_none() {
            if let Some(ref root) = self.project_root {
                let generator = crate::context::RepoMapGenerator::new(root)
                    .with_max_files(200)
                    .with_max_tokens(2048);
                let map = generator.generate();
                if !map.is_empty() {
                    task.repo_map = Some(map);
                }
            }
        }

        // Step 1: Planner analyzes the request
        Self::emit(&event_tx, MultiAgentEvent::AgentStarted(AgentRole::Planner));

        let plan_result = self.run_role(AgentRole::Planner, &task, None).await?;

        Self::emit(
            &event_tx,
            MultiAgentEvent::AgentFinished(plan_result.clone()),
        );

        // Step 2: Coder implements based on the plan
        Self::emit(&event_tx, MultiAgentEvent::AgentStarted(AgentRole::Coder));

        let mut code_result = self
            .run_role(AgentRole::Coder, &task, Some(&plan_result.output))
            .await?;

        Self::emit(
            &event_tx,
            MultiAgentEvent::AgentFinished(code_result.clone()),
        );

        // Step 3: Self-healing refinement loop — build → check → fix → repeat
        Self::emit(
            &event_tx,
            MultiAgentEvent::RefinementStarted {
                max_iterations: self.max_refinement_iterations,
            },
        );

        let mut clean_build = false;
        let mut iterations_used = 0;

        for iteration in 1..=self.max_refinement_iterations {
            iterations_used = iteration;

            let build_result = self.run_build_check().await;

            let (success, error_count, warning_count, raw_output) = match build_result {
                Ok(check) => check,
                Err(e) => {
                    Self::emit(
                        &event_tx,
                        MultiAgentEvent::Error(format!("Build check failed: {e}")),
                    );
                    break;
                }
            };

            Self::emit(
                &event_tx,
                MultiAgentEvent::BuildCheck {
                    iteration,
                    success,
                    error_count,
                    warning_count,
                    raw_output: raw_output.clone(),
                },
            );

            if success && error_count == 0 && warning_count == 0 {
                clean_build = true;
                break;
            }

            // Build had issues — feed errors back to the Coder for fixing
            let fix_prompt = format!(
                "## Build Errors (iteration {iteration}/{max})\n\
                 The code you wrote has build errors. Fix ALL of them.\n\n\
                 ```\n{raw_output}\n```\n\n\
                 ## Your Previous Code\n{prev_code}\n\n\
                 Fix every error and warning. Output the COMPLETE corrected code.",
                max = self.max_refinement_iterations,
                prev_code = code_result.output,
            );

            let fix_result = self
                .run_role(AgentRole::Coder, &task, Some(&fix_prompt))
                .await?;

            Self::emit(
                &event_tx,
                MultiAgentEvent::RefinementIteration {
                    iteration,
                    errors_remaining: error_count + warning_count,
                    fix_output: fix_result.output.clone(),
                },
            );

            code_result = fix_result;
        }

        Self::emit(
            &event_tx,
            MultiAgentEvent::RefinementComplete {
                iterations_used,
                clean_build,
            },
        );

        // Step 4: Reviewer checks the (now hopefully clean) code
        Self::emit(
            &event_tx,
            MultiAgentEvent::AgentStarted(AgentRole::Reviewer),
        );

        let review_context = format!(
            "## Plan\n{}\n\n## Implementation (after {} refinement iterations, build clean: {})\n{}",
            plan_result.output, iterations_used, clean_build, code_result.output
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
            refinement_iterations: iterations_used,
            clean_build,
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

        let client = self.client_for_role(&role);
        let response = client
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

    /// Run a build check against the project to detect errors and warnings.
    /// Returns (success, error_count, warning_count, raw_output).
    async fn run_build_check(&self) -> Result<(bool, usize, usize, String), PhazeError> {
        let project_dir = self.project_root.clone().unwrap_or_else(|| ".".to_string());

        let project_path = std::path::Path::new(&project_dir);

        // Detect language and choose the right check command
        let (command, args) = if project_path.join("Cargo.toml").exists() {
            ("cargo", vec!["check", "--message-format=short"])
        } else if project_path.join("tsconfig.json").exists() {
            ("npx", vec!["tsc", "--noEmit", "--pretty", "false"])
        } else if project_path.join("package.json").exists() {
            ("npx", vec!["eslint", ".", "--format", "compact"])
        } else if project_path.join("pyproject.toml").exists()
            || project_path.join("requirements.txt").exists()
        {
            ("python3", vec!["-m", "py_compile", "."])
        } else if project_path.join("go.mod").exists() {
            ("go", vec!["build", "./..."])
        } else {
            // Default to cargo for PhazeAI's own codebase
            ("cargo", vec!["check", "--message-format=short"])
        };

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            tokio::process::Command::new(command)
                .args(&args)
                .current_dir(&project_dir)
                .output(),
        )
        .await
        .map_err(|_| PhazeError::Other("Build check timed out (120s)".into()))?
        .map_err(|e| PhazeError::Other(format!("Failed to run {command}: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}\n{stderr}");

        // Truncate to avoid blowing up context windows
        let truncated = if combined.len() > 4000 {
            format!("{}\n... [output truncated]", &combined[..4000])
        } else {
            combined.clone()
        };

        let error_count = combined
            .lines()
            .filter(|l| l.contains("error") && !l.contains("aborting due to"))
            .count();
        let warning_count = combined.lines().filter(|l| l.contains("warning")).count();

        Ok((
            output.status.success(),
            error_count,
            warning_count,
            truncated,
        ))
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
    /// How many build→fix iterations were needed
    pub refinement_iterations: usize,
    /// Whether the final code produced a clean build
    pub clean_build: bool,
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
