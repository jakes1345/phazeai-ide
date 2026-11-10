pub mod agent;
pub mod agent_event;
pub mod analysis;
pub mod checkpoint;
pub mod companion;
pub mod config;
pub mod constants;
pub mod context;
pub mod cost;
pub mod dap;
pub mod design;
pub mod error;
pub mod ext_host;
pub mod fim;
pub mod git;
pub mod hooks;
pub mod llm;
pub mod local_ai;
pub mod lsp;
pub mod mcp;
pub mod permissions;
pub mod privacy;
pub mod process;
pub mod project;
pub mod review;
pub mod search;
pub mod skills;
pub mod syntax;
pub mod telemetry;
pub mod text;
pub mod tools;
pub mod updater;

pub mod debug_ndjson;

// Re-export key types
pub use agent::{
    Agent, AgentResponse, ApprovalFn, DiffAnswer, DiffHookFn, SubagentConfig,
    DEFAULT_MAX_ITERATIONS,
};
pub use agent_event::AgentEvent;
pub use checkpoint::{Checkpoints, UndoReport};
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
pub use permissions::{Decision, Permissions};
pub use search::{BuildIndexTool, CodeSearch, CodeSearchTool};
pub use skills::{discover_skills, find_skill, skills_menu_block, Skill, SkillSource};
pub use tools::{Tool, ToolDefinition, ToolRegistry, ToolResult};
