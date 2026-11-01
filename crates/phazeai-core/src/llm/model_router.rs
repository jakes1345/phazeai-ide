use crate::error::PhazeError;
use crate::llm::provider::{ProviderId, ProviderRegistry};
use crate::llm::traits::*;
use crate::tools::ToolDefinition;
use futures::channel::mpsc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The type of task, used to route to the optimal model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    /// Complex reasoning, architecture decisions, multi-step planning
    Reasoning,
    /// Deciding which tool to call and with what parameters
    ToolOrchestration,
    /// Writing new code, implementing features
    CodeGeneration,
    /// Reviewing diffs, finding bugs, code analysis
    CodeReview,
    /// Simple factual answers, quick lookups
    QuickAnswer,
}

impl TaskType {
    /// All defined task types
    pub fn all() -> &'static [TaskType] {
        &[
            TaskType::Reasoning,
            TaskType::ToolOrchestration,
            TaskType::CodeGeneration,
            TaskType::CodeReview,
            TaskType::QuickAnswer,
        ]
    }

    pub fn name(&self) -> &str {
        match self {
            TaskType::Reasoning => "reasoning",
            TaskType::ToolOrchestration => "tool_orchestration",
            TaskType::CodeGeneration => "code_generation",
            TaskType::CodeReview => "code_review",
            TaskType::QuickAnswer => "quick_answer",
        }
    }

    /// Classify user input into a task type based on heuristics.
    pub fn classify(input: &str, has_tools: bool) -> Self {
        let lower = input.to_lowercase();

        // Reasoning keywords - should take priority because complex things can be short
        if starts_word(&lower, "explain")
            || starts_word(&lower, "why")
            || starts_word(&lower, "design")
            || starts_word(&lower, "architect")
            || starts_word(&lower, "plan")
            || starts_word(&lower, "trade-off")
            || starts_word(&lower, "tradeoff")
            || starts_word(&lower, "compare")
            || starts_word(&lower, "perspective")
        {
            return TaskType::Reasoning;
        }

        // If the LLM is being asked to use tools, it's orchestration
        if has_tools {
            return TaskType::ToolOrchestration;
        }

        // Code generation signals
        if starts_word(&lower, "write")
            || starts_word(&lower, "implement")
            || starts_word(&lower, "create")
            || starts_word(&lower, "add a")
            || starts_word(&lower, "build")
            || starts_word(&lower, "generate")
            || starts_word(&lower, "code for")
        {
            return TaskType::CodeGeneration;
        }

        // Code review signals
        if starts_word(&lower, "review")
            || starts_word(&lower, "bug")
            || starts_word(&lower, "wrong")
            || starts_word(&lower, "fix")
            || starts_word(&lower, "issue")
            || starts_word(&lower, "diff")
            || starts_word(&lower, "error")
        {
            return TaskType::CodeReview;
        }

        // Simple question signals - only if not already classified as reasoning/gen/review
        if starts_word(&lower, "what is")
            || starts_word(&lower, "how do")
            || starts_word(&lower, "what does")
            || lower.len() < 80
        {
            return TaskType::QuickAnswer;
        }

        // Default to reasoning for anything complex
        TaskType::Reasoning
    }
}

/// Whether `keyword` appears in `text` starting at a word boundary, so
/// "plan" matches "plan" and "planning" but not "explanation".
fn starts_word(text: &str, keyword: &str) -> bool {
    text.match_indices(keyword).any(|(i, _)| {
        text[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric())
    })
}

/// A route mapping a task type to a specific provider/model pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRoute {
    pub provider: String,
    pub model: String,
}

/// Routes different task types to different LLM provider/model pairs.
///
/// When no route is configured for a task type, falls back to the default client.
pub struct ModelRouter {
    /// Pre-built clients for each routed task type
    routes: HashMap<TaskType, Box<dyn LlmClient>>,
    /// The default client used when no specific route matches
    default_client: Box<dyn LlmClient>,
}

impl ModelRouter {
    /// Build a ModelRouter from route configs and a provider registry.
    ///
    /// Routes that fail to build (e.g., missing API key) are silently skipped
    /// and will fall back to the default client.
    pub fn new(
        route_configs: &HashMap<TaskType, ModelRoute>,
        registry: &ProviderRegistry,
        default_client: Box<dyn LlmClient>,
    ) -> Self {
        let mut routes: HashMap<TaskType, Box<dyn LlmClient>> = HashMap::new();

        for (task_type, route) in route_configs {
            let provider_id = Self::parse_provider_id(&route.provider);
            if let Some(config) = registry.get_config(&provider_id) {
                match registry.build_client_for(config, &route.model) {
                    Ok(client) => {
                        tracing::info!(
                            "Model route: {:?} -> {} / {}",
                            task_type,
                            route.provider,
                            route.model
                        );
                        routes.insert(*task_type, client);
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to build client for route {:?}: {}. Using default.",
                            task_type,
                            e
                        );
                    }
                }
            } else {
                tracing::warn!(
                    "Provider '{}' not found for route {:?}. Using default.",
                    route.provider,
                    task_type
                );
            }
        }

        Self {
            routes,
            default_client,
        }
    }

    /// Get the LLM client for a given task type.
    pub fn client_for(&self, task_type: TaskType) -> &dyn LlmClient {
        self.routes
            .get(&task_type)
            .map(|c| c.as_ref())
            .unwrap_or(self.default_client.as_ref())
    }

    /// Get the default client.
    pub fn default_client(&self) -> &dyn LlmClient {
        self.default_client.as_ref()
    }

    /// How many task types have custom routes (not using default).
    pub fn routed_count(&self) -> usize {
        self.routes.len()
    }

    fn parse_provider_id(name: &str) -> ProviderId {
        match name.to_lowercase().as_str() {
            "claude" | "anthropic" => ProviderId::Claude,
            "openai" => ProviderId::OpenAI,
            "ollama" => ProviderId::Ollama,
            "groq" => ProviderId::Groq,
            "together" => ProviderId::Together,
            "openrouter" => ProviderId::OpenRouter,
            "lmstudio" | "lm_studio" => ProviderId::LmStudio,
            "gemini" => ProviderId::Gemini,
            "mistral" => ProviderId::Mistral,
            "deepseek" => ProviderId::DeepSeek,
            "xai" | "grok" => ProviderId::Xai,
            "cohere" => ProviderId::Cohere,
            "perplexity" => ProviderId::Perplexity,
            "azure" | "azure_openai" => ProviderId::Azure,
            "cerebras" => ProviderId::Cerebras,
            "fireworks" => ProviderId::Fireworks,
            "sambanova" => ProviderId::SambaNova,
            "github" | "github_models" | "githubmodels" => ProviderId::GithubModels,
            other => ProviderId::Custom(other.to_string()),
        }
    }
}

/// Implement LlmClient on ModelRouter so it can be used as a drop-in replacement.
/// Uses ToolOrchestration route when tools are present, Reasoning otherwise.
#[async_trait::async_trait]
impl LlmClient for ModelRouter {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse, PhazeError> {
        let task_type = if !tools.is_empty() {
            TaskType::ToolOrchestration
        } else {
            // Classify from the last user message
            let last_user = messages
                .iter()
                .rev()
                .find(|m| m.role == Role::User)
                .map(|m| m.content.as_str())
                .unwrap_or("");
            TaskType::classify(last_user, false)
        };

        self.client_for(task_type).chat(messages, tools).await
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<mpsc::UnboundedReceiver<StreamEvent>, PhazeError> {
        let task_type = if !tools.is_empty() {
            TaskType::ToolOrchestration
        } else {
            let last_user = messages
                .iter()
                .rev()
                .find(|m| m.role == Role::User)
                .map(|m| m.content.as_str())
                .unwrap_or("");
            TaskType::classify(last_user, false)
        };

        self.client_for(task_type)
            .chat_stream(messages, tools)
            .await
    }
}

#[cfg(test)]
mod word_tests {
    use super::TaskType;

    #[test]
    fn keywords_match_whole_words_not_parts_of_words() {
        // Every IDE helper prompt says "no explanation": that is not planning.
        let commit = "Write a concise git commit message. Reply with ONLY the commit message, no explanation.";
        assert_eq!(TaskType::classify(commit, false), TaskType::CodeGeneration);
        assert_eq!(
            TaskType::classify("make a plan for the migration", false),
            TaskType::Reasoning
        );
        assert_eq!(
            TaskType::classify(
                "we are planning a refactor of the parser module here",
                false
            ),
            TaskType::Reasoning
        );
        assert_eq!(
            TaskType::classify("tell me about the planet mars", false),
            TaskType::Reasoning,
            "word-start match keeps stems"
        );
        assert_eq!(
            TaskType::classify("rename the prefix field", false),
            TaskType::QuickAnswer,
            "'prefix' is not 'fix'"
        );
        assert_eq!(
            TaskType::classify("fix the crash in main", false),
            TaskType::CodeReview
        );
    }
}
