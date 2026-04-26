//! Commands that off-thread plugin code can send to the UI thread to mutate
//! the active editor. The receiver is drained inside `IdeState::new` via a
//! Floem `create_signal_from_channel` effect so all signal writes happen on
//! the UI thread.

use std::sync::mpsc::SyncSender;

#[derive(Clone)]
pub enum EditorCommand {
    /// Insert text at the active editor's cursor (replaces selection if any).
    InsertText(String),
    /// Execute a built-in IDE command. Result is delivered through `reply`.
    ExecuteCommand {
        cmd: String,
        args: String,
        reply: SyncSender<Result<String, String>>,
    },
}
