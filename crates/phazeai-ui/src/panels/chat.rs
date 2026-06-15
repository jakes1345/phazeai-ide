use std::rc::Rc;
use std::sync::Arc;

use floem::{
    event::{Event, EventListener},
    ext_event::create_signal_from_channel,
    keyboard::{Key, Modifiers},
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, label, scroll, stack, text_input, Decorators},
    IntoView,
};
use phazeai_core::{
    Agent, AgentEvent, ConversationHistory, ConversationMetadata, ConversationStore,
    DiffHookFn, SavedConversation, SavedMessage, Settings,
};
use phazeai_sidecar::SidecarClient;

use crate::{
    components::icon::{icons, phaze_icon},
    theme::PhazeTheme,
    util::safe_get,
};

// ── AI Mode ───────────────────────────────────────────────────────────────────

/// Selects the conversational role / system-prompt variant used when sending
/// a message to the AI. Previously this lived in the now-removed `ai_panel`;
/// it has been merged here so there is a single AI surface in the IDE.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiMode {
    Chat,
    Ask,
    Debug,
    Plan,
    Edit,
}

impl AiMode {
    pub fn label(self) -> &'static str {
        match self {
            AiMode::Chat => "Chat",
            AiMode::Ask => "Ask",
            AiMode::Debug => "Debug",
            AiMode::Plan => "Plan",
            AiMode::Edit => "Edit",
        }
    }

    /// Returns a brief system-prompt prefix injected before the user message.
    /// Empty string for the default Chat mode so no prefix is added.
    pub fn system_hint(self) -> &'static str {
        match self {
            AiMode::Chat => "",
            AiMode::Ask => "Answer concisely and precisely. No extra prose.\n\n",
            AiMode::Debug => "You are a debugging expert. Focus on root causes and fixes.\n\n",
            AiMode::Plan => "You are a software architect. Produce clear step-by-step plans.\n\n",
            AiMode::Edit => "You are a code editor. Produce only code changes, no commentary.\n\n",
        }
    }
}

// ── Chat Types ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum ChatRole {
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: ChatRole,
    /// Display content. For the streaming assistant message this is updated live.
    pub content: String,
    /// True while AI is still generating this message.
    pub loading: bool,
    pub is_error: bool,
}

/// Shared slot for diff approval: carries the oneshot sender for Approve/Reject.
type DiffApproveSlot =
    Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<bool>>>>;

/// What the background AI thread sends to the Floem UI thread.
#[derive(Clone, Debug)]
enum ChatUpdate {
    /// Partial streamed text — contains the FULL accumulated text so far.
    Partial(String),
    /// Generation complete — final text.
    Done(String),
    /// Tool execution started.
    ToolStart { name: String },
    /// Tool execution finished.
    ToolResult { name: String, summary: String },
    /// An error occurred.
    Err(String),
    /// The user cancelled generation via the Stop button.
    Cancelled(String),
    /// MCP server process(es) were restarted (stdio recovery).
    McpStatus(String),
    /// A file-write tool wants user approval. Contains before/after content
    /// and a slot to resolve the blocking diff hook in the agent thread.
    DiffApprovalNeeded {
        path: String,
        before: String,
        after: String,
        slot: DiffApproveSlot,
    },
}

fn format_chat_error(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("ollama") && lower.contains("not found") && lower.contains("model") {
        return format!(
            "Error: {raw}\n\nHint: selected Ollama model is missing.\nTry:\n- ollama pull llama3.2:3b\n- open Settings and switch to an installed model"
        );
    }
    if lower.contains("connection refused") && lower.contains("11434") {
        return format!(
            "Error: {raw}\n\nHint: Ollama may not be running.\nStart it with `ollama serve`."
        );
    }
    format!("Error: {raw}")
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn now_str() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // RFC3339-ish: YYYY-MM-DDTHH:MM:SSZ
    let secs = now;
    let days = secs / 86400;
    let rem = secs % 86400;
    let hours = rem / 3600;
    let mins = (rem % 3600) / 60;
    let s = rem % 60;
    // Convert Unix days-since-epoch to civil date using Howard Hinnant's algorithm.
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = mp + if mp < 10 { 3 } else { -9 }; // [1, 12]
    let year = y + if month <= 2 { 1 } else { 0 };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, mins, s
    )
}

fn save_conversation(
    messages: &[ChatMessage],
    conversation_id: &str,
    model_name: &str,
    workspace_root: &std::path::Path,
) {
    let store = ConversationStore::new().unwrap_or_else(|_| ConversationStore::default());

    let saved_messages: Vec<SavedMessage> = messages
        .iter()
        .map(|m| SavedMessage {
            role: match m.role {
                ChatRole::User => "user".into(),
                ChatRole::Assistant => "assistant".into(),
                ChatRole::Tool => "tool".into(),
            },
            content: m.content.clone(),
            timestamp: now_str(),
            tool_name: None,
        })
        .collect();

    let title = messages
        .iter()
        .find_map(|m| {
            if m.role == ChatRole::User {
                let t = m.content.chars().take(80).collect::<String>();
                Some(if m.content.len() > 80 {
                    format!("{}...", t)
                } else {
                    t
                })
            } else {
                None
            }
        })
        .unwrap_or_else(|| "Untitled".into());

    let cwd = Some(workspace_root.display().to_string());

    let metadata = ConversationMetadata {
        id: conversation_id.to_string(),
        title,
        created_at: now_str(),
        updated_at: now_str(),
        message_count: saved_messages.len(),
        model: model_name.to_string(),
        project_dir: cwd,
    };

    let conversation = SavedConversation {
        metadata,
        messages: saved_messages,
        system_prompt: None,
    };

    let _ = store.save(&conversation);
}

fn shape_retry_prior_messages(
    messages: &[ChatMessage],
    retry_user_message: &str,
) -> Vec<ChatMessage> {
    let mut prior_messages = messages.to_vec();
    while let Some(last) = prior_messages.last() {
        if last.role == ChatRole::User {
            break;
        }
        prior_messages.pop();
    }
    if prior_messages
        .last()
        .map(|m| m.role == ChatRole::User && m.content == retry_user_message)
        .unwrap_or(false)
    {
        prior_messages.pop();
    }
    prior_messages
}

struct SendToAiJob {
    user_message: String,
    prior_messages: Vec<ChatMessage>,
    settings: Settings,
    workspace_root: std::path::PathBuf,
    mode_hint: &'static str,
    update_tx: std::sync::mpsc::SyncSender<ChatUpdate>,
    cancel_token: Arc<std::sync::atomic::AtomicBool>,
    sidecar_client: Option<Arc<SidecarClient>>,
}

fn send_to_ai(job: SendToAiJob) {
    let SendToAiJob {
        user_message,
        prior_messages,
        settings,
        workspace_root,
        mode_hint,
        update_tx,
        cancel_token,
        sidecar_client,
    } = job;

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = update_tx.send(ChatUpdate::Err(format!("Runtime error: {e}")));
                return;
            }
        };

        rt.block_on(async move {
            // #region agent log
            phazeai_core::debug_ndjson::log(
                "0179af",
                "full-ide-sweep",
                "H13",
                "chat.rs:send_to_ai",
                "send_to_ai started",
                serde_json::json!({
                    "userMessageLen": user_message.len(),
                    "priorMessages": prior_messages.len(),
                    "modeHintEmpty": mode_hint.is_empty(),
                }),
            );
            // #endregion
            let client = match settings.build_llm_client() {
                Ok(c) => c,
                Err(e) => {
                    let _ = update_tx.send(ChatUpdate::Err(format!("LLM init error: {e}")));
                    return;
                }
            };
            let base_conversation = if mode_hint.is_empty() {
                ConversationHistory::new()
            } else {
                ConversationHistory::new().with_system_prompt(mode_hint)
            };
            let shared_conversation =
                std::sync::Arc::new(tokio::sync::Mutex::new(base_conversation));
            {
                let mut conv = shared_conversation.lock().await;
                let prior_len = prior_messages.len();
                let mut restored_users = 0usize;
                let mut restored_assistants = 0usize;
                let mut skipped_loading = 0usize;
                let mut skipped_duplicate_retry = 0usize;
                for (idx, msg) in prior_messages.into_iter().enumerate() {
                    if msg.loading {
                        skipped_loading += 1;
                        continue;
                    }
                    let is_duplicated_retry_user = idx + 1 == prior_len
                        && msg.role == ChatRole::User
                        && msg.content == user_message;
                    if is_duplicated_retry_user {
                        skipped_duplicate_retry += 1;
                        continue;
                    }
                    match msg.role {
                        ChatRole::User => {
                            restored_users += 1;
                            conv.add_user_message(msg.content);
                        }
                        ChatRole::Assistant => {
                            if msg.is_error {
                                continue;
                            }
                            restored_assistants += 1;
                            conv.add_assistant_message(msg.content);
                        }
                        // Tool bubbles are UI-level status cards and are not part
                        // of the strict role schema expected by providers.
                        ChatRole::Tool => {}
                    }
                }
                // #region agent log
                phazeai_core::debug_ndjson::log(
                    "0179af",
                    "full-ide-sweep",
                    "H14",
                    "chat.rs:send_to_ai",
                    "conversation restored before run",
                    serde_json::json!({
                        "restoredUsers": restored_users,
                        "restoredAssistants": restored_assistants,
                        "skippedLoading": skipped_loading,
                        "skippedDuplicateRetry": skipped_duplicate_retry,
                    }),
                );
                // #endregion
            }

            // Diff review hook — blocks the agent before any file write until
            // the user approves or rejects the change in the chat panel.
            let diff_update_tx = update_tx.clone();
            let diff_hook: DiffHookFn = Box::new(move |path, before, after| {
                let tx = diff_update_tx.clone();
                Box::pin(async move {
                    let (os_tx, os_rx) = tokio::sync::oneshot::channel::<bool>();
                    let slot = Arc::new(std::sync::Mutex::new(Some(os_tx)));
                    let _ = tx.send(ChatUpdate::DiffApprovalNeeded {
                        path,
                        before,
                        after,
                        slot,
                    });
                    os_rx.await.unwrap_or(false)
                })
            });

            let mut agent = Agent::new(client)
                .with_cancel_token(cancel_token)
                .with_shared_conversation(shared_conversation)
                .with_diff_hook(diff_hook);

            // Register semantic search tools if sidecar is running.
            if let Some(sc) = sidecar_client {
                agent.register_tool(Box::new(phazeai_sidecar::CodeSearchTool::new(sc.clone())));
                agent.register_tool(Box::new(phazeai_sidecar::BuildIndexTool::new(sc)));
            }

            // Connect to MCP servers
            let mcp_configs = phazeai_core::mcp::McpManager::load_config(&workspace_root);
            if !mcp_configs.is_empty() {
                let mut mcp_manager = phazeai_core::mcp::McpManager::new();
                mcp_manager.connect_all(&mcp_configs);
                agent.register_mcp_tools(std::sync::Arc::new(std::sync::Mutex::new(mcp_manager)));
            }
            // #region agent log
            phazeai_core::debug_ndjson::log(
                "0179af",
                "full-ide-sweep",
                "H15",
                "chat.rs:send_to_ai",
                "mcp config load/connect stage complete",
                serde_json::json!({ "mcpConfigCount": mcp_configs.len() }),
            );
            // #endregion

            let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();

            let run_fut = agent.run_with_events(&user_message, agent_tx);
            let drain_fut = async {
                let mut accumulated = String::new();
                while let Some(event) = agent_rx.recv().await {
                    match event {
                        AgentEvent::TextDelta(text) => {
                            accumulated.push_str(&text);
                            let _ = update_tx.send(ChatUpdate::Partial(accumulated.clone()));
                        }
                        AgentEvent::ToolStart { name } => {
                            let _ = update_tx.send(ChatUpdate::ToolStart { name });
                        }
                        AgentEvent::ToolResult { name, summary, .. } => {
                            let _ = update_tx.send(ChatUpdate::ToolResult { name, summary });
                        }
                        AgentEvent::McpReconnected { servers } => {
                            let msg = if servers.len() == 1 {
                                format!("MCP server '{}' reconnected", servers[0])
                            } else {
                                format!("MCP servers reconnected: {}", servers.join(", "))
                            };
                            let _ = update_tx.send(ChatUpdate::McpStatus(msg));
                        }
                        AgentEvent::Complete { .. } => {
                            let _ = update_tx.send(ChatUpdate::Done(accumulated.clone()));
                            // #region agent log
                            phazeai_core::debug_ndjson::log(
                                "0179af",
                                "full-ide-sweep",
                                "H16",
                                "chat.rs:send_to_ai",
                                "agent stream completed",
                                serde_json::json!({ "finalTextLen": accumulated.len() }),
                            );
                            // #endregion
                            break;
                        }
                        AgentEvent::Error(e) => {
                            // Cancellation is a normal user action — don't treat it as an error.
                            if e == "Cancelled" {
                                let _ = update_tx.send(ChatUpdate::Cancelled(accumulated.clone()));
                                // #region agent log
                                phazeai_core::debug_ndjson::log(
                                    "0179af",
                                    "full-ide-sweep",
                                    "H16",
                                    "chat.rs:send_to_ai",
                                    "agent stream cancelled",
                                    serde_json::json!({ "partialTextLen": accumulated.len() }),
                                );
                                // #endregion
                            } else {
                                let _ = update_tx.send(ChatUpdate::Err(e));
                                // #region agent log
                                phazeai_core::debug_ndjson::log(
                                    "0179af",
                                    "full-ide-sweep",
                                    "H16",
                                    "chat.rs:send_to_ai",
                                    "agent stream errored",
                                    serde_json::json!({ "partialTextLen": accumulated.len() }),
                                );
                                // #endregion
                            }
                            break;
                        }
                        _ => {}
                    }
                }
            };

            let _ = tokio::join!(run_fut, drain_fut);
        });
    });
}

// ── Chat Panel ────────────────────────────────────────────────────────────────

/// Full AI chat panel with real streaming responses and neon-glass aesthetics.
///
/// `ai_thinking` — shared signal from `IdeState`; set to `true` while the AI
/// is generating a response so the sentient gutter glows.
///
/// Settings are re-loaded from disk on each send so model/provider changes in
/// the settings panel take effect immediately without restarting.
/// Expand `@filename` mentions in a chat message into file context blocks.
///
/// Scans for `@path/to/file` tokens, resolves each relative to `root`,
/// reads file contents, and prepends them as context. Returns the expanded prompt.
fn expand_file_mentions(message: &str, root: &std::path::Path) -> String {
    static RE: std::sync::LazyLock<Option<regex::Regex>> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"@([\w./\-]+\.\w+)").ok());
    let Some(re) = &*RE else {
        return message.to_string();
    };
    let mut context_blocks = Vec::new();
    let mut clean_msg = message.to_string();

    let canonical_root = root.canonicalize().ok();
    for cap in re.captures_iter(message) {
        let mention = &cap[1];
        let file_path = root.join(mention);
        let in_workspace = match (&canonical_root, file_path.canonicalize().ok()) {
            (Some(root), Some(path)) => path.starts_with(root),
            _ => false,
        };
        if file_path.is_file() && in_workspace {
            if let Ok(contents) = std::fs::read_to_string(&file_path) {
                // Truncate very large files
                let truncated = if contents.len() > 30_000 {
                    let end = contents.floor_char_boundary(30_000);
                    format!(
                        "{}...\n[truncated — {} bytes total]",
                        &contents[..end],
                        contents.len()
                    )
                } else {
                    contents
                };
                context_blocks.push(format!(
                    "<file path=\"{}\">\n{}\n</file>",
                    mention, truncated
                ));
            }
            // Remove the @mention from the visible message
            clean_msg = clean_msg.replace(&format!("@{mention}"), &format!("`{mention}`"));
        }
    }

    if context_blocks.is_empty() {
        message.to_string()
    } else {
        format!(
            "I'm providing the following file(s) as context:\n\n{}\n\nUser request: {}",
            context_blocks.join("\n\n"),
            clean_msg
        )
    }
}

pub fn chat_panel(
    theme: RwSignal<PhazeTheme>,
    ai_thinking: RwSignal<bool>,
    chat_inject: RwSignal<Option<String>>,
    workspace_root: RwSignal<std::path::PathBuf>,
    sidecar_client: Arc<std::sync::Mutex<Option<Arc<SidecarClient>>>>,
    status_toast: RwSignal<Option<String>>,
) -> impl IntoView {
    let mut initial_messages = vec![ChatMessage {
        role: ChatRole::Assistant,
        content: "Welcome to PhazeAI. How can I help you?".to_string(),
        loading: false,
        is_error: false,
    }];
    let mut initial_id = ConversationStore::generate_id();

    if let Ok(store) = ConversationStore::new() {
        // Try multiple recent conversations so startup remains resilient if the
        // latest entry was quarantined/corrupt between index refreshes.
        if let Ok(recent) = store.list_recent(20) {
            for meta in recent {
                if let Ok(conv) = store.load(&meta.id) {
                    initial_id = meta.id.clone();
                    initial_messages.clear();
                    for m in conv.messages {
                        #[allow(clippy::wildcard_in_or_patterns)]
                        let role = match m.role.as_str() {
                            "user" => ChatRole::User,
                            "assistant" => ChatRole::Assistant,
                            "tool" | "system" | _ => ChatRole::Tool,
                        };
                        initial_messages.push(ChatMessage {
                            role,
                            content: m.content,
                            loading: false,
                            is_error: false,
                        });
                    }
                    break;
                }
            }
        }
    }

    // #region agent log
    phazeai_core::debug_ndjson::log(
        "0179af",
        "full-ide-sweep",
        "H25",
        "chat.rs:chat_panel",
        "chat panel initialized",
        serde_json::json!({
            "initialMessageCount": initial_messages.len(),
            "initialConversationId": initial_id,
        }),
    );
    // #endregion

    let conversation_id = create_rw_signal(initial_id);
    let messages: RwSignal<Vec<ChatMessage>> = create_rw_signal(initial_messages);
    let input_text = create_rw_signal(String::new());
    let is_loading = create_rw_signal(false);
    let mode = create_rw_signal(AiMode::Chat);
    let current_cancel_token: RwSignal<Option<Arc<std::sync::atomic::AtomicBool>>> =
        create_rw_signal(None);

    // ── Diff review state ─────────────────────────────────────────────────────
    let diff_path: RwSignal<String> = create_rw_signal(String::new());
    let diff_before: RwSignal<String> = create_rw_signal(String::new());
    let diff_after: RwSignal<String> = create_rw_signal(String::new());
    let diff_slot: RwSignal<Option<DiffApproveSlot>> = create_rw_signal(None);

    // ── Conversation history UI state (ROADMAP 2.2) ───────────────────────────
    let show_history: RwSignal<bool> = create_rw_signal(false);
    let history_items: RwSignal<Vec<ConversationMetadata>> = create_rw_signal(Vec::new());

    let refresh_history: Rc<dyn Fn()> = Rc::new(move || {
        if let Ok(store) = ConversationStore::new() {
            if let Ok(list) = store.list_recent(50) {
                history_items.set(list);
            }
        }
    });

    let welcome_msg = || ChatMessage {
        role: ChatRole::Assistant,
        content: "Welcome to PhazeAI. How can I help you?".to_string(),
        loading: false,
        is_error: false,
    };

    let new_conv: Rc<dyn Fn()> = Rc::new(move || {
        if is_loading.get_untracked() {
            return;
        }
        messages.set(vec![welcome_msg()]);
        conversation_id.set(ConversationStore::generate_id());
        show_history.set(false);
    });

    let load_conv: Rc<dyn Fn(String)> = Rc::new(move |id: String| {
        if is_loading.get_untracked() {
            return;
        }
        if let Ok(store) = ConversationStore::new() {
            if let Ok(conv) = store.load(&id) {
                let new_msgs: Vec<ChatMessage> = conv
                    .messages
                    .into_iter()
                    .map(|m| {
                        #[allow(clippy::wildcard_in_or_patterns)]
                        let role = match m.role.as_str() {
                            "user" => ChatRole::User,
                            "assistant" => ChatRole::Assistant,
                            "tool" | "system" | _ => ChatRole::Tool,
                        };
                        ChatMessage {
                            role,
                            content: m.content,
                            loading: false,
                            is_error: false,
                        }
                    })
                    .collect();
                messages.set(new_msgs);
                conversation_id.set(id);
                show_history.set(false);
            }
        }
    });

    let delete_conv: Rc<dyn Fn(String)> = {
        let refresh = refresh_history.clone();
        Rc::new(move |id: String| {
            if let Ok(store) = ConversationStore::new() {
                let _ = store.delete(&id);
            }
            if conversation_id.get_untracked() == id {
                messages.set(vec![welcome_msg()]);
                conversation_id.set(ConversationStore::generate_id());
            }
            (refresh)();
        })
    };

    let (update_tx, update_rx) = std::sync::mpsc::sync_channel::<ChatUpdate>(256);
    let update_signal = create_signal_from_channel(update_rx);

    create_effect(move |_| {
        if let Some(update) = update_signal.get() {
            match update {
                ChatUpdate::Partial(text) => {
                    messages.update(|list| {
                        if let Some(last) = list.last_mut() {
                            if last.role == ChatRole::Assistant && last.loading {
                                last.content = text;
                            }
                        }
                    });
                }
                ChatUpdate::ToolStart { name } => {
                    messages.update(|list| {
                        list.push(ChatMessage {
                            role: ChatRole::Tool,
                            content: format!("Running tool: {}...", name),
                            loading: true,
                            is_error: false,
                        });
                    });
                }
                ChatUpdate::ToolResult { name, summary } => {
                    messages.update(|list| {
                        if let Some(last) = list.last_mut() {
                            if last.role == ChatRole::Tool && last.loading {
                                last.content = format!("{}: {}", name, summary);
                                last.loading = false;
                            }
                        }
                    });
                    let msgs = messages.get_untracked();
                    save_conversation(
                        &msgs,
                        &conversation_id.get_untracked(),
                        &Settings::load().llm.model,
                        &workspace_root.get_untracked(),
                    );
                }
                ChatUpdate::Done(text) => {
                    messages.update(|list| {
                        // Ensure we finalize any "hanging" assistant message
                        if let Some(last) = list.last_mut() {
                            if last.role == ChatRole::Assistant && last.loading {
                                last.content = if text.is_empty() {
                                    "(no response)".to_string()
                                } else {
                                    text
                                };
                                last.loading = false;
                            }
                        }
                    });
                    is_loading.set(false);
                    ai_thinking.set(false);
                    let msgs = messages.get_untracked();
                    save_conversation(
                        &msgs,
                        &conversation_id.get_untracked(),
                        &Settings::load().llm.model,
                        &workspace_root.get_untracked(),
                    );
                }
                ChatUpdate::Err(e) => {
                    messages.update(|list| {
                        // Remove any in-flight loading assistant message so the
                        // error bubble appears cleanly (no empty ghost bubble).
                        if let Some(last) = list.last() {
                            if last.loading && last.role == ChatRole::Assistant {
                                list.pop();
                            }
                        }
                        list.push(ChatMessage {
                            role: ChatRole::Assistant,
                            content: format_chat_error(&e),
                            loading: false,
                            is_error: true,
                        });
                    });
                    is_loading.set(false);
                    ai_thinking.set(false);
                    let msgs = messages.get_untracked();
                    save_conversation(
                        &msgs,
                        &conversation_id.get_untracked(),
                        &Settings::load().llm.model,
                        &workspace_root.get_untracked(),
                    );
                }
                ChatUpdate::McpStatus(msg) => {
                    status_toast.set(Some(msg));
                }
                ChatUpdate::DiffApprovalNeeded {
                    path,
                    before,
                    after,
                    slot,
                } => {
                    diff_path.set(path);
                    diff_before.set(before);
                    diff_after.set(after);
                    diff_slot.set(Some(slot));
                }
                ChatUpdate::Cancelled(partial) => {
                    messages.update(|list| {
                        // Finalise the loading assistant message (may be empty or partial).
                        if let Some(last) = list.last_mut() {
                            if last.role == ChatRole::Assistant && last.loading {
                                last.content = if partial.is_empty() {
                                    "(generation stopped)".to_string()
                                } else {
                                    partial
                                };
                                last.loading = false;
                                last.is_error = false;
                            }
                        }
                    });
                    is_loading.set(false);
                    ai_thinking.set(false);
                    current_cancel_token.set(None);
                    let msgs = messages.get_untracked();
                    save_conversation(
                        &msgs,
                        &conversation_id.get_untracked(),
                        &Settings::load().llm.model,
                        &workspace_root.get_untracked(),
                    );
                }
            }
        }
    });

    // ── Send closure ──────────────────────────────────────────────────────────

    let update_tx = Arc::new(update_tx);

    let do_send: Rc<dyn Fn()> = Rc::new({
        let update_tx = update_tx.clone();
        let sidecar_client = sidecar_client.clone();
        move || {
            let text = input_text.get();
            let trimmed = text.trim().to_string();
            if trimmed.is_empty() || is_loading.get() {
                // #region agent log
                phazeai_core::debug_ndjson::log(
                    "0179af",
                    "full-ide-sweep",
                    "H20",
                    "chat.rs:do_send",
                    "send blocked before dispatch",
                    serde_json::json!({
                        "trimmedEmpty": trimmed.is_empty(),
                        "isLoading": is_loading.get(),
                    }),
                );
                // #endregion
                return;
            }
            let prior_messages = messages.get_untracked();

            // Expand @file mentions into context blocks before sending to AI
            let root = workspace_root.get_untracked();
            let prompt = expand_file_mentions(&trimmed, &root);

            messages.update(|list| {
                list.push(ChatMessage {
                    role: ChatRole::User,
                    content: trimmed.clone(),
                    loading: false,
                    is_error: false,
                });
                list.push(ChatMessage {
                    role: ChatRole::Assistant,
                    content: String::new(),
                    loading: true,
                    is_error: false,
                });
            });
            input_text.set(String::new());
            is_loading.set(true);
            ai_thinking.set(true);

            let token = Arc::new(std::sync::atomic::AtomicBool::new(false));
            current_cancel_token.set(Some(token.clone()));

            // Re-read settings on every send so model/provider changes in the
            // settings panel take effect immediately (no restart needed).
            let live_settings = Settings::load();
            let hint = mode.get_untracked().system_hint();
            let sc_snapshot = sidecar_client.lock().ok().and_then(|g| g.as_ref().cloned());
            // #region agent log
            phazeai_core::debug_ndjson::log(
                "0179af",
                "full-ide-sweep",
                "H21",
                "chat.rs:do_send",
                "dispatching send_to_ai from do_send",
                serde_json::json!({
                    "promptLen": prompt.len(),
                    "priorMessages": prior_messages.len(),
                    "modeHintEmpty": hint.is_empty(),
                    "hasSidecar": sc_snapshot.is_some(),
                }),
            );
            // #endregion
            send_to_ai(SendToAiJob {
                user_message: prompt,
                prior_messages,
                settings: live_settings,
                workspace_root: root,
                mode_hint: hint,
                update_tx: (*update_tx).clone(),
                cancel_token: token,
                sidecar_client: sc_snapshot,
            });
        }
    });

    // ── Inject from context menu (Explain Selection / Generate Tests / Fix) ──
    {
        let do_send = do_send.clone();
        create_effect(move |_| {
            if let Some(text) = chat_inject.get() {
                input_text.set(text);
                chat_inject.set(None);
                do_send();
            }
        });
    }

    // ── Header — neon strip + title ───────────────────────────────────────────

    // 2px accent-colored top strip (the "neon line" on top of the panel)
    let neon_strip = container(label(|| "")).style(move |s| {
        s.height(2.0)
            .width_full()
            .background(theme.get().palette.accent)
    });

    let new_btn = {
        let new_conv = new_conv.clone();
        let hov = create_rw_signal(false);
        container(
            label(|| "+ New")
                .style(move |s| s.font_size(11.0).color(theme.get().palette.text_muted)),
        )
        .style(move |s| {
            let p = &theme.get().palette;
            s.padding_horiz(8.0)
                .padding_vert(3.0)
                .border(1.0)
                .border_color(p.glass_border)
                .border_radius(4.0)
                .cursor(floem::style::CursorStyle::Pointer)
                .margin_right(6.0)
                .background(if hov.get() {
                    p.bg_elevated
                } else {
                    floem::peniko::Color::TRANSPARENT
                })
        })
        .on_event_stop(EventListener::PointerEnter, move |_| hov.set(true))
        .on_event_stop(EventListener::PointerLeave, move |_| hov.set(false))
        .on_click_stop(move |_| (new_conv)())
    };

    let history_btn = {
        let refresh = refresh_history.clone();
        let hov = create_rw_signal(false);
        container(
            label(move || {
                if show_history.get() {
                    "History ▴".to_string()
                } else {
                    "History ▾".to_string()
                }
            })
            .style(move |s| s.font_size(11.0).color(theme.get().palette.text_muted)),
        )
        .style(move |s| {
            let p = &theme.get().palette;
            let active = show_history.get();
            s.padding_horiz(8.0)
                .padding_vert(3.0)
                .border(1.0)
                .border_color(if active { p.accent } else { p.glass_border })
                .border_radius(4.0)
                .cursor(floem::style::CursorStyle::Pointer)
                .background(if active {
                    p.accent_dim
                } else if hov.get() {
                    p.bg_elevated
                } else {
                    floem::peniko::Color::TRANSPARENT
                })
        })
        .on_event_stop(EventListener::PointerEnter, move |_| hov.set(true))
        .on_event_stop(EventListener::PointerLeave, move |_| hov.set(false))
        .on_click_stop(move |_| {
            let open = !show_history.get_untracked();
            show_history.set(open);
            if open {
                (refresh)();
            }
        })
    };

    let header_content = container(
        stack((
            container(
                stack((
                    phaze_icon(icons::AI, 14.0, move |p| p.accent, theme),
                    label(|| "  PHAZEAI").style(move |s| {
                        s.font_size(11.0)
                            .color(theme.get().palette.accent)
                            .font_weight(floem::text::Weight::BOLD)
                    }),
                ))
                .style(|s| s.items_center()),
            )
            .style(|s| s.flex_grow(1.0)),
            new_btn,
            history_btn,
        ))
        .style(|s| s.items_center().width_full()),
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(14.0)
            .padding_vert(10.0)
            .border_bottom(1.0)
            .border_color(p.glass_border)
            .width_full()
            .background(p.glass_bg)
    });

    let header = stack((neon_strip, header_content)).style(|s| s.flex_col().width_full());

    // ── Mode tabs (Chat / Ask / Debug / Plan / Edit) ──────────────────────────

    let all_modes = [
        AiMode::Chat,
        AiMode::Ask,
        AiMode::Debug,
        AiMode::Plan,
        AiMode::Edit,
    ];

    let mode_tab = |m: AiMode| {
        let is_hov = create_rw_signal(false);
        container(label(move || m.label()))
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                let active = mode.get() == m;
                s.padding_horiz(9.0)
                    .padding_vert(4.0)
                    .font_size(11.0)
                    .color(if active { p.accent } else { p.text_muted })
                    .background(if active {
                        p.accent_dim
                    } else if is_hov.get() {
                        p.bg_elevated
                    } else {
                        floem::peniko::Color::TRANSPARENT
                    })
                    .border_radius(4.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .apply_if(active, |s| s.border_bottom(2.0).border_color(p.accent))
            })
            .on_click_stop(move |_| {
                mode.set(m);
            })
            .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
                is_hov.set(true);
            })
            .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
                is_hov.set(false);
            })
    };

    let mode_tabs = stack((
        mode_tab(all_modes[0]),
        mode_tab(all_modes[1]),
        mode_tab(all_modes[2]),
        mode_tab(all_modes[3]),
        mode_tab(all_modes[4]),
    ))
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.width_full()
            .background(p.glass_bg)
            .border_bottom(1.0)
            .border_color(p.glass_border)
            .items_center()
            .padding_horiz(4.0)
            .padding_vert(4.0)
    });

    let do_retry: Rc<dyn Fn()> = Rc::new({
        let update_tx = update_tx.clone();
        let sidecar_client = sidecar_client.clone();
        move || {
            if is_loading.get() {
                // #region agent log
                phazeai_core::debug_ndjson::log(
                    "0179af",
                    "full-ide-sweep",
                    "H22",
                    "chat.rs:do_retry",
                    "retry blocked due to loading state",
                    serde_json::json!({ "isLoading": true }),
                );
                // #endregion
                return;
            }

            let msgs = messages.get_untracked();
            let mut last_user_msg = None;
            for msg in msgs.iter().rev() {
                if msg.role == ChatRole::User {
                    last_user_msg = Some(msg.content.clone());
                    break;
                }
            }

            if let Some(user_msg) = last_user_msg {
                let prior_messages = shape_retry_prior_messages(&msgs, &user_msg);
                messages.update(|list| {
                    while let Some(last) = list.last() {
                        if last.role != ChatRole::User {
                            list.pop();
                        } else {
                            break;
                        }
                    }
                    list.push(ChatMessage {
                        role: ChatRole::Assistant,
                        content: String::new(),
                        loading: true,
                        is_error: false,
                    });
                });

                is_loading.set(true);
                ai_thinking.set(true);

                let token = Arc::new(std::sync::atomic::AtomicBool::new(false));
                current_cancel_token.set(Some(token.clone()));

                let root = workspace_root.get_untracked();
                let prompt = expand_file_mentions(&user_msg, &root);
                let live_settings = Settings::load();
                let hint = mode.get_untracked().system_hint();
                let sc_snapshot = sidecar_client.lock().ok().and_then(|g| g.as_ref().cloned());
                // #region agent log
                phazeai_core::debug_ndjson::log(
                    "0179af",
                    "full-ide-sweep",
                    "H23",
                    "chat.rs:do_retry",
                    "dispatching send_to_ai from retry",
                    serde_json::json!({
                        "promptLen": prompt.len(),
                        "sourceMessages": msgs.len(),
                        "priorMessages": prior_messages.len(),
                        "modeHintEmpty": hint.is_empty(),
                        "hasSidecar": sc_snapshot.is_some(),
                    }),
                );
                // #endregion
                send_to_ai(SendToAiJob {
                    user_message: prompt,
                    prior_messages,
                    settings: live_settings,
                    workspace_root: root,
                    mode_hint: hint,
                    update_tx: (*update_tx).clone(),
                    cancel_token: token,
                    sidecar_client: sc_snapshot,
                });
            } else {
                // #region agent log
                phazeai_core::debug_ndjson::log(
                    "0179af",
                    "full-ide-sweep",
                    "H24",
                    "chat.rs:do_retry",
                    "retry requested but no user message found",
                    serde_json::json!({ "messageCount": msgs.len() }),
                );
                // #endregion
            }
        }
    });

    // ── Message bubbles ───────────────────────────────────────────────────────

    let msg_list = dyn_stack(
        move || {
            let list = safe_get(messages, Vec::new());
            let len = list.len();
            list.into_iter()
                .enumerate()
                .map(|(i, msg)| (i, msg, i == len - 1))
                .collect::<Vec<_>>()
        },
        |(i, _, _)| *i,
        move |(i, msg, is_last)| {
            let is_user = msg.role == ChatRole::User;
            let content = msg.content.clone();
            let loading = msg.loading;
            let is_error = msg.is_error;

            let text_content = if loading && content.is_empty() {
                "●●●".to_string()
            } else {
                content
            };
            let is_typing = loading && text_content.starts_with('●');
            let is_tool = msg.role == ChatRole::Tool;
            // Error messages always show retry; other AI messages only on the last one.
            // Use a signal read inside the style closure so it stays reactive.
            let show_retry_for_error = is_error && !is_user;
            let show_retry_for_last = !is_user && is_last && !is_tool;
            let do_retry_btn = do_retry.clone();
            let do_retry_btn2 = do_retry.clone();

            // Icon-only retry button shown at the trailing edge of normal AI messages.
            let icon_retry_btn = container(phaze_icon(
                icons::REFRESH,
                12.0,
                move |p| p.text_secondary,
                theme,
            ))
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                // Reactive: re-evaluate is_loading every render pass.
                let should_show = show_retry_for_last && !is_loading.get() && !is_error;
                s.padding(4.0)
                    .border_radius(4.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.background(p.bg_elevated))
                    .apply_if(!should_show, |s| s.display(floem::style::Display::None))
            })
            .on_click_stop(move |_| {
                (do_retry_btn)();
            });

            // ✕ dismiss button — removes this error bubble from the message list.
            let dismiss_btn = container(
                label(|| "✕")
                    .style(move |s| s.font_size(10.0).color(theme.get().palette.text_muted)),
            )
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.padding(4.0)
                    .border_radius(4.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.background(p.error.with_alpha(0.15)))
                    .apply_if(!is_error, |s| s.display(floem::style::Display::None))
            })
            .on_click_stop(move |_| {
                messages.update(|list| {
                    if i < list.len() {
                        list.remove(i);
                    }
                });
            });

            // "Retry" text button shown inside error bubbles.
            let error_retry_btn = container(
                stack((
                    phaze_icon(icons::REFRESH, 11.0, move |p| p.error, theme),
                    label(|| " Retry")
                        .style(move |s| s.font_size(11.0).color(theme.get().palette.error)),
                ))
                .style(|s| s.items_center()),
            )
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.padding_horiz(8.0)
                    .padding_vert(4.0)
                    .border_radius(6.0)
                    .border(1.0)
                    .border_color(p.error.with_alpha(0.4))
                    .cursor(floem::style::CursorStyle::Pointer)
                    .margin_top(8.0)
                    .hover(|s| s.background(p.error.with_alpha(0.15)))
                    .apply_if(!show_retry_for_error, |s| {
                        s.display(floem::style::Display::None)
                    })
            })
            .on_click_stop(move |_| {
                (do_retry_btn2)();
            });

            container(
                stack((
                    // Row: tool-chip + message text + action buttons
                    stack((
                        stack((
                            phaze_icon(icons::CHIP, 11.0, move |p| p.accent, theme).style(
                                move |s: floem::style::Style| {
                                    s.apply_if(!is_tool, |s| s.display(floem::style::Display::None))
                                },
                            ),
                            label(move || text_content.clone()).style(move |s| {
                                let t = theme.get();
                                let p = &t.palette;
                                s.font_size(if is_tool { 11.0 } else { 13.0 })
                                    .color(if is_user {
                                        p.text_primary
                                    } else if is_error {
                                        p.error
                                    } else if is_typing || is_tool {
                                        p.accent
                                    } else {
                                        p.text_secondary
                                    })
                                    .max_width_pct(100.0)
                                    .line_height(1.5)
                                    .apply_if(is_tool, |s| {
                                        s.font_weight(floem::text::Weight::MEDIUM)
                                    })
                            }),
                        ))
                        .style(|s| s.items_center().flex_grow(1.0)),
                        // Retry icon (non-error AI messages) + dismiss ✕ (error messages)
                        stack((icon_retry_btn, dismiss_btn)).style(|s| s.items_center().gap(2.0)),
                    ))
                    .style(|s| s.items_center().justify_between().width_full()),
                    // Error retry button below the error text (only for error bubbles)
                    error_retry_btn,
                ))
                .style(|s| s.flex_col().width_full()),
            )
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                if is_user {
                    // User bubble: accent tinted glass
                    s.width_full()
                        .padding_horiz(14.0)
                        .padding_vert(10.0)
                        .background(p.accent_dim)
                        .border(1.0)
                        .border_color(p.glass_border)
                        .border_radius(12.0)
                        .margin_bottom(8.0)
                        // Subtle inner glow
                        .box_shadow_blur(12.0)
                        .box_shadow_color(p.glow)
                        .box_shadow_spread(0.0)
                        .box_shadow_h_offset(0.0)
                        .box_shadow_v_offset(0.0)
                } else if is_tool {
                    // Tool card: specialized micro-bubble
                    s.width_full()
                        .padding_horiz(10.0)
                        .padding_vert(6.0)
                        .background(p.bg_deep.with_alpha(0.6))
                        .border(1.0)
                        .border_color(p.glass_border)
                        .border_radius(6.0)
                        .margin_bottom(6.0)
                        .margin_horiz(20.0) // Indent tool calls
                } else if is_error {
                    s.width_full()
                        .padding_horiz(14.0)
                        .padding_vert(10.0)
                        .background(p.error.with_alpha(0.1))
                        .border(1.0)
                        .border_color(p.error.with_alpha(0.3))
                        .border_radius(10.0)
                        .margin_bottom(8.0)
                } else {
                    // Assistant bubble: darker glass for better readability
                    s.width_full()
                        .padding_horiz(14.0)
                        .padding_vert(10.0)
                        .background(p.bg_panel)
                        .border(1.0)
                        .border_color(p.glass_border)
                        .border_radius(10.0)
                        .margin_bottom(8.0)
                }
            })
        },
    )
    .style(|s| s.flex_col().padding(10.0).gap(0.0).width_full());

    let messages_scroll = scroll(msg_list).style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    // ── Input bar ─────────────────────────────────────────────────────────────

    let do_send_btn = do_send.clone();
    let do_send_key = do_send.clone();
    let _ = do_send;

    let send_btn = container(
        stack((
            // Send state: enter arrow
            label(|| "↵").style(move |s| {
                s.font_size(14.0)
                    .color(theme.get().palette.bg_base)
                    .apply_if(is_loading.get(), |s| s.display(floem::style::Display::None))
            }),
            // Stop state: stop icon + "Stop" text
            stack((
                phaze_icon(icons::STOP, 12.0, move |p| p.text_primary, theme),
                label(|| " Stop").style(move |s| {
                    s.font_size(11.0)
                        .color(theme.get().palette.text_primary)
                        .font_weight(floem::text::Weight::MEDIUM)
                }),
            ))
            .style(move |s| {
                s.items_center().apply_if(!is_loading.get(), |s| {
                    s.display(floem::style::Display::None)
                })
            }),
        ))
        .style(|s| s.items_center().justify_center()),
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        let loading = is_loading.get();
        s.height(32.0)
            .padding_horiz(if loading { 10.0 } else { 0.0 })
            .min_width(32.0)
            .background(if loading { p.bg_elevated } else { p.accent })
            .border_radius(8.0)
            .apply_if(loading, |s| s.border(1.0).border_color(p.glass_border))
            .items_center()
            .justify_center()
            .cursor(floem::style::CursorStyle::Pointer)
            .margin_left(8.0)
            // Glow on the send button when not loading
            .apply_if(!loading, |s| {
                s.width(32.0)
                    .box_shadow_blur(10.0)
                    .box_shadow_color(p.glow)
                    .box_shadow_spread(0.0)
                    .box_shadow_h_offset(0.0)
                    .box_shadow_v_offset(0.0)
            })
    })
    .on_click_stop(move |_| {
        if is_loading.get() {
            if let Some(token) = current_cancel_token.get() {
                token.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        } else {
            (do_send_btn)();
        }
    });

    let input_widget = text_input(input_text)
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.flex_grow(1.0)
                .background(p.glass_bg)
                .border(1.0)
                .border_color(p.border_focus)
                .border_radius(8.0)
                .color(p.text_primary)
                .padding_horiz(12.0)
                .padding_vert(8.0)
                .font_size(13.0)
                .min_width(0.0)
        })
        .on_event_stop(EventListener::KeyDown, move |event| {
            if let Event::KeyDown(e) = event {
                match &e.key.logical_key {
                    Key::Named(floem::keyboard::NamedKey::Escape) => {
                        if is_loading.get() {
                            if let Some(token) = current_cancel_token.get_untracked() {
                                token.store(true, std::sync::atomic::Ordering::SeqCst);
                            }
                        }
                    }
                    key => {
                        let enter = match key {
                            Key::Character(ch) => ch.as_str() == "\r" || ch.as_str() == "\n",
                            Key::Named(floem::keyboard::NamedKey::Enter) => true,
                            _ => false,
                        };
                        if enter && !e.modifiers.contains(Modifiers::SHIFT) {
                            (do_send_key)();
                        }
                    }
                }
            }
        });

    let input_bar = container(
        stack((input_widget, send_btn)).style(|s| s.items_center().width_full()),
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding(10.0)
            .border_top(1.0)
            .border_color(p.glass_border)
            .width_full()
            .background(p.glass_bg)
    });

    // ── Conversation history panel (collapsible) ─────────────────────────────

    let history_panel = {
        let load_conv = load_conv.clone();
        let delete_conv = delete_conv.clone();
        let list = dyn_stack(
            move || history_items.get(),
            |meta: &ConversationMetadata| meta.id.clone(),
            move |meta: ConversationMetadata| {
                let id_load = meta.id.clone();
                let id_del = meta.id.clone();
                let title = if meta.title.is_empty() {
                    "(untitled)".to_string()
                } else {
                    meta.title.clone()
                };
                let subtitle = format!("{} msgs · {}", meta.message_count, meta.updated_at);
                let load_conv = load_conv.clone();
                let delete_conv = delete_conv.clone();
                let row_hov = create_rw_signal(false);
                let is_active = conversation_id.get_untracked() == meta.id;

                stack((
                    container(
                        stack((
                            label(move || title.clone()).style(move |s| {
                                s.font_size(12.0)
                                    .color(theme.get().palette.text_primary)
                                    .font_weight(floem::text::Weight::MEDIUM)
                            }),
                            label(move || subtitle.clone()).style(move |s| {
                                s.font_size(10.0).color(theme.get().palette.text_muted)
                            }),
                        ))
                        .style(|s| s.flex_col()),
                    )
                    .style(|s| s.flex_grow(1.0))
                    .on_click_stop(move |_| (load_conv)(id_load.clone())),
                    label(|| "×")
                        .style(move |s| {
                            s.font_size(16.0)
                                .color(theme.get().palette.text_muted)
                                .padding_horiz(10.0)
                                .cursor(floem::style::CursorStyle::Pointer)
                        })
                        .on_click_stop(move |_| (delete_conv)(id_del.clone())),
                ))
                .style(move |s| {
                    let p = &theme.get().palette;
                    s.items_center()
                        .width_full()
                        .padding_horiz(10.0)
                        .padding_vert(6.0)
                        .border_bottom(1.0)
                        .border_color(p.glass_border)
                        .background(if is_active {
                            p.accent_dim
                        } else if row_hov.get() {
                            p.bg_elevated
                        } else {
                            floem::peniko::Color::TRANSPARENT
                        })
                        .cursor(floem::style::CursorStyle::Pointer)
                })
                .on_event_stop(EventListener::PointerEnter, move |_| row_hov.set(true))
                .on_event_stop(EventListener::PointerLeave, move |_| row_hov.set(false))
            },
        )
        .style(|s| s.flex_col().width_full());

        container(scroll(list).style(|s| s.height(220.0).width_full())).style(move |s| {
            let p = &theme.get().palette;
            s.width_full()
                .background(p.glass_bg)
                .border_bottom(1.0)
                .border_color(p.glass_border)
                .apply_if(!show_history.get(), |s| {
                    s.display(floem::style::Display::None)
                })
        })
    };

    // ── Diff review overlay ───────────────────────────────────────────────────
    // Appears between the message list and input bar while a file change is
    // pending approval. The agent is blocked on a oneshot until the user
    // clicks Approve or Reject.

    let diff_overlay = {
        // Helper: resolve the pending approval with the given bool, then clear.
        let resolve = move |approved: bool| {
            if let Some(slot) = diff_slot.get_untracked() {
                if let Ok(mut g) = slot.lock() {
                    if let Some(tx) = g.take() {
                        let _ = tx.send(approved);
                    }
                }
            }
            diff_path.set(String::new());
            diff_before.set(String::new());
            diff_after.set(String::new());
            diff_slot.set(None);
        };
        let resolve_approve = {
            let r = resolve;
            move || r(true)
        };
        let resolve_reject = {
            let r = resolve;
            move || r(false)
        };

        // Build diff lines from before/after text using the `similar` crate.
        let diff_lines_view = dyn_stack(
            move || {
                let before = diff_before.get();
                let after = diff_after.get();
                if before.is_empty() && after.is_empty() {
                    return vec![];
                }
                let diff = similar::TextDiff::from_lines(&before, &after);
                let mut out: Vec<(char, String)> = Vec::new();
                for change in diff.iter_all_changes() {
                    let tag = match change.tag() {
                        similar::ChangeTag::Delete => '-',
                        similar::ChangeTag::Insert => '+',
                        similar::ChangeTag::Equal => ' ',
                    };
                    let text = change.value().to_string();
                    out.push((tag, text));
                }
                out
            },
            |(tag, text): &(char, String)| format!("{tag}{text}"),
            move |(tag, text): (char, String)| {
                let is_add = tag == '+';
                let is_del = tag == '-';
                label(move || format!("{} {}", tag, text.trim_end_matches('\n')))
                    .style(move |s| {
                        let p = &theme.get().palette;
                        s.font_family("monospace".to_string())
                            .font_size(11.0)
                            .padding_horiz(8.0)
                            .padding_vert(1.0)
                            .width_full()
                            .color(if is_add {
                                floem::peniko::Color::from_rgb8(130, 220, 130)
                            } else if is_del {
                                floem::peniko::Color::from_rgb8(220, 120, 120)
                            } else {
                                p.text_muted
                            })
                            .background(if is_add {
                                floem::peniko::Color::from_rgba8(0, 80, 0, 80)
                            } else if is_del {
                                floem::peniko::Color::from_rgba8(80, 0, 0, 80)
                            } else {
                                floem::peniko::Color::TRANSPARENT
                            })
                    })
            },
        )
        .style(|s| s.flex_col().width_full());

        // Approve button
        let approve_btn = {
            let hov = create_rw_signal(false);
            let resolve_approve = resolve_approve.clone();
            container(label(|| "✓ Approve").style(move |s| {
                s.font_size(12.0)
                    .color(floem::peniko::Color::from_rgb8(40, 180, 40))
                    .font_weight(floem::text::Weight::MEDIUM)
            }))
            .style(move |s| {
                let p = &theme.get().palette;
                s.padding_horiz(14.0)
                    .padding_vert(6.0)
                    .border(1.0)
                    .border_color(floem::peniko::Color::from_rgba8(40, 180, 40, 100))
                    .border_radius(6.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .background(if hov.get() {
                        floem::peniko::Color::from_rgba8(0, 80, 0, 80)
                    } else {
                        p.bg_deep.with_alpha(0.6)
                    })
            })
            .on_event_stop(EventListener::PointerEnter, move |_| hov.set(true))
            .on_event_stop(EventListener::PointerLeave, move |_| hov.set(false))
            .on_click_stop(move |_| (resolve_approve)())
        };

        // Reject button
        let reject_btn = {
            let hov = create_rw_signal(false);
            container(label(|| "✗ Reject").style(move |s| {
                s.font_size(12.0)
                    .color(floem::peniko::Color::from_rgb8(200, 80, 80))
                    .font_weight(floem::text::Weight::MEDIUM)
            }))
            .style(move |s| {
                let p = &theme.get().palette;
                s.padding_horiz(14.0)
                    .padding_vert(6.0)
                    .border(1.0)
                    .border_color(floem::peniko::Color::from_rgba8(200, 80, 80, 100))
                    .border_radius(6.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .background(if hov.get() {
                        floem::peniko::Color::from_rgba8(80, 0, 0, 80)
                    } else {
                        p.bg_deep.with_alpha(0.6)
                    })
            })
            .on_event_stop(EventListener::PointerEnter, move |_| hov.set(true))
            .on_event_stop(EventListener::PointerLeave, move |_| hov.set(false))
            .on_click_stop(move |_| (resolve_reject)())
        };

        let btn_row = stack((approve_btn, reject_btn)).style(|s| s.gap(8.0).items_center());

        container(
            stack((
                // Header bar: path + label
                container(
                    stack((
                        label(|| "Review change —").style(move |s| {
                            s.font_size(11.0).color(theme.get().palette.text_muted)
                        }),
                        label(move || {
                            let p = diff_path.get();
                            std::path::Path::new(&p)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or(&p)
                                .to_string()
                        })
                        .style(move |s| {
                            s.font_size(11.0)
                                .font_family("monospace".to_string())
                                .color(theme.get().palette.accent)
                        }),
                    ))
                    .style(|s| s.items_center().gap(6.0)),
                )
                .style(move |s| {
                    let p = &theme.get().palette;
                    s.padding_horiz(12.0)
                        .padding_vert(6.0)
                        .border_bottom(1.0)
                        .border_color(p.glass_border)
                        .width_full()
                }),
                // Scrollable diff body
                scroll(diff_lines_view).style(|s| s.width_full().max_height(240.0)),
                // Action buttons
                container(btn_row).style(move |s| {
                    let p = &theme.get().palette;
                    s.padding(10.0)
                        .border_top(1.0)
                        .border_color(p.glass_border)
                        .width_full()
                        .justify_end()
                }),
            ))
            .style(|s| s.flex_col().width_full()),
        )
        .style(move |s| {
            let p = &theme.get().palette;
            let visible = !diff_path.get().is_empty();
            s.width_full()
                .border_top(1.0)
                .border_color(p.glass_border)
                .background(p.bg_deep)
                .apply_if(!visible, |s| s.display(floem::style::Display::None))
        })
    };

    // ── Full panel ────────────────────────────────────────────────────────────

    stack((header, history_panel, mode_tabs, messages_scroll, diff_overlay, input_bar))
        .style(move |s| s.flex_col().width_full().height_full())
}

#[cfg(test)]
mod tests {
    use super::{shape_retry_prior_messages, ChatMessage, ChatRole};

    fn msg(role: ChatRole, content: &str, loading: bool, is_error: bool) -> ChatMessage {
        ChatMessage {
            role,
            content: content.to_string(),
            loading,
            is_error,
        }
    }

    #[test]
    fn shape_retry_drops_trailing_assistant_and_tool_messages() {
        let messages = vec![
            msg(ChatRole::User, "u1", false, false),
            msg(ChatRole::Assistant, "a1", false, false),
            msg(ChatRole::User, "u2", false, false),
            msg(ChatRole::Tool, "tool", false, false),
            msg(ChatRole::Assistant, "Error: timeout", false, true),
        ];

        let shaped = shape_retry_prior_messages(&messages, "u2");
        assert_eq!(shaped.len(), 2);
        assert_eq!(shaped[0].content, "u1");
        assert_eq!(shaped[1].content, "a1");
    }

    #[test]
    fn shape_retry_handles_pending_user_bubble() {
        let messages = vec![
            msg(ChatRole::User, "u1", false, false),
            msg(ChatRole::Assistant, "a1", false, false),
            msg(ChatRole::User, "u2", false, false),
        ];

        let shaped = shape_retry_prior_messages(&messages, "u2");
        assert_eq!(shaped.len(), 2);
        assert_eq!(shaped[0].content, "u1");
        assert_eq!(shaped[1].content, "a1");
    }

    #[test]
    fn shape_retry_keeps_history_when_retrying_older_user() {
        let messages = vec![
            msg(ChatRole::User, "u1", false, false),
            msg(ChatRole::Assistant, "a1", false, false),
            msg(ChatRole::User, "u2", false, false),
            msg(ChatRole::Assistant, "a2", false, false),
        ];

        let shaped = shape_retry_prior_messages(&messages, "u1");
        assert_eq!(shaped.len(), 3);
        assert_eq!(shaped[0].content, "u1");
        assert_eq!(shaped[1].content, "a1");
        assert_eq!(shaped[2].content, "u2");
    }
}
