//! `Settings::save` and `Settings::load` on a real config directory.
//!
//! The IDE and the CLI keep their settings in `<config dir>/phazeai/config.toml`
//! and change one by loading the whole file, changing it and saving it again
//! (`save_editor_settings` in the UI's app.rs). These tests call exactly those
//! functions; only the config directory differs from a user's machine. Linux
//! takes it from `XDG_CONFIG_HOME` and macOS from `HOME`, so each test points
//! both at a fresh temp directory. The environment is process-wide, which is
//! why this is a test binary of its own and its tests take turns
//! ([`ConfigHome`]).
//!
//! Not built on Windows: there `dirs::config_dir()` asks the shell for the
//! roaming AppData folder, which no environment variable moves, so the tests
//! would overwrite the developer's real config.toml.
#![cfg(unix)]

use std::sync::{Mutex, MutexGuard};

use phazeai_core::config::{LlmProvider, ProviderEntry};
use phazeai_core::llm::{ModelRoute, TaskType};
use phazeai_core::Settings;

/// Taken by every test for its whole run: the config directory is
/// process-wide.
static CONFIG_DIR: Mutex<()> = Mutex::new(());

/// An empty home directory for the guard's lifetime, as on a new machine:
/// neither config.toml nor its `phazeai` folder exists yet.
struct ConfigHome {
    _dir: tempfile::TempDir,
    _turn: MutexGuard<'static, ()>,
}

impl ConfigHome {
    fn new() -> Self {
        let turn = CONFIG_DIR.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", dir.path());
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("xdg"));
        let path = Settings::config_path();
        // Never write to a real config.toml, whatever `dirs` does here.
        assert!(
            path.starts_with(dir.path()),
            "config_path() is {} with HOME and XDG_CONFIG_HOME in a temp dir",
            path.display()
        );
        #[cfg(target_os = "linux")]
        assert_eq!(path, dir.path().join("xdg/phazeai/config.toml"));
        assert!(!path.parent().unwrap().exists());
        Self {
            _dir: dir,
            _turn: turn,
        }
    }
}

/// Settings with every table changed from its default, so a table that is
/// not written, or not read back, shows up as a difference.
fn customised() -> Settings {
    let mut s = Settings::default();
    s.llm.provider = LlmProvider::Claude;
    s.llm.model = "claude-test-model".to_string();
    s.llm.api_key_env = "PHAZEAI_TEST_KEY".to_string();
    s.llm.base_url = Some("http://127.0.0.1:9/v1".to_string());
    s.llm.max_tokens = 1234;
    s.llm.context_tokens = Some(50_000);
    s.editor.theme = "Dracula".to_string();
    s.editor.font_size = 17.5;
    s.editor.tab_size = 2;
    s.editor.word_wrap = true;
    s.editor.ai_completions = false;
    s.updates.check_at_start = false;
    s.search.enabled = false;
    s.providers.push(ProviderEntry {
        name: "groq".to_string(),
        enabled: true,
        api_key_env: "GROQ_API_KEY".to_string(),
        base_url: "https://api.groq.com/openai/v1".to_string(),
        default_model: "llama-3.3-70b-versatile".to_string(),
    });
    s.model_routes.insert(
        TaskType::QuickAnswer,
        ModelRoute {
            provider: "groq".to_string(),
            model: "llama-3.1-8b-instant".to_string(),
        },
    );
    s.local_ai.ctx_size = Some(8192);
    s.local_ai.auto_start = false;
    s
}

/// `settings` as comparable data (`Settings` has no `PartialEq`): the
/// tables a config.toml holding it would have.
fn tables(settings: &Settings) -> toml::Table {
    toml::Table::try_from(settings).expect("settings serialise to TOML")
}

#[test]
fn first_save_creates_the_config_and_load_reads_every_change_back() {
    let _home = ConfigHome::new();
    // No config.toml yet: a new machine starts from the defaults.
    assert_eq!(tables(&Settings::load()), tables(&Settings::default()));

    let saved = customised();
    let defaults = tables(&Settings::default());
    for (table, value) in &tables(&saved) {
        assert_ne!(
            defaults.get(table),
            Some(value),
            "change [{table}] in customised() so that its persistence is tested"
        );
    }
    saved
        .save()
        .expect("save on a machine without a phazeai config folder");
    assert!(
        Settings::config_path().is_file(),
        "save() wrote no config.toml"
    );
    assert_eq!(tables(&Settings::load()), tables(&saved));
}

#[test]
fn load_change_save_keeps_every_other_setting() {
    // How the IDE changes one setting (save_editor_settings in app.rs).
    let _home = ConfigHome::new();
    customised().save().expect("first save");

    let mut settings = Settings::load();
    settings.editor.font_size = 21.0;
    settings.save().expect("save over the existing config.toml");

    let mut expected = customised();
    expected.editor.font_size = 21.0;
    assert_eq!(tables(&Settings::load()), tables(&expected));
}

#[test]
fn a_config_that_does_not_parse_loads_as_the_defaults() {
    let _home = ConfigHome::new();
    let path = Settings::config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // Broken TOML, TOML without the required [llm] table, and bytes that are
    // not UTF-8: none of them may stop the IDE or the CLI from starting.
    let broken: [&[u8]; 3] = [
        b"[llm\nmodel = ",
        b"[editor]\nfont_size = 30.0\n",
        b"\xff\xfe not UTF-8",
    ];
    for content in broken {
        std::fs::write(&path, content).unwrap();
        assert_eq!(
            tables(&Settings::load()),
            tables(&Settings::default()),
            "{}",
            String::from_utf8_lossy(content)
        );
    }
}
