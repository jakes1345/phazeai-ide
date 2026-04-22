# Contributing to PhazeAI IDE

Thank you for your interest in contributing to PhazeAI IDE! We welcome contributions of all kinds, from bug reports and documentation to new features and performance optimizations.

## Quick Start

1. **Fork and Clone**
   ```bash
   git clone https://github.com/YOUR_USERNAME/phazeai-ide.git
   cd phazeai-ide
   ```

2. **Install Dependencies**
   - **Rust 1.70+** is required.
   - **Linux**: `sudo apt install build-essential libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev`
   - **macOS**: Xcode Command Line Tools.
   - **Windows**: MSVC or MinGW toolchain.

3. **Build and Run**
   ```bash
   # Primary Desktop IDE (Floem/GPU-rendered)
   cargo run -p phazeai-ui --release

   # Terminal UI (ratatui)
   cargo run -p phazeai-cli --release
   ```

## Project Structure

PhazeAI is a multi-crate Rust workspace:

- **`crates/phazeai-core`**: The engine. Contains the agent loop, LLM client implementations, tool definitions, and LSP integration.
- **`crates/phazeai-ui`**: The primary desktop IDE built with [Floem](https://github.com/lapce/floem).
- **`crates/phazeai-cli`**: The terminal-based UI built with `ratatui`.
- **`crates/phazeai-cloud`**: Client for PhazeAI cloud services (auth, hosted models).
- **`crates/phazeai-sidecar`**: Python-based semantic search subprocess.
- **`crates/phazeai-plugin-api`**: API for WASM-based extensions.
- **`crates/ollama-rs`**: A local fork of `ollama-rs` with custom streaming and history features.

## Architecture Notes (`phazeai-ui`)

If you are working on the GUI, keep these key files in mind:

- **`src/app.rs`**: The central "monolith" that manages `IdeState` and coordinates all panels.
- **`src/commands.rs`**: Centralized location for keyboard shortcuts and command palette actions.
- **`src/panels/`**: Individual modules for each IDE panel (Editor, Chat, Explorer, Git, etc.).
- **`src/lsp_bridge.rs`**: Manages communication with Language Servers.

## Development Workflow

### Code Style
We enforce standard Rust styling. Before submitting a PR, please run:
```bash
cargo fmt --all
cargo clippy --workspace -- -D warnings
```

### Running Tests
Ensure all tests pass before submitting changes:
```bash
cargo test --workspace
```

### Submitting Pull Requests
1. Create a new branch for your feature or bugfix: `git checkout -b feature/my-cool-feature`.
2. Commit your changes with descriptive messages.
3. Push to your fork and open a Pull Request against the `main` branch.
4. Ensure CI passes on your PR.

## What to Work On?
- Look for issues labeled **"good first issue"** or **"help wanted"**.
- Check the [Roadmap](./README.md#roadmap) in the README to see current priorities.
- Join our [Discord](https://discord.gg/phazeai) to discuss larger architectural changes before starting.

## License
By contributing to PhazeAI IDE, you agree that your contributions will be licensed under the **MIT License**.
