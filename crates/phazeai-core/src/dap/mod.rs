//! Debug Adapter Protocol (DAP) client.
//!
//! Mirrors the LSP client pattern: spawns a debug adapter as a child process,
//! communicates via JSON-RPC over stdin/stdout with Content-Length framing.
//!
//! Phase 1: protocol types + basic client that can initialize, launch, set
//! breakpoints, and receive stopped events. UI integration (panels/debug.rs)
//! comes next.

mod client;
mod protocol;

pub use client::DapClient;
pub use protocol::*;
