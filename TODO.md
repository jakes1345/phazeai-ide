# PhazeAI IDE — Master TODO

> **Mission**: Best open-source AI-native IDE. Local-first, all Rust, GPU-rendered.
> **Legend**: `[ ]` todo · `[~]` in progress · `[x]` done · `[!]` blocked · `[-]` dropped
> **Model**: MIT open-source core + paid PhazeAI Cloud (hosted AI credits, team features)

---

## 🔴 PHASE 2 — SHIP (Target: 4-6 weeks)
> **Definition of done**: Someone can download, install, and use this daily without losing work.

---

### BLOCK 1: Critical Editor Fixes (blocking daily use)

#### Undo / Redo
- [ ] Wire Floem text_editor undo (`Ctrl+Z`) and redo (`Ctrl+Shift+Z` / `Ctrl+Y`)
- [ ] Persist undo stack per document (survives tab switching — rope history per EditorId)
- [ ] Undo indicator in status bar ("Unsaved changes" disappears after undo-to-clean)

#### Find & Replace (Ctrl+H)
- [ ] Find input already exists — add Replace input below it
- [ ] "Replace" button: replace current match and advance
- [ ] "Replace All" button: replace every match in document
- [ ] Match count display ("3 of 12 matches")
- [ ] Case-sensitive toggle + regex toggle

#### Session Persistence
- [ ] Save open file paths to `~/.config/phazeai/session.toml` on quit
- [ ] Restore open tabs + active tab index on next launch
- [ ] Save/restore panel layout (left width, bottom height, right width)
- [ ] Save/restore editor scroll positions per tab

#### Completion Insertion
- [ ] On Enter/Tab in completion popup → insert `CompletionEntry::insert_text` at cursor
- [ ] Close popup after insertion
- [ ] Replace the word before the cursor (not just insert at position)
- [ ] Filter popup items as user types (prefix filter against `label`)

#### Diagnostic Gutter
- [ ] Render error/warning squiggle underlines in editor using LSP DiagEntry signal
- [ ] Show colored dot in gutter (red=error, yellow=warning) per affected line
- [ ] Problems panel: list all DiagEntry items, click → jump to file:line
- [ ] Status bar badge: show error count (E: 3) and warning count (W: 7) live

---

### BLOCK 2: UI Completeness

#### Panel Resize (drag divider)
- [ ] Left panel width: draggable divider between explorer and editor
- [ ] Bottom panel height: draggable divider above terminal/search area
- [ ] Right panel width: draggable divider between editor and chat
- [ ] Persist panel sizes to session.toml

#### Terminal
- [ ] Render blinking cursor caret at current PTY cursor position
- [ ] Terminal scrollbar: clickable + draggable to navigate scrollback
- [ ] Ctrl+Shift+C / Ctrl+Shift+V for copy/paste in terminal
- [ ] Multiple terminal tabs ("+") button spawns new PTY

#### Git Panel — Stage/Unstage
- [ ] Checkbox per file to stage: run `git add <path>` on check
- [ ] Checkbox to unstage: run `git reset HEAD <path>` on uncheck
- [ ] Click on file → show inline diff (run `git diff <path>`, display in split view)
- [ ] Diff view panel for staged vs unstaged changes

#### Search Panel — Real Implementation
- [ ] Wire `perform_search(query, workspace_root)` using `ripgrep` binary or `grep_lite` crate
- [ ] Display results: file path + line number + matched line preview
- [ ] Click result → open file at that line in editor
- [ ] Regex toggle, case-sensitive toggle
- [ ] Replace-in-files: input + "Replace All in Workspace" button

#### Settings Panel — Fix Font
- [ ] Actually apply font_size change to editor when stepper changes
- [ ] Font family picker: at minimum MonoLisa, Fira Code, JetBrains Mono, Cascadia Code presets
- [ ] Persist font settings to `~/.config/phazeai/settings.toml`
- [ ] Live preview: change font → editor updates without restart

---

### BLOCK 3: phazeai-cli Fixes

#### File Tree (remove the stub)
- [ ] Implement real ratatui file tree (use `tui-tree-widget` crate or hand-roll)
- [ ] Arrow keys navigate, Enter opens file into `/add` context
- [ ] `j`/`k` vim navigation, `o` to expand/collapse
- [ ] Show git status badges (M/A/D/?) next to file names

#### Tool Approval (remove auto-approve hack)
- [ ] Block the agent coroutine when approval_fn is called
- [ ] Show approval popup in TUI: tool name + params + y/n/a (always) / s (skip)
- [ ] `y` = approve once, `a` = approve all, `n` = deny, `s` = skip session
- [ ] Remove the "For now, auto-approve" comment and implement real blocking

#### Code Viewer
- [ ] `/view <file>` command: opens file in a scrollable pane with syntax highlighting
- [ ] Uses `bat`-style rendering via syntect in ratatui
- [ ] Arrow keys + PgUp/PgDn to scroll
- [ ] `q` to close

---

### BLOCK 4: Testing Infrastructure

#### phazeai-core tests
- [ ] Mock LLM client (`MockLlmClient`) that returns scripted responses/tool calls
- [ ] Agent loop test: send message → verify TextDelta events arrive in order
- [ ] Agent loop test: tool call → mock execute → verify ToolResult → verify next LLM call
- [ ] Context trimming test: add 200 messages → verify trim_to_token_budget leaves ≤N tokens
- [ ] ConversationHistory tests: add/get/clear/system prompt roundtrip

#### phazeai-ui tests (GUI snapshot tests)
- [ ] State unit tests: `IdeState` signal reads/writes (no GUI needed)
- [ ] Theme tests: verify all 12 variants produce valid Color values (no NaN, no out-of-range)
- [ ] LSP bridge tests: mock LSP events → verify DiagEntry signal gets updated correctly
- [ ] Completion insertion test: verify cursor offset math (byte offset → line/col → back)
- [ ] Session persistence test: write session.toml → reload → verify same tabs/layout

#### phazeai-cli tests
- [ ] Existing 57 command parser tests: keep passing ✅
- [ ] Add: TUI state tests (AppState transitions on key events)
- [ ] Add: conversation roundtrip (save → load → verify same messages)
- [ ] Add: token count estimation accuracy test

#### Integration tests
- [ ] Build test: `cargo build --workspace` must be warning-free
- [ ] `cargo clippy --workspace -- -D warnings` must pass
- [ ] `cargo fmt --all --check` must pass
- [ ] Run all with `cargo test --workspace` in CI

#### CI (GitHub Actions)
- [ ] `.github/workflows/ci.yml`: build + test on push/PR
- [ ] Matrix: ubuntu-latest, macos-latest, windows-latest
- [ ] Cache: `~/.cargo/registry` and `./target`
- [ ] Badge: show CI status in README

---

### BLOCK 5: Monetization Infrastructure

> Strategy: **MIT open-source core** (phazeai-ui, phazeai-core, phazeai-cli) +
> **Paid PhazeAI Cloud** (hosted AI credits, no API key needed, team features).
> Model: Cursor/Continue.dev approach — IDE is free forever, you pay for AI usage.

#### phazeai-cloud crate (skeleton exists, needs implementation)
- [ ] `CloudClient::login(email, password)` → POST /v1/auth/login → save CloudCredentials
- [ ] `CloudClient::verify_token()` → GET /v1/auth/me → return CloudSession
- [ ] `CloudClient::chat_stream(messages, model)` → POST /v1/chat/stream (SSE) → returns LlmClient impl
- [ ] `CloudClient::usage()` → GET /v1/usage → return credits_remaining, tokens_used_this_month
- [ ] Token credit deduction tracked server-side (not in IDE — prevents bypassing)
- [ ] `CloudLlmClient` struct: implements `LlmClient` trait, routes to our hosted model endpoint

#### Cloud auth UI in phazeai-ui
- [ ] Account panel: shows login form if unauthenticated, shows tier + credits if logged in
- [ ] "Sign in with PhazeAI Cloud" → opens browser to `https://app.phazeai.com/oauth`
- [ ] After OAuth callback → save token → update IdeState::cloud_session signal
- [ ] Show cloud status in status bar: "☁ Cloud · 4,231 credits" or "☁ Sign in"
- [ ] "Upgrade to Pro" button → opens browser to pricing page

#### Feature gating (what's free vs paid)
```
FREE (BYOK — bring your own key):
  - Full IDE (all panels, all features)
  - phazeai-cli (all slash commands)
  - Ollama/local models (unlimited)
  - Your own OpenAI/Claude/Groq keys

PHAZEAI CLOUD ($15/mo):
  - Hosted phaze-beast model (no API key needed)
  - 500,000 tokens/month included
  - Faster inference (priority queue)
  - One-click setup (no config needed for new users)

TEAM ($35/seat/mo):
  - Everything in Cloud
  - Shared conversation history (see what teammates asked)
  - Agent audit log (who ran what commands)
  - Shared workspace context (teammate's open files visible)
  - Team Modelfile sharing

ENTERPRISE (contact):
  - On-premise deployment
  - SSO (SAML/OIDC)
  - VPC model hosting
  - SLA + dedicated support
```
- [ ] `Tier::feature_check(feature: Feature) -> bool` in subscription.rs
- [ ] Gated features show lock icon + "Upgrade" tooltip instead of being hidden
- [ ] Trial mode: 7 days of Cloud tier free on signup (50,000 tokens)

---

### BLOCK 6: Release Prep

#### README
- [ ] Write real README.md: what it is, key features, install instructions, screenshots
- [ ] Record a 60-second GIF/video showing: file open → syntax highlight → chat with AI → terminal
- [ ] Badges: CI status, crates.io version, license (MIT), Discord

#### Distribution
- [ ] `cargo install phazeai-cli` works (publish to crates.io)
- [ ] Linux AppImage: `cargo build --release`, bundle into AppImage via `appimagetool`
- [ ] macOS DMG: universal binary (x86_64 + aarch64), signed if possible
- [ ] Windows MSI: via `cargo-wix` or GitHub Actions windows runner
- [ ] GitHub Releases: automated via `release.yml` workflow on `v*` tag push

#### Legal / Open Source
- [ ] Audit all dependencies for license compatibility (MIT/Apache-2.0 only)
- [ ] Add `LICENSE` file (MIT)
- [ ] Add `CONTRIBUTING.md`: dev setup, how to file issues, PR guidelines
- [ ] Privacy policy for PhazeAI Cloud (what data is stored)
- [ ] Terms of service for paid tiers

#### Community
- [ ] Set up Discord server: #general, #bugs, #feature-requests, #show-and-tell
- [ ] GitHub issue templates: bug report, feature request
- [ ] Product Hunt listing draft (schedule for launch day)
- [ ] HackerNews "Show HN" draft

---

## 🟡 PHASE 3 — GROWTH (After launch, priority order)

### High Priority (most-requested features)
- [ ] **Multi-cursor**: Ctrl+D to select next match, Alt+Click to add cursor
- [ ] **Breadcrumb bar**: shows `crate > mod > fn` above editor, clickable
- [ ] **Symbol outline panel**: tree of functions/structs/traits in current file via tree-sitter
- [ ] **Go-to-definition** (F12): LSP textDocument/definition → jump to file:line
- [ ] **Hover popup**: LSP textDocument/hover → show type info + docs on Ctrl+hover
- [ ] **Rename symbol**: F2 → LSP workspace/rename → apply edits across all files
- [ ] **Inline diff approval**: agent proposes edit → show before/after → Accept/Reject per hunk
- [ ] **Ghost text AI completions**: Tab to accept gray inline prediction (FIM request to LLM)
- [ ] **Ctrl+K inline AI edit**: select code → Ctrl+K → type instruction → AI rewrites in place
- [ ] **Format on save**: run `rustfmt`/`prettier`/`black` on Ctrl+S
- [ ] **Split editor**: Ctrl+\ to split right, Ctrl+- to split down, drag tabs between splits

### Medium Priority
- [ ] Word wrap toggle (Ctrl+Alt+Z)
- [ ] Zen mode: F11 → hide all panels, full screen editor
- [ ] Inlay hints: LSP textDocument/inlayHint → grayed type annotations inline
- [ ] Minimap: right-side code overview with viewport indicator
- [ ] Command palette improvements: recently used, fuzzy score ranking
- [ ] Tab bar overflow: scroll when > 10 tabs open
- [ ] File rename inline: F2 in explorer
- [ ] Drag-and-drop files in explorer to move them
- [ ] Split view for git diff (side-by-side before/after)
- [ ] Branch switcher UI in git panel: list branches, click to checkout
- [ ] Multiple terminal tabs
- [ ] `phazeai-cli`: real file tree (remove stub)
- [ ] `phazeai-cli`: real tool approval (remove auto-approve)

### phazeai-core improvements
- [ ] Web search tool (`brave_search` or `serper.dev` API)
- [ ] `run_tests` tool: detect test framework, run, parse output
- [ ] `git_commit` tool: stage + commit from agent
- [ ] `install_package` tool: `cargo add`, `pip install`, `npm install`
- [ ] Gemini provider (Google AI API)
- [ ] xAI / Grok provider
- [ ] DeepSeek direct API
- [ ] Provider fallback chain (primary → secondary on error)
- [ ] Cost estimation before long agent runs
- [ ] `.phazeai/instructions.md` auto-loaded as system context

---

## 🟢 PHASE 4 — SCALE (6+ months out)

- [ ] **Real-time collaboration** (CRDT): see teammate's cursor live
- [ ] **Remote SSH development**: open remote folder via SSH
- [ ] **devcontainer support**: open project in Docker container
- [ ] **Extension/plugin system**: WASM Component Model, sandboxed plugins
- [ ] **Integrated debugger**: DAP protocol, breakpoints, watch, stack trace
- [ ] **Notebook mode**: Jupyter-style code cells in editor
- [ ] **Voice control**: local Whisper model → agent commands
- [ ] **Browser-based version**: phazeai-web (Floem WASM target)
- [ ] **Mobile companion**: review diffs + chat from phone
- [ ] **LLM fine-tuning UI**: fine-tune local model on your codebase

---

## 💰 MONETIZATION REFERENCE

### Why Open Source + Paid Cloud works
- **VS Code** is open source → $0 → Microsoft monetizes via GitHub Copilot
- **Cursor** is ~$20/mo for AI features → 100k+ paying users
- **Continue.dev** is open source → enterprise support contracts
- **Zed** is open source → planning paid cloud sync

### Our model (already architected in `phazeai-cloud`)
```
Tier          Price     What you get
─────────────────────────────────────────────────────
Self-Hosted   FREE      Full IDE + CLI + BYOK (Ollama, OpenAI, Anthropic, etc.)
Cloud         $15/mo    Hosted phaze model, 500K tokens/mo, priority queue, no setup
Team          $35/seat  Cloud + shared context, audit logs, team Modelfiles
Enterprise    Contact   On-premise, SSO, VPC, SLA
```

### How to make money fast
1. Launch free tier → get GitHub stars + HN front page
2. Add "Get Started Free" + "PhazeAI Cloud $15/mo" CTA in README + IDE
3. Even 100 Cloud users = $1,500/mo recurring. 1000 = $15K/mo.
4. Model inference margin: ~70% (we pay ~$0.004/1K tokens wholesale, charge ~$0.015)

### What NOT to gate (would kill open source adoption)
- Never gate IDE features
- Never gate local model support
- Never gate the CLI
- Never require account to use

---

## ✅ CURRENTLY WORKING

- `cargo run -p phazeai-ui` — Floem IDE (primary GUI, ~47% complete)
- `cargo run -p phazeai-cli` — ratatui TUI (~70% complete)
- `cargo build --workspace` — all 5 crates compile clean
- `cargo test --workspace` — all tests passing
- LSP bridge: debounce + completions + cursor tracking ✅
- Git panel: status + commit ✅
- Terminal: PTY + VTE + 256-color + scrollback ✅
- Chat: real AI streaming via phazeai-core ✅
- Explorer: real file tree + git badges + context menu ✅
- Syntax highlighting: 25+ languages via syntect ✅
- 12 themes: MidnightBlue, Cyberpunk, Dracula, Tokyo Night, etc. ✅
- Command palette, file picker, Ctrl+P, Ctrl+G, Ctrl+F ✅

---

## 📊 PHASE 2 TASK TRACKING

| Block | Tasks | Done | % |
|-------|-------|------|---|
| Block 1: Editor critical | 16 | 0 | 0% |
| Block 2: UI completeness | 21 | 0 | 0% |
| Block 3: CLI fixes | 9 | 0 | 0% |
| Block 4: Testing | 18 | 0 | 0% |
| Block 5: Monetization | 14 | 0 | 0% |
| Block 6: Release prep | 14 | 0 | 0% |
| **Total Phase 2** | **92** | **0** | **0%** |
