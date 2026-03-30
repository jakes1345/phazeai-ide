# Repo Knowledge Pack

This file is the consolidated first-party understanding pass for `/home/jack/phazeai_ide`.

Related docs created in this session:

- `CODEBASE_INDEX.md`
- `CORE_UI_ARCHITECTURE_MAP.md`

This document adds:

- first-party subsystem atlas
- runtime/service ownership summary
- test surface summary
- maturity and risk notes
- practical navigation guidance

Indexed on 2026-03-24.

## Scope Of This Understanding Pass

This pass focused on the first-party product repository, especially:

- Rust workspace crates
- UI and engine interactions
- CLI, sidecar, cloud, extension host
- Python sidecar/training surfaces
- CI/test coverage

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

Current shape:

- Feature-rich
- Highly centralized
- Large files and high coupling

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

Current shape:

- Better-factored than UI
- Strong trait boundaries in a few places
- Still contains some overlapping “source understanding” responsibilities

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

What stands out:

- `app.rs` is large and featureful, not a tiny wrapper
- Implements slash commands, tool approvals, retries, modes, history, saved sessions
- Tries to start the sidecar and register semantic-search tools in single-prompt mode

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

What stands out:

- The protocol is simple and clean
- The Rust wrapper is narrow
- The Python server is stdlib-based and regex/TF-IDF oriented
- UI sidecar wiring looks lighter-weight and possibly less mature than the LSP path

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

What stands out:

- This crate is currently much smaller and narrower than the core/UI crates
- It looks like a support/client library rather than a deep subsystem

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

What stands out:

- The Rust side is more substantial and production-shaped
- The JS host looks more like a prototype/compatibility experiment
- `vscode-shim.js` is intentionally partial

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

What stands out:

- Two layers exist:
  - a small top-level training pipeline
  - a much larger `python/training/` experimentation area
- This part of the repo is important, but not on the main product runtime path

## Maturity Read

This repo has a clear asymmetry:

- The desktop IDE and core engine are the most developed product surfaces
- CLI is strong and feature-rich
- Sidecar/cloud/extensions are meaningful but less central
- Training surfaces are broad but adjacent to the main runtime product

Within the runtime product:

- `phazeai-core` feels like the most intentional architecture layer
- `phazeai-ui` feels like the most feature-accumulated layer

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

This means UI startup owns runtime composition more than a dedicated service layer does.

### Services booted by the CLI

`crates/phazeai-cli/src/main.rs` and `crates/phazeai-cli/src/app.rs` are responsible for:

- settings load
- provider/model overrides
- single-prompt vs interactive mode selection
- agent runtime construction
- conversation persistence
- tool approval flow
- optional sidecar boot

The CLI is architecturally closer to the engine than the GUI is, but still contains substantial orchestration logic.

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

Takeaway:

- Core logic has meaningful test investment
- Many tests are unit-style and helper-style
- Some verification is broad but not necessarily full runtime integration

### `phazeai-ui` tests

Covered areas include:

- LSP behavior
- terminal behavior
- state transitions
- integration scenarios
- snapshot/UI tests
- ignored/full GUI tests using X11 tooling

Takeaway:

- UI has both lightweight and heavyweight test modes
- Some tests mirror app logic in helper form rather than driving full live behavior
- There is real attempt at end-to-end GUI validation

### `phazeai-cli` tests

Covered areas include:

- slash command parsing
- command behavior/results

Takeaway:

- CLI command grammar is well covered
- broader TUI lifecycle behavior is less obvious from the test surface alone

### `phazeai-sidecar` tests

Covered areas include:

- JSON-RPC request/response behavior
- manager lifecycle
- protocol semantics

Takeaway:

- Protocol-level confidence exists
- true Rust-to-Python process integration is likely more lightly covered than the protocol layer itself

## Tests: What Looks More Synthetic

Several tests are valuable but are not full production-path verification:

- helper mirrors in `editor_tests.rs`
- mock LLM tests in `agent_tests.rs`
- state machine and parser-style tests
- GUI snapshot/state assertions without always exercising all live dependencies

That is not a criticism. It just means:

- the repo is better covered at logic seams than at full runtime behavior seams

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

## Key Hotspots By Ownership

### Hotspots that likely need the most care before refactoring

- `crates/phazeai-ui/src/app.rs`
- `crates/phazeai-ui/src/panels/editor.rs`
- `crates/phazeai-ui/src/panels/git.rs`
- `crates/phazeai-ui/src/lsp_bridge.rs`
- `crates/phazeai-cli/src/app.rs`

### Hotspots that are strategically important but cleaner

- `crates/phazeai-core/src/agent/core.rs`
- `crates/phazeai-core/src/tools/traits.rs`
- `crates/phazeai-core/src/llm/provider.rs`
- `crates/phazeai-core/src/lsp/manager.rs`
- `crates/phazeai-sidecar/src/client.rs`

## Things That Look Incomplete Or Transitional

These are the strongest signs of “active evolution” rather than fully-settled architecture:

- `phazeai-ui` sidecar integration path appears lighter than the main LSP path
- `ext-host/src/*` looks experimental compared with the Rust ext-host layer
- duplicate or overlapping source-analysis responsibilities exist in:
  - `crates/phazeai-core/src/analysis/outline.rs`
  - `crates/phazeai-core/src/context/repo_map.rs`
- the repo contains both top-level `training/` and `python/training/` pipelines
- some test files use helper-mirror logic rather than fully reusing the production implementation

## Things That Look Strongest

- core agent loop design
- tool trait and registry pattern
- provider registry concept
- LSP client/manager split
- conversation persistence model
- practical test coverage across core, UI, CLI, and sidecar layers

## Risk Notes

### Architectural risk

- Global state concentration in `IdeState`
- Multi-thousand-line UI files
- UI orchestrating service startup directly
- repeated local thread/runtime spawning from different panels

### Product/runtime risk

- cross-panel behavior may be hard to reason about because logic is spread across `app.rs`, panel files, and `lsp_bridge.rs`
- extension story spans multiple partially-overlapping mechanisms
- analysis and repo-map logic may drift because there are multiple implementations

### Maintenance risk

- large files increase the cost of safe edits
- behavior often depends on reactive side effects, which can make ownership less explicit

## Best “Mental Model” Of The Repository

Think of the repo as four stacked layers:

1. Product shell
   - `phazeai-ui`
   - `phazeai-cli`

2. Shared runtime engine
   - `phazeai-core`

3. Capability providers
   - sidecar
   - LSP servers
   - MCP servers
   - plugins/extensions
   - git subprocesses

4. Adjacent tooling
   - cloud client
   - training scripts
   - packaging
   - CI/release

That model is more accurate than thinking of this as a simple crate-based monolith.

## What “Knowing The Repo” Means From Here

At this point, the following are established:

- the top-level layout
- which code is first-party vs vendor/reference
- the runtime composition model
- the main control-plane object (`IdeState`)
- the engine seams (`Agent`, `Tool`, `LlmClient`, `LspManager`, `McpManager`)
- the UI hotspots
- the main supporting subsystems
- the test coverage shape

What is not yet exhaustively captured:

- every individual function-level call graph
- every keyboard shortcut path in the desktop UI
- every code path inside `editor.rs` and `git.rs`
- every training script’s exact differences and intended lifecycle

If you want a truly exhaustive next pass, the most valuable options are:

1. Function-level call graph for the desktop IDE startup and editor event loop
2. Function-level call graph for the CLI runtime
3. A refactor-oriented ownership map for `app.rs`, `editor.rs`, `git.rs`, and `lsp_bridge.rs`
4. A maturity audit separating “product-critical”, “adjacent”, and “experimental” code paths
