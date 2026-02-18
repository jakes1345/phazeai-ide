# PhazeAI IDE

**The AI-powered IDE that runs 100% on your machine. No cloud. No subscriptions. Your code stays yours.**

![Rust](https://img.shields.io/badge/Rust-000000?style=flat&logo=rust&logoColor=white)
![Ollama](https://img.shields.io/badge/Ollama-Local%20AI-blue)
![License](https://img.shields.io/badge/License-MIT-green)

---

## Overview

PhazeAI is a multi-interface AI coding assistant with a **local-first multi-agent system** and a **custom model training pipeline**. It brings agentic capabilities to your development workflow — all running locally through [Ollama](https://ollama.ai).

The project consists of four integrated crates:

- **phazeai-core**: Core agent engine, LLM providers, tool system, multi-agent orchestrator, LSP client, code analysis
- **phazeai-cli**: Terminal UI with ratatui — slash commands, conversation persistence, file tree
- **phazeai-ide**: GUI application built with eframe/egui — editor, chat, terminal, file explorer
- **phazeai-sidecar**: Python process manager for semantic code search using TF-IDF and optional embeddings

### Key Capabilities

- **🤖 Local Multi-Agent System**: Planner → Coder → Reviewer pipeline running entirely through Ollama
- **🧠 Custom AI Models**: Pre-built Ollama Modelfiles + QLoRA fine-tuning pipeline to train on YOUR codebase
- **🔧 Agentic Tool Execution**: Read, write, edit files; execute bash; search with grep/glob
- **🔌 Multi-Provider LLM Support**: Ollama, Claude, OpenAI, Groq, Together, OpenRouter, LM Studio
- **📊 Code Intelligence**: LSP client (20+ languages), code outline extractor, Aider-style repo map
- **🔀 Git Integration**: View status, diffs, logs; stage and commit from the assistant
- **💬 Conversation Persistence**: Save, load, resume conversations with history management
- **🛡️ Tool Approval System**: Per-tool approval (auto, ask, ask-once) for safe execution
- **📁 Project Instructions**: `.phazerules`, `.cursorrules`, `CLAUDE.md` support
- **🎨 Customizable Themes**: Dark, Tokyo Night, and Dracula themes

## Architecture

```
phazeai_ide/
├── crates/
│   ├── phazeai-core/          # Core engine
│   │   ├── agent/             # Agent loop + multi-agent orchestrator
│   │   ├── llm/               # LLM clients, Ollama manager, model router
│   │   ├── tools/             # Tool definitions and registry
│   │   ├── context/           # Conversation history, system prompts, context builder
│   │   ├── config/            # Settings and configuration
│   │   ├── lsp/               # LSP client + manager (20+ languages)
│   │   ├── analysis/          # Code outline extractor + repo map generator
│   │   └── git/               # Git operations
│   │
│   ├── phazeai-cli/           # Terminal UI (ratatui)
│   ├── phazeai-ide/           # GUI application (egui)
│   └── phazeai-sidecar/       # Python semantic search
│
├── modelfiles/                # Ollama Modelfiles for custom AI models
│   ├── Modelfile.coder        # Code generation (Qwen 2.5 Coder 14B)
│   ├── Modelfile.planner      # Planning (Llama 3.2 3B)
│   ├── Modelfile.reviewer     # Code review (DeepSeek Coder V2 16B)
│   └── install.sh             # One-click model installer
│
├── training/                  # QLoRA fine-tuning pipeline
│   ├── prepare_data.py        # Collect + format training data
│   ├── fine_tune.py           # Unsloth QLoRA training
│   ├── export_gguf.py         # Export to GGUF for Ollama
│   └── README.md              # Training guide
│
└── Cargo.toml                 # Workspace configuration
```

### Multi-Agent Pipeline

```
User Request → Planner (3B, fast) → Coder (14B, precise) → Reviewer (16B, thorough) → Output
                  ↓                      ↓                       ↓
           Step-by-step plan      Production code         Bug/security check
```

Each agent role can use a different model, optimized for its task. All running locally through Ollama.

## Custom AI Models

### Quick Setup (Instant — No Training Required)

```bash
# Install pre-configured PhazeAI models
cd modelfiles && bash install.sh
```

This creates three specialized models in Ollama:

| Model | Base | Purpose | Speed |
|---|---|---|---|
| `phaze-coder` | Qwen 2.5 Coder 14B | Code generation | Medium |
| `phaze-planner` | Llama 3.2 3B | Planning & analysis | Fast |
| `phaze-reviewer` | DeepSeek Coder V2 16B | Code review | Thorough |

### Train Your Own Model (Advanced)

Fine-tune on YOUR codebase using QLoRA. Requires NVIDIA GPU (8GB+ VRAM).

```bash
# 1. Install dependencies
pip install unsloth transformers datasets trl peft bitsandbytes

# 2. Prepare training data from your projects
python training/prepare_data.py ~/my-project1 ~/my-project2

# 3. Fine-tune (2-6 hours depending on GPU)
python training/fine_tune.py

# 4. Export to Ollama
python training/export_gguf.py

# 5. Test
ollama run phaze-coder-custom "Write a function to merge sorted arrays"
```

See [training/README.md](training/README.md) for full details.

## Getting Started

### Prerequisites

- **NVIDIA GPU (8GB+ VRAM Recommended)**: See [HARDWARE.md](HARDWARE.md) for a tested reference setup.
- **Ollama**: Install from [ollama.com](https://ollama.com).
- **Rust 1.70+**: Install from [rustup.rs](https://rustup.rs/).
- **Python 3.8+**: For semantic search and training pipeline.

### Installation

1. Clone the repository:
```bash
git clone https://github.com/phazeai/ide.git
cd phazeai_ide
```

2. Build the project:
```bash
cargo build --release
```

The compiled binaries will be in `target/release/`:
- `phazeai` - Terminal UI
- `phazeai-ide` - GUI application

### Quick Start

#### Terminal UI
```bash
./target/release/phazeai
```

Run a single prompt without interactive mode:
```bash
./target/release/phazeai --prompt "Explain this file" --model claude-3-5-sonnet-20241022
```

Continue the most recent conversation:
```bash
./target/release/phazeai --continue
```

Resume a specific conversation:
```bash
./target/release/phazeai --resume abc123
```

#### GUI Application
```bash
./target/release/phazeai-ide
```

Opens with default window size (1400x800) and loads your saved configuration.

## Configuration

Configuration is stored at `~/.config/phazeai/settings.toml` and is automatically created on first run with sensible defaults.

### Example Configuration

```toml
[llm]
provider = "claude"              # claude, openai, ollama, groq, together, openrouter, lmstudio
model = "claude-3-5-sonnet-20241022"
api_key_env = "ANTHROPIC_API_KEY"
base_url = null                 # Optional: override provider URL
max_tokens = 8192

[editor]
theme = "Dark"                  # Dark, TokyoNight, Dracula
font_size = 14.0
tab_size = 4
show_line_numbers = true
auto_save = true

[sidecar]
enabled = true
python_path = "python3"
auto_start = true

# Optional: Configure additional providers
[[providers]]
name = "local_ollama"
enabled = true
api_key_env = "OLLAMA_API_KEY"
base_url = "http://localhost:11434"
default_model = "llama2"
```

### Environment Variables

Most LLM providers require an API key:

```bash
export ANTHROPIC_API_KEY="your-key-here"        # Claude
export OPENAI_API_KEY="your-key-here"           # OpenAI
export GROQ_API_KEY="your-key-here"             # Groq
export TOGETHER_API_KEY="your-key-here"         # Together
export OPENROUTER_API_KEY="your-key-here"       # OpenRouter
export LM_STUDIO_API_KEY="your-key-here"        # LM Studio
```

For Ollama and LM Studio (local models), no API key is required. They run on your machine.

## CLI Commands

The terminal UI supports the following slash commands:

### General
- `/help` - Show command help
- `/exit`, `/quit` - Exit the application
- `/version` - Show application version
- `/status` - Show current model, token usage, and settings
- `/pwd` - Print working directory
- `/cd <directory>` - Change working directory

### Model & Provider
- `/model <name>` - Change the LLM model
- `/provider <name>` - Change the LLM provider (claude, openai, ollama, groq, together, openrouter, lmstudio)
- `/models` - List available models for the current provider
- `/discover` - Discover local LLM models (Ollama, LM Studio)

### Display & Theme
- `/theme <name>` - Change theme (dark, tokyo-night, dracula)
- `/files` or `/tree` - Toggle file tree panel visibility

### Conversations
- `/new` - Start a fresh conversation
- `/clear` - Clear the chat history
- `/save` - Save the current conversation
- `/load <id>` - Load a saved conversation (by ID prefix)
- `/conversations` or `/history` - List all saved conversations
- `/compact` - Summarize conversation to reduce token usage

### Tools & Git
- `/approve <mode>` - Set tool approval mode: `auto`, `ask`, `ask-once`
- `/diff` - Show git diff
- `/git` - Show git status
- `/log` - Show git log
- `/search <pattern>` - Search files with glob pattern (e.g., `/search **/*.rs`)
- `/context` - Show project context information

### Examples

```bash
# Change model
/model gpt-4o

# Use a local Ollama model
/provider ollama
/model mistral

# Set approval mode
/approve auto

# Search for Python files
/search **/*.py

# View git status and diffs
/git
/diff

# Save and load conversations
/save
/load abc123
```

## IDE Features

The GUI application (phazeai-ide) provides a comprehensive development environment:

### Editor Panel
- Syntax highlighting with language detection
- Line numbering and line wrapping
- Keyboard navigation (Ctrl+G to go to line)
- File content editing with auto-save support

### Chat Panel
- Conversation history with user and assistant messages
- Slash command support (same as CLI)
- Tool execution display and results
- Token usage and cost tracking

### Terminal Panel
- Integrated terminal for running bash commands
- Scrollback history
- Command execution feedback

### File Explorer
- Browse project files and directories
- Quick file opening
- Integrated with chat context

### Settings Panel
- Change LLM provider and model
- Configure editor preferences
- Manage theme selection
- Adjust sidecar settings

### Command Palette
- Quick access to all features via keyboard shortcut
- Search and execute commands
- Model and file switching

## Tools

PhazeAI provides the following tools that the AI can autonomously execute:

### File Operations
- **read_file** - Read file contents
- **write_file** - Create or overwrite files
- **edit_file** - Edit specific regions within files

### System Commands
- **bash** - Execute shell commands and scripts
- **grep** - Search file contents with regex patterns
- **glob** - Find files matching patterns

### Project Context
- **git_status** - View git repository status
- **git_diff** - Show changes between commits
- **git_log** - View commit history

### Code Search
- **semantic_search** - Search code semantically using the Python sidecar

Tools are subject to an approval system. By default, tools are approved automatically, but you can configure per-tool or per-type approval policies.

## Development

### Project Structure

The workspace uses Cargo's workspace feature for easy development:

```bash
# Build all crates
cargo build --release

# Build specific crate
cargo build -p phazeai-cli --release
cargo build -p phazeai-ide --release

# Run tests
cargo test

# Run with logging
RUST_LOG=debug cargo run -p phazeai-cli

# Format code
cargo fmt

# Lint
cargo clippy
```

### Adding New Tools

1. Define the tool in `phazeai-core/src/tools/mod.rs`
2. Implement the tool handler in the agent loop
3. Add approval rules if needed
4. Update CLI commands if user-facing

### Adding New LLM Providers

1. Implement `LlmClient` trait in `phazeai-core/src/llm/`
2. Register the provider in `ProviderRegistry`
3. Update configuration to support new provider

### Adding New Themes

1. Add theme definition to `phazeai-cli/src/theme.rs` or `phazeai-ide/src/themes.rs`
2. Update theme selector commands
3. Test in both TUI and GUI

## Dependencies

Key dependencies:

- **Runtime**: tokio (async runtime), serde (serialization)
- **TUI**: ratatui (terminal UI), crossterm (terminal control), syntect (syntax highlighting)
- **GUI**: egui/eframe (immediate mode GUI), ropey (text editing), rfd (file dialogs)
- **LLM**: reqwest (HTTP client), async-trait (async trait support)
- **Tools**: regex, ignore (gitignore support), globset (glob patterns), similar (diff algorithm)
- **Git**: git2 integration for repository operations
- **Utilities**: uuid (ID generation), chrono (timestamps), tracing (logging)

## License

MIT License - See LICENSE file for details.

## Contributing

Contributions are welcome! Please ensure:

- Code is formatted with `cargo fmt`
- All tests pass with `cargo test`
- No warnings with `cargo clippy`
- Meaningful commit messages
- Documentation for new features

## Support

For issues, questions, or suggestions:

- Open an issue on GitHub
- Check existing documentation
- Review examples in the codebase
