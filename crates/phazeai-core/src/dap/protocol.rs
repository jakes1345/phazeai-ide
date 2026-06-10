//! DAP protocol message types (subset covering launch/attach + breakpoints + stepping).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Outgoing request from IDE to debug adapter.
#[derive(Debug, Clone, Serialize)]
pub struct DapRequest {
    pub seq: u64,
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
}

impl DapRequest {
    pub fn new(seq: u64, command: impl Into<String>, arguments: Option<Value>) -> Self {
        Self {
            seq,
            msg_type: "request",
            command: command.into(),
            arguments,
        }
    }
}

/// Incoming response from debug adapter.
#[derive(Debug, Clone, Deserialize)]
pub struct DapResponse {
    pub seq: u64,
    pub request_seq: u64,
    pub success: bool,
    pub command: String,
    pub message: Option<String>,
    pub body: Option<Value>,
}

/// Incoming event from debug adapter.
#[derive(Debug, Clone, Deserialize)]
pub struct DapEvent {
    pub seq: u64,
    pub event: String,
    pub body: Option<Value>,
}

/// A raw DAP message (could be response or event).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum DapMessage {
    #[serde(rename = "response")]
    Response(DapResponse),
    #[serde(rename = "event")]
    Event(DapEvent),
}

/// Breakpoint location sent to the adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceBreakpoint {
    pub line: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

/// A source reference for breakpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub name: Option<String>,
    pub path: Option<String>,
}

/// Capabilities the adapter declares in its initialize response.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Capabilities {
    pub supports_configuration_done_request: bool,
    pub supports_function_breakpoints: bool,
    pub supports_conditional_breakpoints: bool,
    pub supports_evaluate_for_hovers: bool,
    pub supports_set_variable: bool,
    pub supports_restart_request: bool,
}

/// A stack frame returned by stackTrace request.
#[derive(Debug, Clone, Deserialize)]
pub struct StackFrame {
    pub id: u64,
    pub name: String,
    pub source: Option<Source>,
    pub line: u64,
    pub column: u64,
}

/// A variable in the current scope.
#[derive(Debug, Clone, Deserialize)]
pub struct Variable {
    pub name: String,
    pub value: String,
    #[serde(rename = "type")]
    pub var_type: Option<String>,
    #[serde(rename = "variablesReference")]
    pub variables_reference: u64,
}

/// A scope (locals, globals, etc.).
#[derive(Debug, Clone, Deserialize)]
pub struct Scope {
    pub name: String,
    #[serde(rename = "variablesReference")]
    pub variables_reference: u64,
    pub expensive: bool,
}

/// Thread info.
#[derive(Debug, Clone, Deserialize)]
pub struct Thread {
    pub id: u64,
    pub name: String,
}

/// Known stop reasons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    Breakpoint,
    Step,
    Exception,
    Pause,
    Entry,
    Other(String),
}

impl StopReason {
    pub fn parse(s: &str) -> Self {
        match s {
            "breakpoint" => Self::Breakpoint,
            "step" => Self::Step,
            "exception" => Self::Exception,
            "pause" => Self::Pause,
            "entry" => Self::Entry,
            other => Self::Other(other.to_string()),
        }
    }
}

/// Events the UI cares about.
#[derive(Debug, Clone)]
pub enum DebugEvent {
    Initialized,
    Stopped {
        reason: StopReason,
        thread_id: u64,
    },
    Continued {
        thread_id: u64,
    },
    Exited {
        exit_code: i64,
    },
    Terminated,
    Output {
        category: String,
        output: String,
    },
    Breakpoint {
        reason: String,
        id: Option<u64>,
        verified: bool,
    },
}

/// Launch configuration.
#[derive(Debug, Clone, Serialize)]
pub struct LaunchConfig {
    pub program: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    #[serde(rename = "stopOnEntry")]
    pub stop_on_entry: bool,
}
