# PhazeAI IDE Beta Audit TODO

Last updated: 2026-03-24

This is a practical beta-readiness backlog, not a theoretical cleanup list. It focuses on the work needed to make the IDE start reliably, feel internally consistent, and keep the AI paths usable for day-to-day work.

## Current baseline

Verified in this pass:

- `cargo fmt --all --check` passes
- `cargo test --workspace` passes
- `cargo clippy --workspace -- -D warnings` passes
- Ignored GUI suite in `crates/phazeai-ui/tests/tier2_integration.rs` passes under `xvfb-run`

That means the repo is in a much better state than before. The remaining list below is about getting from "green enough to demo" to "good enough to daily-drive".

## P0: Must fix for a credible beta

- [x] Respect sidecar settings and add an actual indexing lifecycle
  - Why: the desktop app hardcodes `python3` and auto-starts the sidecar whenever `server.py` is found, ignoring `settings.sidecar.enabled`, `settings.sidecar.python_path`, and `settings.sidecar.auto_start`.
  - Why: the GUI now issues real search requests, but it never builds the sidecar index automatically and offers no obvious UI action for rebuilding it.
  - Files:
    - `crates/phazeai-ui/src/app.rs`
    - `crates/phazeai-core/src/config/mod.rs`
    - `crates/phazeai-sidecar/src/client.rs`
    - `crates/phazeai-sidecar/src/tool.rs`
    - `sidecar/server.py`
  - Done means:
    - sidecar start honors settings
    - first-run index build is triggered or clearly offered
    - index status/errors are visible in the UI
    - semantic search never silently fails with "index not built"

- [x] Fix sidecar process ownership and shutdown
  - Why: `SidecarManager::take_process()` transfers the child process into `SidecarClient`, but `SidecarClient` does not keep a child handle for cleanup. That is a likely orphan-process leak.
  - Files:
    - `crates/phazeai-sidecar/src/manager.rs`
    - `crates/phazeai-sidecar/src/client.rs`
    - `crates/phazeai-ui/src/app.rs`
  - Done means:
    - sidecar process is owned by one component
    - IDE shutdown kills the sidecar cleanly
    - restart/reconnect behavior is explicit
  - Resolution: SidecarClient already held `process: Mutex<Child>` with Drop impl. Added `sidecar_client` field to IdeState exposing the shared Arc. Added `EventListener::WindowClosed` handler that calls `client.shutdown()` + saves session on IDE exit.

- [ ] Unify global shortcut dispatch
  - Why: the current fix is pragmatic but still layered across root view, editor view, and terminal view. That will regress again as more focused surfaces are added.
  - Files:
    - `crates/phazeai-ui/src/app.rs`
    - `crates/phazeai-ui/src/panels/editor.rs`
    - `crates/phazeai-ui/src/panels/terminal.rs`
  - Done means:
    - a single action/command layer owns global shortcuts
    - focused widgets dispatch commands instead of each reimplementing toggles
    - `Ctrl+B`, `Ctrl+J`, `Ctrl+\`, `Ctrl+P`, `Ctrl+Shift+P` behave the same from every focused surface

- [x] Make provider/model settings safe and truthful
  - Why: provider changes are persisted from reactive signals on every edit, the UI does not validate API key availability, and the settings UI only exposes a fixed hardcoded provider list.
  - Why: `provider_name_to_llm_provider()` falls back to Ollama for unknown names, which is too implicit.
  - Files:
    - `crates/phazeai-ui/src/app.rs`
    - `crates/phazeai-ui/src/panels/settings.rs`
    - `crates/phazeai-core/src/config/mod.rs`
    - `crates/phazeai-core/src/llm/provider.rs`
  - Done means:
    - provider selection reflects actual configured providers
    - unavailable providers are visibly disabled or flagged
    - unknown provider names do not silently map to Ollama
    - model/provider writes are debounced or explicitly saved

- [x] Add confirmation/undo safety to destructive Git actions
  - Why: discard is still a sharp edge and is implemented as `git checkout -- <path>`.
  - Files:
    - `crates/phazeai-ui/src/panels/git.rs`
  - Done means:
    - discard has confirmation
    - safer command is used where possible
    - bulk vs single-file destructive actions are clearly separated

## P1: Should fix before calling the AI IDE "flawless enough"

- [x] Fix MCP config resolution mismatch in chat
  - Why: chat currently loads MCP config from `current_dir()`, while composer uses the workspace root. That inconsistency will produce "works in one panel, missing in another" behavior.
  - Files:
    - `crates/phazeai-ui/src/panels/chat.rs`
    - `crates/phazeai-ui/src/panels/composer.rs`
  - Done means:
    - all AI entry points resolve MCP config from the same workspace root

- [ ] Add chat cancel, retry, and failure UX
  - Why: chat streams well enough, but there is no explicit cancel/retry path, and failures only show up by mutating the last message.
  - Files:
    - `crates/phazeai-ui/src/panels/chat.rs`
  - Done means:
    - cancel button or shortcut exists
    - retry last request exists
    - errors are visible as UI state, not just message text mutation

- [ ] Persist or intentionally scope chat/composer conversations
  - Why: there is no clear persistence or session model for the GUI AI surfaces. That is fine for a prototype but weak for daily use.
  - Files:
    - `crates/phazeai-ui/src/panels/chat.rs`
    - `crates/phazeai-ui/src/panels/composer.rs`
    - `crates/phazeai-core/src/context/persistence.rs`
  - Done means:
    - either conversations persist intentionally
    - or the UI clearly says they are ephemeral

- [ ] Reduce duplicate AI surfaces or define their roles
  - Why: `ai_panel` and `chat_panel` overlap conceptually and risk drifting into two inconsistent chat implementations.
  - Files:
    - `crates/phazeai-ui/src/panels/ai_panel.rs`
    - `crates/phazeai-ui/src/panels/chat.rs`
    - `crates/phazeai-ui/src/app.rs`
  - Done means:
    - one primary AI interaction surface is clearly canonical
    - the other is removed, renamed, or constrained to a distinct purpose

- [ ] Make composer safer
  - Why: composer is powerful and now MCP-aware, but it is still a broad "agent with bash" surface. For a beta, it needs better guardrails and visibility.
  - Files:
    - `crates/phazeai-ui/src/panels/composer.rs`
  - Done means:
    - explicit workspace shown
    - approval mode visible
    - tool runs and file diffs are easier to inspect
    - non-git workspaces degrade gracefully

- [ ] Clean up session persistence semantics
  - Why: session persistence is split across several reactive saves and still mostly handles layout/editor state manually.
  - Files:
    - `crates/phazeai-ui/src/app.rs`
  - Done means:
    - one session model owns load/save
    - defaults and migration are explicit
    - panel state, tabs, and theme restore consistently

## P2: Architecture debt that will keep causing bugs

- [ ] Break up `IdeState` and `app.rs`
  - Why: this is still the main composition root and the main coupling hotspot.
  - Files:
    - `crates/phazeai-ui/src/app.rs`
  - Suggested split:
    - app bootstrap
    - session persistence
    - sidecar integration
    - global actions/commands
    - overlays/pickers
    - panel layout state

- [ ] Make the repo map implementation honest
  - Why: `repo_map.rs` says tree-sitter-based extraction, but the implementation is regex-based.
  - Files:
    - `crates/phazeai-core/src/context/repo_map.rs`
  - Done means:
    - docs/comments describe the real implementation
    - or the implementation is upgraded

- [ ] Clarify sidecar terminology
  - Why: Rust-side naming says "semantic search" and "embeddings", but the Python server is currently a TF-IDF keyword-style index with no persistent embeddings.
  - Files:
    - `crates/phazeai-sidecar/src/tool.rs`
    - `crates/phazeai-sidecar/src/client.rs`
    - `sidecar/server.py`
    - `sidecar/README.md`
  - Done means:
    - the product either uses true embeddings
    - or the names/docs are corrected

- [ ] Improve MCP process diagnostics
  - Why: MCP server stderr is piped but not surfaced to users, so failures are likely to feel silent.
  - Files:
    - `crates/phazeai-core/src/mcp.rs`
    - `crates/phazeai-ui/src/panels/chat.rs`
    - `crates/phazeai-ui/src/panels/composer.rs`
  - Done means:
    - connection failures expose actionable errors
    - server launch stderr is visible somewhere useful

- [ ] Tighten extension expectations
  - Why: the extensions panel mixes native plugins and VS Code assets, but the actual capability set is narrower than users may infer from ".vsix support".
  - Files:
    - `crates/phazeai-ui/src/panels/extensions.rs`
    - `crates/phazeai-core/src/ext_host/*`
  - Done means:
    - supported extension capabilities are explicit in UI/docs
    - unsupported behaviors are not implied

## P3: Quality and product polish

- [ ] Add first-run readiness checks
  - Check:
    - active provider availability
    - missing API keys
    - sidecar availability
    - git workspace detection
    - missing language servers
  - Files:
    - `crates/phazeai-ui/src/app.rs`
    - `crates/phazeai-ui/src/panels/settings.rs`

- [ ] Add AI verification scenarios to GUI tests
  - Missing scenarios:
    - chat send with provider failure
    - chat send with mocked success
    - composer run/stop
    - sidecar index build and search results
  - Files:
    - `crates/phazeai-ui/tests/tier2_integration.rs`

- [ ] Add explicit non-git workspace behavior
  - Why: several flows assume git or use git-derived workspace state.
  - Files:
    - `crates/phazeai-ui/src/app.rs`
    - `crates/phazeai-ui/src/panels/git.rs`
    - `crates/phazeai-ui/src/panels/composer.rs`

- [ ] Audit silent fallbacks
  - Examples:
    - settings parse/load silently falling back to defaults
    - provider fallback to Ollama
    - sidecar "not connected" UI strings without next-step guidance
  - Files:
    - `crates/phazeai-core/src/config/mod.rs`
    - `crates/phazeai-ui/src/app.rs`
    - `crates/phazeai-ui/src/panels/settings.rs`

## Recommended execution order

1. Sidecar settings + index lifecycle
2. Sidecar ownership/shutdown cleanup
3. Provider/settings validation
4. MCP path consistency
5. Git safety
6. Global shortcut command layer
7. Chat/composer UX hardening
8. `IdeState` decomposition

## Definition of "beta good enough"

Before claiming the IDE is ready for regular use, all of the following should be true:

- The IDE starts without hidden dependency assumptions
- The configured AI provider is either usable or clearly marked unusable
- Chat works or fails with a clear reason
- Composer works inside the right workspace and shows what it changed
- Sidecar search either works end-to-end or is clearly disabled
- GUI shortcuts work regardless of focus
- Destructive Git actions require confirmation
- Session restore is consistent
- GUI regression tests remain green under Xvfb
