# PhazeAI — What To Do Next
> Full deep dive complete. Every source file read. This is the honest assessment.

---

## What This Project Actually Is

A Rust-native AI-first IDE. The core engine (`phazeai-core`) is solid and well-architected. The desktop UI (`phazeai-ui`) is feature-rich but massive and tightly coupled. The CLI (`phazeai-cli`) is production-ready. The cloud crate is a skeleton. The sidecar works. The plugin system is real but untested in the wild.

The project is in a "feature-complete but not shippable" state. Most of the TODO list is done. What's left is polish, stability, and the things that make it feel like a real product vs a demo.

---

## Priority 1 — Ship Blockers (Do These First)

### 1. The cloud crate is completely fake
`phazeai-cloud/src/client.rs` has `stream_chat_url()` that just returns a URL string — it never actually streams anything. `auth.rs` has no real auth flow. The "Sign in" button in the UI opens a browser URL that may not even exist. Either:
- Wire up real auth (OAuth flow, token exchange) and real streaming
- Or remove the cloud sign-in button entirely until it works

### 2. The sidecar Python script path is fragile
`IdeState::new()` searches 6 candidate paths for `server.py`. If none exist, sidecar silently fails. The TODO.md even has a bug note: "Python sidecar server.py does not exist" (outdated — it does exist at `sidecar/server.py`). The real problem: when installed as a binary, the relative path `sidecar/server.py` won't exist. Need to either bundle the Python script or ship it alongside the binary.

### 3. `app.rs` is 6770 lines — it will break you
The entire IDE state, all overlays, all keyboard handlers, the menu bar, the status bar, the cosmic canvas, the drag overlay, the sidecar startup, the LSP bridge startup — all in one file. This isn't a bug but it's a maintenance timebomb. Before adding any more features, split it:
- `state.rs` — `IdeState` struct + `new()`
- `overlays.rs` — all the popup/overlay views
- `keybindings.rs` — the global key handler
- `layout.rs` — `ide_root()`, `menu_bar()`, `activity_bar()`, `status_bar()`

### 4. Session persistence is broken for tabs
`session_save_tabs()` writes `open_tab = "path"` lines but `session_load()` only reads `open_file = ` and `open_tab = ` lines. The tab restore works but the active file and tabs can get out of sync. Also: if a file was deleted between sessions, the tab shows empty with no error message (known bug in TODO.md).

### 5. The `edit_file` tool fails on multi-occurrence text
`edit.rs` returns an error if `old_text` matches more than once and `replace_all` is false. This is correct behavior but the error message is confusing. The agent frequently hits this. Add a `context` parameter that lets the agent provide surrounding lines to disambiguate.

---

## Priority 2 — Core Quality Issues

### 6. Duplicate symbol extraction code
There are THREE separate implementations of "extract symbols from source code":
- `crates/phazeai-core/src/context/repo_map.rs` — flat regex extraction
- `crates/phazeai-core/src/analysis/outline.rs` — hierarchical regex extraction  
- `sidecar/server.py` — Python regex extraction

They all do the same thing with slightly different logic. Pick one Rust implementation, make it the canonical one, delete the others. The `outline.rs` version is better (hierarchical, handles impl blocks with methods).

### 7. The `lsp_bridge.rs` code actions are fake
`RequestCodeActions` in `lsp_bridge.rs` calls `generate_code_actions()` which is a local function that just returns "Format Document", "Organize Imports", and "Find All References" — it never actually calls the LSP server's `textDocument/codeAction`. The comment says "No LSP codeAction yet". Wire it up to the real LSP client.

### 8. Inlay hints fallback is too simplistic
`inlay_hints_from_file()` in `lsp_bridge.rs` only handles `let x = <literal>` patterns and only for Rust. It's better than nothing but produces wrong hints for complex expressions. The LSP path works fine — the fallback just needs to be removed or improved.

### 9. The `web_search` tool HTML parser is fragile
`parse_ddg_results()` in `web_search.rs` splits on `class="result__a"` — this will break whenever DuckDuckGo changes their HTML. Use a proper HTML parser crate (`scraper` or `select`) or switch to a search API.

### 10. Memory tool stores to `.phazeai/memory.json` in CWD
`memory.rs` uses `std::env::current_dir()` to find the memory file. If the agent changes directory (via bash tool), the memory file location changes. Should use a fixed path like `~/.phazeai/memory.json` or the workspace root.

### 11. `BashTool` doesn't persist working directory
`bash.rs` has a `cwd: Arc<Mutex<PathBuf>>` but there's no `cd` command handling — the cwd never actually changes between calls. If the agent runs `cd /some/dir` in bash, the next bash call still runs from the original directory. Either implement proper cwd tracking or document this limitation clearly.

### 12. Screenshot tool uses `which` to detect tools
`screenshot.rs` calls `std::process::Command::new("which")` to check if `scrot`/`grim`/`import` exist. This is fine on Linux but `which` doesn't exist on Windows. Use `std::process::Command::new(tool).arg("--version")` instead, or just try to run the tool and handle the error.

---

## Priority 3 — Missing Features That Matter

### 13. DAP debugger — not started
The TODO.md has a full debugger spec (breakpoints, step over/into/out, variables panel, watch panel, call stack, debug console). This is a major competitive gap vs VS Code. Start with a basic DAP client that can connect to `lldb-vscode` or `codelldb` for Rust debugging.

### 14. Test runner panel — not started
No way to run tests from the IDE. At minimum: parse `cargo test` output, show pass/fail per test, click to jump to failing test. This is table stakes for a coding IDE.

### 15. Multi-repo support — not started
The git panel only handles one repo. If you open a monorepo or a workspace with multiple git repos, only the root repo is tracked. The `GitOps::find_root()` function exists but isn't used for multi-repo detection.

### 16. Terminal resize not propagated to PTY
Known bug in TODO.md. When the terminal panel is resized, the PTY doesn't get the new dimensions. This causes display glitches in terminal apps that care about terminal size (vim, htop, etc.). Fix: call `portable_pty::PtySize` update when the terminal panel size changes.

### 17. Ghost text FIM fires on empty prefix
Known bug in TODO.md. The fill-in-the-middle completion triggers even when the cursor is at the start of a line with no prefix. Add a check: only trigger FIM if there's at least 1 non-whitespace character before the cursor.

### 18. Completion popup position overlaps status bar
Known bug in TODO.md. The completion popup is positioned at a fixed `padding_top(120.0)` from the top of the screen. On short files or when the cursor is near the bottom, it overlaps the status bar. Need to calculate position based on cursor location.

---

## Priority 4 — Architecture Improvements

### 19. `IdeState` has too many signals (~100+)
The struct has over 100 `RwSignal` fields. This makes it hard to understand what's related to what. Group them into sub-structs:
- `EditorState` — cursor, tabs, vim mode, nonces
- `LspState` — diagnostics, completions, definitions, etc.
- `SidecarState` — ready, status, results, nonces
- `UiState` — panels, overlays, theme, zen mode

### 20. Multiple panels spawn their own tokio runtimes
`chat.rs`, `composer.rs`, `git.rs`, and `app.rs` all do `tokio::runtime::Builder::new_current_thread().enable_all().build()` in `std::thread::spawn`. This means there are 4+ separate tokio runtimes running simultaneously. Consolidate to one shared runtime.

### 21. The `ModelRouter` task classification is too simple
`TaskType::classify()` uses keyword matching. "fix the bug" → `CodeReview` (because "fix" matches). "write a plan" → `CodeGeneration` (because "write" matches before "plan"). The heuristics are wrong in common cases. Either improve the classifier or remove it — most users won't configure model routes anyway.

### 22. `ConversationHistory::estimate_tokens()` is wrong
It uses `len / 4` (chars per token). This is a rough approximation that's off by 2-3x for code-heavy conversations. Use a proper tokenizer or at least use `len / 3` for code (code is denser than prose).

---

## Priority 5 — Things That Are Just Missing

### 23. No way to install extensions from the UI
The `extensions_panel` shows loaded plugins but there's no install button, no marketplace, no way to add a `.vsix` file from the UI. The `install_vsix()` function exists in `asset_loader.rs` but nothing calls it from the UI.

### 24. The `phazeai-cloud` subscription tiers are defined but unused
`Tier` enum has `SelfHosted`, `Cloud`, `Team`, `Enterprise` with pricing. `CloudSession` has `credits_remaining`. None of this is wired to anything — the UI just shows "☁ Sign in" which opens a browser URL.

### 25. No keyboard shortcut to open the sidecar search
The sidecar semantic search UI exists in `IdeState` (signals for query, results, nonces) but there's no keyboard shortcut or panel to trigger it from the UI. The signals are there but nothing in `app.rs` or any panel exposes them to the user.

### 26. The `Modelfile-Phaze-Lite` resource is embedded but the content is unknown
`ollama_manager.rs` does `include_str!("../../resources/Modelfile-Phaze-Lite")` for `ensure_phaze_beast()`. Need to verify this file exists and has the right content for the `phaze-beast` model.

### 27. No CI test for the actual UI
The CI runs `cargo test --workspace` but the UI tests in `tests/tier1_state.rs`, `tests/tier2_integration.rs`, etc. likely require a display server. The CI workflow installs `libxcb` deps but doesn't set up a virtual display (Xvfb). UI tests probably get skipped silently.

### 28. The `ext-host/` Node.js host is dead code
`ext-host/src/main.js` auto-loads a `dummy-extension` that doesn't exist in the repo. The `vscode-shim.js` is a stub. The `test-extension.vsix` is a binary artifact checked into git. This whole directory should either be developed properly or removed.

---

## The Honest Priority Order

If I had to pick what to work on right now:

1. Fix the cloud crate (real auth or remove the button)
2. Fix sidecar script bundling for binary distribution
3. Split `app.rs` into smaller files
4. Fix terminal PTY resize
5. Fix ghost text FIM empty prefix bug
6. Wire real LSP code actions
7. Consolidate the three symbol extractors into one
8. Add a basic test runner panel (cargo test output)
9. Fix the `edit_file` tool disambiguation
10. Start the DAP debugger client

The project is genuinely impressive for what it is. The agent loop, LSP integration, multi-agent pipeline, MCP support, and the full-featured editor are all real and working. The gaps are mostly in polish and the "last mile" features that make it feel complete.
