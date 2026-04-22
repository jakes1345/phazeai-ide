use std::sync::Arc;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate};
use crate::domain_state::IdeState;

#[derive(Clone)]
pub struct Command {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub keybinding: Option<String>,
    pub action: Arc<dyn Fn(&IdeState) + Send + Sync>}

#[derive(Clone)]
pub struct GlobalCommandState {
    pub show_left_panel: RwSignal<bool>,
    pub left_panel_width: RwSignal<f64>,
    pub show_bottom_panel: RwSignal<bool>,
    pub show_right_panel: RwSignal<bool>,
    pub file_picker_open: RwSignal<bool>,
    pub file_picker_query: RwSignal<String>,
    pub command_palette_open: RwSignal<bool>,
    pub zen_mode: RwSignal<bool>,
    pub split_editor: RwSignal<bool>}

pub struct CommandRegistry {
    commands: Vec<Command>}

impl CommandRegistry {
    pub fn new() -> Self {
        Self {
            commands: Vec::new()}
    }

    pub fn register(&mut self, cmd: Command) {
        self.commands.push(cmd);
    }

    pub fn get_all(&self) -> &[Command] {
        &self.commands
    }

    pub fn execute(&self, id: &str, state: &IdeState) {
        if let Some(cmd) = self.commands.iter().find(|c| c.id == id) {
            (cmd.action)(state);
        }
    }
}

pub fn execute_command(id: &str, state: &IdeState) {
    // For now, this just uses a temporary registry or a global one
    // In a real app, this would use a global singleton registry
    let reg = create_default_commands();
    reg.execute(id, state);
}

pub fn match_global_shortcut(_key: &floem::keyboard::Key, _modifiers: &floem::keyboard::Modifiers) -> Option<String> {
    // Shortcut matching logic (mocked for now, will be wired to registry)
    None
}

/// Execute a global command using only `GlobalCommandState` signals.
/// Used in contexts where the full `IdeState` is not available (e.g. terminal panel).
pub fn execute_command_global(id: &str, state: &GlobalCommandState) {
    match id {
        "workbench.action.toggleSidebar" => {
            state.show_left_panel.update(|v| *v = !*v);
        }
        "workbench.action.toggleBottomPanel" => {
            state.show_bottom_panel.update(|v| *v = !*v);
        }
        "workbench.action.quickOpen" => {
            state.file_picker_open.set(true);
        }
        "workbench.action.showCommands" => {
            state.command_palette_open.set(true);
        }
        _ => {}
    }
}

pub fn create_default_commands() -> CommandRegistry {
    let mut reg = CommandRegistry::new();

    reg.register(Command {
        id: "workbench.action.toggleSidebar".into(),
        label: "Toggle Sidebar".into(),
        description: Some("Show or hide the primary sidebar".into()),
        keybinding: Some("Ctrl+B".into()),
        action: Arc::new(|s| {
            s.workbench.show_left_panel.update(|v| *v = !*v);
        })});

    reg.register(Command {
        id: "workbench.action.toggleBottomPanel".into(),
        label: "Toggle Bottom Panel".into(),
        description: Some("Show or hide the bottom panel".into()),
        keybinding: Some("Ctrl+J".into()),
        action: Arc::new(|s| {
            s.workbench.show_bottom_panel.update(|v| *v = !*v);
        })});

    reg.register(Command {
        id: "workbench.action.quickOpen".into(),
        label: "Go to File...".into(),
        description: Some("Quickly open files by name".into()),
        keybinding: Some("Ctrl+P".into()),
        action: Arc::new(|s| {
            s.workbench.file_picker_open.set(true);
        })});

    reg.register(Command {
        id: "workbench.action.showCommands".into(),
        label: "Show All Commands".into(),
        description: Some("Open the command palette".into()),
        keybinding: Some("Ctrl+Shift+P".into()),
        action: Arc::new(|s| {
            s.workbench.command_palette_open.set(true);
        })});

    reg
}
