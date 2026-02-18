mod manager;
mod protocol;
mod client;
mod tool;

pub use manager::SidecarManager;
pub use protocol::{JsonRpcRequest, JsonRpcResponse};
pub use client::SidecarClient;
pub use tool::{SemanticSearchTool, BuildIndexTool};
