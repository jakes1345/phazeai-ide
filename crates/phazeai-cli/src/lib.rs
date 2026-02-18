// Library interface for phazeai-cli
// This allows integration tests to access internal modules

// NOTE: Since commands.rs and theme.rs are also declared in main.rs,
// we need to use a path attribute to reference the same source file
// to avoid "file loaded multiple times" errors.

#[path = "commands.rs"]
pub mod commands;

#[path = "theme.rs"]
pub mod theme;

// Re-export commonly used items for easier testing
pub use commands::{handle_command, CommandResult};
pub use theme::Theme;
