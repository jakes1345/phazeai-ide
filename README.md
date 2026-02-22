# PhazeAI IDE

**A full-featured, AI-native code editor built entirely in Rust. Local-first. No cloud required.**

[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.70%2B-orange?logo=rust)](https://rustup.rs)
[![Build](https://github.com/jakes1345/phazeai-ide/actions/workflows/ci.yml/badge.svg)](https://github.com/jakes1345/phazeai-ide/actions)

---

## What It Is

PhazeAI IDE is a desktop code editor built with [egui](https://github.com/emilk/egui) and backed by a full agentic AI engine. It runs entirely on your machine — plug in Ollama for local models, or wire up Claude/OpenAI/Groq for cloud. Everything else (editor, terminal, LSP, diff, search) is pure Rust.

**Not a VS Code extension. Not an Electron app. A real native binary.**

---

## Download

Pre-built binaries for Linux, macOS, and Windows are on the [Releases page](https://github.com/jakes1345/phazeai-ide/releases).

| Platform | Download |
|---|---|
| Linux x86_64 | `phazeai-linux-x86_64.tar.gz` |
| macOS Apple Silicon | `phazeai-macos-aarch64.tar.gz` |
| macOS Intel | `phazeai-macos-x86_64.tar.gz` |
| Windows x86_64 | `phazeai-windows-x86_64.zip` |

Or build from source (see below).

---

## Features

### Editor
- **Tree-sitter syntax highlighting** — Semantic coloring for Rust (24 token types). Syntect fallback for all other languages.
- **Rope-based text buffer** — O(log n) edits, efficient undo/redo
- **Multi-cursor editing** — Alt+Click, Ctrl+D select-next-occurrence, column selection
- **LSP integration** — Hover, go-to-definition (F12), find references (Shift+F12), autocomplete (Ctrl+Space), formatting (Shift+Alt+F), inline diagnostics
- **Code folding** — Gutter triangles, click to fold/unfold functions and blocks
- **Symbol outline** — Sidebar panel listing functions/classes/structs in the current file
- **Bracket matching** — Cursor-scan highlight of matching `()`, `[]`, `{}`
- **Find & Replace** — Ctrl+H panel, regex support, replace-in-files
- **Split editor** — Horizontal split with draggable separator
- **Minimap** — Downscaled file overview, click to scroll
- **Breadcrumb navigation** — File → Module → Function context bar
- **Git gutter** — Added/modified/deleted line decorations, inline blame on hover
- **Tab management** — Drag-to-reorder, middle-click to close, file watching

### Terminal
- **Full VTE emulator** — Real PTY, VT100/ANSI parsing, 256-color, truecolor
- **Multiple terminal tabs** — Each tab is an independent PTY session with its own shell
- **PTY resize** — Terminal dimensions follow the panel size in real time
- **Input history** — Up/Down arrow history, Ctrl+C / Ctrl+D / Ctrl+L / Tab passthrough
- **Scrollback** — 10,000-line buffer

### AI
- **Five AI modes** — Chat (general), Ask (current file), Debug (file + terminal output), Plan (project tree), Edit (inline with diff)
- **Inline chat** — Ctrl+K popup in the editor, streams AI response with diff highlighting
- **Per-hunk diff approval** — Agent file edits show a colored diff; Accept / Reject each hunk individually
- **Agent history** — View the last 20 agent runs
- **Cancel running agent** — Stop button in the chat panel
- **Bash → terminal** — Agent shell commands stream output directly into the terminal panel
- **Token usage meter** — Live token count in the status bar

### Workspace
- **Workspace search** — ripgrep-powered, regex, case-sensitive toggle, file glob filter, replace-in-files
- **Git diff viewer** — Unified and side-by-side diff panel
- **Git commit panel** — Stage files, write message, commit
- **Docs viewer** — In-app browser panel for documentation

### Other
- **Persistent state** — Panel sizes, open folder, AI mode, terminal height saved across sessions
- **Settings editor** — Searchable settings panel with keybindings reference (26 bindings)
- **Welcome screen** — Onboarding for new users
- **Command palette** — Quick access to all actions
- **Multiple themes** — Dark, Tokyo Night, Dracula (more coming)

---

## LLM Providers

| Provider | Type | Notes |
|---|---|---|
| [Ollama](https://ollama.ai) | Local | Qwen, Llama, Mistral, DeepSeek, etc. |
| [LM Studio](https://lmstudio.ai) | Local | Any GGUF model |
| [Anthropic Claude](https://anthropic.com) | Cloud | claude-sonnet-4-6, opus, etc. |
| [OpenAI](https://openai.com) | Cloud | gpt-4o, o3, etc. |
| [Groq](https://groq.com) | Cloud | Ultra-fast inference |
| [Together AI](https://together.ai) | Cloud | Open models |
| [OpenRouter](https://openrouter.ai) | Cloud | Any model via unified API |

---

## Build from Source

### Prerequisites

- **Rust 1.70+** — [rustup.rs](https://rustup.rs)
- **Linux**: `libxcb`, `libxkbcommon`, `libwayland`, `libgtk-3` dev packages
- **macOS / Windows**: no extra deps

```bash
# Linux dep install
sudo apt-get install -y \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libssl-dev libgtk-3-dev libwayland-dev libxdo-dev
```

### Build

```bash
git clone https://github.com/jakes1345/phazeai-ide.git
cd phazeai-ide

# GUI (desktop IDE)
cargo build --release -p phazeai-ide
./target/release/phazeai-ide

# Terminal UI
cargo build --release -p phazeai-cli
./target/release/phazeai
```

### Tests

```bash
cargo test --workspace       # all tests
cargo test -p phazeai-ide    # IDE tests only (12 integration tests)
```

---

## Configuration

Config is auto-created at `~/.config/phazeai/settings.toml` on first run.

```toml
[llm]
provider = "ollama"           # ollama, claude, openai, groq, together, openrouter, lmstudio
model = "qwen2.5-coder:14b"
api_key_env = "ANTHROPIC_API_KEY"   # only needed for cloud providers
max_tokens = 8192

[editor]
theme = "Dark"                # Dark, TokyoNight, Dracula
font_size = 14.0
tab_size = 4
show_line_numbers = true
```

**Cloud provider API keys** (set as env vars):

```bash
export ANTHROPIC_API_KEY="sk-ant-..."   # Claude
export OPENAI_API_KEY="sk-..."          # OpenAI
export GROQ_API_KEY="gsk_..."           # Groq
export TOGETHER_API_KEY="..."           # Together AI
export OPENROUTER_API_KEY="sk-or-..."   # OpenRouter
```

---

## Workspace Structure

```
phazeai-ide/
├── crates/
│   ├── phazeai-core/       # Agent loop, LLM clients, tools, LSP, context
│   ├── phazeai-ide/        # Desktop GUI (egui 0.28 / eframe 0.28)
│   ├── phazeai-cli/        # Terminal UI (ratatui 0.28)
│   ├── phazeai-sidecar/    # Python semantic search subprocess
│   └── ollama-rs/          # Custom fork of ollama-rs with streaming + chat history
└── Cargo.toml
```

### Key Dependencies

| Crate | Purpose |
|---|---|
| `egui 0.28` / `eframe 0.28` | Immediate-mode GUI |
| `ropey` | Efficient rope text buffer |
| `tree-sitter 0.24` + `tree-sitter-rust` | Syntax parsing + semantic highlighting |
| `syntect` | Highlighting fallback for non-Rust languages |
| `vte` | VT100/ANSI terminal emulator |
| `portable-pty` | Cross-platform PTY |
| `lsp-types` | Language Server Protocol types |
| `similar` | Diff algorithm for inline edit approval |
| `notify` | Filesystem watching |
| `arboard` | Cross-platform clipboard |
| `reqwest` | HTTP client for LLM API calls |

---

## Keyboard Shortcuts

| Shortcut | Action |
|---|---|
| Ctrl+P | Open file / command palette |
| Ctrl+K | Inline AI chat |
| Ctrl+H | Find & Replace |
| Ctrl+Space | LSP autocomplete |
| F12 | Go to definition |
| Shift+F12 | Find references |
| Shift+Alt+F | Format document |
| Ctrl+D | Select next occurrence |
| Alt+Click | Add cursor |
| Ctrl+Z / Ctrl+Y | Undo / Redo |
| Ctrl+` | Toggle terminal |
| Ctrl+W | Close tab |

---

## Releases

Releases are built automatically via GitHub Actions on every version tag (`v*`), producing binaries for:
- Linux x86_64 (`.tar.gz`)
- macOS Apple Silicon (`aarch64`, `.tar.gz`)
- macOS Intel (`x86_64`, `.tar.gz`)
- Windows x86_64 (`.zip`)

To trigger a release: push a tag like `git tag v0.2.0 && git push origin v0.2.0`.

---

## License

MIT — see [LICENSE](LICENSE).

## Contributing

PRs welcome. Please run `cargo fmt`, `cargo clippy`, and `cargo test` before submitting.
