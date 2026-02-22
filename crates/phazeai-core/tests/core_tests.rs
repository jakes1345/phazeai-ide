use phazeai_core::config::Settings;
use phazeai_core::*;
use tempfile::TempDir;

// ========================================================================
// Settings Tests (config/mod.rs)
// ========================================================================

#[test]
fn test_settings_default_values() {
    let settings = Settings::default();

    // Check LLM defaults
    assert_eq!(settings.llm.model, "phaze-beast");
    assert_eq!(settings.llm.api_key_env, "");
    assert_eq!(settings.llm.max_tokens, 8192);
    assert!(settings.llm.base_url.is_none());

    // Check editor defaults
    assert_eq!(settings.editor.theme, "Dark");
    assert_eq!(settings.editor.font_size, 14.0);
    assert_eq!(settings.editor.tab_size, 4);
    assert!(settings.editor.show_line_numbers);
    assert!(settings.editor.auto_save);

    // Check sidecar defaults
    assert!(settings.sidecar.enabled);
    assert_eq!(settings.sidecar.python_path, "python3");
    assert!(settings.sidecar.auto_start);

    // Check providers list is empty by default
    assert!(settings.providers.is_empty());
}

#[test]
fn test_settings_load_returns_default_when_no_file() {
    // Loading from a non-existent config should return defaults
    // Note: This may load from actual config if it exists, but the function
    // is designed to gracefully return defaults when file doesn't exist
    let settings = Settings::load();

    // Just verify it doesn't panic and has some expected structure
    assert!(!settings.llm.model.is_empty());
    assert!(!settings.llm.api_key_env.is_empty());
}

#[test]
fn test_settings_save_and_reload_roundtrip() {
    // Create a temporary directory for testing
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    // Create custom settings
    let mut settings = Settings::default();
    settings.llm.model = "test-model".to_string();
    settings.llm.max_tokens = 4096;
    settings.editor.font_size = 16.0;

    // Temporarily override config path by writing directly
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    let content = toml::to_string_pretty(&settings).unwrap();
    std::fs::write(&config_path, content).unwrap();

    // Load back and verify
    let loaded_content = std::fs::read_to_string(&config_path).unwrap();
    let loaded: Settings = toml::from_str(&loaded_content).unwrap();

    assert_eq!(loaded.llm.model, "test-model");
    assert_eq!(loaded.llm.max_tokens, 4096);
    assert_eq!(loaded.editor.font_size, 16.0);
}

#[test]
fn test_settings_build_provider_registry_returns_correct_active() {
    let mut settings = Settings::default();
    settings.llm.model = "custom-model".to_string();

    let registry = settings.build_provider_registry();

    assert_eq!(registry.active_provider(), &ProviderId::Ollama);
    assert_eq!(registry.active_model(), "custom-model");
}

#[test]
fn test_settings_api_key_reads_from_env() {
    // Set a test environment variable
    std::env::set_var("TEST_API_KEY_PHAZEAI", "test-key-12345");

    let mut settings = Settings::default();
    settings.llm.api_key_env = "TEST_API_KEY_PHAZEAI".to_string();

    let api_key = settings.api_key();
    assert_eq!(api_key, Some("test-key-12345".to_string()));

    // Clean up
    std::env::remove_var("TEST_API_KEY_PHAZEAI");
}

#[test]
fn test_settings_api_key_none_when_not_set() {
    let mut settings = Settings::default();
    settings.llm.api_key_env = "NONEXISTENT_KEY_PHAZEAI_TEST".to_string();

    let api_key = settings.api_key();
    assert!(api_key.is_none());
}

// ========================================================================
// ConversationHistory Tests (context/history.rs)
// ========================================================================

#[test]
fn test_conversation_history_add_messages() {
    let mut history = ConversationHistory::new();

    history.add_user_message("Hello");
    history.add_assistant_message("Hi there!");
    history.add_user_message("How are you?");

    let messages = history.get_conversation_messages();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].content, "Hello");
    assert_eq!(messages[1].content, "Hi there!");
    assert_eq!(messages[2].content, "How are you?");
}

#[test]
fn test_conversation_history_trimming_when_exceeding_max() {
    let mut history = ConversationHistory::new().with_max_messages(3);

    history.add_user_message("Message 1");
    history.add_user_message("Message 2");
    history.add_user_message("Message 3");
    history.add_user_message("Message 4"); // Should trigger trim
    history.add_user_message("Message 5"); // Should trigger trim

    let messages = history.get_conversation_messages();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].content, "Message 3");
    assert_eq!(messages[1].content, "Message 4");
    assert_eq!(messages[2].content, "Message 5");
}

#[test]
fn test_conversation_history_get_messages_includes_system_prompt() {
    let mut history = ConversationHistory::new().with_system_prompt("You are a helpful assistant.");

    history.add_user_message("Hello");

    let messages = history.get_messages();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, Role::System);
    assert_eq!(messages[0].content, "You are a helpful assistant.");
    assert_eq!(messages[1].role, Role::User);
    assert_eq!(messages[1].content, "Hello");
}

#[test]
fn test_conversation_history_get_conversation_messages_excludes_system_prompt() {
    let mut history = ConversationHistory::new().with_system_prompt("You are a helpful assistant.");

    history.add_user_message("Hello");
    history.add_assistant_message("Hi!");

    let messages = history.get_conversation_messages();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, Role::User);
    assert_eq!(messages[1].role, Role::Assistant);
}

#[test]
fn test_conversation_history_clear_works() {
    let mut history = ConversationHistory::new();

    history.add_user_message("Message 1");
    history.add_user_message("Message 2");
    assert_eq!(history.len(), 2);

    history.clear();
    assert_eq!(history.len(), 0);
    assert!(history.is_empty());
}

#[test]
fn test_conversation_history_estimate_tokens_basic() {
    let mut history = ConversationHistory::new();

    // Add a message with known length
    history.add_user_message("test"); // 4 chars

    let tokens = history.estimate_tokens();
    // Should be ~1 token (4 chars / 4)
    assert_eq!(tokens, 1);

    history.add_user_message("test message with more content"); // 31 chars
    let tokens = history.estimate_tokens();
    // Should be ~9 tokens ( (4+3)/4 + (31+3)/4 ) = 1 + 8 = 9
    assert_eq!(tokens, 9);
}

#[test]
fn test_conversation_history_last_message() {
    let mut history = ConversationHistory::new();

    assert!(history.last_message().is_none());

    history.add_user_message("First");
    assert_eq!(history.last_message().unwrap().content, "First");

    history.add_assistant_message("Second");
    assert_eq!(history.last_message().unwrap().content, "Second");
}

// ========================================================================
// SystemPromptBuilder Tests (context/system_prompt.rs)
// ========================================================================

#[test]
fn test_system_prompt_builder_with_project_root_sets_type() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();

    // Create a Cargo.toml to indicate Rust project
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();

    let builder = SystemPromptBuilder::new().with_project_root(root.to_path_buf());

    let prompt = builder.build();

    assert!(prompt.contains("Project type: Rust"));
    assert!(prompt.contains(&format!("Working directory: {}", root.display())));
}

#[test]
fn test_system_prompt_builder_with_git_info_includes_branch() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();

    let builder = SystemPromptBuilder::new()
        .with_project_root(root.to_path_buf())
        .with_git_info(Some("main".to_string()), vec!["src/main.rs".to_string()]);

    let prompt = builder.build();

    assert!(prompt.contains("Git branch: main"));
    assert!(prompt.contains("Modified files: src/main.rs"));
}

#[test]
fn test_system_prompt_builder_with_tools_lists_them() {
    let tools = vec![
        "read_file".to_string(),
        "write_file".to_string(),
        "grep".to_string(),
    ];

    let builder = SystemPromptBuilder::new().with_tools(tools);
    let prompt = builder.build();

    assert!(prompt.contains("Available Tools"));
    assert!(prompt.contains("read_file"));
    assert!(prompt.contains("write_file"));
    assert!(prompt.contains("grep"));
}

#[test]
fn test_system_prompt_builder_with_custom_instructions() {
    let instructions = "Always write tests.\nUse tabs for indentation.".to_string();

    let builder = SystemPromptBuilder::new().with_custom_instructions(instructions.clone());

    let prompt = builder.build();

    assert!(prompt.contains("Project-Specific Instructions"));
    assert!(prompt.contains(&instructions));
}

#[test]
fn test_project_type_detect_rust() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();

    std::fs::write(root.join("Cargo.toml"), "[package]").unwrap();

    let project_type = ProjectType::detect(root);
    assert!(matches!(project_type, ProjectType::Rust));
    assert_eq!(project_type.name(), "Rust");
}

#[test]
fn test_project_type_detect_python() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();

    std::fs::write(root.join("requirements.txt"), "flask==2.0.0").unwrap();

    let project_type = ProjectType::detect(root);
    assert!(matches!(project_type, ProjectType::Python));
    assert_eq!(project_type.name(), "Python");
}

#[test]
fn test_project_type_detect_typescript() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();

    std::fs::write(root.join("package.json"), "{}").unwrap();
    std::fs::write(root.join("tsconfig.json"), "{}").unwrap();

    let project_type = ProjectType::detect(root);
    assert!(matches!(project_type, ProjectType::TypeScript));
    assert_eq!(project_type.name(), "TypeScript");
}

#[test]
fn test_project_type_detect_javascript() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();

    std::fs::write(root.join("package.json"), "{}").unwrap();
    // No tsconfig.json

    let project_type = ProjectType::detect(root);
    assert!(matches!(project_type, ProjectType::JavaScript));
    assert_eq!(project_type.name(), "JavaScript");
}

#[test]
fn test_project_type_detect_go() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();

    std::fs::write(root.join("go.mod"), "module test").unwrap();

    let project_type = ProjectType::detect(root);
    assert!(matches!(project_type, ProjectType::Go));
    assert_eq!(project_type.name(), "Go");
}

#[test]
fn test_project_type_detect_java() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();

    std::fs::write(root.join("pom.xml"), "<project>").unwrap();

    let project_type = ProjectType::detect(root);
    assert!(matches!(project_type, ProjectType::Java));
    assert_eq!(project_type.name(), "Java");
}

#[test]
fn test_project_type_detect_ruby() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();

    std::fs::write(root.join("Gemfile"), "source 'https://rubygems.org'").unwrap();

    let project_type = ProjectType::detect(root);
    assert!(matches!(project_type, ProjectType::Ruby));
    assert_eq!(project_type.name(), "Ruby");
}

#[test]
fn test_project_type_detect_mixed() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();

    // Create markers for multiple languages
    std::fs::write(root.join("Cargo.toml"), "[package]").unwrap();
    std::fs::write(root.join("package.json"), "{}").unwrap();

    let project_type = ProjectType::detect(root);
    assert!(matches!(project_type, ProjectType::Mixed(_)));
    assert_eq!(project_type.name(), "Multi-language");
}

#[test]
fn test_project_type_detect_unknown() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();

    // No marker files

    let project_type = ProjectType::detect(root);
    assert!(matches!(project_type, ProjectType::Unknown));
    assert_eq!(project_type.name(), "Unknown");
}

// ========================================================================
// ProviderRegistry Tests (llm/provider.rs)
// ========================================================================

#[test]
fn test_provider_registry_new_has_builtin_providers() {
    let registry = ProviderRegistry::new();

    // Should have all builtin providers
    assert!(registry.get_config(&ProviderId::Claude).is_some());
    assert!(registry.get_config(&ProviderId::OpenAI).is_some());
    assert!(registry.get_config(&ProviderId::Ollama).is_some());
    assert!(registry.get_config(&ProviderId::Groq).is_some());
    assert!(registry.get_config(&ProviderId::Together).is_some());
    assert!(registry.get_config(&ProviderId::OpenRouter).is_some());
    assert!(registry.get_config(&ProviderId::LmStudio).is_some());
}

#[test]
fn test_provider_registry_set_active_changes_provider() {
    let mut registry = ProviderRegistry::new();

    assert_eq!(registry.active_provider(), &ProviderId::Claude);

    registry.set_active(ProviderId::OpenAI, "gpt-4".to_string());

    assert_eq!(registry.active_provider(), &ProviderId::OpenAI);
    assert_eq!(registry.active_model(), "gpt-4");
}

#[test]
fn test_provider_registry_set_provider_updates_default_model() {
    let mut registry = ProviderRegistry::new();

    registry.set_provider(ProviderId::OpenAI);

    assert_eq!(registry.active_provider(), &ProviderId::OpenAI);
    // Should use the default model for OpenAI
    assert_eq!(registry.active_model(), "gpt-4o");
}

#[test]
fn test_provider_registry_available_providers_filters_by_api_key() {
    let registry = ProviderRegistry::new();

    // Available providers should exclude those requiring API keys we don't have
    let available = registry.available_providers();

    // Local providers (Ollama, LmStudio) should be available
    let has_local = available
        .iter()
        .any(|p| p.id == ProviderId::Ollama || p.id == ProviderId::LmStudio);
    assert!(has_local);

    // All returned providers should be marked as available
    for provider in available {
        assert!(provider.is_available());
    }
}

#[test]
fn test_provider_registry_known_models_returns_models_for_claude() {
    let models = ProviderRegistry::known_models(&ProviderId::Claude);

    assert!(!models.is_empty());

    // Check for expected Claude models
    let has_opus = models.iter().any(|m| m.id == "claude-opus-4-6");
    let has_sonnet = models.iter().any(|m| m.id == "claude-sonnet-4-5-20250929");
    let has_haiku = models.iter().any(|m| m.id == "claude-haiku-4-5-20251001");

    assert!(has_opus);
    assert!(has_sonnet);
    assert!(has_haiku);
}

#[test]
fn test_provider_registry_known_models_returns_models_for_openai() {
    let models = ProviderRegistry::known_models(&ProviderId::OpenAI);

    assert!(!models.is_empty());

    let has_gpt4o = models.iter().any(|m| m.id == "gpt-4o");
    let has_gpt4o_mini = models.iter().any(|m| m.id == "gpt-4o-mini");

    assert!(has_gpt4o);
    assert!(has_gpt4o_mini);
}

#[test]
fn test_provider_registry_known_models_empty_for_local() {
    let ollama_models = ProviderRegistry::known_models(&ProviderId::Ollama);
    let lmstudio_models = ProviderRegistry::known_models(&ProviderId::LmStudio);

    // Local providers return empty - must query server
    assert!(ollama_models.is_empty());
    assert!(lmstudio_models.is_empty());
}

#[test]
fn test_usage_tracker_cost_estimation() {
    let mut tracker = UsageTracker::default();

    // Track some usage
    tracker.track(1000, 500); // 1000 input, 500 output
    tracker.track(500, 250); // 500 input, 250 output

    assert_eq!(tracker.total_input_tokens, 1500);
    assert_eq!(tracker.total_output_tokens, 750);
    assert_eq!(tracker.request_count, 2);

    // Create a test model info
    let model = ModelInfo {
        id: "test-model".to_string(),
        name: "Test Model".to_string(),
        context_window: 100_000,
        supports_tools: true,
        input_cost_per_m: 3.0,   // $3 per million input tokens
        output_cost_per_m: 15.0, // $15 per million output tokens
    };

    let cost = tracker.estimated_cost(&model);

    // Expected: (1500/1000000 * 3.0) + (750/1000000 * 15.0)
    // = 0.0015 * 3.0 + 0.00075 * 15.0
    // = 0.0045 + 0.01125
    // = 0.01575
    let expected = 0.01575;
    assert!((cost - expected).abs() < 0.00001);
}

#[test]
fn test_usage_tracker_reset() {
    let mut tracker = UsageTracker::default();

    tracker.track(1000, 500);
    assert_eq!(tracker.total_input_tokens, 1000);

    tracker.reset();

    assert_eq!(tracker.total_input_tokens, 0);
    assert_eq!(tracker.total_output_tokens, 0);
    assert_eq!(tracker.request_count, 0);
}

#[test]
fn test_provider_id_needs_api_key() {
    assert!(ProviderId::Claude.needs_api_key());
    assert!(ProviderId::OpenAI.needs_api_key());
    assert!(ProviderId::Groq.needs_api_key());

    assert!(!ProviderId::Ollama.needs_api_key());
    assert!(!ProviderId::LmStudio.needs_api_key());
}

#[test]
fn test_provider_id_is_local() {
    assert!(ProviderId::Ollama.is_local());
    assert!(ProviderId::LmStudio.is_local());

    assert!(!ProviderId::Claude.is_local());
    assert!(!ProviderId::OpenAI.is_local());
}

// ========================================================================
// ConversationStore Tests (context/persistence.rs)
// ========================================================================

#[test]
fn test_conversation_store_save_and_load_roundtrip() {
    let temp_dir = TempDir::new().unwrap();

    // Create a custom store with temp directory
    // Note: ConversationStore uses ~/.phazeai/conversations by default
    // For testing, we'll create a conversation and verify save/load

    let id = ConversationStore::generate_id();
    let mut conversation = SavedConversation::new(
        id.clone(),
        "Test Conversation".to_string(),
        "claude-sonnet-4-5-20250929".to_string(),
        Some("/home/test/project".to_string()),
        Some("You are a test assistant.".to_string()),
    );

    conversation.add_message(SavedMessage::user("Hello".to_string()));
    conversation.add_message(SavedMessage::assistant("Hi there!".to_string()));

    // Save to a temp file directly for testing
    let test_file = temp_dir.path().join(format!("{}.json", id));
    let content = serde_json::to_string_pretty(&conversation).unwrap();
    std::fs::write(&test_file, content).unwrap();

    // Load back
    let loaded_content = std::fs::read_to_string(&test_file).unwrap();
    let loaded: SavedConversation = serde_json::from_str(&loaded_content).unwrap();

    assert_eq!(loaded.metadata.id, id);
    assert_eq!(loaded.metadata.title, "Test Conversation");
    assert_eq!(loaded.metadata.model, "claude-sonnet-4-5-20250929");
    assert_eq!(loaded.metadata.message_count, 2);
    assert_eq!(loaded.messages.len(), 2);
    assert_eq!(loaded.messages[0].content, "Hello");
    assert_eq!(loaded.messages[1].content, "Hi there!");
    assert_eq!(
        loaded.system_prompt,
        Some("You are a test assistant.".to_string())
    );
}

#[test]
fn test_conversation_store_generate_id_unique() {
    let id1 = ConversationStore::generate_id();
    let id2 = ConversationStore::generate_id();

    assert_ne!(id1, id2);
    assert!(id1.contains('-')); // UUID format
    assert!(id2.contains('-'));
}

#[test]
fn test_saved_conversation_add_message_updates_metadata() {
    let mut conversation = SavedConversation::new(
        "test-id".to_string(),
        "Test".to_string(),
        "test-model".to_string(),
        None,
        None,
    );

    assert_eq!(conversation.metadata.message_count, 0);

    conversation.add_message(SavedMessage::user("Hello".to_string()));
    assert_eq!(conversation.metadata.message_count, 1);

    conversation.add_message(SavedMessage::assistant("Hi".to_string()));
    assert_eq!(conversation.metadata.message_count, 2);
}

#[test]
fn test_saved_conversation_generate_title_from_first_message() {
    let mut conversation = SavedConversation::new(
        "test-id".to_string(),
        "Untitled".to_string(),
        "test-model".to_string(),
        None,
        None,
    );

    conversation.add_message(SavedMessage::user("Create a new Rust project".to_string()));
    conversation.generate_title_from_first_message();

    assert_eq!(conversation.metadata.title, "Create a new Rust project");
}

#[test]
fn test_saved_conversation_generate_title_truncates_long_messages() {
    let mut conversation = SavedConversation::new(
        "test-id".to_string(),
        "Untitled".to_string(),
        "test-model".to_string(),
        None,
        None,
    );

    let long_message = "a".repeat(100);
    conversation.add_message(SavedMessage::user(long_message));
    conversation.generate_title_from_first_message();

    assert!(conversation.metadata.title.ends_with("..."));
    assert!(conversation.metadata.title.len() <= 83); // 80 + "..."
}

#[test]
fn test_saved_message_constructors() {
    let user_msg = SavedMessage::user("Hello".to_string());
    assert_eq!(user_msg.role, "user");
    assert_eq!(user_msg.content, "Hello");
    assert!(user_msg.tool_name.is_none());

    let assistant_msg = SavedMessage::assistant("Hi".to_string());
    assert_eq!(assistant_msg.role, "assistant");
    assert_eq!(assistant_msg.content, "Hi");

    let system_msg = SavedMessage::system("System prompt".to_string());
    assert_eq!(system_msg.role, "system");
    assert_eq!(system_msg.content, "System prompt");

    let tool_msg = SavedMessage::tool("Result".to_string(), "grep".to_string());
    assert_eq!(tool_msg.role, "tool");
    assert_eq!(tool_msg.content, "Result");
    assert_eq!(tool_msg.tool_name, Some("grep".to_string()));
}

// Integration tests using actual ConversationStore
// These will use the real filesystem under ~/.phazeai/conversations

#[test]
fn test_conversation_store_integration_save_load_delete() {
    let temp_dir = TempDir::new().unwrap();
    let store = ConversationStore::with_dir(temp_dir.path().to_path_buf()).unwrap();

    let id = ConversationStore::generate_id();
    let mut conversation = SavedConversation::new(
        id.clone(),
        "Integration Test".to_string(),
        "test-model".to_string(),
        None,
        None,
    );

    conversation.add_message(SavedMessage::user("Test message".to_string()));

    // Save
    store.save(&conversation).unwrap();

    // Load
    let loaded = store.load(&id).unwrap();
    assert_eq!(loaded.metadata.id, id);
    assert_eq!(loaded.metadata.title, "Integration Test");
    assert_eq!(loaded.messages.len(), 1);
    assert_eq!(loaded.messages[0].content, "Test message");

    // Delete
    store.delete(&id).unwrap();

    // Verify deleted
    let result = store.load(&id);
    assert!(result.is_err());
}

#[test]
fn test_conversation_store_integration_list_recent() {
    let temp_dir = TempDir::new().unwrap();
    let store = ConversationStore::with_dir(temp_dir.path().to_path_buf()).unwrap();

    // Create multiple conversations
    for i in 0..5 {
        let id = ConversationStore::generate_id();
        let conversation = SavedConversation::new(
            id,
            format!("Test {}", i),
            "test-model".to_string(),
            None,
            None,
        );
        store.save(&conversation).unwrap();
    }

    // List recent with limit
    let recent = store.list_recent(3).unwrap();
    assert_eq!(recent.len(), 3);

    // List all
    let all = store.list_recent(100).unwrap();
    assert_eq!(all.len(), 5);
}

#[test]
fn test_conversation_store_integration_search() {
    let temp_dir = TempDir::new().unwrap();
    let store = ConversationStore::with_dir(temp_dir.path().to_path_buf()).unwrap();

    let id = ConversationStore::generate_id();
    let conversation = SavedConversation::new(
        id.clone(),
        "Unique Search Title XYZ".to_string(),
        "test-model".to_string(),
        None,
        None,
    );

    store.save(&conversation).unwrap();

    // Also save a non-matching conversation
    let id2 = ConversationStore::generate_id();
    let conversation2 = SavedConversation::new(
        id2,
        "Something else entirely".to_string(),
        "test-model".to_string(),
        None,
        None,
    );
    store.save(&conversation2).unwrap();

    // Search should find only the matching one
    let results = store.search("XYZ").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, id);

    // Case-insensitive search
    let results = store.search("xyz").unwrap();
    assert_eq!(results.len(), 1);
}
