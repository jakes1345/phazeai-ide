# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
# Build
cargo build --workspace          # debug build
cargo build --release --workspace  # release build
cargo build -p phazeai-ide       # single crate

# Run
cargo run -p phazeai-ide         # desktop GUI
cargo run -p phazeai-cli         # terminal UI

# Test
cargo test --workspace           # all tests
cargo test -p phazeai-core       # single crate
cargo test agent::tests          # single test module

# Lint & Format
cargo clippy --workspace -- -D warnings
cargo fmt --all
```

## Workspace Structure

5-crate Cargo workspace:

- **`phazeai-core`** — shared engine used by all interfaces: agent loop, LLM clients, tools, LSP, context, config
- **`phazeai-ide`** — desktop GUI (`egui`/`eframe`), 6 panels
- **`phazeai-cli`** — terminal UI (`ratatui`)
- **`phazeai-sidecar`** — Python semantic search subprocess
- **`ollama-rs`** — local fork of `ollama-rs` with custom streaming/chat-history features

Config is stored at `~/.config/phazeai/settings.toml` (auto-created on first run).

## Core Architecture

### Agent Loop (`phazeai-core/src/agent/core.rs`)

The central execution model is a streaming agentic loop:

1. Send user message to LLM, receive streaming tokens
2. Parse tool calls from the stream
3. Invoke approval callback (can be user-facing confirmation UI or auto-approve)
4. Execute tool, append result to conversation history
5. Repeat until LLM returns a response with no tool calls

`run_with_events()` emits `AgentEvent` variants (`TextDelta`, `ToolCall`, `ToolResult`, etc.) over a `tokio::sync::mpsc` channel for real-time UI updates.

### LLM Provider System (`phazeai-core/src/llm/`)

- `LlmClient` trait (in `traits.rs`) — all providers implement this async streaming interface
- `ProviderRegistry` (in `provider.rs`) — registry of `ProviderConfig` structs keyed by `ProviderId`
- All cloud providers except Claude use `OpenAIClient` (OpenAI-compatible API)
- `ModelRouter` (in `model_router.rs`) — selects the best provider/model per task type (planner, coder, reviewer)
- Local model auto-discovery via `discovery.rs`

Supported providers: Anthropic Claude, OpenAI, Groq, Together.ai, OpenRouter, Ollama (local), LM Studio (local).

### IDE Panel Architecture (`phazeai-ide/src/`)

`PhazeApp` (in `app.rs`) owns all panel state. Each panel (`editor`, `chat`, `explorer`, `terminal`, `browser`, `settings`) exposes a `show()` method taking egui context. Panels communicate through shared state on `PhazeApp` — panels emit requests, `app.rs` processes them in the event loop.

The editor uses `ropey` for efficient text buffers. The terminal panel uses `portable-pty` for cross-platform PTY support.

### Tool Approval

```rust
pub type ApprovalFn = Box<dyn Fn(String, Value) -> Pin<Box<dyn Future<Output = bool>>> + Send + Sync>;
```

Injected into the agent at construction. The IDE uses this to show confirmation UI before destructive tool calls.

### Multi-Agent (`phazeai-core/src/agent/multi_agent.rs`)

Planner → Coder → Reviewer pipeline, each backed by independently configured `LlmClient` instances selected by `ModelRouter`.

## Key Dependencies

| Purpose | Crate |
|---------|-------|
| Async runtime | `tokio` (full features) |
| GUI | `egui 0.28` / `eframe 0.28` |
| TUI | `ratatui 0.28` |
| Text buffer | `ropey` |
| PTY | `portable-pty` |
| AST / LSP | `tree-sitter 0.24`, `lsp-types 0.97` |
| HTTP | `reqwest 0.12` (json + stream) |
| Serialization | `serde` / `serde_json` |
| Error types | `thiserror` (in library code), `anyhow` (in binaries) |
