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
        settings.llm.provider = match provider.as_str() {
            "openai" => phazeai_core::config::LlmProvider::OpenAI,
            "ollama" => phazeai_core::config::LlmProvider::Ollama,
            _ => phazeai_core::config::LlmProvider::Claude,
        };
    }

    if let Some(prompt) = cli.prompt {
        app::run_single_prompt(&settings, &prompt).await?;
    } else {
        app::run_tui(settings, &cli.theme).await?;
    }

    Ok(())
}
