mod client;
mod manager;
mod protocol;
mod tool;

pub use client::SidecarClient;
pub use manager::SidecarManager;
pub use protocol::{JsonRpcRequest, JsonRpcResponse};
pub use tool::{BuildIndexTool, SemanticSearchTool};
