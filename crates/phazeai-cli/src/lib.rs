//! PhazeAI terminal UI.
//!
//! The binary (`src/main.rs`) is a thin wrapper around [`app`]; everything
//! lives in this library so the pure pieces (markdown rendering, wrapping,
//! input editing, command parsing…) are unit-testable.

pub mod app;
pub mod commands;
pub mod companion;
pub mod input;
pub mod markdown;
pub mod oneshot;
pub mod setup;
pub mod term;
pub mod text;
pub mod theme;
pub mod tool_view;

// Re-export commonly used items for easier testing
pub use commands::{handle_command, CommandResult};
pub use theme::Theme;
