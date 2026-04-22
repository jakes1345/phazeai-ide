# PhazeAI IDE - Comprehensive Project Analysis

*Synthesized from a deep dive across all workspace crates and architectural components.*

## Overview
PhazeAI IDE is an AI-native code editor written in Rust. It differentiates itself by:
1. **Avoiding Electron/Web tech**: It uses a GPU-accelerated Rust UI toolkit called Floem.
2. **Local-First AI**: Native support for Ollama and LM Studio to run LLMs locally without API keys, alongside cloud providers like Anthropic and OpenAI.
3. **Multi-Agent Orchestration**: A built-in pipeline for autonomous Planning, Coding, and Reviewing.
4. **Native Performance**: Very fast startup and low memory footprint compared to traditional IDEs.

The workspace is divided into several layered crates:
- `phazeai-core`: The engine (Agent loop, LLM clients, Tool abstractions, LSP orchestration).
- `phazeai-ui`: The primary product (Floem-based desktop GUI).
- `phazeai-cli`: The terminal product (Ratatui-based TUI).
- `phazeai-sidecar`: Python-based semantic search subprocess.
- `phazeai-plugin-api` / `ext-host`: Native and WASM plugin hosting infrastructure.

---

## 1. The Core Engine (`phazeai-core`)

This crate is the heart of the project. It provides the abstractions that the UI and CLI consume.

### The Agent Loop
At the center is the `Agent` struct (`src/agent/core.rs`). It implements a streaming agentic loop:
1. **Context Management**: It maintains a `ConversationHistory`, automatically trimming older messages to fit within the `max_context_tokens` budget.
2. **Streaming Execution**: It calls `llm.chat_stream()` and processes `StreamEvent`s. Text deltas are emitted immediately to the UI.
3. **Tool Invocation**: If the LLM generates tool calls, the agent pauses the stream, requests user approval (via a configured `ApprovalFn`), executes the tools using the `ToolRegistry`, appends the results to the conversation, and loops back to the LLM.
4. **Completion**: The loop ends when the LLM returns a response with no tool calls.

### Multi-Agent Pipeline (`src/agent/multi_agent.rs`)
For complex tasks, PhazeAI provides a `MultiAgentOrchestrator` that implements a Planner → Coder → Reviewer pipeline.
Crucially, it includes a **Self-Healing Refinement Loop**: After the Coder generates code, the orchestrator runs a build check (e.g., `cargo check`, `tsc`). If it fails, the errors are fed back to the Coder for refinement before passing to the Reviewer.

### LLM Provider Abstraction
The `LlmClient` trait standardizes interactions across all providers.
The `ProviderRegistry` manages configurations for Claude, OpenAI, Ollama, Groq, Together, etc. 
A highly intelligent `ModelRouter` can classify user prompts (e.g., Reasoning, CodeGen, QuickAnswer) and route requests to specialized models if configured.

### Tool System
Tools implement the `Tool` async trait, which defines a JSON schema for parameters. 
The `ToolRegistry` groups tools into sets (e.g., `read_only`, `standard`, `default`). 
A `ToolApprovalManager` classifies tools into permission levels (ReadOnly, Write, Execute, Destructive) and supports modes like `AutoApprove`, `AlwaysAsk`, and `AskOnce`.

---

## 2. The Desktop IDE (`phazeai-ui`)

The UI is built on Floem, a reactive GUI framework for Rust. It uses a very large, centralized state object.

### State Management (`IdeState`)
`IdeState` (in `app.rs`) is a massive struct containing dozens of `RwSignal<T>` fields. Because Floem's signals are thread-local and reactive, this object can be cheaply cloned and passed around without heavy `Arc<Mutex<T>>` locking for UI updates.
It holds everything: active tabs, panel widths, LSP diagnostics, AI streaming state, and Vim mode flags.

### Reactive Async Bridging
Because Floem's UI updates must happen on the main thread, the project bridges background Tokio tasks using:
1. **`create_signal_from_channel`**: Reads from an `mpsc` channel and updates a signal automatically.
2. **`create_ext_action`**: Creates a thread-safe callback that can be invoked from a background task to safely mutate a signal on the UI thread.

### The Editor Panel
The editor (`src/panels/editor.rs`) is a massive 5000+ line custom implementation on top of Floem's text widget. It handles:
- Syntax highlighting via `syntect`
- Multi-cursor support, Vim mode, and Git gutter indicators
- LSP rendering (squiggle underlines, inlay hints, code lens)
- Inline AI edits (Ctrl+K) with diff previews

### LSP Integration
The `LspManager` runs in a background Tokio thread. `lsp_bridge.rs` adapts this by listening for UI commands (`LspCommand::OpenFile`, `RequestCompletions`), dispatching them to the background LSP process, and piping the JSON-RPC responses back into Floem `RwSignal`s (like `diagnostics` or `completions`). It also provides local regex-based fallbacks if an LSP server isn't available.

---

## 3. The Terminal UI (`phazeai-cli`)

For users who prefer the terminal, `phazeai-cli` offers a rich Ratatui-based interface.

### Architecture
It uses a standard `crossterm` + `ratatui` event loop.
The `AppState` struct holds input history, chat messages, scrolling state, and tool approval state.
It runs a background Tokio task for the `Agent`, communicating via `mpsc::unbounded_channel` for streaming events (TextDelta, ToolStart, etc.).

### Features
- **Slash Commands**: Supports over 40 commands (e.g., `/model`, `/theme`, `/diff`, `/yolo`).
- **AI Modes**: Specific instructions can be prepended using modes like `/plan`, `/debug`, or `/edit`.
- **Tool Approval**: Native TUI prompts (y/n/a/s) for intercepting dangerous tool executions.

---

## 4. Semantic Search Sidecar (`phazeai-sidecar`)

To enable fast codebase understanding without relying entirely on the LLM or LSP, PhazeAI uses a Python sidecar process.

### Implementation
It is a zero-dependency Python script (`sidecar/server.py`) that implements a custom **TF-IDF (Term Frequency-Inverse Document Frequency)** search index. It does not currently use vector embeddings, though the architecture supports adding them later.

### IPC Communication
The Rust crate (`phazeai-sidecar`) spawns the Python process and communicates using **JSON-RPC 2.0 over stdio** (newline-delimited JSON).
The Rust `SidecarClient` provides strongly-typed methods (`search_embeddings`, `build_index`) which are wrapped into agent `Tool`s (`SemanticSearchTool`, `BuildIndexTool`) so the LLM can query the codebase directly.

### Symbol Extraction
The Python sidecar also includes a `CodeAnalyzer` that uses regex heuristics to extract structural symbols (functions, classes, structs) across 15+ languages to build the context map.

---

## 5. Workspace and Configuration

### Configuration
Settings are loaded from `~/.config/phazeai/config.toml`. The `Settings` struct manages LLM API keys, editor preferences (theme, font size), and sidecar options. 

### Dependencies
The workspace heavily relies on standard Rust async ecosystem (`tokio`, `reqwest`, `serde`), but notably uses a custom fork of `ollama-rs` to add streaming and tool-calling support which the upstream crate lacked.

### Build and Deployment
The project uses a `Makefile` and standard Cargo commands. GitHub Actions handle CI/CD, producing cross-platform binaries, macOS DMGs, Windows MSIs, and Linux Flatpaks.
