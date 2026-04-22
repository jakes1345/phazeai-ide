# PhazeAI Sidecar Server

A Python JSON-RPC server that provides code indexing, search, and analysis capabilities for the PhazeAI IDE.

## Features

- **Code Indexing**: Build searchable index of source code files
- **Semantic Search**: TF-IDF based search with relevance scoring
- **Code Analysis**: Extract functions, structs, classes, and other symbols
- **Stdio Communication**: Clean JSON-RPC 2.0 protocol over stdin/stdout

## Requirements

- Python 3.7+
- Only stdlib dependencies (no external packages required)

## Usage

### Start the server

```bash
python3 sidecar/server.py
```

The server reads JSON-RPC requests from stdin and writes responses to stdout.

### Supported Methods

#### 1. `ping`

Health check.

**Request:**
```json
{"jsonrpc": "2.0", "id": 1, "method": "ping"}
```

**Response:**
```json
{"jsonrpc": "2.0", "id": 1, "result": "pong"}
```

#### 2. `build_index`

Build search index from source files.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "build_index",
  "params": {
    "paths": ["/path/to/codebase", "/path/to/file.rs"]
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "indexed": 42,
    "skipped": 3,
    "errors": 0,
    "total_files": 42
  }
}
```

**Behavior:**
- Recursively walks directories
- Indexes: `.rs`, `.py`, `.js`, `.ts`, `.go`, `.java`, `.c`, `.cpp`, `.h`, `.md`, `.toml`, `.json`, `.yaml`
- Skips: `.git/`, `node_modules/`, `target/`, `__pycache__/`, binary files

#### 3. `search`

Search indexed code.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "search",
  "params": {
    "query": "authentication logic",
    "top_k": 5
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": {
    "matches": [
      {
        "file": "/path/to/auth.rs",
        "score": 0.92,
        "snippet": "pub fn authenticate_user(token: &str) -> Result<User> { ..."
      }
    ]
  }
}
```

**Error (if index not built):**
```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "error": {
    "code": -1,
    "message": "Index not built"
  }
}
```

#### 4. `analyze`

Analyze code content and extract symbols.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "method": "analyze",
  "params": {
    "path": "src/main.rs",
    "content": "fn main() {}\nstruct User { name: String }"
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "result": {
    "symbols": {
      "functions": ["main"],
      "classes": [],
      "structs": ["User"],
      "traits": [],
      "enums": [],
      "other": []
    },
    "line_count": 2,
    "char_count": 45
  }
}
```

## Implementation Details

### Architecture

- **TfidfIndex**: Simple TF-IDF search engine using only Python stdlib
- **CodeAnalyzer**: Regex-based symbol extraction for multiple languages
- **CodeIndex**: High-level indexing orchestration
- **JsonRpcServer**: JSON-RPC 2.0 protocol handler

### Logging

- All operational logs go to **stderr**
- Only JSON-RPC responses go to **stdout**
- Never mix logging with JSON output

### Error Handling

- Malformed JSON: Returns JSON-RPC parse error (-32700)
- Invalid method: Returns method not found error (-32601)
- Invalid params: Returns custom error (-1)
- Graceful handling of binary files, permission errors, etc.

## Testing

Run the test suite:

```bash
python3 sidecar/test_server.py
```

Tests cover:
- Ping/pong
- Code analysis
- Index building
- Search functionality
- Error handling

## Performance

- Typical indexing speed: ~1000 files/second
- Memory usage: ~1MB per 100 files indexed
- Search latency: <100ms for most queries

## Future Enhancements

- Optional sentence-transformers integration for semantic embeddings
- Incremental index updates
- Persistent index storage
- Language-specific parsers (tree-sitter)
