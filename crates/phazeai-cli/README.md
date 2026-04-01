# phazeai-cli

A terminal UI for PhazeAI built with ratatui — brings powerful AI-assisted coding to any terminal, with zero external dependencies beyond Rust.

## Features

- **Interactive AI Chat**: Talk to Claude, GPT-4, Groq, and local models (Ollama, LM Studio) from your terminal
- **Markdown Rendering**: Beautiful markdown output with syntax-highlighted code blocks via syntect and pulldown-cmark
- **Tool Approval UI**: Approve file reads, writes, edits, bash commands, and more with an interactive prompt
- **Syntax Highlighting**: Code blocks highlighted per language — no raw HTML in the terminal
- **Session Persistence**: Conversations auto-saved to `~/.phazeai/conversations/` — resume anytime
- **File Tree View**: Browse workspace files, open in editor, explore project structure
- **7 Themes**: MidnightBlue, Cyberpunk, Dracula, Solarized Light, and more
- **Search History**: Cycle through previous queries with arrow keys in chat input
- **Model Selector**: Quick dropdown to switch between available LLM providers

## Build & Run

```bash
cargo build -p phazeai-cli           # debug build
cargo run -p phazeai-cli             # launch terminal UI
phazeai --help                       # show help
```

## Usage

Launch the terminal UI and start chatting:

```bash
cargo run -p phazeai-cli
# Or after install:
phazeai
```

The CLI uses the same `phazeai-core` agent loop as the desktop IDE — all tools, providers, and settings are shared.

## Architecture

- `main.rs` — App initialization, main event loop
- `app.rs` — TUI state and layout
- `commands.rs` — Command processing and dispatch
- `theme.rs` — 7 color themes
- `panels/` — Input, output, tools approval UI

## License

MIT — See LICENSE in repository root
