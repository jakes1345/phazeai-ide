mod manager;
mod protocol;
mod client;

pub use manager::SidecarManager;
pub use protocol::{JsonRpcRequest, JsonRpcResponse};
pub use client::SidecarClient;
