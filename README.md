# PhazeAI IDE

[![CI](https://github.com/jakes1345/phazeai-ide/actions/workflows/ci.yml/badge.svg)](https://github.com/jakes1345/phazeai-ide/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/Rust-1.93+-orange.svg)](https://www.rust-lang.org/)
[![Discord](https://img.shields.io/badge/Discord-Community-5865F2.svg)](https://discord.gg/phazeai)

**The AI-native IDE built entirely in Rust. GPU-rendered, local-first, open-source core.**

PhazeAI IDE is a next-generation code editor designed from the ground up for AI-assisted development. No Electron bloat. No heavy Python processes. Just pure Rust performance with a GPU-accelerated UI that responds instantly.

---

## Why PhazeAI?

- **Native Performance**: Built entirely in Rust with GPU acceleration via [Floem](https://github.com/lapce/floem). 3-5x faster startup than VS Code, responsive UI that never freezes.
- **True Open Source**: Core IDE, CLI, and all features are permanently free and MIT licensed. Zero telemetry. No license servers. Your code stays yours.
- **Local Models Work**: Use Ollama, LM Studio, or any OpenAI-compatible provider locally. No API keys required. Run your own inference on consumer hardware.
- **Choose Your AI**: Bring your own OpenAI/Claude/Groq keys, or subscribe to PhazeAI Cloud for hosted models with one-click setup. Start free forever.

---

## Quick Start

### Desktop IDE (Recommended)
```bash
git clone https://github.com/jakes1345/phazeai-ide.git
cd phazeai-ide
cargo run -p phazeai-ui --release
```

### Terminal UI
```bash
cargo install --path crates/phazeai-cli
phazeai
```

Binary releases coming soon to crates.io and GitHub Releases.

---

## Features

### Editor Core
- **Syntax highlighting** for 25+ languages via syntect
- **Multi-tab editing** with persistent session across restarts
- **Multi-cursor editing** — Ctrl+D selects next occurrence, Alt+Click adds cursors
- **Find & Replace** with regex support (Ctrl+F / Ctrl+H)
- **Code folding** — Ctrl+Shift+[ / Ctrl+Shift+]
- **Bracket matching** with auto-close
- **LSP integration** — Autocomplete (Ctrl+Space), go-to-definition (F12), hover docs (Ctrl+F1)
- **File explorer** with git status badges
- **Command palette** (Ctrl+P) and quick file picker (Ctrl+Shift+P)
- **Vim mode** — Normal/Insert modes, motions h/j/k/l/w/b/0/$, dd, x, o, and more

### AI Integration
- **Inline AI edit** (Ctrl+K): Select code, describe what you want, AI rewrites it in place
- **Streaming chat panel**: Real-time responses from Claude, GPT-4, or local models
- **Multi-agent pipeline**: Planner → Coder → Reviewer with approval gates
- **Cancel/retry**: Stop a running AI request, retry from the last message
- **Conversation persistence**: Chat history saved to disk, survives restarts
- **Chat modes**: Chat, Ask, Debug, Plan, Edit — each with tailored system prompts
- **Terminal integration**: Agent runs shell commands, output streams into terminal
- **Ghost text completions**: Tab to accept AI suggestions (FIM fill-in-the-middle)

### Terminals & Tools
- **Terminal emulation**: Full PTY with 256-color, multiple tabs, named terminals
- **Shell profile selection**: bash, zsh, fish — auto-detected
- **Clipboard integration**: Ctrl+Shift+C/V copy/paste
- **Hyperlink detection**: Click URLs to open in browser
- **Working directory tracking**: Auto-detects shell CWD via OSC 7
- **Git panel**: Status, stage/unstage, discard, commit with message editor
- **Git gutter decorations**: Green/yellow/red indicators in editor margin
- **Git blame**: Per-line commit attribution with hover info
- **Branch operations**: Switch, create, merge, stash via UI
- **Pull/push**: One-click Git pull and push buttons
- **Problems panel**: LSP diagnostics with error/warning badges
- **Search panel**: Workspace search with ripgrep, regex, replace-in-files
- **Settings panel**: Theme, font size, tab size, AI provider/model — all persisted

### Built-in Themes
MidnightBlue, Cyberpunk, Dracula, Tokyo Night, Material, Nord, Catppuccin, Solarized, Gruvbox, Monokai, One Dark, GitHub Light.

---

## Keybindings

| Action | Windows/Linux | macOS |
|--------|---------------|-------|
| Command palette | Ctrl+Shift+P | Cmd+Shift+P |
| Quick open (file picker) | Ctrl+P | Cmd+P |
| Find in file | Ctrl+F | Cmd+F |
| Find and replace | Ctrl+H | Cmd+H |
| Go to line | Ctrl+G | Cmd+G |
| AI inline edit | Ctrl+K | Cmd+K |
| Show completions | Ctrl+Space | Cmd+Space |
| Toggle explorer (left panel) | Ctrl+B | Cmd+B |
| Toggle terminal (bottom) | Ctrl+J | Cmd+J |
| Toggle chat (right panel) | Ctrl+\ | Cmd+\ |
| Undo | Ctrl+Z | Cmd+Z |
| Redo | Ctrl+Shift+Z | Cmd+Shift+Z |
| Multi-cursor next | Ctrl+D | Cmd+D |
| Cut line | Ctrl+X | Cmd+X |
| Comment line | Ctrl+/ | Cmd+/ |
| Format document | Shift+Alt+F | Shift+Cmd+F |
| Go to definition | F12 | F12 |
| Rename symbol | F2 | F2 |

---

## AI Providers

PhazeAI works with any LLM provider. Configure in Settings panel or `~/.config/phazeai/settings.toml`:

| Provider | Type | Cost | Setup |
|----------|------|------|-------|
| **Anthropic Claude** | Cloud | BYOK | API key |
| **OpenAI** | Cloud | BYOK | API key |
| **Google Gemini** | Cloud | BYOK | API key |
| **Groq** | Cloud | BYOK | API key |
| **Together.ai** | Cloud | BYOK | API key |
| **OpenRouter** | Cloud | BYOK | API key |
| **Ollama** | Local | Free | [Download](https://ollama.ai) + `ollama pull llama2` |
| **LM Studio** | Local | Free | [Download](https://lmstudio.ai) |

**Recommended for new users**: Download [Ollama](https://ollama.ai), run `ollama pull llama2`, then configure PhazeAI to use `http://localhost:11434`. Zero cost, zero setup, runs offline.

---

## Build from Source

### Requirements
- **Rust 1.70+** ([install](https://rustup.rs/))
- **Linux**: `build-essential`, `libxcb-render0-dev`, `libxcb-shape0-dev`, `libxcb-xfixes0-dev`
- **macOS**: Xcode Command Line Tools
- **Windows**: MSVC or MinGW toolchain

### Install Linux Dependencies
```bash
# Ubuntu/Debian
sudo apt install build-essential libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev

# Fedora
sudo dnf install gcc libxcb-devel libxkbcommon-devel

# Arch
sudo pacman -S base-devel libxcb xorg-x11-util-macros
```

### Build Commands
```bash
# Desktop GUI (Floem-based, GPU-accelerated)
cargo build -p phazeai-ui --release
./target/release/phazeai-ui

# Terminal UI (ratatui-based)
cargo build -p phazeai-cli --release
./target/release/phazeai

# All crates
cargo build --workspace --release

# Run tests
cargo test --workspace

# Lint and format
cargo fmt --all
cargo clippy --workspace -- -D warnings
```

---

## Configuration

Settings are stored at `~/.config/phazeai/settings.toml` (auto-created on first run):

```toml
# AI Provider
[ai]
provider = "ollama"  # ollama, claude, openai, groq, together, openrouter
model = "llama2"
api_key = ""         # leave empty for local providers
api_url = "http://localhost:11434"  # for ollama

# Editor
[editor]
font_family = "Fira Code"
font_size = 14
theme = "MidnightBlue"
tab_size = 4

# IDE
[ide]
auto_save = true
```

### Cloud Provider API Keys
```bash
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENAI_API_KEY="sk-..."
export GROQ_API_KEY="gsk_..."
export TOGETHER_API_KEY="..."
export OPENROUTER_API_KEY="sk-or-..."
```

---

## Workspace Structure

```
crates/
├── phazeai-core/        # Shared engine: agent loop, LLM clients, tools, LSP
├── phazeai-ui/          # Desktop GUI (Floem + GPU acceleration) [PRIMARY]
├── phazeai-cli/         # Terminal UI (ratatui)
├── phazeai-cloud/       # Optional: paid cloud client
├── phazeai-sidecar/     # Optional: Python semantic search
├── phazeai-plugin-api/  # Plugin API for native extensions
├── ollama-rs/           # Custom fork with streaming + chat history
└── ext-host/wasm-extension/  # WASM plugin host
```

All crates are optional except `phazeai-core`. Use what you need.

---

## Contributing

PhazeAI is open-source and community-driven. See [CONTRIBUTING.md](./CONTRIBUTING.md) for guidelines.

---

## Roadmap

### Phase 2/3: Ship ✅ (Mostly complete)
Core editor, LSP, terminal, git panel, AI chat, session persistence, Vim mode, and all major IDE features are implemented. See [TODO.md](./TODO.md) for the full feature checklist.

### Coming Soon
- Multi-line find in editor
- Terminal split views
- Scrollback buffer limit configuration
- Multi-repo workspace support
- LSP call hierarchy
- Semantic token highlighting
- Real-time collaboration (CRDT sync)
- Remote SSH development
- Integrated debugger (DAP)
- Plugin system (WASM sandbox)

---

## License

PhazeAI IDE is licensed under the **MIT License**. See [LICENSE](./LICENSE) for details.

All dependencies are either MIT or Apache-2.0 licensed.

---

## FAQ

**Q: Is PhazeAI production-ready?**
A: The IDE is usable daily for coding. Phase 2/3 features (core editor, LSP, terminal, AI chat, git) are complete. Advanced features (debugger, collaboration) are planned for Phase 4.

**Q: Can I use my own OpenAI API key?**
A: Yes. Set `OPENAI_API_KEY` env var and configure the provider in settings.

**Q: Does PhazeAI collect data?**
A: No. Core IDE has zero telemetry. Cloud tier stores conversation history on encrypted servers only.

**Q: Why Rust instead of Electron?**
A: Rust gives us native performance (3-5x faster startup) without the 400MB memory footprint. GPU acceleration makes the UI instant.

**Q: Can I run PhazeAI offline?**
A: Yes. Use with Ollama or LM Studio. Cloud tier requires internet for model inference only.

**Q: How do I report a bug?**
A: Open a GitHub Issue with reproduction steps. Include `rustc --version`, `cargo --version`, and your OS.

---

## Community

- **Discord**: [Join our server](https://discord.gg/phazeai)
- **GitHub Issues**: Bug reports and feature requests
- **GitHub Discussions**: Ask questions and share ideas

---

**Built with love by the PhazeAI team and community.**
