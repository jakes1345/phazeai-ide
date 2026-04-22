# phazeai-sidecar

Rust bridge to a Python sidecar process for semantic search and code analysis — enables the Agent to search large codebases efficiently using embeddings.

## Features

- **Semantic Search Tool**: Finds relevant code snippets and documentation using embeddings
- **Index Building**: Auto-builds and updates indices for workspace files
- **JSON-RPC 2.0 Protocol**: Communicates with Python process over stdio with request/response pairs
- **Process Lifecycle Management**: SidecarManager handles startup, shutdown, and crash recovery
- **Async Integration**: Full async/await support via tokio for non-blocking tool calls

## Protocol

The sidecar speaks JSON-RPC 2.0:

```json
{
  "jsonrpc": "2.0",
  "id": "uuid",
  "method": "semantic_search",
  "params": {
    "query": "how to implement async iterators",
    "limit": 5
  }
}
```

Responses include matched file paths, line ranges, and relevance scores.

## Tools

- **SemanticSearchTool**: Searches indexed code for semantically similar content
- **BuildIndexTool**: Builds or updates the embedding index for a directory

## Usage

Both tools are automatically registered in the Agent's ToolRegistry and can be invoked by the LLM:

```rust
let manager = SidecarManager::new()?;
let search_tool = SemanticSearchTool::new(manager.client());
agent.register_tool(search_tool);
```

The sidecar is spawned on first tool use and stays alive for the session.

## Dependencies

- `phazeai-core` — Agent and tool infrastructure
- tokio, serde, UUID generation, async-trait

## Implementation

- `manager.rs` — SidecarManager lifecycle, process spawning
- `client.rs` — JSON-RPC client over stdio
- `protocol.rs` — Request/response serialization
- `tool.rs` — SemanticSearchTool and BuildIndexTool implementations

## License

MIT — See LICENSE in repository root
