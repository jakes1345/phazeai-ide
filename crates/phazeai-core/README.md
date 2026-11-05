# phazeai-core

The engine behind the PhazeAI desktop IDE (`phazeai-ui`) and terminal UI (`phazeai-cli`). It is a library crate with no UI of its own.

## What's in it

| Module | What it does |
|---|---|
| `agent` | The streaming agent loop: send to the model, run the tool calls it asks for (through an approval function), repeat until it answers without tools. Supports cancellation and a diff hook for file writes. |
| `llm` | One `LlmClient` trait with clients for Claude, OpenAI-compatible APIs and Ollama, a provider registry, a per-task model router, context budgets and local server discovery. |
| `tools` | The built-in tools (`read_file`, `write_file`, `edit_file`, `bash`, `grep`, `glob`, `list_files`, `fetch`, `memory` and others) and `approval.rs`, which classes each call as ReadOnly, Write, Execute or Destructive. |
| `privacy` | The per-project policy in `.phazeai/privacy.toml`, the `GuardedClient` wrapper every production client goes through, the audit log, and the checks on MCP tool calls. |
| `search` | In-process BM25 keyword search over the workspace, exposed to the agent as `code_search` and `build_search_index`. |
| `context` | Conversation history and its on-disk store, system prompt building, context compaction, and the repo map. |
| `config` | `Settings`, read from `~/.config/phazeai/config.toml`. |
| `local_ai` | The built-in model: hardware probe, a pinned catalog of GGUF files with sizes and SHA-256 hashes, resumable verified downloads, and starting `llama-server`. |
| `lsp`, `dap` | Language server and debug adapter clients over stdio. |
| `mcp` | MCP client for stdio servers listed in `.phazeai/mcp.json`, with restarts capped per server. |
| `design` | Prompt-to-HTML prototypes for the IDE's Design panel. |
| `skills` | Loads skill files from `~/.config/phazeai/skills/` and `.phazeai/skills/`. |
| `fim` | Fill-in-the-middle requests for inline completion. |
| `ext_host` | Loads native `cdylib` plugins and installs `.vsix` files (their contents aren't applied yet). |
| `git`, `project`, `analysis`, `syntax` | Git helpers, workspace and file watching, outline and lint helpers, tree-sitter highlighting. |
| `telemetry`, `updater` | The opt-in launch ping and the GitHub release check. |

## Example

```rust
use phazeai_core::{Agent, AgentEvent, Settings, ToolRegistry};

let settings = Settings::load();
let client = settings.build_llm_client()?; // active provider, wrapped in the privacy guard

// An empty registry gives a plain chat with no tools.
let agent = Agent::new(client).with_tools(ToolRegistry::new());

let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
let (result, ()) = tokio::join!(agent.run_with_events("Explain this code", tx), async {
    while let Some(event) = rx.recv().await {
        println!("{event:?}"); // TextDelta, ToolStart, ToolResult, Complete, ...
    }
});
result?;
```

Be careful with `Agent::new` on its own. It comes with every built-in tool and no approval function, so the model can write files and run commands unasked. Either pass an empty or narrow `ToolRegistry`, or add `.with_approval(..)` with a closure that actually refuses calls that aren't read-only. `docs/MISSION.md` explains the rules the IDE follows.

## Tests

```bash
cargo test -p phazeai-core
```

Some tests read your real `~/.config/phazeai/config.toml`.

The MCP integration tests run against the official filesystem server and are ignored by default. To run them you need Node.js:

```bash
npm install --prefix /tmp/mcp-fs --no-save --ignore-scripts --before=2026-10-17 \
  @modelcontextprotocol/server-filesystem@1.0.0
PHAZEAI_MCP_FS_SERVER=/tmp/mcp-fs/node_modules/.bin/mcp-server-filesystem \
  cargo test -p phazeai-core --test mcp_server_integration -- --ignored
```

## License

MIT. See `LICENSE` in the repository root.
