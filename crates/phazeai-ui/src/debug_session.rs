//! DAP debug session controller.
//!
//! Bridges the blocking `phazeai_core::dap::DapClient` to the reactive UI:
//! the session runs on its own thread, receives `DebugCmd`s from the UI and
//! streams `SessionUpdate`s back over a sync_channel that the Run & Debug
//! panel turns into signal writes via `create_signal_from_channel`.

use phazeai_core::dap::{DapClient, DebugEvent, LaunchConfig, SourceBreakpoint};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;

use crate::domain_state::project::DebugStatus;

/// Commands the UI sends to a live debug session.
#[derive(Debug, Clone)]
pub enum DebugCmd {
    Continue,
    StepOver,
    StepIn,
    StepOut,
    Stop,
    /// Replace all breakpoints for one file (1-based lines).
    SetBreakpoints(PathBuf, Vec<u64>),
}

/// A stack frame row for the UI: (frame id, function name, file path, 1-based line).
pub type FrameRow = (u64, String, String, u64);
/// A variable row for the UI: (name, value, type).
pub type VarRow = (String, String, String);

/// Updates the session thread streams back to the UI.
#[derive(Debug, Clone)]
pub enum SessionUpdate {
    Status(DebugStatus),
    /// Where execution is stopped: (file, 1-based line). None = running/done.
    StoppedAt(Option<(PathBuf, u64)>),
    Frames(Vec<FrameRow>),
    Vars(Vec<VarRow>),
    Output(String),
}

/// Find an installed DAP adapter for native code, in preference order.
/// Returns (command, args). codelldb is skipped: it speaks TCP, not stdio.
pub fn detect_adapter() -> Option<(String, Vec<String>)> {
    fn in_path(bin: &str) -> bool {
        std::process::Command::new("which")
            .arg(bin)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    if in_path("lldb-dap") {
        return Some(("lldb-dap".into(), vec![]));
    }
    if in_path("lldb-vscode") {
        return Some(("lldb-vscode".into(), vec![]));
    }
    if in_path("gdb") {
        // GDB ≥ 14 ships a built-in DAP interpreter.
        let ok = std::process::Command::new("gdb")
            .args(["--version"])
            .output()
            .ok()
            .and_then(|o| {
                let v = String::from_utf8_lossy(&o.stdout);
                v.split_whitespace()
                    .find_map(|t| t.split('.').next()?.parse::<u32>().ok())
            })
            .map(|major| major >= 14)
            .unwrap_or(false);
        if ok {
            return Some(("gdb".into(), vec!["-i".into(), "dap".into()]));
        }
    }
    None
}

/// Internal unified message for the session loop.
enum Msg {
    Dap(DebugEvent),
    Cmd(DebugCmd),
}

/// Run a debug session to completion. Call from a dedicated thread.
#[allow(clippy::too_many_arguments)]
pub fn run_session(
    adapter_cmd: String,
    adapter_args: Vec<String>,
    program: String,
    cwd: PathBuf,
    initial_breakpoints: Vec<(PathBuf, u64)>,
    cmd_rx: mpsc::Receiver<DebugCmd>,
    update_tx: mpsc::SyncSender<SessionUpdate>,
) {
    let send = |u: SessionUpdate| {
        let _ = update_tx.send(u);
    };

    let args_ref: Vec<&str> = adapter_args.iter().map(|s| s.as_str()).collect();
    let (client, event_rx) = match DapClient::spawn(&adapter_cmd, &args_ref) {
        Ok(pair) => pair,
        Err(e) => {
            send(SessionUpdate::Output(format!(
                "Failed to start adapter '{adapter_cmd}': {e}\n"
            )));
            send(SessionUpdate::Status(DebugStatus::Idle));
            return;
        }
    };

    if let Err(e) = client.initialize() {
        send(SessionUpdate::Output(format!(
            "DAP initialize failed: {e}\n"
        )));
        send(SessionUpdate::Status(DebugStatus::Idle));
        client.disconnect();
        return;
    }

    // Group breakpoints by file (DAP setBreakpoints replaces per-source).
    let mut bps_by_file: HashMap<PathBuf, Vec<u64>> = HashMap::new();
    for (path, line) in initial_breakpoints {
        bps_by_file.entry(path).or_default().push(line);
    }

    // The launch request usually doesn't get a response until after
    // configurationDone, which we can only send once the 'initialized' event
    // arrives — so launch must not block this loop.
    {
        let client = client.clone();
        let config = LaunchConfig {
            program: program.clone(),
            args: vec![],
            cwd: Some(cwd.to_string_lossy().to_string()),
            env: HashMap::new(),
            stop_on_entry: false,
        };
        std::thread::spawn(move || {
            let _ = client.launch(&config);
        });
    }

    send(SessionUpdate::Status(DebugStatus::Running));
    send(SessionUpdate::Output(format!(
        "Debugging {program} ({adapter_cmd})\n"
    )));

    // Unify DAP events and UI commands into one receiver.
    let (msg_tx, msg_rx) = mpsc::channel::<Msg>();
    {
        let tx = msg_tx.clone();
        std::thread::spawn(move || {
            while let Ok(evt) = event_rx.recv() {
                if tx.send(Msg::Dap(evt)).is_err() {
                    break;
                }
            }
        });
    }
    std::thread::spawn(move || {
        while let Ok(cmd) = cmd_rx.recv() {
            if msg_tx.send(Msg::Cmd(cmd)).is_err() {
                break;
            }
        }
    });

    let mut thread_id: u64 = 1;

    while let Ok(msg) = msg_rx.recv() {
        match msg {
            Msg::Dap(DebugEvent::Initialized) => {
                for (path, lines) in &bps_by_file {
                    let bps: Vec<SourceBreakpoint> = lines
                        .iter()
                        .map(|&line| SourceBreakpoint {
                            line,
                            column: None,
                            condition: None,
                        })
                        .collect();
                    let _ = client.set_breakpoints(path, &bps);
                }
                let _ = client.configuration_done();
            }
            Msg::Dap(DebugEvent::Stopped {
                reason: _,
                thread_id: tid,
            }) => {
                thread_id = tid;
                send(SessionUpdate::Status(DebugStatus::Stopped));

                let frames = client.stack_trace(tid).unwrap_or_default();
                let rows: Vec<FrameRow> = frames
                    .iter()
                    .map(|f| {
                        let file = f
                            .source
                            .as_ref()
                            .and_then(|s| s.path.clone())
                            .unwrap_or_default();
                        (f.id, f.name.clone(), file, f.line)
                    })
                    .collect();

                // Highlight the topmost frame that has a source file.
                let stopped_at = rows
                    .iter()
                    .find(|(_, _, file, _)| !file.is_empty())
                    .map(|(_, _, file, line)| (PathBuf::from(file), *line));
                send(SessionUpdate::StoppedAt(stopped_at));

                // Variables from the top frame's first non-expensive scope(s).
                let mut vars: Vec<VarRow> = Vec::new();
                if let Some(frame) = frames.first() {
                    let scopes = client.scopes(frame.id).unwrap_or_default();
                    for scope in scopes.iter().filter(|s| !s.expensive).take(2) {
                        for v in client
                            .variables(scope.variables_reference)
                            .unwrap_or_default()
                            .into_iter()
                            .take(100)
                        {
                            vars.push((v.name, v.value, v.var_type.unwrap_or_default()));
                        }
                    }
                }
                send(SessionUpdate::Frames(rows));
                send(SessionUpdate::Vars(vars));
            }
            Msg::Dap(DebugEvent::Continued { .. }) => {
                send(SessionUpdate::Status(DebugStatus::Running));
                send(SessionUpdate::StoppedAt(None));
                send(SessionUpdate::Frames(Vec::new()));
                send(SessionUpdate::Vars(Vec::new()));
            }
            Msg::Dap(DebugEvent::Output { output, .. }) => {
                send(SessionUpdate::Output(output));
            }
            Msg::Dap(DebugEvent::Exited { exit_code }) => {
                send(SessionUpdate::Output(format!(
                    "Process exited with code {exit_code}\n"
                )));
            }
            Msg::Dap(DebugEvent::Terminated) => {
                send(SessionUpdate::Output("Debug session ended.\n".into()));
                break;
            }
            Msg::Dap(DebugEvent::Breakpoint { .. }) => {}
            Msg::Cmd(cmd) => {
                let stepped = matches!(
                    cmd,
                    DebugCmd::Continue | DebugCmd::StepOver | DebugCmd::StepIn | DebugCmd::StepOut
                );
                match cmd {
                    DebugCmd::Continue => {
                        let _ = client.continue_execution(thread_id);
                    }
                    DebugCmd::StepOver => {
                        let _ = client.next(thread_id);
                    }
                    DebugCmd::StepIn => {
                        let _ = client.step_in(thread_id);
                    }
                    DebugCmd::StepOut => {
                        let _ = client.step_out(thread_id);
                    }
                    DebugCmd::Stop => {
                        client.disconnect();
                        break;
                    }
                    DebugCmd::SetBreakpoints(path, lines) => {
                        bps_by_file.insert(path.clone(), lines.clone());
                        let bps: Vec<SourceBreakpoint> = lines
                            .iter()
                            .map(|&line| SourceBreakpoint {
                                line,
                                column: None,
                                condition: None,
                            })
                            .collect();
                        let _ = client.set_breakpoints(&path, &bps);
                    }
                }
                // Optimistically mark running; the adapter's stopped/continued
                // events will correct this if the request failed.
                if stepped {
                    send(SessionUpdate::Status(DebugStatus::Running));
                    send(SessionUpdate::StoppedAt(None));
                }
            }
        }
        if !client.is_alive() {
            break;
        }
    }

    client.disconnect();
    send(SessionUpdate::Status(DebugStatus::Idle));
    send(SessionUpdate::StoppedAt(None));
    send(SessionUpdate::Frames(Vec::new()));
    send(SessionUpdate::Vars(Vec::new()));
}
