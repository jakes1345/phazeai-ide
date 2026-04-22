# Codebase Index

Indexed on 2026-03-24 in `/home/jack/phazeai_ide`.

## Executive Summary

This repository is an AI-first IDE project centered on a Rust workspace with:

- A shared engine in `crates/phazeai-core`
- A GPU desktop UI in `crates/phazeai-ui`
- A terminal UI in `crates/phazeai-cli`
- Optional cloud and sidecar integrations in `crates/phazeai-cloud` and `crates/phazeai-sidecar`
- A plugin API plus JavaScript/WASM extension-host experiments
- Python sidecar and model-training utilities
- Packaging and CI/release automation

The repo also contains a very large vendored/upstream tree under `phazeai-arsenal/` with 9,000+ files. That subtree is not part of the Rust workspace and should be treated as reference/vendor material unless a task explicitly targets it.

## Scale And Boundaries

Top-level file counts by major area:

| Area | Approx. files |
|---|---:|
| `phazeai-arsenal/` | 9062 |
| `crates/` | 214 |
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
| `_archive/` | Archived older code and experiments |
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

### `crates/ollama-rs`

Purpose: PhazeAI fork of `ollama-rs` with native tool-calling support.

Notable points:

- Package name: `ollama-rs-phazeai`
- Feature flags include `stream`, `chat-history`, `function-calling`
- Includes example programs and extensive integration-style tests

### `ext-host/wasm-extension`

Purpose: minimal WASM extension crate compiled as `cdylib`.

Likely role:

- Experimental or placeholder host/plugin target rather than a fully-developed subsystem.

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

Implementation notes from the code/docs:

- Pure-stdlib Python
- TF-IDF style search index
- Regex-based symbol extraction
- Reads/writes over stdio using JSON-RPC 2.0
- Skips common large/build directories during indexing

### `python/`

Purpose: additional Python support code outside the standalone sidecar folder.

Key files:

| Path | Role |
|---|---|
| `python/analyzer.py` | Large analysis utility |
| `python/embeddings.py` | Embedding-related helpers |
| `python/sidecar_server.py` | Alternate/related sidecar server implementation |
| `python/training/` | Expanded training, data collection, and research scripts |

Notable scale:

- `python/analyzer.py` is one of the larger first-party Python files
- `python/training/advanced_collect.py`, `train_pipeline.py`, `advanced_fine_tune.py`, and `sota_fine_tune.py` are substantial script surfaces

### `training/`

Purpose: top-level training pipeline for custom models.

Files:

- `training/prepare_data.py`
- `training/prepare_tool_data.py`
- `training/fine_tune.py`
- `training/export_gguf.py`
- `training/README.md`
- `training/datasets/*.jsonl`

Pipeline described in docs:

1. Prepare data from local code + public datasets
2. Fine-tune using QLoRA/Unsloth
3. Export GGUF
4. Register/test with Ollama

This top-level `training/` directory is separate from the larger `python/training/` experimentation toolkit.

## Packaging, Assets, And Model Files

### `packaging/`

| Path | Role |
|---|---|
| `packaging/flatpak/com.phazeai.IDE.json` | Flatpak manifest |
| `packaging/flatpak/com.phazeai.IDE.desktop` | Desktop entry |
| `packaging/flatpak/com.phazeai.IDE.metainfo.xml` | App metadata |
| `packaging/macos/build-dmg.sh` | macOS DMG packaging |
| `packaging/macos/entitlements.plist` | macOS entitlements |
| `packaging/windows/build-msi.ps1` | Windows MSI build script |
| `packaging/windows/phazeai-ide.wxs` | WiX installer definition |

### `modelfiles/`

Purpose: Ollama model definitions.

Files:

- `Modelfile.coder`
- `Modelfile.planner`
- `Modelfile.reviewer`
- `install.sh`

### `assets/`

Purpose: brand and desktop-launcher assets.

Files include:

- `branding/logo.png`
- `branding/icon.png`
- `branding/icon_256.png`
- `PhazeAI.desktop`

## CI And Release Automation

Workflow files:

- `.github/workflows/ci.yml`
- `.github/workflows/feature-tests.yml`
- `.github/workflows/release.yml`

### `ci.yml`

Primary checks include:

- `cargo fmt --all --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo audit`
- Crate-specific tests for core, CLI, UI, agent, git, MCP, sidecar
- Linux system dependency installation for GUI-related builds/tests

### `feature-tests.yml`

Broader scenario coverage includes:

- Tool-system tests
- Full MCP integration
- LSP integration
- Context-engine tests
- Provider registry tests
- Eval harness smoke checks
- Settings persistence
- Scheduled stress tests

### `release.yml`

Release flow:

- Builds cross-platform artifacts for Linux, macOS ARM/Intel, and Windows
- Packages both `phazeai-ui` and `phazeai`
- Publishes GitHub Release artifacts on version tags or manual dispatch

## Vendored / Reference Source

### `phazeai-arsenal/`

This directory is the biggest subtree in the repo and appears to be an internal arsenal/reference collection of upstream projects, not an active Cargo workspace member of the main product.

Major groups observed:

| Path | Contents |
|---|---|
| `phazeai-arsenal/ai-llm/async-openai` | async-openai workspace |
| `phazeai-arsenal/ai-llm/kalosm` | kalosm project |
| `phazeai-arsenal/ai-llm/llm-chain` | llm-chain |
| `phazeai-arsenal/ai-llm/mistral.rs` | mistral.rs ecosystem |
| `phazeai-arsenal/ai-llm/ollama-rs` | upstream ollama-rs |
| `phazeai-arsenal/ai-llm/rig` | rig framework plus skills |
| `phazeai-arsenal/ide-editor/egui` | egui |
| `phazeai-arsenal/ide-editor/helix` | Helix editor |
| `phazeai-arsenal/ide-editor/lapce` | Lapce editor |
| `phazeai-arsenal/ide-editor/syntect` | syntect |
| `phazeai-arsenal/ide-editor/zed` | Zed editor |

Recommendation for future work:

- Exclude `phazeai-arsenal/` from most searches, indexing, and refactors unless you explicitly need to inspect or import upstream reference code.

## Archived And Generated Areas

### `_archive/`

Contains older assets, experiments, tests, and build outputs. Treat as historical unless a task explicitly references it.

### `target/`

Rust build output. Not source.

### Observed untracked/generated items

Current git status showed:

- `.plandex-v2/`
- `rust_out`

These are not part of tracked source as of indexing time.

## Entry Points And Likely Developer Starting Points

Best starting files for understanding behavior:

- `README.md`
- `Cargo.toml`
- `crates/phazeai-core/src/lib.rs`
- `crates/phazeai-ui/src/app.rs`
- `crates/phazeai-ui/src/bin/phazeai-ui.rs`
- `crates/phazeai-cli/src/main.rs`
- `crates/phazeai-sidecar/src/lib.rs`
- `sidecar/server.py`
- `.github/workflows/ci.yml`

## Search Strategy For Future Tasks

Recommended default focus order:

1. `crates/phazeai-core/`
2. `crates/phazeai-ui/`
3. `crates/phazeai-cli/`
4. `crates/phazeai-sidecar/`
5. `ext-host/`
6. `sidecar/` and `python/`
7. `packaging/`, `modelfiles/`, workflow files

Recommended default exclusions:

- `phazeai-arsenal/`
- `_archive/`
- `target/`
- generated datasets or binaries unless directly relevant

## Current Assessment

This is a mixed monorepo with three distinct layers:

- Product code: Rust-first IDE/agent/runtime
- Support code: Python sidecars, training scripts, packaging, extension host
- Reference/vendor code: large imported arsenals for editor/LLM ecosystems

The most important architectural center of gravity is `crates/phazeai-core`, with `phazeai-ui` and `phazeai-cli` acting as the two primary user-facing frontends over the same engine.
