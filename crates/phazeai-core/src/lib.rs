pub mod error;
pub mod llm;
pub mod tools;
pub mod context;
pub mod config;
pub mod project;
pub mod agent;
pub mod analysis;
pub mod git;

// Re-export key types
pub use error::PhazeError;
pub use agent::{Agent, AgentEvent, AgentResponse};
pub use llm::{LlmClient, LlmResponse, Message, Role, StreamEvent};
pub use tools::{Tool, ToolDefinition, ToolRegistry, ToolResult};
pub use context::{ConversationHistory, ContextBuilder};
pub use config::Settings;
