# phazeai-core

The brain of PhazeAI IDE — a shared Rust engine providing agentic AI capabilities, multi-provider LLM support, tool execution, LSP integration, and project analysis.

## Features

- **Agent Loop**: Streaming agentic execution with tool calling, approval callbacks, and real-time event emission
- **LLM Provider System**: 8 supported providers — Anthropic Claude, OpenAI, Groq, Together.ai, OpenRouter, Google Gemini, Ollama (local), LM Studio (local)
- **Tool Registry**: Built-in tools for file operations, bash execution, git, grep, glob, semantic search, and more
- **LSP Client**: Language Server Protocol integration with completions, diagnostics, symbol navigation, rename, definition lookup
- **MCP Bridge**: Model Context Protocol support for extended tool capabilities
- **Multi-Agent Orchestrator**: Planner → Coder → Reviewer pipeline with role-based model routing
- **Project Analysis**: Repo map generation, git context collection, project type detection, conversation persistence
- **Configuration**: TOML-based settings at `~/.config/phazeai/settings.toml` with provider keys, model defaults, and feature flags

## Usage

This is a library crate — it is a dependency of both `phazeai-ui` (desktop IDE) and `phazeai-cli` (terminal UI).

### Core API Example

```rust
use phazeai_core::{Agent, ProviderRegistry, Settings};

// Load settings and initialize provider
let settings = Settings::load()?;
let registry = ProviderRegistry::from_settings(&settings);

// Create and run an agent
let client = registry.get_client("claude")?;
let agent = Agent::new(client);

// Stream events in real-time
let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
tokio::spawn(async move {
    agent.run_with_events("Explain this code", tx).await
});

while let Some(event) = rx.recv().await {
    println!("{:?}", event);  // TextDelta, ToolCall, ToolResult, Complete
}
```

## Modules

- `agent/` — Agent loop, event streaming, tool approval
- `llm/` — LLM clients, provider registry, model routing
- `lsp/` — LSP client and manager
- `mcp/` — Model Context Protocol bridge
- `tools/` — Tool registry and definitions
- `config/` — Settings loading and management
- `context/` — Project context, conversation history, repo analysis
- `git/` — Git operations and blame parsing
- `ext_host/` — Native plugin loading via libloading

## License

MIT — See LICENSE in repository root
