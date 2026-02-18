/// Result of processing a slash command.
#[derive(Debug, Clone)]
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
    /// Change the LLM provider.
    ProviderChanged(String),
    /// Compact/summarize conversation to save tokens.
    Compact,
    /// Save current conversation.
    SaveConversation,
    /// Load conversation by ID.
    LoadConversation(String),
    /// List saved conversations.
    ListConversations,
    /// Start a fresh conversation.
    NewConversation,
    /// Set tool approval mode (auto, ask, ask-once).
    SetApprovalMode(String),
    /// Show status (token count, model, etc.).
    ShowStatus,
    /// Show git diff.
    ShowDiff,
    /// Show git status.
    ShowGitStatus,
    /// Show git log.
    ShowLog,
    /// Search files with glob pattern.
    SearchFiles(String),
    /// List available models for current provider or discover local models.
    ListModels,
    /// Discover local model providers.
    DiscoverModels,
    /// Show project context information.
    ShowContext,
}

pub fn handle_command(input: &str) -> CommandResult {
    let parts: Vec<&str> = input.splitn(2, ' ').collect();
    let cmd = parts[0];
    let arg = parts.get(1).map(|s| s.trim()).unwrap_or("");

    match cmd {
        "/help" | "/h" => show_help(),
        "/exit" | "/quit" | "/q" => CommandResult::Quit,
        "/clear" => CommandResult::Clear,
        "/new" => CommandResult::NewConversation,

        // Model commands
        "/model" => {
            if arg.is_empty() {
                CommandResult::Message("Current model info will be shown in /status. Use: /model <model-name>".into())
            } else {
                CommandResult::ModelChanged(arg.to_string())
            }
        }
        "/provider" => {
            if arg.is_empty() {
                CommandResult::Message("Available providers: anthropic, openai, ollama\nUsage: /provider <provider-name>".into())
            } else {
                CommandResult::ProviderChanged(arg.to_string())
            }
        }
        "/approve" => {
            if arg.is_empty() {
                CommandResult::Message("Tool approval modes:\n  auto - approve all tools automatically\n  ask - ask before each tool\n  ask-once - ask once per tool type\nUsage: /approve <mode>".into())
            } else {
                match arg {
                    "auto" | "ask" | "ask-once" => CommandResult::SetApprovalMode(arg.to_string()),
                    _ => CommandResult::Message("Invalid approval mode. Options: auto, ask, ask-once".into()),
                }
            }
        }
        "/cost" => CommandResult::ShowStatus, // Token usage and cost

        // Display commands
        "/theme" => {
            if arg.is_empty() {
                let themes = crate::theme::Theme::all_names().join(", ");
                CommandResult::Message(format!("Available themes: {themes}\nUsage: /theme <theme-name>"))
            } else {
                CommandResult::ThemeChanged(arg.to_string())
            }
        }
        "/files" | "/tree" => CommandResult::ToggleFiles,
        "/status" => CommandResult::ShowStatus,

        // Conversation commands
        "/compact" => CommandResult::Compact,
        "/save" => CommandResult::SaveConversation,
        "/load" => {
            if arg.is_empty() {
                CommandResult::Message("Usage: /load <conversation-id>".into())
            } else {
                CommandResult::LoadConversation(arg.to_string())
            }
        }
        "/conversations" | "/history" => CommandResult::ListConversations,

        // Project commands
        "/diff" => CommandResult::ShowDiff,
        "/git" => CommandResult::ShowGitStatus,
        "/log" => CommandResult::ShowLog,
        "/search" => {
            if arg.is_empty() {
                CommandResult::Message("Usage: /search <glob-pattern>\nExample: /search **/*.rs".into())
            } else {
                CommandResult::SearchFiles(arg.to_string())
            }
        }
        "/pwd" => {
            match std::env::current_dir() {
                Ok(path) => CommandResult::Message(format!("Current directory: {}", path.display())),
                Err(e) => CommandResult::Message(format!("Error getting current directory: {}", e)),
            }
        }
        "/cd" => {
            if arg.is_empty() {
                CommandResult::Message("Usage: /cd <directory>".into())
            } else {
                match std::env::set_current_dir(arg) {
                    Ok(_) => match std::env::current_dir() {
                        Ok(path) => CommandResult::Message(format!("Changed directory to: {}", path.display())),
                        Err(_) => CommandResult::Message("Directory changed.".into()),
                    },
                    Err(e) => CommandResult::Message(format!("Error changing directory: {}", e)),
                }
            }
        }
        "/version" => CommandResult::Message(format!("PhazeAI CLI v{}", env!("CARGO_PKG_VERSION"))),
        "/models" => CommandResult::ListModels,
        "/discover" => CommandResult::DiscoverModels,
        "/context" => CommandResult::ShowContext,

        // Unknown command
        _ => {
            if input.starts_with('/') {
                CommandResult::Message(format!("Unknown command: {cmd}. Type /help for commands."))
            } else {
                CommandResult::NotACommand
            }
        }
    }
}

fn show_help() -> CommandResult {
    let help_text = "\
╭─ PhazeAI CLI Commands ─────────────────────────────────────────╮

  CHAT MANAGEMENT
    /clear                    Clear chat history
    /new                      Start a fresh conversation
    /compact                  Summarize conversation to save tokens
    /save                     Save current conversation
    /load <id>                Load conversation by ID
    /conversations, /history  List saved conversations

  MODEL & PROVIDER
    /model <name>             Change LLM model
    /models                   List available models for current provider
    /provider <name>          Change provider (anthropic, openai, ollama)
    /discover                 Scan for local model providers (Ollama, LM Studio)
    /approve <mode>           Set tool approval (auto, ask, ask-once)
    /cost                     Show token usage and estimated cost

  DISPLAY & UI
    /theme <name>             Change color theme
    /files, /tree             Toggle file tree panel
    /status                   Show status (model, tokens, directory)

  PROJECT & GIT
    /diff                     Show git diff
    /git                      Show git status
    /log                      Show git log (last 20 commits)
    /search <pattern>         Search files with glob pattern
    /pwd                      Show current directory
    /cd <dir>                 Change directory
    /context                  Show loaded project context

  OTHER
    /help, /h                 Show this help message
    /version                  Show version information
    /exit, /quit, /q          Quit the application

╰────────────────────────────────────────────────────────────────╯";

    CommandResult::Message(help_text.into())
}
