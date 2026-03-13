use super::{Extension, IdeContext};
use deno_core::{op2, Extension as DenoExtension, JsRuntime, OpState, RuntimeOptions};
use serde_json::Value;
use std::sync::mpsc;
use std::thread;
use tokio::sync::oneshot;

// Deno operations must be synchronous or use specific async setup.
// We pass messages to Deno via Ops.
#[op2(fast)]
fn op_vscode_show_message(state: &mut OpState, #[string] msg: String) {
    if let Some(ctx) = state.try_borrow::<IdeContext>() {
        ctx.delegate.show_message(&msg);
    } else {
        tracing::info!("[JS] vscode.window.showInformationMessage: {}", msg);
    }
}

#[op2]
#[string]
fn op_vscode_get_active_text(state: &mut OpState) -> String {
    if let Some(ctx) = state.try_borrow::<IdeContext>() {
        ctx.delegate.get_active_text()
    } else {
        String::new()
    }
}

#[allow(dead_code)]
enum JsCommand {
    InitContext {
        context: IdeContext,
        reply: oneshot::Sender<Result<(), String>>,
    },
    ExecuteScript {
        name: String,
        code: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    ExecuteExtensionCommand {
        command: String,
        args: Value,
        reply: oneshot::Sender<Result<Value, String>>,
    },
    RegisterCommand {
        command: String,
    }
}

pub struct JsExtension {
    id: String,
    commands: Vec<String>,
    tx: mpsc::Sender<JsCommand>,
    script_content: String,
}

impl JsExtension {
    pub fn new(id: impl Into<String>, commands: Vec<String>, js_code: impl Into<String>) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel::<JsCommand>();
        
        thread::spawn(move || {
            let decls = vec![
                op_vscode_show_message(),
                op_vscode_get_active_text(),
            ];
            let ext = DenoExtension {
                name: "vscode_shim",
                ops: std::borrow::Cow::Owned(decls),
                ..Default::default()
            };

            let mut runtime = JsRuntime::new(RuntimeOptions {
                extensions: vec![ext],
                ..Default::default()
            });

            // Inject the vscode shim globally
            let shim = r#"
                globalThis.vscode = {
                    window: {
                        showInformationMessage: (msg) => {
                            Deno.core.ops.op_vscode_show_message(msg);
                        },
                        showErrorMessage: (msg) => {
                            Deno.core.ops.op_vscode_show_message("[ERROR] " + msg);
                        },
                        get activeTextEditor() {
                            return {
                                document: {
                                    getText: () => Deno.core.ops.op_vscode_get_active_text()
                                }
                            };
                        }
                    },
                    commands: {
                        registerCommand: (id, callback) => {
                            // Register internally in JS
                            if (!globalThis._vscode_commands) {
                                globalThis._vscode_commands = {};
                            }
                            globalThis._vscode_commands[id] = callback;
                        },
                        executeCommand: (id, ...args) => {
                            if (globalThis._vscode_commands && globalThis._vscode_commands[id]) {
                                return globalThis._vscode_commands[id](...args);
                            }
                            return null;
                        }
                    }
                };
            "#;
            
            if let Err(e) = runtime.execute_script("vscode_shim", shim) {
                tracing::error!("Failed to inject VSCode shim: {}", e);
                return;
            }

            // Message loop
            while let Ok(cmd) = rx.recv() {
                match cmd {
                    JsCommand::InitContext { context, reply } => {
                        runtime.op_state().borrow_mut().put(context);
                        let _ = reply.send(Ok(()));
                    }
                    JsCommand::ExecuteScript { name: _, code, reply } => {
                        let res = runtime.execute_script("<extension>", code)
                            .map(|_| ())
                            .map_err(|e| e.to_string());
                        let _ = reply.send(res);
                    }
                    JsCommand::ExecuteExtensionCommand { command, args, reply } => {
                        let args_json = serde_json::to_string(&args).unwrap_or_else(|_| "null".to_string());
                        // JSON-encode command to prevent injection (adds quotes + escapes internals)
                        let command_json = serde_json::to_string(&command).unwrap_or_else(|_| "\"\"".to_string());
                        let invoke_code = format!(
                            "if (globalThis.vscode && globalThis.vscode.commands && globalThis.vscode.commands.executeCommand) {{ globalThis.vscode.commands.executeCommand({}, {}); }} else {{ null }}",
                            command_json, args_json
                        );
                        let res = runtime.execute_script("<command>", invoke_code)
                            .map(|_| Value::Null)
                            .map_err(|e| e.to_string());
                        let _ = reply.send(res);
                    }
                    JsCommand::RegisterCommand { command: _ } => {}
                }
            }
        });

        Ok(Self {
            id: id.into(),
            commands,
            tx,
            script_content: js_code.into(),
        })
    }
}

#[async_trait::async_trait]
impl Extension for JsExtension {
    fn id(&self) -> &str {
        &self.id
    }

    fn commands(&self) -> Vec<String> {
        self.commands.clone()
    }

    async fn activate(&mut self, context: IdeContext) -> Result<(), String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx.send(JsCommand::InitContext {
            context,
            reply: reply_tx,
        }).map_err(|_| "JS thread died".to_string())?;
        reply_rx.await.map_err(|_| "JS thread died before replying".to_string())??;

        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx.send(JsCommand::ExecuteScript {
            name: self.id.clone(),
            code: self.script_content.clone(),
            reply: reply_tx,
        }).map_err(|_| "JS thread died".to_string())?;
        
        reply_rx.await.map_err(|_| "JS thread died before replying".to_string())??;
        Ok(())
    }

    async fn execute_command(&mut self, command: &str, args: Value) -> Result<Value, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx.send(JsCommand::ExecuteExtensionCommand {
            command: command.to_string(),
            args,
            reply: reply_tx,
        }).map_err(|_| "JS thread died".to_string())?;
        
        reply_rx.await.map_err(|_| "JS thread died before replying".to_string())?
    }

    async fn deactivate(&mut self) -> Result<(), String> {
        Ok(())
    }
}
