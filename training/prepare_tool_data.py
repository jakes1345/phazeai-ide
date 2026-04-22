#!/usr/bin/env python3
"""
PhazeAI Tool-Calling Training Data Generator
=============================================
Generates high-quality training examples that teach the model HOW to use
PhazeAI's 17 tools in realistic multi-step agentic workflows.

This produces ChatML-format JSONL where the assistant's responses contain
actual tool calls matching PhazeAI's tool schemas.

Output: datasets/tool_calling_train.jsonl
"""

import json
import random
from pathlib import Path

OUTPUT_DIR = Path(__file__).parent / "datasets"

TOOL_CALL_EXAMPLES = [
    # ── READ → EDIT → VERIFY patterns ──────────────────────────────────
    {
        "messages": [
            {"role": "system", "content": "You are PhazeAI, an expert coding assistant with access to file system tools."},
            {"role": "user", "content": "Fix the unused import warning in src/main.rs"},
            {"role": "assistant", "content": "I'll read the file first to find the unused import.", "tool_calls": [
                {"name": "read_file", "arguments": {"path": "src/main.rs", "offset": 0, "limit": 50}}
            ]},
            {"role": "tool", "content": '{"content": "use std::collections::HashMap;\\nuse std::fmt;\\nuse std::io;\\n\\nfn main() {\\n    let name = \\"PhazeAI\\";\\n    println!(\\"Hello, {name}\\");\\n}\\n", "total_lines": 8}'},
            {"role": "assistant", "content": "`HashMap` and `fmt` are imported but never used. I'll remove them.", "tool_calls": [
                {"name": "edit_file", "arguments": {"path": "src/main.rs", "search": "use std::collections::HashMap;\nuse std::fmt;\nuse std::io;\n", "replace": "use std::io;\n"}}
            ]},
            {"role": "tool", "content": '{"success": true, "path": "src/main.rs"}'},
            {"role": "assistant", "content": "Now let me verify it compiles cleanly.", "tool_calls": [
                {"name": "bash", "arguments": {"command": "cargo check 2>&1 | tail -5"}}
            ]},
            {"role": "tool", "content": '{"stdout": "    Finished dev [unoptimized + debuginfo] target(s) in 0.3s", "exit_code": 0}'},
            {"role": "assistant", "content": "Done. Removed unused imports `HashMap` and `fmt` from `src/main.rs`. Build passes clean."}
        ]
    },

    # ── GREP → READ → FIX pattern ──────────────────────────────────────
    {
        "messages": [
            {"role": "system", "content": "You are PhazeAI, an expert coding assistant with access to file system tools."},
            {"role": "user", "content": "Find everywhere we use unwrap() and replace with proper error handling"},
            {"role": "assistant", "content": "I'll search for all `unwrap()` calls across the codebase first.", "tool_calls": [
                {"name": "grep", "arguments": {"pattern": "\\.unwrap()", "path": "src/", "regex": False}}
            ]},
            {"role": "tool", "content": '{"matches": [{"file": "src/config.rs", "line": 15, "content": "    let data = std::fs::read_to_string(path).unwrap();"}, {"file": "src/config.rs", "line": 22, "content": "    let config: Config = serde_json::from_str(&data).unwrap();"}, {"file": "src/server.rs", "line": 8, "content": "    let listener = TcpListener::bind(addr).unwrap();"}]}'},
            {"role": "assistant", "content": "Found 3 unwrap() calls. Let me fix `src/config.rs` first since it has two.", "tool_calls": [
                {"name": "read_file", "arguments": {"path": "src/config.rs", "offset": 10, "limit": 20}}
            ]},
            {"role": "tool", "content": '{"content": "pub fn load_config(path: &str) -> Config {\\n    let data = std::fs::read_to_string(path).unwrap();\\n    // ... more lines ...\\n    let config: Config = serde_json::from_str(&data).unwrap();\\n    config\\n}", "total_lines": 30}'},
            {"role": "assistant", "content": "I'll change the return type to `Result` and propagate errors properly.", "tool_calls": [
                {"name": "edit_file", "arguments": {"path": "src/config.rs", "search": "pub fn load_config(path: &str) -> Config {\n    let data = std::fs::read_to_string(path).unwrap();", "replace": "pub fn load_config(path: &str) -> Result<Config, Box<dyn std::error::Error>> {\n    let data = std::fs::read_to_string(path)?;"}}
            ]},
            {"role": "tool", "content": '{"success": true, "path": "src/config.rs"}'},
            {"role": "assistant", "content": "Now the second unwrap on the JSON parsing:", "tool_calls": [
                {"name": "edit_file", "arguments": {"path": "src/config.rs", "search": "    let config: Config = serde_json::from_str(&data).unwrap();\n    config", "replace": "    let config: Config = serde_json::from_str(&data)?;\n    Ok(config)"}}
            ]},
            {"role": "tool", "content": '{"success": true, "path": "src/config.rs"}'},
            {"role": "assistant", "content": "Fixed both unwrap() calls in config.rs. The function now returns `Result` and uses `?` for error propagation. Let me fix `src/server.rs` next.", "tool_calls": [
                {"name": "edit_file", "arguments": {"path": "src/server.rs", "search": "    let listener = TcpListener::bind(addr).unwrap();", "replace": "    let listener = TcpListener::bind(addr).map_err(|e| {\n        eprintln!(\"Failed to bind to {addr}: {e}\");\n        e\n    })?;"}}
            ]},
            {"role": "tool", "content": '{"success": true, "path": "src/server.rs"}'},
            {"role": "assistant", "content": "All 3 unwrap() calls replaced with proper error handling. `config.rs` now returns `Result`, `server.rs` propagates with a descriptive error message."}
        ]
    },

    # ── CREATE NEW MODULE pattern ──────────────────────────────────────
    {
        "messages": [
            {"role": "system", "content": "You are PhazeAI, an expert coding assistant with access to file system tools."},
            {"role": "user", "content": "Add a rate limiter module to the server"},
            {"role": "assistant", "content": "I'll check the current project structure first.", "tool_calls": [
                {"name": "list_files", "arguments": {"path": "src/"}}
            ]},
            {"role": "tool", "content": '{"files": ["main.rs", "server.rs", "config.rs", "handlers.rs", "middleware.rs"]}'},
            {"role": "assistant", "content": "I see there's already a `middleware.rs`. Let me read it to understand the pattern.", "tool_calls": [
                {"name": "read_file", "arguments": {"path": "src/middleware.rs"}}
            ]},
            {"role": "tool", "content": '{"content": "use std::future::Future;\\nuse std::pin::Pin;\\n\\npub trait Middleware: Send + Sync {\\n    fn handle(&self, req: Request) -> Pin<Box<dyn Future<Output = Response> + Send>>;\\n}\\n\\npub struct LoggingMiddleware;\\n\\nimpl Middleware for LoggingMiddleware {\\n    fn handle(&self, req: Request) -> Pin<Box<dyn Future<Output = Response> + Send>> {\\n        Box::pin(async move {\\n            println!(\\"[{}] {} {}\\", req.method, req.path, chrono::Utc::now());\\n            req.next().await\\n        })\\n    }\\n}\\n", "total_lines": 18}'},
            {"role": "assistant", "content": "The project uses a `Middleware` trait pattern. I'll create a `RateLimiter` that implements it, using a token bucket algorithm with `DashMap` for thread-safe per-IP tracking.", "tool_calls": [
                {"name": "write_file", "arguments": {"path": "src/rate_limiter.rs", "content": "use crate::middleware::{Middleware, Request, Response};\nuse dashmap::DashMap;\nuse std::future::Future;\nuse std::pin::Pin;\nuse std::sync::Arc;\nuse std::time::Instant;\n\nstruct TokenBucket {\n    tokens: f64,\n    last_refill: Instant,\n    max_tokens: f64,\n    refill_rate: f64,\n}\n\nimpl TokenBucket {\n    fn new(max_tokens: f64, refill_rate: f64) -> Self {\n        Self {\n            tokens: max_tokens,\n            last_refill: Instant::now(),\n            max_tokens,\n            refill_rate,\n        }\n    }\n\n    fn try_consume(&mut self) -> bool {\n        let now = Instant::now();\n        let elapsed = now.duration_since(self.last_refill).as_secs_f64();\n        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.max_tokens);\n        self.last_refill = now;\n\n        if self.tokens >= 1.0 {\n            self.tokens -= 1.0;\n            true\n        } else {\n            false\n        }\n    }\n}\n\npub struct RateLimiter {\n    buckets: Arc<DashMap<String, TokenBucket>>,\n    max_requests: f64,\n    refill_rate: f64,\n}\n\nimpl RateLimiter {\n    pub fn new(max_requests_per_window: u32, window_secs: u64) -> Self {\n        let max_requests = max_requests_per_window as f64;\n        let refill_rate = max_requests / window_secs as f64;\n        Self {\n            buckets: Arc::new(DashMap::new()),\n            max_requests,\n            refill_rate,\n        }\n    }\n}\n\nimpl Middleware for RateLimiter {\n    fn handle(&self, req: Request) -> Pin<Box<dyn Future<Output = Response> + Send>> {\n        let buckets = self.buckets.clone();\n        let max = self.max_requests;\n        let rate = self.refill_rate;\n\n        Box::pin(async move {\n            let ip = req.remote_addr().to_string();\n            let mut bucket = buckets\n                .entry(ip)\n                .or_insert_with(|| TokenBucket::new(max, rate));\n\n            if bucket.try_consume() {\n                req.next().await\n            } else {\n                Response::too_many_requests(\"Rate limit exceeded. Try again later.\")\n            }\n        })\n    }\n}\n"}}
            ]},
            {"role": "tool", "content": '{"success": true, "path": "src/rate_limiter.rs", "bytes_written": 1847}'},
            {"role": "assistant", "content": "Now I'll register the module in lib.rs:", "tool_calls": [
                {"name": "edit_file", "arguments": {"path": "src/main.rs", "search": "mod middleware;", "replace": "mod middleware;\nmod rate_limiter;"}}
            ]},
            {"role": "tool", "content": '{"success": true, "path": "src/main.rs"}'},
            {"role": "assistant", "content": "Created `src/rate_limiter.rs` with a token bucket implementation that:\n- Tracks per-IP request rates using `DashMap` (thread-safe)\n- Uses a configurable token bucket algorithm (requests per window)\n- Implements the existing `Middleware` trait for seamless integration\n- Returns 429 Too Many Requests when the limit is exceeded\n\nYou'll need to add `dashmap = \"5\"` to your `Cargo.toml` dependencies."}
        ]
    },

    # ── INVESTIGATION pattern (grep → read → analyze) ──────────────────
    {
        "messages": [
            {"role": "system", "content": "You are PhazeAI, an expert coding assistant with access to file system tools."},
            {"role": "user", "content": "How does authentication work in this project?"},
            {"role": "assistant", "content": "Let me search for authentication-related code.", "tool_calls": [
                {"name": "grep", "arguments": {"pattern": "auth|login|token|jwt|session|password", "path": "src/", "regex": True}}
            ]},
            {"role": "tool", "content": '{"matches": [{"file": "src/auth.rs", "line": 1, "content": "use jsonwebtoken::{encode, decode, Header, Validation, EncodingKey, DecodingKey};"}, {"file": "src/auth.rs", "line": 25, "content": "pub fn create_token(user_id: i64) -> Result<String, AuthError> {"}, {"file": "src/auth.rs", "line": 45, "content": "pub fn validate_token(token: &str) -> Result<Claims, AuthError> {"}, {"file": "src/handlers.rs", "line": 12, "content": "use crate::auth::validate_token;"}]}'},
            {"role": "assistant", "content": "Found `src/auth.rs` with JWT-based auth. Let me read the full implementation.", "tool_calls": [
                {"name": "read_file", "arguments": {"path": "src/auth.rs"}}
            ]},
            {"role": "tool", "content": '{"content": "use jsonwebtoken::{encode, decode, Header, Validation, EncodingKey, DecodingKey};\\nuse serde::{Serialize, Deserialize};\\n\\n#[derive(Debug, Serialize, Deserialize)]\\npub struct Claims {\\n    pub sub: i64,\\n    pub exp: usize,\\n}\\n\\npub struct AuthError(pub String);\\n\\nconst SECRET: &str = \\"my-secret-key\\";\\n\\npub fn create_token(user_id: i64) -> Result<String, AuthError> {\\n    let claims = Claims {\\n        sub: user_id,\\n        exp: (chrono::Utc::now() + chrono::Duration::hours(24)).timestamp() as usize,\\n    };\\n    encode(&Header::default(), &claims, &EncodingKey::from_secret(SECRET.as_bytes()))\\n        .map_err(|e| AuthError(e.to_string()))\\n}\\n", "total_lines": 22}'},
            {"role": "assistant", "content": "Here's how authentication works in this project:\n\n**Architecture**: JWT (JSON Web Token) based authentication in `src/auth.rs`.\n\n**Flow**:\n1. `create_token(user_id)` generates a JWT with a 24-hour expiry using HS256\n2. `validate_token(token)` decodes and validates incoming JWTs\n3. `src/handlers.rs` imports `validate_token` to protect endpoints\n\n**⚠️ Security Issue Found**: The JWT secret is hardcoded as `\"my-secret-key\"` on line 12. This should be loaded from an environment variable. Want me to fix that?"}
        ]
    },

    # ── BASH + DIAGNOSTICS pattern ─────────────────────────────────────
    {
        "messages": [
            {"role": "system", "content": "You are PhazeAI, an expert coding assistant with access to file system tools."},
            {"role": "user", "content": "The build is failing, can you figure out why?"},
            {"role": "assistant", "content": "Let me run the build and check the errors.", "tool_calls": [
                {"name": "bash", "arguments": {"command": "cargo build 2>&1 | head -30"}}
            ]},
            {"role": "tool", "content": '{"stdout": "error[E0308]: mismatched types\\n --> src/handlers.rs:42:12\\n  |\\n42 |     return Err(format!(\\"Not found: {}\\", id));\\n  |            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ expected `AppError`, found `String`\\n\\nerror: aborting due to 1 previous error", "exit_code": 101}'},
            {"role": "assistant", "content": "Type mismatch on line 42 of `src/handlers.rs` — returning a `String` where `AppError` is expected. Let me see the context.", "tool_calls": [
                {"name": "read_file", "arguments": {"path": "src/handlers.rs", "offset": 35, "limit": 15}}
            ]},
            {"role": "tool", "content": '{"content": "pub async fn get_item(id: i64) -> Result<Item, AppError> {\\n    let item = db::find_item(id).await?;\\n    match item {\\n        Some(i) => Ok(i),\\n        None => return Err(format!(\\"Not found: {}\\", id)),\\n    }\\n}", "total_lines": 7}'},
            {"role": "assistant", "content": "The function returns `Result<Item, AppError>` but line 42 returns a `String`. Need to wrap it in `AppError`.", "tool_calls": [
                {"name": "edit_file", "arguments": {"path": "src/handlers.rs", "search": "        None => return Err(format!(\"Not found: {}\", id)),", "replace": "        None => return Err(AppError::NotFound(format!(\"Not found: {}\", id))),"}}
            ]},
            {"role": "tool", "content": '{"success": true, "path": "src/handlers.rs"}'},
            {"role": "assistant", "content": "Let me verify it compiles now.", "tool_calls": [
                {"name": "bash", "arguments": {"command": "cargo check 2>&1 | tail -3"}}
            ]},
            {"role": "tool", "content": '{"stdout": "    Finished dev [unoptimized + debuginfo] target(s) in 1.2s", "exit_code": 0}'},
            {"role": "assistant", "content": "Fixed. The issue was returning a raw `String` instead of `AppError::NotFound(...)` in `get_item()`. Build passes now."}
        ]
    },

    # ── WEB SEARCH → IMPLEMENT pattern ─────────────────────────────────
    {
        "messages": [
            {"role": "system", "content": "You are PhazeAI, an expert coding assistant with access to file system tools."},
            {"role": "user", "content": "How do I implement graceful shutdown in tokio?"},
            {"role": "assistant", "content": "Let me search for the best practice.", "tool_calls": [
                {"name": "web_search", "arguments": {"query": "tokio graceful shutdown signal handler Rust 2024"}}
            ]},
            {"role": "tool", "content": '{"results": [{"title": "Graceful Shutdown - Tokio", "url": "https://tokio.rs/tokio/topics/shutdown", "snippet": "Use tokio::signal::ctrl_c() with tokio::select! to handle shutdown..."}]}'},
            {"role": "assistant", "content": "The standard pattern is `tokio::signal::ctrl_c()` with `tokio::select!`. Here's a production implementation:\n\n```rust\nuse tokio::signal;\nuse tokio::sync::watch;\n\npub async fn run_server() {\n    let (shutdown_tx, shutdown_rx) = watch::channel(false);\n\n    let server = tokio::spawn({\n        let mut shutdown_rx = shutdown_rx.clone();\n        async move {\n            loop {\n                tokio::select! {\n                    _ = accept_connection() => {}\n                    _ = shutdown_rx.changed() => {\n                        println!(\"Shutting down gracefully...\");\n                        break;\n                    }\n                }\n            }\n        }\n    });\n\n    signal::ctrl_c().await.expect(\"Failed to listen for ctrl+c\");\n    println!(\"Received shutdown signal\");\n    let _ = shutdown_tx.send(true);\n    let _ = server.await;\n}\n```\n\nThis uses a `watch` channel to broadcast the shutdown signal to all tasks. Want me to integrate this into your server?"}
        ]
    },

    # ── MEMORY (context persistence) pattern ───────────────────────────
    {
        "messages": [
            {"role": "system", "content": "You are PhazeAI, an expert coding assistant with access to file system tools."},
            {"role": "user", "content": "Remember that this project uses PostgreSQL with sqlx and the connection pool is in src/db.rs"},
            {"role": "assistant", "content": "Storing that context for future reference.", "tool_calls": [
                {"name": "memory", "arguments": {"action": "store", "key": "database_architecture", "content": "Project uses PostgreSQL with sqlx. Connection pool (PgPool) is initialized in src/db.rs. Use sqlx::query! macro for compile-time checked queries."}}
            ]},
            {"role": "tool", "content": '{"success": true, "message": "Stored memory under key: database_architecture"}'},
            {"role": "assistant", "content": "Got it. I've saved that the project uses PostgreSQL + sqlx with the pool in `src/db.rs`. I'll reference this architecture whenever working with database code."}
        ]
    },

    # ── FIND → GLOB → ANALYZE pattern ──────────────────────────────────
    {
        "messages": [
            {"role": "system", "content": "You are PhazeAI, an expert coding assistant with access to file system tools."},
            {"role": "user", "content": "Show me all the test files in the project"},
            {"role": "assistant", "content": "I'll search for test files using glob patterns.", "tool_calls": [
                {"name": "glob", "arguments": {"pattern": "**/*test*", "path": "."}}
            ]},
            {"role": "tool", "content": '{"matches": ["src/tests/mod.rs", "src/tests/auth_test.rs", "src/tests/api_test.rs", "tests/integration.rs"]}'},
            {"role": "assistant", "content": "Found 4 test files:\n\n| File | Type |\n|---|---|\n| `src/tests/mod.rs` | Unit test module root |\n| `src/tests/auth_test.rs` | Auth unit tests |\n| `src/tests/api_test.rs` | API unit tests |\n| `tests/integration.rs` | Integration tests |\n\nWant me to run the tests or look at any specific file?"}
        ]
    },

    # ── MULTI-FILE REFACTOR pattern ────────────────────────────────────
    {
        "messages": [
            {"role": "system", "content": "You are PhazeAI, an expert coding assistant with access to file system tools."},
            {"role": "user", "content": "Rename the User struct to Account everywhere"},
            {"role": "assistant", "content": "I'll first find every occurrence of `User` across the codebase.", "tool_calls": [
                {"name": "grep", "arguments": {"pattern": "\\bUser\\b", "path": "src/", "regex": True}}
            ]},
            {"role": "tool", "content": '{"matches": [{"file": "src/models.rs", "line": 5, "content": "pub struct User {"}, {"file": "src/models.rs", "line": 15, "content": "impl User {"}, {"file": "src/handlers.rs", "line": 3, "content": "use crate::models::User;"}, {"file": "src/handlers.rs", "line": 20, "content": "    let user: User = db.find(id).await?;"}, {"file": "src/db.rs", "line": 8, "content": "pub async fn find_user(id: i64) -> Result<User, DbError> {"}]}'},
            {"role": "assistant", "content": "Found `User` in 3 files. I'll rename in dependency order: models first, then consumers.", "tool_calls": [
                {"name": "bash", "arguments": {"command": "sed -i 's/\\bUser\\b/Account/g' src/models.rs src/handlers.rs src/db.rs && echo 'Done'"}}
            ]},
            {"role": "tool", "content": '{"stdout": "Done", "exit_code": 0}'},
            {"role": "assistant", "content": "Let me verify the build passes.", "tool_calls": [
                {"name": "bash", "arguments": {"command": "cargo check 2>&1 | tail -5"}}
            ]},
            {"role": "tool", "content": '{"stdout": "    Finished dev [unoptimized + debuginfo] target(s) in 0.8s", "exit_code": 0}'},
            {"role": "assistant", "content": "Renamed `User` → `Account` across all 3 files (`models.rs`, `handlers.rs`, `db.rs`). Build passes. The `sed` command used word boundaries (`\\b`) to avoid renaming things like `UserAgent` or `username`."}
        ]
    },
]


def generate_tool_calling_dataset():
    """Generate the tool-calling training dataset."""
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    output_path = OUTPUT_DIR / "tool_calling_train.jsonl"

    # Write each example
    entries = []
    for example in TOOL_CALL_EXAMPLES:
        entries.append(example)

    # Create variations with different programming languages
    lang_variations = [
        ("Python", "python", "pip install", "pytest", "def ", "import "),
        ("TypeScript", "typescript", "npm install", "jest", "function ", "import "),
        ("Go", "go", "go get", "go test", "func ", "import "),
    ]

    for lang, ext, pkg_cmd, test_cmd, fn_kw, imp_kw in lang_variations:
        # Read-edit-verify in different language
        entries.append({
            "messages": [
                {"role": "system", "content": "You are PhazeAI, an expert coding assistant with access to file system tools."},
                {"role": "user", "content": f"Fix the syntax error in main.{ext}"},
                {"role": "assistant", "content": "Let me read the file and find the error.", "tool_calls": [
                    {"name": "read_file", "arguments": {"path": f"src/main.{ext}"}}
                ]},
                {"role": "tool", "content": f'{{"content": "# {lang} source file\\n{imp_kw}os\\n\\n{fn_kw}main():\\n    print(\\"hello\\n", "total_lines": 5}}'},
                {"role": "assistant", "content": f"Missing closing quote on the print statement.", "tool_calls": [
                    {"name": "edit_file", "arguments": {"path": f"src/main.{ext}", "search": '    print("hello', "replace": '    print("hello")'}}
                ]},
                {"role": "tool", "content": f'{{"success": true, "path": "src/main.{ext}"}}'},
                {"role": "assistant", "content": f"Fixed the unclosed string literal in `main.{ext}`."}
            ]
        })

    # Shuffle for training diversity
    random.shuffle(entries)

    with open(output_path, "w") as f:
        for entry in entries:
            f.write(json.dumps(entry) + "\n")

    print(f"✅ Generated {len(entries)} tool-calling training examples")
    print(f"💾 Saved to {output_path}")
    print(f"📊 File size: {output_path.stat().st_size / 1024:.1f} KB")

    return entries


def merge_with_existing():
    """Merge tool-calling data into the combined training set."""
    combined_path = OUTPUT_DIR / "combined_train.jsonl"
    tool_path = OUTPUT_DIR / "tool_calling_train.jsonl"

    if not tool_path.exists():
        print("Run generate_tool_calling_dataset() first")
        return

    existing_entries = []
    if combined_path.exists():
        with open(combined_path) as f:
            for line in f:
                try:
                    existing_entries.append(json.loads(line.strip()))
                except json.JSONDecodeError:
                    continue

    tool_entries = []
    with open(tool_path) as f:
        for line in f:
            try:
                tool_entries.append(json.loads(line.strip()))
            except json.JSONDecodeError:
                continue

    # Weight tool-calling examples higher (repeat 5x) so the model
    # sees them more often during training
    weighted_tool_entries = tool_entries * 5

    all_entries = existing_entries + weighted_tool_entries
    random.shuffle(all_entries)

    with open(combined_path, "w") as f:
        for entry in all_entries:
            f.write(json.dumps(entry) + "\n")

    print(f"✅ Merged {len(tool_entries)} tool examples (5x weighted = {len(weighted_tool_entries)})")
    print(f"📊 Combined dataset: {len(all_entries)} total entries")


if __name__ == "__main__":
    generate_tool_calling_dataset()
    print()
    merge_with_existing()
