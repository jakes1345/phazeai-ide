# PhazeAI IDE

[![CI](https://github.com/jakes1345/phazeai-ide/actions/workflows/ci.yml/badge.svg)](https://github.com/jakes1345/phazeai-ide/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

PhazeAI is a code editor with an AI agent built in, written in Rust. The desktop app draws its UI on the GPU through [Floem](https://github.com/lapce/floem), so there is no Electron and no web view. A terminal UI shares the same engine.

I started it because I wanted an AI editor where I pick the model, keys stay on my machine, and I can see exactly what was sent. You can use a cloud provider with your own key, Ollama or LM Studio, or a small model that PhazeAI downloads and runs itself.

## Status

Pre-beta. It's usable for day-to-day coding on a single repo, but expect rough edges. Things I know are missing or wrong:

- Ctrl+K inline edit inserts the model's reply at the cursor. It does not replace the selection yet.
- Workspace symbols (Ctrl+T), Peek Definition and inlay hints don't show language-server results yet.
- The Settings panel lists 19 providers, but only the 9 under [AI providers](#ai-providers) can be made the active one.
- `.vsix` extensions install and get listed, but their themes, snippets and language configs are never applied. Editor syntax colours are fixed; the 12 themes recolour the rest of the UI.
- No installer has been tested. Build from source.

[docs/STATUS.md](docs/STATUS.md) has the full list with file references, and [ROADMAP.md](ROADMAP.md) has the longer-term plan.

## Quick start

You need Rust (the repo pins 1.93 in `rust-toolchain.toml`, so install through [rustup](https://rustup.rs/)) and, on Linux, the system packages under [Building](#building).

```bash
git clone https://github.com/jakes1345/phazeai-ide.git
cd phazeai-ide
cargo run -p phazeai-ui     # desktop IDE
cargo run -p phazeai-cli    # terminal UI (binary name: phazeai)
```

The first build downloads Floem from git and takes a while. A debug `target/` directory ends up around 14 GB.

With no config file, PhazeAI talks to Ollama at `http://localhost:11434` using `llama3.2:3b`. So the shortest path to a working setup is `ollama pull llama3.2:3b` and then launching. To use something else, open Settings with Ctrl+, or edit the config file described below.

## What works today

### Editor

- Tabs that are restored on the next launch, with syntect and tree-sitter highlighting.
- Find and replace in the file (Ctrl+F, Ctrl+H) with regex, whole-word and in-selection options.
- Multi-cursor with Ctrl+D, column cursors with Ctrl+Alt+Up/Down, code folding, bracket matching, split editor.
- Language servers are started when found on `PATH`: `rust-analyzer`, `pyright-langserver`, `typescript-language-server`, `gopls` and `clangd`. You get completions, hover, go to definition, references, code actions and diagnostics. Rename works, but has known problems with non-ASCII text and line endings (see STATUS).
- Vim mode: normal, insert and visual modes, marks, and `:w`, `:q`, `:wq`, `:x`, `:qa`, `:wqa`. No counts yet.
- 12 built-in UI themes, plus your own TOML themes in `~/.config/phazeai/themes/`.
- Unsaved buffers are kept in `~/.config/phazeai/crash-recovery/`.

### AI

The chat panel runs the full agent loop, so the model can read and edit files and run commands. When it wants to write a file you get a diff first, where you can keep or undo each changed part (the model is told what you left out). After a run, **Undo edits** on its last message puts back every file it wrote with its file tools, except any you've changed since; press it again to step back another run. Changes made by shell commands aren't tracked. The status bar counts the chat's tokens and cost live, call by call, with the share served from the provider's prompt cache; prices are the providers' own, read November 2026, and a model without a checked price shows tokens only. You can attach files with `@file`, chats are saved to `~/.phazeai/conversations/`, and long conversations are summarised automatically as they get close to the model's context limit. One request can take up to 200 model turns (`[agent] max_iterations`), and when the model asks for several reads or searches at once they run together instead of one after another. The chat and the CLI read the rules files other tools keep in a project, so none needs a copy for PhazeAI: `CLAUDE.md`, `AGENTS.md`, `GEMINI.md`, `.cursorrules` and `.cursor/rules`, Copilot's `.github/copilot-instructions.md` and `.github/instructions`, Windsurf's and Cline's rules, and `@path` imports of files inside the project (a repository's rules can't pull in files from elsewhere on your machine). Only the main file is sent with every request; the rest are listed one line each, with when they apply, for the model to read when its work needs them, and copies (a `CLAUDE.md` that is a symlink to `AGENTS.md`, say) are left out. For a search that would take many reads, the model can hand the question to a `subagent`: a helper on the provider's cheaper model (`[llm] small_model`) with only read-only file tools, whose answer alone comes back, so the files it read never fill the conversation; several asked at once run together, and their calls show on the meter at the helper's price. On longer tasks it keeps a `todo` checklist you can follow. Both sit behind `more_tools`, so they cost a line each per request, not a schema.

Tools that only read (files, search, `subagent`, `todo`) run without asking; reading is checked by the privacy policy. For the rest you can set rules once instead of answering the same question every time, in your `config.toml` (the format Claude Code's `permissions` use works too):

```toml
[permissions]
allow = ["bash(cargo test*)", "bash(git status)", "edit_file(src/**)"]
ask   = ["bash(git push*)"]                        # always asks, even in auto-approve modes
deny  = ["bash(rm -rf*)", "write_file(.github/**)", "fetch(domain:example.com)"]

[[hooks.post_tool]]                                # your own command after a call
tools = ["edit_file(*.rs)", "write_file(*.rs)"]
command = "cargo fmt"
```

Deny wins over ask, and ask over allow. A command like `cd x && cargo test` runs unasked only when every part is allowed and nothing is hidden in it (`$(...)`, backticks, `>` to a file); one denied part stops the whole command. A project can add its own `deny` and `ask` rules in `.phazeai/permissions.toml` or Claude Code's `.claude/settings.json`, but never `allow` rules or hooks, which would let a cloned repository run commands unasked. A `pre_tool` hook that exits with code 2 stops the call and tells the model why; a `post_tool` hook, which runs after a call that succeeded, can exit with 2 to hand the model what it printed on stderr (a linter's complaints, say). Hooks get the call as JSON on stdin. Rules for `bash` guard against mistakes; they are not a sandbox. A rule ending in `*` allows any arguments after it (`bash(cargo test*)` also allows `cargo test --config ...`, and cargo's `--config` can run other programs), and a deny rule matches the command as written, so `/bin/rm -rf` or `rm -r -f` gets past `bash(rm -rf*)` to the usual approval question instead of being refused.

Composer is the multi-file version, with three approval modes: auto-approve everything, ask for anything that isn't read-only, or ask for every call. The diff cards it shows at the end come from `git diff HEAD`, so they also include changes you made yourself before the run.

Also in the IDE:

- Inline edit on Ctrl+K, and a button in the Git panel that writes a commit message from the staged diff. Both are plain text calls to the model with no tools attached.
- Code completion, shown in a strip under the editor and inserted with Tab. It's on by default and sends the code around your cursor to the active provider. Turn it off with `[editor] ai_completions = false`, or "Toggle AI Completions" in the command palette.
- Code search for the agent: BM25 keyword search over the workspace, run in-process with nothing extra to install. It matches words, not meaning.
- Skills, which are Markdown files in `~/.config/phazeai/skills/` or `<project>/.phazeai/skills/` that the chat can switch into.
- MCP servers over stdio, listed in `<project>/.phazeai/mcp.json` as `{"servers": [{"name", "command", "args", "env"}]}`. CI tests this against the official filesystem server.
- A Design panel that turns a prompt into a self-contained HTML prototype in `.phazeai/designs/`, following `.phazeai/design-system.md` if you have one.

The built-in local model lives in Settings › Local AI. It checks your RAM and disk, downloads one of five Apache-2.0 GGUF models (from 1.5B up to a 35B mixture-of-experts), verifies the size and SHA-256, and runs it with llama.cpp's `llama-server`. The smallest one writes tool calls as plain text, so the agent can't reliably act through it.

### Privacy controls

Each project can have a `.phazeai/privacy.toml`:

```toml
deny = ["secrets/**", "*.sqlite"]   # gitignore-style globs, never sent to a model
local_only = false                   # true: only localhost providers are allowed
```

A built-in list of secret files (`.env` and similar) is always denied on top of that. The agent's tools, @mentions, inline edit, completions, code search and the commit-message diff all check the policy, and the agent can't edit the policy file itself. Every request to a provider is logged, and Settings › Privacy shows the log. If the file doesn't parse, PhazeAI switches to local-only until you fix it.

MCP tool calls are checked too, but only on a best-effort basis: a server can open files on its own that PhazeAI never sees.

### Everything else

- A PTY terminal with tabs, a split view and 256 colours. It renders lines rather than a full cell grid, so full-screen terminal apps may not draw correctly.
- Git panel: stage, unstage, discard, commit, branches, push, pull, fetch, stash, tags, merge, cherry-pick, log, blame and diffs. It shells out to the `git` CLI.
- Workspace search with `rg` if it's installed, or a plain substring scan if it isn't.
- Problems, References, Output and Test panels. The test panel runs `cargo test`.
- A debugger over DAP: breakpoints and stepping with `lldb-dap`, `lldb-vscode` or `gdb` 14 and newer. Run presets come from `.vscode/launch.json` plus a few Cargo commands.
- Small side panels for Makefile targets, Docker containers, SSH hosts from `~/.ssh/config`, and GitHub Actions runs.
- Native plugins (`cdylib`, see `crates/phazeai-plugin-api`) loaded from `~/.phazeai/plugins/`. They run in-process with full privileges, outside the privacy and approval rules.

### Terminal UI

`phazeai` is the same agent in a terminal: streaming Markdown, tool cards, an approval prompt with a diff preview, slash commands, saved sessions and seven colour themes. `phazeai -p "explain main.rs"` runs one prompt and prints the answer, and `--allow read|edit|all` decides what it may do without asking. See [crates/phazeai-cli/README.md](crates/phazeai-cli/README.md).

## Keybindings

Shortcuts in the desktop IDE. Other commands are in the command palette (Ctrl+Shift+P).

| Action | Keys |
|---|---|
| Go to file | Ctrl+P |
| Command palette | Ctrl+Shift+P |
| Settings | Ctrl+, |
| Save | Ctrl+S |
| New untitled file | Ctrl+N |
| Close tab | Ctrl+W |
| Find / replace in file | Ctrl+F / Ctrl+H |
| Find in files | Ctrl+Shift+F |
| Go to line | Ctrl+G |
| Toggle sidebar / bottom panel / chat panel | Ctrl+B / Ctrl+J / Ctrl+\ |
| Explorer / Source control | Ctrl+Shift+E / Ctrl+Shift+G |
| Split editor | Ctrl+Alt+\ |
| Zen mode | Ctrl+Shift+Z |
| Inline AI edit | Ctrl+K |
| Explain selection with AI | Ctrl+Shift+A |
| Select next occurrence | Ctrl+D |
| Toggle line comment | Ctrl+/ |
| Uppercase / lowercase / title case | Ctrl+Shift+U / Ctrl+Shift+L / Ctrl+Shift+T |
| Move line up / down | Alt+Up / Alt+Down |
| Word wrap | Alt+Z |
| Font size bigger / smaller / reset | Ctrl+= / Ctrl+- / Ctrl+0 |
| Trigger completion | Ctrl+Space |
| Code actions | Ctrl+. |
| Go to definition / references / implementation | F12 / Shift+F12 / Ctrl+F12 |
| Rename symbol | F2 |
| Next / previous problem | F8 / Shift+F8 |
| Toggle breakpoint | F9 |
| Continue / stop debugging | F5 / Shift+F5 |
| Step over / into / out | F10 / F11 / Shift+F11 |

## AI providers

These can be the active provider:

| Provider | Config value | Key |
|---|---|---|
| Anthropic Claude | `claude` | `ANTHROPIC_API_KEY` |
| OpenAI | `openai` | `OPENAI_API_KEY` |
| Google Gemini | `gemini` | `GEMINI_API_KEY` |
| Groq | `groq` | `GROQ_API_KEY` |
| Together.ai | `together` | `TOGETHER_API_KEY` |
| OpenRouter | `openrouter` | `OPENROUTER_API_KEY` |
| Ollama | `ollama` | none, `http://localhost:11434` |
| LM Studio | `lmstudio` | none, `http://localhost:1234` |
| Built-in model | `builtin` | none |

Keys are read from the OS keyring first (service name `phazeai`), then from the environment variable. A key you paste into Settings goes to the keyring, never into `config.toml`. Mistral, DeepSeek, xAI, Cohere, Perplexity, Azure OpenAI, Cerebras, Fireworks, SambaNova and GitHub Models only work through `[model_routes]` for now.

## Configuration

Settings live in `~/.config/phazeai/config.toml`. If the file doesn't exist PhazeAI uses defaults, and Settings writes it when you change something. A hand-written one needs at least the `[llm]` and `[editor]` tables:

```toml
[llm]
provider = "ollama"
model = "llama3.2:3b"
api_key_env = ""          # must be present, but keys come from the variables in the table above
max_tokens = 8192
# base_url = "http://192.168.1.20:11434"   # optional: provider on another address

[editor]
theme = "Midnight Blue"
ai_completions = true
# completion_model = "qwen2.5-coder:1.5b"   # optional: Ollama, DeepSeek or Mistral model for completions

[search]
enabled = true
auto_index = true

[updates]
check_at_start = true

[agent]
max_iterations = 200      # most model turns one request may take (the CLI's --max-turns overrides it)
```

If the file doesn't parse, PhazeAI prints a warning and falls back to the defaults.

Other places PhazeAI writes to:

| Path | What |
|---|---|
| `~/.config/phazeai/session.toml` | Open tabs and panel sizes |
| `~/.config/phazeai/logs/` | Desktop IDE logs, one file per day |
| `~/.config/phazeai/cli.log` | Terminal UI log |
| `~/.phazeai/conversations/` | Saved chats |
| `~/.phazeai/memory.json` | The agent's `memory` tool (global, not per project) |
| `~/.local/share/phazeai/` | Downloaded models and llama.cpp |
| `<project>/.phazeai/` | `privacy.toml`, `mcp.json`, `skills/`, `designs/` |

### Network use

Besides calls to the AI provider you configured, PhazeAI can make these requests:

- An update check against the GitHub releases API when the IDE starts. It's on by default. Set `[updates] check_at_start = false` to turn it off. It's also skipped when the folder you start in is local-only.
- An anonymous launch ping, only if you opt in with `PHAZEAI_TELEMETRY=1` or `telemetry = true` in `~/.config/phazeai/telemetry.toml`. It sends the app name, version, OS, CPU architecture and a random ID made fresh for each launch. No code or prompts.
- Model and llama.cpp downloads from Hugging Face and GitHub, when you start one from Settings › Local AI.
- The GitHub Actions panel fetches run status while it's open.

## Building

Linux is where this is developed and tested. CI also builds Windows binaries but runs no tests on Windows, and macOS isn't covered.

On Ubuntu or Debian, this covers the build and matches what CI installs:

```bash
sudo apt install build-essential pkg-config libssl-dev libxkbcommon-dev libxkbcommon-x11-dev \
  libwayland-dev libxcb-xfixes0-dev libxcb-shape0-dev libxcb-render0-dev \
  libudev-dev libinput-dev libdbus-1-dev
```

Then:

```bash
cargo build --workspace                      # debug build of everything
cargo build --release -p phazeai-ui          # release desktop binary: target/release/phazeai-ui
cargo install --path crates/phazeai-ui       # or install it to ~/.cargo/bin (same as make install-ide)
```

`install.sh` and the scripts under `scripts/` are out of date and fail.

## Tests

```bash
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

The GUI suite drives the real app under a virtual X server and compares screenshots. It needs `xvfb xdotool imagemagick fluxbox mesa-vulkan-drivers libgl1-mesa-dri`. Always run it through `xvfb-run`, because without it the suite falls back to your real display:

```bash
xvfb-run -a cargo test -p phazeai-ui --test tier2_integration -- --ignored --test-threads=1
```

It rewrites the PNGs in `crates/phazeai-ui/tests/snapshots/`, so run `git checkout -- crates/phazeai-ui/tests/snapshots` afterwards. Don't run `--ignored` across the whole workspace: the `ollama-rs` tests pull and delete models on a local Ollama.

Some workspace tests read your real `~/.config/phazeai/config.toml`.

## Workspace layout

```
crates/
├── phazeai-core/           engine: agent loop, LLM clients, tools, privacy policy,
│                           code search, LSP, DAP, MCP, built-in local model
├── phazeai-ui/             desktop IDE (Floem), binary phazeai-ui
├── phazeai-cli/            terminal UI and one-shot mode (ratatui), binary phazeai
├── phazeai-plugin-api/     ABI for native cdylib plugins
├── phazeai-plugin-canary/  test plugin used by the plugin tests
└── ollama-rs/              vendored fork of the ollama-rs crate
docs/                       MISSION.md and STATUS.md
training/, python/training/ model fine-tuning scripts (not used by the editor)
```

## Contributing

Issues and pull requests are welcome. Read [docs/STATUS.md](docs/STATUS.md) first, since the known problems there are the current work queue. [CONTRIBUTING.md](CONTRIBUTING.md) has the general workflow.

## License

MIT. See [LICENSE](LICENSE). `crates/ollama-rs` keeps its upstream MIT licence (`crates/ollama-rs/LICENSE.md`).
