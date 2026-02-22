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
    ToggleBrowser,
    SelectAll,
    Copy,
    Cut,
    Paste,
    ToggleSearch,
    InlineChat,
    SelectNextOccurrence,
    FindReplace,
    ToggleGit,
    GotoDefinition,
    Completion,
    FindReferences,
    FormatDocument,
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
        Keybinding { key: Key::B, modifiers: Modifiers::COMMAND.plus(Modifiers::SHIFT), action: Action::ToggleBrowser },
        Keybinding { key: Key::A, modifiers: Modifiers::COMMAND, action: Action::SelectAll },
        Keybinding { key: Key::C, modifiers: Modifiers::COMMAND, action: Action::Copy },
        Keybinding { key: Key::X, modifiers: Modifiers::COMMAND, action: Action::Cut },
        Keybinding { key: Key::V, modifiers: Modifiers::COMMAND, action: Action::Paste },
        Keybinding { key: Key::F, modifiers: Modifiers::COMMAND.plus(Modifiers::SHIFT), action: Action::ToggleSearch },
        Keybinding { key: Key::K, modifiers: Modifiers::COMMAND, action: Action::InlineChat },
        Keybinding { key: Key::D, modifiers: Modifiers::COMMAND, action: Action::SelectNextOccurrence },
        Keybinding { key: Key::H, modifiers: Modifiers::COMMAND, action: Action::FindReplace },
        Keybinding { key: Key::G, modifiers: Modifiers::COMMAND.plus(Modifiers::SHIFT), action: Action::ToggleGit },
        Keybinding { key: Key::F12, modifiers: Modifiers::NONE, action: Action::GotoDefinition },
        Keybinding { key: Key::Space, modifiers: Modifiers::COMMAND, action: Action::Completion },
        Keybinding { key: Key::F12, modifiers: Modifiers::SHIFT, action: Action::FindReferences },
        Keybinding { key: Key::F, modifiers: Modifiers::ALT.plus(Modifiers::SHIFT), action: Action::FormatDocument },
    ]
}

impl Action {
    pub fn label(&self) -> &'static str {
        match self {
            Action::Save => "Save File",
            Action::Open => "Open File",
            Action::NewFile => "New File",
            Action::CloseTab => "Close Tab",
            Action::ToggleExplorer => "Toggle Explorer",
            Action::ToggleChat => "Toggle Chat",
            Action::ToggleTerminal => "Toggle Terminal",
            Action::FocusChat => "Focus Chat Input",
            Action::CommandPalette => "Command Palette",
            Action::Undo => "Undo",
            Action::Redo => "Redo",
            Action::Find => "Find in File",
            Action::ToggleBrowser => "Toggle Docs Viewer",
            Action::SelectAll => "Select All",
            Action::Copy => "Copy",
            Action::Cut => "Cut",
            Action::Paste => "Paste",
            Action::ToggleSearch => "Workspace Search",
            Action::InlineChat => "Inline AI Chat",
            Action::SelectNextOccurrence => "Select Next Occurrence",
            Action::FindReplace => "Find & Replace",
            Action::ToggleGit => "Toggle Git Panel",
            Action::GotoDefinition => "Go to Definition",
            Action::Completion => "Trigger Completion",
            Action::FindReferences => "Find References",
            Action::FormatDocument => "Format Document",
        }
    }
}

pub fn modifiers_label(mods: Modifiers) -> String {
    let mut parts = Vec::new();
    if mods.command { parts.push("Ctrl"); }
    if mods.shift { parts.push("Shift"); }
    if mods.alt { parts.push("Alt"); }
    parts.join("+")
}

pub fn key_label(key: Key) -> &'static str {
    match key {
        Key::A => "A", Key::B => "B", Key::C => "C", Key::D => "D",
        Key::E => "E", Key::F => "F", Key::G => "G", Key::H => "H",
        Key::I => "I", Key::J => "J", Key::K => "K", Key::L => "L",
        Key::M => "M", Key::N => "N", Key::O => "O", Key::P => "P",
        Key::Q => "Q", Key::R => "R", Key::S => "S", Key::T => "T",
        Key::U => "U", Key::V => "V", Key::W => "W", Key::X => "X",
        Key::Y => "Y", Key::Z => "Z",
        Key::F1 => "F1", Key::F2 => "F2", Key::F3 => "F3", Key::F4 => "F4",
        Key::F5 => "F5", Key::F6 => "F6", Key::F7 => "F7", Key::F8 => "F8",
        Key::F9 => "F9", Key::F10 => "F10", Key::F11 => "F11", Key::F12 => "F12",
        Key::Space => "Space", Key::Enter => "Enter", Key::Escape => "Escape",
        Key::Backspace => "Backspace", Key::Delete => "Delete",
        Key::Tab => "Tab",
        Key::Home => "Home", Key::End => "End",
        Key::PageUp => "PageUp", Key::PageDown => "PageDown",
        Key::ArrowLeft => "Left", Key::ArrowRight => "Right",
        Key::ArrowUp => "Up", Key::ArrowDown => "Down",
        Key::Backtick => "`",
        Key::Num0 => "0", Key::Num1 => "1", Key::Num2 => "2",
        Key::Num3 => "3", Key::Num4 => "4", Key::Num5 => "5",
        Key::Num6 => "6", Key::Num7 => "7", Key::Num8 => "8",
        Key::Num9 => "9",
        _ => "?",
    }
}

pub fn binding_label(b: &Keybinding) -> String {
    let mods = modifiers_label(b.modifiers);
    let key = key_label(b.key);
    if mods.is_empty() {
        key.to_string()
    } else {
        format!("{}+{}", mods, key)
    }
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
