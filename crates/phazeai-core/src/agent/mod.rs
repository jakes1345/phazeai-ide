mod core;
pub mod multi_agent;

pub use core::{Agent, AgentEvent, AgentResponse, ApprovalFn};
pub use multi_agent::{
    AgentRole, AgentRoleResult, AgentTask, MultiAgentEvent, MultiAgentOrchestrator, PipelineResult,
};
