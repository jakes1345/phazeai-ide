# Core/UI Architecture Map

Deep map for the two main product layers:

- `crates/phazeai-core`
- `crates/phazeai-ui`

Indexed on 2026-03-24.

## What Matters Most

The codebase has one clear center of gravity:

- `phazeai-core` is the engine layer
- `phazeai-ui` is the stateful desktop shell over that engine

If you need to understand behavior quickly, start with these files:

1. `crates/phazeai-ui/src/app.rs`
2. `crates/phazeai-core/src/agent/core.rs`
3. `crates/phazeai-ui/src/lsp_bridge.rs`
4. `crates/phazeai-ui/src/panels/editor.rs`
5. `crates/phazeai-core/src/llm/provider.rs`
6. `crates/phazeai-core/src/tools/traits.rs`

## Architectural Split

### `phazeai-core`

Responsibilities:

- Agent loop
- LLM provider abstraction
- Tool abstraction and built-in tool implementations
- Context/history/persistence
- Repo map and lightweight analysis
- LSP process management
- MCP client/server integration
- Project/workspace detection
- Plugin/extension loading
- Git helpers

Design style:

- Mostly service-oriented Rust modules
- Async used at boundaries that talk to LLMs/tools/processes
- Several process-hosting subsystems: LSP, MCP, plugins, sidecar-facing tools

### `phazeai-ui`

Responsibilities:

- Desktop shell and window composition
- Global reactive state via Floem `RwSignal`
- Panel composition
- LSP bridge from background runtime into UI signals
- Editor implementation and editor-local behaviors
- Terminal rendering and PTY management
- Git, chat, composer, explorer, search, settings, extensions panels

Design style:

- Very large centralized app-state object: `IdeState`
- Heavy use of reactive side effects
- Several panels instantiate their own background threads/runtimes for agent work
- Significant behavior lives directly in large UI files rather than thin view wrappers

## File Size Hotspots

These files are the main complexity centers:

| File | Approx. lines | Why it matters |
|---|---:|---|
| `crates/phazeai-ui/src/app.rs` | 6453 | Global shell, startup, overlays, keybindings, panel orchestration |
| `crates/phazeai-ui/src/panels/editor.rs` | 5153 | Editor implementation, syntax highlighting, tabs, editing features |
| `crates/phazeai-ui/src/panels/git.rs` | 4245 | Git panel + many command wrappers and views |
| `crates/phazeai-ui/src/panels/terminal.rs` | 1733 | PTY + terminal parser + renderer |
| `crates/phazeai-ui/src/lsp_bridge.rs` | 1662 | UI-facing LSP command/event bridge |
| `crates/phazeai-core/src/context/repo_map.rs` | 882 | Repo-map extraction and formatting |
| `crates/phazeai-core/src/lsp/client.rs` | 755 | Raw LSP stdio client |
| `crates/phazeai-core/src/analysis/outline.rs` | 735 | Symbol extraction / repo-map-like source analysis |
| `crates/phazeai-core/src/mcp.rs` | 591 | MCP process client + manager |
| `crates/phazeai-core/src/llm/provider.rs` | 525 | Provider registry and client construction |
| `crates/phazeai-core/src/ext_host/mod.rs` | 511 | Native plugin host |
| `crates/phazeai-core/src/agent/multi_agent.rs` | 555 | Multi-agent pipeline orchestration |

Practical implication:

- Large architectural changes usually touch `app.rs`, `editor.rs`, `lsp_bridge.rs`, and one `phazeai-core` service file.
- Bugs often come from cross-file coupling rather than isolated modules.

## Startup Path

Primary launch path:

1. `crates/phazeai-ui/src/bin/phazeai-ui.rs`
2. `phazeai_ui::launch_phaze_ide()`
3. `crates/phazeai-ui/src/app.rs::launch_phaze_ide`
4. `IdeState::new(&settings)`
5. Floem window composition using `ide_root(...)` plus overlay views

### Notable startup behaviors

- Linux binary raises the process stack limit before building the UI tree.
- Settings are loaded from disk up front.
- `IdeState::new` performs substantial initialization, not just state allocation.
- LSP bridge is started during state construction.
- Session state is restored from `~/.config/phazeai/session.toml`.
- Editor settings are loaded and wired to persistence effects.
- Sidecar startup is attempted if `sidecar/server.py` is found.
- Extension manager is created during `IdeState::new`.

This means `IdeState::new` is a true composition root, not a plain data constructor.

## `IdeState`: The UI Control Plane

`crates/phazeai-ui/src/app.rs` defines `IdeState`, which is the global state hub for nearly everything in the UI.

It owns signals for:

- Layout and panel visibility
- Open files and tabs
- Workspace root
- LSP outputs: diagnostics, completions, definition, references, symbols, code lens, inlay hints, folding ranges, progress
- Editor behavior toggles: vim mode, auto-save, word wrap, relative lines, organize imports, code lens visibility
- Command palette, file picker, overlays, rename, hover, peek definition, workspace symbols
- Git branch and git-related UI state
- Sidecar readiness and semantic-search results
- Extension manager and extension UI state
- Split editor panes
- Terminal command injection
- Toast/status output

This is the single most important state object in the product.

### What `IdeState::new` actually does

It does all of the following:

- Resolves workspace root
- Restores session
- Loads editor config
- Starts LSP bridge
- Wires reactive navigation from definition results
- Wires `didOpen` and document-symbol requests on active-file changes
- Wires requests for folding ranges, code lens, and inlay hints on active-file changes
- Detects read-only and line-ending state for the active file
- Creates persistent settings signals and save effects
- Attempts sidecar boot
- Creates extension manager
- Persists AI provider/model changes

This is one of the strongest signs that `app.rs` is both state model and service bootstrap layer.

## `phazeai-core` Module Map

### `agent`

Files:

- `agent/core.rs`
- `agent/multi_agent.rs`

#### `agent/core.rs`

Primary type: `Agent`

Responsibilities:

- Maintain conversation history
- Stream model output
- Collect tool calls from streamed events
- Execute tools
- Re-insert tool results into conversation
- Repeat until model stops calling tools
- Emit user-facing `AgentEvent`s

The runtime loop is:

1. Add user message to conversation
2. Trim history to token budget
3. Ask LLM for streamed response
4. Accumulate text and tool calls
5. If tool calls exist:
6. Optionally request approval
7. Execute each tool from `ToolRegistry`
8. Append tool results to conversation
9. Repeat
10. If no tool calls, finalize assistant message and return

Important properties:

- Conversation state is shared through `Arc<Mutex<ConversationHistory>>`
- Cancellation is supported via `Arc<AtomicBool>`
- Tool approvals are injected as callback behavior
- Tool execution is uniform through the `Tool` trait

This is the main abstraction that chat/composer/git/other AI panels depend on.

#### `agent/multi_agent.rs`

Primary type: `MultiAgentOrchestrator`

Responsibilities:

- Planner/Coder/Reviewer style orchestration
- Optional per-role model overrides
- Build-check and refinement loop
- Repo-map generation if available

The pipeline is explicit:

- Planner
- Coder
- Build/check loop
- Fix iterations
- Reviewer

This is conceptually important, but it appears less central to the everyday UI path than `Agent`.

### `llm`

Files:

- `llm/traits.rs`
- `llm/provider.rs`
- `llm/model_router.rs`
- `llm/claude.rs`
- `llm/openai.rs`
- `llm/ollama.rs`
- `llm/ollama_manager.rs`
- `llm/discovery.rs`

#### `llm/traits.rs`

This defines the protocol surface:

- `Message`
- `LlmResponse`
- `StreamEvent`
- `LlmClient`
- tool-call representations

Everything else hangs off this trait boundary.

#### `llm/provider.rs`

Primary type: `ProviderRegistry`

Responsibilities:

- Define provider identities
- Track provider configs and availability
- Pick active provider/model
- Build a concrete `LlmClient`

This is the configuration-to-runtime bridge.

#### `llm/model_router.rs`

Primary type: `ModelRouter`

Responsibilities:

- Classify tasks into `TaskType`
- Route requests to different models/clients
- Fall back to default client when routing is unavailable

Classification is heuristic and message-driven. Tool presence forces `ToolOrchestration`.

This matters because `Settings::build_llm_client()` may return either a direct client or a router.

### `tools`

Files:

- `tools/traits.rs`
- plus concrete tool files like `bash.rs`, `file.rs`, `grep.rs`, `web_search.rs`, `browse.rs`, `mcp_bridge.rs`

#### `tools/traits.rs`

Primary types:

- `Tool`
- `ToolDefinition`
- `ToolRegistry`

Responsibilities:

- Standardize tool metadata and execution
- Maintain named tool registry
- Expose tool definitions to the LLM
- Provide precomposed tool sets: default, read-only, standard
- Register MCP tools as normal tools

The tool model is simple and strong:

- Every tool has name/description/schema
- Every tool executes from JSON params to JSON result

This is the key extensibility seam between LLM output and runtime side effects.

### `context`

Files:

- `context/builder.rs`
- `context/history.rs`
- `context/persistence.rs`
- `context/repo_map.rs`
- `context/system_prompt.rs`

#### `context/history.rs`

Owns in-memory conversation state.

Used by:

- `Agent`
- likely CLI resume/continue flows

#### `context/persistence.rs`

Primary type: `ConversationStore`

Responsibilities:

- Persist conversations to `~/.phazeai/conversations`
- Maintain `index.json`
- Save/load/delete/list conversations

This is the disk boundary for AI history.

#### `context/repo_map.rs`

Primary type: `RepoMapGenerator`

Responsibilities:

- Walk the repository
- Extract symbols using regex-based language-specific scanners
- Format a compact repo map under a token/character budget

Despite the header comment mentioning tree-sitter, the current implementation path shown in `generate()` uses regex extractors over supported language extensions.

This module is strategically important for model context assembly.

#### `context/system_prompt.rs`

Primary type: `SystemPromptBuilder`

Responsibilities:

- Detect project type
- Gather git information
- Build a prompt describing the repo and working context

This sits upstream of `Agent` quality and tool behavior.

### `lsp`

Files:

- `lsp/client.rs`
- `lsp/manager.rs`

#### `lsp/client.rs`

Primary type: `LspClient`

Responsibilities:

- Spawn one language server process
- Speak JSON-RPC/LSP over stdio
- Send initialize/open/change/save/completion/hover/definition/etc.
- Emit `LspEvent`s back to consumers

This is a low-level transport/process client.

#### `lsp/manager.rs`

Primary type: `LspManager`

Responsibilities:

- Detect available server binaries
- Map file extensions to language IDs
- Lazily start one `LspClient` per language
- Dispatch open/change/save to correct client

This is the workspace-level LSP orchestrator consumed by the UI bridge.

### `mcp`

File:

- `mcp.rs`

Primary types:

- `McpClient`
- `McpManager`

Responsibilities:

- Load `.phazeai/mcp.json`
- Spawn external MCP server processes
- Initialize protocol handshake
- Discover tools/resources/prompts
- Call MCP tools and read resources
- Expose discovered tools for registration into `ToolRegistry`

This is the external capability ingress path.

### `ext_host`

Files:

- `ext_host/mod.rs`
- `ext_host/asset_loader.rs`
- `ext_host/registry.rs`
- `ext_host/theme_convert.rs`
- `ext_host/vscode_assets.rs`

Responsibilities:

- Native plugin loading via shared libraries
- VSIX/VS Code asset ingestion
- Theme/language/snippet/grammar registration
- Extension metadata summaries and uninstall support

Important types:

- `ExtensionManager`
- `ExtensionRegistry`
- `NativePlugin`

This subsystem is half runtime plugin host and half VS Code asset importer.

### `project`

Files:

- `project/workspace.rs`
- `project/watcher.rs`

Responsibilities:

- Detect workspace root and project type by walking upward
- Watch files for change events

This is foundational but comparatively small.

### `analysis`

Files:

- `analysis/outline.rs`
- `analysis/linter.rs`

Responsibilities:

- Lightweight symbol extraction
- Repo-map formatting helpers
- Basic code metrics and issue analysis

These modules overlap conceptually with `context/repo_map.rs`. There is some duplication in “source understanding” responsibilities.

## `phazeai-ui` Module Map

### `app.rs`

This is not just a shell file. It contains:

- `IdeState`
- session load/save
- settings persistence helpers
- command palette
- file picker
- left/bottom panel composition
- status bar
- multiple overlays
- menu bar
- root keybinding handling
- main application launch

This file is effectively:

- app state model
- service bootstrap
- window layout composition
- global interaction controller

That is the core architectural fact of the UI layer.

### `lsp_bridge.rs`

Purpose:

- Adapt background `LspManager` activity into Floem signals
- Expose a sync-safe `LspCommand` sender to the UI
- Flatten LSP data into UI-specific structs

Key responsibilities:

- Debounce/route file change events
- Return completions/hover/definition/references/signature help/symbols
- Offer local fallbacks such as ripgrep-based references and symbol extraction
- Apply workspace edits
- Produce code actions, organize imports, code lens, inlay hints, folding ranges

This file is the bridge between:

- low-level `phazeai-core` LSP process management
- high-level reactive editor UX

### `panels/editor.rs`

This is the second major center of gravity after `app.rs`.

Responsibilities:

- Render text editor instances
- Syntax highlighting via syntect
- Word highlighting
- Git gutter
- Diagnostics highlighting
- Folding
- Bracket matching and guides
- Find matches
- Inline blame
- Tab handling
- Multi-cursor/editor operations
- EditorConfig reading
- Integration with many `IdeState` nonces and signals

Architectural role:

- This is a custom editor implementation, not a thin wrapper around a stock widget.
- It absorbs a large amount of feature logic directly.

### `panels/terminal.rs`

Responsibilities:

- Spawn PTY
- Parse VTE output
- Maintain styled scrollback buffer
- Render terminal lines
- Handle keyboard input and clipboard
- Track cwd and prompt positions

This is effectively an embedded terminal emulator panel.

### `panels/git.rs`

Responsibilities:

- Run many git subprocess commands directly
- Parse `git status --porcelain`
- Commit/stage/reset/discard/pull/push/stash/branch/tag/log/blame flows
- Render large source-control UI
- Trigger AI-assisted git-related features

This panel is unusually large and command-heavy. It acts more like a mini feature module than a view.

### `panels/chat.rs`

Responsibilities:

- Maintain chat transcript UI
- Spawn a dedicated runtime thread for each send
- Build an `Agent`
- Optionally connect MCP tools
- Stream `AgentEvent`s into UI updates
- Expand `@file` mentions into prompt context

This is the simplest direct UI-to-agent path.

### `panels/composer.rs`

Responsibilities:

- Multi-file AI task panel
- Build an `Agent` with a workspace-aware `BashTool`
- Stream agent events into event log and diff cards
- Support cancellation

This is the more action-oriented agent surface compared with chat.

### `panels/explorer.rs`

Responsibilities:

- Build visible file tree
- Manage create/delete actions
- Rebuild tree and show git badges

This is a filesystem-oriented panel with some local git awareness.

### `panels/search.rs`

Responsibilities:

- Workspace text search
- Replace-all support

Likely shell/ripgrep backed.

### `panels/settings.rs`

Responsibilities:

- Theme/editor/provider UI
- Writes back into signals that persist to settings

This is a pure configuration surface over `IdeState`.

### `panels/extensions.rs`

Responsibilities:

- UI over `ExtensionManager`
- Extension listing/loading behavior

### `panels/github_actions.rs`

Responsibilities:

- Parse repo remote
- Call GitHub API using token
- Display workflow runs/jobs

This is a standalone integration panel.

## Main Runtime Flows

### Flow 1: AI chat/composer

1. Panel collects user input
2. Panel loads `Settings`
3. Panel builds LLM client from `Settings`
4. Panel constructs `Agent`
5. Optional MCP servers are connected and tools registered
6. Agent streams `AgentEvent`s
7. Panel maps events into UI-specific updates
8. Final content/diffs/tool summaries are rendered

Key files:

- `phazeai-ui/src/panels/chat.rs`
- `phazeai-ui/src/panels/composer.rs`
- `phazeai-core/src/agent/core.rs`
- `phazeai-core/src/tools/traits.rs`
- `phazeai-core/src/mcp.rs`

### Flow 2: File open/edit/LSP

1. `open_file` signal changes in `IdeState`
2. `app.rs` effect sends `LspCommand::OpenFile`
3. `lsp_bridge.rs` routes command to background `LspManager`
4. `LspManager` ensures correct language server exists
5. `LspClient` sends LSP notifications/requests
6. LSP results become UI signals
7. `editor.rs` reads diagnostics/completions/symbols/etc. and updates rendering

Key files:

- `phazeai-ui/src/app.rs`
- `phazeai-ui/src/lsp_bridge.rs`
- `phazeai-core/src/lsp/manager.rs`
- `phazeai-core/src/lsp/client.rs`
- `phazeai-ui/src/panels/editor.rs`

### Flow 3: Sidecar startup

1. `IdeState::new` searches for `sidecar/server.py`
2. If found, it spawns a `SidecarManager`
3. Startup success toggles `sidecar_ready`
4. Search nonce/query signals can update sidecar search UI state

Key files:

- `phazeai-ui/src/app.rs`
- `crates/phazeai-sidecar/*`
- `sidecar/server.py`

This path currently appears lighter-weight than the LSP path and may still be evolving.

### Flow 4: Extensions

1. `IdeState::new` creates `ExtensionManager`
2. Extension UI interacts with manager
3. Core ext-host loads plugin binaries and/or VSIX assets
4. `ExtensionRegistry` aggregates languages/themes/snippets/grammars

Key files:

- `phazeai-ui/src/app.rs`
- `phazeai-ui/src/panels/extensions.rs`
- `phazeai-core/src/ext_host/mod.rs`
- `phazeai-core/src/ext_host/registry.rs`
- `phazeai-core/src/ext_host/asset_loader.rs`

## Ownership Boundaries

### Strong boundaries

- `LlmClient` trait boundary is clean
- `Tool` trait boundary is clean
- `LspManager` vs `LspClient` split is clean
- `ExtensionManager` / `ExtensionRegistry` split is understandable

### Weak boundaries

- `app.rs` owns too much behavior
- `editor.rs` owns too much feature logic
- `git.rs` is both command runner and complex UI
- `lsp_bridge.rs` mixes transport adaptation, fallbacks, edit application, and feature logic
- analysis/repo-map responsibilities are spread across multiple core modules

## Likely Change Zones By Feature

### New AI behavior

Usually touches:

- `phazeai-core/src/agent/core.rs`
- one or more files in `phazeai-core/src/tools/`
- `phazeai-ui/src/panels/chat.rs` or `composer.rs`

### New editor capability

Usually touches:

- `phazeai-ui/src/panels/editor.rs`
- `phazeai-ui/src/app.rs`
- `phazeai-ui/src/lsp_bridge.rs` if it depends on LSP

### New provider/model routing feature

Usually touches:

- `phazeai-core/src/config/mod.rs`
- `phazeai-core/src/llm/provider.rs`
- `phazeai-core/src/llm/model_router.rs`
- `phazeai-ui/src/panels/settings.rs`

### New extension/theme/language asset support

Usually touches:

- `phazeai-core/src/ext_host/asset_loader.rs`
- `phazeai-core/src/ext_host/vscode_assets.rs`
- `phazeai-core/src/ext_host/registry.rs`
- `phazeai-ui/src/panels/extensions.rs`

### New git workflow

Usually touches:

- `phazeai-ui/src/panels/git.rs`
- sometimes `phazeai-core/src/git/ops.rs`

## Architectural Risks

These are the main complexity risks visible from the current shape:

- `IdeState` is very wide and acts as a global dependency container.
- `app.rs` is simultaneously composition root, controller layer, and view layer.
- `editor.rs` and `git.rs` are large enough to resist safe refactoring.
- Multiple panels create their own background runtimes and agent instances.
- Some core responsibilities overlap, especially analysis/repo-map/symbol extraction.
- UI panels often talk directly to subprocesses or runtime services rather than through narrower controller APIs.

## Best Reading Order For Real Work

If you need to modify or debug the system efficiently:

1. Read `crates/phazeai-ui/src/app.rs` for startup and state ownership.
2. Read `crates/phazeai-ui/src/lsp_bridge.rs` for the editor-service data plane.
3. Read `crates/phazeai-ui/src/panels/editor.rs` for text-editing behavior.
4. Read `crates/phazeai-core/src/agent/core.rs` for AI execution semantics.
5. Read `crates/phazeai-core/src/tools/traits.rs` and the relevant tool file.
6. Read `crates/phazeai-core/src/llm/provider.rs` and `model_router.rs` for model selection.
7. Read `crates/phazeai-core/src/lsp/manager.rs` and `client.rs` for language-server behavior.

## Bottom Line

The architecture is fundamentally:

- one shared Rust engine
- one very stateful desktop shell
- several subprocess-backed capability providers

The cleanest abstraction seams are in `phazeai-core` around:

- `LlmClient`
- `Tool`
- `LspClient`/`LspManager`
- `McpClient`/`McpManager`

The highest-coupling zones are in `phazeai-ui`, especially:

- `app.rs`
- `editor.rs`
- `git.rs`
- `lsp_bridge.rs`

If you are planning future refactors, the best leverage points are:

- split `IdeState` by domain
- carve controller/service logic out of `app.rs`
- split `editor.rs` into rendering, editing actions, and language-service integration
- narrow `lsp_bridge.rs` into transport, adaptation, and local-fallback layers
