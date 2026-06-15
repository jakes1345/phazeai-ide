use crate::app::Tab;
use crate::theme::PhazeTheme;
use floem::reactive::RwSignal;

#[derive(Clone, Debug, PartialEq)]
pub struct SearchResult {
    pub path: std::path::PathBuf,
    pub line: usize,
    pub content: String,
}

#[derive(Clone)]
pub struct WorkbenchState {
    pub theme: RwSignal<PhazeTheme>,
    pub left_panel_tab: RwSignal<Tab>,
    pub bottom_panel_tab: RwSignal<Tab>,
    pub show_left_panel: RwSignal<bool>,
    pub show_right_panel: RwSignal<bool>,
    pub show_bottom_panel: RwSignal<bool>,
    pub left_panel_width: RwSignal<f64>,
    pub zen_mode: RwSignal<bool>,
    pub bottom_panel_maximized: RwSignal<bool>,
    pub panel_drag_active: RwSignal<bool>,
    pub panel_drag_start_x: RwSignal<f64>,
    pub command_palette_open: RwSignal<bool>,
    pub command_palette_query: RwSignal<String>,
    pub status_toast: RwSignal<Option<String>>,
    /// Set to "v0.X.Y" when a newer release is available. Drives the update banner.
    pub update_available: RwSignal<Option<String>>,
    pub file_picker_open: RwSignal<bool>,
    pub file_picker_query: RwSignal<String>,
    pub file_picker_files: RwSignal<Vec<std::path::PathBuf>>,
    pub search_query: RwSignal<String>,
    pub search_results: RwSignal<Vec<SearchResult>>,
    pub output_log: RwSignal<Vec<String>>,
    pub run_in_terminal_text: RwSignal<Option<String>>,
    /// Text shown in the Debug Console bottom tab (run/debug, Makefile, etc.).
    pub debug_console_log: RwSignal<String>,
    pub panel_drag_start_width: RwSignal<f64>,
    pub extensions: RwSignal<Vec<String>>,
    pub ext_loading: RwSignal<bool>,
    pub ext_manager: std::sync::Arc<std::sync::Mutex<phazeai_core::ext_host::ExtensionManager>>,
    /// Thread-safe view of the active editor for plugins. Populated from the
    /// open_file/active_text signals via create_effect; the plugin host reads
    /// from this snapshot off the UI thread.
    pub editor_snapshot: std::sync::Arc<phazeai_core::ext_host::EditorSnapshot>,
    /// Sender for plugin-originated editor mutations. The receiver is drained
    /// on the UI thread via `create_signal_from_channel` so all signal writes
    /// stay on-thread.
    pub editor_cmd_tx: std::sync::mpsc::SyncSender<crate::editor_command::EditorCommand>,
    /// Bumped by the command palette "New Terminal Tab" command.
    pub new_terminal_nonce: RwSignal<u64>,
}
