// PhazeAI — centralized constants.
// All magic numbers, strings, and limits live here.
// Never hardcode these values elsewhere.

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
    /// Default Gemini model — 2.5 Flash: fast, cheap, 1M context, optional thinking
    pub const DEFAULT_GEMINI_MODEL: &str = "gemini-2.5-flash";
    pub const GEMINI_PRO_MODEL: &str = "gemini-2.5-pro";
    pub const GEMINI_FLASH_MODEL: &str = "gemini-2.5-flash";
    pub const GEMINI_FLASH_LITE_MODEL: &str = "gemini-2.0-flash-lite";
    pub const GEMINI_FLASH_20_MODEL: &str = "gemini-2.0-flash";
    pub const GEMINI_15_PRO_MODEL: &str = "gemini-1.5-pro";
    pub const GEMINI_15_FLASH_MODEL: &str = "gemini-1.5-flash";
    pub const GEMINI_EMBEDDING_MODEL: &str = "text-embedding-004";

    /// Mistral AI
    pub const DEFAULT_MISTRAL_MODEL: &str = "mistral-large-latest";
    /// DeepSeek (direct API, cheaper than via Together/OpenRouter)
    pub const DEFAULT_DEEPSEEK_MODEL: &str = "deepseek-chat";
    /// xAI Grok
    pub const DEFAULT_XAI_MODEL: &str = "grok-3";
    /// Cohere Command
    pub const DEFAULT_COHERE_MODEL: &str = "command-r-plus-08-2024";
    /// Perplexity Sonar (web-grounded)
    pub const DEFAULT_PERPLEXITY_MODEL: &str = "sonar-pro";
    /// Azure OpenAI — deployment name set by user in base URL
    pub const DEFAULT_AZURE_MODEL: &str = "gpt-4o";
    /// Cerebras (fast Llama inference)
    pub const DEFAULT_CEREBRAS_MODEL: &str = "llama3.1-70b";
    /// Fireworks AI
    pub const DEFAULT_FIREWORKS_MODEL: &str = "accounts/fireworks/models/llama-v3p1-70b-instruct";
    /// SambaNova Cloud
    pub const DEFAULT_SAMBANOVA_MODEL: &str = "Meta-Llama-3.1-405B-Instruct";
    /// GitHub Models (Azure-hosted OSS + OpenAI models, free with GitHub token)
    pub const DEFAULT_GITHUB_MODELS_MODEL: &str = "gpt-4o";
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
    pub const LMSTUDIO_PORT: u16 = 1234;
    /// Google Gemini OpenAI-compatible endpoint (works with existing OpenAIClient)
    pub const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai/";
    /// Gemini native REST endpoint (for context caching, thinking mode, grounding)
    pub const GEMINI_NATIVE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
    /// Pi Ollama LAN endpoint (configured via setup script)
    pub const PI_OLLAMA_LAN_URL: &str = "http://192.168.1.155:8080";
    pub const SEARCH_ENGINE_URL: &str = "https://html.duckduckgo.com/html/?q={}";

    pub const MISTRAL_BASE_URL: &str = "https://api.mistral.ai/v1";
    pub const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/v1";
    pub const XAI_BASE_URL: &str = "https://api.x.ai/v1";
    pub const COHERE_BASE_URL: &str = "https://api.cohere.com/compatibility/v1";
    pub const PERPLEXITY_BASE_URL: &str = "https://api.perplexity.ai";
    /// Azure OpenAI: user must set base_url = https://{resource}.openai.azure.com/openai/deployments/{deployment}
    pub const AZURE_BASE_URL: &str = "";
    pub const CEREBRAS_BASE_URL: &str = "https://api.cerebras.ai/v1";
    pub const FIREWORKS_BASE_URL: &str = "https://api.fireworks.ai/inference/v1";
    pub const SAMBANOVA_BASE_URL: &str = "https://fast-api.snova.ai/v1";
    /// GitHub Models — free OSS + OpenAI models via GitHub token
    pub const GITHUB_MODELS_BASE_URL: &str = "https://models.inference.ai.azure.com";
}

// ─── Default Settings ─────────────────────────────────────────────────────────

pub mod defaults {
    pub const THEME: &str = "Midnight Blue";
    pub const FONT_SIZE: f32 = 14.0;
    pub const TAB_SIZE: u32 = 4;
    pub const MAX_TOKENS: u32 = 8192;
    pub const CONTEXT_WINDOW: u32 = 8192;
    pub const PYTHON_PATH: &str = "python3";
    /// Safer out-of-the-box default than custom phaze-* models.
    /// `llama3.2:3b` is a public Ollama model users can pull directly.
    pub const DEFAULT_MODEL: &str = super::models::BASE_PLANNER;
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
        (30, 30, 30),    // 0  Black
        (205, 49, 49),   // 1  Red
        (13, 188, 121),  // 2  Green
        (229, 229, 16),  // 3  Yellow
        (36, 114, 200),  // 4  Blue
        (188, 63, 188),  // 5  Magenta
        (17, 168, 205),  // 6  Cyan
        (229, 229, 229), // 7  White
        (102, 102, 102), // 8  Bright Black
        (241, 76, 76),   // 9  Bright Red
        (35, 209, 139),  // 10 Bright Green
        (245, 245, 67),  // 11 Bright Yellow
        (59, 142, 234),  // 12 Bright Blue
        (214, 112, 214), // 13 Bright Magenta
        (41, 184, 219),  // 14 Bright Cyan
        (229, 229, 229), // 15 Bright White
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

// ─── UI Layout (Floem-based layout in phazeai-ui) ──────────────────────────────

pub mod ui {
    pub const ACTIVITY_BAR_WIDTH: f32 = 48.0;
    pub const DEFAULT_EXPLORER_WIDTH: f32 = 220.0;
    pub const DEFAULT_CHAT_WIDTH: f32 = 320.0;
    pub const DEFAULT_TERMINAL_HEIGHT: f32 = 200.0;
    pub const STATUS_BAR_HEIGHT: f32 = 22.0;
    pub const MENU_BAR_HEIGHT: f32 = 28.0;
    pub const TAB_BAR_HEIGHT: f32 = 32.0;
    pub const BREADCRUMB_HEIGHT: f32 = 24.0;
    pub const MIN_PANEL_WIDTH: f32 = 150.0;
    pub const MAX_PANEL_WIDTH: f32 = 800.0;
    pub const MINIMAP_WIDTH: f32 = 80.0;
    pub const LINE_HEIGHT_OFFSET: f32 = 4.0;
    pub const MONOSPACE_CHAR_WIDTH_RATIO: f32 = 0.601;

    // Popup/dropdown constraints
    pub const MAX_DROPDOWN_HEIGHT: f64 = 200.0;
    pub const MAX_LIST_HEIGHT: f64 = 150.0;
    pub const MAX_DIFF_HEIGHT: f64 = 400.0;
    pub const COMPLETION_POPUP_WIDTH: f64 = 420.0;
    pub const COMPLETION_POPUP_MAX_HEIGHT: f64 = 280.0;

    // Text truncation
    pub const GIT_CONTENT_TRUNCATE: usize = 60;
    pub const DIFF_LINE_TRUNCATE: usize = 200;

    // Overlay z-index levels (higher = on top)
    pub const Z_DRAG_OVERLAY: i32 = 50;
    pub const Z_COMMAND_PALETTE: i32 = 100;
    pub const Z_FILE_PICKER: i32 = 200;
    pub const Z_HOVER_TIP: i32 = 250;
    pub const Z_COMPLETIONS: i32 = 300;
    pub const Z_CODE_ACTIONS: i32 = 350;
    pub const Z_SIG_HELP: i32 = 380;
    pub const Z_INLINE_EDIT: i32 = 400;
    pub const Z_RENAME: i32 = 420;
    pub const Z_TOAST: i32 = 450;
    pub const Z_WS_SYMBOLS: i32 = 460;
    pub const Z_BRANCH_PICKER: i32 = 470;
    pub const Z_PEEK_DEF: i32 = 485;
    pub const Z_VIM_EX: i32 = 490;
    pub const Z_GOTO: i32 = 495;
}
