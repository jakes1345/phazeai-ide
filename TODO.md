# PhazeAI IDE — Rebuild Roadmap

> Goal: Best open-source AI-native IDE since opencode.
> Architecture: Keep `phazeai-core` (solid). Gut and rebuild `phazeai-ide` panels.

---

## Legend
- `[ ]` Not started
- `[~]` In progress
- `[x]` Done
- `[!]` Blocked

---

## Phase 1 — Real Editor + Real Terminal
> Foundation everything else depends on.

### Editor — Text Editing Fundamentals
- [x] Implement text selection model (`Selection { anchor, cursor }` with `TextPosition { line, col }`)
- [x] Mouse selection (click to place cursor, click+drag to select)
- [x] Keyboard selection (Shift+Arrow, Shift+Home/End, Ctrl+Shift+Arrow)
- [x] Select all (Ctrl+A)
- [x] Clipboard: Copy (Ctrl+C), Cut (Ctrl+X), Paste (Ctrl+V) via `arboard` crate
- [x] Word-level navigation: Ctrl+Left/Right word jump
- [x] Word-level deletion: Ctrl+Backspace (delete word left), Ctrl+Delete (delete word right)
- [x] Forward delete (Delete key)
- [x] Auto-indent on Enter (match previous line's leading whitespace + smart home)
- [ ] Basic bracket matching (highlight matching `()`, `[]`, `{}`) — Phase 2 with tree-sitter

### Editor — Undo/Redo Overhaul
- [x] Replace string-snapshot undo with Rope-clone undo (O(log n) clone, not O(n) String copy)
- [x] Group rapid edits into single undo entries (coalesce by edit count)
- [x] Undo/redo restores selection state correctly

### Editor — Syntax Highlighting Fix
- [x] Cache syntect `ParseState` per line (stateful multi-line highlighting — block comments, strings work)
- [x] Invalidate only changed lines on edit (`highlight_cache.invalidate_from(line)`)
- [x] Fix cursor positioning (use char-based column math, not pixel hardcode)
- [x] Cursor drawn as a 2px bar at exact column position
- [x] Tab character rendering (render as N spaces)

### Terminal — Real Emulator
- [x] Add `vte` crate for VT100/ANSI escape sequence parsing
- [x] Implement `TermState` — line buffer with colored segments (vte-based)
- [x] Process PTY output through `vte::Parser` → colored `TermLine` segments
- [x] Render lines with per-segment colors using `LayoutJob`
- [x] Implement Ctrl+C / Ctrl+D / Ctrl+L / Tab passthrough to PTY
- [x] Input history (Up/Down arrows)
- [x] Scrollback buffer capped at 10,000 lines
- [x] Shell spawned with `TERM=xterm-256color` / `COLORTERM=truecolor`
- [x] PTY resize on panel resize — `pty_master.resize(PtySize{cols,rows})` called when panel dimensions change
- [x] Multiple terminal tabs — tab bar with +/× buttons, each tab is a full `TerminalSession`

### Infrastructure
- [x] Move file I/O off UI thread (use `tokio::fs` + channel for results)
- [x] Wire `notify` crate for real filesystem watching (replace 2-second polling)
- [x] Deduplicate explorer directory-loading logic (extracted shared function)
- [x] Basic integration tests in `crates/phazeai-ide/tests/` (12 tests passing)

---

## Phase 2 — Language Intelligence
> Make the editor feel like a real IDE.

### LSP Integration
- [x] Implement LSP JSON-RPC client in `phazeai-core/src/lsp/` (spawn lang server, handle requests/responses)
- [x] Auto-detect and spawn language servers (rust-analyzer, pyright, typescript-language-server, etc.)
- [x] Wire diagnostics into editor: underline errors/warnings, gutter icons (⚠ ✗)
- [x] Hover information popup (show type/docs on mouse hover with 500ms delay)
- [x] Go-to-definition (F12)
- [x] Find references (Shift+F12)
- [x] Basic autocomplete popup (trigger on Ctrl+Space)
- [x] Document formatting (Shift+Alt+F)

### Tree-sitter Integration
- [x] Replace syntect with tree-sitter for syntax highlighting — tree-sitter used for Rust files (24 highlight types, semantic coloring); syntect used as fallback for other languages
- [x] Map tree-sitter node types to `ThemeColors` syntax color fields — `ts_color()` maps 24 capture types to theme colors
- [x] Incremental re-parsing on edit (highlight_cache.invalidate_from per-line — already done)
- [x] Bracket matching using tree-sitter structure (cursor-scan implementation, language-agnostic)
- [x] Code folding markers (collapse functions, blocks) — gutter triangles, click to fold/unfold
- [x] Symbol outline panel (list functions/classes/structs in current file) — Outline tab in left panel

### Workspace Search
- [x] Workspace-wide search panel (ripgrep-powered, `rg` with grep fallback)
- [x] Search results list: file path + line number + preview
- [x] Click result → open file at that line
- [x] Regex mode toggle
- [x] Case-sensitive toggle, file glob filter
- [x] Replace in all files (with preview before apply)

### Git Integration UI
- [x] Git status icons in explorer (M=modified, A=added, ?=untracked, D=deleted)
- [x] Gutter decorations in editor (added/modified/deleted line indicators)
- [x] Basic diff viewer panel (unified or side-by-side)
- [x] Commit panel (stage files, write message, commit)
- [x] Git blame inline (show commit + author on hover)

---

## Phase 3 — AI-Native Integration
> The differentiator. This is what beats opencode.

### AI Modes
- [x] Implement `AiMode` enum: `Chat`, `Ask`, `Debug`, `Plan`, `Edit`
- [x] Mode switcher UI in chat panel with icons + hover tooltips
- [x] Auto-context collection per mode:
  - Chat: no auto-context
  - Ask: current file + selection
  - Debug: current file + last 50 lines of terminal output
  - Plan: project tree + current file
  - Edit: current file + instruction
- [x] Per-mode system prompts wired into agent construction
- [x] Per-mode tool restrictions (Ask = read-only, Edit = all tools, etc.)

### Inline AI Editing (The Killer Feature)
- [x] When agent uses `write_file`/`edit_file` → intercept, show unified diff with color coding, require approval
- [x] Allow / Deny buttons with diff preview window
- [x] Full `similar`-based unified diff with green=added, red=removed rendering
- [x] Per-hunk Accept / Reject (checkbox per hunk, Apply Selected / Accept All / Deny)
- [x] "Apply to Editor" button on code blocks in chat

### Inline Chat (Ctrl+K Equivalent)
- [x] Ctrl+K popup in editor area, hint text changes per mode
- [x] Streams AI response with diff-style coloring
- [x] Uses Edit mode system prompt + current file context
- [x] Esc to close, fresh conversation each invocation

### Context Management
- [x] "Include current file" toggle in chat panel (📎 button)
- [x] File attachment UI (drag from explorer or @filename in chat)
- [x] Token usage meter
- [x] Smart context pruning

### Agent-IDE Event Integration
- [x] When agent edits a file → force-reload affected tab immediately
- [x] Status bar notification when agent runs/completes tools
- [x] "Apply to Editor" button on code blocks in chat
- [x] When agent runs bash → stream output to terminal panel
- [x] Plan mode → structured checklist display

### Multi-Agent Orchestration UI
- [x] Agent mode indicator in status bar (current AI mode)
- [x] Streaming spinner in status bar while agent runs
- [x] Cancel running agent (Stop button in chat)
- [x] Agent history (🕐 History button in chat, popup shows last 20 runs)

---

## Phase 4 — Polish & Ship
> Make it feel complete.

### Editor Polish
- [x] Split editor views (horizontal split, draggable separator, View menu toggle)
- [x] Minimap (downscaled view of file, click to scroll)
- [x] Breadcrumb navigation bar (File > Module > Function)
- [x] File tabs: drag-to-reorder (swap on drag), middle-click to close
- [x] Find & Replace panel (Ctrl+H) with regex support
- [x] Multi-cursor editing (Alt+Click, Ctrl+D for select-next-occurrence)
- [x] Column/block selection (Alt+drag)

### IDE Polish
- [x] Persistent window state (save/restore panel sizes, open folder, AI mode)
- [x] Command palette: AI mode switching, Search, Inline Chat actions
- [x] Settings editor with search (filter bar, keybindings reference tab)
- [x] Keyboard shortcut reference UI (table in settings, all 26 bindings shown)
- [x] Welcome screen / onboarding for new users

### CLI Polish
- [x] Proper mode switching in CLI (planning mode, debug mode via slash commands)
- [x] Better conversation list UI (`/conversations` as scrollable list)
- [x] Token + cost display in status bar (already tracked, needs display)
- [x] Clipboard paste support in TUI input

### Remove / Replace
- [x] Browser panel replaced with docs viewer (quick links, URL bar, code block rendering)
- [x] Remove fake `clean_html_to_markdown` function

### Infrastructure & Packaging
- [x] Error handling audit (no `.unwrap()` in IDE code — use proper error recovery)
- [x] Performance profiling pass — 6 fixes applied: LayoutJob cache (skip syntect per-frame), bracket_match cursor cache, fold_hidden generation cache, match_lines HashSet eliminated, double line_text() call fixed, syntect walk budget-capped at 500 lines/frame
- [x] AppImage / Flatpak packaging for Linux (`scripts/build-appimage.sh`, `scripts/build-deb.sh`, `packaging/flatpak/`)
- [x] .dmg packaging for macOS (`packaging/macos/build-dmg.sh`)
- [x] .msi packaging for Windows (`packaging/windows/phazeai-ide.wxs`, `build-msi.ps1`)
- [x] CI/CD: GitHub Actions (build, test, lint on push)
- [x] Release workflow (semver tags → build artifacts)

---

## Completed
_(move items here as they're done)_

---

## Notes

### Key Files
- Agent loop: `crates/phazeai-core/src/agent/core.rs`
- Provider registry: `crates/phazeai-core/src/llm/provider.rs`
- Tool system: `crates/phazeai-core/src/tools/`
- App orchestration: `crates/phazeai-ide/src/app.rs`
- Editor panel: `crates/phazeai-ide/src/panels/editor.rs`
- Chat panel: `crates/phazeai-ide/src/panels/chat.rs`
- Terminal panel: `crates/phazeai-ide/src/panels/terminal.rs`
- Theme system: `crates/phazeai-ide/src/themes.rs`

### New Crates/Deps Needed
- `arboard` — clipboard (cross-platform)
- `vte` — VT100/ANSI parser for terminal emulator
- `tree-sitter` + grammars — replace syntect for highlighting
- `lsp-types` (already in workspace) — wire up LSP client
- `similar` (already present) — diff rendering for inline edits
- `notify` (already in workspace) — replace polling file watcher

### Architecture Decisions
- Keep `phazeai-core` as-is — it's the strongest part
- Rebuild panels in-place (don't create new crates)
- Agent-IDE communication stays on `AgentEvent` channel (extend enum as needed)
- AI mode context collection happens in `app.rs` before spawning agent
- Inline diff rendering uses `similar` crate (already a dependency)
