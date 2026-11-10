# phazeai-cli

The PhazeAI terminal UI, binary name `phazeai`, built with ratatui and crossterm. It runs the same `phazeai-core` agent loop, tools, providers and settings as the desktop IDE, and it also has a one-shot mode for scripts.

## Build and run

```bash
cargo run -p phazeai-cli                         # interactive TUI
cargo run -p phazeai-cli -- --provider ollama    # pick a provider for this run
cargo run -p phazeai-cli -- -p "explain main.rs" # one prompt, answer on stdout
git diff | cargo run -p phazeai-cli -- -p "review this"
cargo install --path crates/phazeai-cli          # install phazeai to ~/.cargo/bin
```

Useful flags:

| Flag | Meaning |
|---|---|
| `-p, --prompt <text>` | Run one prompt and exit. Piped stdin is appended as context. |
| `--allow read\|edit\|all` | What `-p` mode may do without asking. `read` (the default) allows reading files and read-only commands, `edit` adds the file tools, `all` adds any shell command. Anything above that is asked about on the terminal, or refused when stdin is piped. |
| `--provider <name>` | `anthropic`, `openai`, `gemini`, `groq`, `together`, `openrouter`, `ollama`, `lmstudio` or `builtin`. |
| `-m, --model <name>` | Model for this run. |
| `-c, --continue` / `--resume <id>` | Pick up the last conversation, or one by ID prefix. |
| `--max-turns <N>` | The most model turns one request may take before the agent stops. Defaults to `[agent] max_iterations` in `config.toml`, which defaults to 200. |
| `--theme <name>` / `--no-color` | Colour theme, or no colour at all (`NO_COLOR` works too). |
| `--instructions <file>` | Extra instructions file for the system prompt. |

## Using it

Replies stream in as Markdown: headings, lists, tables, block quotes and fenced code with syntect highlighting, wrapped to the terminal width.

Each tool call gets a card with its arguments, a spinner and the elapsed time, then a one-line result (exit code, match count, lines read and so on). Ctrl+T expands the full output.

Before a tool runs that needs approval, you see exactly what it will do: the shell command, or a coloured diff for `edit_file` and `write_file`. Press `y` to allow it once, `a` to always allow it this session (for `bash` that means that exact command, for other tools the whole tool), `n` to refuse, or Esc to stop the run.

Esc or Ctrl+C stops a run, even in the middle of a request, and the conversation is repaired so you can carry on. With OpenAI-compatible providers the HTTP request itself may keep going in the background until the provider finishes (known problem 27 in `docs/STATUS.md`). Ctrl+C twice on an empty prompt, or Ctrl+D, quits.

The prompt is multi-line: Shift+Enter, Alt+Enter, Ctrl+J, or end a line with `\`. It handles bracketed paste and the usual readline keys, keeps history in `~/.config/phazeai/cli_history`, and queues messages you type while a reply is still streaming.

Type `/` to see the slash commands with descriptions, and Tab to complete one. `/help` lists them all. A few that come up often: `/model`, `/provider`, `/key`, `/add`, `/diff`, `/compact`, `/privacy`.

Other keys: Ctrl+O opens the session picker, Ctrl+B toggles the file list, Ctrl+E opens the prompt in your external editor, Ctrl+L clears the screen (the model keeps the conversation; `/clear` forgets it).

The header and status line show provider, model, directory, git branch, token usage, estimated cost, context usage and approval mode.

If a key is missing or a local server isn't reachable, the error says what to do about it. `/key <api-key>` stores a key in the OS keyring. Keys are never echoed or written to history.

Conversations are saved to `~/.phazeai/conversations/`. Resume one with `phazeai -c`, `phazeai --resume <id>`, `/load` or Ctrl+O.

Colour adapts to the terminal (true colour, 256 or 16 colours). There are seven themes, set with `--theme` or `/theme`: dark, tokyo-night, dracula, catppuccin, gruvbox, nord and one-dark. The terminal is restored on exit, on error and on panic.

## Configuration

Settings come from `~/.config/phazeai/config.toml`, the same file the desktop IDE uses. API keys are read from the OS keyring (service `phazeai`) first, then from environment variables such as `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY` and `GROQ_API_KEY`. MCP servers are read from `.phazeai/mcp.json` in the current directory, not the git root.

The TUI logs to `~/.config/phazeai/cli.log` (set `RUST_LOG` for more detail). `-p` mode logs to stderr.

## Layout

- `main.rs`: argument parsing, logging, settings overrides.
- `oneshot.rs`: `-p` mode and the `--allow` levels.
- `app/`: the TUI. `mod.rs` holds state, the event loop and agent events; `worker.rs` the agent task, approvals and interruption; `keys.rs`; `actions.rs` runs slash commands; `view.rs` does layout and drawing; `chat.rs` holds transcript items and their rendering.
- `markdown.rs`: Markdown to wrapped ratatui lines, with syntax highlighting.
- `text.rs`: width-aware wrapping, truncation, cleaning up tool output.
- `input.rs`: the multi-line prompt editor and history.
- `commands.rs`: slash command registry and parsing.
- `tool_view.rs`: tool labels, result digests, diff previews, approval rules.
- `setup.rs`: provider parsing, cost estimates, setup and error guidance.
- `term.rs`: terminal setup and restore, including on panic.
- `theme.rs`: themes and colour capability detection.
- `companion.rs`: the small character shown on tall terminals.

## License

MIT. See `LICENSE` in the repository root.
