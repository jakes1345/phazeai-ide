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
