mod core;
pub mod multi_agent;

pub use crate::agent_event::AgentEvent;
pub use core::{Agent, AgentResponse, ApprovalFn};
pub use multi_agent::{
    AgentRole, AgentRoleResult, AgentTask, MultiAgentEvent, MultiAgentOrchestrator, PipelineResult,
};
