//! Canary plugin — exercises every hook in the `PhazePlugin` trait so the
//! end-to-end loader test can assert that a real `cdylib` built against the
//! stable ABI actually loads, activates, handles events, executes a command,
//! and deactivates cleanly.
//!
//! The plugin records every host interaction in a process-global `Mutex<Vec<_>>`
//! so integration tests can verify the exact sequence of calls. Tests run in
//! the host process, which means they can link `canary_trace()` directly and
//! read the recorded events without serialising them.

use phazeai_plugin_api::{declare_plugin, PhazePlugin, PluginCommand, PluginEvent, PluginHost};
use std::sync::Mutex;

/// One entry per host-visible action the plugin took.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanaryEvent {
    Activated,
    Deactivated,
    EventReceived(String),
    CommandExecuted { cmd: String, args: String },
}

static TRACE: Mutex<Vec<CanaryEvent>> = Mutex::new(Vec::new());

/// Drain the trace buffer. Called from integration tests.
#[no_mangle]
pub extern "C" fn canary_drain_trace_len() -> usize {
    TRACE.lock().map(|t| t.len()).unwrap_or(0)
}

fn record(ev: CanaryEvent) {
    if let Ok(mut t) = TRACE.lock() {
        t.push(ev);
    }
}

/// Plugin struct — stateless beyond the shared TRACE.
#[derive(Default)]
pub struct CanaryPlugin;

impl PhazePlugin for CanaryPlugin {
    fn name(&self) -> &str {
        "phazeai-plugin-canary"
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn description(&self) -> &str {
        "End-to-end canary for the PhazePlugin ABI."
    }

    fn on_activate(&mut self, host: &dyn PluginHost) {
        host.log(2, "canary: activate");
        host.show_message("canary active");
        record(CanaryEvent::Activated);
    }

    fn on_deactivate(&mut self) {
        record(CanaryEvent::Deactivated);
    }

    fn commands(&self) -> Vec<PluginCommand> {
        vec![PluginCommand {
            id: "canary.echo".to_string(),
            title: "Canary: Echo".to_string(),
            keybinding: None,
        }]
    }

    fn execute_command(&mut self, cmd: &str, args: &str) -> Result<String, String> {
        record(CanaryEvent::CommandExecuted {
            cmd: cmd.to_string(),
            args: args.to_string(),
        });
        match cmd {
            "canary.echo" => Ok(args.to_string()),
            other => Err(format!("unknown command: {other}")),
        }
    }

    fn on_event(&mut self, event: &PluginEvent) {
        let label = match event {
            PluginEvent::FileOpened { path } => format!("FileOpened:{path}"),
            PluginEvent::FileSaved { path } => format!("FileSaved:{path}"),
            PluginEvent::FileClosed { path } => format!("FileClosed:{path}"),
            PluginEvent::CursorMoved { line, col } => format!("CursorMoved:{line}:{col}"),
            PluginEvent::SelectionChanged { text } => format!("SelectionChanged:{text}"),
            PluginEvent::Custom { kind, data } => format!("Custom:{kind}:{data}"),
        };
        record(CanaryEvent::EventReceived(label));
    }
}

declare_plugin!(CanaryPlugin);
