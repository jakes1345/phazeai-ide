# PhazeAI IDE — Full Project Deep Dive

> Generated: 2026-03-29 | Full read of every first-party source file.

---

## What Is This?

PhazeAI IDE is an **AI-native code editor built entirely in Rust**. The pitch: no Electron, no Python bloat, GPU-accelerated UI via [Floem](https://github.com/lapce/floem), local-first AI with Ollama/LM Studio support, and a clean MIT open-source core with an optional paid cloud tier.

It's a Cargo workspace with 8 crates, a Python sidecar, a Node.js extension host prototype, training scripts, and a massive vendored reference tree (`phazeai-arsenal/`) containing upstream projects like Zed, Helix, Lapce, egui, async-openai, rig, and mistral.rs for reference.

**Current status**: Phase 2 (active development). The IDE is daily-usable. CLI is production-ready. Many advanced features are done; some are still in progress.

---

## Repo Layout

```
crates/
  phazeai-core/       # Shared engine: agent, LLM, tools, LSP, MCP, context
  phazeai-ui/         # Desktop GUI (Floem + GPU) — PRIMARY product
  phazeai-cli/        # Terminal UI (ratatui)
  phazeai-cloud/      # Optional paid cloud client (skeleton)
  phazeai-sidecar/    # Rust manager for Python sidecar process
  phazeai-plugin-api/ # Native plugin ABI contract
  ollama-rs/          # Forked ollama-rs with streaming + tool calling
ext-host/             # Node.js VS Code-compatible extension host (prototype)
sidecar/              # Python JSON-RPC code indexing server
python/               # Extra Python utilities, embeddings, training helpers
training/             # Model fine-tune pipeline (QLoRA/Unsloth → GGUF → Ollama)
packaging/            # Flatpak, macOS DMG, Windows MSI
modelfiles/           # Ollama Modelfile definitions
assets/               # Branding, desktop launcher
.github/workflows/    # CI, feature tests, release automation
phazeai-arsenal/      # ~9000 files of vendored upstream reference code (NOT workspace member)
_archive/             # Old experiments, archived
```

---

## The 4-Layer Mental Model

1. **Product shell** — `phazeai-ui` (desktop) + `phazeai-cli` (terminal)
2. **Shared runtime engine** — `phazeai-core`
3. **Capability providers** — LSP servers, MCP servers, Python sidecar, git subprocesses, plugins
4. **Adjacent tooling** — cloud client, training scripts, packaging, CI

---

## Workspace Dependencies (root Cargo.toml)

Key shared deps: `tokio` (full), `serde`/`serde_json`, `reqwest 0.12` (json+stream), `async-trait`, `thiserror`/`anyhow`, `tracing`, `regex`, `ignore`, `globset`, `notify`, `futures`, `uuid`, `chrono`, `dirs`, `toml`, `ratatui 0.28`, `crossterm 0.28`, `clap 4.5`, `syntect 5.2`, `ropey 1.6`, `rfd 0.15`, `floem` (git rev e0dd862), `floem-editor-core`, `ollama-rs` (local fork), `comrak 0.36`, `tui-textarea 0.7`, `tui-tree-widget 0.22`, `tree-sitter 0.24`, `lsp-types 0.97`, `portable-pty 0.8`, `arboard 3.4`, `vte 0.13`.

---

---

## `phazeai-core` — The Engine

This is the most important crate. Everything else depends on it.

### `src/error.rs`

Single `PhazeError` enum with variants: `Llm(String)`, `Tool { tool, message }`, `Config(String)`, `Io`, `Http`, `Json`, `MaxIterations(usize)`, `Sidecar(String)`, `Other(String)`, `Cancelled`. Clean `thiserror` usage. `pub type Result<T>` alias.

### `src/constants.rs`

All magic numbers live here. Key modules:
- `models` — `PHAZE_BEAST`, `PHAZE_CODER`, `PHAZE_PLANNER`, `PHAZE_REVIEWER`, base models (`qwen2.5-coder:14b`, `llama3.2:3b`, `deepseek-coder-v2:16b`), default cloud models per provider. Default Claude model: `claude-sonnet-4-5-20250929`.
- `endpoints` — all API base URLs (Claude, OpenAI, Groq, Together, OpenRouter, Ollama `localhost:11434`, LM Studio `localhost:1234`, Gemini, a Pi LAN endpoint `192.168.1.155:8080`, DuckDuckGo search URL).
- `defaults` — theme `"Midnight Blue"`, font size 14.0, tab size 4, max tokens 8192, python path `"python3"`, default model `PHAZE_BEAST`.
- `modelfile` — temperature/top_p/num_ctx/repeat_penalty per role (coder: 0.3 temp, 32768 ctx; planner: 0.5 temp, 8192 ctx; reviewer: 0.2 temp, 16384 ctx).
- `terminal` — scrollback 10000 lines, read buffer 8192, `xterm-256color`, full 16-color ANSI palette as RGB tuples.
- `limits` — bash output max 30000 chars, bash timeout 120s, max files 5000, git status max 20 files, agent history max 20 runs, autosave debounce 500ms, file watch poll 5s.
- `paths` — config dir `"phazeai"`, config file `"config.toml"`, conversations dir, instruction file candidates (`CLAUDE.md`, `.phazeai/instructions.md`, etc.), project markers (Cargo.toml→rust, package.json→js, etc.).
- `ui` — all layout constants: activity bar 48px, explorer 220px, chat 320px, terminal 200px, status bar 24px, tab bar 32px, minimap 80px, z-index levels for every overlay type (command palette 100, file picker 200, completions 300, inline edit 400, toast 450, peek def 485, vim ex 490, goto 495).

### `src/config/mod.rs` — Settings

`Settings` struct with `llm: LlmSettings`, `editor: EditorSettings`, `sidecar: SidecarSettings`, `providers: Vec<ProviderEntry>`, `model_routes: HashMap<TaskType, ModelRoute>`.

`LlmSettings`: provider enum, model string, api_key_env, base_url, max_tokens.

`EditorSettings` (with defaults): theme, font_size 14.0, tab_size 4, show_line_numbers true, auto_save true, word_wrap false, relative_line_numbers false, inlay_hints true, code_lens true, organize_imports_on_save false.

`SidecarSettings`: enabled, python_path, auto_start.

Config path: `~/.config/phazeai/config.toml`. `Settings::load()` reads TOML, falls back to defaults. `Settings::save()` writes TOML. `Settings::build_llm_client()` builds either a direct `LlmClient` or a `ModelRouter` if routes are configured.

### `src/llm/traits.rs` — The LLM Protocol

Core types:
- `Role` enum: `User`, `Assistant`, `System`
- `Message` struct: role, content, optional `tool_calls: Vec<ToolCall>`, optional `tool_call_id: String`
- `ToolCall` struct: id, call_type, `function: FunctionCall { name, arguments: String }`
- `LlmResponse` struct: message, optional usage
- `Usage` struct: input_tokens u32, output_tokens u32
- `StreamEvent` enum: `TextDelta(String)`, `ToolCallStart { id, name }`, `ToolCallDelta { id, arguments_delta }`, `ToolCallEnd { id }`, `Usage(Usage)`, `Done`, `Error(String)`
- `LlmClient` trait (async_trait): `chat(messages, tools) -> LlmResponse`, `chat_stream(messages, tools) -> UnboundedReceiver<StreamEvent>`

### `src/llm/provider.rs` — Provider Registry

`ProviderId` enum: Claude, OpenAI, Ollama, Groq, Together, OpenRouter, LmStudio, Gemini, Custom(String). Each has `name()`, `is_local()`, `needs_api_key()`, `default_base_url()`, `default_api_key_env()`.

`ProviderConfig`: id, enabled, api_key_env, base_url, default_model. `api_key()` reads from env var. `is_available()` checks enabled + key present.

`ProviderRegistry`: HashMap of configs, active provider/model. `build_active_client()` dispatches to `ClaudeClient`, `OllamaClient`, or `OpenAIClient` (used for all other providers including Groq, Together, OpenRouter, Gemini — they're all OpenAI-compatible).

`known_models()` returns static model lists with context windows and pricing per million tokens. Notable: Gemini 1.5 Pro has 2M context window. Claude Opus 4.6 costs $15/$75 per M tokens.

`UsageTracker`: tracks cumulative tokens and estimates cost.

### `src/llm/model_router.rs` — Task-Based Routing

`TaskType` enum: `Reasoning`, `ToolOrchestration`, `CodeGeneration`, `CodeReview`, `QuickAnswer`.

`TaskType::classify(input, has_tools)` — heuristic keyword matching. If tools present → `ToolOrchestration`. Keywords like "explain/why/design/architect/plan" → `Reasoning`. "write/implement/create/build" → `CodeGeneration`. "review/bug/fix/error" → `CodeReview`. Short messages → `QuickAnswer`.

`ModelRouter` implements `LlmClient` itself — it routes each request to the appropriate pre-built client based on task type, falling back to default client when no route matches.

### `src/llm/claude.rs`

`ClaudeClient` with `reqwest::Client`. Builds Anthropic-format request bodies (system prompt extracted separately, tool calls as `tool_use` content blocks, tool results as `tool_result` content blocks). Streaming via SSE: handles `content_block_start`, `content_block_delta` (text_delta + input_json_delta), `content_block_stop`, `message_delta` (usage), `message_stop`. Maps content block indices to tool call IDs for delta routing.

### `src/llm/openai.rs`

`OpenAIClient` — handles OpenAI-format requests. Tool calls as `tool_calls` array in assistant messages, tool results as `role: "tool"` messages. Streaming via SSE `[DONE]` sentinel. Maps tool call indices to IDs across delta chunks (OpenAI only sends ID on first chunk). Also handles `stream_options: { include_usage: true }` for usage tracking.

### `src/llm/ollama.rs`

`OllamaClient` wraps the forked `ollama-rs` crate. For tool calls: falls back to non-streaming `chat()` then converts to stream events (Ollama streaming doesn't reliably support tool calls). For text-only: uses native `send_chat_messages_stream()`. Converts between PhazeAI `Message` types and `ollama_rs::ChatMessage` types.

### `src/llm/ollama_manager.rs`

`OllamaManager` — checks if Ollama is running, lists local models, auto-pulls `phaze-beast` if not present. Used by CLI's `main.rs` for auto-provisioning.

### `src/llm/discovery.rs`

Scans for local model providers (Ollama on port 11434, LM Studio on port 1234) by making HTTP requests. Returns discovered providers and their available models.

---

### `src/agent/core.rs` — The Agent Loop

`Agent` struct owns: `llm: Box<dyn LlmClient>`, `tools: ToolRegistry`, `conversation: Arc<Mutex<ConversationHistory>>`, `max_iterations: usize` (default 15), `max_context_tokens: usize` (default 32768), `approval_fn: Option<ApprovalFn>`, `cancel_token: Option<Arc<AtomicBool>>`.

Builder pattern: `with_tools()`, `with_max_iterations()`, `with_context_budget()`, `with_approval()`, `with_shared_conversation()`, `with_system_prompt()`, `with_cancel_token()`, `register_tool()`, `register_mcp_tools()`.

`AgentEvent` enum (shared CLI/IDE interface): `Thinking { iteration }`, `TextDelta(String)`, `ToolApprovalRequest { name, params }`, `ToolStart { name }`, `ToolResult { name, success, summary }`, `Complete { iterations }`, `Error(String)`, `BrowserFetchStart/Complete/Error`.

`AgentResponse`: content, tool_calls: Vec<ToolExecution>, iterations, total_input_tokens, total_output_tokens.

`ApprovalFn` type alias: `Box<dyn Fn(String, Value) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>`.

**The loop** (`run_with_events`):
1. Add user message to conversation
2. Check cancellation
3. Check max iterations
4. Trim conversation to token budget
5. Call `llm.chat_stream()` — accumulate text deltas and tool calls from stream
6. If tool calls: add assistant message with tool calls, execute each tool (with optional approval), add tool results, loop back
7. If no tool calls: add final assistant message, emit `Complete`, return `AgentResponse`

Tool results are truncated to 200 chars in the summary event. Tool result content is truncated to 12000 chars in conversation history.

### `src/agent/multi_agent.rs` — Multi-Agent Pipeline

`MultiAgentOrchestrator` with `llm: Arc<dyn LlmClient>`, `full_pipeline: bool`, `max_refinement_iterations: usize` (default 5), `role_clients: HashMap<AgentRole, Arc<dyn LlmClient>>`, `project_root: Option<String>`.

`AgentRole` enum: Planner, Coder, Reviewer, Orchestrator. Each has a hardcoded system prompt.

**Full pipeline** (`execute_full_pipeline`):
1. Auto-generate repo map if project root set
2. Run Planner → get plan
3. Run Coder with plan → get code
4. **Self-healing refinement loop** (up to `max_refinement_iterations`):
   - Run build check (`cargo check`, `tsc --noEmit`, `eslint`, `go build`, etc. — auto-detected by project type)
   - If errors: feed errors back to Coder with previous code, get fixed code
   - If clean: break
5. Run Reviewer on final code
6. Return `PipelineResult { plan, code, review, final_output, refinement_iterations, clean_build }`

Build check timeout: 120 seconds. Output truncated to 4000 chars. Error/warning counts parsed from output lines.

`MultiAgentEvent` enum: `AgentStarted`, `AgentOutput`, `AgentFinished`, `RefinementStarted`, `BuildCheck`, `RefinementIteration`, `RefinementComplete`, `PipelineComplete`, `Error`.

**Role prompts** (hardcoded strings):
- Planner: analyze request, produce numbered steps, identify files to touch, no code
- Coder: write complete code changes, diff format for modifications, full content for new files
- Reviewer: check correctness/bugs/security/style/performance, output ✅ APPROVED / ⚠️ CONCERNS / ❌ REJECTED

### `src/context/history.rs` — Conversation History

`ConversationHistory` with `VecDeque<Message>`, `max_messages: usize` (default 100), `system_prompt: Option<String>`.

`get_messages()` prepends system prompt as a `Message::system()`. `trim_to_token_budget()` pops from front. `estimate_tokens()` uses `len / 4` heuristic. Tool results truncated to 12000 chars on add.

### `src/context/persistence.rs` — Conversation Store

`ConversationStore` persists to `~/.phazeai/conversations/`. Uses `index.json` + per-conversation `{uuid}.json` files. Atomic writes via temp file + rename. `generate_id()` uses UUID v4. `list_recent(limit)` returns sorted by `updated_at`. `search(query)` does case-insensitive title substring match.

`SavedConversation` has metadata (id, title, created_at, updated_at, message_count, model, project_dir) + messages + system_prompt. `generate_title_from_first_message()` takes first 80 chars of first user message.

### `src/context/repo_map.rs` — Repo Map Generator

`RepoMapGenerator` walks the project with `ignore::WalkBuilder` (respects .gitignore), extracts symbols from source files using regex-based language-specific extractors, formats as a compact text map grouped by directory.

Supports: Rust, Python, JS/TS/JSX/TSX, Go, C/C++/H, Java, Ruby. Max 500 files, 4096 tokens by default.

Symbol kinds: Function, Struct, Enum, Trait, Impl, Const, TypeAlias, Macro, Module, Class, Interface, Method.

Rust extractor handles: `pub fn`/`fn`/`pub async fn`/`async fn`, `pub struct`/`struct`, `pub enum`/`enum`, `pub trait`/`trait`, `impl`/`impl<`, `pub const`/`const`, `pub type`/`type`, `macro_rules!`, `pub mod`/`mod`.

Output format: `📁 dir/\n  📄 file.rs\n    fn function_name (L42)\n    struct StructName (L10)\n...`

### `src/context/system_prompt.rs` — System Prompt Builder

`SystemPromptBuilder` detects project type from root dir (Cargo.toml→Rust, package.json→JS/TS, go.mod→Go, etc.), collects git branch + dirty files, loads custom instructions from multiple candidates in priority order: `.phazerules`, `.cursorrules`, `CLAUDE.md`, `AGENTS.md`, `.phazeai/instructions.md`, `.phazeai/config.md`, `.ai/instructions.md`. Also walks up parent dirs (max 5 levels) for `CLAUDE.md`/`AGENTS.md`. Also loads `~/.phazeai/instructions.md` for global user instructions.

`build()` assembles: core identity + project context + available tools + planning guidelines + tool guidelines (with examples) + safety rules + custom instructions.

**Core identity** (hardcoded): "You are PhazeAI, an elite AI coding assistant... Direct & Technical... High Agency... Read First... Minimal Changes... Verification..."

**Tool guidelines** describe all 17 tools with examples of the Read→Edit→Verify workflow pattern.

**Safety rules**: refuse to delete critical paths, ask before destructive commands, never read/write `.env` files, monitor iteration count.

### `src/context/builder.rs`

Context builder that assembles the full context for an agent run, combining system prompt, repo map, and conversation history.

---

### `src/tools/traits.rs` — Tool Registry

`ToolDefinition`: name, description, parameters (JSON schema Value).

`Tool` trait (async_trait): `name()`, `description()`, `parameters_schema() -> Value`, `execute(params: Value) -> ToolResult`, `to_definition()`.

`ToolRegistry`: HashMap of `Box<dyn Tool>`. `register()`, `get()`, `list()`, `definitions()`.

**Default registry** (24 tools): ReadFile, WriteFile, BashTool, Grep, ListFiles, Glob, Edit, FindPath, Fetch, WebSearch, CopyPath, MovePath, DeletePath, CreateDirectory, Now, Open, Diagnostics, Memory, Browse, Download, Screenshot.

**Read-only registry**: ReadFile, Grep, Glob, ListFiles, FindPath, Now, Memory.

**Standard registry**: read-only + Fetch, WebSearch, Diagnostics, Bash, Browse, Download, Screenshot.

`register_mcp_tools()` creates `McpToolBridge` instances for each MCP server tool, naming them `mcp__serverName__toolName`.

### Individual Tools

- **`bash.rs`** — `BashTool` runs shell commands. Default timeout 120s (from constants). Output truncated to 30000 chars. Supports working directory override.
- **`file.rs`** — `ReadFileTool` (path, optional offset/limit), `WriteFileTool` (path, content, creates parent dirs).
- **`edit.rs`** — `EditTool` — search-and-replace in files. Preferred over WriteFile for existing files.
- **`grep.rs`** — `GrepTool` — regex search in files/dirs.
- **`list.rs`** — `ListFilesTool` — non-recursive directory listing.
- **`glob.rs`** — `GlobTool` — glob pattern file search.
- **`find_path.rs`** — `FindPathTool` — regex-based file path search (like `find`/`fd`).
- **`fetch.rs`** — `FetchTool` — HTTP GET requests to external URLs.
- **`web_search.rs`** — `WebSearchTool` — DuckDuckGo search via HTML scraping.
- **`browse.rs`** — `BrowseTool` — headless browser-like page fetching.
- **`copy_path.rs`** — `CopyPathTool` — copy file/dir.
- **`move_path.rs`** — `MovePathTool` — move/rename file/dir.
- **`delete_path.rs`** — `DeletePathTool` — delete file/dir.
- **`create_directory.rs`** — `CreateDirectoryTool` — recursive mkdir.
- **`now.rs`** — `NowTool` — current timestamp.
- **`open.rs`** — `OpenTool` — `xdg-open`/`open`/`start` for files/URLs.
- **`diagnostics.rs`** — `DiagnosticsTool` — runs `cargo check` or language linter, parses structured errors.
- **`memory.rs`** — `MemoryTool` — persistent key-value memory store for the agent.
- **`download.rs`** — `DownloadTool` — download file from URL to disk.
- **`screenshot.rs`** — `ScreenshotTool` — take a screenshot.
- **`mcp_bridge.rs`** — `McpToolBridge` — wraps an MCP tool as a native `Tool`. `create_mcp_tool_bridges()` creates one bridge per tool per connected server.
- **`approval.rs`** — `ToolApprovalManager` with modes: `Auto` (approve all), `Ask` (ask each time), `AskOnce` (ask once per tool type, remember). `needs_approval()` checks mode and history. `record_approval()` stores approved tools.

### `src/lsp/client.rs` — LSP Client

`LspClient` spawns a language server process, speaks JSON-RPC/LSP over stdio. Reader thread parses Content-Length framed messages and dispatches responses to pending oneshot channels or emits `LspEvent`s.

`LspEvent` enum: `Diagnostics { uri, diagnostics }`, `Completions`, `Hover`, `Definition`, `References`, `Formatting`, `DocumentSymbols`, `Initialized(String)`, `Shutdown`, `Log(String)`.

Supported requests: `initialize`, `completion`, `hover`, `goto_definition`, `signature_help`, `rename_symbol`, `find_references`, `formatting`, `document_symbols`, `goto_implementation`, `inlay_hints`, `folding_range`, `code_action`, `workspace_symbol`.

Notifications: `did_open`, `did_change`, `did_save`.

Progress events encoded as special log messages: `__progress__message%`, `__progress_end__`, `__progress_create__`.

### `src/lsp/manager.rs` — LSP Manager

`LspManager` maps language IDs to `Arc<LspClient>` instances. `ensure_server_for_file()` lazily starts the right server. `detect_available_servers()` uses `which` to check if binaries exist.

Default configs: `rust-analyzer`, `pyright-langserver --stdio`, `typescript-language-server --stdio`, `gopls`, `clangd`.

Language ID mapping: `.rs`→rust, `.py`→python, `.js/.mjs/.cjs`→javascript, `.jsx`→javascriptreact, `.ts/.mts`→typescript, `.tsx`→typescriptreact, `.go`→go, `.c/.h`→c, `.cpp/.cc/.cxx/.hpp`→cpp, `.java`→java, `.rb`→ruby, `.lua`→lua, `.sh/.bash`→shellscript, `.json`→json, `.yaml/.yml`→yaml, `.toml`→toml, `.md`→markdown, `.html/.htm`→html, `.css`→css.

### `src/mcp.rs` — MCP Client/Manager

`McpClient` connects to an MCP server by spawning a process (stdio transport). Speaks JSON-RPC 2.0 with Content-Length framing (same as LSP). Reader thread dispatches responses to pending oneshot channels.

`initialize()` sends protocol version `"2024-11-05"`, client info `"PhazeAI"`, then discovers tools/resources/prompts.

`McpManager` manages multiple `McpClient` instances. Loads config from `.phazeai/mcp.json` (array of `McpServerConfig { name, command, args, env }`). `all_tools()` returns `(server_name, McpToolDef)` pairs. `call_tool(server_name, tool_name, arguments)` routes to correct client.

### `src/git/ops.rs` — Git Operations

`GitOps` wraps `git` subprocess calls. `find_root()` walks up from a path looking for `.git`. Methods: `status()` (parses `--porcelain`), `diff(staged)`, `add(paths)`, `commit(message)`, `log(count)`.

`GitStatus`: branch, files: Vec<FileStatus>, is_clean. `FileState` enum: Modified, Added, Deleted, Renamed, Untracked, Conflicted.

### `src/analysis/outline.rs` — Symbol Extraction

`CodeSymbol` with name, kind, start_line, end_line, signature, children: Vec<CodeSymbol>.

`SymbolKind` enum with icons: Function/Method→"ƒ", Class→"C", Struct→"S", Enum→"E", Interface/Trait→"I", Module→"M", Constant→"K", Variable→"V", Import→"⬇", Type→"T".

`extract_symbols_generic(source, extension)` dispatches to language-specific extractors. Handles impl blocks with method children for Rust. Handles class/method nesting for Python.

`symbols_to_repo_map(path, symbols)` formats as `filename:\n  icon name | signature\n`.

`generate_repo_map(root)` walks entire directory and generates map for all source files.

Note: This module overlaps with `context/repo_map.rs`. Both do regex-based symbol extraction. `outline.rs` produces hierarchical `CodeSymbol` trees; `repo_map.rs` produces flat text output. Some duplication exists.

### `src/analysis/linter.rs`

Basic code metrics and issue analysis (line counts, complexity heuristics).

### `src/project/workspace.rs`

`ProjectType` enum: Rust, Node, Python, Go, Java, Cpp, Unknown. Detects by walking up from a path looking for project markers. `find_workspace_root()` returns the root path.

### `src/project/watcher.rs`

File system watcher using the `notify` crate. Debounced change events.

### `src/ext_host/mod.rs` — Native Plugin Host

`ExtensionManager` scans `~/.phazeai/plugins/` for subdirectories with `plugin.toml` manifests. Loads shared libraries via `libloading`. Checks API version compatibility (`API_VERSION = 1`). Calls `_phazeai_plugin_create()` → `on_activate(host)`. Dispatches commands and events. Supports hot reload.

`NativePlugin` owns the raw `*mut dyn PhazePlugin` pointer and the `Library`. Drop calls `on_deactivate()` then `_phazeai_plugin_destroy()` then drops the library.

`IdeDelegate` trait (for UI layer): `log()`, `show_message()`, `get_active_text()`. `IdeDelegateHost` bridges it to `PluginHost`.

Platform lib naming: `lib{name}.so` (Linux), `lib{name}.dylib` (macOS), `{name}.dll` (Windows).

### `src/ext_host/asset_loader.rs`, `registry.rs`, `theme_convert.rs`, `vscode_assets.rs`, `vsix.rs`

VS Code asset ingestion pipeline: loads VSIX packages, extracts themes (TextMate → syntect format), language configs, snippets, grammar files. `ExtensionRegistry` aggregates all loaded assets. `theme_convert.rs` converts VS Code theme JSON to syntect `ThemeSet`.

### `src/ext_host/js.rs`, `wasm.rs`

Stub modules for JS and WASM extension loading (experimental/placeholder).

---

---

## `phazeai-ui` — The Desktop IDE

### `src/bin/phazeai-ui.rs`

Entry point. On Linux: raises `RLIMIT_STACK` before building the Floem view tree (the UI hierarchy is deep enough to overflow default stack). Calls `phazeai_ui::launch_phaze_ide()`.

### `src/lib.rs`

Re-exports `launch_phaze_ide`.

### `src/theme.rs`

`PhazeTheme` and `PhazePalette` structs. 12 built-in themes: MidnightBlue, Cyberpunk, Dracula, Tokyo Night, Material, Nord, Catppuccin, Solarized, Gruvbox, Monokai, One Dark, GitHub Light. Each theme defines colors for background, foreground, accent, border, syntax tokens, gutter, status bar, etc.

### `src/util.rs`

UI utility functions: string truncation, path formatting, color helpers.

### `src/app.rs` — The Control Plane (~6453 lines)

`IdeState` is a `#[derive(Clone)]` struct of `RwSignal<T>` fields. It is the global state hub for the entire UI. Signals are `Copy` and UI-thread-only — no `Arc<Mutex<>>` needed.

**`IdeState` owns signals for:**
- Layout: panel visibility (left, bottom, right), panel widths/heights, split editor panes
- Files: workspace root, open tabs, active file path, file contents
- LSP: diagnostics, completions, definition results, references, symbols, code lens, inlay hints, folding ranges, progress indicator
- Editor toggles: vim mode, auto-save, word wrap, relative line numbers, organize imports, code lens visibility, whitespace rendering, sticky scroll, minimap
- Overlays: command palette, file picker, rename dialog, hover popup, peek definition, workspace symbols, branch picker, goto line, vim ex mode
- Git: current branch, git status
- Sidecar: readiness flag, semantic search results
- Extensions: `ExtensionManager`, extension UI state
- Terminal: command injection channel
- Toast/status: notification messages, spinner state
- AI: pending chat injection, composer state

**`IdeState::new(&settings)` does:**
1. Resolve workspace root (walks up from cwd looking for project markers)
2. Restore session from `~/.config/phazeai/session.toml`
3. Load editor config
4. Start LSP bridge (spawns background thread)
5. Wire reactive effects: active file changes → LSP didOpen, document symbols, folding ranges, code lens, inlay hints
6. Wire definition results → navigation
7. Detect read-only and line-ending state for active file
8. Create persistent settings signals with save effects
9. Attempt sidecar boot (looks for `sidecar/server.py`)
10. Create `ExtensionManager`
11. Wire AI provider/model persistence

**Key overlays in `app.rs`:**
- Command palette (Ctrl+P): fuzzy file search + command list
- File picker: full file browser overlay
- Completion popup: LSP completions with prefix filter, 300ms debounce
- Ctrl+K inline edit: select code → describe change → AI rewrites with diff preview
- Rename overlay (F2): LSP workspace/rename
- Hover popup (Ctrl+F1): LSP hover info
- Peek definition (Alt+F12): inline definition preview at z_index 485
- Workspace symbols (Ctrl+T): cross-file symbol search
- Branch picker: git branch list + checkout + create
- Goto line (Ctrl+G): jump to line number
- Vim ex mode: `:` command input

**Global key handler** in `app.rs` handles: Ctrl+P (command palette), Ctrl+B (toggle explorer), Ctrl+J (toggle terminal), Ctrl+\ (toggle chat), Ctrl+= / Ctrl+- (font zoom), Ctrl+Shift+Z (zen mode), F11 (fullscreen via wmctrl/xdotool), Ctrl+K (inline AI edit), and many more.

**`launch_phaze_ide()`**: loads settings, creates `IdeState`, composes the full window layout using Floem's reactive system, opens the window.

### `src/lsp_bridge.rs` — LSP Bridge (~1662 lines)

Adapts background `LspManager` activity into Floem signals. Exposes a sync-safe `LspCommand` sender.

`LspCommand` enum covers: OpenFile, CloseFile, ChangeFile, SaveFile, RequestCompletions, RequestHover, RequestDefinition, RequestReferences, RequestFormatting, RequestDocumentSymbols, RequestSignatureHelp, RequestRename, RequestCodeActions, RequestWorkspaceSymbols, RequestInlayHints, RequestFoldingRanges, RequestCodeLens, ApplyWorkspaceEdit, OrganizeImports.

**Local fallbacks**: ripgrep-based references when LSP unavailable, regex-based symbol extraction for document symbols.

**Debouncing**: file change events debounced before sending to LSP.

**Workspace edit application**: applies LSP `WorkspaceEdit` responses (text edits across multiple files).

### `src/panels/editor.rs` — The Editor (~5153 lines)

Custom editor implementation on top of Floem's `text_editor` widget. Not a thin wrapper — absorbs massive feature logic.

**Features implemented directly:**
- Syntax highlighting via syntect (skipped for files > 2MB)
- Word highlighting (double-click or cursor word)
- Git gutter decorations (green/yellow/red bars for added/modified/deleted lines)
- LSP diagnostic squiggles (wave_line for errors, under_line for warnings)
- Code folding (Ctrl+Shift+[ / Ctrl+Shift+])
- Bracket pair colorization (4-color cycling: gold/sky-blue/violet/mint)
- Bracket pair guides (vertical 1px lines)
- Auto-close brackets and quotes
- Auto-surround (select + type bracket)
- Smart indent on Enter
- De-indent on `}`
- Word wrap toggle (Alt+Z)
- Sticky scroll (function/class headers pinned at top)
- Minimap (right-side canvas with viewport indicator)
- Breadcrumbs (file path segments above editor)
- Indentation guides
- Whitespace rendering (Ctrl+Shift+W)
- Relative line numbers
- Current line highlight
- Bracket match highlight
- Find/replace with regex, case-sensitive, whole-word toggles + match count
- Split editor (Ctrl+Alt+\)
- Diff view (GitDiff bottom tab)
- Large file handling (> 2MB → plain text)
- Line ending indicator (CRLF/LF/Mixed)
- Encoding indicator (UTF-8)
- Read-only mode
- Multi-cursor (Ctrl+D)
- Column/box selection
- Vim mode (Normal/Insert, motions, dd/x, o, i/a, visual mode, yank/paste)
- Inline blame
- Inlay hints
- Code lens
- Inline diagnostics in status bar
- Context menu (right-click): Copy/Paste/Go to Def/Find Refs/Rename/Code Actions/AI actions
- Sort lines, join lines, transform case
- Auto-save (1.5s debounce via AtomicU64 cancel token)
- Format on save (rustfmt/prettier/black)
- EditorConfig reading

### `src/panels/terminal.rs` — Terminal Emulator (~1733 lines)

Full PTY terminal panel. Spawns PTY via `portable-pty`. Parses VTE output (256-color, bold, italic, underline, reverse video). Maintains styled scrollback buffer (10000 lines). Renders terminal lines as Floem canvas. Handles keyboard input and clipboard (Ctrl+Shift+C/V). Multiple terminal tabs with "+" button and × to close. Named terminals (click to rename). Shell profile selection. Hyperlink detection (URL_RE regex + xdg-open). OSC 7 shell integration for cwd tracking and command markers. Terminal zoom (Ctrl+Shift+=/-). Clear terminal (⌫ button writes Ctrl+L to PTY).

### `src/panels/chat.rs` — AI Chat

Maintains chat transcript UI. For each send: spawns dedicated runtime thread, builds `Agent` with `Settings::build_llm_client()`, optionally connects MCP tools, streams `AgentEvent`s into UI updates. Expands `@filename` mentions into file contents as context blocks. Conversation persistence via `ConversationStore`. Session browser (⊟ button).

### `src/panels/composer.rs` — AI Composer

Multi-file AI task panel. Builds `Agent` with workspace-aware `BashTool`. Streams agent events into event log and diff cards. Supports cancellation via `Arc<AtomicBool>`. More action-oriented than chat — designed for autonomous multi-file edits.

### `src/panels/git.rs` — Git Panel (~4245 lines)

Runs many git subprocess commands directly. Parses `git status --porcelain`. Full UI for: commit/stage/unstage/discard/pull/push/stash/branch/tag/log/blame/cherry-pick/revert hunk. Git gutter decorations. Inline diff hunk preview. Commit history log with diff-between-commits. Branch switching/creation/merge. Stash list. Pull/push buttons. AI-assisted commit message generation (runs `git diff --cached` → AI). AI code review button. `.git/index` mtime polling for auto-refresh.

### `src/panels/explorer.rs` — File Explorer

File tree with expand/collapse. Create/delete/rename/duplicate/reveal/copy-path via right-click context menu. File watcher (notify + 300ms debounce). Git status badges (M/U/D). Collapse all button. Excludes: target, node_modules, dist, .next, __pycache__, etc.

### `src/panels/search.rs` — Workspace Search

Ripgrep-backed workspace text search. Regex, case-sensitive, whole-word toggles. Include/exclude glob inputs. Replace-in-files. Search history (Up/Down, capped at 50). Tree view toggle (flat list vs grouped-by-file). Result click → jump to file/line.

### `src/panels/settings.rs` — Settings Panel

Theme picker, font size slider, tab size, AI provider/model dropdowns. Writes back into `IdeState` signals that persist to `~/.config/phazeai/config.toml`.

### `src/panels/ai_panel.rs` — AI Panel

Unified AI panel that hosts both chat and composer views with tab switching.

### `src/panels/extensions.rs` — Extensions Panel

UI over `ExtensionManager`. Lists loaded plugins, shows name/version/description/commands. Load/unload/reload actions.

### `src/panels/github_actions.rs` — GitHub Actions Panel

Parses repo remote URL to extract owner/repo. Calls GitHub API using token from env. Displays workflow runs and jobs. Auto-refresh. Rerun support.

### `src/components/`

Shared UI primitives: `button.rs`, `icon.rs`, `input.rs`, `panel.rs`, `scroll.rs`, `tabs.rs`. Reusable Floem view components.

---

---

## `phazeai-cli` — Terminal UI

### `src/main.rs`

`Cli` struct (clap): `--prompt` (single-shot), `--model`, `--provider`, `--theme` (default "dark"), `--continue`/`-c` (resume last), `--resume <id>` (resume by ID prefix), `--instructions <path>` (extra instructions file).

Reads piped stdin and injects into prompt context. Auto-provisions `phaze-beast` via `OllamaManager::ensure_phaze_beast()` if using Ollama with that model. Dispatches to `run_single_prompt()` or `run_tui()`.

### `src/commands.rs`

`CommandResult` enum with 35+ variants. `handle_command(input)` dispatches slash commands.

Full command list: `/help`, `/exit`/`/quit`/`/q`, `/clear`, `/new`, `/model <name>`, `/provider <name>`, `/approve <mode>` (auto/ask/ask-once), `/cost`, `/theme <name>`, `/files`/`/tree`, `/status`, `/compact`, `/save`, `/load <id>`, `/conversations`/`/history`, `/diff`, `/git`, `/log`, `/search <glob>`, `/pwd`, `/cd <dir>`, `/version`, `/models`, `/discover`, `/context`, `/mode <mode>` (plan/debug/chat/ask/edit), `/plan`, `/debug`, `/ask`, `/edit`, `/chat`, `/add <file>`, `/retry`, `/cancel`/`/stop`, `/yolo` (auto-approve all), `/grep <pattern>`, `/skill`/`/cmd`/`/run <name> [args]`, `/install-github-action`.

### `src/app.rs` — TUI App (~2646 lines)

**`run_single_prompt()`**: builds agent, tries to start sidecar, runs `agent.run_with_events()`, prints events to stdout/stderr.

**`run_tui()`**: full ratatui TUI. `AppState` struct holds: input buffer + cursor, input history (Up/Down cycling), chat messages as `Vec<ChatItem>` (Message or ToolCard), scroll state, processing flag, pending approval, status/model/token info, conversation management, session picker UI, tool approval manager, AI mode, agent task handle, approval oneshot channel.

**Agent worker**: single long-lived tokio task that receives user inputs via `mpsc::UnboundedSender<String>` and runs `agent.run_with_events()` for each. Connects MCP servers from `.phazeai/mcp.json`. Tries to start Python sidecar. Loads restored history. Approval callback uses oneshot channel to block on UI response.

**AI modes** with prompt prefixes:
- `plan`: "[PLANNING MODE] You are a senior software architect. Produce a clear, structured, step-by-step plan. No code yet."
- `debug`: "[DEBUG MODE] You are an expert debugger. Diagnose root cause before suggesting fix."
- `ask`: "[READ-ONLY MODE] Answer the question. Do NOT modify files."
- `edit`: "[EDIT MODE] Make precise, minimal code changes. Use edit_file tool."
- `chat`: no prefix (natural conversation)

**UI layout**: vertical split — chat area (with optional file tree sidebar) + input/approval area + status bar. Scrollbar when content exceeds visible height. Tool cards show name + output + success/error status.

**Keyboard shortcuts**: Ctrl+C (quit/abort), Ctrl+L (clear), Ctrl+U (kill line), Ctrl+W (delete word), Ctrl+B (toggle file tree), Ctrl+E (open $EDITOR for prompt), Up/Down (history navigation), PgUp/PgDn (scroll chat), Enter (send).

**Conversation restore**: `--continue` loads most recent conversation. `--resume <id>` does prefix match. Restored messages are loaded into agent history via `agent.load_history()`.

### `src/theme.rs`

`Theme` struct for ratatui colors: fg, bg, accent, border, user_color, assistant_color, tool_color, muted. Named themes: dark, tokyo-night, dracula. `Theme::by_name()` and `Theme::all_names()`.

---

## `phazeai-sidecar` — Python Sidecar Integration

### Rust side

`SidecarClient`: JSON-RPC client over stdio to Python process. Methods: `call(method, params)`, `search_embeddings(query, top_k)`, `build_index(paths)`, `analyze_file(path, content)`, `health_check()`. Uses `AtomicU64` for request IDs. Async tokio I/O.

`SidecarManager`: spawns Python process with `stdin/stdout/stderr` piped. `start()`, `stop()`, `is_running()`, `check_python()`. Warns on drop if process not explicitly stopped.

`BuildIndexTool` and `SemanticSearchTool`: wrap `SidecarClient` as `Tool` implementations for registration in the agent's `ToolRegistry`.

`JsonRpcRequest`/`JsonRpcResponse`: protocol types.

### Python side (`sidecar/server.py`)

Pure stdlib Python. JSON-RPC 2.0 over stdio (newline-delimited).

**`TfidfIndex`**: TF-IDF search index. `tokenize()` (lowercase + `\b\w+\b` regex), `compute_term_freq()`, `compute_idf()` (with cache), `compute_tfidf()`, `cosine_similarity()` (sparse vectors). `add_document()`, `search(query, top_k)`.

**`CodeAnalyzer`**: regex-based symbol extraction. Patterns for Rust (fn, struct, enum, trait, impl), Python (def, class), JS (function, class, const arrow), Go (func, type struct/interface), Java (method, class), C (function heuristic). Returns `{ symbols: { functions, classes, structs, traits, enums, other }, line_count, char_count }`.

**`CodeIndex`**: manages `TfidfIndex` + indexed file set. `should_index_file()` checks extension (`.rs`, `.py`, `.js`, `.ts`, `.jsx`, `.tsx`, `.go`, `.java`, `.c`, `.cpp`, `.h`, `.hpp`, `.md`, `.toml`, `.json`, `.yaml`, `.yml`, `.txt`) and skips SKIP_DIRS (`.git`, `node_modules`, `target`, `__pycache__`, `dist`, `build`, `.next`, `.venv`, `venv`, `vendor`). `build_index(paths)` walks dirs, reads files, adds to index. `search(query, top_k)` returns `[{ file, score, snippet }]`.

**`JsonRpcServer`**: handles `ping`, `build_index`, `search`, `analyze`. Main loop reads stdin line by line, parses JSON, dispatches, writes response.

---

## `phazeai-cloud` — Cloud Client

`cloud_api_url()` returns `https://api.phazeai.com/v1` (overridable via `PHAZEAI_CLOUD_URL`).

`CloudCredentials`: email + api_token. Persisted to `~/.config/phazeai/cloud.toml`. `is_authenticated()` checks token non-empty.

`CloudSession`: email, token, tier, credits_remaining.

`CloudClient`: reqwest HTTP client. `validate()` calls `/account` endpoint. `stream_chat_url()` returns `/chat/completions` (OpenAI-compatible). Currently a skeleton — no real streaming implementation wired up.

`Tier` enum (in `subscription.rs`): Free, Cloud, Team, Enterprise.

**Status**: skeleton crate. The cloud sign-in in the UI opens a browser URL stub. No real auth flow implemented yet.

---

## `phazeai-plugin-api` — Plugin ABI

`API_VERSION: u32 = 1`.

`PluginHost` trait: `log(level, msg)`, `show_message(msg)`, `get_active_text() -> String`, `get_active_file_path() -> String`, `insert_text(text)`, `execute_command(cmd, args) -> Result<String, String>`.

`PhazePlugin` trait: `name()`, `version()`, `description()`, `on_activate(host)`, `on_deactivate()`, `commands() -> Vec<PluginCommand>`, `execute_command(cmd, args)`, `on_event(event)` (default no-op).

`PluginCommand`: id, title, optional keybinding.

`PluginEvent` enum: `FileOpened { path }`, `FileSaved { path }`, `FileClosed { path }`, `CursorMoved { line, col }`, `SelectionChanged { text }`, `Custom { kind, data }`.

`PluginManifest`: name, version, description, author, min_api_version, optional library override.

`declare_plugin!(MyPlugin)` macro generates 3 `extern "C"` entry points: `_phazeai_plugin_api_version()`, `_phazeai_plugin_create()`, `_phazeai_plugin_destroy(ptr)`.

---

## `ollama-rs` (Local Fork)

Package name: `ollama-rs-phazeai`. Feature flags: `stream`, `chat-history`, `function-calling`. Adds native tool-calling support and streaming chat history that the upstream crate lacked. Used by `OllamaClient` in `phazeai-core`.

---

## `ext-host/` — Node.js Extension Host

`src/main.js`: entry point. Listens for `loadExtension` RPC calls. Auto-loads `dummy-extension` on startup. Notifies `hostReady` with PID.

`src/extension-loader.js`: loads Node.js extensions from a directory.

`src/rpc.js`: host-side RPC layer for communication with the Rust IDE.

`src/vscode-shim.js`: partial VS Code API shim. Implements `vscode.window.showInformationMessage/showErrorMessage`, `vscode.commands.registerCommand/executeCommand`, `vscode.workspace.getConfiguration` (returns defaults), `vscode.ExtensionContext`. Intercepts `require('vscode')` calls from extensions.

`ext-host/dummy-extension/`: example extension for testing.

`ext-host/test-extension.vsix`: packaged VSIX test artifact.

**Status**: prototype/experimental. The Rust ext_host layer is more production-shaped. The JS host is a compatibility experiment.

---

---

## Training Pipeline

### `training/` (top-level)

`prepare_data.py`: collects training data from local code + public datasets.
`prepare_tool_data.py`: prepares tool-call specific training examples.
`fine_tune.py`: QLoRA fine-tuning via Unsloth. Optimized for 8GB+ VRAM.
`export_gguf.py`: exports fine-tuned model to GGUF format for Ollama.
`datasets/*.jsonl`: training data files.

Pipeline: prepare data → fine-tune (2-6 hours on RTX 2060 Super) → export GGUF → register with Ollama → test.

### `python/training/` (expanded toolkit)

`advanced_collect.py`, `train_pipeline.py`, `advanced_fine_tune.py`, `sota_fine_tune.py` — larger experimentation surface for model research.

### `modelfiles/`

`Modelfile.coder`: based on `qwen2.5-coder:14b`, temp 0.3, top_p 0.9, num_ctx 32768, repeat_penalty 1.1. System prompt: "You are phaze-coder, an expert coding assistant..."
`Modelfile.planner`: based on `llama3.2:3b`, temp 0.5, num_ctx 8192. System prompt: "You are phaze-planner, a strategic software architect..."
`Modelfile.reviewer`: based on `deepseek-coder-v2:16b`, temp 0.2, num_ctx 16384. System prompt: "You are phaze-reviewer, a meticulous code reviewer..."
`install.sh`: creates all three models via `ollama create`.

---

## CI/CD

### `.github/workflows/ci.yml`

Runs on push/PR. Steps: `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo audit`, then crate-specific tests: core, CLI, UI, agent, git, MCP, sidecar. Installs Linux GUI deps (libxcb-render0-dev, etc.) for UI tests.

### `.github/workflows/feature-tests.yml`

Broader scenario coverage: tool-system tests, full MCP integration, LSP integration, context-engine tests, provider registry tests, eval harness smoke checks, settings persistence, scheduled stress tests.

### `.github/workflows/release.yml`

Cross-platform builds for Linux, macOS ARM/Intel, Windows. Packages both `phazeai-ui` and `phazeai` (CLI). Publishes GitHub Release artifacts on version tags or manual dispatch.

---

## Packaging

`packaging/flatpak/com.phazeai.IDE.json`: Flatpak manifest.
`packaging/flatpak/com.phazeai.IDE.desktop`: Desktop entry.
`packaging/flatpak/com.phazeai.IDE.metainfo.xml`: App metadata.
`packaging/macos/build-dmg.sh`: macOS DMG packaging script.
`packaging/macos/entitlements.plist`: macOS entitlements.
`packaging/windows/build-msi.ps1`: Windows MSI build script.
`packaging/windows/phazeai-ide.wxs`: WiX installer definition.

---

## Hardware Context

Developer machine: AMD Ryzen 5 3600 (6-core), NVIDIA RTX 2060 Super (8GB VRAM), 46GB DDR4, Linux Mint 22.3, kernel 6.14.0-37-generic.

With 8GB VRAM: can run multi-agent pipelines (phaze-coder 14B + phaze-planner 3B + phaze-reviewer 16B at 4-bit quantization), fine-tune custom models in 2-6 hours.

Minimum: 6GB VRAM, 16GB RAM, 4-core CPU, 50GB storage.

---

## Key Architectural Facts

### What's Clean

- `LlmClient` trait boundary — all providers implement the same async streaming interface
- `Tool` trait + `ToolRegistry` — uniform tool execution, easy to add new tools
- `LspClient`/`LspManager` split — transport vs orchestration
- `McpClient`/`McpManager` split — same pattern
- `ConversationHistory` + `ConversationStore` — clean separation of in-memory vs disk
- `ProviderRegistry` — config-to-runtime bridge
- `Agent` builder pattern — composable, testable

### What's Messy / High Coupling

- `app.rs` (~6453 lines) — simultaneously state model, service bootstrap, window layout, global controller
- `editor.rs` (~5153 lines) — rendering + editing actions + language service integration all in one
- `git.rs` (~4245 lines) — command runner + complex UI + AI integration
- `lsp_bridge.rs` (~1662 lines) — transport adaptation + fallbacks + edit application + feature logic
- `IdeState` is very wide — acts as a global dependency container
- Multiple panels create their own background runtimes and agent instances
- Analysis/repo-map responsibilities overlap between `analysis/outline.rs` and `context/repo_map.rs`
- UI panels often talk directly to subprocesses rather than through narrower controller APIs

### What's Incomplete / Skeleton

- `phazeai-cloud` — no real auth/API calls, just stubs
- Python sidecar `server.py` exists but the TODO.md notes it as a known bug ("Python sidecar server.py does not exist — phazeai-sidecar Rust client stubs") — this is outdated, the file does exist
- `ext-host/` JS host — prototype, not production
- `ext-host/wasm.rs` and `ext-host/js.rs` — stub modules
- DAP debugger — not started
- Test runner panel — not started
- Plugin marketplace — not started
- Real-time collaboration — not started

---

## Runtime Flows

### AI Chat/Composer

1. Panel collects user input
2. Loads `Settings`, builds `LlmClient`
3. Constructs `Agent` with tools + optional MCP
4. `agent.run_with_events(prompt, tx)` streams `AgentEvent`s
5. Panel maps events → UI updates (text deltas, tool cards, diff cards)

### File Open/Edit/LSP

1. `open_file` signal changes in `IdeState`
2. `app.rs` effect sends `LspCommand::OpenFile`
3. `lsp_bridge.rs` routes to background `LspManager`
4. `LspManager` ensures correct language server exists
5. `LspClient` sends LSP notifications/requests
6. LSP results become UI signals
7. `editor.rs` reads signals and updates rendering

### Sidecar Startup

1. `IdeState::new` searches for `sidecar/server.py`
2. If found: spawns `SidecarManager`, starts Python process
3. Creates `SidecarClient` from process
4. Registers `SemanticSearchTool` and `BuildIndexTool` in agent registry
5. `sidecar_ready` signal set to true

### Multi-Agent Pipeline

1. User triggers from composer or git panel
2. `MultiAgentOrchestrator::execute()` called
3. Auto-generates repo map from project root
4. Planner → Coder → Build Check Loop → Reviewer
5. Events streamed via `MultiAgentEvent` channel
6. UI shows progress per agent role

---

## What We're Trying to Do (Summary)

Build the best open-source AI-native IDE. Local-first (Ollama/LM Studio work out of the box), all Rust (no Electron, no Python processes for the core), GPU-accelerated UI (Floem/Vello/wgpu), MIT licensed core, optional paid cloud tier for hosted models and team features.

The IDE is a real competitor to VS Code for developers who want:
- Native performance (3-5x faster startup claimed)
- True local AI (no API keys required)
- Full control (no telemetry, no license servers)
- Modern AI features (inline edit, streaming chat, multi-agent composer, ghost text FIM)

The monetization model: free forever for self-hosted + BYOK, $15/month for PhazeAI Cloud (hosted models, 500k tokens/month), $35/seat/month for Team, custom for Enterprise.

---

## Known Bugs / Tech Debt (from TODO.md)

- Completion popup position can overlap status bar on short files
- Ghost text FIM fires on empty prefix — should skip
- Terminal: no Ctrl+Left/Right word navigation
- Terminal: resize not propagated to PTY on window resize
- Settings: ai_provider change doesn't update FIM client
- Multi-tab session restore: deleted files show empty tab with no error
- Syntax highlighting cache can get stale after large edits
- Find/replace: `\n` in replace string not handled
- `phazeai-cloud` crate is a skeleton

---

*End of deep dive. Every first-party source file has been read.*
