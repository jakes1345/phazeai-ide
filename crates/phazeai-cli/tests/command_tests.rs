use phazeai_cli::commands::{handle_command, CommandResult};

// ========================================================================
// Command Parsing Tests (commands.rs)
// ========================================================================

// NOTE: The handle_command function is public and can be tested.
// The function returns CommandResult enum variants based on slash command input.

// --- BASIC SLASH COMMANDS ---

#[test]
fn test_help_command() {
    let result = handle_command("/help");
    matches!(result, CommandResult::Message(_));

    if let CommandResult::Message(msg) = result {
        assert!(msg.contains("PhazeAI CLI Commands"));
        assert!(msg.contains("/help"));
    }
}

#[test]
fn test_help_command_short_alias() {
    let result = handle_command("/h");
    matches!(result, CommandResult::Message(_));
}

#[test]
fn test_exit_command() {
    let result = handle_command("/exit");
    assert!(matches!(result, CommandResult::Quit));
}

#[test]
fn test_quit_command() {
    let result = handle_command("/quit");
    assert!(matches!(result, CommandResult::Quit));
}

#[test]
fn test_quit_short_alias() {
    let result = handle_command("/q");
    assert!(matches!(result, CommandResult::Quit));
}

#[test]
fn test_clear_command() {
    let result = handle_command("/clear");
    assert!(matches!(result, CommandResult::Clear));
}

#[test]
fn test_new_conversation_command() {
    let result = handle_command("/new");
    assert!(matches!(result, CommandResult::NewConversation));
}

#[test]
fn test_compact_command() {
    let result = handle_command("/compact");
    assert!(matches!(result, CommandResult::Compact));
}

#[test]
fn test_save_conversation_command() {
    let result = handle_command("/save");
    assert!(matches!(result, CommandResult::SaveConversation));
}

#[test]
fn test_version_command() {
    let result = handle_command("/version");
    matches!(result, CommandResult::Message(_));

    if let CommandResult::Message(msg) = result {
        assert!(msg.contains("PhazeAI CLI"));
        assert!(msg.contains("v"));
    }
}

// --- COMMANDS WITH ARGUMENTS ---

#[test]
fn test_load_command_with_id() {
    let result = handle_command("/load abc-123-xyz");

    match result {
        CommandResult::LoadConversation(id) => {
            assert_eq!(id, "abc-123-xyz");
        }
        _ => panic!("Expected LoadConversation, got {:?}", result),
    }
}

#[test]
fn test_load_command_without_id() {
    let result = handle_command("/load");

    match result {
        CommandResult::Message(msg) => {
            assert!(msg.contains("Usage: /load <conversation-id>"));
        }
        _ => panic!("Expected Message (error), got {:?}", result),
    }
}

#[test]
fn test_load_command_with_whitespace() {
    let result = handle_command("/load   spaced-id-123  ");

    match result {
        CommandResult::LoadConversation(id) => {
            assert_eq!(id, "spaced-id-123");
        }
        _ => panic!("Expected LoadConversation, got {:?}", result),
    }
}

#[test]
fn test_conversations_command() {
    let result = handle_command("/conversations");
    assert!(matches!(result, CommandResult::ListConversations));
}

#[test]
fn test_history_command_alias() {
    let result = handle_command("/history");
    assert!(matches!(result, CommandResult::ListConversations));
}

#[test]
fn test_model_command_with_name() {
    let result = handle_command("/model gpt-4o");

    match result {
        CommandResult::ModelChanged(name) => {
            assert_eq!(name, "gpt-4o");
        }
        _ => panic!("Expected ModelChanged, got {:?}", result),
    }
}

#[test]
fn test_model_command_without_name() {
    let result = handle_command("/model");

    match result {
        CommandResult::Message(msg) => {
            assert!(msg.contains("/model <model-name>"));
        }
        _ => panic!("Expected Message (usage hint), got {:?}", result),
    }
}

#[test]
fn test_models_command() {
    let result = handle_command("/models");
    assert!(matches!(result, CommandResult::ListModels));
}

#[test]
fn test_provider_command_with_name() {
    let result = handle_command("/provider openai");

    match result {
        CommandResult::ProviderChanged(name) => {
            assert_eq!(name, "openai");
        }
        _ => panic!("Expected ProviderChanged, got {:?}", result),
    }
}

#[test]
fn test_provider_command_without_name() {
    let result = handle_command("/provider");

    match result {
        CommandResult::Message(msg) => {
            assert!(msg.contains("anthropic"));
            assert!(msg.contains("openai"));
            assert!(msg.contains("ollama"));
        }
        _ => panic!("Expected Message (provider list), got {:?}", result),
    }
}

#[test]
fn test_discover_models_command() {
    let result = handle_command("/discover");
    assert!(matches!(result, CommandResult::DiscoverModels));
}

#[test]
fn test_approve_command_with_auto() {
    let result = handle_command("/approve auto");

    match result {
        CommandResult::SetApprovalMode(mode) => {
            assert_eq!(mode, "auto");
        }
        _ => panic!("Expected SetApprovalMode, got {:?}", result),
    }
}

#[test]
fn test_approve_command_with_ask() {
    let result = handle_command("/approve ask");

    match result {
        CommandResult::SetApprovalMode(mode) => {
            assert_eq!(mode, "ask");
        }
        _ => panic!("Expected SetApprovalMode, got {:?}", result),
    }
}

#[test]
fn test_approve_command_with_ask_once() {
    let result = handle_command("/approve ask-once");

    match result {
        CommandResult::SetApprovalMode(mode) => {
            assert_eq!(mode, "ask-once");
        }
        _ => panic!("Expected SetApprovalMode, got {:?}", result),
    }
}

#[test]
fn test_approve_command_with_invalid_mode() {
    let result = handle_command("/approve invalid");

    match result {
        CommandResult::Message(msg) => {
            assert!(msg.contains("Invalid approval mode"));
            assert!(msg.contains("auto"));
            assert!(msg.contains("ask"));
            assert!(msg.contains("ask-once"));
        }
        _ => panic!("Expected Message (error), got {:?}", result),
    }
}

#[test]
fn test_approve_command_without_mode() {
    let result = handle_command("/approve");

    match result {
        CommandResult::Message(msg) => {
            assert!(msg.contains("Tool approval modes"));
            assert!(msg.contains("auto"));
            assert!(msg.contains("ask"));
            assert!(msg.contains("ask-once"));
        }
        _ => panic!("Expected Message (help), got {:?}", result),
    }
}

#[test]
fn test_cost_command() {
    let result = handle_command("/cost");
    assert!(matches!(result, CommandResult::ShowStatus));
}

#[test]
fn test_status_command() {
    let result = handle_command("/status");
    assert!(matches!(result, CommandResult::ShowStatus));
}

#[test]
fn test_theme_command_with_name() {
    let result = handle_command("/theme tokyo-night");

    match result {
        CommandResult::ThemeChanged(name) => {
            assert_eq!(name, "tokyo-night");
        }
        _ => panic!("Expected ThemeChanged, got {:?}", result),
    }
}

#[test]
fn test_theme_command_without_name() {
    let result = handle_command("/theme");

    match result {
        CommandResult::Message(msg) => {
            assert!(msg.contains("Available themes"));
            assert!(msg.contains("dark"));
            assert!(msg.contains("tokyo-night"));
            assert!(msg.contains("dracula"));
        }
        _ => panic!("Expected Message (theme list), got {:?}", result),
    }
}

#[test]
fn test_files_command() {
    let result = handle_command("/files");
    assert!(matches!(result, CommandResult::ToggleFiles));
}

#[test]
fn test_tree_command_alias() {
    let result = handle_command("/tree");
    assert!(matches!(result, CommandResult::ToggleFiles));
}

#[test]
fn test_diff_command() {
    let result = handle_command("/diff");
    assert!(matches!(result, CommandResult::ShowDiff));
}

#[test]
fn test_git_status_command() {
    let result = handle_command("/git");
    assert!(matches!(result, CommandResult::ShowGitStatus));
}

#[test]
fn test_git_log_command() {
    let result = handle_command("/log");
    assert!(matches!(result, CommandResult::ShowLog));
}

#[test]
fn test_search_command_with_query() {
    let result = handle_command("/search **/*.rs");

    match result {
        CommandResult::SearchFiles(query) => {
            assert_eq!(query, "**/*.rs");
        }
        _ => panic!("Expected SearchFiles, got {:?}", result),
    }
}

#[test]
fn test_search_command_without_query() {
    let result = handle_command("/search");

    match result {
        CommandResult::Message(msg) => {
            assert!(msg.contains("Usage: /search <glob-pattern>"));
            assert!(msg.contains("Example"));
        }
        _ => panic!("Expected Message (usage hint), got {:?}", result),
    }
}

#[test]
fn test_pwd_command() {
    let result = handle_command("/pwd");

    match result {
        CommandResult::Message(msg) => {
            assert!(msg.contains("Current directory"));
        }
        _ => panic!("Expected Message (current directory), got {:?}", result),
    }
}

#[test]
fn test_cd_command_with_directory() {
    // Note: This test may change the actual current directory,
    // so we use a path that likely exists on the system (temp dir)
    let result = handle_command("/cd /tmp");

    match result {
        CommandResult::Message(msg) => {
            // Should either succeed or show error
            assert!(msg.contains("directory") || msg.contains("Error"));
        }
        _ => panic!("Expected Message, got {:?}", result),
    }
}

#[test]
fn test_cd_command_without_directory() {
    let result = handle_command("/cd");

    match result {
        CommandResult::Message(msg) => {
            assert!(msg.contains("Usage: /cd <directory>"));
        }
        _ => panic!("Expected Message (usage hint), got {:?}", result),
    }
}

#[test]
fn test_context_command() {
    let result = handle_command("/context");
    assert!(matches!(result, CommandResult::ShowContext));
}

// --- EDGE CASES ---

#[test]
fn test_empty_input_is_not_a_command() {
    let result = handle_command("");
    assert!(matches!(result, CommandResult::NotACommand));
}

#[test]
fn test_whitespace_only_is_not_a_command() {
    let result = handle_command("   ");
    assert!(matches!(result, CommandResult::NotACommand));
}

#[test]
fn test_regular_text_is_not_a_command() {
    let result = handle_command("Hello, how are you?");
    assert!(matches!(result, CommandResult::NotACommand));
}

#[test]
fn test_regular_text_starting_with_non_slash_is_not_a_command() {
    let result = handle_command("help me with this");
    assert!(matches!(result, CommandResult::NotACommand));
}

#[test]
fn test_unknown_slash_command_shows_error() {
    let result = handle_command("/foobar");

    match result {
        CommandResult::Message(msg) => {
            assert!(msg.contains("Unknown command"));
            assert!(msg.contains("/foobar"));
            assert!(msg.contains("/help"));
        }
        _ => panic!("Expected Message (unknown command error), got {:?}", result),
    }
}

#[test]
fn test_unknown_slash_command_with_args() {
    let result = handle_command("/invalid-cmd with args");

    match result {
        CommandResult::Message(msg) => {
            assert!(msg.contains("Unknown command"));
            assert!(msg.contains("/invalid-cmd"));
        }
        _ => panic!("Expected Message (unknown command error), got {:?}", result),
    }
}

#[test]
fn test_command_with_extra_leading_whitespace() {
    // Note: Leading whitespace before slash makes it not a command
    let result = handle_command("  /help");
    assert!(matches!(result, CommandResult::NotACommand));
}

#[test]
fn test_command_with_trailing_whitespace() {
    let result = handle_command("/help   ");
    matches!(result, CommandResult::Message(_));
}

#[test]
fn test_command_with_multiple_spaces_in_args() {
    let result = handle_command("/search    **/*.rs   **/*.toml   ");

    match result {
        CommandResult::SearchFiles(query) => {
            // Should trim and preserve the argument
            assert_eq!(query, "**/*.rs   **/*.toml");
        }
        _ => panic!("Expected SearchFiles, got {:?}", result),
    }
}

#[test]
fn test_model_command_with_multiword_model_name() {
    let result = handle_command("/model claude-sonnet-4-5-20250929");

    match result {
        CommandResult::ModelChanged(name) => {
            assert_eq!(name, "claude-sonnet-4-5-20250929");
        }
        _ => panic!("Expected ModelChanged, got {:?}", result),
    }
}

#[test]
fn test_load_command_with_uuid_format() {
    let result = handle_command("/load 550e8400-e29b-41d4-a716-446655440000");

    match result {
        CommandResult::LoadConversation(id) => {
            assert_eq!(id, "550e8400-e29b-41d4-a716-446655440000");
        }
        _ => panic!("Expected LoadConversation, got {:?}", result),
    }
}

#[test]
fn test_search_with_complex_glob_pattern() {
    let result = handle_command("/search src/**/*.{rs,toml,md}");

    match result {
        CommandResult::SearchFiles(query) => {
            assert_eq!(query, "src/**/*.{rs,toml,md}");
        }
        _ => panic!("Expected SearchFiles, got {:?}", result),
    }
}

#[test]
fn test_theme_command_case_sensitive() {
    // Theme names should be passed as-is
    let result = handle_command("/theme Tokyo-Night");

    match result {
        CommandResult::ThemeChanged(name) => {
            assert_eq!(name, "Tokyo-Night");
        }
        _ => panic!("Expected ThemeChanged, got {:?}", result),
    }
}

#[test]
fn test_slash_only_is_unknown_command() {
    let result = handle_command("/");

    match result {
        CommandResult::Message(msg) => {
            assert!(msg.contains("Unknown command"));
        }
        _ => panic!("Expected Message (unknown command), got {:?}", result),
    }
}

// --- COMPREHENSIVE ARGUMENT PARSING ---

#[test]
fn test_cd_command_with_path_containing_spaces() {
    let result = handle_command("/cd /home/user/My Documents");

    match result {
        CommandResult::Message(_) => {
            // This should attempt to change directory
            // The exact message depends on whether the path exists
        }
        _ => panic!("Expected Message, got {:?}", result),
    }
}

#[test]
fn test_provider_command_case_preservation() {
    let result = handle_command("/provider OpenAI");

    match result {
        CommandResult::ProviderChanged(name) => {
            assert_eq!(name, "OpenAI");
        }
        _ => panic!("Expected ProviderChanged, got {:?}", result),
    }
}
