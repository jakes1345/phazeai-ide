# PhazeAI IDE — Master Feature TODO

> **Mission**: Best open-source AI-native IDE. Local-first, all Rust, GPU-rendered.
> **Legend**: `[ ]` todo · `[~]` in progress · `[x]` done · `[!]` blocked · `[-]` dropped
> **Model**: MIT open-source core + paid PhazeAI Cloud (hosted AI credits, team features)

---

## ✅ ALREADY DONE (Phase 1 + Phase 2 progress)

- [x] Multi-tab editor with syntect syntax highlighting
- [x] LSP completions popup with 300ms debounce + prefix filter
- [x] LSP go-to-definition (F12)
- [x] LSP hover (Ctrl+F1)
- [x] LSP diagnostic squiggles (wave_line for errors, under_line for warnings)
- [x] Diagnostic colored dots in tab bar
- [x] Terminal panel (PTY via portable-pty + vte, 256-color)
- [x] Terminal clipboard (Ctrl+Shift+C/V)
- [x] Git status/stage/unstage/discard/commit UI
- [x] Git "Stage All" button
- [x] Per-file +/−/↩ hover buttons in git panel
- [x] Workspace search (grep-based, click to jump)
- [x] AI chat panel with real streaming
- [x] Find/replace in file (Ctrl+F / Ctrl+H)
- [x] Goto line (Ctrl+G)
- [x] Vim mode (Normal/Insert, motions h/j/k/l/w/b/0/$, dd/x, o, i/a)
- [x] FIM ghost text completions with Tab to accept
- [x] Settings panel (theme, font size, tab size, AI provider/model)
- [x] 12 color themes (MidnightBlue, Dracula, Nord, Tokyo Night, etc.)
- [x] Command palette (Ctrl+P)
- [x] File picker overlay
- [x] File explorer (expand/collapse dirs, click to open)
- [x] Output/Debug Console/Ports bottom tabs
- [x] Session persistence (open tabs, theme, font size, panel layout)
- [x] Ctrl+K inline AI editing overlay
- [x] Comment toggle (Ctrl+/)
- [x] Format on save (rustfmt / prettier / black)
- [x] Cross-platform open tool (xdg-open / open / start)
- [x] Cloud sign-in stub opens browser
- [x] Undo/redo (Floem built-in via default_key_handler)
- [x] LSP bridge (textDocument/didOpen, didChange, publishDiagnostics)
- [x] Ctrl+B toggle left panel, Ctrl+J toggle terminal, Ctrl+\ toggle chat
- [x] Font zoom (Ctrl+= / Ctrl+-)
- [x] Sentient gutter AI glow animation
- [x] Neon scrollbar heatmap canvas
- [x] AI multi-agent: Planner → Coder → Reviewer pipeline
- [x] Cancel token for agent runs
- [x] Usage tracking (input/output tokens)
- [x] phazeai-core: OpenAI streaming serialization fix
- [x] phazeai-cli: real tool approval (oneshot channel), /cancel abort, file tree

---

## 🔴 PHASE 2 — CRITICAL (ship blockers)

### Editor — Core Editing
- [x] **Multi-cursor (Ctrl+D)** — Ctrl+D selects next occurrence of word/selection and adds as second cursor region via Selection::add_region(SelRegion)
- [x] **Column/box selection** — Ctrl+Alt+Down/Up adds cursor on adjacent line at same column
- [x] **Code folding** — Ctrl+Shift+[ fold / Ctrl+Shift+] unfold, fold icon in gutter; brace-matching-based ranges; line_height=0 for collapsed lines
- [x] **Bracket pair colorization** — 4-color cycling (gold/sky-blue/violet/mint) via bracket_pairs in SyntaxStyle
- [x] **Bracket pair guides** — vertical 1px lines connecting bracket pairs via apply_layout_styles
- [x] **Auto-close brackets** — type `(` → inserts `()` with cursor inside (cursor-watching effect)
- [x] **Auto-close quotes** — type `"` → inserts `""` with cursor inside (escape-aware, lifetime-aware for `'`)
- [x] **Auto-surround** — select text, type bracket → wraps selection (surr_prev_sel tracking)
- [x] **Smart indent on Enter** — auto-indent to same or deeper level after `{`, `:`, etc.
- [x] **De-indent on `}`** — type `}` and it un-indents the current line (4-space / 2-space / 1-tab)
- [x] **Word wrap toggle** — Alt+Z toggles, WrapMethod::EditorWidth via settings + reactive styling rebuild
- [x] **Sticky scroll** — function/class headers pinned at top via sticky_hdr signal + backward indent scan
- [x] **Minimap** — right-side overview canvas with per-line stripes, viewport indicator, click-to-scroll
- [x] **Breadcrumbs** — file path segments in toolbar bar above editor (path relative to workspace root)
- [x] **Indentation guides** — vertical 1px bars at 4-space intervals via LineExtraStyle
- [x] **Whitespace rendering** — Ctrl+Shift+W toggle, dots for spaces, dashes for tabs
- [x] **Line numbers** — relative line numbers toggle via relative_line_numbers signal + settings
- [x] **Highlight current line** — rgba(255,255,255,12) background via LineExtraStyle on current_line
- [x] **Match bracket highlight** — cursor-watching effect + find_bracket_match() + box/underline highlight
- [x] **Find: regex mode** — .* toggle in Ctrl+F find bar (uses regex crate)
- [x] **Find: case-sensitive toggle** — Aa toggle in Ctrl+F find bar
- [x] **Find: whole word toggle** — \b word boundary matching in find bar + search panel
- [x] **Find: match count** — match count memo + result index display
- [ ] **Multi-line find** — allow newlines in search pattern
- [x] **Split editor** — Ctrl+Alt+\ split right + Ctrl+Alt+Shift+D split down, independent tabs
- [x] **Diff view** — GitDiff bottom tab with colorized per-file diff output
- [x] **Large file handling** — files > 2MB skip syntect highlighting (fall back to plain-text styling)
- [x] **Line ending indicator** — show CRLF/LF/Mixed in status bar (auto-detected per file)
- [x] **Encoding indicator** — UTF-8 encoding label in status bar
- [x] **Read-only mode** — active_readonly signal, file permission check on open, read-only badge

### Editor — LSP / Language Intelligence
- [x] **Find all references** (Shift+F12) — results in References bottom tab (LSP + ripgrep fallback)
- [x] **Rename symbol** (F2) — rename overlay + LSP workspace/rename + ripgrep replace fallback
- [x] **Code actions / Quick fix** (Ctrl+.) — LSP code actions dropdown popup
- [x] **Signature help** (Ctrl+Shift+Space) — shows function signature + active param at bottom of editor
- [x] **Document symbols** (Ctrl+Shift+O) — Symbols left-panel tab, click to jump, LSP + regex fallback
- [x] **Workspace symbols** (Ctrl+T) — search symbols across all files (LSP + ripgrep fallback)
- [x] **Peek definition** (Alt+F12) — peek_def_overlay at z_index(485) with RequestPeekDefinition
- [ ] **Call hierarchy** — who calls this function / what does it call
- [ ] **Semantic token highlighting** — LSP semantic tokens override syntect colors
- [x] **Inlay hints** — Ctrl+Alt+I toggle, InlayHintEntry with line/col/label, RequestInlayHints
- [x] **Code lens** — code_lens signal + RequestCodeLens + code lens bar in editor_panel
- [x] **Inline diagnostics** — diagnostic message for current cursor line shown in status bar

### Git
- [x] **Git gutter decorations** — green bar (added), yellow bar (modified), red triangle (deleted) via canvas in editor.rs
- [x] **Inline diff hunk preview** — GitDiff bottom tab shows colorized diff output per file (git diff HEAD)
- [x] **Revert hunk** — run_git_revert_hunk() with hunk extraction and git apply --reverse
- [x] **Git blame** — run_git_blame() + BlameEntry + collapsible section + inline blame info
- [x] **Branch display** — current branch name in status bar (git rev-parse --abbrev-ref HEAD)
- [x] **Branch switching** — click branch in status bar → branch picker overlay → checkout selected
- [x] **Create branch** — "+" New Branch button in branch picker with git checkout -b
- [x] **Branch merge** — merge picker UI with branch selection and git merge
- [x] **Stash** — Stash/Pop buttons with run_git_stash/run_git_stash_pop + stash list
- [x] **Pull/push buttons** — Pull and Push buttons in git panel header with background threads
- [x] **Commit history log** — scrollable log of recent commits (hash, message, author, date)
- [x] **Diff between commits** — click commit → show diff via commit_diff channel
- [ ] **Multi-repo support** — detect and show multiple git repos in one workspace

### Terminal
- [x] **Multiple terminal instances** — "+" button, tab bar to switch, × to close tabs
- [x] **Named terminals** — click active tab label to rename via inline text_input
- [x] **Shell profile selection** — SHELLS const + shell cycler button in tab bar
- [ ] **Terminal split** — split terminal pane horizontally or vertically
- [ ] **Terminal find** (Ctrl+Shift+F in terminal) — search through terminal output
- [x] **Hyperlink detection** — URL_RE regex + xdg-open/open click handler
- [x] **Command navigation** — OSC 7 shell integration + command markers
- [x] **Working directory tracking** — OSC 7 cwd_out signal in terminal tab
- [x] **Terminal zoom** — independent term_font_size signal, Ctrl+Shift+=/-
- [ ] **Scrollback limit** — configurable scrollback buffer size (default 10k lines)
- [x] **Clear terminal** — clear_nonce per tab + ⌫ button writes Ctrl+L to PTY
- [ ] **Run in terminal** — right-click in editor → "Run in Terminal" sends selected code

### Search
- [x] **Regex search** — .* toggle button in workspace search panel
- [x] **Case-sensitive toggle** — Aa toggle button in workspace search panel
- [x] **Whole word toggle** — whole_word signal + \b word boundary in rg args
- [x] **Include/exclude globs** — include_glob/exclude_glob inputs wired to rg --glob args
- [x] **Replace in files** — replace input + "Replace All" button with regex/case-sensitive support
- [ ] **Show only open editors** — search only currently-open tabs
- [x] **Search history** — Up/Down arrow cycling through past queries (capped at 50)
- [ ] **Symbol search** — search for symbols (functions, classes) across workspace
- [x] **Search result tree view** — tree_view toggle between flat list and grouped-by-file tree

### File Explorer
- [x] **Create new file** — right-click context menu in explorer, prompts for filename
- [x] **Create new folder** — right-click context menu in explorer, prompts for folder name
- [x] **Rename file/folder** — right-click → rename dialog (fs_rename helper)
- [x] **Delete file/folder** — right-click → delete (fs_delete helper), files only
- [x] **Duplicate file** — right-click → "Duplicate" via std::fs::copy to <stem>_copy.<ext>
- [x] **Reveal in file manager** — right-click → xdg-open/open/explorer on parent dir
- [x] **Copy relative path** — right-click → "Copy Path" → clipboard via arboard
- [ ] **Drag-and-drop** — drag file to move it to a different directory
- [x] **File watcher** — auto-refresh explorer when files change on disk (notify + debounce 300 ms)
- [x] **Git status decorations in explorer** — M/U/D badges via git_status HashMap + periodic refresh
- [x] **Collapse all** — ⊟ button in explorer header sets all entries expanded=false
- [x] **Exclude patterns** — load_children skips target, node_modules, dist, .next, __pycache__, etc.

### UI / Workbench
- [x] **Status bar diagnostic summary** — shows ⊗ N  ⚠ N in status bar, colored by severity
- [x] **Problems panel** — "PROBLEMS" bottom tab with full workspace diagnostic list (problems_view)
- [x] **Notifications** — toast notifications (`show_toast()` + auto-dismiss overlay at z_index 450)
- [x] **Progress indicator** — braille spinner + AI thinking indicator in status bar
- [ ] **Keybindings editor** — UI to view and remap all keyboard shortcuts
- [ ] **Panel resize** — persist panel sizes across launches (already done for layout, add fine-grained control)
- [ ] **Activity bar reorder** — drag-and-drop to reorder activity bar icons
- [x] **Zen mode** — Ctrl+Shift+Z — hides all panels, distraction-free editor
- [x] **Full-screen toggle** — F11 via wmctrl/xdotool
- [x] **Context menus** — right-click in editor → Copy/Paste/Go to Def/Find Refs/Rename/Code Actions; right-click in explorer → CRUD + Copy Path
- [ ] **Drag tab to split** — drag a tab to the side to create a split view
- [x] **Tab overflow** — scroll + dropdown button showing all tabs with count badge

---

## 🟡 PHASE 3 — SHOULD HAVE (competitive with modern IDEs)

### AI Features (Differentiators)
- [x] **AI explain code** — select code → right-click → "Explain with AI" → pending_chat_inject to chat
- [x] **AI generate tests** — right-click → "Generate Tests" → pending_chat_inject to chat
- [x] **AI fix diagnostic** — right-click → "Fix with AI" → pending_chat_inject with diagnostic context
- [x] **AI code review** — "AI Review" button in git panel → git diff HEAD → chat injection
- [x] **AI chat with file context** — @filename mentions expand to file contents as context blocks
- [x] **AI chat with selection** — selection-based context menu items inject into chat
- [ ] **AI refactor** — select code → "Refactor with AI" → shows before/after diff to approve
- [x] **AI commit message** — ✨ AI button in git commit area → runs git diff --cached → AI generates message
- [ ] **AI docstrings** — cursor on function → "Generate Docstring" → inserts doc comment
- [ ] **AI rename suggestions** — F2 rename → AI suggests better names based on usage
- [ ] **AI PR description** — generate PR description from branch diff
- [x] **AI chat history** — ConversationStore persistence + session browser (⊟ button)
- [ ] **AI model context window indicator** — show token usage / limit in chat panel
- [x] **Multi-file AI editing** — AI Composer panel with Agent + all tools + diff cards
- [ ] **AI ask about error** — click diagnostic → "Ask AI about this error"
- [ ] **Inline AI diff review** — when Ctrl+K applies changes, show before/after inline diff to approve

### Debugging (DAP)
- [ ] **Debug Adapter Protocol (DAP) client** — connect to any DAP-compatible debugger
- [ ] **Breakpoints** — click gutter → set/clear breakpoints, shown as red circles
- [ ] **Conditional breakpoints** — right-click breakpoint → add condition expression
- [ ] **Run/Continue/Step Over/Step Into/Step Out** — standard debug controls in toolbar
- [ ] **Variables panel** — show all locals and their values when paused
- [ ] **Watch panel** — add expressions to watch, updated on each pause
- [ ] **Call stack panel** — show call stack, click frame to navigate to source
- [ ] **Debug console** — REPL for evaluating expressions during debug session
- [ ] **Inline variable values** — show current variable values inline in editor while paused
- [ ] **Exception breakpoints** — break on uncaught/all exceptions
- [ ] **Debug toolbar** — floating toolbar with play/pause/step buttons while debugging
- [ ] **Hover to evaluate** — hover over expression in editor during debug to see value

### Testing
- [ ] **Test runner panel** — list tests from LSP/cargo test output, show pass/fail
- [ ] **Run test at cursor** — Ctrl+Shift+T or code lens → run the test the cursor is in
- [ ] **Run all tests** — button to run entire test suite, stream output
- [ ] **Test status decorations** — green tick / red X in gutter next to test functions
- [ ] **Test failure inline** — show assertion failure message inline in editor
- [ ] **Code coverage** — highlight covered/uncovered lines with green/red background
- [ ] **Re-run failed tests** — filter to failed and re-run only those

### Editor — Advanced
- [ ] **Emmet expansion** — type `div.container>p` + Tab → expands to HTML
- [ ] **Snippet support** — define custom snippets triggered by prefix + Tab
- [ ] **Tab stops in snippets** — cursor cycles through $1, $2 placeholders with Tab
- [ ] **Parameter hints** — show all overloads for a function signature
- [ ] **Type hierarchy** — show super/sub-types for a class/trait
- [x] **Sort lines** — sort_lines_nonce + command palette "Sort Lines (Ascending)"
- [x] **Join lines** — join_line_nonce + command palette "Join Lines"
- [x] **Transform case** — transform_upper_nonce/transform_lower_nonce + command palette
- [ ] **Transpose characters** — swap character before/after cursor (Ctrl+T in Emacs)
- [ ] **Delete line** — Ctrl+Shift+K delete line without clipboard
- [ ] **Duplicate line** — Alt+Shift+Down duplicate current line below
- [ ] **Move line up/down** — Alt+Up/Down move current line up or down
- [ ] **Indent / outdent** — Tab/Shift+Tab without selection indents/outdents whole line
- [ ] **Balance brackets** — select from bracket to its matching bracket
- [ ] **Code minimap highlight** — highlight search results in minimap

### Git — Advanced
- [ ] **Interactive rebase UI** — visual reorder/squash/fixup of commits
- [x] **Cherry-pick** — cherry_pick_tx + run_git_cherry_pick in commit history rows
- [x] **Git tag support** — tag_list signal + run_git_tag_create/push + collapsible tag section
- [ ] **Submodule support** — show/update submodules in explorer
- [ ] **Merge conflict editor** — when conflicts exist, show 3-way merge UI (incoming/current/result)
- [ ] **Commit amend** — amend the last commit (add --amend flag to commit)
- [ ] **Sign commits** — GPG signing support

### Settings & Config
- [ ] **JSON settings file** — edit `settings.toml` raw with syntax highlighting and LSP
- [ ] **Workspace settings** — per-workspace `.phazeai/settings.toml` override
- [ ] **Keybindings JSON** — edit keybindings in a structured file (like VS Code's keybindings.json)
- [ ] **Settings sync** — sync settings via PhazeAI Cloud account
- [ ] **Font family picker** — dropdown/search to select from installed monospace fonts
- [ ] **Editor cursor style** — beam / block / underline cursor shapes
- [ ] **Minimap settings** — enable/disable, show slider, max columns
- [x] **Auto-save** — configurable in settings panel; 1.5 s debounce after last keystroke (AtomicU64 cancel token)
- [ ] **Trim trailing whitespace on save** — configurable per language
- [ ] **Insert final newline** — ensure file ends with `\n` on save
- [ ] **Detect indentation** — auto-detect tab/space indent from file content

### Workbench
- [ ] **Workspace switcher** — quick-switch between recently opened workspaces/folders
- [ ] **Recent files** — Ctrl+P recent files at the top, sorted by last-opened
- [ ] **Welcome tab** — show welcome/getting-started page on first launch
- [ ] **Keyboard shortcuts reference** — Ctrl+K Ctrl+S show all keybindings in a searchable panel
- [ ] **Multi-root workspace** — open multiple root folders in one window
- [ ] **Window title** — show `filename — folder — PhazeAI` in window title bar
- [x] **Confirm before close** — rfd::MessageDialog on dirty tab close
- [ ] **Auto-detect project type** — detect Rust/Python/Node project, configure LSP automatically
- [ ] **Project template** — "New Project" dialog with templates (Rust binary, Node, Python)

---

## 🟢 PHASE 4 — NICE TO HAVE (stretch / long-term)

### Extensions / Plugin System
- [ ] **Plugin API design** — define stable Rust/WASM plugin API
- [ ] **Plugin discovery** — built-in marketplace or link to curated plugin list
- [ ] **Plugin sandboxing** — WASM sandbox for safe plugin execution
- [ ] **Extension pack** — group of plugins installed together
- [ ] **Theme plugin** — allow third-party themes as plugins
- [ ] **Language server plugin** — auto-install LSP servers via plugin

### Notebooks
- [ ] **Jupyter notebook renderer** — .ipynb file support with cell-by-cell execution
- [ ] **REPL panel** — language-specific REPL (Python, Node, Julia)
- [ ] **Output rendering** — render images, tables, plots from notebook output

### Remote Development
- [ ] **SSH remote** — open folder on remote machine via SSH
- [ ] **Container dev** — open project inside Docker container
- [ ] **WSL support** — open WSL filesystem on Windows
- [ ] **Remote terminal** — terminal connects to remote process

### Performance
- [ ] **Treesitter parsing** — faster, more accurate parse tree (replace syntect for some langs)
- [ ] **Incremental re-highlighting** — only re-highlight changed regions of file
- [ ] **Virtual rendering** — only render visible lines in huge files (100k+ lines)
- [ ] **File indexing** — background index of all symbols for fast workspace search
- [ ] **Code search index** — ripgrep-based index for instant search results

### Collaboration
- [ ] **Live Share** — real-time collaborative editing with cursor sharing
- [ ] **Session recording** — record and replay terminal/editing sessions
- [ ] **Code review UI** — browse PR comments alongside the diff

### Accessibility
- [ ] **High contrast themes** — dedicated HC Black and HC Light themes
- [ ] **Screen reader mode** — ARIA announcements for cursor position, errors, completions
- [ ] **Keyboard-only navigation** — full access to all panels without mouse
- [ ] **Focus mode** — reduce motion / animations for users with vestibular disorders

### Mobile / Tablet
- [ ] **Touch input** — handle touch events in editor for basic editing on tablets
- [ ] **Virtual keyboard aware** — adjust layout when software keyboard appears

### Integrations
- [ ] **Jira / Linear integration** — show issues, link commits to issues
- [x] **GitHub Actions log** — github_actions panel with run list, auto-refresh, rerun support
- [ ] **Docker panel** — list running containers, attach terminal, view logs
- [ ] **Database viewer** — connect to Postgres/SQLite, browse schema, run queries
- [ ] **HTTP client** — send HTTP requests like REST Client extension
- [ ] **Markdown preview** — live rendered preview of .md files side-by-side
- [ ] **Image preview** — show PNG/JPG/SVG directly in an editor tab
- [ ] **PDF viewer** — view PDFs inline
- [ ] **CSV viewer** — show CSV files as sortable table
- [ ] **Hex editor** — view binary files as hex + ASCII
- [ ] **JSON tree view** — show JSON files as collapsible tree

### AI — Advanced
- [ ] **Project-wide AI context** — vector index of codebase for semantic search in chat
- [ ] **AI agent mode** — agent autonomously edits files, runs commands, iterates
- [ ] **AI test generation** — generate full test suite for a module
- [ ] **AI migration assistant** — upgrade dependencies, migrate API versions
- [ ] **AI security scan** — flag potential security issues with explanations
- [ ] **AI code review bot** — auto-review every commit with AI comments
- [ ] **Custom system prompt** — per-project AI instructions in `.phazeai/instructions.md`
- [ ] **AI chat slash commands** — `/explain`, `/fix`, `/test`, `/refactor`, `/doc`
- [ ] **Multiple AI chat threads** — tabs in chat panel for different conversations
- [ ] **AI suggested follow-ups** — after response, show clickable follow-up questions

### Cloud / Team (PhazeAI Cloud — paid tier)
- [ ] **Shared AI credits** — team pool of tokens, usage dashboard
- [ ] **Team settings sync** — shared linting rules, formatter config
- [ ] **Audit log** — record all AI interactions for enterprise compliance
- [ ] **SSO / SAML login** — enterprise auth
- [ ] **Private model routing** — route AI requests to company-hosted models
- [ ] **Codebase context upload** — send private codebase to PhazeAI Cloud for better suggestions

---

## 📊 METRICS TO TRACK

- [ ] Time-to-first-keypress (< 300ms cold start)
- [ ] Memory usage idle (< 200MB)
- [ ] LSP response latency (< 100ms for completions)
- [ ] AI streaming TTFT (< 1s for first token)
- [ ] Frame rate (60fps during typing, 144fps capable)

---

## 🐛 KNOWN BUGS / TECH DEBT

- [x] Explorer file watcher — notify crate, debounced 300 ms, auto-refresh tree
- [ ] Completion popup position can overlap status bar on short files
- [ ] Ghost text FIM fires on empty prefix — should skip
- [x] Vim mode: paste (p/P) implemented (after-line / before-line from register)
- [x] Vim mode: visual mode (v/V) — VisualCharStart/VisualLineStart with vim_visual_anchor
- [x] Vim mode: yank (y/yy) to internal RwSignal register — implemented
- [x] Git panel auto-refresh — .git/index mtime polling + channel-based refresh
- [ ] Terminal: no Ctrl+Left/Right word navigation
- [ ] Terminal: resize not propagated to PTY on window resize (may cause display glitches)
- [ ] Settings: ai_provider change doesn't update the FIM client (uses Settings::load() each time — OK)
- [ ] Multi-tab session restore: if a file was deleted, tab shows empty (no error message)
- [ ] Syntax highlighting cache can get stale after large edits (states_cache truncation)
- [ ] Find/replace: `\n` in replace string not handled
- [x] LSP: textDocument/didSave sent via LspCommand::SaveFile on every Ctrl+S
- [ ] Python sidecar server.py does not exist — phazeai-sidecar Rust client stubs
- [ ] phazeai-cloud crate is a skeleton — no real auth/API calls

---

*Last updated: 2026-03-14*
