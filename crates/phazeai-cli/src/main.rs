use anyhow::Result;
use clap::Parser;

mod app;
mod commands;
mod theme;

#[derive(Parser)]
#[command(name = "phazeai")]
#[command(about = "PhazeAI - AI-powered coding assistant")]
#[command(version)]
struct Cli {
    /// Run a single prompt and exit
    #[arg(short, long)]
    prompt: Option<String>,

    /// LLM model to use
    #[arg(short, long)]
    model: Option<String>,

    /// LLM provider (claude, openai, ollama)
    #[arg(long)]
    provider: Option<String>,

    /// Color theme (dark, tokyo-night, dracula)
    #[arg(long, default_value = "dark")]
    theme: String,

    /// Continue the most recent conversation
    #[arg(short = 'c', long = "continue")]
    continue_last: bool,

    /// Resume a specific conversation by ID (prefix match)
    #[arg(long)]
    resume: Option<String>,

    /// Path to custom instructions file
    #[arg(long)]
    instructions: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();

    let mut settings = phazeai_core::Settings::load();

    if let Some(ref model) = cli.model {
        settings.llm.model = model.clone();
    }
    if let Some(ref provider) = cli.provider {
        settings.llm.provider = match provider.to_lowercase().as_str() {
            "openai" | "gpt" => phazeai_core::config::LlmProvider::OpenAI,
            "ollama" | "local" => phazeai_core::config::LlmProvider::Ollama,
            "groq" => phazeai_core::config::LlmProvider::Groq,
            "together" => phazeai_core::config::LlmProvider::Together,
            "openrouter" | "or" => phazeai_core::config::LlmProvider::OpenRouter,
            "lmstudio" | "lm-studio" | "lm_studio" => phazeai_core::config::LlmProvider::LmStudio,
            "claude" | "anthropic" => phazeai_core::config::LlmProvider::Claude,
            _ => phazeai_core::config::LlmProvider::Claude,
        };
    }

    let extra_instructions = if let Some(ref instructions_path) = cli.instructions {
        std::fs::read_to_string(instructions_path)
            .ok()
    } else {
        None
    };

    // Auto-provision phaze-beast if needed
    if settings.llm.provider == phazeai_core::config::LlmProvider::Ollama 
        && settings.llm.model == "phaze-beast" 
    {
        let base_url = settings.llm.base_url.clone()
            .unwrap_or_else(|| "http://localhost:11434".to_string());
        
        if let Ok(manager) = phazeai_core::llm::OllamaManager::new(&base_url) {
            if let Err(e) = manager.ensure_phaze_beast().await {
                tracing::warn!("Failed to auto-provision phaze-beast: {e}. Falling back to existing models.");
            }
        }
    }

    if let Some(prompt) = cli.prompt {
        app::run_single_prompt(&settings, &prompt, extra_instructions.as_deref()).await?;
    } else {
        app::run_tui(settings, &cli.theme, cli.continue_last, cli.resume, extra_instructions.as_deref()).await?;
    }

    Ok(())
}
