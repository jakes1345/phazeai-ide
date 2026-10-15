//! Executing slash commands and other user actions against the app state.

use phazeai_core::context::ConversationStore;
use phazeai_core::tools::ToolApprovalMode;
use phazeai_core::{ProviderRegistry, Settings};

use super::chat::{ChatItem, ChatMessage, MessageRole};
use super::worker::{WorkerCommand, CONTEXT_BUDGET};
use super::{AppState, SessionPicker};
use crate::commands::CommandResult;
use crate::setup;
use crate::theme::Theme;

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

pub(super) fn fmt_tokens(n: u64) -> String {
    format_tokens(n)
}

/// Build a client for `settings`; on success hand it to the worker and adopt
/// the settings. Returns the friendly error text on failure.
fn switch_llm(state: &mut AppState, settings: Settings) -> Result<(), String> {
    match settings.build_llm_client() {
        Ok(llm) => {
            let _ = state.worker_tx.send(WorkerCommand::SetLlm(llm));
            state.settings = settings;
            state.llm_error = None;
            Ok(())
        }
        Err(e) => Err(setup::explain_build_error(
            &settings,
            &e.to_string(),
            setup::Surface::Tui,
        )),
    }
}

fn busy(state: &mut AppState, what: &str) -> bool {
    if state.processing {
        state.notify(format!(
            "Can't {what} while a response is running — press Esc to interrupt first"
        ));
        true
    } else {
        false
    }
}

pub(super) fn open_session_picker(state: &mut AppState) {
    match state.conversation_store.list_recent(30) {
        Ok(list) if list.is_empty() => state.notify("No saved conversations yet"),
        Ok(list) => state.session_picker = Some(SessionPicker { list, index: 0 }),
        Err(e) => state.error(format!("Couldn't list conversations: {e}")),
    }
}

pub(super) fn open_external_editor(state: &mut AppState) {
    let path = std::env::temp_dir().join(format!("phazeai_prompt_{}.md", std::process::id()));
    if let Err(e) = std::fs::write(&path, state.input.text()) {
        state.error(format!("Couldn't create a temp file for the editor: {e}"));
        return;
    }
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                "notepad".into()
            } else {
                "nano".into()
            }
        });
    // $EDITOR may include arguments (e.g. "code --wait").
    let mut parts = editor.split_whitespace();
    let program = parts.next().unwrap_or("nano").to_string();
    let args: Vec<String> = parts.map(String::from).collect();
    let status = crate::term::suspend(|| {
        std::process::Command::new(&program)
            .args(&args)
            .arg(&path)
            .status()
    });
    state.needs_clear = true;
    match status {
        Ok(Ok(s)) if s.success() => match std::fs::read_to_string(&path) {
            Ok(content) => state.input.set(content.trim_end().to_string()),
            Err(e) => state.error(format!("Couldn't read the edited prompt: {e}")),
        },
        Ok(Ok(s)) => state.notify(format!("{program} exited with {s}; prompt unchanged")),
        Ok(Err(e)) => state.error(format!(
            "Couldn't start editor `{program}`: {e}. Set $EDITOR to your editor."
        )),
        Err(e) => state.error(format!("Terminal error while running the editor: {e}")),
    }
    let _ = std::fs::remove_file(path);
}

fn clipboard(state: &mut AppState) -> Result<&mut arboard::Clipboard, String> {
    if state.clipboard.is_none() {
        state.clipboard = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
    }
    Ok(state.clipboard.as_mut().expect("just initialised"))
}

pub(super) fn paste_clipboard(state: &mut AppState) {
    let text = clipboard(state).and_then(|cb| cb.get_text().map_err(|e| e.to_string()));
    match text {
        Ok(t) => {
            state.input.insert_str(&t);
            state.palette_dismissed = false;
        }
        Err(e) => state.notify(format!("Clipboard unavailable: {e}")),
    }
}

fn last_assistant_text(state: &AppState) -> Option<String> {
    state.items.iter().rev().find_map(|i| match i {
        ChatItem::Message(m) if m.role == MessageRole::Assistant => Some(m.content.clone()),
        _ => None,
    })
}

fn status_report(state: &AppState) -> String {
    let s = &state.settings;
    let provider_id = s.llm.provider.to_provider_id();
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "?".into());
    let key_line = match setup::active_key_env(s) {
        Some(env) => {
            let src = s
                .build_provider_registry()
                .active_config()
                .map(|c| c.api_key_source());
            let where_ = match src {
                Some(phazeai_core::llm::provider::ApiKeySource::Keyring) => "found in OS keyring",
                Some(phazeai_core::llm::provider::ApiKeySource::Env) => "found in environment",
                _ => "not set",
            };
            format!("{env} ({where_})")
        }
        None => "not needed (local)".into(),
    };
    let ctx = match state.context_tokens {
        Some(t) => format!(
            "~{} of {} token budget",
            format_tokens(t as u64),
            format_tokens(CONTEXT_BUDGET as u64)
        ),
        None => "—".into(),
    };
    let window = setup::model_info(&s.llm.provider, &s.llm.model)
        .map(|m| format!("{} tokens", format_tokens(m.context_window as u64)))
        .unwrap_or_else(|| "unknown".into());
    let rules = state.policy.lock().map(|p| p.len()).unwrap_or(0);
    format!(
        "Status\n\
         \n  Provider       {}\
         \n  Model          {}  (context window {window})\
         \n  Endpoint       {}\
         \n  API key        {key_line}\
         \n  Mode           {}\
         \n  Tool approval  {}{}\
         \n  Directory      {cwd}\
         \n  Git            {}\
         \n\
         \n  Tokens         {} in · {} out\
         \n  Est. cost      ${:.4}\
         \n  Context        {ctx}\
         \n  Conversation   {}",
        provider_id.name(),
        s.llm.model,
        setup::active_base_url(s),
        state.ai_mode,
        super::keys::approval_label(&state.approval_mode()),
        if rules > 0 {
            format!(" (+{rules} always-allow rules this session)")
        } else {
            String::new()
        },
        match &state.git_branch {
            Some(b) if state.git_dirty > 0 => format!("{b} ({} changed files)", state.git_dirty),
            Some(b) => format!("{b} (clean)"),
            None => "not a repository".into(),
        },
        format_tokens(state.tokens_in),
        format_tokens(state.tokens_out),
        state.cost,
        &state.conversation_id[..8.min(state.conversation_id.len())],
    )
}

pub(super) fn run_command(state: &mut AppState, result: CommandResult) {
    match result {
        CommandResult::NotACommand => {}
        CommandResult::Message(msg) => state.system(msg),
        CommandResult::Quit => state.should_quit = true,

        CommandResult::Clear => {
            if busy(state, "clear the conversation") {
                return;
            }
            if state.has_user_content() {
                state.save_conversation();
            }
            state.reset_conversation_view();
            state.conversation_id = ConversationStore::generate_id();
            let _ = state.worker_tx.send(WorkerCommand::Clear);
            state.needs_clear = true;
            state.notify("Conversation cleared — the model starts fresh");
        }
        CommandResult::NewConversation => {
            if busy(state, "start a new conversation") {
                return;
            }
            if state.has_user_content() {
                state.save_conversation();
            }
            state.reset_conversation_view();
            state.conversation_id = ConversationStore::generate_id();
            state.tokens_in = 0;
            state.tokens_out = 0;
            state.cost = 0.0;
            let _ = state.worker_tx.send(WorkerCommand::Clear);
            state.needs_clear = true;
            state.notify("New conversation started");
        }

        CommandResult::ModelChanged(model) => {
            if busy(state, "switch models") {
                return;
            }
            let mut next = state.settings.clone();
            next.llm.model = model.clone();
            match switch_llm(state, next) {
                Ok(()) => {
                    let known = setup::model_info(&state.settings.llm.provider, &model).is_some()
                        || ProviderRegistry::known_models(
                            &state.settings.llm.provider.to_provider_id(),
                        )
                        .is_empty();
                    let note = if known {
                        String::new()
                    } else {
                        "\nThis model isn't in the built-in list for this provider; if requests fail, check the name with /models.".to_string()
                    };
                    state.system(format!("Model set to {model} for this session.{note}"));
                }
                Err(msg) => state.error(msg),
            }
        }
        CommandResult::ProviderChanged(arg) => {
            if busy(state, "switch providers") {
                return;
            }
            let mut parts = arg.split_whitespace();
            let name = parts.next().unwrap_or("");
            let Some(provider) = setup::parse_provider(name) else {
                state.error(format!(
                    "Unknown provider '{name}'. Available: {}",
                    setup::PROVIDER_NAMES.join(", ")
                ));
                return;
            };
            let model = parts
                .next()
                .map(String::from)
                .unwrap_or_else(|| setup::default_model_for(&state.settings, &provider));
            let mut next = state.settings.clone();
            next.llm.provider = provider.clone();
            next.llm.model = model.clone();
            let current = state
                .settings
                .llm
                .provider
                .to_provider_id()
                .name()
                .to_string();
            match switch_llm(state, next) {
                Ok(()) => state.system(format!(
                    "Switched to {} · {model} for this session.",
                    provider.to_provider_id().name()
                )),
                Err(msg) => state.error(format!("{msg}\n\n(Still using {current}.)")),
            }
        }
        CommandResult::SetApiKey(key) => {
            let Some(env) = setup::active_key_env(&state.settings) else {
                state.system(format!(
                    "{} runs locally and doesn't need an API key.",
                    state.settings.llm.provider.to_provider_id().name()
                ));
                return;
            };
            let key = key.trim().to_string();
            if let Err(e) = phazeai_core::llm::provider::keyring_set(&env, &key) {
                state.error(format!(
                    "Couldn't save the key to the OS keyring ({e}).\n\
                     Set it as an environment variable instead:\n  \
                     macOS / Linux:  export {env}=...\n  \
                     Windows:        setx {env} \"...\""
                ));
                return;
            }
            let provider_name = state
                .settings
                .llm
                .provider
                .to_provider_id()
                .name()
                .to_string();
            let next = state.settings.clone();
            match switch_llm(state, next) {
                Ok(()) => state.system(format!(
                    "Saved your {provider_name} key to the OS keyring (entry \"{env}\"). You're ready to go."
                )),
                Err(msg) => state.error(msg),
            }
        }

        CommandResult::ThemeChanged(name) => {
            if !Theme::exists(&name) {
                state.error(format!(
                    "Unknown theme '{name}'. Available: {}",
                    Theme::all_names().join(", ")
                ));
                return;
            }
            state.theme = Theme::by_name(&name).with_mode(state.color_mode);
            state.theme_gen += 1;
            state.notify(format!("Theme: {}", state.theme.name));
        }
        CommandResult::ToggleFiles => state.show_files = !state.show_files,

        CommandResult::Copy { code_only } => {
            let Some(reply) = last_assistant_text(state) else {
                state.notify("Nothing to copy yet");
                return;
            };
            let text = if code_only {
                match crate::markdown::last_code_block(&reply) {
                    Some(code) => code,
                    None => {
                        state.notify("The last reply has no code block");
                        return;
                    }
                }
            } else {
                reply
            };
            let lines = text.lines().count();
            let res = clipboard(state).and_then(|cb| cb.set_text(text).map_err(|e| e.to_string()));
            match res {
                Ok(()) => state.notify(format!(
                    "Copied {lines} line{} to the clipboard",
                    if lines == 1 { "" } else { "s" }
                )),
                Err(e) => state.error(format!("Clipboard unavailable: {e}")),
            }
        }

        CommandResult::Compact => compact(state),
        CommandResult::SaveConversation => {
            state.save_conversation();
            state.notify(format!(
                "Saved conversation {}",
                &state.conversation_id[..8.min(state.conversation_id.len())]
            ));
        }
        CommandResult::LoadConversation(id) => {
            if busy(state, "load a conversation") {
                return;
            }
            match state.find_saved(&id) {
                Some(conv) => {
                    if state.has_user_content() {
                        state.save_conversation();
                    }
                    let restore = state.load_saved(conv);
                    let _ = state.worker_tx.send(WorkerCommand::Restore(restore));
                    state.needs_clear = true;
                }
                None => state.error(format!("No saved conversation matching '{id}'.")),
            }
        }
        CommandResult::ListConversations => match state.conversation_store.list_recent(20) {
            Ok(convs) if convs.is_empty() => state.system("No saved conversations."),
            Ok(convs) => {
                let mut list = String::from("Recent conversations\n");
                for c in &convs {
                    list.push_str(&format!(
                        "\n  {}  {}  ({} msgs, {})",
                        &c.id[..8.min(c.id.len())],
                        crate::text::truncate_to_width(&c.title, 50),
                        c.message_count,
                        c.updated_at,
                    ));
                }
                list.push_str("\n\nResume with /load <id> or browse with Ctrl+O.");
                state.system(list);
            }
            Err(e) => state.error(format!("Couldn't list conversations: {e}")),
        },

        CommandResult::SetApprovalMode(mode) => {
            let new_mode = match mode.as_str() {
                "auto" => ToolApprovalMode::AutoApprove,
                "ask" => ToolApprovalMode::AlwaysAsk,
                "ask-once" | "askonce" => ToolApprovalMode::AskOnce,
                _ => {
                    state.error("Invalid mode. Use: auto, ask, or ask-once");
                    return;
                }
            };
            if new_mode == ToolApprovalMode::AlwaysAsk {
                if let Ok(mut p) = state.policy.lock() {
                    p.clear();
                }
            }
            if let Ok(mut mgr) = state.approval_manager.lock() {
                mgr.set_mode(new_mode.clone());
            }
            let detail = match new_mode {
                ToolApprovalMode::AutoApprove => {
                    "every tool call now runs without asking. Use /approve ask to undo."
                }
                ToolApprovalMode::AlwaysAsk => {
                    "you'll be asked before every non-read-only tool call (session rules cleared)."
                }
                ToolApprovalMode::AskOnce => "you'll be asked once per distinct command / file.",
            };
            state.system(format!("Tool approval: {mode} — {detail}"));
        }
        CommandResult::ShowStatus => {
            let report = status_report(state);
            state.system(report);
        }

        CommandResult::ShowDiff => {
            let out = std::process::Command::new("git")
                .args(["diff", "--no-color"])
                .output();
            match out {
                Ok(o) if !o.status.success() => state.error(format!(
                    "git diff failed: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                )),
                Ok(o) => {
                    let diff = String::from_utf8_lossy(&o.stdout).to_string();
                    if diff.trim().is_empty() {
                        state.system("No unstaged changes.");
                        return;
                    }
                    let lines: Vec<&str> = diff.lines().collect();
                    let shown = lines.len().min(400);
                    let mut body = lines[..shown].join("\n");
                    if lines.len() > shown {
                        body.push_str(&format!(
                            "\n… {} more lines (run `git diff` for the rest)",
                            lines.len() - shown
                        ));
                    }
                    state.push(ChatItem::message(MessageRole::Diff, body));
                    state.follow = true;
                }
                Err(e) => state.error(format!("Couldn't run git: {e}")),
            }
        }
        CommandResult::ShowGitStatus => {
            let out = std::process::Command::new("git")
                .args(["status", "--short", "--branch"])
                .output();
            match out {
                Ok(o) if o.status.success() => {
                    let text = String::from_utf8_lossy(&o.stdout).trim_end().to_string();
                    let text = if text.lines().count() <= 1 {
                        format!("{text}\nWorking tree clean")
                    } else {
                        text
                    };
                    state.system(text);
                }
                Ok(o) => state.error(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                Err(e) => state.error(format!("Couldn't run git: {e}")),
            }
        }
        CommandResult::ShowLog => {
            let out = std::process::Command::new("git")
                .args(["log", "--oneline", "--decorate", "-20"])
                .output();
            match out {
                Ok(o) if o.status.success() => state.system(format!(
                    "Last 20 commits\n\n{}",
                    String::from_utf8_lossy(&o.stdout).trim_end()
                )),
                Ok(o) => state.error(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                Err(e) => state.error(format!("Couldn't run git: {e}")),
            }
        }
        CommandResult::SearchFiles(pattern) => search_files(state, &pattern),
        CommandResult::Grep(pattern) => grep(state, &pattern),
        CommandResult::ListModels => list_models(state),
        CommandResult::DiscoverModels => discover_models(state),
        CommandResult::ShowContext => show_context(state),

        CommandResult::SetMode(mode) => {
            let description = match mode.as_str() {
                "plan" => "creates structured plans and checklists, no code",
                "debug" => "diagnoses the root cause before fixing",
                "ask" => "read-only, answers questions about your code",
                "edit" => "makes minimal, targeted code changes",
                _ => "general conversation and assistance",
            };
            state.ai_mode = mode.clone();
            state.notify(format!("Mode: {mode} — {description}"));
        }
        CommandResult::AddFile(path_str) => {
            let path = std::path::Path::new(&path_str);
            match std::fs::read_to_string(path) {
                Ok(contents) => {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path_str.clone());
                    let lines = contents.lines().count();
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    let prompt = format!(
                        "Here are the contents of `{path_str}` for context. Reply with a one-line acknowledgement only.\n\n```{ext}\n{contents}\n```"
                    );
                    state.send_prompt(
                        format!("Added {name} to the context ({lines} lines)"),
                        prompt,
                    );
                }
                Err(e) => state.error(format!("Couldn't read '{path_str}': {e}")),
            }
        }
        CommandResult::Retry => {
            let last = state.last_user_input.clone();
            if last.is_empty() {
                state.notify("Nothing to retry yet");
            } else if !busy(state, "retry") {
                let agent_input = super::apply_mode_prefix(&state.ai_mode, &last);
                state.send_prompt(last, agent_input);
            }
        }
        CommandResult::Cancel => {
            if state.processing {
                state.cancel_run();
            } else {
                state.notify("Nothing is running");
            }
        }
        CommandResult::RunSkill { name, args } => run_skill(state, &name, &args),
        CommandResult::InstallGithubApp => install_github_action(state),
        CommandResult::UndoConfirmed => undo(state),
    }
}

fn compact(state: &mut AppState) {
    if busy(state, "compact") {
        return;
    }
    let conversational: Vec<&ChatMessage> = state
        .items
        .iter()
        .filter_map(|i| match i {
            ChatItem::Message(m)
                if matches!(m.role, MessageRole::User | MessageRole::Assistant) =>
            {
                Some(m)
            }
            _ => None,
        })
        .collect();
    if conversational.len() <= 6 {
        state.notify("Conversation is too short to compact");
        return;
    }
    let keep_from = conversational.len() - 4;
    let mut summary = String::from("Summary of the earlier conversation:\n");
    for m in &conversational[..keep_from] {
        let who = if m.role == MessageRole::User {
            "User"
        } else {
            "Assistant"
        };
        let first = m
            .content
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        summary.push_str(&format!(
            "- {who}: {}\n",
            crate::text::truncate_to_width(first.trim(), 160)
        ));
    }
    let mut restore = vec![("user".to_string(), summary.clone())];
    restore.push((
        "assistant".to_string(),
        "Understood — I have the summary of our earlier conversation.".to_string(),
    ));
    for m in &conversational[keep_from..] {
        restore.push((m.role.as_saved().to_string(), m.content.clone()));
    }
    let before = conversational.len();
    let _ = state.worker_tx.send(WorkerCommand::Restore(restore));
    state.context_tokens = None;
    state.system(format!(
        "Compacted the model's context: {before} messages → a summary plus the last 4. \
         Your scrollback is unchanged."
    ));
}

fn search_files(state: &mut AppState, pattern: &str) {
    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let matcher = match globset::GlobBuilder::new(pattern)
        .literal_separator(false)
        .build()
    {
        Ok(g) => g.compile_matcher(),
        Err(e) => {
            state.error(format!("Invalid glob '{pattern}': {e}"));
            return;
        }
    };
    let mut results = Vec::new();
    let mut truncated = false;
    for entry in ignore::WalkBuilder::new(&root)
        .hidden(true)
        .git_ignore(true)
        .build()
        .flatten()
    {
        if let Ok(rel) = entry.path().strip_prefix(&root) {
            if matcher.is_match(rel) {
                if results.len() >= 100 {
                    truncated = true;
                    break;
                }
                results.push(rel.display().to_string());
            }
        }
    }
    if results.is_empty() {
        state.system(format!("No files match '{pattern}'."));
    } else {
        let more = if truncated { " (first 100)" } else { "" };
        state.system(format!(
            "{} files match '{pattern}'{more}\n\n{}",
            results.len(),
            results.join("\n")
        ));
    }
}

fn grep(state: &mut AppState, pattern: &str) {
    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let output = std::process::Command::new("rg")
        .args([
            "--color=never",
            "--line-number",
            "--with-filename",
            "--",
            pattern,
        ])
        .current_dir(&root)
        .output()
        .or_else(|_| {
            std::process::Command::new("grep")
                .args([
                    "-rnI",
                    "--exclude-dir=.git",
                    "--exclude-dir=target",
                    "-e",
                    pattern,
                    ".",
                ])
                .current_dir(&root)
                .output()
        });
    match output {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let total = text.lines().count();
            if total == 0 {
                state.system(format!("No matches for '{pattern}'."));
            } else {
                let shown: Vec<&str> = text.lines().take(60).collect();
                let more = if total > 60 {
                    format!("\n… {} more matches", total - 60)
                } else {
                    String::new()
                };
                state.system(format!(
                    "{total} matches for '{pattern}'\n\n{}{more}",
                    shown.join("\n")
                ));
            }
        }
        Err(e) => state.error(format!(
            "Search failed (neither rg nor grep available): {e}"
        )),
    }
}

fn list_models(state: &mut AppState) {
    let provider = state.settings.llm.provider.to_provider_id();
    let models = ProviderRegistry::known_models(&provider);
    if models.is_empty() {
        state.system(format!(
            "{} has no built-in model list. Use /discover to see local models, then /model <name>.",
            provider.name()
        ));
        return;
    }
    let width = models.iter().map(|m| m.id.len()).max().unwrap_or(10) + 2;
    let mut msg = format!("Models for {}\n", provider.name());
    for m in &models {
        let active = if m.id == state.settings.llm.model {
            "  ← active"
        } else {
            ""
        };
        let price = if m.input_cost_per_m > 0.0 {
            format!("  ${}/{} per M", m.input_cost_per_m, m.output_cost_per_m)
        } else {
            String::new()
        };
        msg.push_str(&format!(
            "\n  {:<width$}{}  ctx {}{}{}{active}",
            m.id,
            m.name,
            format_tokens(m.context_window as u64),
            if m.supports_tools { "" } else { "  (no tools)" },
            price,
        ));
    }
    msg.push_str("\n\nSwitch with /model <id>.");
    state.system(msg);
}

fn discover_models(state: &mut AppState) {
    let mut msg = String::from("Local model discovery\n");
    match std::process::Command::new("ollama").arg("list").output() {
        Ok(output) if output.status.success() => {
            let list = String::from_utf8_lossy(&output.stdout);
            msg.push_str(&format!("\n  Ollama:\n{}\n", list.trim_end()));
        }
        _ => msg.push_str("\n  Ollama: not running or not installed (https://ollama.com)\n"),
    }
    let lm_port = phazeai_core::constants::endpoints::LMSTUDIO_PORT;
    let reachable = format!("127.0.0.1:{lm_port}")
        .parse::<std::net::SocketAddr>()
        .ok()
        .and_then(|addr| {
            std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500)).ok()
        })
        .is_some();
    if reachable {
        msg.push_str(&format!("  LM Studio: running on port {lm_port}\n"));
    } else {
        msg.push_str("  LM Studio: not detected\n");
    }
    msg.push_str("\nUse /provider ollama <model> (or lmstudio) to switch.");
    state.system(msg);
}

fn show_context(state: &mut AppState) {
    use phazeai_core::context::ProjectType;
    let root = std::env::current_dir().unwrap_or_default();
    let mut info = format!("Project context\n\n  Directory     {}\n", root.display());

    let candidates = [
        root.join("CLAUDE.md"),
        root.join(".phazeai/instructions.md"),
        root.join(".ai/instructions.md"),
    ];
    let mut found = false;
    for path in &candidates {
        if path.exists() {
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            info.push_str(&format!(
                "  Instructions  {} ({size} bytes)\n",
                path.display()
            ));
            found = true;
            break;
        }
    }
    if !found {
        let mut current = root.parent();
        while let Some(dir) = current {
            let p = dir.join("CLAUDE.md");
            if p.exists() {
                let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                info.push_str(&format!("  Instructions  {} ({size} bytes)\n", p.display()));
                break;
            }
            current = dir.parent();
        }
    }
    if let Some(home) = dirs::home_dir() {
        let global = home.join(".phazeai").join("instructions.md");
        if global.exists() {
            let size = std::fs::metadata(&global).map(|m| m.len()).unwrap_or(0);
            info.push_str(&format!(
                "  Global        {} ({size} bytes)\n",
                global.display()
            ));
        }
    }
    info.push_str(&format!(
        "  Project type  {}\n",
        ProjectType::detect(&root).name()
    ));
    let (branch, dirty) = phazeai_core::collect_git_info(&root);
    if let Some(b) = branch {
        info.push_str(&format!("  Git branch    {b}"));
        if !dirty.is_empty() {
            info.push_str(&format!(" ({} modified)", dirty.len()));
        }
    }
    state.system(info.trim_end().to_string());
}

fn run_skill(state: &mut AppState, name: &str, args: &str) {
    let pwd = std::env::current_dir().unwrap_or_default();
    let mut candidates = vec![pwd
        .join(".phazeai")
        .join("commands")
        .join(format!("{name}.md"))];
    if let Some(h) = dirs::home_dir() {
        candidates.push(
            h.join(".config")
                .join("phazeai")
                .join("commands")
                .join(format!("{name}.md")),
        );
        candidates.push(
            h.join(".phazeai")
                .join("commands")
                .join(format!("{name}.md")),
        );
    }
    let Some(path) = candidates.into_iter().find(|c| c.exists()) else {
        state.error(format!(
            "Skill '{name}' not found. Looked in .phazeai/commands/ and ~/.config/phazeai/commands/."
        ));
        return;
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let text = text.replace("$ARGS", args);
            state.last_user_input = text.clone();
            let agent_input = super::apply_mode_prefix(&state.ai_mode, &text);
            let shown = if args.is_empty() {
                format!("/skill {name}")
            } else {
                format!("/skill {name} {args}")
            };
            state.send_prompt(shown, agent_input);
        }
        Err(e) => state.error(format!("Couldn't read {}: {e}", path.display())),
    }
}

/// A workflow that answers `@phazeai` comments using this CLI, built from
/// `repo`. The agent runs with `--allow read`, so it can read the checkout
/// but not change files or run commands, and only trusted commenters can
/// trigger it (each run spends the repository's API credits).
pub(crate) fn github_action_workflow(repo: &str) -> String {
    format!(
        r#"name: PhazeAI

on:
  issue_comment:
    types: [created]
  pull_request_review_comment:
    types: [created]

jobs:
  answer:
    if: >-
      contains(github.event.comment.body, '@phazeai') &&
      contains(fromJSON('["OWNER", "MEMBER", "COLLABORATOR"]'), github.event.comment.author_association)
    runs-on: ubuntu-latest
    permissions:
      contents: read
      issues: write
      pull-requests: write
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: actions/cache@v4
        with:
          path: ~/.cargo
          key: phazeai-cli-${{{{ runner.os }}}}
      - name: Install system libraries
        run: sudo apt-get update && sudo apt-get install -y libdbus-1-dev pkg-config
      - name: Install the PhazeAI CLI
        run: cargo install --locked --git {repo} phazeai-cli
      - name: Ask PhazeAI (read-only)
        env:
          ANTHROPIC_API_KEY: ${{{{ secrets.ANTHROPIC_API_KEY }}}}
          COMMENT: ${{{{ github.event.comment.body }}}}
        run: phazeai --provider anthropic --allow read --no-color -p "$COMMENT" > reply.md
      - name: Post the answer
        env:
          GH_TOKEN: ${{{{ github.token }}}}
          NUMBER: ${{{{ github.event.issue.number || github.event.pull_request.number }}}}
        run: gh issue comment "$NUMBER" --repo "$GITHUB_REPOSITORY" --body-file reply.md
"#
    )
}

fn install_github_action(state: &mut AppState) {
    let workflow_dir = std::path::Path::new(".github/workflows");
    if let Err(e) = std::fs::create_dir_all(workflow_dir) {
        state.error(format!("Couldn't create .github/workflows: {e}"));
        return;
    }
    let workflow_path = workflow_dir.join("phazeai-action.yml");
    if workflow_path.exists() {
        state.error(format!(
            "{} already exists; not overwriting it.",
            workflow_path.display()
        ));
        return;
    }
    let workflow_content = github_action_workflow(env!("CARGO_PKG_REPOSITORY"));
    match std::fs::write(&workflow_path, workflow_content) {
        Ok(()) => state.system(format!(
            "Created {}. Add an ANTHROPIC_API_KEY repository secret (Settings → Secrets \
             and variables → Actions), then commit and push the file. Comment `@phazeai \
             <question>` on an issue or pull request to get an answer; only the repo's \
             owner, members and collaborators can trigger it, and it can only read code.",
            workflow_path.display()
        )),
        Err(e) => state.error(format!("Couldn't write the workflow file: {e}")),
    }
}

fn undo(state: &mut AppState) {
    if busy(state, "undo") {
        return;
    }
    let stat = std::process::Command::new("git")
        .args(["diff", "--stat", "HEAD"])
        .output();
    match stat {
        Ok(o) if !o.status.success() => state.error(format!(
            "git diff failed: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Ok(o) => {
            let diff_stat = String::from_utf8_lossy(&o.stdout).to_string();
            if diff_stat.trim().is_empty() {
                state.system("Nothing to undo — no uncommitted changes.");
                return;
            }
            match std::process::Command::new("git")
                .args(["checkout", "--", "."])
                .output()
            {
                Ok(r) if r.status.success() => {
                    state.system(format!("Reverted all uncommitted changes:\n{diff_stat}"));
                    state.refresh_git();
                }
                Ok(r) => state.error(format!(
                    "git checkout failed: {}",
                    String::from_utf8_lossy(&r.stderr).trim()
                )),
                Err(e) => state.error(format!("Couldn't run git: {e}")),
            }
        }
        Err(e) => state.error(format!("Git not available: {e}")),
    }
}

#[cfg(test)]
mod workflow_tests {
    use super::github_action_workflow;

    #[test]
    fn workflow_builds_this_cli_and_only_reads() {
        let text = github_action_workflow("https://github.com/example/repo");
        assert!(text
            .contains("cargo install --locked --git https://github.com/example/repo phazeai-cli"));
        assert!(text.contains("--allow read"));
        assert!(text.contains("contents: read"));
        assert!(text.contains("author_association"));
        assert!(text.contains("${{ secrets.ANTHROPIC_API_KEY }}"));
        assert!(text.contains("${{ github.event.comment.body }}"));
        // The comment reaches the shell only through an environment variable.
        assert!(!text.contains("-p \"${{"));
        assert!(!text.contains("phazeai-code-action"));
    }
}
