# PhazeAI IDE

[![CI](https://github.com/jakes1345/phazeai-ide/actions/workflows/ci.yml/badge.svg)](https://github.com/jakes1345/phazeai-ide/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/Rust-1.93+-orange.svg)](https://www.rust-lang.org/)

**An AI-native IDE built in Rust. GPU-rendered, local-first, MIT-licensed core.**

PhazeAI is a code editor designed around AI-assisted development without the Electron bloat. The UI is GPU-accelerated via [Floem](https://github.com/lapce/floem) (Vello/wgpu); the agent loop, LSP client, MCP host, and tool registry are all Rust. You bring your own provider — local (Ollama, LM Studio) or cloud (Claude, OpenAI, Groq, Gemini, Together, OpenRouter).

> **Status: pre-beta.** The desktop GUI launches cleanly, the editor + LSP + chat + Composer paths work end-to-end, and the test/clippy/fmt baseline is green. See [BETA_AUDIT_TODO.md](./BETA_AUDIT_TODO.md) for the remaining gaps before a 1.0 tag.

---

## Quick Start

```bash
git clone https://github.com/jakes1345/phazeai-ide.git
cd phazeai-ide
cargo run -p phazeai-ui          # desktop IDE (primary)
cargo run -p phazeai-cli         # terminal UI (ratatui)
```

First launch creates `~/.config/phazeai/settings.toml`. Open the Settings panel (Ctrl+,) to point at your LLM provider.

---

## Features (working today)

**Editor**
- Multi-tab editing with persistent session, syntax highlighting (syntect, 25+ languages)
- Find / replace with regex (Ctrl+F / Ctrl+H), code folding, bracket matching, multi-cursor (Ctrl+D)
- LSP integration: completions, hover, go-to-def, rename, diagnostics, code actions, inlay hints
- Vim mode (normal/insert/visual, marks, ex commands)
- 12 built-in themes, VS Code theme `.vsix` import

**AI**
- Streaming chat panel against any configured LLM
- Inline AI edit (Ctrl+K) — describe a change, AI rewrites the selection in place
- Composer — multi-file agent loop with three approval modes (auto / approve-destructive / approve-all), tool diffs, cancel token, MCP server integration
- Multi-agent pipeline (planner → coder → reviewer) with per-stage provider config

**Tools & Infra**
- Real PTY terminal (portable-pty + VTE), 256-color, multi-tab
- Git panel: status, stage/discard with confirmation, commit, branch picker, diff view, blame
- Workspace search (ripgrep-backed), problems panel from LSP diagnostics
- Run & Debug panel (Cargo presets + `.vscode/launch.json` launch command parsing)
- Ops panels: Makefile targets, Docker containers/logs/shell, SSH hosts from `~/.ssh/config`
- Account panel: PhazeAI Cloud token + status wiring (keyring-backed)
- MCP (Model Context Protocol) client for plugging in external tool servers
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

---

## AI Providers

Configure in the Settings panel or `~/.config/phazeai/settings.toml`:

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

API keys you paste through the Settings panel are stored in the OS keyring (Secret Service / Keychain / Credential Manager), not in `settings.toml`.

**Recommended for new users:** install [Ollama](https://ollama.ai), `ollama pull llama3.2`, point PhazeAI at `http://localhost:11434`. Free, offline, no key.

---

## Build from Source

**Requirements:** Rust 1.93+ ([rustup](https://rustup.rs/))

**Linux deps:**
```bash
# Ubuntu/Debian
sudo apt install build-essential libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev

# Fedora
sudo dnf install gcc libxcb-devel libxkbcommon-devel

# Arch
sudo pacman -S base-devel libxcb libxkbcommon
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
├── phazeai-cloud/         cloud-tier client skeleton (in development)
├── phazeai-sidecar/       Python semantic-search subprocess
├── phazeai-plugin-api/    plugin ABI for native cdylib plugins
├── phazeai-plugin-canary/ smoke-test plugin used by tests
└── ollama-rs/             local fork with streaming + chat history
```

Config lives at `~/.config/phazeai/settings.toml`; session state at `~/.config/phazeai/session.toml`. Logs roll daily into `~/.config/phazeai/logs/`.

---

## Cloud Tier (in development)

The `phazeai-cloud` crate is a skeleton: the auth flow stores its API token in the OS keyring, but the backend (Stripe billing, hosted models, audit log, multi-tenant RLS, OIDC SSO) is not built yet. We're undecided between Cloudflare Workers + D1, Supabase, and a self-hosted Postgres+Stripe-Webhooks setup. Until that decision and implementation land, the cloud tier doesn't do anything you can't already do with a BYO-key cloud provider.

If you want PhazeAI to host inference for you, **track this in [BETA_AUDIT_TODO.md](./BETA_AUDIT_TODO.md)** — it's intentionally not on the roadmap below until the architecture is settled.

---

## Roadmap

| Status | Item |
|---|---|
| ✅ | Editor core + LSP + Vim mode + tabs + session persistence |
| ✅ | Streaming chat, inline AI edit, multi-agent pipeline |
| ✅ | Composer with approval modes, MCP integration, cancel + diff cards |
| ✅ | Git panel, terminal, problems, workspace search |
| ✅ | LSP/MCP watchdog with rate-capped restart and `did_open` replay |
| ✅ | Run & Debug + Makefile + Containers + Remote + Account side panels |
| ✅ | Tool sandbox confined to workspace root |
| 🚧 | Single canonical AI surface (consolidate `chat_panel` and `ai_panel`) |
| 🚧 | Conversation persistence for chat (composer chat is ephemeral by design) |
| ✅ | Unified global keyboard-shortcut dispatch in terminal and editor contexts |
| 🚧 | Cloud tier (architecture undecided — see above) |
| 📋 | Multi-line find, terminal split, multi-repo workspaces |
| 📋 | Integrated debugger (DAP) |
| 📋 | ACP (Agent Client Protocol) client — let any ACP server drive the chat panel |
| 📋 | Real-time collab (CRDT) |

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
A single anonymous launch ping (no payload, no IP retention) is sent for usage counting, behind a settings toggle. No code, prompts, or telemetry beyond that ever leaves your machine.

**Can I use it offline?**
Yes — point it at a local Ollama or LM Studio instance. The IDE itself never needs the network.

**Why Rust instead of Electron?**
~50MB native binary vs ~400MB+ Electron, GPU-rendered UI that doesn't drop frames at scale, no JS runtime to manage.
