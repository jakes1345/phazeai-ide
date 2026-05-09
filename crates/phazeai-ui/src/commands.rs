use crate::domain_state::IdeState;
use floem::keyboard::{Key, Modifiers};
use floem::reactive::{RwSignal, SignalUpdate};
use std::sync::Arc;

// ── Command definition ────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct Command {
    pub id: &'static str,
    pub label: &'static str,
    pub category: &'static str,
    pub description: &'static str,
    pub keybinding: Option<&'static str>,
    pub action: Arc<dyn Fn(&IdeState) + Send + Sync>,
}

// ── Global command state (for terminal panel etc.) ────────────────────────────

#[derive(Clone)]
pub struct GlobalCommandState {
    pub show_left_panel: RwSignal<bool>,
    pub left_panel_width: RwSignal<f64>,
    pub left_panel_tab: RwSignal<crate::app::Tab>,
    pub show_bottom_panel: RwSignal<bool>,
    pub show_right_panel: RwSignal<bool>,
    pub file_picker_open: RwSignal<bool>,
    pub file_picker_query: RwSignal<String>,
    pub command_palette_open: RwSignal<bool>,
    pub zen_mode: RwSignal<bool>,
    pub split_editor: RwSignal<bool>,
    pub font_size: RwSignal<u32>,
    pub ws_syms_open: RwSignal<bool>,
    pub ws_syms_query: RwSignal<String>,
    pub inlay_hints_toggle: RwSignal<bool>,
    pub code_lens_visible: RwSignal<bool>,
    pub minimap_visible: RwSignal<bool>,
    pub inline_edit_open: RwSignal<bool>,
}

// ── Command Registry ──────────────────────────────────────────────────────────

#[derive(Default)]
pub struct CommandRegistry {
    commands: Vec<Command>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self::default()
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

    /// Fuzzy search commands by label, category, or description.
    pub fn search(&self, query: &str) -> Vec<&Command> {
        if query.is_empty() {
            return self.commands.iter().collect();
        }
        let q = query.to_lowercase();
        let mut scored: Vec<(&Command, i32)> = self
            .commands
            .iter()
            .filter_map(|cmd| {
                let label = cmd.label.to_lowercase();
                let cat = cmd.category.to_lowercase();
                let desc = cmd.description.to_lowercase();
                let full = format!("{} {} {}", cat, label, desc);

                // Exact substring match in label = highest score
                if label.contains(&q) {
                    Some((cmd, 100))
                } else if cat.contains(&q) {
                    Some((cmd, 80))
                } else if desc.contains(&q) {
                    Some((cmd, 60))
                } else if fuzzy_match(&q, &full) {
                    Some((cmd, 40))
                } else {
                    None
                }
            })
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        scored.into_iter().map(|(cmd, _)| cmd).collect()
    }
}

/// Simple fuzzy match: all chars of needle appear in order within haystack.
fn fuzzy_match(needle: &str, haystack: &str) -> bool {
    let mut hay = haystack.chars();
    for n in needle.chars() {
        loop {
            match hay.next() {
                Some(h) if h == n => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

// ── Dispatch functions ────────────────────────────────────────────────────────

pub fn execute_command(id: &str, state: &IdeState) {
    let reg = create_default_commands();
    reg.execute(id, state);
}

/// Match a key event against all registered keybindings.
/// Returns the command ID if a match is found.
pub fn match_global_shortcut(key: &Key, modifiers: &Modifiers) -> Option<String> {
    let ctrl = modifiers.contains(Modifiers::CONTROL);
    let shift = modifiers.contains(Modifiers::SHIFT);
    let alt = modifiers.contains(Modifiers::ALT);

    match key {
        Key::Character(ref ch) => {
            let c = ch.to_lowercase();
            match c.as_str() {
                "b" if ctrl && !shift && !alt => Some("workbench.action.toggleSidebar".into()),
                "j" if ctrl && !shift && !alt => Some("workbench.action.toggleBottomPanel".into()),
                "p" if ctrl && !shift && !alt => Some("workbench.action.quickOpen".into()),
                "p" if ctrl && shift && !alt => Some("workbench.action.showCommands".into()),
                "\\" if ctrl && !shift && !alt => Some("workbench.action.toggleRightPanel".into()),
                "\\" if ctrl && alt && !shift => Some("editor.action.splitEditor".into()),
                "z" if ctrl && shift && !alt => Some("workbench.action.toggleZenMode".into()),
                "," if ctrl && !shift && !alt => Some("workbench.action.openSettings".into()),
                "t" if ctrl && !shift && !alt => Some("workbench.action.workspaceSymbols".into()),
                "e" if ctrl && shift && !alt => Some("workbench.action.focusExplorer".into()),
                "g" if ctrl && shift && !alt => Some("workbench.action.focusGit".into()),
                "f" if ctrl && shift && !alt => Some("workbench.action.findInFiles".into()),
                "i" if ctrl && shift && !alt => Some("editor.action.toggleInlayHints".into()),
                "l" if ctrl && shift && !alt => Some("editor.action.toggleCodeLens".into()),
                "m" if ctrl && shift && !alt => Some("editor.action.toggleMinimap".into()),
                "=" if ctrl && !shift && !alt => Some("editor.action.fontZoomIn".into()),
                "-" if ctrl && !shift && !alt => Some("editor.action.fontZoomOut".into()),
                "0" if ctrl && !shift && !alt => Some("editor.action.fontZoomReset".into()),
                "k" if ctrl && !shift && !alt => Some("editor.action.inlineEdit".into()),
                _ => None,
            }
        }
        _ => None,
    }
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
        "workbench.action.toggleRightPanel" => {
            state.show_right_panel.update(|v| *v = !*v);
        }
        "workbench.action.quickOpen" => {
            state.file_picker_open.set(true);
        }
        "workbench.action.showCommands" => {
            state.command_palette_open.set(true);
        }
        "workbench.action.toggleZenMode" => {
            state.zen_mode.update(|v| *v = !*v);
        }
        "editor.action.splitEditor" => {
            state.split_editor.update(|v| *v = !*v);
        }
        "workbench.action.openSettings" => {
            state.show_left_panel.set(true);
            state.left_panel_tab.set(crate::app::Tab::Settings);
        }
        "workbench.action.workspaceSymbols" => {
            state.ws_syms_open.set(true);
            state.ws_syms_query.set(String::new());
        }
        "workbench.action.focusExplorer" => {
            state.show_left_panel.set(true);
            state.left_panel_tab.set(crate::app::Tab::Explorer);
        }
        "workbench.action.focusGit" => {
            state.show_left_panel.set(true);
            state.left_panel_tab.set(crate::app::Tab::Git);
        }
        "workbench.action.findInFiles" => {
            state.show_left_panel.set(true);
            state.left_panel_tab.set(crate::app::Tab::Search);
        }
        "editor.action.toggleInlayHints" => {
            state.inlay_hints_toggle.update(|v| *v = !*v);
        }
        "editor.action.toggleCodeLens" => {
            state.code_lens_visible.update(|v| *v = !*v);
        }
        "editor.action.toggleMinimap" => {
            state.minimap_visible.update(|v| *v = !*v);
        }
        "editor.action.fontZoomIn" => {
            state.font_size.update(|v| *v = (*v + 1).min(32));
        }
        "editor.action.fontZoomOut" => {
            state.font_size.update(|v| *v = v.saturating_sub(1).max(8));
        }
        "editor.action.fontZoomReset" => {
            state.font_size.set(14);
        }
        "editor.action.inlineEdit" => {
            state.inline_edit_open.update(|v| *v = !*v);
        }
        _ => {}
    }
}

// ── Default command registry ──────────────────────────────────────────────────

pub fn create_default_commands() -> CommandRegistry {
    let mut reg = CommandRegistry::new();

    // ── Workbench: Layout ─────────────────────────────────────────────────

    reg.register(Command {
        id: "workbench.action.toggleSidebar",
        label: "Toggle Primary Sidebar",
        category: "View",
        description: "Show or hide the left sidebar (explorer, search, git)",
        keybinding: Some("Ctrl+B"),
        action: Arc::new(|s| {
            s.workbench.show_left_panel.update(|v| *v = !*v);
        }),
    });

    reg.register(Command {
        id: "workbench.action.toggleBottomPanel",
        label: "Toggle Bottom Panel",
        category: "View",
        description: "Show or hide the terminal / output panel",
        keybinding: Some("Ctrl+J"),
        action: Arc::new(|s| {
            s.workbench.show_bottom_panel.update(|v| *v = !*v);
        }),
    });

    reg.register(Command {
        id: "workbench.action.toggleRightPanel",
        label: "Toggle Right Panel",
        category: "View",
        description: "Show or hide the AI chat panel",
        keybinding: Some("Ctrl+Shift+\\"),
        action: Arc::new(|s| {
            s.workbench.show_right_panel.update(|v| *v = !*v);
        }),
    });

    reg.register(Command {
        id: "workbench.action.toggleZenMode",
        label: "Toggle Zen Mode",
        category: "View",
        description: "Hide all chrome for distraction-free editing",
        keybinding: Some("Ctrl+Shift+Z"),
        action: Arc::new(|s| {
            s.workbench.zen_mode.update(|v| *v = !*v);
        }),
    });

    reg.register(Command {
        id: "workbench.action.maximizeBottomPanel",
        label: "Toggle Maximize Bottom Panel",
        category: "View",
        description: "Maximize or restore the bottom panel height",
        keybinding: None,
        action: Arc::new(|s| {
            s.workbench.bottom_panel_maximized.update(|v| *v = !*v);
        }),
    });

    // ── Workbench: Navigation ─────────────────────────────────────────────

    reg.register(Command {
        id: "workbench.action.quickOpen",
        label: "Go to File…",
        category: "Navigation",
        description: "Quickly open files by name with fuzzy search",
        keybinding: Some("Ctrl+P"),
        action: Arc::new(|s| {
            s.workbench.file_picker_open.set(true);
        }),
    });

    reg.register(Command {
        id: "workbench.action.showCommands",
        label: "Show All Commands",
        category: "Navigation",
        description: "Open the command palette",
        keybinding: Some("Ctrl+Shift+P"),
        action: Arc::new(|s| {
            s.workbench.command_palette_open.set(true);
        }),
    });

    reg.register(Command {
        id: "workbench.action.workspaceSymbols",
        label: "Go to Symbol in Workspace…",
        category: "Navigation",
        description: "Search for symbols across all project files",
        keybinding: Some("Ctrl+T"),
        action: Arc::new(|s| {
            s.editor.ws_syms_open.set(true);
            s.editor.ws_syms_query.set(String::new());
        }),
    });

    reg.register(Command {
        id: "workbench.action.openSettings",
        label: "Open Settings",
        category: "Preferences",
        description: "Open the settings panel",
        keybinding: Some("Ctrl+,"),
        action: Arc::new(|s| {
            s.workbench.show_left_panel.set(true);
            s.workbench.left_panel_tab.set(crate::app::Tab::Settings);
        }),
    });

    reg.register(Command {
        id: "workbench.action.focusExplorer",
        label: "Focus File Explorer",
        category: "View",
        description: "Switch to the file explorer panel",
        keybinding: Some("Ctrl+Shift+E"),
        action: Arc::new(|s| {
            s.workbench.show_left_panel.set(true);
            s.workbench.left_panel_tab.set(crate::app::Tab::Explorer);
        }),
    });

    reg.register(Command {
        id: "workbench.action.focusGit",
        label: "Focus Source Control",
        category: "View",
        description: "Switch to the Git / Source Control panel",
        keybinding: Some("Ctrl+Shift+G"),
        action: Arc::new(|s| {
            s.workbench.show_left_panel.set(true);
            s.workbench.left_panel_tab.set(crate::app::Tab::Git);
        }),
    });

    reg.register(Command {
        id: "workbench.action.findInFiles",
        label: "Search: Find in Files",
        category: "Search",
        description: "Open the project-wide search panel",
        keybinding: Some("Ctrl+Shift+F"),
        action: Arc::new(|s| {
            s.workbench.show_left_panel.set(true);
            s.workbench.left_panel_tab.set(crate::app::Tab::Search);
        }),
    });

    // ── Editor: Splits ────────────────────────────────────────────────────

    reg.register(Command {
        id: "editor.action.splitEditor",
        label: "Split Editor Right",
        category: "Editor",
        description: "Open a second editor pane side-by-side",
        keybinding: Some("Ctrl+Alt+\\"),
        action: Arc::new(|s| {
            s.editor.split_editor.update(|v| *v = !*v);
        }),
    });

    reg.register(Command {
        id: "editor.action.splitEditorDown",
        label: "Split Editor Down",
        category: "Editor",
        description: "Open a second editor pane below",
        keybinding: None,
        action: Arc::new(|s| {
            s.editor.split_editor_down.update(|v| *v = !*v);
        }),
    });

    // ── Editor: Font ──────────────────────────────────────────────────────

    reg.register(Command {
        id: "editor.action.fontZoomIn",
        label: "Zoom In",
        category: "View",
        description: "Increase editor font size",
        keybinding: Some("Ctrl+="),
        action: Arc::new(|s| {
            s.editor.font_size.update(|v| *v = (*v + 1).min(32));
        }),
    });

    reg.register(Command {
        id: "editor.action.fontZoomOut",
        label: "Zoom Out",
        category: "View",
        description: "Decrease editor font size",
        keybinding: Some("Ctrl+-"),
        action: Arc::new(|s| {
            s.editor
                .font_size
                .update(|v| *v = v.saturating_sub(1).max(8));
        }),
    });

    reg.register(Command {
        id: "editor.action.fontZoomReset",
        label: "Reset Zoom",
        category: "View",
        description: "Reset editor font size to default",
        keybinding: Some("Ctrl+0"),
        action: Arc::new(|s| {
            s.editor.font_size.set(14);
        }),
    });

    // ── Editor: Toggles ───────────────────────────────────────────────────

    reg.register(Command {
        id: "editor.action.toggleWordWrap",
        label: "Toggle Word Wrap",
        category: "Editor",
        description: "Enable or disable word wrapping in the editor",
        keybinding: None,
        action: Arc::new(|s| {
            s.editor.word_wrap.update(|v| *v = !*v);
        }),
    });

    reg.register(Command {
        id: "editor.action.toggleVimMode",
        label: "Toggle Vim Mode",
        category: "Editor",
        description: "Enable or disable Vim keybindings",
        keybinding: None,
        action: Arc::new(|s| {
            s.editor.vim_mode.update(|v| *v = !*v);
        }),
    });

    reg.register(Command {
        id: "editor.action.toggleRelativeLineNumbers",
        label: "Toggle Relative Line Numbers",
        category: "Editor",
        description: "Switch between absolute and relative line numbers",
        keybinding: None,
        action: Arc::new(|s| {
            s.editor.relative_line_numbers.update(|v| *v = !*v);
        }),
    });

    reg.register(Command {
        id: "editor.action.toggleInlayHints",
        label: "Toggle Inlay Hints",
        category: "Editor",
        description: "Show or hide LSP inlay type/parameter hints",
        keybinding: Some("Ctrl+Shift+I"),
        action: Arc::new(|s| {
            s.editor.inlay_hints_toggle.update(|v| *v = !*v);
        }),
    });

    reg.register(Command {
        id: "editor.action.toggleCodeLens",
        label: "Toggle Code Lens",
        category: "Editor",
        description: "Show or hide code lens annotations",
        keybinding: Some("Ctrl+Shift+L"),
        action: Arc::new(|s| {
            s.editor.code_lens_visible.update(|v| *v = !*v);
        }),
    });

    reg.register(Command {
        id: "editor.action.toggleMinimap",
        label: "Toggle Minimap",
        category: "Editor",
        description: "Show or hide the editor minimap strip",
        keybinding: Some("Ctrl+Shift+M"),
        action: Arc::new(|s| {
            s.editor.minimap_visible.update(|v| *v = !*v);
        }),
    });

    reg.register(Command {
        id: "editor.action.toggleAutoSave",
        label: "Toggle Auto Save",
        category: "Editor",
        description: "Enable or disable automatic file saving",
        keybinding: None,
        action: Arc::new(|s| {
            s.editor.auto_save.update(|v| *v = !*v);
        }),
    });

    // ── Editor: Code Actions ──────────────────────────────────────────────

    reg.register(Command {
        id: "editor.action.commentLine",
        label: "Toggle Line Comment",
        category: "Editor",
        description: "Comment or uncomment the current line",
        keybinding: Some("Ctrl+/"),
        action: Arc::new(|s| {
            s.editor.comment_toggle_nonce.update(|v| *v += 1);
        }),
    });

    reg.register(Command {
        id: "editor.action.foldCode",
        label: "Fold Code Block",
        category: "Editor",
        description: "Collapse the current code block",
        keybinding: Some("Ctrl+Shift+["),
        action: Arc::new(|s| {
            s.editor.fold_nonce.update(|v| *v += 1);
        }),
    });

    reg.register(Command {
        id: "editor.action.unfoldCode",
        label: "Unfold Code Block",
        category: "Editor",
        description: "Expand the current collapsed code block",
        keybinding: Some("Ctrl+Shift+]"),
        action: Arc::new(|s| {
            s.editor.unfold_nonce.update(|v| *v += 1);
        }),
    });

    reg.register(Command {
        id: "editor.action.foldAll",
        label: "Fold All",
        category: "Editor",
        description: "Collapse all foldable regions in the file",
        keybinding: None,
        action: Arc::new(|s| {
            s.editor.fold_all_nonce.update(|v| *v += 1);
        }),
    });

    reg.register(Command {
        id: "editor.action.unfoldAll",
        label: "Unfold All",
        category: "Editor",
        description: "Expand all collapsed regions in the file",
        keybinding: None,
        action: Arc::new(|s| {
            s.editor.unfold_all_nonce.update(|v| *v += 1);
        }),
    });

    reg.register(Command {
        id: "editor.action.transformToUppercase",
        label: "Transform to Uppercase",
        category: "Editor",
        description: "Convert selected text to UPPERCASE",
        keybinding: None,
        action: Arc::new(|s| {
            s.editor.transform_upper_nonce.update(|v| *v += 1);
        }),
    });

    reg.register(Command {
        id: "editor.action.transformToLowercase",
        label: "Transform to Lowercase",
        category: "Editor",
        description: "Convert selected text to lowercase",
        keybinding: None,
        action: Arc::new(|s| {
            s.editor.transform_lower_nonce.update(|v| *v += 1);
        }),
    });

    reg.register(Command {
        id: "editor.action.transformToTitleCase",
        label: "Transform to Title Case",
        category: "Editor",
        description: "Convert selected text to Title Case",
        keybinding: None,
        action: Arc::new(|s| {
            s.editor.transform_title_nonce.update(|v| *v += 1);
        }),
    });

    reg.register(Command {
        id: "editor.action.joinLines",
        label: "Join Lines",
        category: "Editor",
        description: "Join the selected lines into one",
        keybinding: None,
        action: Arc::new(|s| {
            s.editor.join_line_nonce.update(|v| *v += 1);
        }),
    });

    reg.register(Command {
        id: "editor.action.sortLines",
        label: "Sort Lines Ascending",
        category: "Editor",
        description: "Sort the selected lines alphabetically",
        keybinding: None,
        action: Arc::new(|s| {
            s.editor.sort_lines_nonce.update(|v| *v += 1);
        }),
    });

    // ── AI: Actions ───────────────────────────────────────────────────────

    reg.register(Command {
        id: "editor.action.inlineEdit",
        label: "AI Inline Edit",
        category: "AI",
        description: "Open inline AI edit prompt at cursor",
        keybinding: Some("Ctrl+K"),
        action: Arc::new(|s| {
            s.ai.inline_edit_open.update(|v| *v = !*v);
        }),
    });

    reg.register(Command {
        id: "ai.action.openChat",
        label: "Open AI Chat",
        category: "AI",
        description: "Open the AI chat panel",
        keybinding: None,
        action: Arc::new(|s| {
            s.workbench.show_right_panel.set(true);
        }),
    });

    // ── Terminal ───────────────────────────────────────────────────────────

    reg.register(Command {
        id: "workbench.action.terminal.focus",
        label: "Focus Terminal",
        category: "Terminal",
        description: "Show and focus the integrated terminal",
        keybinding: None,
        action: Arc::new(|s| {
            s.workbench.show_bottom_panel.set(true);
            s.workbench.bottom_panel_tab.set(crate::app::Tab::Terminal);
        }),
    });

    // ── Theme ─────────────────────────────────────────────────────────────

    reg.register(Command {
        id: "workbench.action.selectTheme",
        label: "Color Theme",
        category: "Preferences",
        description: "Browse and select a color theme",
        keybinding: None,
        action: Arc::new(|s| {
            s.workbench.show_left_panel.set(true);
            s.workbench.left_panel_tab.set(crate::app::Tab::Settings);
        }),
    });

    reg
}
