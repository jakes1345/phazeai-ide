/// Result of processing a slash command.
pub enum CommandResult {
    /// Display a message to the user.
    Message(String),
    /// Clear the chat.
    Clear,
    /// Quit the application.
    Quit,
    /// Change the model.
    ModelChanged(String),
    /// Change the theme.
    ThemeChanged(String),
    /// Toggle file tree.
    ToggleFiles,
    /// Not a command - treat as regular input.
    NotACommand,
}

pub fn handle_command(input: &str) -> CommandResult {
    let parts: Vec<&str> = input.splitn(2, ' ').collect();
    let cmd = parts[0];
    let arg = parts.get(1).map(|s| s.trim()).unwrap_or("");

    match cmd {
        "/help" | "/h" => CommandResult::Message(
            "Commands:\n\
             /help       - Show this help\n\
             /clear      - Clear chat history\n\
             /model <m>  - Change LLM model\n\
             /theme <t>  - Change theme (dark, tokyo-night, dracula)\n\
             /files      - Toggle file tree panel\n\
             /compact    - Summarize conversation to save tokens\n\
             /exit       - Quit"
                .into(),
        ),
        "/exit" | "/quit" | "/q" => CommandResult::Quit,
        "/clear" => CommandResult::Clear,
        "/model" => {
            if arg.is_empty() {
                CommandResult::Message("Usage: /model <model-name>".into())
            } else {
                CommandResult::ModelChanged(arg.to_string())
            }
        }
        "/theme" => {
            if arg.is_empty() {
                let themes = crate::theme::Theme::all_names().join(", ");
                CommandResult::Message(format!("Available themes: {themes}"))
            } else {
                CommandResult::ThemeChanged(arg.to_string())
            }
        }
        "/files" | "/tree" => CommandResult::ToggleFiles,
        "/compact" => CommandResult::Message("Conversation compacted.".into()),
        _ => {
            if input.starts_with('/') {
                CommandResult::Message(format!("Unknown command: {cmd}. Type /help for commands."))
            } else {
                CommandResult::NotACommand
            }
        }
    }
}
