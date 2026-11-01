//! Every child process the LSP, DAP and MCP clients start is reaped, on every
//! path that ends it.
//!
//! A process that has ended stays in the process table as a zombie, holding
//! its pid, until its parent collects its exit status with `wait`. Neither
//! `Child::kill` nor dropping a `Child` does that, so a server the IDE killed,
//! or one that crashed, used to stay a zombie until PhazeAI exited (one more
//! for every watchdog restart of a crashing server).
//!
//! These tests start real stand-in servers through the production clients:
//! `sleep`, or a few lines of `sh` that answer just enough of each protocol
//! (one LSP `shutdown`, one DAP `disconnect`, one MCP `initialize`) and then
//! `exec sleep`, so the recorded pid stays the server's. Each test ends the
//! server one way (dropping the client, `shutdown`, `disconnect`, or the
//! server exiting on its own while the client lives) and then asserts that the
//! process is gone from `/proc`, not left behind in state `Z`. Linux only:
//! they read `/proc`.

#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use phazeai_core::dap::DapClient;
use phazeai_core::mcp::{McpClient, McpServerConfig};
use phazeai_core::LspClient;
use tokio::sync::mpsc;

/// How long a killed or exited server may take to leave the process table.
const REAP_BOUND: Duration = Duration::from_secs(10);

/// Seconds a stand-in that exits on its own lives: long enough for the test
/// to see it running first.
const EXITS_AFTER: &str = "3";

/// Writes the shell's pid to the file named by `$1`, atomically, so a reader
/// never sees half of it. Every script below ends in `exec`, which keeps the
/// pid, so this is the server's pid for its whole life.
const RECORD_PID: &str = r#"printf '%s\n' "$$" > "$1.tmp" && mv "$1.tmp" "$1""#;

/// Fields of `/proc/<pid>/stat` after the command name, which may itself
/// hold spaces and parentheses: `[0]` is the state, `[19]` the start time.
fn stat_fields(pid: u32) -> Option<Vec<String>> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, rest) = stat.rsplit_once(')')?;
    Some(rest.split_whitespace().map(str::to_string).collect())
}

/// One process, told apart from a later one that is given the same pid by
/// its start time (pid_max can be as low as 32768).
struct Proc {
    pid: u32,
    start: Option<String>,
}

impl Proc {
    fn new(pid: u32) -> Self {
        let start = stat_fields(pid).and_then(|f| f.get(19).cloned());
        Self { pid, start }
    }

    /// Its state letter (`R`, `S`, `Z`, ...), or `None` once it is gone.
    fn state(&self) -> Option<char> {
        let start = self.start.as_ref()?;
        let fields = stat_fields(self.pid)?;
        if fields.get(19) != Some(start) {
            return None; // the pid belongs to a newer process now
        }
        fields.first()?.chars().next()
    }

    /// Fails unless the process is running: neither gone nor a zombie.
    fn assert_running(&self, what: &str) {
        let state = self.state();
        assert!(
            matches!(state, Some(s) if s != 'Z'),
            "{what} (pid {}) should be running before the test ends it; /proc says {state:?}",
            self.pid
        );
    }

    /// Waits up to [`REAP_BOUND`] for the process to leave the process table.
    /// One that ended but was never waited for stays there as a zombie until
    /// this test process exits, so an unreaped child fails here.
    fn assert_reaped(&self, what: &str) {
        let deadline = Instant::now() + REAP_BOUND;
        let mut state = self.state();
        while state.is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
            state = self.state();
        }
        assert!(
            state.is_none(),
            "{what} (pid {}) was never reaped: {}s later /proc still lists it, in state {state:?} \
             (Z = zombie: it ended, but nobody collected its exit status)",
            self.pid,
            REAP_BOUND.as_secs()
        );
    }
}

/// Polls `done` every 10ms for up to 10s; fails with `what` if it never holds.
fn wait_until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The pid a stand-in wrote with [`RECORD_PID`], once the file exists (it
/// appears whole, by a rename).
fn recorded_pid(pidfile: &Path) -> u32 {
    wait_until("the stand-in recorded its pid", || pidfile.exists());
    let text = std::fs::read_to_string(pidfile).expect("read the pid file");
    text.trim().parse().expect("the pid file holds a pid")
}

/// `sh -c` arguments for `script`, with `$1` = `pidfile` and `$2` = `arg`.
fn sh_args(script: &str, pidfile: &Path, arg: &str) -> Vec<String> {
    vec![
        "-c".to_string(),
        script.to_string(),
        "stand-in".to_string(),
        pidfile.to_str().expect("UTF-8 temp path").to_string(),
        arg.to_string(),
    ]
}

// ── LSP ─────────────────────────────────────────────────────────────────────

fn start_lsp(cmd: &str, args: &[&str]) -> LspClient {
    let (tx, _rx) = mpsc::unbounded_channel();
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    LspClient::start(cmd, &args, &std::env::temp_dir(), tx).expect("start the stand-in server")
}

#[test]
fn lsp_server_is_reaped_when_the_client_is_dropped() {
    let client = start_lsp("sleep", &["30"]);
    let server = Proc::new(client.child_pid().expect("pid of a server just started"));
    server.assert_running("the language server");

    drop(client);

    server.assert_reaped("the language server of a dropped LspClient");
}

#[test]
fn lsp_server_that_exits_on_its_own_is_reaped_while_the_client_lives() {
    // Stands in for a server that crashes: it exits while the client (and
    // whatever holds it, like the watchdog's map) is still alive.
    let client = start_lsp("sleep", &[EXITS_AFTER]);
    let server = Proc::new(client.child_pid().expect("pid of a server just started"));
    server.assert_running("the language server");

    wait_until("the client noticed its server exit", || !client.is_alive());

    server.assert_reaped("a language server that exited on its own");
    assert_eq!(
        client.child_pid(),
        None,
        "a reaped server's pid may belong to another process; child_pid must not report it"
    );
}

/// Answers the client's first request (id 1: the `shutdown` below) and then
/// ignores everything: the `exit` notification and SIGTERM alike.
const LSP_ANSWERS_SHUTDOWN_IGNORES_EXIT: &str = r#"trap '' TERM
IFS= read -r _header
body='{"jsonrpc":"2.0","id":1,"result":null}'
printf 'Content-Length: %d\r\n\r\n%s' "${#body}" "$body"
exec sleep 30"#;

#[tokio::test]
async fn lsp_shutdown_kills_and_reaps_a_server_that_ignores_exit() {
    let mut client = start_lsp("sh", &["-c", LSP_ANSWERS_SHUTDOWN_IGNORES_EXIT]);
    let server = Proc::new(client.child_pid().expect("pid of a server just started"));
    server.assert_running("the language server");

    tokio::time::timeout(Duration::from_secs(15), client.shutdown())
        .await
        .expect("shutdown must not hang on a server that ignores `exit`")
        .expect("shutdown");

    // The client is still alive: shutdown itself must have reaped the server.
    server.assert_reaped("a language server after LspClient::shutdown");
    assert_eq!(client.child_pid(), None, "no pid after shutdown");
    drop(client);
}

// ── DAP ─────────────────────────────────────────────────────────────────────

/// A stand-in adapter running `script` (after [`RECORD_PID`]), and its pid.
fn spawn_dap(script: &str, pidfile: &Path, arg: &str) -> (std::sync::Arc<DapClient>, Proc) {
    let args = sh_args(&format!("{RECORD_PID}\n{script}"), pidfile, arg);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    // The event receiver is dropped here: these adapters send no events.
    let (client, _events) = DapClient::spawn("sh", &args).expect("spawn the stand-in adapter");
    let adapter = Proc::new(recorded_pid(pidfile));
    adapter.assert_running("the debug adapter");
    (client, adapter)
}

#[test]
fn dap_adapter_is_reaped_when_the_client_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let (client, adapter) = spawn_dap(r#"exec sleep "$2""#, &dir.path().join("pid"), "30");

    drop(client);

    adapter.assert_reaped("the debug adapter of a dropped DapClient");
}

/// Answers the client's first request (seq 1: the `disconnect` below), then
/// ignores everything, SIGTERM included.
const DAP_ANSWERS_DISCONNECT: &str = r#"IFS= read -r _header
body='{"seq":1,"type":"response","request_seq":1,"success":true,"command":"disconnect"}'
printf 'Content-Length: %d\r\n\r\n%s' "${#body}" "$body"
trap '' TERM
exec sleep 30"#;

#[test]
fn dap_adapter_is_reaped_after_disconnect() {
    let dir = tempfile::tempdir().unwrap();
    let (client, adapter) = spawn_dap(DAP_ANSWERS_DISCONNECT, &dir.path().join("pid"), "");

    client.disconnect();

    // The client is still alive (the debug session holds it until it
    // returns): disconnect itself must have reaped the adapter.
    adapter.assert_reaped("a debug adapter after DapClient::disconnect");
    drop(client);
}

#[test]
fn dap_adapter_that_exits_on_its_own_is_reaped_while_the_client_lives() {
    let dir = tempfile::tempdir().unwrap();
    let (client, adapter) = spawn_dap(r#"exec sleep "$2""#, &dir.path().join("pid"), EXITS_AFTER);

    wait_until("the client noticed its adapter exit", || !client.is_alive());

    adapter.assert_reaped("a debug adapter that exited on its own");
    drop(client);
}

// ── MCP ─────────────────────────────────────────────────────────────────────

/// Answers `initialize` (id 1) declaring no tools, resources or prompts, so
/// `McpClient::connect` succeeds; then sleeps `$2` seconds (or until killed).
const MCP_ANSWERS_INITIALIZE: &str = r#"IFS= read -r _request
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"stand-in","version":"0"}}}'
exec sleep "$2""#;

/// A connected stand-in MCP server that sleeps `sleep_secs`, and its pid.
fn connect_mcp(pidfile: &Path, sleep_secs: &str) -> (McpClient, Proc) {
    let config = McpServerConfig {
        name: "stand-in".to_string(),
        command: "sh".to_string(),
        args: sh_args(
            &format!("{RECORD_PID}\n{MCP_ANSWERS_INITIALIZE}"),
            pidfile,
            sleep_secs,
        ),
        env: HashMap::new(),
    };
    let client = McpClient::connect(&config).expect("connect to the stand-in server");
    let server = Proc::new(recorded_pid(pidfile));
    server.assert_running("the MCP server");
    (client, server)
}

#[test]
fn mcp_server_is_reaped_when_the_client_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let (client, server) = connect_mcp(&dir.path().join("pid"), "30");

    drop(client);

    server.assert_reaped("the MCP server of a dropped McpClient");
}

#[test]
fn mcp_server_that_exits_on_its_own_is_reaped_while_the_client_lives() {
    let dir = tempfile::tempdir().unwrap();
    let (client, server) = connect_mcp(&dir.path().join("pid"), EXITS_AFTER);

    wait_until("the client noticed its server exit", || !client.is_alive());

    server.assert_reaped("an MCP server that exited on its own");
    drop(client);
}
