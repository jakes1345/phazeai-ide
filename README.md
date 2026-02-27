# PhazeAI IDE

[![CI](https://github.com/jakes1345/phazeai-ide/actions/workflows/ci.yml/badge.svg)](https://github.com/jakes1345/phazeai-ide/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/Rust-1.70+-orange.svg)](https://www.rust-lang.org/)
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
# Clone the repository
git clone https://github.com/phazeai/ide.git
cd ide

# Build and run (requires Rust 1.70+)
cargo run -p phazeai-ui --release
```

Binary releases coming soon to crates.io and GitHub Releases.

### Terminal UI
```bash
# Install from source
cargo install --path crates/phazeai-cli

# Or from crates.io (coming soon)
cargo install phazeai-cli

# Run TUI
phazeai
```

---

## Features

### Editor Core
- **Syntax highlighting** for 25+ languages (Rust, Python, JavaScript, Go, TypeScript, etc.)
- **Rope-based text buffer** with O(log n) edits and efficient undo/redo
- **Multi-cursor editing** — Alt+Click to add cursors, Ctrl+D to select next occurrence
- **Find & Replace** with regex support (Ctrl+H)
- **LSP integration** — Autocomplete (Ctrl+Space), diagnostics with inline squiggle underlines
- **Code folding** and bracket matching
- **File explorer** with git status badges (modified, added, deleted, untracked)
- **Command palette** (Ctrl+P for file quick-open, Ctrl+G for goto line)
- **Tab management** with persistent open files and scroll positions
- **Persistent session** — Remember open tabs, panel layout, scroll positions across restarts

### AI Integration
- **Inline AI edit** (Ctrl+K): Select code, describe what you want, AI rewrites it in place with diff preview
- **Streaming chat panel**: Real-time responses from Claude, GPT-4, or local models
- **Ghost text completions**: Tab to accept AI suggestions (FIM fill-in-the-middle)
- **Terminal integration**: Agent runs shell commands, output streams into terminal
- **Inline diff approval**: Accept/reject AI suggestions per hunk
- **Agent history**: View previous AI runs and results

### Terminals & Tools
- **Terminal emulation**: Full PTY support with 256-color, scrollback buffer
- **Git panel**: View status, stage/unstage files, commit with editor
- **Problems panel**: LSP diagnostics with error/warning badges and counts
- **Search panel**: Workspace search with ripgrep, regex support, replace-in-files
- **Settings panel**: Theme, font size, AI provider configuration (persisted)

### Built-in Themes
MidnightBlue, Cyberpunk, Dracula, Tokyo Night, Material, Nord, Catppuccin, Solarized, Gruvbox, Monokai, One Dark, GitHub Light.

---

## Keybindings

| Action | Windows/Linux | macOS |
|--------|---------------|-------|
| Open file (Quick open) | Ctrl+P | Cmd+P |
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
# Clone repository
git clone https://github.com/phazeai/ide.git
cd ide

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
show_breadcrumbs = true
show_minimap = false
```

### Cloud Provider API Keys
```bash
# Set environment variables for cloud providers
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
└── ollama-rs/           # Custom fork with streaming support
```

All crates are optional except `phazeai-core`. Use what you need.

---

## Monetization & Pricing

### Free Forever (Self-Hosted)
- Full IDE with all features
- CLI with all slash commands
- Local models (Ollama, LM Studio) with unlimited inference
- Bring your own OpenAI/Claude/Groq keys
- No telemetry, no license servers, no data collection

### PhazeAI Cloud ($15/month)
- Hosted phaze-beast model (fine-tuned for coding)
- 500,000 tokens/month included
- Priority queue for faster inference
- One-click setup (no API keys needed)
- Same free IDE + CLI available

### Team ($35/seat/month)
- Everything in Cloud
- Shared conversation history
- Agent audit logs
- Shared workspace context
- Team Modelfile sharing

### Enterprise (Custom)
- On-premise deployment
- SSO/SAML authentication
- VPC model hosting
- SLA + dedicated support

**No account required** to use the IDE with local models or your own API keys.

---

## Contributing

PhazeAI is open-source and community-driven. We welcome:
- Bug reports and feature requests (GitHub Issues)
- Pull requests (fork → branch → PR)
- Documentation improvements
- New provider integrations
- Translations

**Getting started as a contributor:**
```bash
# 1. Fork and clone
git clone https://github.com/YOUR_USERNAME/ide.git
cd ide

# 2. Create a feature branch
git checkout -b feature/my-feature

# 3. Make changes and test
cargo test --workspace
cargo fmt --all
cargo clippy --workspace -- -D warnings

# 4. Commit and push
git commit -m "Add feature: X"
git push origin feature/my-feature

# 5. Open a pull request on GitHub
```

See [CONTRIBUTING.md](./CONTRIBUTING.md) for detailed guidelines.

---

## Roadmap

### Phase 2: Ship (Current - 4-6 weeks)
- Critical editor features (undo/redo, find & replace, completions)
- Session persistence (remember open tabs & layout)
- Terminal improvements (multiple tabs, scrollbar, cursor)
- Git panel (stage/unstage, inline diff)
- Panel resizing (drag dividers)
- Testing infrastructure (snapshot tests, integration tests)
- Release packaging (AppImage, DMG, MSI, crates.io)

### Phase 3: Growth
- Go-to-definition and symbol rename (LSP)
- Hover popups with type info and documentation
- Symbol outline panel (tree of functions/structs)
- Split editor views (Ctrl+\ for side-by-side)
- Format on save (rustfmt, prettier, black)
- Zen mode (F11 for distraction-free coding)
- Breadcrumb navigation
- Minimap

### Phase 4: Scale (6+ months)
- Real-time collaboration (CRDT sync)
- Remote SSH development
- Integrated debugger (DAP)
- Plugin system (WASM sandbox)
- Browser-based version (Floem WASM)
- Voice control (Whisper model)
- Mobile companion app

See [TODO.md](./TODO.md) for detailed tracking by phase and block.

---

## License

PhazeAI IDE is licensed under the **MIT License**. See [LICENSE](./LICENSE) for details.

All dependencies are either MIT or Apache-2.0 licensed (verified on each release).

---

## FAQ

**Q: Is PhazeAI production-ready?**
A: We're in active development (Phase 2). The IDE is usable daily for coding, but some advanced features are still coming. CLI is production-ready.

**Q: Can I use my own OpenAI API key?**
A: Yes. Set `OPENAI_API_KEY` env var and configure the provider in settings.

**Q: Does PhazeAI collect data?**
A: No. Core IDE has zero telemetry. Cloud tier stores conversation history on encrypted servers only.

**Q: Why Rust instead of Electron?**
A: Rust gives us native performance (3-5x faster startup) without the 400MB memory footprint. GPU acceleration makes the UI instant.

**Q: Can I run PhazeAI offline?**
A: Yes. Use with Ollama or LM Studio. Cloud tier requires internet for model inference only.

**Q: What about VS Code extensions?**
A: Not supported yet, but exploring plugin systems for Phase 4.

**Q: How do I report a bug?**
A: Open a GitHub Issue with reproduction steps. Include `rustc --version`, `cargo --version`, and your OS.

---

## Community

- **Discord**: [Join our server](https://discord.gg/phazeai) for live chat and support
- **GitHub Issues**: Bug reports and feature requests
- **GitHub Discussions**: Ask questions and share ideas
- **Twitter**: [@phazeai](https://twitter.com/phazeai) for updates

---

**Built with love by the PhazeAI team and community.**

Have feedback? Open an issue or join our [Discord](https://discord.gg/phazeai).
