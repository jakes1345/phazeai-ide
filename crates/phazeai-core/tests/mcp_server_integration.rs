//! The MCP host against the real, official filesystem MCP server.
//!
//! Spawns `@modelcontextprotocol/server-filesystem` over stdio, sandboxed to a
//! fresh temp directory, and drives it only through production code:
//! `McpClient::connect` (initialize handshake and tools/list), `call_tool`,
//! and the `mcp__<server>__<tool>` bridges an `Agent` registers from an
//! `McpManager` loaded from `.phazeai/mcp.json`, exactly as the IDE and the
//! CLI do. The model is the only stand-in.
//!
//! Ignored by default because it needs Node and the server. A missing server
//! fails the tests; it never passes them. Run with:
//!
//! ```text
//! npm install --prefix /tmp/mcp-fs --no-save --ignore-scripts --before=2026-10-17 \
//!     @modelcontextprotocol/server-filesystem@1.0.0
//! PHAZEAI_MCP_FS_SERVER=/tmp/mcp-fs/node_modules/.bin/mcp-server-filesystem \
//!     cargo test -p phazeai-core --test mcp_server_integration -- --ignored
//! ```
//!
//! 1.0.0 lists read_file (deprecated), read_text_file, read_media_file,
//! read_multiple_files, write_file, edit_file, create_directory,
//! list_directory, list_directory_with_sizes, directory_tree, move_file,
//! search_files, get_file_info and list_allowed_directories.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};

use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use phazeai_core::mcp::{McpClient, McpManager, McpServerConfig, McpToolResult};
use phazeai_core::{
    Agent, LlmClient, LlmResponse, Message, PhazeError, StreamEvent, ToolDefinition,
};
use serde_json::{json, Value};

const SERVER_ENV: &str = "PHAZEAI_MCP_FS_SERVER";

/// Not valid JSON, so the bridge hands it back as text, and full of what a
/// framing or escaping bug would mangle: quotes, a backslash, non-ASCII and
/// newlines inside the message.
const NOTE: &str =
    "first line\n\"quoted\" and a back\\slash\nunicode: héllo ✓ 日本\nno newline at the end";
const SECRET: &str = "outside the sandbox: never read this";

/// The server's bin, or a failure that says how to get one.
fn server_executable() -> String {
    match std::env::var(SERVER_ENV) {
        Ok(path) if !path.trim().is_empty() => path,
        _ => panic!(
            "{SERVER_ENV} is not set. It must hold the absolute path of the \
             mcp-server-filesystem executable. Install the pinned server and run:\n\
             npm install --prefix /tmp/mcp-fs --no-save --ignore-scripts --before=2026-10-17 \
             @modelcontextprotocol/server-filesystem@1.0.0\n\
             {SERVER_ENV}=/tmp/mcp-fs/node_modules/.bin/mcp-server-filesystem \
             cargo test -p phazeai-core --test mcp_server_integration -- --ignored"
        ),
    }
}

/// The filesystem server, allowed to touch `allowed` and nothing else.
fn fs_server(allowed: &Path) -> McpServerConfig {
    McpServerConfig {
        name: "fs".to_string(),
        command: server_executable(),
        args: vec![allowed.to_str().expect("UTF-8 temp path").to_string()],
        env: HashMap::new(),
    }
}

fn texts(result: &McpToolResult) -> Vec<&str> {
    result
        .content
        .iter()
        .filter_map(|c| c.text.as_deref())
        .collect()
}

#[test]
#[ignore = "needs Node and @modelcontextprotocol/server-filesystem@1.0.0; set PHAZEAI_MCP_FS_SERVER to its bin"]
fn client_reads_inside_the_sandbox_and_is_refused_outside_it() {
    let allowed = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let note = allowed.path().join("notes.txt");
    std::fs::write(&note, NOTE).unwrap();
    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, SECRET).unwrap();

    let config = fs_server(allowed.path());
    let client = McpClient::connect(&config).unwrap_or_else(|e| {
        panic!(
            "connect to the filesystem server at {}: {e}",
            config.command
        )
    });

    // initialize: the handshake's answer was read.
    let info = client.server_info().expect("serverInfo from initialize");
    assert_eq!(info.name, "secure-filesystem-server");
    eprintln!("connected to {} {}", info.name, info.version);

    // tools/list: the read tool is there, with the input schema the server sent.
    let names: Vec<&str> = client.tools().iter().map(|t| t.name.as_str()).collect();
    let read = client
        .tools()
        .iter()
        .find(|t| t.name == "read_text_file")
        .unwrap_or_else(|| panic!("no read_text_file in {names:?}"));
    assert_eq!(
        read.input_schema["properties"]["path"]["type"], "string",
        "read_text_file's inputSchema was lost: {:?}",
        read.input_schema
    );

    // tools/call inside the allowed directory returns the file exactly.
    let result = client
        .call_tool("read_text_file", json!({ "path": note }))
        .expect("read_text_file inside the sandbox");
    assert!(!result.is_error, "{result:?}");
    assert_eq!(texts(&result), [NOTE]);

    // The same tool on an existing file outside it is refused by the server.
    let refused = client
        .call_tool("read_text_file", json!({ "path": secret }))
        .expect("a refusal is a tool result, not a transport error");
    assert!(refused.is_error, "read outside the sandbox: {refused:?}");
    let message = texts(&refused).join("\n");
    assert!(message.contains("Access denied"), "{message}");
    assert!(!message.contains(SECRET), "leaked the file: {message}");

    // Writes: allowed inside, refused outside with nothing created.
    let written = allowed.path().join("written.txt");
    let result = client
        .call_tool(
            "write_file",
            json!({ "path": written, "content": "from the agent" }),
        )
        .expect("write_file inside the sandbox");
    assert!(!result.is_error, "{result:?}");
    assert_eq!(std::fs::read_to_string(&written).unwrap(), "from the agent");

    let planted = outside.path().join("planted.txt");
    let refused = client
        .call_tool("write_file", json!({ "path": planted, "content": "x" }))
        .expect("a refusal is a tool result, not a transport error");
    assert!(refused.is_error, "write outside the sandbox: {refused:?}");
    assert!(
        texts(&refused).join("\n").contains("Access denied"),
        "{refused:?}"
    );
    assert!(!planted.exists(), "the server wrote outside its sandbox");

    assert!(client.is_alive());
}

/// One model turn's input: the conversation so far and the tools offered.
type Request = (Vec<Message>, Vec<ToolDefinition>);

/// A model that replays scripted turns and keeps every request it was sent,
/// so the test reads what the agent loop fed back after each tool call.
struct ScriptedModel {
    turns: Mutex<VecDeque<Vec<StreamEvent>>>,
    requests: Arc<Mutex<Vec<Request>>>,
}

#[async_trait::async_trait]
impl LlmClient for ScriptedModel {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse, PhazeError> {
        Err(PhazeError::Llm(
            "the agent loop streams; chat is not scripted".into(),
        ))
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<UnboundedReceiver<StreamEvent>, PhazeError> {
        self.requests
            .lock()
            .unwrap()
            .push((messages.to_vec(), tools.to_vec()));
        let turn = self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .expect("the agent asked for more turns than were scripted");
        let (tx, rx) = unbounded();
        for event in turn {
            tx.unbounded_send(event).unwrap();
        }
        Ok(rx)
    }
}

fn tool_call(id: &str, name: &str, arguments: Value) -> Vec<StreamEvent> {
    vec![
        StreamEvent::ToolCallStart {
            id: id.to_string(),
            name: name.to_string(),
        },
        StreamEvent::ToolCallDelta {
            id: id.to_string(),
            arguments_delta: arguments.to_string(),
        },
        StreamEvent::ToolCallEnd { id: id.to_string() },
        StreamEvent::Done,
    ]
}

fn tool_result<'a>(messages: &'a [Message], id: &str) -> &'a str {
    messages
        .iter()
        .find(|m| m.tool_call_id.as_deref() == Some(id))
        .map(|m| m.content.as_str())
        .unwrap_or_else(|| panic!("no tool result for {id} in {messages:#?}"))
}

#[tokio::test]
#[ignore = "needs Node and @modelcontextprotocol/server-filesystem@1.0.0; set PHAZEAI_MCP_FS_SERVER to its bin"]
async fn agent_runs_the_servers_tools_through_the_bridge() {
    const READ: &str = "mcp__fs__read_text_file";
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let note = workspace.path().join("notes.txt");
    std::fs::write(&note, NOTE).unwrap();
    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, SECRET).unwrap();

    // The IDE and the CLI read servers from <workspace>/.phazeai/mcp.json.
    std::fs::create_dir(workspace.path().join(".phazeai")).unwrap();
    std::fs::write(
        workspace.path().join(".phazeai/mcp.json"),
        json!({ "servers": [fs_server(workspace.path())] }).to_string(),
    )
    .unwrap();
    let configs = McpManager::load_config(workspace.path());
    assert_eq!(configs.len(), 1, "mcp.json should hold one server");
    let mut manager = McpManager::new();
    manager.connect_all(&configs);
    assert_eq!(
        manager.connection_count(),
        1,
        "the filesystem server did not connect; connect_all only logs why, \
         client_reads_inside_the_sandbox_and_is_refused_outside_it shows the error"
    );

    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = ScriptedModel {
        turns: Mutex::new(VecDeque::from([
            tool_call("call_1", READ, json!({ "path": note })),
            tool_call("call_2", READ, json!({ "path": secret })),
            vec![
                StreamEvent::TextDelta("done".to_string()),
                StreamEvent::Done,
            ],
        ])),
        requests: requests.clone(),
    };
    let mut agent = Agent::new(Box::new(model));
    agent.register_mcp_tools(Arc::new(Mutex::new(manager)));
    let response = agent.run("read my notes").await.expect("agent run");

    let requests = requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        3,
        "one model turn per tool call, then the answer"
    );

    // The model is offered the server's tool with the server's own schema.
    let offered = requests[0]
        .1
        .iter()
        .find(|t| t.name == READ)
        .unwrap_or_else(|| panic!("{READ} was not offered to the model"));
    assert_eq!(
        offered.parameters["properties"]["path"]["type"], "string",
        "{READ} reached the model without its schema: {:?}",
        offered.parameters
    );

    // Inside the sandbox the model gets the file's exact text back.
    let first = tool_result(&requests[1].0, "call_1");
    let value: Value = serde_json::from_str(first)
        .unwrap_or_else(|e| panic!("tool result is not the bridge's JSON ({e}): {first}"));
    assert_eq!(value["result"], NOTE, "{first}");

    // Outside it the server's refusal reaches the model as a failed call.
    let second = tool_result(&requests[2].0, "call_2");
    assert!(second.contains("Access denied"), "{second}");
    assert!(!second.contains(SECRET), "leaked the file: {second}");
    let outcomes: Vec<(&str, bool)> = response
        .tool_calls
        .iter()
        .map(|t| (t.tool_name.as_str(), t.success))
        .collect();
    assert_eq!(outcomes, [(READ, true), (READ, false)]);
    assert_eq!(response.content, "done");
}

// ── The privacy policy applied to MCP calls (recommendations A, B, C) ────────

use phazeai_core::privacy::{self, audit, GuardedClient};
use phazeai_core::ProviderId;
use std::path::PathBuf;

const ENV_SECRET: &str = "API_KEY=sk-live-0123-never-send";
const PLAN_SECRET: &str = "PLAN-CONFIDENTIAL-9c3f2a";
const CONTRACT_SECRET: &str = "CONTRACT-CONFIDENTIAL-7a1b";
const PRIVE_SECRET: &str = "PRIVE-CONFIDENTIAL-2d4e";
const NOTE_OK: &str = "ordinary project notes";

/// A path as a JSON string argument.
fn s(p: &Path) -> String {
    p.to_str().expect("UTF-8 path").to_string()
}

/// A project that denies `.env` (built-in), `private/**`, `Client Files/` and
/// `privé/`, with a secret behind each and one allowed file, and an mcp.json
/// pointing the filesystem server at the project root.
fn denied_project() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join(".phazeai")).unwrap();
    std::fs::write(
        root.join(".phazeai/privacy.toml"),
        "deny = [\"private/**\", \"Client Files/\", \"priv\u{e9}/\"]\n",
    )
    .unwrap();
    std::fs::write(root.join(".env"), ENV_SECRET).unwrap();
    std::fs::write(root.join(".env.example"), "EXAMPLE=placeholder\n").unwrap();
    std::fs::write(root.join("notes.txt"), NOTE_OK).unwrap();
    std::fs::create_dir_all(root.join("private")).unwrap();
    std::fs::write(root.join("private/plan.md"), PLAN_SECRET).unwrap();
    std::fs::create_dir_all(root.join("Client Files")).unwrap();
    std::fs::write(root.join("Client Files/contract.txt"), CONTRACT_SECRET).unwrap();
    std::fs::create_dir_all(root.join("priv\u{e9}")).unwrap();
    std::fs::write(root.join("priv\u{e9}/plan.txt"), PRIVE_SECRET).unwrap();
    std::fs::write(
        root.join(".phazeai/mcp.json"),
        json!({ "servers": [fs_server(&root)] }).to_string(),
    )
    .unwrap();
    (dir, root)
}

/// The active project is process-wide, so tests that set it run one at a time.
static ACTIVE: Mutex<()> = Mutex::new(());

struct ActiveGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        privacy::clear_active_project();
    }
}

fn activate(root: &Path) -> ActiveGuard {
    let lock = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    privacy::set_active_project(root);
    ActiveGuard { _lock: lock }
}

/// Build the agent exactly as the IDE does — load `.phazeai/mcp.json`, connect,
/// register the bridges — with the scripted model wrapped in `GuardedClient`
/// (as production wraps every client), run the scripted `calls` in order, and
/// return the response and every request the model was sent.
async fn run_calls(
    root: &Path,
    calls: &[(&str, &str, Value)],
) -> (phazeai_core::AgentResponse, Vec<Request>) {
    let configs = McpManager::load_config(root);
    assert_eq!(configs.len(), 1, "mcp.json should hold one server");
    let mut manager = McpManager::new();
    manager.connect_all(&configs);
    assert_eq!(
        manager.connection_count(),
        1,
        "the filesystem server did not connect; see \
         client_reads_inside_the_sandbox_and_is_refused_outside_it for the error"
    );

    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut turns: VecDeque<Vec<StreamEvent>> = calls
        .iter()
        .map(|(id, name, args)| tool_call(id, name, args.clone()))
        .collect();
    turns.push_back(vec![
        StreamEvent::TextDelta("done".to_string()),
        StreamEvent::Done,
    ]);
    let model = ScriptedModel {
        turns: Mutex::new(turns),
        requests: requests.clone(),
    };
    let guarded = GuardedClient::new(
        Box::new(model),
        ProviderId::Ollama,
        "scripted",
        "http://127.0.0.1:9",
    );
    let mut agent = Agent::new(Box::new(guarded)).with_max_iterations(64);
    agent.register_mcp_tools(Arc::new(Mutex::new(manager)));
    let response = agent.run("go").await.expect("agent run");
    let reqs = requests.lock().unwrap().clone();
    (response, reqs)
}

fn outcomes(response: &phazeai_core::AgentResponse) -> Vec<bool> {
    response.tool_calls.iter().map(|t| t.success).collect()
}

/// The parsed JSON of the tool result for `id`.
fn result_json(requests: &[Request], turn: usize, id: &str) -> Value {
    let raw = tool_result(&requests[turn].0, id);
    serde_json::from_str(raw)
        .unwrap_or_else(|e| panic!("tool result for {id} is not JSON ({e}): {raw}"))
}

#[tokio::test]
#[ignore = "needs Node and @modelcontextprotocol/server-filesystem@1.0.0; set PHAZEAI_MCP_FS_SERVER to its bin"]
async fn agent_never_sees_files_the_privacy_policy_denies() {
    let (_dir, root) = denied_project();
    let _active = activate(&root);
    let calls = vec![
        (
            "c1",
            "mcp__fs__read_text_file",
            json!({ "path": s(&root.join(".env")) }),
        ),
        ("c2", "mcp__fs__read_text_file", json!({ "path": ".env" })),
        (
            "c3",
            "mcp__fs__read_multiple_files",
            json!({ "paths": ["notes.txt", "private/plan.md"] }),
        ),
        (
            "c4",
            "mcp__fs__edit_file",
            json!({ "path": ".env", "edits": [{ "oldText": "API_KEY", "newText": "API_KEY" }], "dryRun": true }),
        ),
        (
            "c5",
            "mcp__fs__get_file_info",
            json!({ "path": "private/plan.md" }),
        ),
        (
            "c6",
            "mcp__fs__list_directory",
            json!({ "path": "private" }),
        ),
        ("c7", "mcp__fs__list_directory", json!({ "path": s(&root) })),
        ("c8", "mcp__fs__directory_tree", json!({ "path": s(&root) })),
        (
            "c9",
            "mcp__fs__search_files",
            json!({ "path": s(&root), "pattern": "**/*" }),
        ),
        (
            "c10",
            "mcp__fs__read_text_file",
            json!({ "path": "notes.txt" }),
        ),
    ];
    let (response, requests) = run_calls(&root, &calls).await;

    // Every denied read/list is refused; the three safe listings and the one
    // allowed read succeed.
    assert_eq!(
        outcomes(&response),
        vec![false, false, false, false, false, false, true, true, true, true],
    );

    // No secret appears in anything the model was sent.
    let seen = format!("{requests:?}");
    for secret in [ENV_SECRET, PLAN_SECRET, CONTRACT_SECRET, PRIVE_SECRET] {
        assert!(
            !seen.contains(secret),
            "a secret reached the model: {secret}"
        );
    }

    // The refusals are recorded, tagged with the MCP tool that tried.
    let blocked: Vec<(String, String)> = audit::recent(audit::CAPACITY)
        .into_iter()
        .filter_map(|e| match e.kind {
            audit::AuditKind::BlockedFile { path, via, .. } => Some((path, via)),
            _ => None,
        })
        .collect();
    assert!(
        blocked
            .iter()
            .any(|(p, v)| p == ".env" && v == "mcp__fs__read_text_file"),
        "{blocked:?}"
    );
    assert!(
        blocked
            .iter()
            .any(|(p, v)| p == "private/plan.md" && v == "mcp__fs__read_multiple_files"),
        "{blocked:?}"
    );

    // list_directory of the root keeps the allowed template, drops `.env`.
    let listing = result_json(&requests, 7, "c7");
    let listing = listing["result"].as_str().unwrap();
    assert!(listing.contains(".env.example"), "{listing}");
    assert!(
        !listing.lines().any(|l| l == "[FILE] .env"),
        "the listing named .env: {listing}"
    );

    // directory_tree drops the denied nodes.
    let tree = serde_json::to_string(&result_json(&requests, 8, "c8")).unwrap();
    assert!(tree.contains("notes.txt"));
    for gone in ["\".env\"", "plan.md", "plan.txt", "contract.txt"] {
        assert!(!tree.contains(gone), "the tree named {gone}: {tree}");
    }

    // search_files drops the denied paths and reports how many it hid.
    let search = result_json(&requests, 9, "c9");
    assert!(search["privacy_skipped"].as_u64().unwrap() >= 2, "{search}");
    let hits = search["result"].as_str().unwrap();
    assert!(hits.contains("notes.txt"));
    for gone in ["plan.md", "plan.txt", "contract.txt"] {
        assert!(!hits.contains(gone), "search named {gone}: {hits}");
    }

    // The allowed file reads back exactly.
    assert_eq!(result_json(&requests, 10, "c10")["result"], NOTE_OK);

    // The successful read is noted in the audit scope (shown in the UI as sent).
    assert!(
        audit::recent(audit::CAPACITY).iter().any(|e| matches!(
            &e.kind,
            audit::AuditKind::Request { model, files, .. }
                if model == "scripted" && files.iter().any(|f| f == "notes.txt")
        )),
        "the audit log did not note notes.txt as sent"
    );
}

#[tokio::test]
#[ignore = "needs Node and @modelcontextprotocol/server-filesystem@1.0.0; set PHAZEAI_MCP_FS_SERVER to its bin"]
async fn agent_cannot_move_denied_files_out_or_rewrite_the_policy() {
    let (_dir, root) = denied_project();
    let _active = activate(&root);
    let policy_text = std::fs::read_to_string(root.join(".phazeai/privacy.toml")).unwrap();

    let calls = vec![
        (
            "m1",
            "mcp__fs__move_file",
            json!({ "source": "private", "destination": "public" }),
        ),
        (
            "m2",
            "mcp__fs__write_file",
            json!({ "path": ".phazeai/privacy.toml", "content": "deny = []\n" }),
        ),
        (
            "m3",
            "mcp__fs__edit_file",
            json!({ "path": ".phazeai/privacy.toml", "edits": [{ "oldText": "private", "newText": "x" }] }),
        ),
        (
            "m4",
            "mcp__fs__move_file",
            json!({ "source": ".phazeai", "destination": "moved" }),
        ),
        (
            "m5",
            "mcp__fs__write_file",
            json!({ "path": "ok.txt", "content": "hi" }),
        ),
    ];
    let (response, _requests) = run_calls(&root, &calls).await;

    assert_eq!(outcomes(&response), vec![false, false, false, false, true]);

    // The denied directory was not moved out of the policy.
    assert!(
        root.join("private/plan.md").exists(),
        "the denied file was moved"
    );
    assert!(
        !root.join("public").exists(),
        "the denied directory was moved out"
    );
    // The policy file is byte-for-byte unchanged, and .phazeai was not moved.
    assert_eq!(
        std::fs::read_to_string(root.join(".phazeai/privacy.toml")).unwrap(),
        policy_text
    );
    assert!(!root.join("moved").exists());
    // A write that touches nothing protected still works.
    assert_eq!(std::fs::read_to_string(root.join("ok.txt")).unwrap(), "hi");
}

#[tokio::test]
#[ignore = "needs Node and @modelcontextprotocol/server-filesystem@1.0.0; set PHAZEAI_MCP_FS_SERVER to its bin"]
async fn server_spellings_of_denied_names_are_refused() {
    let (_dir, root) = denied_project();
    let _active = activate(&root);
    let calls = vec![
        (
            "u1",
            "mcp__fs__read_text_file",
            json!({ "path": "Client\u{a0}Files/contract.txt" }),
        ),
        (
            "u2",
            "mcp__fs__read_text_file",
            json!({ "path": "prive\u{301}/plan.txt" }),
        ),
        (
            "u3",
            "mcp__fs__write_file",
            json!({ "path": "Client\u{a0}Files/new.txt", "content": "x" }),
        ),
        (
            "u4",
            "mcp__fs__read_text_file",
            json!({ "path": "notes.txt" }),
        ),
    ];
    let (response, requests) = run_calls(&root, &calls).await;

    assert_eq!(outcomes(&response), vec![false, false, false, true]);

    // The Unicode-equivalent reads never returned the confidential contents.
    let seen = format!("{requests:?}");
    assert!(
        !seen.contains(CONTRACT_SECRET),
        "no-break-space read leaked"
    );
    assert!(!seen.contains(PRIVE_SECRET), "NFD read leaked");
    // The write through the no-break-space spelling created nothing.
    assert!(!root.join("Client Files/new.txt").exists());
    // The allowed file still reads.
    assert_eq!(result_json(&requests, 4, "u4")["result"], NOTE_OK);
}

#[tokio::test]
#[ignore = "needs Node and @modelcontextprotocol/server-filesystem@1.0.0; set PHAZEAI_MCP_FS_SERVER to its bin"]
async fn agent_cannot_launder_a_denied_path_through_dotdot() {
    // The server resolves `..` lexically before opening, even through a
    // directory that does not exist, so `nonexistent/../private` reaches
    // `private`. The check must resolve candidates the same way.
    let (_dir, root) = denied_project();
    let _active = activate(&root);
    let calls = vec![
        (
            "d1",
            "mcp__fs__read_text_file",
            json!({ "path": "nonexistent/../private/plan.md" }),
        ),
        (
            "d2",
            "mcp__fs__read_text_file",
            json!({ "path": "nonexistent/../prive\u{301}/plan.txt" }), // NFD é
        ),
        (
            "d3",
            "mcp__fs__move_file",
            json!({ "source": "nonexistent/../private", "destination": "public" }),
        ),
        (
            "d4",
            "mcp__fs__read_text_file",
            json!({ "path": "notes.txt" }),
        ),
    ];
    let (response, requests) = run_calls(&root, &calls).await;

    assert_eq!(outcomes(&response), vec![false, false, false, true]);
    let seen = format!("{requests:?}");
    for secret in [PLAN_SECRET, PRIVE_SECRET] {
        assert!(
            !seen.contains(secret),
            "a secret reached the model: {secret}"
        );
    }
    // The denied directory was not moved out of the policy.
    assert!(
        root.join("private/plan.md").exists(),
        "the denied file was moved out through `..`"
    );
    assert!(
        !root.join("public").exists(),
        "the denied directory was moved"
    );
    assert_eq!(result_json(&requests, 4, "d4")["result"], NOTE_OK);
}

#[tokio::test]
#[ignore = "needs Node and @modelcontextprotocol/server-filesystem@1.0.0; set PHAZEAI_MCP_FS_SERVER to its bin"]
async fn agent_cannot_name_denied_files_in_listings_through_unicode_or_dotdot() {
    let (_dir, root) = denied_project();
    let _active = activate(&root);
    let calls = vec![
        // A no-break-space spelling of a wholly denied directory.
        (
            "l1",
            "mcp__fs__list_directory",
            json!({ "path": "Client\u{a0}Files" }),
        ),
        // `..` through a missing directory onto a denied one.
        (
            "l2",
            "mcp__fs__list_directory",
            json!({ "path": "nonexistent/../private" }),
        ),
        // `..` that resolves the whole tree back to the project root.
        (
            "l3",
            "mcp__fs__directory_tree",
            json!({ "path": "nonexistent/.." }),
        ),
    ];
    let (response, requests) = run_calls(&root, &calls).await;

    // The two denied-directory listings are refused; the whole-tree listing
    // succeeds but is pruned.
    assert_eq!(outcomes(&response), vec![false, false, true]);
    let tree = serde_json::to_string(&result_json(&requests, 3, "l3")).unwrap();
    assert!(tree.contains("notes.txt"));
    for gone in ["\".env\"", "plan.md", "plan.txt", "contract.txt"] {
        assert!(!tree.contains(gone), "the tree named {gone}: {tree}");
    }
}

#[tokio::test]
#[ignore = "needs Node and @modelcontextprotocol/server-filesystem@1.0.0; set PHAZEAI_MCP_FS_SERVER to its bin"]
async fn agent_reads_non_ascii_arguments_without_dropping_the_server() {
    // A multi-byte character straddling byte 5 used to panic the path checker
    // and the result filter; repeated panics poisoned the client and burned
    // the server's restarts. Ordinary non-ASCII work must just succeed.
    let (_dir, root) = denied_project();
    let jp = "\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30c6}\u{30ad}\u{30b9}\u{30c8}";
    std::fs::write(root.join("jp.txt"), jp).unwrap();
    let _active = activate(&root);
    let calls = vec![
        ("j1", "mcp__fs__read_text_file", json!({ "path": "jp.txt" })),
        (
            "j2",
            "mcp__fs__write_file",
            json!({ "path": "\u{65e5}\u{672c}\u{8a9e}.txt", "content": "x" }),
        ),
        ("j3", "mcp__fs__read_text_file", json!({ "path": "jp.txt" })),
        (
            "j4",
            "mcp__fs__read_text_file",
            json!({ "path": "notes.txt" }),
        ),
    ];
    let (response, requests) = run_calls(&root, &calls).await;

    assert_eq!(outcomes(&response), vec![true, true, true, true]);
    assert_eq!(result_json(&requests, 1, "j1")["result"], jp);
    // The server was never dropped: the last read still works.
    assert_eq!(result_json(&requests, 4, "j4")["result"], NOTE_OK);
}

#[tokio::test]
#[ignore = "needs Node and @modelcontextprotocol/server-filesystem@1.0.0; set PHAZEAI_MCP_FS_SERVER to its bin"]
async fn agent_reads_allowed_file_contents_verbatim() {
    // An allowed file's contents are never edited, even when a line is itself a
    // denied absolute path (a `.gitignore` rule `/.env`) — it is a mention, not
    // the secret, and dropping it would corrupt the file the model reads.
    let (_dir, root) = denied_project();
    std::fs::write(root.join(".gitignore"), "/target\n/.env\nnode_modules/\n").unwrap();
    let _active = activate(&root);
    let calls = vec![
        (
            "g1",
            "mcp__fs__read_text_file",
            json!({ "path": ".gitignore" }),
        ),
        (
            "g2",
            "mcp__fs__read_multiple_files",
            json!({ "paths": [".gitignore", "notes.txt"] }),
        ),
    ];
    let (response, requests) = run_calls(&root, &calls).await;

    assert_eq!(outcomes(&response), vec![true, true]);
    let g1 = result_json(&requests, 1, "g1");
    assert_eq!(g1["result"], "/target\n/.env\nnode_modules/\n");
    assert!(g1.get("privacy_skipped").is_none(), "{g1}");
    let g2 = result_json(&requests, 2, "g2");
    let joined = g2["result"].as_str().unwrap_or_default();
    assert!(
        joined.contains("/.env") && joined.contains(NOTE_OK),
        "read_multiple_files mangled an allowed file: {joined}"
    );
    assert!(g2.get("privacy_skipped").is_none(), "{g2}");
}

#[tokio::test]
#[ignore = "needs Node and @modelcontextprotocol/server-filesystem@1.0.0; set PHAZEAI_MCP_FS_SERVER to its bin"]
async fn listing_errors_reach_the_model_as_errors_not_refusals() {
    // The server answers a listing of a missing directory (or of a file) with
    // an error result. That message is not a listing: the model must get the
    // server's error, as it does from read_text_file, not a privacy refusal.
    let (_dir, root) = denied_project();
    // directory_tree stops at the first descendant it can't open and quotes
    // that path in its error. A chain deeper than PATH_MAX inside the denied
    // `private/` makes the server's realpath fail with ENAMETOOLONG on a path
    // the policy denies, so the error itself names a denied directory.
    let level = format!("deep-denied-{}", "q".repeat(188));
    let chain = vec![level.as_str(); 22].join("/");
    let made = std::process::Command::new("mkdir")
        .arg("-p")
        .arg(&chain)
        .current_dir(root.join("private"))
        .status()
        .expect("run mkdir");
    assert!(made.success(), "mkdir -p could not build the deep chain");
    let _active = activate(&root);
    let calls = vec![
        (
            "e1",
            "mcp__fs__list_directory",
            json!({ "path": "does_not_exist" }),
        ),
        (
            "e2",
            "mcp__fs__list_directory_with_sizes",
            json!({ "path": "does_not_exist" }),
        ),
        (
            "e3",
            "mcp__fs__directory_tree",
            json!({ "path": "does_not_exist" }),
        ),
        (
            "e4",
            "mcp__fs__list_directory",
            json!({ "path": "notes.txt" }),
        ),
        ("e5", "mcp__fs__directory_tree", json!({ "path": s(&root) })),
        (
            "e6",
            "mcp__fs__read_text_file",
            json!({ "path": "notes.txt" }),
        ),
    ];
    let (response, requests) = run_calls(&root, &calls).await;

    assert_eq!(
        outcomes(&response),
        vec![false, false, false, false, false, true]
    );
    for (turn, id, code) in [
        (1, "e1", "ENOENT"),
        (2, "e2", "ENOENT"),
        (3, "e3", "ENOENT"),
        (4, "e4", "ENOTDIR"),
        (5, "e5", "ENAMETOOLONG"),
    ] {
        let text = tool_result(&requests[turn].0, id);
        assert!(
            !text.contains("REFUSED") && !text.contains("could not check"),
            "{id}: the server's error was reported as a privacy refusal: {text}"
        );
        assert!(text.contains(code), "{id}: {text}");
    }
    // The denied path in directory_tree's error never reaches the model.
    let e5 = tool_result(&requests[5].0, "e5");
    assert!(
        !e5.contains("deep-denied"),
        "the error named a denied path: {e5}"
    );
    assert!(
        e5.contains("hidden by the project's privacy policy"),
        "{e5}"
    );
    // A server error is not a privacy refusal, so it is not logged as one.
    let logged: Vec<String> = audit::recent(audit::CAPACITY)
        .into_iter()
        .filter_map(|e| match e.kind {
            audit::AuditKind::BlockedFile { path, .. } => Some(path),
            _ => None,
        })
        .filter(|p| p.contains("does_not_exist") || p == "notes.txt")
        .collect();
    assert!(logged.is_empty(), "logged as refused: {logged:?}");
    assert_eq!(result_json(&requests, 6, "e6")["result"], NOTE_OK);
}

#[tokio::test]
#[ignore = "needs Node and @modelcontextprotocol/server-filesystem@1.0.0; set PHAZEAI_MCP_FS_SERVER to its bin"]
async fn a_listing_the_filter_cannot_parse_is_refused_and_logged() {
    // A file name holding a newline splits its entry across two lines, so the
    // listing filter can't tell what the second line names. It refuses the
    // listing (fail closed) and, like every refusal, records it in the audit
    // log the user reads in Settings › Privacy.
    let (_dir, root) = denied_project();
    std::fs::create_dir(root.join("odd names")).unwrap();
    std::fs::write(root.join("odd names/two\nlines.txt"), "x").unwrap();
    let _active = activate(&root);
    let calls = vec![
        (
            "p1",
            "mcp__fs__list_directory",
            json!({ "path": "odd names" }),
        ),
        (
            "p2",
            "mcp__fs__list_directory_with_sizes",
            json!({ "path": s(&root.join("odd names")) }),
        ),
        (
            "p3",
            "mcp__fs__read_text_file",
            json!({ "path": "notes.txt" }),
        ),
    ];
    let (response, requests) = run_calls(&root, &calls).await;

    assert_eq!(outcomes(&response), vec![false, false, true]);
    for (turn, id) in [(1, "p1"), (2, "p2")] {
        let text = tool_result(&requests[turn].0, id);
        assert!(
            text.contains("could not check this listing"),
            "{id}: {text}"
        );
    }
    let blocked: Vec<(String, String)> = audit::recent(audit::CAPACITY)
        .into_iter()
        .filter_map(|e| match e.kind {
            audit::AuditKind::BlockedFile { path, via, .. } => Some((path, via)),
            _ => None,
        })
        .collect();
    for via in [
        "mcp__fs__list_directory",
        "mcp__fs__list_directory_with_sizes",
    ] {
        assert!(
            blocked.iter().any(|(p, v)| p == "odd names" && v == via),
            "the refused listing ({via}) is not in the audit log: {blocked:?}"
        );
    }
    assert_eq!(result_json(&requests, 3, "p3")["result"], NOTE_OK);
}

#[tokio::test]
#[ignore = "needs Node and @modelcontextprotocol/server-filesystem@1.0.0; set PHAZEAI_MCP_FS_SERVER to its bin"]
async fn agent_cannot_hide_a_denied_path_behind_padding_or_a_line_break() {
    // The server resolves `..` before opening, however long the string is, so
    // `a/a/.../../../.env` past 4096 bytes opens `.env`. A file name may also
    // hold a line break. Neither may skip the check.
    let (_dir, root) = denied_project();
    std::fs::write(root.join("private/two\nlines.txt"), PLAN_SECRET).unwrap();
    let _active = activate(&root);
    let padded = format!("{}{}.env", "a/".repeat(2000), "../".repeat(2000));
    let calls = vec![
        ("l1", "mcp__fs__read_text_file", json!({ "path": padded })),
        (
            "l2",
            "mcp__fs__read_text_file",
            json!({ "path": "private/two\nlines.txt" }),
        ),
        (
            "l3",
            "mcp__fs__read_text_file",
            json!({ "path": "notes.txt" }),
        ),
    ];
    let (response, requests) = run_calls(&root, &calls).await;

    assert_eq!(outcomes(&response), vec![false, false, true]);
    let seen = format!("{requests:?}");
    for secret in [ENV_SECRET, PLAN_SECRET] {
        assert!(
            !seen.contains(secret),
            "a secret reached the model: {secret}"
        );
    }
    assert_eq!(result_json(&requests, 3, "l3")["result"], NOTE_OK);
}
