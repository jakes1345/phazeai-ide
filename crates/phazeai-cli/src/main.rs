use anyhow::Result;
use clap::Parser;

use phazeai_cli::app;
use phazeai_cli::oneshot::{self, Allow};
use phazeai_cli::setup;
use phazeai_cli::theme::{ColorMode, Theme};

#[derive(Parser)]
#[command(name = "phazeai")]
#[command(about = "PhazeAI - AI-powered coding assistant")]
#[command(version)]
struct Cli {
    /// Run a single prompt and exit (stdin, if piped, is appended as context)
    #[arg(short, long)]
    prompt: Option<String>,

    /// LLM model to use
    #[arg(short, long)]
    model: Option<String>,

    /// LLM provider: anthropic, openai, gemini, groq, together, openrouter, ollama, lmstudio
    #[arg(long)]
    provider: Option<String>,

    /// Color theme (dark, tokyo-night, dracula, catppuccin, gruvbox, nord, one-dark)
    #[arg(long, default_value = "dark")]
    theme: String,

    /// Disable colors (also honoured: the NO_COLOR environment variable)
    #[arg(long)]
    no_color: bool,

    /// Continue the most recent conversation
    #[arg(short = 'c', long = "continue")]
    continue_last: bool,

    /// Resume a specific conversation by ID (prefix match)
    #[arg(long)]
    resume: Option<String>,

    /// What -p mode may do without asking: read (default), edit (also change
    /// files with the file tools; shell commands stay read-only) or all (also
    /// run any command). Anything more is asked about on the terminal, or
    /// refused when input is piped.
    #[arg(long, value_enum, default_value_t = Allow::Read)]
    allow: Allow,

    /// Path to custom instructions file
    #[arg(long)]
    instructions: Option<String>,
}

/// In the TUI, log to a file: anything written to stderr would corrupt the
/// screen. Single-prompt mode logs to stderr as usual.
fn init_tracing(tui: bool) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false);
    if tui {
        let log_file = phazeai_core::Settings::config_path()
            .parent()
            .map(|dir| {
                let _ = std::fs::create_dir_all(dir);
                dir.join("cli.log")
            })
            .and_then(|path| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .ok()
            });
        match log_file {
            Some(file) => builder
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(file))
                .init(),
            None => builder.with_writer(std::io::sink).init(),
        }
    } else {
        builder.with_writer(std::io::stderr).init();
    }
}

#[tokio::main]
async fn main() {
    // Print errors as plain, readable text (the terminal is already restored
    // by the time an error reaches here) rather than anyhow's debug format.
    if let Err(e) = run(Cli::parse()).await {
        let color = ColorMode::detect(false).has_color()
            && std::io::IsTerminal::is_terminal(&std::io::stderr());
        let label = if color {
            "\x1b[1;31merror:\x1b[0m"
        } else {
            "error:"
        };
        eprintln!("{label} {e:#}");
        std::process::exit(1);
    }
}

/// Stops the built-in local AI server when dropped.
struct StopLocalAi;

impl Drop for StopLocalAi {
    fn drop(&mut self) {
        phazeai_core::local_ai::LOCAL_AI.stop();
    }
}

async fn start_local_ai_if_selected(
    settings: &phazeai_core::Settings,
    tui: bool,
) -> Result<Option<StopLocalAi>> {
    use phazeai_core::local_ai::LOCAL_AI;
    if settings.llm.provider != phazeai_core::config::LlmProvider::BuiltIn {
        return Ok(None);
    }
    eprintln!("Starting PhazeAI Local (llama.cpp)…");
    match LOCAL_AI.ensure_running(&settings.local_ai).await {
        Ok(url) => {
            eprintln!("PhazeAI Local ready at {url}");
            Ok(Some(StopLocalAi))
        }
        // The TUI still opens and shows why the model isn't available.
        Err(e) if tui => {
            eprintln!("PhazeAI Local couldn't start: {e}");
            Ok(Some(StopLocalAi))
        }
        Err(e) => anyhow::bail!("PhazeAI Local couldn't start: {e}"),
    }
}

async fn run(cli: Cli) -> Result<()> {
    use std::io::{IsTerminal, Read};
    let stdin_piped = !std::io::stdin().is_terminal();
    let tui = cli.prompt.is_none() && !stdin_piped;
    init_tracing(tui);

    // Anonymous telemetry — single fire-and-forget ping, no personal data
    phazeai_core::telemetry::report_launch(phazeai_core::telemetry::AppKind::Cli);

    if !Theme::exists(&cli.theme) {
        anyhow::bail!(
            "unknown theme '{}'. Available: {}",
            cli.theme,
            Theme::all_names().join(", ")
        );
    }

    // Sandbox tool filesystem access to the project root (git toplevel, falling
    // back to current dir). Tools refuse paths outside this root, including `..`
    // escapes and symlink escapes.
    if let Ok(cwd) = std::env::current_dir() {
        let workspace = std::process::Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(&cwd)
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    String::from_utf8(o.stdout)
                        .ok()
                        .map(|s| std::path::PathBuf::from(s.trim()))
                } else {
                    None
                }
            })
            .unwrap_or(cwd);
        phazeai_core::tools::sandbox::set_workspace_root(Some(workspace.clone()));
        // The project's AI privacy policy (.phazeai/privacy.toml at the same root).
        phazeai_core::privacy::set_active_project(&workspace);
        if let Some(err) = phazeai_core::privacy::active().load_error() {
            eprintln!("warning: {err} — cloud AI is blocked until it's fixed (see /privacy)");
        }
    }

    let mut settings = phazeai_core::Settings::load();

    if let Some(ref provider) = cli.provider {
        let Some(p) = setup::parse_provider(provider) else {
            anyhow::bail!(
                "unknown provider '{provider}'. Available: {}",
                setup::PROVIDER_NAMES.join(", ")
            );
        };
        if p != settings.llm.provider && cli.model.is_none() {
            // The configured model belongs to the old provider.
            settings.llm.model = setup::default_model_for(&settings, &p);
        }
        settings.llm.provider = p;
    }
    if let Some(ref model) = cli.model {
        settings.llm.model = model.clone();
    }

    let extra_instructions = match cli.instructions.as_deref() {
        Some(path) => Some(
            std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("couldn't read instructions file '{path}': {e}"))?,
        ),
        None => None,
    };

    // The built-in provider needs its llama.cpp server before the first request.
    // The guard stops it however `run` returns (Ok, `?`, or after the TUI exits).
    let _local_ai = start_local_ai_if_selected(&settings, tui).await?;

    // Auto-provision phaze-beast if needed
    if settings.llm.provider == phazeai_core::config::LlmProvider::Ollama
        && settings.llm.model == "phaze-beast"
    {
        let base_url = settings
            .llm
            .base_url
            .clone()
            .unwrap_or_else(|| "http://localhost:11434".to_string());

        if let Ok(manager) = phazeai_core::llm::OllamaManager::new(&base_url) {
            if let Err(e) = manager.ensure_phaze_beast().await {
                tracing::warn!(
                    "Failed to auto-provision phaze-beast: {e}. Falling back to existing models."
                );
            }
        }
    }

    let color = ColorMode::detect(cli.no_color);

    let mut stdin_data = String::new();
    if stdin_piped {
        if let Err(e) = std::io::stdin().read_to_string(&mut stdin_data) {
            tracing::warn!("Failed to read stdin: {e}");
        }
    }

    if let Some(prompt) = cli.prompt {
        let final_prompt = if stdin_data.is_empty() {
            prompt
        } else {
            format!("{}\n\n<stdin>\n{}\n</stdin>", prompt, stdin_data)
        };
        app::run_single_prompt(
            &settings,
            &final_prompt,
            extra_instructions.as_deref(),
            color,
            cli.allow,
            oneshot::can_ask(stdin_piped),
        )
        .await?;
    } else if stdin_piped {
        if stdin_data.trim().is_empty() {
            anyhow::bail!("stdin was empty; pass a prompt with -p or pipe some text in");
        }
        app::run_single_prompt(
            &settings,
            &stdin_data,
            extra_instructions.as_deref(),
            color,
            cli.allow,
            false,
        )
        .await?;
    } else {
        app::run_tui(
            settings,
            app::TuiOptions {
                theme: &cli.theme,
                color,
                continue_last: cli.continue_last,
                resume_id: cli.resume,
                extra_instructions: extra_instructions.as_deref(),
            },
        )
        .await?;
    }

    Ok(())
}
