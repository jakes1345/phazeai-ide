use egui::{Key, Modifiers};

pub struct Keybinding {
    pub key: Key,
    pub modifiers: Modifiers,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Save,
    Open,
    NewFile,
    CloseTab,
    ToggleExplorer,
    ToggleChat,
    ToggleTerminal,
    FocusChat,
    CommandPalette,
    Undo,
    Redo,
    Find,
}

pub fn default_keybindings() -> Vec<Keybinding> {
    vec![
        Keybinding { key: Key::S, modifiers: Modifiers::COMMAND, action: Action::Save },
        Keybinding { key: Key::O, modifiers: Modifiers::COMMAND, action: Action::Open },
        Keybinding { key: Key::N, modifiers: Modifiers::COMMAND, action: Action::NewFile },
        Keybinding { key: Key::W, modifiers: Modifiers::COMMAND, action: Action::CloseTab },
        Keybinding { key: Key::E, modifiers: Modifiers::COMMAND, action: Action::ToggleExplorer },
        Keybinding { key: Key::J, modifiers: Modifiers::COMMAND, action: Action::ToggleChat },
        Keybinding { key: Key::Backtick, modifiers: Modifiers::COMMAND, action: Action::ToggleTerminal },
        Keybinding { key: Key::L, modifiers: Modifiers::COMMAND, action: Action::FocusChat },
        Keybinding { key: Key::P, modifiers: Modifiers::COMMAND.plus(Modifiers::SHIFT), action: Action::CommandPalette },
        Keybinding { key: Key::Z, modifiers: Modifiers::COMMAND, action: Action::Undo },
        Keybinding { key: Key::Z, modifiers: Modifiers::COMMAND.plus(Modifiers::SHIFT), action: Action::Redo },
        Keybinding { key: Key::F, modifiers: Modifiers::COMMAND, action: Action::Find },
    ]
}

pub fn check_keybindings(ctx: &egui::Context) -> Option<Action> {
    let bindings = default_keybindings();
    for binding in &bindings {
        if ctx.input(|i| i.key_pressed(binding.key) && i.modifiers == binding.modifiers) {
            return Some(binding.action.clone());
        }
    }
    None
}
