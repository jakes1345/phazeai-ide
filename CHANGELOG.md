# Changelog

All notable changes to PhazeAI IDE will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-04-01

### Added

#### Desktop IDE (`phazeai-ui`)
- GPU-accelerated editor via Floem (Vello/wgpu renderer)
- Multi-tab editing with persistent sessions across restarts
- Syntax highlighting for 25+ languages via syntect
- Multi-cursor editing (Ctrl+D, Alt+Click)
- Vim mode with full motion support (normal, insert, visual, ex commands)
- Find & Replace with regex support and highlight-all
- Code folding, bracket pair colorization, indent guides
- Matching bracket highlighting and auto-close (brackets, quotes)
- Minimap with viewport indicator
- Sticky scroll headers for enclosing scope
- Split editor (vertical and horizontal splits)
- Column/box selection (Ctrl+Alt+Up/Down)
- AI chat panel with real streaming via agent loop
- AI Composer panel for multi-file agent tasks
- Terminal panel with PTY, 256-color rendering, OSC 7, named tabs
- File explorer with git status indicators, file watcher
- Git panel with staging, commit, blame, stash, merge, cherry-pick, tags
- Search panel with tree view results and keyboard navigation
- Settings panel for theme, font, AI provider configuration
- Extensions panel for native Rust plugins
- GitHub Actions panel for CI status monitoring
- LSP integration (completions, diagnostics, hover, rename, signature help, inlay hints, workspace symbols)
- Command palette (Ctrl+Shift+P)
- File picker (Ctrl+P)
- Breadcrumbs navigation
- 12 themes: Midnight Blue, Dark, Light, Cyberpunk, Synthwave84, Andromeda, Dracula, Tokyo Night, Monokai, Nord, Matrix Green, Root Shell
- Zen mode (Ctrl+Shift+Z)
- Auto-save with debounce
- Whitespace rendering toggle
- Transform case, join lines, sort lines commands
- Context menu with AI actions (Explain, Generate Tests, Fix)
- AI code review from git panel
- Conversation history persistence
- Token usage and cost tracking
- MCP (Model Context Protocol) server configuration

#### Terminal UI (`phazeai-cli`)
- Interactive AI chat with markdown rendering
- Syntax-highlighted code blocks with line numbers
- Tool approval UI with allow/deny/allow-all/session modes
- File tree sidebar (Ctrl+B)
- Session persistence and conversation management
- 7 themes: Dark, Tokyo Night, Dracula, Catppuccin, Gruvbox, Nord, One Dark
- Branded header bar with provider/model/mode display
- Professional status bar with token usage and cost
- Slash commands: /help, /model, /provider, /theme, /mode, /diff, /git, /grep, /compact, and more
- External editor support (Ctrl+E, uses $EDITOR)
- Input history with Up/Down navigation
- Clipboard support (Ctrl+V)
- Tab completion for commands
- AI mode switching (chat, ask, debug, plan, edit)
- Custom skills via .phazeai/commands/
- GitHub Action installer (/install-github-action)

#### Core Engine (`phazeai-core`)
- Streaming agentic loop with tool execution
- LLM provider system: Anthropic Claude, OpenAI, Groq, Together.ai, OpenRouter, Gemini, Ollama, LM Studio
- Tool system: read_file, write_file, edit_file, bash, grep, glob, list_files, memory
- Tool approval callbacks with configurable modes
- Agent cancellation via cancel token
- Token usage tracking per request
- Context auto-compaction at 80% budget
- Multi-agent orchestrator (Planner, Coder, Reviewer pipeline with self-healing)
- LSP client with multi-language server support
- MCP bridge for external tool servers
- System prompt builder with project context injection
- Git info collection for context
- Conversation persistence store
- Model router for per-task provider selection
- Local model auto-discovery

#### Cloud Client (`phazeai-cloud`)
- Authentication with PhazeAI Cloud
- Subscription tiers: SelfHosted, Cloud, Team, Enterprise
- Proxied LLM API via OpenAI-compatible client

#### Sidecar (`phazeai-sidecar`)
- Python subprocess management for semantic search
- JSON-RPC 2.0 protocol over stdio
- SemanticSearchTool and BuildIndexTool for agent integration

#### Plugin System (`phazeai-plugin-api`)
- Stable ABI for native Rust plugins
- PhazePlugin and PluginHost traits
- declare_plugin! macro for FFI entry points
- Event-driven lifecycle (FileOpened, FileSaved, CursorMoved, etc.)

#### Infrastructure
- 12-job CI pipeline (format, clippy, audit, per-crate tests, build gate)
- MIT license
- Comprehensive README with feature list, keybindings, provider setup
- Contributing guidelines

[0.1.0]: https://github.com/jakes1345/phazeai-ide/releases/tag/v0.1.0
