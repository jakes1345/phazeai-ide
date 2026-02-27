# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
# Build
cargo build --workspace            # debug build — all crates
cargo build --release --workspace  # release build
cargo build -p phazeai-ui          # Floem GUI only (primary IDE)
cargo build -p phazeai-cli         # ratatui TUI only

# Run
cargo run -p phazeai-ui            # launch Floem desktop IDE  ← PRIMARY
cargo run -p phazeai-cli           # launch ratatui terminal UI

# Test
cargo test --workspace             # all tests
cargo test -p phazeai-core         # core engine tests
cargo test -p phazeai-ui           # UI state + snapshot tests

# Lint & Format
cargo clippy --workspace -- -D warnings
cargo fmt --all
```

## Workspace Structure

6-crate Cargo workspace:

- **`phazeai-ui`** — PRIMARY desktop IDE, GPU-accelerated via Floem (Vello/wgpu renderer)
- **`phazeai-core`** — shared engine: agent loop, LLM clients, tools, LSP, config
- **`phazeai-cli`** — terminal UI (`ratatui`)
- **`phazeai-cloud`** — paid cloud client: auth, hosted models, team features (skeleton)
- **`phazeai-sidecar`** — Python semantic search subprocess
- **`ollama-rs`** — local fork with custom streaming/chat-history features

> **NOTE**: `phazeai-ide` (the old egui/eframe GUI) has been **permanently deleted**.
> All GUI work is in `phazeai-ui` (Floem). Never reference egui or eframe.

Config is stored at `~/.config/phazeai/settings.toml` (auto-created on first run).
Session (open files, panel sizes) at `~/.config/phazeai/session.toml`.

## Core Architecture

### Agent Loop (`phazeai-core/src/agent/core.rs`)

The central execution model is a streaming agentic loop:

1. Send user message to LLM, receive streaming tokens
2. Parse tool calls from the stream
3. Invoke approval callback (user-facing confirmation or auto-approve)
4. Execute tool, append result to conversation history
5. Repeat until LLM returns a response with no tool calls

`run_with_events()` emits `AgentEvent` variants (`TextDelta`, `ToolCall`, `ToolResult`, `Complete`, `Error`) over a `tokio::sync::mpsc` channel for real-time UI updates.

### LLM Provider System (`phazeai-core/src/llm/`)

- `LlmClient` trait (`traits.rs`) — all providers implement this async streaming interface
- `ProviderRegistry` (`provider.rs`) — registry of `ProviderConfig` structs keyed by `ProviderId`
- All cloud providers except Claude use `OpenAIClient` (OpenAI-compatible API)
- `ModelRouter` (`model_router.rs`) — selects provider/model per task type
- Local model auto-discovery via `discovery.rs`

Supported providers: Anthropic Claude, OpenAI, Groq, Together.ai, OpenRouter, Gemini, Ollama (local), LM Studio (local).

### IDE Architecture (`phazeai-ui/src/`)

`IdeState` (in `app.rs`) is a `#[derive(Clone)]` struct of `RwSignal<T>` fields shared across all panels via Floem's reactive system. No `Arc<Mutex<>>` needed — signals are `Copy` and UI-thread-only.

Panels: `editor`, `chat`, `explorer`, `git`, `terminal`, `search`, `settings`, `ai_panel`.

Key files:
- `app.rs` — `IdeState`, all overlay views (command palette, file picker, completion popup, Ctrl+K inline edit), key handler, `launch_phaze_ide()`
- `panels/editor.rs` — multi-tab code editor with syntect highlighting, LSP, find/replace, reactive font-size
- `panels/terminal.rs` — PTY via `portable-pty` + VTE parser, 256-color rendering, clipboard
- `panels/chat.rs` — AI chat with real streaming via `Agent::run_with_events()`
- `panels/git.rs` — git status, stage/commit UI
- `lsp_bridge.rs` — LSP manager, completions, diagnostics signals
- `theme.rs` — 12 themes, `PhazeTheme` / `PhazePalette`

### Floem Reactive Patterns (CRITICAL)

```rust
// Async → UI signal update: use create_ext_action, NOT tokio::spawn signal writes
use floem::reactive::{create_ext_action, Scope};
let scope = Scope::new();
let send = create_ext_action(scope, move |result| { signal.set(result); });
std::thread::spawn(move || { send(do_work()); });

// Channel → signal
let (tx, rx) = std::sync::mpsc::sync_channel(64);
let sig = create_signal_from_channel(rx);  // reads in create_effect

// AI streaming (chat.rs pattern)
std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let agent = Agent::new(client);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
        let _ = tokio::join!(agent.run_with_events(&prompt, tx), async {
            while let Some(event) = rx.recv().await { /* handle */ }
        });
    });
});
```

### Floem API Notes (validated)

- `Color::from_rgb8(r,g,b)` / `Color::from_rgba8(r,g,b,a)` — NOT `Color::rgb8()`
- `.with_alpha(f32)` — NOT `.multiply_alpha()`
- `stack()` for fixed tuples, `dyn_stack(items_fn, key_fn, view_fn)` for `Vec`
- `on_event_stop` uses bubble phase (leaf → root); text_editor handles keys before outer containers
- `editor.update_styling(Rc::new(new_style))` — live font-size update without recreating editor
- `SyncSender<T>` is `Clone + Send` — never wrap in `Rc` when passing to threads
- Import `SignalGet` and `SignalUpdate` explicitly from `floem::reactive`
- No `.bottom()` / `.left()` on Style — use `.inset_bottom()` / `.inset_left()`
- `Renderer` trait must be imported for `cx.fill()` in canvas views

### Tool Approval

```rust
pub type ApprovalFn = Box<dyn Fn(String, Value) -> Pin<Box<dyn Future<Output = bool>>> + Send + Sync>;
```

Injected into the agent at construction.

### Multi-Agent (`phazeai-core/src/agent/multi_agent.rs`)

Planner → Coder → Reviewer pipeline, each backed by independently configured `LlmClient` instances.

## Key Dependencies

| Purpose | Crate |
|---------|-------|
| Async runtime | `tokio` (full features) |
| GUI | `floem` (rev `e0dd862`, Lapce fork) |
| TUI | `ratatui 0.28` |
| PTY | `portable-pty` |
| Terminal parsing | `vte` |
| Syntax highlighting | `syntect` |
| LSP | `lsp-types 0.97` |
| HTTP | `reqwest 0.12` (json + stream) |
| Serialization | `serde` / `serde_json` |
| Clipboard | `arboard` |
| File dialog | `rfd` |
| Error types | `thiserror` (libs), `anyhow` (binaries) |
