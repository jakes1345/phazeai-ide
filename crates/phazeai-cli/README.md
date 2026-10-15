# phazeai-cli

The PhazeAI terminal UI (`phazeai`), built with ratatui + crossterm. It runs
the same `phazeai-core` agent loop, tools, providers and settings as the
desktop IDE.

## Features

- **Streaming chat** with Markdown rendering: headings, emphasis, inline code,
  links, nested / task lists, block quotes, tables and fenced code blocks with
  syntect highlighting. Text is word-wrapped to the terminal width.
- **Tool cards**: every tool call shows its arguments, a running spinner and
  elapsed time, then ✓ / ✗ with a one-line digest (exit code, match count,
  lines read…). `Ctrl+T` expands the output.
- **Approval prompt** with a preview of exactly what will happen — the shell
  command, or a coloured diff for `edit_file` / `write_file`. `y` yes, `a`
  always (bash: that exact command; other tools: the tool) for this session,
  `n` no, `Esc` stop the run.
- **Interrupt instantly** with `Esc` or `Ctrl+C` — even mid-request. The
  conversation is repaired so you can carry on. `Ctrl+C` on an empty prompt
  twice (or `Ctrl+D`) quits.
- **Prompt editor**: multi-line (`Shift+Enter`, `Alt+Enter`, `Ctrl+J`, or end a
  line with `\`), bracketed paste, readline keys, persistent history
  (`~/.config/phazeai/cli_history`), and messages typed while a response is
  running are queued.
- **Command palette**: type `/` to see matching commands with descriptions;
  `Tab` completes. `/help` lists everything.
- **Header and status line**: provider, model, directory, git branch, token
  usage, estimated cost, context usage and approval mode.
- **Friendly setup errors**: a missing key or an unreachable local server
  explains exactly how to fix it. `/key <api-key>` stores a key in the OS
  keyring; keys are never echoed or written to history.
- **Sessions** are saved to `~/.phazeai/conversations/`; resume with
  `phazeai -c`, `phazeai --resume <id>`, `/load`, or `Ctrl+O`.
- **Colour that adapts**: true colour, 256 or 16 colours depending on the
  terminal, and no colour at all with `NO_COLOR` or `--no-color`. Seven themes
  (`--theme` / `/theme`): dark, tokyo-night, dracula, catppuccin, gruvbox,
  nord, one-dark.
- The terminal is always restored on exit, error or panic; works with Windows
  terminals (crossterm).

## Build & Run

```bash
cargo run -p phazeai-cli                         # interactive TUI
cargo run -p phazeai-cli -- --provider ollama    # use a local model
cargo run -p phazeai-cli -- -p "explain main.rs" # one-shot, prints to stdout
git diff | cargo run -p phazeai-cli -- -p "review this"
```

Configuration lives in `~/.config/phazeai/config.toml`. API keys are read from
the OS keyring (service `phazeai`) or environment variables such as
`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, `GROQ_API_KEY`.
In the TUI, logs go to `~/.config/phazeai/cli.log` (set `RUST_LOG` for more).

## Architecture

- `main.rs` — argument parsing, logging, settings overrides
- `app/` — the TUI: `mod.rs` (state, event loop, agent events), `worker.rs`
  (agent task, approvals, interruption), `keys.rs`, `actions.rs` (slash
  command execution), `view.rs` (layout and drawing), `chat.rs` (transcript
  items and their rendering)
- `markdown.rs` — Markdown → wrapped ratatui lines, syntax highlighting
- `text.rs` — width-aware wrapping, truncation, sanitising tool output
- `input.rs` — multi-line prompt editor and history
- `commands.rs` — slash-command registry and parsing
- `tool_view.rs` — tool labels, result digests, diff previews, approval rules
- `setup.rs` — provider parsing, cost estimates, setup / error guidance
- `term.rs` — terminal setup and restore (including on panic)
- `theme.rs` — themes and colour-capability detection
- `companion.rs` — the buddy shown on tall terminals

## License

MIT — See LICENSE in repository root
