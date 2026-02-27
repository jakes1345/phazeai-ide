/// PhazeAI — centralized constants.
/// All magic numbers, strings, and limits live here.
/// Never hardcode these values elsewhere.

// ─── Models ───────────────────────────────────────────────────────────────────

pub mod models {
    /// PhazeAI custom Ollama models
    pub const PHAZE_BEAST: &str = "phaze-beast";
    pub const PHAZE_CODER: &str = "phaze-coder";
    pub const PHAZE_PLANNER: &str = "phaze-planner";
    pub const PHAZE_REVIEWER: &str = "phaze-reviewer";

    /// All custom phaze models (for auto-pull / discovery)
    pub const PHAZE_MODELS: &[&str] = &[PHAZE_BEAST, PHAZE_CODER, PHAZE_PLANNER, PHAZE_REVIEWER];

    /// Base models used to build phaze Modelfiles
    pub const BASE_CODER: &str = "qwen2.5-coder:14b";
    pub const BASE_PLANNER: &str = "llama3.2:3b";
    pub const BASE_REVIEWER: &str = "deepseek-coder-v2:16b";

    /// Default cloud provider models
    pub const DEFAULT_CLAUDE_MODEL: &str = "claude-sonnet-4-5-20250929";
    pub const DEFAULT_OPENAI_MODEL: &str = "gpt-4o";
    pub const DEFAULT_GROQ_MODEL: &str = "llama-3.3-70b-versatile";
    pub const DEFAULT_TOGETHER_MODEL: &str = "deepseek-r1-distill-llama-70b";
    pub const DEFAULT_OPENROUTER_MODEL: &str = "anthropic/claude-sonnet-4-5";
    pub const DEFAULT_LMSTUDIO_MODEL: &str = "local-model";
}

// ─── API Endpoints ────────────────────────────────────────────────────────────

pub mod endpoints {
    pub const CLAUDE_BASE_URL: &str = "https://api.anthropic.com";
    pub const OPENAI_BASE_URL: &str = "https://api.openai.com";
    pub const GROQ_BASE_URL: &str = "https://api.groq.com/openai";
    pub const TOGETHER_BASE_URL: &str = "https://api.together.xyz";
    pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api";
    pub const OLLAMA_BASE_URL: &str = "http://localhost:11434";
    pub const LMSTUDIO_BASE_URL: &str = "http://localhost:1234";
    pub const SEARCH_ENGINE_URL: &str = "https://html.duckduckgo.com/html/?q={}";
}

// ─── Default Settings ─────────────────────────────────────────────────────────

pub mod defaults {
    pub const THEME: &str = "Midnight Blue";
    pub const FONT_SIZE: f32 = 14.0;
    pub const TAB_SIZE: u32 = 4;
    pub const MAX_TOKENS: u32 = 8192;
    pub const CONTEXT_WINDOW: u32 = 8192;
    pub const PYTHON_PATH: &str = "python3";
    pub const DEFAULT_MODEL: &str = super::models::PHAZE_BEAST;
}

// ─── Modelfile Hyperparameters ────────────────────────────────────────────────

pub mod modelfile {
    pub const CODER_TEMPERATURE: f32 = 0.3;
    pub const CODER_TOP_P: f32 = 0.9;
    pub const CODER_NUM_CTX: u32 = 32768;
    pub const CODER_REPEAT_PENALTY: f32 = 1.1;

    pub const PLANNER_TEMPERATURE: f32 = 0.5;
    pub const PLANNER_NUM_CTX: u32 = 8192;

    pub const REVIEWER_TEMPERATURE: f32 = 0.2;
    pub const REVIEWER_NUM_CTX: u32 = 16384;

    pub const DEFAULT_TEMPERATURE: f32 = 0.7;
    pub const DEFAULT_TOP_P: f32 = 0.9;
    pub const DEFAULT_NUM_CTX: u32 = 8192;
    pub const DEFAULT_REPEAT_PENALTY: f32 = 1.1;
    pub const MIN_CONTEXT_WINDOW: u32 = 1024;
    pub const MAX_CONTEXT_WINDOW: u32 = 128000;
}

// ─── Terminal ─────────────────────────────────────────────────────────────────

pub mod terminal {
    pub const SCROLLBACK_LIMIT: usize = 10000;
    pub const SCROLLBACK_DRAIN: usize = 1000;
    pub const READ_BUFFER_SIZE: usize = 8192;
    pub const TERM_TYPE: &str = "xterm-256color";
    pub const COLOR_TERM: &str = "truecolor";

    /// ANSI 16 base colors as (r, g, b)
    pub const ANSI_COLORS: [(u8, u8, u8); 16] = [
        (30, 30, 30),   // 0  Black
        (205, 49, 49),  // 1  Red
        (13, 188, 121), // 2  Green
        (229, 229, 16), // 3  Yellow
        (36, 114, 200), // 4  Blue
        (188, 63, 188), // 5  Magenta
        (17, 168, 205), // 6  Cyan
        (229, 229, 229),// 7  White
        (102, 102, 102),// 8  Bright Black
        (241, 76, 76),  // 9  Bright Red
        (35, 209, 139), // 10 Bright Green
        (245, 245, 67), // 11 Bright Yellow
        (59, 142, 234), // 12 Bright Blue
        (214, 112, 214),// 13 Bright Magenta
        (41, 184, 219), // 14 Bright Cyan
        (229, 229, 229),// 15 Bright White
    ];
}

// ─── Resource Limits ──────────────────────────────────────────────────────────

pub mod limits {
    pub const MAX_BASH_OUTPUT_CHARS: usize = 30000;
    pub const DEFAULT_BASH_TIMEOUT_SECS: u64 = 120;
    pub const MAX_FILES_PER_WORKSPACE: usize = 5000;
    pub const GIT_STATUS_MAX_FILES: usize = 20;
    pub const AGENT_HISTORY_MAX_RUNS: usize = 20;
    pub const AUTOSAVE_DEBOUNCE_MS: u64 = 500;
    pub const FILE_WATCH_POLL_SECS: u64 = 5;
}

// ─── Config Paths ─────────────────────────────────────────────────────────────

pub mod paths {
    pub const CONFIG_DIR: &str = "phazeai";
    pub const CONFIG_FILE: &str = "config.toml";
    pub const IDE_STATE_FILE: &str = "ide_state.json";
    pub const CONVERSATIONS_DIR: &str = "conversations";
    pub const INSTRUCTION_FILES: &[&str] = &[
        "CLAUDE.md",
        ".phazeai/instructions.md",
        ".phazeai/config.md",
        ".ai/instructions.md",
    ];
    pub const PROJECT_MARKERS: &[(&str, &str)] = &[
        ("Cargo.toml", "rust"),
        ("pyproject.toml", "python"),
        ("setup.py", "python"),
        ("requirements.txt", "python"),
        ("package.json", "javascript"),
        ("tsconfig.json", "typescript"),
        ("go.mod", "go"),
        ("pom.xml", "java"),
        ("build.gradle", "java"),
        ("CMakeLists.txt", "cpp"),
        ("Makefile", "make"),
        ("Gemfile", "ruby"),
    ];
}

// ─── UI Layout (egui — will be replaced by Floem layout in phazeai-ui) ────────

pub mod ui {
    pub const ACTIVITY_BAR_WIDTH: f32 = 48.0;
    pub const DEFAULT_EXPLORER_WIDTH: f32 = 220.0;
    pub const DEFAULT_CHAT_WIDTH: f32 = 320.0;
    pub const DEFAULT_TERMINAL_HEIGHT: f32 = 200.0;
    pub const STATUS_BAR_HEIGHT: f32 = 24.0;
    pub const MENU_BAR_HEIGHT: f32 = 24.0;
    pub const TAB_BAR_HEIGHT: f32 = 32.0;
    pub const BREADCRUMB_HEIGHT: f32 = 24.0;
    pub const MIN_PANEL_WIDTH: f32 = 150.0;
    pub const MAX_PANEL_WIDTH: f32 = 800.0;
    pub const MINIMAP_WIDTH: f32 = 80.0;
    pub const LINE_HEIGHT_OFFSET: f32 = 4.0;
    pub const MONOSPACE_CHAR_WIDTH_RATIO: f32 = 0.601;
}
