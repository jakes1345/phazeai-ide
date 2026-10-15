//! Slash-command parsing.
//!
//! [`COMMANDS`] is the single source of truth for the command list: `/help`,
//! Tab completion and the inline command palette are all generated from it,
//! and a unit test guarantees every entry is actually handled by
//! [`handle_command`].

/// Result of processing a slash command.
#[derive(Debug, Clone)]
pub enum CommandResult {
    /// Display a message to the user.
    Message(String),
    /// Clear the chat and the model's conversation context.
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
    /// Change the LLM provider (optionally followed by a model name).
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
    /// Switch AI mode (plan, debug, chat, ask, edit).
    SetMode(String),
    /// Add a file to the conversation context (/add <path>).
    AddFile(String),
    /// Retry the last user message.
    Retry,
    /// Cancel the current agent run.
    Cancel,
    /// Grep for a pattern in the project (/grep <pattern>).
    Grep(String),
    /// Run a custom agent skill or command (.md file).
    RunSkill { name: String, args: String },
    /// Set up a PhazeAI GitHub Action workflow.
    InstallGithubApp,
    /// Undo the last AI-made file changes via git (requires explicit confirmation).
    UndoConfirmed,
    /// Copy the last assistant reply (or its last code block) to the clipboard.
    Copy { code_only: bool },
    /// Store an API key for the active provider in the OS keyring.
    SetApiKey(String),
}

/// Grouping used when rendering `/help`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Chat,
    Model,
    Mode,
    Project,
    Session,
    Other,
}

impl Group {
    fn title(self) -> &'static str {
        match self {
            Group::Chat => "CHAT",
            Group::Model => "MODEL & PROVIDER",
            Group::Mode => "AI MODES",
            Group::Project => "PROJECT & GIT",
            Group::Session => "SESSION CONTROL",
            Group::Other => "OTHER",
        }
    }

    const ORDER: [Group; 6] = [
        Group::Chat,
        Group::Model,
        Group::Mode,
        Group::Project,
        Group::Session,
        Group::Other,
    ];
}

/// Static description of one slash command.
#[derive(Debug)]
pub struct CommandSpec {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    /// Argument synopsis, empty when the command takes none.
    pub args: &'static str,
    pub help: &'static str,
    pub group: Group,
}

macro_rules! cmd {
    ($name:expr, [$($alias:expr),*], $args:expr, $help:expr, $group:ident) => {
        CommandSpec {
            name: $name,
            aliases: &[$($alias),*],
            args: $args,
            help: $help,
            group: Group::$group,
        }
    };
}

pub static COMMANDS: &[CommandSpec] = &[
    cmd!(
        "/help",
        ["/h"],
        "",
        "Show commands and keyboard shortcuts",
        Chat
    ),
    cmd!(
        "/clear",
        [],
        "",
        "Clear the screen and the model's context",
        Chat
    ),
    cmd!(
        "/new",
        [],
        "",
        "Save this conversation and start a fresh one",
        Chat
    ),
    cmd!(
        "/compact",
        [],
        "",
        "Summarise older messages to save tokens",
        Chat
    ),
    cmd!("/save", [], "", "Save the current conversation", Chat),
    cmd!(
        "/load",
        [],
        "<id>",
        "Load a saved conversation (prefix match)",
        Chat
    ),
    cmd!(
        "/conversations",
        ["/history"],
        "",
        "List saved conversations",
        Chat
    ),
    cmd!(
        "/copy",
        [],
        "[code]",
        "Copy the last reply (or its last code block)",
        Chat
    ),
    cmd!(
        "/model",
        [],
        "<name>",
        "Switch model for this session",
        Model
    ),
    cmd!(
        "/models",
        [],
        "",
        "List known models for the active provider",
        Model
    ),
    cmd!(
        "/provider",
        [],
        "<name> [model]",
        "Switch provider for this session",
        Model
    ),
    cmd!(
        "/key",
        [],
        "<api-key>",
        "Save an API key for the active provider in the OS keyring",
        Model
    ),
    cmd!(
        "/local",
        [],
        "",
        "Show the built-in local AI: model, server, llama.cpp",
        Model
    ),
    cmd!(
        "/discover",
        [],
        "",
        "Scan for local Ollama / LM Studio models",
        Model
    ),
    cmd!(
        "/approve",
        [],
        "<auto|ask|ask-once>",
        "Set tool approval mode",
        Model
    ),
    cmd!("/yolo", [], "", "Auto-approve every tool call", Model),
    cmd!(
        "/status",
        ["/cost", "/tokens"],
        "",
        "Provider, model, tokens, cost and context",
        Model
    ),
    cmd!("/theme", [], "<name>", "Change colour theme", Model),
    cmd!(
        "/mode",
        [],
        "<chat|ask|plan|debug|edit>",
        "Switch AI mode",
        Mode
    ),
    cmd!("/chat", [], "", "General conversation (default)", Mode),
    cmd!(
        "/ask",
        [],
        "",
        "Read-only: answer without editing files",
        Mode
    ),
    cmd!(
        "/plan",
        [],
        "",
        "Produce a step-by-step plan, no code",
        Mode
    ),
    cmd!("/debug", [], "", "Diagnose the root cause first", Mode),
    cmd!("/edit", [], "", "Make minimal, targeted code changes", Mode),
    cmd!("/diff", [], "", "Show uncommitted git diff", Project),
    cmd!("/git", [], "", "Show git status", Project),
    cmd!("/log", [], "", "Show the last 20 commits", Project),
    cmd!(
        "/search",
        [],
        "<glob>",
        "Find files by glob pattern",
        Project
    ),
    cmd!("/grep", [], "<pattern>", "Search file contents", Project),
    cmd!(
        "/add",
        [],
        "<file>",
        "Add a file's contents to the context",
        Project
    ),
    cmd!(
        "/files",
        ["/tree"],
        "",
        "Toggle the file tree pane",
        Project
    ),
    cmd!(
        "/context",
        [],
        "",
        "Show loaded instructions and project info",
        Project
    ),
    cmd!("/pwd", [], "", "Show the working directory", Project),
    cmd!("/cd", [], "<dir>", "Change the working directory", Project),
    cmd!("/retry", [], "", "Resend the last prompt", Session),
    cmd!(
        "/cancel",
        ["/stop"],
        "",
        "Interrupt the running request",
        Session
    ),
    cmd!(
        "/undo",
        [],
        "confirm",
        "Discard ALL uncommitted changes (git checkout)",
        Session
    ),
    cmd!(
        "/skill",
        ["/cmd", "/run"],
        "<name> [args]",
        "Run .phazeai/commands/<name>.md",
        Session
    ),
    cmd!(
        "/install-github-action",
        ["/install-github-app", "/setup-github-action"],
        "",
        "Write a PhazeAI GitHub Actions workflow",
        Other
    ),
    cmd!("/version", [], "", "Show version", Other),
    cmd!("/exit", ["/quit", "/q"], "", "Quit", Other),
];

/// Keyboard shortcuts shown in `/help`.
pub static SHORTCUTS: &[(&str, &str)] = &[
    ("Enter", "Send (or accept the highlighted command)"),
    (
        "Shift+Enter / Alt+Enter / Ctrl+J",
        "Insert a newline (also: end a line with \\)",
    ),
    (
        "Up / Down",
        "Move between lines, then browse prompt history",
    ),
    ("Tab", "Complete slash command"),
    ("Esc", "Interrupt the running request / close popup"),
    ("Ctrl+C", "Interrupt, clear the input, or (twice) quit"),
    ("Ctrl+D", "Quit when the input is empty"),
    (
        "PgUp / PgDn, Shift+Up / Shift+Down",
        "Scroll the conversation",
    ),
    ("Ctrl+Home / Ctrl+End", "Jump to top / bottom"),
    ("Ctrl+T", "Expand / collapse tool output"),
    ("Ctrl+O", "Browse saved sessions"),
    ("Ctrl+N", "New conversation"),
    ("Ctrl+S", "Save conversation"),
    ("Ctrl+L", "Clear the screen (keeps context)"),
    ("Ctrl+B", "Toggle file tree"),
    ("Ctrl+E", "Edit the prompt in $EDITOR"),
    ("Ctrl+V", "Paste from clipboard"),
    (
        "Ctrl+A/E, Ctrl+U/K, Ctrl+W",
        "Line start / editor, kill line, delete word",
    ),
];

impl CommandSpec {
    pub fn matches_prefix(&self, prefix: &str) -> bool {
        self.name.starts_with(prefix) || self.aliases.iter().any(|a| a.starts_with(prefix))
    }

    pub fn is_named(&self, name: &str) -> bool {
        self.name == name || self.aliases.contains(&name)
    }
}

/// Commands whose name or alias starts with the first word of `input`.
/// Exact matches come first. Empty when `input` is not a bare `/word`.
pub fn matching_commands(input: &str) -> Vec<&'static CommandSpec> {
    if !input.starts_with('/') || input.contains(char::is_whitespace) {
        return Vec::new();
    }
    let mut out: Vec<&'static CommandSpec> = COMMANDS
        .iter()
        .filter(|c| c.matches_prefix(input))
        .collect();
    out.sort_by_key(|c| (!c.is_named(input), !c.name.starts_with(input)));
    out
}

/// Look up a command by exact name or alias.
pub fn find_command(name: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|c| c.is_named(name))
}

/// Tab completion: a unique match completes to `"<name> "`; several matches
/// complete to their longest common prefix (if longer than the input).
pub fn complete_command(input: &str) -> Option<String> {
    let matches = matching_commands(input);
    match matches.as_slice() {
        [] => None,
        [only] => Some(format!("{} ", only.name)),
        many => {
            if let Some(exact) = many.iter().find(|c| c.is_named(input)) {
                return Some(format!("{} ", exact.name));
            }
            let names: Vec<&str> = many.iter().map(|c| c.name).collect();
            let first = names[0];
            let mut len = first.len();
            for n in &names[1..] {
                len = len.min(
                    first
                        .bytes()
                        .zip(n.bytes())
                        .take_while(|(a, b)| a == b)
                        .count(),
                );
            }
            (len > input.len()).then(|| first[..len].to_string())
        }
    }
}

/// Whether `input` should be kept out of prompt history and masked on screen
/// (it contains a secret).
pub fn is_secret_input(input: &str) -> bool {
    let first = input.split_whitespace().next().unwrap_or("");
    first == "/key"
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
                CommandResult::Message(
                    "Usage: /model <model-name>   (see /models for known models)".into(),
                )
            } else {
                CommandResult::ModelChanged(arg.to_string())
            }
        }
        "/provider" => {
            if arg.is_empty() {
                CommandResult::Message(format!(
                    "Available providers: {}\nUsage: /provider <provider-name> [model]",
                    crate::setup::PROVIDER_NAMES.join(", ")
                ))
            } else {
                CommandResult::ProviderChanged(arg.to_string())
            }
        }
        "/key" => {
            if arg.is_empty() {
                CommandResult::Message(
                    "Usage: /key <api-key>\nStores the key in your OS keyring for the active provider. \
                     The key is never written to history or shown on screen."
                        .into(),
                )
            } else {
                CommandResult::SetApiKey(arg.to_string())
            }
        }
        "/approve" => {
            if arg.is_empty() {
                CommandResult::Message("Tool approval modes:\n  auto - approve all tools automatically\n  ask - ask before each tool\n  ask-once - ask once per tool type\nUsage: /approve <mode>".into())
            } else {
                match arg {
                    "auto" | "ask" | "ask-once" => CommandResult::SetApprovalMode(arg.to_string()),
                    _ => CommandResult::Message(
                        "Invalid approval mode. Options: auto, ask, ask-once".into(),
                    ),
                }
            }
        }
        "/cost" | "/tokens" | "/status" => CommandResult::ShowStatus,

        // Display commands
        "/theme" => {
            if arg.is_empty() {
                let themes = crate::theme::Theme::all_names().join(", ");
                CommandResult::Message(format!(
                    "Available themes: {themes}\nUsage: /theme <theme-name>"
                ))
            } else {
                CommandResult::ThemeChanged(arg.to_string())
            }
        }
        "/files" | "/tree" => CommandResult::ToggleFiles,
        "/copy" => match arg {
            "" => CommandResult::Copy { code_only: false },
            "code" => CommandResult::Copy { code_only: true },
            _ => CommandResult::Message("Usage: /copy [code]".into()),
        },

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
                CommandResult::Message(
                    "Usage: /search <glob-pattern>\nExample: /search **/*.rs".into(),
                )
            } else {
                CommandResult::SearchFiles(arg.to_string())
            }
        }
        "/pwd" => match std::env::current_dir() {
            Ok(path) => CommandResult::Message(format!("Current directory: {}", path.display())),
            Err(e) => CommandResult::Message(format!("Error getting current directory: {}", e)),
        },
        "/cd" => {
            if arg.is_empty() {
                CommandResult::Message("Usage: /cd <directory>".into())
            } else {
                match std::env::set_current_dir(arg) {
                    Ok(_) => match std::env::current_dir() {
                        Ok(path) => CommandResult::Message(format!(
                            "Changed directory to: {}",
                            path.display()
                        )),
                        Err(_) => CommandResult::Message("Directory changed.".into()),
                    },
                    Err(e) => CommandResult::Message(format!("Error changing directory: {}", e)),
                }
            }
        }
        "/version" => CommandResult::Message(format!("PhazeAI CLI v{}", env!("CARGO_PKG_VERSION"))),
        "/models" => CommandResult::ListModels,
        "/discover" => CommandResult::DiscoverModels,
        "/local" => CommandResult::Message(crate::setup::local_ai_status_text(
            &phazeai_core::Settings::load(),
        )),
        "/context" => CommandResult::ShowContext,

        // AI Mode switching
        "/mode" => {
            let modes = "plan, debug, chat, ask, edit";
            if arg.is_empty() {
                CommandResult::Message(format!("Usage: /mode <mode>\nAvailable modes: {modes}"))
            } else {
                match arg {
                    "plan" | "debug" | "chat" | "ask" | "edit" => {
                        CommandResult::SetMode(arg.to_string())
                    }
                    _ => {
                        CommandResult::Message(format!("Unknown mode '{arg}'. Available: {modes}"))
                    }
                }
            }
        }
        "/plan" => CommandResult::SetMode("plan".into()),
        "/debug" => CommandResult::SetMode("debug".into()),
        "/ask" => CommandResult::SetMode("ask".into()),
        "/edit" => CommandResult::SetMode("edit".into()),
        "/chat" => CommandResult::SetMode("chat".into()),

        // Context and session control
        "/add" => {
            if arg.is_empty() {
                CommandResult::Message(
                    "Usage: /add <file-path>\nAdds file contents to the conversation context."
                        .into(),
                )
            } else {
                CommandResult::AddFile(arg.to_string())
            }
        }
        "/retry" => CommandResult::Retry,
        "/cancel" | "/stop" => CommandResult::Cancel,
        "/undo" => {
            if matches!(arg, "confirm" | "--confirm") {
                CommandResult::UndoConfirmed
            } else {
                CommandResult::Message(
                    "Undo is destructive and discards all uncommitted changes.\n\
                     Run `/undo confirm` to proceed."
                        .into(),
                )
            }
        }
        "/yolo" => CommandResult::SetApprovalMode("auto".into()),
        "/grep" => {
            if arg.is_empty() {
                CommandResult::Message(
                    "Usage: /grep <pattern>\nSearch for pattern in project files.".into(),
                )
            } else {
                CommandResult::Grep(arg.to_string())
            }
        }
        "/skill" | "/cmd" | "/run" => {
            let parts: Vec<&str> = arg.splitn(2, ' ').collect();
            if parts[0].is_empty() {
                CommandResult::Message("Usage: /skill <name> [args...]\nRuns a custom command from .phazeai/commands/<name>.md".into())
            } else {
                let name = parts[0].to_string();
                let params = parts.get(1).unwrap_or(&"").to_string();
                CommandResult::RunSkill { name, args: params }
            }
        }
        "/install-github-action" | "/install-github-app" | "/setup-github-action" => {
            CommandResult::InstallGithubApp
        }

        // Unknown command
        _ => {
            if input.starts_with('/') {
                let hint = matching_commands(cmd)
                    .first()
                    .map(|c| format!(" Did you mean {}?", c.name))
                    .unwrap_or_default();
                CommandResult::Message(format!(
                    "Unknown command: {cmd}.{hint} Type /help for commands."
                ))
            } else {
                CommandResult::NotACommand
            }
        }
    }
}

/// Build the `/help` text from [`COMMANDS`] and [`SHORTCUTS`].
pub fn help_text() -> String {
    let synopsis = |c: &CommandSpec| {
        if c.args.is_empty() {
            c.name.to_string()
        } else {
            format!("{} {}", c.name, c.args)
        }
    };
    let col = COMMANDS
        .iter()
        .map(|c| synopsis(c).chars().count())
        .max()
        .unwrap_or(20)
        + 2;

    let mut out = String::from("PhazeAI CLI Commands\n");
    for group in Group::ORDER {
        out.push('\n');
        out.push_str(group.title());
        out.push('\n');
        for c in COMMANDS.iter().filter(|c| c.group == group) {
            let syn = synopsis(c);
            let pad = col.saturating_sub(syn.chars().count());
            out.push_str(&format!("  {syn}{}{}", " ".repeat(pad), c.help));
            if !c.aliases.is_empty() {
                out.push_str(&format!("  ({})", c.aliases.join(", ")));
            }
            out.push('\n');
        }
    }
    out.push_str("\nKEYBOARD\n");
    let kcol = SHORTCUTS
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(10)
        + 2;
    for (k, d) in SHORTCUTS {
        let pad = kcol.saturating_sub(k.chars().count());
        out.push_str(&format!("  {k}{}{d}\n", " ".repeat(pad)));
    }
    out
}

fn show_help() -> CommandResult {
    CommandResult::Message(help_text())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every advertised command (and alias) must be handled — no dead entries.
    #[test]
    fn every_registered_command_is_handled() {
        for c in COMMANDS {
            for name in std::iter::once(&c.name).chain(c.aliases.iter()) {
                if *name == "/cd" {
                    continue; // has side effects on the test process cwd
                }
                if let CommandResult::Message(m) = handle_command(name) {
                    assert!(!m.starts_with("Unknown command"), "{name} is not handled");
                }
            }
        }
    }

    #[test]
    fn help_lists_every_command() {
        let help = help_text();
        assert!(help.contains("PhazeAI CLI Commands"));
        for c in COMMANDS {
            assert!(help.contains(c.name), "{} missing from help", c.name);
        }
    }

    #[test]
    fn palette_matching() {
        let names: Vec<&str> = matching_commands("/mo").iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["/model", "/models", "/mode"]);
        // Exact match first.
        assert_eq!(matching_commands("/models")[0].name, "/models");
        // Aliases match.
        assert_eq!(matching_commands("/tok")[0].name, "/status");
        // Not a bare command word -> no palette.
        assert!(matching_commands("/model gpt").is_empty());
        assert!(matching_commands("hello").is_empty());
    }

    #[test]
    fn tab_completion() {
        assert_eq!(complete_command("/he").as_deref(), Some("/help "));
        assert_eq!(complete_command("/mod").as_deref(), Some("/mode"));
        assert_eq!(complete_command("/model").as_deref(), Some("/model "));
        assert_eq!(complete_command("/zzz"), None);
    }

    #[test]
    fn unknown_command_suggests() {
        match handle_command("/modle") {
            CommandResult::Message(m) => assert!(m.contains("Unknown command")),
            other => panic!("unexpected {other:?}"),
        }
        match handle_command("/sta") {
            CommandResult::Message(m) => assert!(m.contains("Did you mean /status?")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn copy_and_key_commands() {
        assert!(matches!(
            handle_command("/copy"),
            CommandResult::Copy { code_only: false }
        ));
        assert!(matches!(
            handle_command("/copy code"),
            CommandResult::Copy { code_only: true }
        ));
        match handle_command("/key sk-test") {
            CommandResult::SetApiKey(k) => assert_eq!(k, "sk-test"),
            other => panic!("unexpected {other:?}"),
        }
        assert!(is_secret_input("/key sk-123"));
        assert!(!is_secret_input("/keys"));
        assert!(!is_secret_input("hello /key"));
    }
}
