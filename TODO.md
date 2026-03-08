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
- [ ] **Column/box selection** — Alt+Shift+drag or Alt+Shift+Arrow
- [x] **Code folding** — Ctrl+Shift+[ fold / Ctrl+Shift+] unfold, fold icon in gutter; brace-matching-based ranges; line_height=0 for collapsed lines
- [x] **Bracket pair colorization** — highlight matching brackets with distinct colors (cycling 4 colors by depth in apply_attr_styles)
- [x] **Bracket pair guides** — vertical indent guides connecting bracket pairs (LineExtraStyle at open_col from open_line+1 to close_line)
- [x] **Auto-close brackets** — type `(` → inserts `()` with cursor inside (cursor-watching effect)
- [x] **Auto-close quotes** — type `"` → inserts `""` with cursor inside (escape-aware, lifetime-aware for `'`)
- [x] **Auto-surround** — select text, type `(` → wraps selection in parens (editor.rs:1846)
- [x] **Smart indent on Enter** — auto-indent to same or deeper level after `{`, `:`, etc.
- [x] **De-indent on `}`** — type `}` and it un-indents the current line (4-space / 2-space / 1-tab)
- [x] **Word wrap toggle** — Alt+Z toggles, WrapMethod::EditorWidth via settings + reactive styling rebuild
- [ ] **Sticky scroll** — function/class headers stay pinned at top of viewport while scrolling
- [ ] **Minimap** — optional right-side miniature overview of the whole file, click to scroll
- [x] **Breadcrumbs** — file path segments in toolbar bar above editor (path relative to workspace root)
- [x] **Indentation guides** — vertical lines showing indent depth (1px LineExtraStyle per 4-space indent level)
- [x] **Whitespace rendering** — show/hide spaces as dots and tabs as arrows via show_whitespace toggle (Alt+W)
- [x] **Line numbers** — relative line numbers toggle via command palette / Alt+R (relative_line_numbers signal)
- [x] **Highlight current line** — rgba(255,255,255,12) background via LineExtraStyle on current_line
- [x] **Match bracket highlight** — when cursor is on bracket, highlight matching bracket (matching_bracket field)
- [x] **Find: regex mode** — .* toggle in Ctrl+F find bar (uses regex crate)
- [x] **Find: case-sensitive toggle** — Aa toggle in Ctrl+F find bar
- [x] **Find: whole word toggle** — toggle `\b` matching in find bar (find_whole_word signal + W button)
- [ ] **Find: match count** — already done; add result index display in status bar
- [ ] **Multi-line find** — allow newlines in search pattern
- [x] **Split editor** — Ctrl+Alt+\ splits horizontally; Ctrl+Alt+↓ splits vertically (split_editor / split_editor_down)
- [ ] **Diff view** — show inline git diff (before/after) in a special diff editor tab
- [x] **Large file handling** — files > 2MB skip syntect highlighting (fall back to plain-text styling)
- [x] **Line ending indicator** — show CRLF/LF/Mixed in status bar (auto-detected per file)
- [x] **Encoding indicator** — UTF-8/etc shown in status bar (app.rs:2176, dynamic encoding detection)
- [x] **Read-only mode** — lock a tab when file is not writable; 🔒 indicator in status bar (active_readonly signal)

### Editor — LSP / Language Intelligence
- [x] **Find all references** (Shift+F12) — results in References bottom tab (LSP + ripgrep fallback)
- [x] **Rename symbol** (F2) — rename overlay + LSP workspace/rename + ripgrep replace fallback
- [x] **Code actions / Quick fix** (Ctrl+.) — LSP code actions dropdown popup
- [x] **Signature help** (Ctrl+Shift+Space) — shows function signature + active param at bottom of editor
- [x] **Document symbols** (Ctrl+Shift+O) — Symbols left-panel tab, click to jump, LSP + regex fallback
- [x] **Workspace symbols** (Ctrl+T) — search symbols across all files (LSP + ripgrep fallback)
- [x] **Peek definition** (Alt+F12) — shows definition source lines in popup overlay (peek_def_overlay in app.rs)
- [ ] **Call hierarchy** — who calls this function / what does it call
- [x] **Semantic token highlighting** — LSP semantic tokens override syntect colors (semantic_tokens signal + apply_attr_styles)
- [x] **Inlay hints** — show type hints inline after variable names; toggled via Ctrl+Alt+I (inlay_hints_sig signal)
- [x] **Code lens** — clickable annotations above functions; run test / N references (code_lens signal, gutter labels)
- [x] **Inline diagnostics** — diagnostic message for current cursor line shown in status bar

### Git
- [x] **Git gutter decorations** — green bar (added), yellow bar (modified), red triangle (deleted) via canvas in editor.rs
- [x] **Inline diff hunk preview** — GitDiff bottom tab shows colorized diff output per file (git diff HEAD)
- [x] **Revert hunk** — "↩" button per hunk in GitDiff tab, runs git apply --reverse (run_git_revert_hunk)
- [x] **Git blame** — inline blame for cursor line in gutter; full blame panel via git panel Blame tab (run_git_blame)
- [x] **Branch display** — current branch name in status bar (git rev-parse --abbrev-ref HEAD)
- [x] **Branch switching** — click branch in status bar → branch picker overlay → checkout selected
- [x] **Create branch** — "New Branch" button in git panel → input overlay → git checkout -b (run_git_checkout_new)
- [x] **Branch merge** — "Merge" button in branch picker → select branch → git merge (run_git_merge)
- [x] **Stash** — stash push / pop / apply / drop in git panel Stash section (run_git_stash*)
- [x] **Pull/push buttons** — Pull and Push buttons in git panel header row (run_git_pull / run_git_push)
- [x] **Commit history log** — scrollable log of recent commits in git panel Commits tab (run_git_log_full)
- [x] **Diff between commits** — click commit in log → shows git show --stat --patch in diff tab (run_git_show_diff)
- [ ] **Multi-repo support** — detect and show multiple git repos in one workspace

### Terminal
- [x] **Multiple terminal instances** — "+" button, tab bar to switch, × to close tabs
- [x] **Named terminals** — double-click tab label → inline text_input rename (editing_tab signal, terminal.rs:1278)
- [x] **Shell profile selection** — shell selector button cycles bash/zsh/fish/pwsh/nu (SHELLS array + shell_idx)
- [x] **Terminal split** — "⊟" button toggles side-by-side split view (term_split signal)
- [ ] **Terminal find** (Ctrl+Shift+F in terminal) — search through terminal output
- [x] **Hyperlink detection** — URLs (http/https) in terminal output are clickable; opens via xdg-open/open/start
- [x] **Command navigation** — OSC 133;A shell integration markers; ⬆/⬇ buttons jump between prompts
- [x] **Working directory tracking** — OSC 7 tracks cwd; shown in terminal tab title (cwd_signal)
- [x] **Terminal zoom** — A- / A+ buttons; independent per-terminal font size (term_font_size)
- [x] **Scrollback limit** — 10 000 line ring buffer (MAX_SCROLLBACK constant)
- [x] **Clear terminal** — ⌫ toolbar button sends Ctrl+L to PTY (clear_nonce); Ctrl+Shift+K also clears
- [x] **Run in terminal** — right-click in editor → "Run in Terminal" / "Run File"; run_in_terminal_text signal

### Search
- [x] **Regex search** — .* toggle button in workspace search panel
- [x] **Case-sensitive toggle** — Aa toggle button in workspace search panel
- [x] **Whole word toggle** — W button in workspace search panel (whole_word + --word-regexp rg flag)
- [x] **Include/exclude globs** — two text inputs below search bar: Include (*.rs) and Exclude (target/) — passed as --glob to rg
- [x] **Replace in files** — replace input + "Replace All" button with regex/case-sensitive support
- [x] **Show only open editors** — ⊞ toggle button filters results to currently-open tabs only
- [x] **Search history** — up/down arrows in search input cycle through past 50 queries (search_history signal)
- [x] **Symbol search** — Ctrl+T workspace symbols via LSP + ripgrep fallback (RequestWorkspaceSymbols)
- [x] **Search result tree view** — ⊟/⊞ toggle button switches between flat and grouped-by-file tree views

### File Explorer
- [x] **Create new file** — right-click context menu in explorer, prompts for filename
- [x] **Create new folder** — right-click context menu in explorer, prompts for folder name
- [x] **Rename file/folder** — right-click → rename dialog (fs_rename helper)
- [x] **Delete file/folder** — right-click → delete (fs_delete helper), files only
- [ ] **Duplicate file** — right-click → duplicate
- [ ] **Reveal in file manager** — right-click → open in Finder/Nautilus/Explorer
- [x] **Copy relative path** — right-click → "Copy Path" → clipboard via arboard
- [ ] **Drag-and-drop** — drag file to move it to a different directory
- [x] **File watcher** — auto-refresh explorer when files change on disk (notify + debounce 300 ms)
- [ ] **Git status decorations in explorer** — M/U/D badges next to modified/untracked/deleted files
- [ ] **Collapse all** — button to collapse entire tree back to root
- [ ] **Exclude patterns** — configurable list of folders/files to hide (`.git`, `target`, `node_modules`)

### UI / Workbench
- [x] **Status bar diagnostic summary** — shows ⊗ N  ⚠ N in status bar, colored by severity
- [x] **Problems panel** — "PROBLEMS" bottom tab with full workspace diagnostic list (problems_view)
- [x] **Notifications** — toast notifications (`show_toast()` + auto-dismiss overlay at z_index 450)
- [ ] **Progress indicator** — spinning indicator in status bar during AI requests / indexing
- [ ] **Keybindings editor** — UI to view and remap all keyboard shortcuts
- [ ] **Panel resize** — persist panel sizes across launches (already done for layout, add fine-grained control)
- [ ] **Activity bar reorder** — drag-and-drop to reorder activity bar icons
- [x] **Zen mode** — Ctrl+Shift+Z — hides all panels, distraction-free editor
- [ ] **Full-screen toggle** — F11 full-screen mode
- [x] **Context menus** — right-click in editor → Copy/Paste/Go to Def/Find Refs/Rename/Code Actions; right-click in explorer → CRUD + Copy Path
- [ ] **Drag tab to split** — drag a tab to the side to create a split view
- [ ] **Tab overflow** — when too many tabs, add left/right scroll arrows or dropdown

---

## 🟡 PHASE 3 — SHOULD HAVE (competitive with modern IDEs)

### AI Features (Differentiators)
- [ ] **AI explain code** — select code → right-click → "Explain with AI" → shows in chat
- [ ] **AI generate tests** — right-click → "Generate Tests" → inserts test code
- [ ] **AI fix diagnostic** — click lightbulb on error → "Fix with AI" auto-applies suggestion
- [ ] **AI code review** — button in git panel → AI reviews the full diff, posts comments
- [ ] **AI chat with file context** — @file mentions to include specific files in chat context
- [ ] **AI chat with selection** — select code → "Chat about selection" → sends to AI with code
- [ ] **AI refactor** — select code → "Refactor with AI" → shows before/after diff to approve
- [x] **AI commit message** — ✨ AI button in git commit area → runs git diff --cached → AI generates message
- [ ] **AI docstrings** — cursor on function → "Generate Docstring" → inserts doc comment
- [ ] **AI rename suggestions** — F2 rename → AI suggests better names based on usage
- [ ] **AI PR description** — generate PR description from branch diff
- [ ] **AI chat history** — persist chat sessions across restarts, browse history
- [ ] **AI model context window indicator** — show token usage / limit in chat panel
- [ ] **Multi-file AI editing** — AI can propose edits across multiple files simultaneously
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
- [ ] **Sort lines** — sort selected lines alphabetically
- [ ] **Join lines** — Ctrl+J join selected lines (remove newlines)
- [ ] **Transform case** — uppercase/lowercase/title case selected text
- [ ] **Transpose characters** — swap character before/after cursor (Ctrl+T in Emacs)
- [ ] **Delete line** — Ctrl+Shift+K delete line without clipboard
- [ ] **Duplicate line** — Alt+Shift+Down duplicate current line below
- [ ] **Move line up/down** — Alt+Up/Down move current line up or down
- [ ] **Indent / outdent** — Tab/Shift+Tab without selection indents/outdents whole line
- [ ] **Balance brackets** — select from bracket to its matching bracket
- [ ] **Code minimap highlight** — highlight search results in minimap

### Git — Advanced
- [ ] **Interactive rebase UI** — visual reorder/squash/fixup of commits
- [ ] **Cherry-pick** — pick a specific commit onto current branch
- [ ] **Git tag support** — create, list, push tags
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
- [ ] **Confirm before close** — prompt if unsaved files exist when quitting
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
- [ ] **GitHub Actions log** — stream CI run output from PR/commit in IDE
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
- [ ] Vim mode: visual mode (v/V) not yet implemented
- [x] Vim mode: yank (y/yy) to internal RwSignal register — implemented
- [ ] Git panel doesn't auto-refresh on external `git` command (requires panel switch)
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

*Last updated: 2026-02-27*
