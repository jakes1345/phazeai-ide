# PhazeAI IDE — Phase 3 Master Feature List
> 200+ items. Grounded in Lapce's changelog + gaps we identified.
> Status: `[ ]` = not started · `[~]` = in progress · `[x]` = done

---

## 🔴 P0 — Without These It's Not a Real IDE

### Editor Core
- [~] **Inlay hints** — show type/param hints inline (e.g. `let x/*: i32*/`) via LSP `textDocument/inlayHint` — *deprioritized: requires Floem Document fork*
- [ ] **Semantic token highlighting** — LSP semantic tokens override syntect colors per-token
- [x] **Bracket pair colorization** — matching bracket pairs get distinct cycling colors (4 levels)
- [ ] **Bracket pair guides** — vertical lines connecting `{`→`}` at matching indent
- [ ] **Split editor right** (Ctrl+\\) — side-by-side editing of two different files
- [ ] **Split editor down** — horizontal split for top/bottom layout
- [ ] **Split editor: sync scroll** — optional linked scrolling between splits
- [x] **Indent guides** — vertical lines at each indent level
- [x] **Matching bracket highlight** — when cursor is on `(`, highlight the matching `)`
- [x] **Auto-surround** — select text, type `(` → wraps selection in `(…)`; same for `[`, `{`, `"`, `'`
- [ ] **Sticky scroll** — function/class signature stays pinned at top while scrolling into body
- [ ] **Minimap** — right-side pixel-art overview of full file; click to scroll; highlight viewport region
- [ ] **Column/box selection** — Alt+Shift+drag or Alt+Shift+Arrow for rectangular multi-line selection
- [ ] **Multi-line find** — allow `\n` in search pattern for cross-line matching
- [ ] **Whole word toggle** — `\b` boundary toggle in find bar (already done in workspace search; add to editor find)
- [x] **Find: highlight all** — all matches softly highlighted in editor body simultaneously

### LSP
- [ ] **Code lens** — clickable annotations above functions: reference count, "Run Test", "Debug Test"
- [ ] **Peek definition** (Alt+F12) — inline definition panel without leaving current file
- [ ] **Go to implementation** (Ctrl+F12) — navigate to concrete implementation of interface/trait
- [ ] **Call hierarchy** — "Incoming calls" and "outgoing calls" tree panel
- [ ] **Type hierarchy** — show supertype/subtype relationships
- [ ] **LSP `codeAction/resolve`** — lazy-resolve code action details only when selected
- [ ] **LSP resource operations** — rename/create/delete files as part of workspace edits
- [ ] **Multiple hover items** — show all hover results when LSP returns multiple, not just first
- [ ] **LSP progress notifications** — show `window/workDoneProgress` in status bar (indexing…)
- [ ] **LSP server status** — indicator when language server crashes or fails to start
- [ ] **Document color provider** — show color swatches inline for CSS hex/rgb values
- [ ] **Folding ranges from LSP** — use `textDocument/foldingRange` response instead of brace-only regex
- [ ] **Selection-based folding** — fold arbitrary selected region, not just brace-matched blocks
- [ ] **Toggle all folds** — fold/unfold everything in file at once
- [ ] **Fold by level** — fold all blocks at indent level N
- [ ] **Format selection** — run formatter on selected text only, not whole file
- [ ] **Organize imports** — trigger LSP organize-imports code action on save

### Git
- [x] **Pull button** — pull from remote in git panel header
- [x] **Push button** — push to remote in git panel header
- [x] **Git blame** — inline blame annotation per line (author + date + commit hash), collapsible section with "Blame File" button
- [x] **Commit history log** — scrollable list of recent commits with hash/message/author/date
- [ ] **Diff between commits** — click commit in log → open diff editor showing what changed
- [ ] **Git stash push** — stash current changes from git panel
- [ ] **Git stash pop** — apply latest stash from git panel
- [ ] **Stash list** — view all stashes, apply or drop any of them
- [ ] **Revert hunk** — button in hunk popup to undo that specific changed section
- [ ] **Create branch** — prompt for name, create and checkout from git panel
- [ ] **Delete branch** — delete local branch from branch picker
- [ ] **Fetch** — fetch from all remotes without merging
- [ ] **Merge branch** — merge another branch into current from git panel
- [ ] **Discard single file changes** — right-click unstaged file → discard
- [ ] **Cherry-pick** — apply a specific commit from history to current branch
- [ ] **Tag management** — create/delete tags from git panel
- [ ] **Git file status in explorer** — M / U / A / D badges next to files in file tree
- [ ] **Open file diff from explorer** — right-click modified file → "Open Diff"
- [ ] **Missing git user config error** — show clear error if git user.name/email not set

---

## 🟠 P1 — Makes It Competitive

### Editor Features
- [ ] **Relative line numbers** — toggle between absolute and relative (vim users need this)
- [ ] **Whitespace rendering** — show spaces as `·` and tabs as `→` (toggle in settings)
- [ ] **Cursor surrounding lines** — configurable min lines above/below cursor when scrolling
- [ ] **Sort lines** — sort selected lines alphabetically (ascending/descending)
- [ ] **Join lines** — merge current line with next (remove newline + trim whitespace)
- [ ] **Duplicate line up** — copy current line above cursor
- [ ] **Duplicate line down** — copy current line below cursor
- [ ] **Move line up** — swap current line with line above (Alt+Up)
- [ ] **Move line down** — swap current line with line below (Alt+Down)
- [ ] **Transform to uppercase** — selected text → UPPER CASE
- [ ] **Transform to lowercase** — selected text → lower case
- [ ] **Transform to title case** — selected text → Title Case
- [ ] **Selection expand** — Ctrl+Shift+→ grow selection to next syntactic boundary
- [ ] **Selection shrink** — Ctrl+Shift+← shrink selection back one syntactic boundary
- [ ] **Goto column** — extend Ctrl+G overlay to accept `line:col` format
- [ ] **Multiple clipboards** — cycle through clipboard history (last 5 yanks)
- [ ] **Scratch file** — Ctrl+N opens untitled buffer not backed by disk
- [ ] **Save without formatting** — Alt+Ctrl+S saves file skipping format-on-save
- [ ] **Read-only mode** — lock tab when file is not writable; show lock icon + read-only badge
- [ ] **Encoding indicator** — show UTF-8 / UTF-16 / Latin-1 in status bar
- [ ] **Line ending toggle** — click CRLF/LF in status bar → convert file line endings
- [ ] **Auto-detect indentation** — detect tabs-vs-spaces and size from file content on open
- [ ] **.editorconfig support** — read `.editorconfig` and apply indent style/size per-file
- [ ] **Atomic soft tabs** — move cursor over 4-space indent as single unit (already in Lapce)
- [ ] **Input method (IME)** — proper CJK input support (composition window, candidate list)
- [ ] **Non-US keyboard layouts** — ensure all shortcuts work on AZERTY, QWERTZ, etc.

### Terminal
- [ ] **Named terminals** — rename terminal tab (double-click tab label)
- [ ] **Shell profile selection** — choose bash/zsh/fish/pwsh per new terminal
- [ ] **Terminal split horizontal** — split current terminal pane left/right
- [ ] **Terminal split vertical** — split current terminal pane top/bottom
- [ ] **Terminal find** (Ctrl+Shift+F in terminal) — regex search through terminal scrollback
- [ ] **Hyperlink detection** — URLs and file paths in output become clickable
- [ ] **Command markers** — shell integration: track where each command starts/ends, jump between
- [ ] **Working directory tracking** — show cwd in terminal tab title
- [ ] **Terminal zoom** — independent font size for terminal (separate from editor font)
- [ ] **Configurable scrollback limit** — settings field for scrollback buffer size
- [ ] **Clear terminal** — Ctrl+K (or toolbar button) clears terminal output
- [ ] **Run in terminal** — right-click in editor → "Run in Terminal" sends selected code
- [ ] **Run file** — toolbar button or right-click → run current file with detected runtime
- [ ] **Alt+Backspace** — delete word backwards in terminal
- [ ] **Double-click to maximize** — double-click bottom panel header to maximize/restore
- [ ] **Terminal default profile** — save chosen shell as default in settings
- [ ] **Host shell detection** — detect parent shell even inside Flatpak/Snap/container

### File Explorer
- [ ] **Drag-and-drop to move** — drag file/folder to new location in tree
- [ ] **Duplicate file/folder** — right-click → duplicate with `_copy` suffix
- [ ] **Reveal in system file manager** — right-click → open in Nautilus/Finder/Explorer
- [ ] **Collapse all** — toolbar button collapses entire tree back to root
- [ ] **Exclude patterns** — configurable glob list to hide files (`.git`, `target`, `node_modules`)
- [ ] **Horizontal scrolling** — explorer tree scrolls horizontally for deep paths
- [ ] **Unique path disambiguation** — show parent directory when two files share same name
- [ ] **Reveal in file tree** — editor tab right-click → highlight in explorer
- [ ] **Copy absolute path** — right-click → copy full path to clipboard
- [ ] **Copy relative path** — right-click → copy path relative to workspace root
- [ ] **New file from explorer** — click + icon in explorer header → create file
- [ ] **New folder from explorer** — click folder+ icon → create directory
- [ ] **Git status in explorer** — M/A/U/D color-coded file names matching git status
- [ ] **Open Editors section** — collapsible section showing currently open tabs (Lapce has hide toggle)

### Search
- [ ] **Include globs** — filter workspace search to `*.rs` or `src/**`
- [ ] **Exclude globs** — exclude `target/`, `node_modules/`, `*.min.js` from results
- [ ] **Search history** — up/down arrows cycle through previous queries
- [ ] **Search result tree view** — toggle between flat list and grouped-by-file tree
- [ ] **Search only open editors** — checkbox to limit search to open tabs
- [ ] **Replace preview** — show diff preview of replacements before applying
- [ ] **Whole word search toggle** — workspace search panel whole-word button
- [ ] **Search panel keyboard navigation** — arrow keys to move between results

### Vim Mode
- [ ] **Vim marks** — `ma` set mark, `` `a `` jump to mark
- [ ] **Multi-line motions** — `3dd`, `2yy`, `5j` etc. work correctly with repeat count
- [ ] **`cw`, `ce`, `cc`, `S`** — change-word, change-end, change-line motions
- [ ] **`gf`** — go to file under cursor
- [ ] **`Shift+C`** — delete rest of line and enter insert mode
- [ ] **Visual line mode** — `V` for line-wise visual selection
- [ ] **Visual block mode** — `Ctrl+V` for block selection
- [ ] **`:w`, `:q`, `:wq`, `:e`** — ex commands in command line
- [ ] **Vim search** (`/` and `?`) — vim-native search with `n`/`N` navigation
- [ ] **Vim `%`** — jump to matching bracket
- [ ] **`r` replace char** — replace single char without entering insert mode
- [ ] **Vim `.` repeat** — repeat last change action
- [ ] **Macros** (`q` to record, `@` to replay)
- [ ] **`za` fold toggle** — already done; add `zM` (fold all) / `zR` (unfold all)
- [ ] **`gg` / `G`** — go to top / bottom of file
- [ ] **`Ctrl+d` / `Ctrl+u`** — half-page scroll down/up in normal mode

---

## 🟡 P2 — Polish and Power-User Features

### UI / UX
- [ ] **Command palette search by keybind** — type a shortcut in palette to find its command
- [ ] **Keybindings editor** — UI panel to view all shortcuts, remap, add chords
- [ ] **Activity bar reorder** — drag icons to reorder panels
- [ ] **Panel resize persistence** — remember exact panel sizes across launches (not just on/off)
- [ ] **Full-screen toggle** — F11 toggles borderless full-screen window
- [ ] **Drag tab to split** — drag tab to viewport edge → create split
- [ ] **Tab context menu** — right-click tab: Close, Close Others, Close to the Right, Reveal in Explorer
- [ ] **Tooltip system** — hover tooltips on all toolbar buttons and status bar items
- [ ] **Progress bar in status bar** — spinning indicator during AI requests / LSP indexing
- [ ] **Notification center** — dismissed toasts accessible in a notification history panel
- [ ] **Window scale persistence** — remember zoom level across restarts
- [ ] **Multiple windows** — File > New Window opens second independent IDE window
- [ ] **Welcome screen updates** — show recent files, quick actions, getting-started links
- [ ] **About dialog** — show version, commit hash, Rust version, Floem version
- [ ] **Update checker** — notify when new version available; one-click self-update
- [ ] **Crash reporter** — on panic, write crash log to ~/.config/phazeai/crash.log

### Themes & Appearance
- [ ] **Icon theme system** — file icons per extension (folder, Rust crab, JS lightning, etc.)
- [ ] **Theme export** — export current customizations as a shareable TOML
- [ ] **Theme import** — import community theme TOML from file or URL
- [ ] **Custom color overrides** — override specific palette keys in settings without full theme
- [ ] **Color preview in settings** — show color swatches next to hex values in theme settings
- [ ] **Cursor style setting** — block / line / underline cursor shapes
- [ ] **Font ligatures toggle** — enable/disable ligatures (Fira Code, JetBrains Mono)
- [ ] **Line height setting** — configurable line-height multiplier (1.0 – 2.0)
- [ ] **Letter spacing setting** — configurable character spacing
- [ ] **Panel background blur** — frosted glass effect on panels (compositor-dependent)

### Language Support
- [ ] **Markdown preview** — Ctrl+Shift+V opens side-by-side rendered preview
- [ ] **Image preview** — clicking image file shows preview in editor tab
- [ ] **SVG preview** — render SVG files inline
- [ ] **Hex editor** — binary file view with hex + ASCII columns
- [ ] **CSV viewer** — tabular view for `.csv` / `.tsv` files
- [ ] **JSON pretty-print** — auto-format JSON files on open
- [ ] **TOML schema validation** — validate `Cargo.toml` / settings files against schema
- [ ] **Embedded language highlighting** — JS in HTML `<script>`, CSS in `<style>` tags
- [ ] **Language auto-detection** — detect language from shebang line (`#!/usr/bin/env python`)
- [ ] **File associations** — map custom extensions to languages in settings
- [ ] **Emmet** — HTML/CSS abbreviation expansion (e.g. `div.foo>p*3` → structure)
- [ ] **Path intellisense** — autocomplete file paths in strings as you type

### Snippets
- [ ] **Built-in snippets** — common snippets per language (fn, struct, if, for, etc.)
- [ ] **User snippets** — define custom snippets in `~/.config/phazeai/snippets/*.json`
- [ ] **Snippet variables** — `$TM_FILENAME`, `$CURRENT_DATE`, `$CLIPBOARD` etc.
- [ ] **Tab stops** — cursor jumps through `$1`, `$2`, `$3` placeholders on Tab
- [ ] **Snippet picker** — command palette shows all available snippets for current language

### Code Intelligence
- [ ] **Import auto-suggestions** — suggest adding missing imports for used symbols
- [ ] **Auto-import on completion** — selecting a completion item adds its import automatically
- [ ] **Quick fix lightbulb** — bulb icon appears on lines with available code actions
- [ ] **Extract variable** — refactor: wrap selection in `let x = <selection>`
- [ ] **Inline value display** — debugger: show variable values inline during debug session

### Workspace
- [ ] **Multi-root workspace** — open multiple unrelated folders in one window
- [ ] **Workspace trust** — prompt before running tasks from untrusted workspace
- [ ] **Recent files list** — File > Open Recent shows last N opened files/folders
- [ ] **Recent workspaces** — welcome screen shows recent workspace paths
- [ ] **Open at line** — open file from CLI with `phazeai file.rs:42` jumping to line 42
- [ ] **Workspace settings** — per-workspace `.phazeai/settings.toml` overrides global settings
- [ ] **Task runner** — define and run build/test tasks from `tasks.toml` in workspace
- [ ] **Problem matcher** — parse compiler output and populate Problems panel automatically

### Debug (DAP)
- [ ] **Debug adapter protocol** — connect to `lldb-vscode`, `codelldb`, `debugpy` etc.
- [ ] **Breakpoints** — click gutter to set/clear breakpoints; shown as red dots
- [ ] **Conditional breakpoints** — set break condition expression
- [ ] **Step over / into / out** — standard debugger navigation controls
- [ ] **Variables panel** — inspect local variables + their values during pause
- [ ] **Watch expressions** — user-defined expressions evaluated in debugger context
- [ ] **Call stack panel** — show current call stack during pause; click to navigate
- [ ] **Debug console** — REPL for evaluating expressions in current stack frame
- [ ] **Debug toolbar** — Continue / Pause / Stop / Restart controls
- [ ] **Inline variable values** — show current var values as ghost text inline after assignments

### Remote Development
- [ ] **SSH remote** — open folder on remote machine over SSH, editor stays local
- [ ] **SSH host picker** — UI to add/remove SSH hosts from `~/.ssh/config`
- [ ] **Remote proxy** — auto-upload `phazeai-proxy` binary to remote host
- [ ] **Remote LSP** — run language servers on the remote, not local machine
- [ ] **Remote terminal** — terminal connects to remote shell over SSH
- [ ] **Container / Dev Container** — detect `devcontainer.json` and offer to reopen in container
- [ ] **WSL support** — open WSL paths and run terminals in WSL on Windows

### Plugins / Extensions
- [ ] **Plugin architecture** — WASM-based plugin API (safe sandboxed execution)
- [ ] **Plugin registry** — built-in UI to search, install, update plugins
- [ ] **Plugin settings UI** — plugins expose settings rendered in Settings panel
- [ ] **Plugin icon painting** — plugins can register file icons and color theme entries
- [ ] **Tree-sitter grammar plugins** — plugins can ship additional language grammars
- [ ] **LSP plugin bridge** — plugins can register and manage additional LSP servers
- [ ] **Plugin enable/disable** — toggle plugins without uninstalling
- [ ] **Plugin update notifications** — notify when installed plugins have updates

### Accessibility
- [ ] **Screen reader support** — semantic ARIA roles on UI elements
- [ ] **High contrast mode** — dedicated high-contrast theme for visibility
- [ ] **Keyboard-only navigation** — every UI element reachable without mouse
- [ ] **Focus ring visibility** — clear visible focus indicator on all interactive elements
- [ ] **Zoom support** — respect system font scale settings

---

## 🔵 P3 — Nice-to-Have / Future

### Editor
- [ ] **Diff editor** — side-by-side diff view for any two files or git revisions
- [ ] **Merge conflict editor** — 3-way merge UI with Accept/Reject/Both buttons per hunk
- [ ] **Timeline view** — local edit history per file (like VS Code's Local History)
- [ ] **Breadcrumb navigation** — click path segment to navigate (already have display, add click-nav)
- [ ] **Hover card with actions** — hover shows type + docs + "Go to Def" / "Find Refs" buttons
- [ ] **Completion lens** — show top completion inline before user presses Ctrl+Space
- [ ] **Ghost text second line** — show multi-line FIM suggestion (not just first line)
- [ ] **Tab to accept ghost** — already implemented; add partial acceptance (accept one word)
- [ ] **Outline panel symbols tree** — hierarchical symbol tree (not just flat list)
- [ ] **Folding: custom regions** — `// #region`…`// #endregion` marker-based folding
- [ ] **Unicode character picker** — insert any Unicode codepoint by search
- [ ] **Emoji picker** — insert emoji from searchable palette

### Git
- [ ] **Interactive rebase** — `git rebase -i` UI for reordering/squashing commits
- [ ] **Git log graph** — ASCII-art branch graph in commit history panel
- [ ] **Conflict resolution UI** — inline "Accept Ours / Accept Theirs / Accept Both" controls
- [ ] **GitHub/GitLab PR integration** — view open PRs, create PR from current branch
- [ ] **Issue references** — detect `#123` in commit messages, make them links to GitHub
- [ ] **Signed commits** — GPG/SSH commit signing support

### AI Features
- [ ] **AI code review** — right-click selection → "Review with AI" → shows issues/improvements
- [ ] **AI explain selection** — explain what selected code does in plain English
- [ ] **AI test generation** — generate unit tests for selected function
- [ ] **AI documentation** — generate doc comments for selected function/struct
- [ ] **AI fix diagnostic** — one-click "Fix with AI" for any LSP error
- [ ] **AI refactor** — suggest refactoring approaches for selected code
- [ ] **AI commit message** — already implemented; improve with conventional commits format
- [ ] **Multi-file edits** — AI can propose changes across multiple files simultaneously
- [ ] **Agent mode** — AI autonomously edits, runs tests, iterates until tests pass
- [ ] **Context inclusion** — checkboxes to include open files / LSP diagnostics in AI context
- [ ] **Conversation history** — persist chat history per workspace
- [ ] **Model selector** — dropdown in chat panel to choose model per conversation
- [ ] **Token usage meter** — show input/output tokens used per request
- [ ] **AI search** — natural language query over codebase ("where is auth handled?")

### Distribution
- [ ] **AppImage build** — single-file Linux executable
- [ ] **`.deb` package** — Debian/Ubuntu installer
- [ ] **Flatpak** — sandboxed Flatpak on Flathub
- [ ] **AUR package** — Arch Linux AUR entry
- [ ] **Homebrew formula** — `brew install phazeai` on macOS
- [ ] **macOS DMG** — drag-to-Applications installer
- [ ] **Windows MSI** — Windows installer with PATH registration
- [ ] **Windows portable** — `.zip` no-install version
- [ ] **Auto-updater** — check for new release, download delta, restart
- [ ] **CLI: `phazeai` command** — open files/folders from terminal, `phazeai .` opens current dir
- [ ] **CLI: `--install-extension`** — install plugin from command line
