# PhazeAI IDE — Master TODO

> **Goal**: Best AI-native IDE ever built. Local-first, all Rust, GPU-rendered.
> **Legend**: `[ ]` todo · `[~]` in progress · `[x]` done · `[!]` blocked · `[-]` dropped

---

## 🔥 FLOEM UI (phazeai-ui) — Phase 1: Wire Real Functionality

### Chat Panel → AI Integration
- [ ] Wire `create_ext_action` callback bridge to `phazeai-core` agent loop
- [ ] Stream AI tokens into chat messages in real-time (async → Floem signal)
- [ ] Show typing indicator / spinner while agent runs
- [ ] Stop/cancel button that kills the agent mid-stream
- [ ] Display tool call events inline (e.g. "Reading file: foo.rs...")
- [ ] Diff view popup when agent wants to edit a file (Accept / Reject)
- [ ] AI mode switcher (Chat / Ask / Debug / Plan / Edit tabs in chat header)
- [ ] Context include-file toggle (📎 button, shows current file path)
- [ ] @filename mention in chat input to include file context
- [ ] Chat history persistence (save/restore across sessions via JSON)
- [ ] Multi-conversation tabs in chat panel

### Explorer Panel
- [ ] Keyboard navigation in file tree (arrow keys, Enter to open)
- [ ] Right-click context menu (New File, New Folder, Rename, Delete, Copy Path)
- [ ] File rename inline (F2 or double-click)
- [ ] New file / new folder via keyboard shortcut
- [ ] Drag-and-drop files to move them
- [ ] File search within explorer (Ctrl+P fuzzy file picker)
- [ ] Show git status badges next to files (M, A, D, U icons)
- [ ] Configurable root folder (open folder dialog via `rfd`)
- [ ] Recent folders dropdown
- [ ] Exclude patterns (.gitignore aware, hide target/, node_modules/)

### Editor Panel
- [x] Load actual file content when switching tabs
- [x] Save file on Ctrl+S (write back to disk)
- [x] Dirty indicator (dot on tab when unsaved changes)
- [x] Line numbers gutter — themed via `gutter_dim_color` / `gutter_accent_color`
- [x] Syntax highlighting via syntect (base16-ocean.dark theme, 20+ languages)
- [x] Multiple editor tabs with independent editor state
- [x] Editor font size zoom (Ctrl+= / Ctrl+- / Ctrl+0 to reset)
- [ ] File type detection → apply correct language mode (tree-sitter for advanced features)
- [ ] Go-to-line (Ctrl+G)
- [ ] Find in file (Ctrl+F) using floem editor search API
- [ ] Find & Replace (Ctrl+H)
- [ ] Word wrap toggle
- [ ] Breadcrumb bar above editor showing file path
- [ ] Tab size setting (2 vs 4 spaces)

### Terminal Panel (new in phazeai-ui)
- [x] Build terminal panel using `portable-pty` + vte parser
- [x] PTY spawns real shell ($SHELL env var)
- [x] 256-color ANSI rendering (full xterm 256-color + truecolor)
- [x] Scrollback buffer (10k lines)
- [x] Direct PTY input — every keystroke forwarded with ANSI encoding (arrows, Tab, F1-F12, Ctrl+letter, Alt+key, etc.)
- [x] Tab completion, arrow-key history, interactive programs (vim, htop, python REPL) all work
- [ ] Multiple terminal tabs
- [ ] Copy/paste in terminal (Ctrl+Shift+C/V)

### Search Panel (new in phazeai-ui)
- [ ] Workspace search panel powered by ripgrep
- [ ] Search results list (file + line + preview)
- [ ] Click result → open file at that line in editor
- [ ] Regex toggle, case-sensitive toggle
- [ ] Replace-in-files workflow

### Git Panel (new in phazeai-ui)
- [ ] Show git status (modified, staged, untracked files)
- [ ] Stage/unstage individual files
- [ ] Commit message input + commit button
- [ ] Branch name in status bar (real `git rev-parse --abbrev-ref HEAD`)
- [ ] Inline diff for changed files (click file → see diff)

### Layout & UI
- [ ] Resizable panels (drag divider between explorer/editor)
- [ ] Bottom panel (terminal/problems/search) with toggle
- [ ] Panel collapse/expand animations
- [ ] Keyboard shortcut to toggle each panel (Ctrl+B, Ctrl+J, etc.)
- [ ] Tab bar overflow scrolling when many tabs open
- [ ] Split editor (horizontal and vertical)
- [ ] Status bar: show real git branch, real cursor position (Ln/Col)
- [ ] Status bar: show encoding (UTF-8), line ending (LF/CRLF)
- [ ] Command palette (Ctrl+Shift+P) - fuzzy search all actions

---

## 🎨 FLOEM UI — Phase 2: Polish & Features

### Theme System
- [ ] Theme picker in settings (MidnightBlue / Dark / Light / Catppuccin / Dracula)
- [ ] Custom theme JSON support (load from ~/.config/phazeai/themes/)
- [ ] Font picker UI (choose editor font family + fallbacks)
- [ ] Font size setting persisted to config
- [ ] High DPI / display scaling support
- [ ] Window transparency / blur (glassmorphism on Linux via compositor)

### Completions & Intelligence
- [ ] LSP integration via `phazeai-core/src/lsp/` (already exists!)
- [ ] Autocomplete popup in editor (trigger on typed chars / Ctrl+Space)
- [ ] Hover popup (show type info / docs)
- [ ] Go-to-definition (F12)
- [ ] Inline diagnostics (squiggly underlines for errors/warnings)
- [ ] Problems panel listing all LSP diagnostics
- [ ] Inlay hints (type annotations inline, grayed out)

### Ghost Text / AI Completions
- [ ] Ghost text completions (gray inline prediction, Tab to accept)
- [ ] Wire ghost text to phazeai-core LLM (fill-in-the-middle requests)
- [ ] Ctrl+K inline AI edit popup
- [ ] Alt+Enter shadow agent (quick one-shot AI edit of selection)

### Settings Panel
- [ ] Settings UI in phazeai-ui (editor font, theme, tab size, AI provider)
- [ ] AI provider configuration (API key input, model selector)
- [ ] Keybindings viewer/editor
- [ ] Modelfile editor (GUI for Ollama Modelfiles like in egui version)

---

## 🦀 phazeai-core — Agent & LLM Improvements

### Agent Loop
- [ ] Streaming tool call rendering (show partial tool calls as they stream)
- [ ] Tool approval UI hook (inject into phazeai-ui approval flow)
- [ ] Agent interrupt/pause (pause mid-run, review, resume)
- [ ] Persistent conversation history across sessions (save to disk)
- [ ] Agent memory (summarize old context, inject summary as system message)
- [ ] Multi-file context (send multiple open files as context)
- [ ] Agent "thinking" mode (extended reasoning via Claude / deepseek-r1)

### Tool System
- [ ] `read_file` — already exists, needs tree-sitter AST extraction option
- [ ] `search_symbol` — search for function/class def across workspace using LSP
- [ ] `run_tests` — detect test framework, run tests, parse results
- [ ] `web_search` — integrate a search API (Brave Search API / SerpAPI)
- [ ] `open_browser` — open URL in system browser
- [ ] `create_branch` — git branch creation tool
- [ ] `git_commit` — stage + commit with message
- [ ] `install_package` — cargo add / pip install / npm install
- [ ] `lint_fix` — run clippy/eslint/mypy and auto-apply fixes
- [ ] `run_lsp_format` — trigger LSP format document

### LLM Providers
- [ ] Add Gemini provider (Google Generative AI API)
- [ ] Add xAI / Grok provider
- [ ] Add Mistral provider
- [ ] Add Perplexity provider (good for search-augmented answers)
- [ ] Add DeepSeek direct API (not just via OpenRouter)
- [ ] Local model auto-pull UI (detect missing phaze-beast, offer to pull)
- [ ] Provider health check / ping on startup
- [ ] Fallback chain (if primary provider fails, try secondary)
- [ ] Cost estimation before long agent runs

### Context Management
- [ ] CRDT-based shared context (Phase 4 — deferred)
- [ ] `.phazeai/` project config (per-project AI instructions, tool whitelist)
- [ ] Auto-read CLAUDE.md / .phazeai/instructions.md as system context
- [ ] Workspace indexing (chunk files, embed, semantic search via sidecar)
- [ ] @codebase mention (include entire project summary as context)

---

## 🖥️ phazeai-ide (egui) — Bug Fixes & Polish

### Known Bugs
- [ ] Minimap sometimes renders with stale content after large edits
- [ ] Split editor divider can be dragged past panel boundaries
- [ ] Terminal: pasting multi-line text sometimes misorders lines
- [ ] Explorer: rename doesn't update open editor tab title
- [ ] Git blame hover popup flickers when moving mouse quickly
- [ ] Autocomplete popup doesn't dismiss on Escape in all cases
- [ ] Settings search bar doesn't filter keybindings tab
- [ ] Welcome screen sometimes appears after a project was already opened

### Editor
- [ ] Smart bracket closing (type `(` → inserts `()`, cursor inside)
- [ ] Smart quote closing (type `"` → inserts `""`)
- [ ] Surround selection (select text, type `(` → wraps selection)
- [ ] Emmet-style HTML tag expansion
- [ ] Snippet system (tab-triggered code templates)
- [ ] Auto-import (LSP code action for Rust / TypeScript)
- [ ] Multi-cursor: Ctrl+Alt+Down/Up to add cursor above/below
- [ ] Column select: full rectangular selection mode
- [ ] Zen mode (hide all panels, full-screen editor)

### AI Features
- [ ] "Explain this code" right-click → sends selection to Ask mode
- [ ] "Fix this error" on LSP diagnostic → sends error to Debug mode
- [ ] Agent runs show token count + cost estimate when complete
- [ ] Save agent conversation as markdown file
- [ ] Replay last agent run (re-run from history)
- [ ] Diff review: keyboard shortcuts (A=accept hunk, D=deny, N=next hunk)

### Terminal
- [ ] OSC 8 hyperlinks (clickable file paths in terminal output)
- [ ] Terminal search (Ctrl+F inside terminal)
- [ ] Session name (double-click tab to rename terminal tab)
- [ ] Send selection to terminal (highlight code, Ctrl+Enter)
- [ ] Terminal profile selector (bash/zsh/fish/nushell)

---

## 📦 phazeai-cli — TUI Improvements

- [ ] Fix conversation persistence (currently loses history on quit)
- [ ] Syntax highlighting in CLI output (code blocks)
- [ ] Image attachment support (pipe image path, send to vision models)
- [ ] Streaming tool call display (show tool name while agent runs)
- [ ] `/model` command to switch model mid-session
- [ ] `/context` command to show current token count
- [ ] `/save` command to export conversation to markdown
- [ ] Tab completion for slash commands
- [ ] Mouse support (clickable links in output)
- [ ] Configurable color theme for TUI

---

## 🏗️ Infrastructure & DevOps

### Build & Release
- [ ] Cargo workspace version management (single version bump script)
- [ ] Reproducible builds (pin all git deps to exact SHAs — already done for Floem)
- [ ] Binary size optimization (LTO, strip, opt-level = "z" for release)
- [ ] Cross-compilation: build Linux binary from macOS CI
- [ ] ARM64 builds (Apple Silicon native, Raspberry Pi)
- [ ] Windows ARM64 build
- [ ] Auto-update mechanism (check GitHub releases, prompt user)
- [ ] Crash reporter (capture panic backtraces, offer to send report)

### Testing
- [ ] Add 20 more integration tests for editor (selection, copy/paste, undo/redo edge cases)
- [ ] Terminal emulator tests (feed ANSI sequences, assert rendered output)
- [ ] Agent loop tests (mock LLM, test tool call parsing)
- [ ] LSP client tests (mock language server, test request/response)
- [ ] Snapshot tests for theme rendering
- [ ] Fuzzing the VTE parser with arbitrary byte sequences
- [ ] Performance benchmark: time-to-first-render on 10k line file

### Documentation
- [ ] README.md: feature list, screenshots, install instructions
- [ ] CONTRIBUTING.md: dev setup, architecture overview
- [ ] User docs site (mdBook or Docusaurus)
- [ ] Architecture decision records (ADR) in docs/
- [ ] In-app help panel (? key opens keyboard shortcut cheatsheet)
- [ ] Video demo / GIF of key features for README

### Distribution
- [ ] AUR package (Arch Linux — PKGBUILD)
- [ ] Homebrew formula (macOS — tap + formula)
- [ ] Snap package (Ubuntu Snap Store)
- [ ] Winget package (Windows Package Manager)
- [ ] GitHub Releases automation (tag → build → upload artifacts)
- [ ] Docker image for running headless agent in CI (phazeai-cli)

---

## 💰 Monetization & Product

- [ ] PhazeAI Cloud account system (sign up, API key management)
- [ ] Hosted phaze-beast model endpoint (pay-per-token, faster than local)
- [ ] Pro tier features (unlimited cloud AI, team workspaces, priority support)
- [ ] License key validation for Pro features
- [ ] Landing page update with feature screenshots + pricing
- [ ] Discord community setup
- [ ] Product Hunt launch preparation
- [ ] Open source license audit (MIT-clean for everything in phazeai-ui, egui IDE)
- [ ] Privacy policy + terms of service

---

## 🧪 Experimental / Future

- [ ] CRDT collaboration (multiple devs editing same file live) — Phase 4
- [ ] Voice control (speech-to-text → agent command)
- [ ] AI-powered test generation (select function → generate tests)
- [ ] AI code review (PR diff → inline review comments)
- [ ] Notebook mode (Jupyter-style cells in editor)
- [ ] Plugin/extension system (Lua or WASM plugins)
- [ ] Remote development (SSH into server, edit files remotely)
- [ ] Container development (open folder in Docker devcontainer)
- [ ] Language server protocol: implement PhazeAI as an LSP server (use IDE features inside vim/neovim)
- [ ] Browser-based version (phazeai-web using egui WASM or Floem WASM target)
- [ ] Mobile companion app (review diffs, chat with agent from phone)

---

## Currently Compiling / Working ✅
- `cargo run -p phazeai-ide` — full egui IDE with all Phase 1-4 features
- `cargo run -p phazeai-ui` — new Floem IDE (activity bar, explorer, editor, chat, status bar)
- `cargo run -p phazeai-cli` — terminal UI
- `cargo test --workspace` — 12 tests passing
- Rust 1.93.1, Floem rev e0dd862

---

## 🏛️ GPU/Vello Rendering (phazeai-ui)

- [ ] Aura Syntax: GPU-based glowing keywords (pub, await) via Vello bloom without blurring text
- [ ] Ghost Snapshots: Translucent Rope overlays showing previous version of a line for instant revert
- [ ] Neon Scrollbar Heatmap: diagnostics rendered as glowing pulses on the scrollbar via custom canvas
- [ ] Fluid Panel Transitions: Lerp panel widths using Floem animated signals for "living" workspace
- [ ] Parallax Layers: Background patterns that move slower than code during scroll for 3D depth
- [ ] GPU Shape Morphing: Tabs that morph between square/rounded shapes based on activity
- [ ] Dynamic Glass Distortion: Panels that slightly warp background texture when dragged
- [ ] Minimalist HUD: Alt-key overlay showing only LSP status + Git branch essentials
- [ ] Tab "Burn-in" Effect: Frequently used tabs glow slightly brighter than others (track usage count)
- [ ] Zen Smoke: Subtle "fog" effect that clears as you type, indicating flow state
- [ ] Animated Gutter Glyphs: Git status icons with 0.2s pop animation on change
- [ ] High-Contrast "Hack" Mode: Theme with max contrast + vector-glow borders
- [ ] Vello Path Indicators: Glowing line connecting source and target when following function calls
- [ ] Subline Spans: Visual indicators between lines showing logic block boundaries
- [ ] Contextual Scaling: UI elements scale up/down based on mouse proximity
- [ ] Vector minimap: High-fidelity minimap rendering exact shapes of code blocks via Vello
- [ ] Custom Scroll Curves: User-defined inertia for scroll wheel in native Rust
- [ ] Glassmorphic Tabs: Fully transparent tabs with blurred backdrop
- [ ] Animated Breadcrumbs: Smooth sliding breadcrumbs when switching file contexts
- [ ] Spectrum Identifiers: Variable names auto-tinted with subtle gradients based on type/scope
- [ ] Occlusion-Aware Overlays: UI panels that intelligently dodge the cursor
- [ ] Haptic Cursor Snap: UI "resistance" when dragging selection to semantic boundaries
- [ ] Chromatic Aberration Warnings: Color-fringe effect on screen edge during critical build errors
- [ ] Depth-of-Field View: Blur out-of-focus panels while active stays pin-sharp
- [ ] Stencil Outlines: Glowing silhouettes around the active function/block
- [ ] Real-time Palette Lerping: Smoothly shifting colors as sun sets (time-of-day adaptive themes)
- [ ] Vector Icon Morphing: Activity bar icons that fluidly transition between states
- [ ] Magnetic Panels: Panels snap together with physical "gravity-well" animations
- [ ] Type-Driven Glow: Structs, Enums, Traits each have unique subtle glow signature
- [ ] Adaptive UI Density: Auto-increase padding/font-size when IDE detects "reading mode"
- [ ] GPU Sparklines: Live-updating perf graphs embedded in status bar
- [ ] Multi-Stage Undo Visualization: Visual "stack" showing undo history as vertical deck of cards
- [ ] Vector Border Pulsing: UI borders that "breathe" with neon energy during expensive computations
- [ ] LSP-Driven Motion: Code blocks that slightly "shimmy" if they have a pending fix/suggestion
- [ ] Haze Selection: Selections appear as soft glowing cloud behind text instead of solid block
- [ ] Instant UI Hot-Reload: Edit IDE CSS/Layout in real-time without restart

---

## 🤖 Agentic AI Features (phazeai-ui)

- [ ] Sentient Gutter: Canvas pulse showing exactly where Agent is reading/thinking
- [ ] AI Comment Injection: `// /refactor` inline comments directly parsed by IDE core
- [ ] Self-Healing Terminal: Non-zero exit codes trigger "Fix with Phaze" overlay
- [ ] Contextual Documentation: Markdown-rendered docs for symbol under cursor, AI-synthesized
- [ ] Review Ghosting: Agent shows proposed fixes as translucent ghost text before acceptance
- [ ] Predictive Discovery: AI proactively opens docs in side-pane based on imports
- [ ] Conflict Mediator: Agent suggests synthesized version of Git merge conflicts in 3-way view
- [ ] Silent Reviewer: Background agent scans every Save for logic flaws, highlights in purple
- [ ] Omniscient Search: AI-powered project search ("where is the token stored?") vs regex
- [ ] Architecture Guard: Agent warns if you break project architectural patterns
- [ ] Snippet Architect: Agent combines existing functions to draft a new one
- [ ] Auto-commit messages: Intelligent per-file commit messages written by Coder Agent
- [ ] Dependency Scout: Suggests best crate when you type `// need a db...`
- [ ] Approval Loops: One-click "Approve" buttons for Agent-proposed terminal commands
- [ ] Agent Event Stream: Live log of everything AI has done/seen during session
- [ ] Local LLM Toggle: One-click switch between cloud and local Ollama
- [ ] Prompt Chaining UI: Visual editor to link AI steps into a complex Workflow
- [ ] Automatic TODO tracking: Agent syncs `// TODO` comments to project sidebar
- [ ] Project Archeology: Agent highlights "oldest" code to suggest debt cleanup
- [ ] Style Mimic: Agent learns your naming/spacing habits and auto-corrects to match
- [ ] Agentic Test Runner: AI decides which tests to run based on changed code paths
- [ ] Intent-Based Refactor: "Refactor this to be more memory efficient" triggers specialized prompt
- [ ] Semantic Code Fold: AI automatically folds code to show only "high-signal" logic
- [ ] Prompt Mirroring: See exact prompt being sent to LLM for full transparency
- [ ] Agent Feedback Score: Rate-limit or boost Agent aggressiveness based on preference
- [ ] Shadow Code Review: Agent simulates "Reviewer" and leaves comments before you open PR
- [ ] Automated Readme Sync: Agent updates README.md as you add new public APIs
- [ ] Deep Context Injection: "Pin" relevant files to give Agent specific context for a task
- [ ] Agent-Managed Tasks: Kanban board that Agent updates as you complete features
- [ ] Crate Intelligence: Agent reads source of dependencies to help use undocumented traits
- [ ] Commit "Risk" Analysis: Agent flags commits touching high-risk areas (Auth, DB Migrations)
- [ ] LSP-to-LLM Bridge: Agent uses compiler errors to iterate on fixes until build passes
- [ ] Interactive Debugger Chat: Chat with Agent while at a breakpoint to analyze the stack
- [ ] Auto-Mock Generation: Agent builds mocks for external services during test suites
- [ ] Dead-Code Cleanup Agent: Background task proposing deletion of verified unreachable exports
- [ ] Intelligent Porting: "Port this Python logic to Rust" using PhazeAI native SDK patterns
- [ ] Agent Personality Selector: Switch between "Hacker (concise)" and "Architect (verbose)" styles
- [ ] Proactive Bug Hunter: Agent attempts to "break" your new function with edge-case tests
- [ ] One-Click "Cleanup": Agent wipes all debugging `println!` calls when ready to commit

---

## ⌨️ PTY & Terminal Advanced (phazeai-ui)

- [ ] Hacker Terminal (Matrix Rain): Vello character rain falling behind live terminal text
- [ ] Command Rewind: Scrubber to see past terminal output exactly as it appeared
- [ ] Visual PTY Pipes: Glowing lines showing data flow between open terminal tabs
- [ ] Terminal Error Overlays: Floating glass panes explaining Errno/compiler failures in-place
- [ ] Shell Snapshot: Save current shell env/history and restore it later
- [ ] Live Log Filtering: Agent-driven real-time terminal log filtering
- [ ] Integrated PTY Image View: Render images directly in terminal using GPU canvas
- [ ] Terminal Macro Recorder: Record a CLI sequence and have Agent "humanize" it
- [ ] Tabbed Terminal Groups: Group terminals by "Backend", "Frontend", etc.
- [ ] Remote SSH PTY: High-performance native SSH client built into IDE
- [ ] Visual Environment Variable Editor: Table UI for managing .env files
- [ ] Terminal Search-and-Replace: Advanced search inside terminal output
- [ ] GPU-Powered Terminal Scroll: Smooth 144Hz scrolling with millions of lines
- [ ] Command Autocomplete Popups: Neon-bordered overlays for bash/zsh completions
- [ ] Process Tree Visualizer: Graphical map of sub-processes spawned by terminal
- [ ] Terminal Output Diffing: Select two terminal runs and see exact output differences
- [ ] Interactive CLI Prompt: AI suggests shell commands as you start typing (like fig but native)
- [ ] Terminal Theme Sync: Auto-matching PTY colors to IDE's PhazeTheme
- [ ] Detached Terminal Panes: Drag terminal tab to floating "Always-on-top" window
- [ ] Visual Port Monitor: List all active TCP/UDP ports with "Kill" buttons
- [ ] Terminal Character Glow: Characters glow based on ANSI color intensity
- [ ] Output "Collapse" Blocks: Auto-fold repetitive terminal lines (identical warnings)
- [ ] Terminal-to-Chat Pipe: Drag terminal output chunk to chat panel to ask "WTF is this?"
- [ ] Command "Duration" HUD: See exactly how long each terminal command took
- [ ] Terminal Background "Noise": Low-opacity animated noise patterns to reduce eye strain

---

## 🔍 Debugging & Verification (phazeai-ui)

- [ ] Time-Travel Diff: Scrub file history like a movie using Git/Ropey
- [ ] Macro Expansion Viewer: Live side-pane showing `cargo expand` output for current file
- [ ] Performance Flamegraph: Integrated real-time profiling UI using GPU rendering
- [ ] Live Data Flow: Visualize variable values changing in real-time next to code
- [ ] Heap Visualizer: See memory allocations for a Rust block as a 2D map
- [ ] Trace Navigator: Arrows linking function calls across multiple files during trace
- [ ] Dead Code Ghosting: Unused code fades out in the UI
- [ ] Vulnerability Scanner: Real-time flagging of insecure Rust patterns (unsafe blocks, leaks)
- [ ] Auto-test Generation: One-click `#[test]` module generation for any file
- [ ] Breakpoint Overlays: Floating indicators showing hit counts for active breakpoints
- [ ] Crate Dependency Map: Visual graph of all imported crates and their versions
- [ ] Panic Backtrace Visualizer: Clean, clickable UI for Rust panics
- [ ] Trait Implementation Tracer: See where a trait is implemented in a tree view
- [ ] Visual Reference Counting: Indicators showing Arc/Rc count variations during execution
- [ ] Lifetime Visualizer: Highlight start/end of variable's lifetime with glowing lines
- [ ] Pattern Match Checker: Agent flags missing enum variants before compile
- [ ] Code "Hotness" Map: Highlight functions called most frequently in trace
- [ ] Visual Borrow Checker: Translucent overlays showing which reference holds a lock
- [ ] Test Coverage Heatmap: Fade code lines not hit by current test suite
- [ ] Panic Recovery Guide: Agent provides "Fix This Panic" button with one-click patch
- [ ] Struct Layout Visualizer: See memory alignment and padding of Rust structs
- [ ] Unsafe Block Audit: Dedicated dashboard listing every `unsafe` block for review
- [ ] Secrets Detector: Highlight API keys and passwords in red before they get staged
- [ ] Pre-Commit Security Scan: Agent auto-runs `cargo-audit` and `cargo-deny` before every commit

---

## 🤝 Productivity & Workflow (phazeai-ui)

- [ ] Neural Map (Project Graph): Multi-dimensional project structure view rendered in Vello
- [ ] One-Click Deploy: Status bar buttons for CI/CD tasks controlled by Agent
- [ ] Project Heartbeat: Real-time coding velocity graph in status bar
- [ ] Refactor Preview: See "vision" of refactored code before applying
- [ ] Symbol Radar: Directional indicators to symbols related to current code
- [ ] Focus Pinning: Floating function windows that stay visible while scrolling
- [ ] Smart Bookmarks: Context-aware markers the Agent uses as "anchors"
- [ ] Unified Search: Single bar for code, docs, terminal, and AI memory
- [ ] Git Branch Timeline: Visual horizontal bar showing branch history and HEAD position
- [ ] Code "Snippets" Canvas: Side-pane where you drag-and-drop chunks of code to "save" them
- [ ] Active Task HUD: Corner overlay showing current sub-task Agent is assisting with
- [ ] Visual Command Palette: Command palette with rich icons and "Last Used" logic (Ctrl+Shift+P)
- [ ] Integrated Issue Tracker: View and update GitHub/GitLab issues from IDE
- [ ] Pair-Programming Presence: See teammate's cursor and "active selection" in real-time
- [ ] Workspace Snapshot: One-click save of all open tabs/panels/terminals to a named "Session"
- [ ] "Focus" Filter: Auto-hide all project files not related to current Git branch
- [ ] Keyboard Shortcut Visualizer: Graphical overlay showing most-used keybindings
- [ ] Intent-Based Navigation: "Show me the entry point for the API" jumps editor
- [ ] Multi-File Selection Sync: Select files in explorer, open as "Grid" view
- [ ] Project Statistics Dashboard: Lines of code, complexity, test coverage, docs percentage
- [ ] Custom "Context" Hub: Panel where you pin Files/Docs/Links for current feature
- [ ] Interactive Git Log: Clicking a commit opens diff and lists files affected
- [ ] Scratchpad Tab: Auto-saving temporary file for quick notes/snippets
- [ ] Project "To-Do" Heatmap: Visualize which folders have most unaddressed TODO comments
- [ ] Dynamic Layout Switching: Auto-switch between "Coding", "Debugging", "Research" layouts
- [ ] Curation Sidebar: List of "Favorite" functions/structs for quick access

---

## 🛡️ Security & Ops (phazeai-ui)

- [ ] Air-Gap Mode: Disable all telemetry/remote-calls with one physical status indicator
- [ ] Audit Trail: Cryptographic log of every human/agent action in the workspace
- [ ] Network Monitor: Integrated view of outbound calls made by the code you're writing
- [ ] Sandboxed Run: One-click project execution in restricted container
- [ ] Permission Manager: Grant Agent specific permissions (File access, Shell, Web)
- [ ] Phaze-Vault Integration: Built-in manager for SSH keys and environment secrets
- [ ] Dependency Tree Lockdown: Highlight any dependency with "Impure" source (e.g. git links)
- [ ] Visual Firewall: List of all active network sockets opened by IDE process
- [ ] Private AI Gateway: Route all LLM traffic through local anonymizing proxy
- [ ] One-Click "Clean Wipe": Wipe all build artifacts, caches, and secrets from local disk

---

## 🔌 Native Extension SDK

- [ ] PhazePlugin trait: `on_load`, `register_panels`, `register_ai_tools`, `on_unload` lifecycle
- [ ] PluginContext: Access to IdeState (Theme, Open Files, Workspace) + AgentManager + LspManager
- [ ] Phase 1: Static plugins compiled directly into phazeai-ui binary for core features
- [ ] Phase 2: WASM Component Model (Wasmtime) — sandboxed `.wasm` drag-and-drop plugins
- [ ] Phase 3: Plugin Registry — decentralized plugin fetch/update without corporate gatekeeper
- [ ] Plugin UI: Extensions render directly to GPU Canvas via Floem `impl IntoView`
- [ ] Agentic Plugin Hooks: Pre-Plan, Review, and Tool Injection hooks for AI pipeline

---

## 🚀 Future / Experimental

- [ ] Voice-to-Command: Local Whisper model for high-speed voice editing
- [ ] 3D Project Diorama: Visualize project structure in VR/AR space
- [ ] Integrated LLM Trainer: Fine-tune local model on project codebase directly
- [ ] Neural Code Generation: Generate entire crates from a 10-paragraph spec
- [ ] Autonomous Repo Maintenance: Agent handles deps, security fixes, docs while you sleep
- [ ] IDE-to-Cloud Mirror: Real-time sync of IDE state to secure cloud backup
- [ ] Collaborative Agent-to-Agent: Your IDE Agent talking to teammate's Agent for sync
- [ ] Universal Code Translator: Instant translation between languages maintaining Rust logic patterns
- [ ] Browser-based version: phazeai-web using Floem WASM target
