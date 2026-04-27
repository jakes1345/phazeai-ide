# Codebase Index

Indexed on 2026-04-27 in `/app`.

## Executive Summary

This repository is an AI-first IDE project centered on a Rust workspace with:

- A shared engine in `crates/phazeai-core`
- A GPU desktop UI in `crates/phazeai-ui`
- A terminal UI in `crates/phazeai-cli`
- Optional cloud and sidecar integrations in `crates/phazeai-cloud` and `crates/phazeai-sidecar`
- A plugin API plus JavaScript/WASM extension-host experiments
- A canary plugin in `crates/phazeai-plugin-canary` for testing the plugin ABI
- Python sidecar and model-training utilities
- Packaging and CI/release automation

The repo also contains a very large vendored/upstream tree under `phazeai-arsenal/` with 9,000+ files. That subtree is not part of the Rust workspace and should be treated as reference/vendor material unless a task explicitly targets it.

## Scale And Boundaries

Top-level file counts by major area:

| Area | Approx. files |
|---|---:|
| `phazeai-arsenal/` | 9062 |
| `crates/` | 222 |
| `python/` | 33 |
| `ext-host/` | 11 |
| `training/` | 8 |
| `packaging/` | 7 |
| `modelfiles/` | 4 |
| `assets/` | 4 |
| `sidecar/` | 3 |
| `scripts/` | 3 |
| `.github/` | 3 |

Important practical boundary:

- First-party product code lives primarily in `crates/`, `ext-host/`, `sidecar/`, `python/`, `training/`, `packaging/`, and top-level build/config files.
- `phazeai-arsenal/` is a bundled collection of upstream editor/LLM projects including Zed, Helix, Lapce, egui, async-openai, rig, mistral.rs, kalosm, and related ecosystems.

## Top-Level Layout

| Path | Purpose |
|---|---|
| `Cargo.toml` | Root Rust workspace manifest |
| `Cargo.lock` | Rust dependency lockfile |
| `README.md` | Primary project overview and developer entrypoint |
| `Makefile` | Convenience build automation |
| `install.sh` | Local install/bootstrap script |
| `index.html`, `CNAME` | Lightweight web/release-site assets |
| `crates/` | Main Rust workspace crates |
| `ext-host/` | Node-based extension host prototype plus WASM extension crate |
| `sidecar/` | Python JSON-RPC sidecar server |
| `python/` | Extra Python utilities, analyzers, embeddings, training helpers |
| `training/` | Top-level model fine-tune/export data pipeline |
| `packaging/` | Flatpak, macOS DMG, Windows MSI packaging |
| `modelfiles/` | Ollama model definitions and installer |
| `scripts/`, `script/` | Shell utility scripts |
| `assets/` | Branding and desktop launcher assets |
| `.github/workflows/` | CI, feature, and release workflows |
| `phazeai-arsenal/` | Large vendored/reference source tree |
| `target/` | Rust build artifacts |

## Rust Workspace

Workspace members from the root `Cargo.toml`:

- `crates/phazeai-core`
- `crates/phazeai-sidecar`
- `crates/phazeai-cli`
- `crates/phazeai-ui`
- `crates/phazeai-cloud`
- `crates/ollama-rs`
- `crates/phazeai-plugin-api`
- `crates/phazeai-plugin-canary`
- `ext-host/wasm-extension`

Shared traits/dependencies at the workspace level:

- Async/runtime: `tokio`, `futures`, `async-trait`
- Serialization/config: `serde`, `serde_json`, `toml`
- HTTP/network: `reqwest`
- Diagnostics/errors: `tracing`, `thiserror`, `anyhow`
- Files/search: `ignore`, `globset`, `notify`, `regex`
- UI: `floem`, `floem-editor-core`, `ratatui`, `crossterm`
- AI/tooling: local forked `ollama-rs`

## First-Party Crates

### `crates/phazeai-core`

Purpose: shared engine for agenting, context assembly, tools, provider routing, project scanning, git, LSP, extension loading, and MCP.

Public modules from `src/lib.rs`:

- `agent`
- `analysis`
- `config`
- `constants`
- `context`
- `error`
- `ext_host`
- `git`
- `llm`
- `lsp`
- `mcp`
- `project`
- `tools`

Key sub-areas:

| Path | Role |
|---|---|
| `src/agent/` | Agent loop and multi-agent orchestration |
| `src/context/` | System prompt, history, persistence, repo-map building |
| `src/tools/` | File/system/network/search/tool abstractions |
| `src/llm/` | Provider clients and model routing |
| `src/lsp/` | LSP client/manager plumbing |
| `src/ext_host/` | JS/WASM/VSIX plugin loading and asset handling |
| `src/project/` | Workspace scanning and filesystem watching |
| `src/git/` | Git operations |
| `src/analysis/` | Outline/linter style source analysis |
| `src/mcp.rs` | MCP integration surface |

Notable tools implemented under `src/tools/`:

- `approval`
- `bash`
- `browse`
- `copy_path`
- `create_directory`
- `delete_path`
- `diagnostics`
- `download`
- `edit`
- `fetch`
- `file`
- `find_path`
- `glob`
- `grep`
- `list`
- `mcp_bridge`
- `memory`
- `move_path`
- `now`
- `open`
- `screenshot`
- `web_search`

Tests:

- `tests/agent_tests.rs`
- `tests/core_tests.rs`
- `tests/editor_tests.rs`
- `tests/ext_host_tests.rs`
- `tests/git_tests.rs`
- `tests/sprint3_verification.rs`
- `tests/stress_test.rs`
- `tests/tool_tests.rs`

### `crates/phazeai-ui`

Purpose: primary desktop UI built on Floem.

Entry points:

- Library: `src/lib.rs`
- Binary: `src/bin/phazeai-ui.rs`

Important behavior:

- Linux binary raises `RLIMIT_STACK` before constructing the Floem view tree, implying a very deep or large UI hierarchy.

Main source areas:

| Path | Role |
|---|---|
| `src/app.rs` | UI launch/application shell |
| `src/panels/` | IDE panels: editor, explorer, terminal, git, chat, search, settings, extensions |
| `src/components/` | Shared UI primitives |
| `src/lsp_bridge.rs` | UI-facing LSP integration |
| `src/theme.rs` | Theme system |
| `src/util.rs` | UI utilities |

Panel surface:

- `ai_panel`
- `chat`
- `composer`
- `editor`
- `explorer`
- `extensions`
- `git`
- `github_actions`
- `search`
- `settings`
- `terminal`

Tests:

- `tests/lsp_tests.rs`
- `tests/terminal_tests.rs`
- `tests/tier1_state.rs`
- `tests/tier2_integration.rs`
- Snapshot baselines under `tests/snapshots/`

### `crates/phazeai-cli`

Purpose: terminal UI and single-prompt CLI.

Entry points:

- Binary: `src/main.rs`
- Library: `src/lib.rs`

CLI capabilities visible in `main.rs`:

- Interactive TUI mode
- Single prompt execution via `--prompt`
- Provider/model overrides
- Continue/resume conversation options
- Optional extra instructions file
- Reads piped stdin and injects it into prompt context
- Auto-provisions `phaze-beast` when using Ollama with that model

Main source areas:

| Path | Role |
|---|---|
| `src/app.rs` | TUI execution paths |
| `src/commands.rs` | Command dispatch |
| `src/theme.rs` | Terminal theme configuration |

Tests:

- `tests/command_tests.rs`

### `crates/phazeai-sidecar`

Purpose: Rust-side manager/client for the Python sidecar process.

Modules:

- `client`
- `manager`
- `protocol`
- `tool`

Exports show the intended responsibilities:

- `SidecarClient`
- `SidecarManager`
- JSON-RPC request/response types
- `BuildIndexTool`
- `SemanticSearchTool`

Tests:

- `tests/sidecar_tests.rs`

### `crates/phazeai-cloud`

Purpose: optional cloud auth, subscriptions, and hosted-model client.

Modules:

- `auth`
- `client`
- `subscription`

Behavior:

- `cloud_api_url()` defaults to `https://api.phazeai.com/v1`
- Override supported via `PHAZEAI_CLOUD_URL`

### `crates/phazeai-plugin-api`

Purpose: native plugin interface contract shared between host and plugins.

Core types:

- `PluginHost`
- `PhazePlugin`
- `PluginCommand`
- `PluginEvent`
- `PluginManifest`
- `declare_plugin!` macro

This crate defines the ABI/protocol expectations for dynamic plugin loading and command/event dispatch.

### `crates/phazeai-plugin-canary`

Purpose: end-to-end canary for the PhazePlugin ABI. Exercises the API to ensure stability.

### `crates/ollama-rs`

Purpose: PhazeAI fork of `ollama-rs` with native tool-calling support.

Notable points:

- Package name: `ollama-rs-phazeai`
- Feature flags include `stream`, `chat-history`, `function-calling`
- Includes example programs and extensive integration-style tests

### `ext-host/wasm-extension`

Purpose: minimal WASM extension crate compiled as `cdylib`.

## Non-Rust Runtime Subsystems

### `ext-host/`

Purpose: Node.js extension host process.

Key files:

| Path | Role |
|---|---|
| `ext-host/package.json` | Declares Node commonjs host package |
| `ext-host/src/main.js` | Host process entrypoint |
| `ext-host/src/extension-loader.js` | Extension loading |
| `ext-host/src/rpc.js` | Host-side RPC layer |
| `ext-host/src/vscode-shim.js` | VS Code compatibility shim |
| `ext-host/dummy-extension/` | Example extension |
| `ext-host/test-extension.vsix` | Packaged VSIX test artifact |

### `sidecar/`

Purpose: Python JSON-RPC code indexing and search server.

Files:

- `sidecar/server.py`
- `sidecar/test_server.py`
- `sidecar/README.md`

Capabilities described and implemented:

- `ping`
- `build_index`
- `search`
- `analyze`

### `python/`

Purpose: additional Python support code outside the standalone sidecar folder.

Key files:

| Path | Role |
|---|---|
| `python/analyzer.py` | Large analysis utility |
| `python/embeddings.py` | Embedding-related helpers |
| `python/sidecar_server.py` | Alternate/related sidecar server implementation |
| `python/training/` | Expanded training, data collection, and research scripts |

### `training/`

Purpose: top-level training pipeline for custom models.

Files:

- `training/prepare_data.py`
- `training/prepare_tool_data.py`
- `training/fine_tune.py`
- `training/export_gguf.py`
- `training/README.md`
- `training/datasets/*.jsonl`

## CI And Release Automation

Workflow files:

- `.github/workflows/ci.yml`
- `.github/workflows/feature-tests.yml`
- `.github/workflows/release.yml`

## Vendored / Reference Source

### `phazeai-arsenal/`

This directory is an internal arsenal/reference collection of upstream projects, not an active Cargo workspace member.
