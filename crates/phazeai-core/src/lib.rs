pub mod error;
pub mod llm;
pub mod tools;
pub mod context;
pub mod config;
pub mod project;
pub mod agent;
pub mod analysis;
pub mod git;
pub mod lsp;

// Re-export key types
pub use error::PhazeError;
pub use agent::{Agent, AgentEvent, AgentResponse};
pub use llm::{LlmClient, LlmResponse, Message, Role, StreamEvent, ProviderId, ProviderRegistry, ModelInfo, UsageTracker, LocalDiscovery};
pub use tools::{Tool, ToolDefinition, ToolRegistry, ToolResult};
pub use context::{ConversationHistory, ContextBuilder, SystemPromptBuilder, ProjectType, collect_git_info, ConversationStore, ConversationMetadata, SavedConversation, SavedMessage};
pub use config::Settings;
