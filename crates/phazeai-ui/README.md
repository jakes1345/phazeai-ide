# phazeai-ui

The PhazeAI desktop IDE. It's built on [Floem](https://github.com/lapce/floem) (pinned to a git revision in the workspace `Cargo.toml`), which renders on the GPU through wgpu. The AI, LSP, search and privacy logic all live in `phazeai-core`. This crate is the UI around them.

The root [README](../../README.md) lists features, keybindings and configuration. This file covers the code.

## Build and run

```bash
cargo run -p phazeai-ui                    # debug
cargo build -p phazeai-ui --release        # target/release/phazeai-ui
cargo install --path crates/phazeai-ui     # install to ~/.cargo/bin
```

The workspace, the agent's file sandbox and the privacy policy all start from the git root of the directory you launch in, or the directory itself if it isn't a repo. Launching from `$HOME` gives the agent write access to everything under it.

## Layout

- `app.rs`: builds the window, the overlays (palette, file picker, completion popup, Ctrl+K inline edit, rename, toasts) and the root key handler. `app/` holds the overlay views, the menu bar and the Vim ex commands.
- `domain_state/`: `IdeState`, split into `WorkbenchState`, `EditorState`, `AiState` and `ProjectState`. Every field is a Floem `RwSignal`, so panels share state without locks.
- `commands.rs`: the command palette registry and the global shortcut table.
- `panels/`: one file per panel. `editor`, `chat`, `composer`, `explorer`, `git`, `blame`, `terminal`, `search`, `settings`, `privacy` and `local_ai` (both shown inside Settings), `design`, `run_debug`, `tests`, `extensions`, `makefile`, `containers`, `remote`, `github_actions`.
- `lsp_bridge.rs`: runs `LspManager` on its own thread and turns its results into signals.
- `debug_session.rs`: drives a DAP session and streams updates back to the UI.
- `ai_helpers.rs`: the text-only model calls (inline edit, commit message, terminal bar). They offer no tools.
- `theme.rs`, `custom_theme.rs`, `tokens.rs`: the 12 built-in themes, user themes from `~/.config/phazeai/themes/*.toml`, and the spacing and type scales.
- `crash_recovery.rs`, `notify.rs`, `components/`: unsaved-buffer recovery, notifications, shared widgets.

## Tests

```bash
cargo test -p phazeai-ui
```

The screenshot suite in `tests/tier2_integration.rs` starts the real binary under Xvfb and is ignored by default:

```bash
xvfb-run -a cargo test -p phazeai-ui --test tier2_integration -- --ignored --test-threads=1
```

Never run it without `xvfb-run`. It falls back to `DISPLAY=:0` and would drive your real desktop. It also rewrites the PNGs in `tests/snapshots/`, so restore them with `git checkout -- crates/phazeai-ui/tests/snapshots` before committing. [docs/STATUS.md](../../docs/STATUS.md) lists the packages it needs and how flaky it is.

Several older test files (`tier1_state.rs`, `lsp_tests.rs`, `terminal_tests.rs`) test copies of the logic rather than the real functions, so a green run there proves less than it looks.

## License

MIT. See `LICENSE` in the repository root.
