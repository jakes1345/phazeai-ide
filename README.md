# PhazeAI IDE

[![CI](https://github.com/jakes1345/phazeai-ide/actions/workflows/ci.yml/badge.svg)](https://github.com/jakes1345/phazeai-ide/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/Rust-1.93+-orange.svg)](https://www.rust-lang.org/)

**An AI-native IDE built in Rust. GPU-rendered, local-first, MIT-licensed core.**

PhazeAI is a code editor designed around AI-assisted development without the Electron bloat. The UI is GPU-accelerated via [Floem](https://github.com/lapce/floem) (Vello/wgpu); the agent loop, LSP client, MCP host, and tool registry are all Rust. You bring your own provider — local (Ollama, LM Studio) or cloud (Claude, OpenAI, Groq, Gemini, Together, OpenRouter).

> **Status: pre-beta.** The desktop GUI launches cleanly, the editor + LSP + chat + Composer + debugger paths work end-to-end, and the test/clippy/fmt baseline is green. It has not yet been tested on macOS or Windows hardware. See [BETA_AUDIT_TODO.md](./BETA_AUDIT_TODO.md) for the remaining gaps before a 1.0 tag.

---

## Quick Start

```bash
git clone https://github.com/jakes1345/phazeai-ide.git
cd phazeai-ide
cargo run -p phazeai-ui          # desktop IDE (primary)
cargo run -p phazeai-cli         # terminal UI (ratatui)
```

First launch creates `~/.config/phazeai/config.toml`. Open the Settings panel (Ctrl+,) to point at your LLM provider.

To install both apps for your user on Linux (`~/.local/bin` plus an app-menu entry), run `./install.sh` from the clone.

---

## Features (working today)

**Editor**
- Multi-tab editing with persistent session, syntax highlighting (syntect, 25+ languages)
- Find / replace with regex, multi-line patterns and find-in-selection (Ctrl+F / Ctrl+H), code folding, bracket matching, multi-cursor (Ctrl+D)
- LSP integration: completions, hover, go-to-def, rename, diagnostics, code actions, inlay hints
- Vim mode (normal/insert/visual, marks, ex commands)
- 12 built-in themes, VS Code theme `.vsix` import

**AI**
- Streaming chat panel against any configured LLM
- Inline AI edit (Ctrl+K) — describe a change, AI rewrites the selection in place
- Composer — multi-file agent loop with three approval modes (auto / approve-destructive / approve-all), tool diffs, cancel, MCP server integration
- Multi-agent pipeline (IDE: Composer → **Pipeline** toggle; CLI: `/pipeline <task>`): a Planner explores the repo and writes a plan, a Coder implements it with real file edits (same approval rules), the project's check (`cargo check`, `tsc`, `go build`, …) runs with up to 3 automatic fix rounds, and a Reviewer reads the diff of that run and approves or requests changes. Each stage can use a different model — see [Per-stage models](#per-stage-models)
- Saved chat history, cancel / retry, and a picker to switch models from the chat panel

**Tools & Infra**
- Real PTY terminal (portable-pty + VTE), 256-color, multi-tab, split panes, AI command bar (Ctrl+K in terminal)
- Git panel: status, stage/discard with confirmation, commit, branch picker, diff view, blame
- Workspace search (ripgrep-backed), problems panel from LSP diagnostics
- Run & Debug panel (Cargo presets + `.vscode/launch.json` launch command parsing)
- Debugger (DAP via `lldb-dap` / `lldb-vscode` / `gdb -i dap`): breakpoints in the gutter (F9), continue / step over / into / out (F5 / F10 / F11 / Shift+F11)
- Ops panels: Makefile targets, Docker containers/logs/shell, SSH hosts from `~/.ssh/config`
- MCP (Model Context Protocol) client for plugging in external tool servers — see [MCP servers](#mcp-servers)
- Watchdog: dead LSP and MCP servers are detected and restarted with rate-cap, with cached `did_open` replay

---

## Keybindings

| Action | Keys |
|---|---|
| Quick file picker | Ctrl+P |
| Command palette | Ctrl+Shift+P |
| Find / replace in file | Ctrl+F / Ctrl+H |
| Workspace search | Ctrl+Shift+F |
| Workspace symbols | Ctrl+T |
| Go to line | Ctrl+G |
| Inline AI edit | Ctrl+K |
| Toggle sidebar / terminal / chat panel | Ctrl+B / Ctrl+J / Ctrl+\ |
| Multi-cursor next | Ctrl+D |
| Toggle comment | Ctrl+/ |
| Go to definition / rename | F12 / F2 |
| Find references | Shift+F12 |
| Code actions | Ctrl+. |
| Toggle breakpoint | F9 |
| Debug: continue / step over / step into / step out | F5 / F10 / F11 / Shift+F11 |
| Save / save without formatting | Ctrl+S / command palette |

---

## AI Providers

Configure in the Settings panel or `~/.config/phazeai/config.toml`:

| Provider | Type | Setup |
|---|---|---|
| Anthropic Claude | Cloud | API key (`ANTHROPIC_API_KEY` or OS keyring) |
| OpenAI | Cloud | API key (`OPENAI_API_KEY`) |
| Google Gemini | Cloud | API key (`GEMINI_API_KEY`) |
| Groq | Cloud | API key (`GROQ_API_KEY`) |
| Together.ai | Cloud | API key (`TOGETHER_API_KEY`) |
| OpenRouter | Cloud | API key (`OPENROUTER_API_KEY`) |
| Ollama | Local | [Download](https://ollama.ai), `ollama pull <model>` |
| LM Studio | Local | [Download](https://lmstudio.ai) |

API keys you paste through the Settings panel are stored in the OS keyring (Secret Service / Keychain / Credential Manager), not in `config.toml`.

**Recommended for new users:** install [Ollama](https://ollama.ai), `ollama pull llama3.2`, point PhazeAI at `http://localhost:11434`. Free, offline, no key.

### Per-stage models

Different kinds of work can go to different models. Add routes to `config.toml`; anything without a route uses the active provider/model:

```toml
[model_routes.reasoning]        # Pipeline Planner
provider = "claude"
model = "claude-opus-4-7"

[model_routes.code_generation]  # Pipeline Coder
provider = "ollama"
model = "qwen2.5-coder:14b"

[model_routes.code_review]      # Pipeline Reviewer
provider = "openai"
model = "gpt-4.1"
```

### MCP servers

Servers listed in `~/.config/phazeai/mcp.json` always start. Servers in a project's `.phazeai/mcp.json` only start after you trust that project's exact server list — command palette **MCP: Trust Workspace Servers** in the IDE, `/mcp-trust` in the CLI — so opening a cloned repo can never launch programs by itself. Both files use:

```json
{ "servers": [ { "name": "github", "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"], "env": {} } ] }
```

---

## Build from Source

**Requirements:** Rust 1.93+ ([rustup](https://rustup.rs/))

**Linux deps:**
```bash
# Ubuntu/Debian
sudo apt install build-essential pkg-config libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libdbus-1-dev

# Fedora
sudo dnf install gcc pkgconf libxcb-devel libxkbcommon-devel dbus-devel

# Arch
sudo pacman -S base-devel libxcb libxkbcommon dbus
```

**Build:**
```bash
cargo build --workspace                      # everything
cargo build -p phazeai-ui --release          # desktop release binary
cargo test --workspace                       # tests
cargo clippy --workspace -- -D warnings      # lints
cargo fmt --all                              # format
```

GUI integration tests (tier 2) live in `crates/phazeai-ui/tests/` and require `xvfb-run`:
```bash
xvfb-run -a cargo test -p phazeai-ui --tests -- --ignored --test-threads=1
```

---

## Workspace Structure

```
crates/
├── phazeai-core/          shared engine: agent loop, LLM clients, tools, LSP, MCP
├── phazeai-ui/            desktop GUI (Floem + GPU)  ← PRIMARY
├── phazeai-cli/           terminal UI (ratatui)
├── phazeai-sidecar/       Python semantic-search subprocess
├── phazeai-plugin-api/    plugin ABI for native cdylib plugins
├── phazeai-plugin-canary/ smoke-test plugin used by tests
└── ollama-rs/             local fork with streaming + chat history
```

Config lives at `~/.config/phazeai/config.toml` (macOS: `~/Library/Application Support/phazeai/`, Windows: `%APPDATA%\phazeai\`); session state in `session.toml` and daily logs in `logs/` in the same folder.

---

## Cloud Tier (planned)

There is no hosted PhazeAI service yet: no accounts, billing or hosted models. Everything works with your own provider keys or local models. A cloud tier is on the roadmap but its architecture isn't decided, and the earlier client skeleton and Account panel were removed until there is a backend behind them.

---

## Roadmap

| Status | Item |
|---|---|
| ✅ | Editor core + LSP + Vim mode + tabs + session persistence |
| ✅ | Multi-line find, find-in-selection, split editors, terminal split |
| ✅ | Streaming chat with saved history, cancel / retry, inline AI edit |
| ✅ | Single canonical AI chat surface |
| ✅ | Composer with approval modes, MCP integration, cancel + diff cards |
| ✅ | Multi-agent pipeline: planner → coder → check/fix loop → reviewer, per-stage models |
| ✅ | Git panel, terminal, problems, workspace search |
| ✅ | Integrated debugger (DAP) |
| ✅ | LSP/MCP watchdog with rate-capped restart and `did_open` replay |
| ✅ | Run & Debug + Makefile + Containers + Remote side panels |
| ✅ | Tool sandbox confined to workspace root; credential stores off-limits; MCP workspace trust |
| ✅ | Unified global shortcut dispatch |
| 🚧 | macOS / Windows testing and signed installers |
| 🚧 | AI connection hardening (timeouts, mid-stream cancel, provider error surfacing) |
| 📋 | Multi-repo workspaces |
| 📋 | ACP (Agent Client Protocol) client — let any ACP server drive the chat panel |
| 📋 | Real-time collab (CRDT) |
| 📋 | Cloud tier (architecture undecided — see above) |

See [BETA_AUDIT_TODO.md](./BETA_AUDIT_TODO.md) for the day-to-day work tracker.

---

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md). PRs welcome, but please read the audit TODO first to avoid colliding with in-flight work.

---

## License

MIT. See [LICENSE](./LICENSE). All workspace dependencies are MIT or Apache-2.0.

---

## FAQ

**Is PhazeAI production-ready?**
No. It's pre-beta — usable for daily coding on a single repo, but the BETA_AUDIT_TODO list still has open P0/P1 items. Don't ship anything mission-critical with it yet.

**Does PhazeAI phone home?**
Telemetry is opt-in and off by default. If enabled (`PHAZEAI_TELEMETRY=1` or `~/.config/phazeai/telemetry.toml`), startup sends a launch ping containing app kind, version, OS, arch, and a per-launch random session ID. No source code or prompt content is sent.

**Can I use it offline?**
Yes — point it at a local Ollama or LM Studio instance. The IDE itself never needs the network.

**Why Rust instead of Electron?**
~50MB native binary vs ~400MB+ Electron, GPU-rendered UI that doesn't drop frames at scale, no JS runtime to manage.
