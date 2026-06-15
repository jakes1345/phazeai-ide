pub mod agent;
pub mod agent_event;
pub mod analysis;
pub mod companion;
pub mod config;
pub mod constants;
pub mod context;
pub mod dap;
pub mod error;
pub mod ext_host;
pub mod git;
pub mod llm;
pub mod lsp;
pub mod mcp;
pub mod project;
pub mod syntax;
pub mod telemetry;
pub mod tools;
pub mod updater;

pub mod debug_ndjson;

// Re-export key types
pub use agent::{Agent, AgentResponse, ApprovalFn};
pub use agent_event::AgentEvent;
pub use config::Settings;
pub use context::{
    collect_git_info, ContextBuilder, ConversationHistory, ConversationMetadata, ConversationStore,
    ProjectType, RepoMapGenerator, SavedConversation, SavedMessage, SystemPromptBuilder,
};
pub use error::PhazeError;
pub use llm::{
    LlmClient, LlmResponse, LocalDiscovery, Message, ModelInfo, ProviderId, ProviderRegistry, Role,
    StreamEvent, UsageTracker,
};
pub use lsp::{LspClient, LspEvent, LspManager};
pub use tools::{Tool, ToolDefinition, ToolRegistry, ToolResult};
