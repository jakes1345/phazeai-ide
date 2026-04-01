# phazeai-ui

The primary desktop IDE for PhazeAI — a GPU-accelerated, feature-rich code editor built with Floem (Vello/wgpu renderer) and backed by phazeai-core.

## Features

- **Multi-Tab Code Editor**: Syntax highlighting via syntect, vim keybindings, multi-cursor, bracket pair colorization, indent guides, code folding, bracket matching
- **AI Chat Panel**: Streaming responses from any LLM provider, conversation history browser, token usage display, context auto-compaction
- **AI Composer**: Multi-file AI agent that reads, writes, and edits code across your workspace with built-in approval UI
- **Terminal Emulator**: Full PTY support via portable-pty, VTE parsing, 256-color rendering, named tabs, hyperlink detection, shell profile selection
- **File Explorer**: Tree view with git awareness, file icons, exclude patterns (target/, node_modules/, etc.), drag-drop support, reveal in file manager
- **Git Integration**: Status, stage/commit, branch switching, merge, cherry-pick, stash, tag management, inline diff viewer, git blame, AI commit messages
- **Search**: Full-text search with file tree view, keyboard navigation, search history
- **Symbols & Outline**: LSP-backed symbol navigation, document outline, workspace symbol search
- **Settings & Themes**: 12 professional themes, font size control, auto-save, whitespace rendering, shell selection
- **Productivity Tools**: Command palette (Ctrl+K), file picker (Ctrl+P), workspace symbols (Ctrl+T), inline code edits, Zen mode, terminal split layout

## Build & Run

```bash
cargo build -p phazeai-ui           # debug build
cargo build -p phazeai-ui --release # release build
cargo run -p phazeai-ui             # launch IDE
```

## Architecture

- `app.rs` — Main IDE state (IdeState), all panels and overlays, keyboard dispatch
- `panels/` — Editor, chat, terminal, explorer, git, search, settings, composer
- `lsp_bridge.rs` — LSP manager integration with reusable LSP helpers
- `theme.rs` — 12 themes with PhazeTheme and PhazePalette system
- `components/` — Reusable UI components (buttons, inputs, etc.)

## License

MIT — See LICENSE in repository root
