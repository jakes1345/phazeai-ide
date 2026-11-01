//! Background task that owns the [`Agent`] and runs prompts.
//!
//! The UI talks to it through [`WorkerCommand`]s and receives [`UiEvent`]s.
//! Interruption is immediate: the running agent future is dropped as soon as
//! the cancel flag is raised, without waiting the up to 0.1 s the core takes
//! to notice the flag itself (see `repair_after_interrupt` for what a run
//! dropped mid-round leaves behind).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use phazeai_core::tools::{ToolApprovalManager, ToolPermission};
use phazeai_core::{
    Agent, AgentEvent, CodeSearch, ConversationHistory, LlmClient, SystemPromptBuilder,
};
use tokio::sync::{mpsc, oneshot};

use crate::tool_view::{approval_preview, unanswered_tool_calls, ApprovalPolicy, PreviewLine};

pub enum WorkerCommand {
    /// Run the agent on a prompt.
    Run(String),
    /// Forget the conversation context.
    Clear,
    /// Replace the LLM client (provider / model / key change) and the
    /// context budget that goes with the new model; the system prompt is
    /// resized to that budget too.
    SetLlm {
        llm: Box<dyn LlmClient>,
        context_budget: usize,
    },
    /// Replace the context with restored (role, content) pairs.
    Restore(Vec<(String, String)>),
}

/// A tool call waiting for the user's decision.
pub struct ApprovalRequest {
    pub name: String,
    pub params: serde_json::Value,
    pub preview: Vec<PreviewLine>,
    pub risk: ToolPermission,
    pub respond: oneshot::Sender<bool>,
}

pub enum UiEvent {
    Agent(AgentEvent),
    Approval(ApprovalRequest),
    /// Estimated tokens currently in the context window.
    Context(usize),
    /// The current run ended (completed, failed or interrupted).
    RunFinished {
        cancelled: bool,
    },
}

pub struct WorkerConfig {
    pub llm: Option<Box<dyn LlmClient>>,
    /// What the system prompt is built from; each agent gets it sized to
    /// `context_budget` (see [`system_prompt`]).
    pub prompt: SystemPromptBuilder,
    pub restore: Vec<(String, String)>,
    pub approval_manager: Arc<Mutex<ToolApprovalManager>>,
    pub policy: Arc<Mutex<ApprovalPolicy>>,
    pub cancel: Arc<AtomicBool>,
    /// Conversation tokens the agent may send before compacting, for the
    /// current model.
    pub context_budget: usize,
    /// Shared by every agent the worker builds; `None` when search is off.
    pub code_search: Option<CodeSearch>,
}

fn build_approval_fn(
    manager: Arc<Mutex<ToolApprovalManager>>,
    policy: Arc<Mutex<ApprovalPolicy>>,
    approval_tx: mpsc::UnboundedSender<ApprovalRequest>,
) -> phazeai_core::ApprovalFn {
    Box::new(move |name, params| {
        let manager = manager.clone();
        let policy = policy.clone();
        let approval_tx = approval_tx.clone();
        Box::pin(async move {
            let (needs, risk) = {
                let m = manager.lock().unwrap_or_else(|e| e.into_inner());
                (
                    m.needs_approval(&name, &params),
                    m.classify_tool(&name, &params),
                )
            };
            let allowed_by_rule = policy
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .allows(&name, &params);
            if !needs || allowed_by_rule {
                return true;
            }

            let preview = approval_preview(&name, &params).await;
            let (tx, rx) = oneshot::channel();
            let request = ApprovalRequest {
                name: name.clone(),
                params: params.clone(),
                preview,
                risk,
                respond: tx,
            };
            if approval_tx.send(request).is_err() {
                return false;
            }
            // A dropped sender (run interrupted) counts as a denial.
            let approved = rx.await.unwrap_or(false);
            if approved {
                manager
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .record_approval(&name, &params);
            }
            approved
        })
    })
}

/// The system prompt for the current model: instructions take at most their
/// share of its context budget.
fn system_prompt(cfg: &WorkerConfig) -> String {
    cfg.prompt
        .clone()
        .with_context_budget(cfg.context_budget)
        .build()
}

fn build_agent(
    llm: Box<dyn LlmClient>,
    cfg: &WorkerConfig,
    conversation: Arc<tokio::sync::Mutex<ConversationHistory>>,
    approval_tx: &mpsc::UnboundedSender<ApprovalRequest>,
) -> Agent {
    let approval = build_approval_fn(
        cfg.approval_manager.clone(),
        cfg.policy.clone(),
        approval_tx.clone(),
    );
    let mut agent = Agent::new(llm)
        .with_system_prompt(system_prompt(cfg))
        .with_shared_conversation(conversation)
        .with_context_budget(cfg.context_budget)
        .with_approval(approval)
        .with_cancel_token(cfg.cancel.clone());

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mcp_configs = phazeai_core::mcp::McpManager::load_config(&cwd);
    if !mcp_configs.is_empty() {
        let mut mcp = phazeai_core::mcp::McpManager::new();
        mcp.connect_all(&mcp_configs);
        agent.register_mcp_tools(Arc::new(Mutex::new(mcp)));
    }

    if let Some(search) = &cfg.code_search {
        super::register_code_search(&mut agent, search);
    }
    agent
}

/// Answer any tool calls left dangling by an interrupted run so the next
/// request is accepted by the provider.
async fn repair_after_interrupt(conversation: &tokio::sync::Mutex<ConversationHistory>) {
    let mut conv = conversation.lock().await;
    let orphans = unanswered_tool_calls(&conv.get_conversation_messages());
    for id in orphans {
        conv.add_tool_result(id, "Interrupted by the user before this tool ran.");
    }
}

pub fn spawn(
    mut cfg: WorkerConfig,
    mut rx: mpsc::UnboundedReceiver<WorkerCommand>,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Approval requests go through the worker (not straight to the UI) so
        // they are always delivered after the agent's ToolApprovalRequest
        // event for the same call — see the biased select below.
        let (approval_tx, mut approval_rx) = mpsc::unbounded_channel::<ApprovalRequest>();
        let conversation = Arc::new(tokio::sync::Mutex::new(ConversationHistory::new()));
        {
            let mut conv = conversation.lock().await;
            for (role, content) in std::mem::take(&mut cfg.restore) {
                match role.as_str() {
                    "user" => conv.add_user_message(content),
                    "assistant" => conv.add_assistant_message(content),
                    _ => {}
                }
            }
        }

        let mut agent = cfg
            .llm
            .take()
            .map(|llm| build_agent(llm, &cfg, conversation.clone(), &approval_tx));

        while let Some(cmd) = rx.recv().await {
            match cmd {
                WorkerCommand::Clear => conversation.lock().await.clear(),
                WorkerCommand::Restore(msgs) => {
                    let mut conv = conversation.lock().await;
                    conv.clear();
                    for (role, content) in msgs {
                        match role.as_str() {
                            "user" => conv.add_user_message(content),
                            "assistant" => conv.add_assistant_message(content),
                            _ => {}
                        }
                    }
                }
                WorkerCommand::SetLlm {
                    llm,
                    context_budget,
                } => {
                    // The new model's budget sizes the conversation and the
                    // system prompt alike.
                    cfg.context_budget = context_budget;
                    match agent.as_mut() {
                        Some(a) => {
                            a.swap_llm(llm);
                            a.set_context_budget(context_budget);
                            a.set_system_prompt(system_prompt(&cfg));
                        }
                        None => {
                            agent = Some(build_agent(llm, &cfg, conversation.clone(), &approval_tx))
                        }
                    }
                }
                WorkerCommand::Run(prompt) => {
                    cfg.cancel.store(false, Ordering::SeqCst);
                    let Some(agent) = agent.as_ref() else {
                        let _ = ui_tx.send(UiEvent::Agent(AgentEvent::Error(
                            "No LLM provider is configured.".into(),
                        )));
                        let _ = ui_tx.send(UiEvent::RunFinished { cancelled: false });
                        continue;
                    };

                    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<AgentEvent>();
                    let mut run = Box::pin(agent.run_with_events(prompt, ev_tx));
                    let mut tick = tokio::time::interval(Duration::from_millis(50));
                    loop {
                        tokio::select! {
                            biased;
                            Some(ev) = ev_rx.recv() => {
                                let _ = ui_tx.send(UiEvent::Agent(ev));
                            }
                            Some(req) = approval_rx.recv() => {
                                let _ = ui_tx.send(UiEvent::Approval(req));
                            }
                            _ = &mut run => break,
                            _ = tick.tick() => {
                                if cfg.cancel.load(Ordering::SeqCst) {
                                    break;
                                }
                            }
                        }
                    }
                    // Drop the (possibly unfinished) run before draining so
                    // it releases its borrow of the conversation. This also
                    // ends any tool call still in flight: a `bash` child is
                    // killed together with its process group when its future
                    // goes (see `BashTool`), so an interrupted `cargo build`
                    // does not keep running behind the user's back.
                    drop(run);
                    // Requests from a dropped run can no longer be answered.
                    while approval_rx.try_recv().is_ok() {}
                    while let Ok(ev) = ev_rx.try_recv() {
                        let _ = ui_tx.send(UiEvent::Agent(ev));
                    }
                    let cancelled = cfg.cancel.load(Ordering::SeqCst);
                    if cancelled {
                        repair_after_interrupt(&conversation).await;
                    }
                    let tokens = conversation.lock().await.estimate_tokens();
                    let _ = ui_tx.send(UiEvent::Context(tokens));
                    let _ = ui_tx.send(UiEvent::RunFinished { cancelled });
                }
            }
        }
    })
}
