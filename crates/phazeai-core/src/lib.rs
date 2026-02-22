pub mod agent;
pub mod analysis;
pub mod config;
pub mod context;
pub mod error;
pub mod git;
pub mod llm;
pub mod lsp;
pub mod project;
pub mod tools;

// Re-export key types
pub use agent::{Agent, AgentEvent, AgentResponse, ApprovalFn};
pub use config::Settings;
pub use context::{
    collect_git_info, ContextBuilder, ConversationHistory, ConversationMetadata, ConversationStore,
    ProjectType, SavedConversation, SavedMessage, SystemPromptBuilder,
};
pub use error::PhazeError;
pub use llm::{
    LlmClient, LlmResponse, LocalDiscovery, Message, ModelInfo, ProviderId, ProviderRegistry, Role,
    StreamEvent, UsageTracker,
};
pub use lsp::{LspClient, LspEvent, LspManager};
pub use tools::{Tool, ToolDefinition, ToolRegistry, ToolResult};
