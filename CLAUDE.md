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

Cargo workspace:

- **`phazeai-ui`** — PRIMARY desktop IDE, GPU-accelerated via Floem (Vello/wgpu renderer)
- **`phazeai-core`** — shared engine: agent loop, LLM clients, tools, LSP, DAP, config, privacy policy, built-in local AI
- **`phazeai-cli`** — terminal UI (`ratatui`)
- **`phazeai-sidecar`** — Python semantic search subprocess
- **`phazeai-plugin-api`** / **`phazeai-plugin-canary`** — native plugin ABI + test plugin
- **`ollama-rs`** — local fork with custom streaming/chat-history features

> **NOTE**: `phazeai-ide` (the old egui/eframe GUI) has been **permanently deleted**.
> All GUI work is in `phazeai-ui` (Floem). Never reference egui or eframe.
> `phazeai-cloud` and the Account panel were removed (no backend exists);
> the multi-agent orchestrator was removed (no callers) — both live in git history.

Config is stored at `~/.config/phazeai/config.toml` (auto-created on first run).
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

`IdeState` (in `domain_state/mod.rs`) is a `#[derive(Clone)]` struct of `RwSignal<T>` fields shared across all panels via Floem's reactive system. No `Arc<Mutex<>>` needed — signals are `Copy` and UI-thread-only.

Panels: `editor`, `chat`, `explorer`, `git`, `terminal`, `search`, `settings`, `design`, `ai_panel`.

Key files:
- `domain_state/mod.rs` — `IdeState` decomposition (`WorkbenchState`, `EditorState`, `AiState`, `ProjectState`)
- `app.rs` — overlay views (command palette, file picker, completion popup, Ctrl+K inline edit), key handler, `launch_phaze_ide()`
- `panels/editor.rs` — multi-tab code editor with syntect highlighting, LSP, find/replace, reactive font-size
- `panels/terminal.rs` — PTY via `portable-pty` + VTE parser, 256-color rendering, clipboard
- `panels/chat.rs` — AI chat with real streaming via `Agent::run_with_events()`
- `panels/git.rs` — git status, stage/commit UI
- `lsp_bridge.rs` — LSP manager, completions, diagnostics signals
- `theme.rs` — 12 themes, `PhazeTheme` / `PhazePalette`; `custom_theme.rs` loads user themes from `~/.config/phazeai/themes/*.toml`; `tokens.rs` holds the spacing/radius/type scales (use them instead of literal sizes)
- `panels/design.rs` + `phazeai-core/src/design.rs` — AI design: prompt → self-contained HTML prototype in `.phazeai/designs/`, guided by `.phazeai/design-system.md`
- `panels/privacy.rs` — Settings › Privacy (policy, local-only toggle, audit log); `panels/local_ai.rs` — Settings › Local AI (hardware, catalog, download, server)

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

Injected into the agent at construction. `tools/approval.rs` classifies calls
(ReadOnly / Write / Execute / Destructive); a bash command is read-only only
when every `;|&`-separated part is a bare whitelisted program with no
writing/executing flags — extend the allowlists there, with a test, never by
substring matching. CLI `-p` mode uses `--allow read|edit|all` (`oneshot.rs`):
`edit` permits file tools only; any bash call above read-only needs `all`.

### Privacy (`phazeai-core/src/privacy.rs`)

`.phazeai/privacy.toml` (`deny` globs + `local_only`) plus a built-in secrets
list decide what may reach an AI provider. `privacy::active()` is the
process-wide policy (reloaded on file change); every tool, `@mention`, editor
action, FIM request, design job and git-diff prompt checks it, and every LLM
client is wrapped in `GuardedClient`, which enforces `local_only` per request
and records the audit log (`privacy::audit`, shown in Settings › Privacy). The
policy file itself is protected from mutating tools. New code that sends
project content to a model must go through `PrivacyPolicy::check` and note
attached files in the audit scope.

### Context compaction & budgets (`phazeai-core/src/context/`)

`Agent::run_with_events` calls `compaction::maybe_compact` before each model
call: past 80% of the budget, the oldest turns (never splitting a tool call
from its results, always leaving a user turn first) are folded into a
model-written summary kept in the system prompt. The budget comes from
`Settings::context_budget()` (`llm/context_budget.rs`: model window minus
reply/schema reserve; `[llm] context_tokens` overrides). The IDE chat carries
the summary across sends (`CompactionState` in `panels/chat.rs`).

### Built-in local AI (`phazeai-core/src/local_ai/`)

`ProviderId::BuiltIn` runs llama.cpp's `llama-server` as a child process on a
verified-download GGUF from a curated catalog (`catalog.rs`: exact size +
SHA-256 + license per entry, all Apache-2.0). `hardware.rs` probes RAM/disk,
`download.rs` resumes and verifies, `install.rs` fetches llama.cpp releases
(digest-verified; refuses without one), `server.rs` finds/starts/stops the
server. Config lives in `[local_ai]`; UI in `panels/local_ai.rs`.

### DAP Debugger (`phazeai-core/src/dap/` + `phazeai-ui/src/debug_session.rs`)

`DapClient` spawns a debug adapter (lldb-dap / lldb-vscode / `gdb -i dap`) and
speaks Content-Length-framed DAP over stdio. `debug_session::run_session` runs
on its own thread, takes `DebugCmd`s from the UI and streams `SessionUpdate`s
back through a sync_channel → `create_signal_from_channel`. Editor: F9 toggles
breakpoints (red gutter dots), stopped line gets a full-width highlight.
F5/F10/F11/⇧F11 = continue/step-over/step-into/step-out.

## Key Dependencies

| Purpose | Crate |
|---------|-------|
| Async runtime | `tokio` (full features) |
| GUI | `floem` (rev `e0dd862`, Lapce fork) |
| TUI | `ratatui 0.29` |
| PTY | `portable-pty` |
| Terminal parsing | `vte` |
| Syntax highlighting | `syntect` |
| LSP | `lsp-types 0.97` |
| HTTP | `reqwest 0.12` (json + stream) |
| Serialization | `serde` / `serde_json` |
| Clipboard | `arboard` |
| File dialog | `rfd` |
| Error types | `thiserror` (libs), `anyhow` (binaries) |
