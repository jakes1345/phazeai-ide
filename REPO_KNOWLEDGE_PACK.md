# Repo Knowledge Pack

This file is the consolidated first-party understanding pass for `/app`.

Related docs created in this session:

- `CODEBASE_INDEX.md`
- `CORE_UI_ARCHITECTURE_MAP.md`

This document adds:

- first-party subsystem atlas
- runtime/service ownership summary
- test surface summary
- maturity and risk notes
- practical navigation guidance

Indexed on 2026-04-27.

## Scope Of This Understanding Pass

This pass focused on the first-party product repository, especially:

- Rust workspace crates
- UI and engine interactions
- CLI, sidecar, cloud, extension host
- Python sidecar/training surfaces
- CI/test coverage
- New `phazeai-plugin-canary` crate for plugin ABI testing

It intentionally deprioritized:

- `phazeai-arsenal/`
- `_archive/`
- `target/`

Those areas are either vendored/reference or generated/historical.

## First-Party Subsystem Atlas

### 1. Desktop IDE

Primary files:

- `crates/phazeai-ui/src/app.rs`
- `crates/phazeai-ui/src/panels/editor.rs`
- `crates/phazeai-ui/src/lsp_bridge.rs`
- `crates/phazeai-ui/src/panels/terminal.rs`
- `crates/phazeai-ui/src/panels/git.rs`

Role:

- Main desktop product shell
- Editor, terminal, chat, git, explorer, search, settings, extensions
- Owns global UI state and bootstraps background services

### 2. Shared Engine

Primary files:

- `crates/phazeai-core/src/agent/core.rs`
- `crates/phazeai-core/src/tools/traits.rs`
- `crates/phazeai-core/src/llm/provider.rs`
- `crates/phazeai-core/src/lsp/*`
- `crates/phazeai-core/src/mcp.rs`
- `crates/phazeai-core/src/context/*`

Role:

- Agent runtime
- Tools
- Provider/model selection
- Context and persistence
- LSP and MCP support
- Plugin host support

### 3. Terminal CLI

Primary files:

- `crates/phazeai-cli/src/main.rs`
- `crates/phazeai-cli/src/app.rs`
- `crates/phazeai-cli/src/commands.rs`
- `crates/phazeai-cli/src/theme.rs`

Role:

- Interactive TUI for the same core agent engine
- Single-prompt execution path
- Conversation persistence and command handling

### 4. Python Sidecar Integration

Rust side:

- `crates/phazeai-sidecar/src/client.rs`
- `crates/phazeai-sidecar/src/manager.rs`
- `crates/phazeai-sidecar/src/tool.rs`
- `crates/phazeai-sidecar/src/protocol.rs`

Python side:

- `sidecar/server.py`
- `sidecar/test_server.py`

Role:

- Separate JSON-RPC process for code indexing/search/analysis
- Exposed to the core agent as tools

### 5. Cloud Client

Primary files:

- `crates/phazeai-cloud/src/auth.rs`
- `crates/phazeai-cloud/src/client.rs`
- `crates/phazeai-cloud/src/subscription.rs`

Role:

- Credentials persistence
- Account validation
- Tier metadata
- Hosted model endpoint URL construction

### 6. Extension Host

Rust side:

- `crates/phazeai-core/src/ext_host/*`

Node side:

- `ext-host/src/main.js`
- `ext-host/src/extension-loader.js`
- `ext-host/src/rpc.js`
- `ext-host/src/vscode-shim.js`

Role:

- Two related ideas live here:
  - Native plugin loading via Rust shared libraries
  - JS/VS Code-style extension loading through a Node host

### 7. Training / Model Ops

Top-level:

- `training/prepare_data.py`
- `training/prepare_tool_data.py`
- `training/fine_tune.py`
- `training/export_gguf.py`

Expanded toolkit:

- `python/training/*`

Role:

- Data collection
- Fine-tuning
- GGUF export
- model experiments

## Maturity Read

The IDE and Core engine remain the most mature areas. The plugin system is evolving, with the canary plugin now in place to verify the ABI. The Python sidecar provides essential indexing but remains keyword-oriented (TF-IDF).

## Runtime Service Ownership

### Services booted by `phazeai-ui`

`IdeState::new()` in `crates/phazeai-ui/src/app.rs` is effectively responsible for:

- workspace detection
- session restoration
- editor settings loading
- LSP bridge startup
- active-file LSP wiring
- sidecar startup attempt
- extension manager creation
- settings persistence effects

### Services booted by the CLI

`crates/phazeai-cli/src/main.rs` and `crates/phazeai-cli/src/app.rs` are responsible for:

- settings load
- provider/model overrides
- single-prompt vs interactive mode selection
- agent runtime construction
- conversation persistence
- tool approval flow
- optional sidecar boot

## Tests: What Is Covered

### `phazeai-core` tests

Covered areas include:

- agent loop behavior
- config/settings
- conversation history
- system prompt behavior
- git helpers
- tool behavior
- extension host basics
- editor-oriented pure logic helpers
- stress and verification scenarios

### `phazeai-ui` tests

Covered areas include:

- LSP behavior
- terminal behavior
- state transitions
- integration scenarios
- snapshot/UI tests
- ignored/full GUI tests using X11 tooling

### `phazeai-cli` tests

Covered areas include:

- slash command parsing
- command behavior/results

### `phazeai-sidecar` tests

Covered areas include:

- JSON-RPC request/response behavior
- manager lifecycle
- protocol semantics

## Practical Navigation Map

### If the bug is “AI behaved oddly”

Read in this order:

1. `crates/phazeai-core/src/agent/core.rs`
2. `crates/phazeai-core/src/tools/traits.rs`
3. relevant tool file under `crates/phazeai-core/src/tools/`
4. `crates/phazeai-core/src/llm/provider.rs`
5. `crates/phazeai-core/src/llm/model_router.rs`
6. panel that invoked the agent (`chat.rs`, `composer.rs`, `git.rs`, `ai_panel.rs`)

### If the bug is “editor / autocomplete / diagnostics”

Read in this order:

1. `crates/phazeai-ui/src/app.rs`
2. `crates/phazeai-ui/src/lsp_bridge.rs`
3. `crates/phazeai-core/src/lsp/manager.rs`
4. `crates/phazeai-core/src/lsp/client.rs`
5. `crates/phazeai-ui/src/panels/editor.rs`

### If the bug is “sidecar / semantic search”

Read in this order:

1. `crates/phazeai-ui/src/app.rs`
2. `crates/phazeai-sidecar/src/manager.rs`
3. `crates/phazeai-sidecar/src/client.rs`
4. `crates/phazeai-sidecar/src/tool.rs`
5. `sidecar/server.py`

### If the bug is “CLI behavior”

Read in this order:

1. `crates/phazeai-cli/src/main.rs`
2. `crates/phazeai-cli/src/app.rs`
3. `crates/phazeai-cli/src/commands.rs`

### If the bug is “extensions / themes / VSIX”

Read in this order:

1. `crates/phazeai-core/src/ext_host/mod.rs`
2. `crates/phazeai-core/src/ext_host/asset_loader.rs`
3. `crates/phazeai-core/src/ext_host/registry.rs`
4. `crates/phazeai-ui/src/panels/extensions.rs`
5. `ext-host/src/*` only if the JS path is involved
