use crate::app::VimMotion;
use crate::lsp_bridge::{
    CodeAction, CodeLensEntry, CompletionEntry, DefinitionResult, DiagEntry, InlayHintEntry,
    LspBridgeSignals, ReferenceEntry, SymbolEntry,
};
use crate::theme::PhazeTheme;
use floem::reactive::{create_effect, create_rw_signal, RwSignal, SignalGet};
use phazeai_core::config::EditorSettings;
use std::path::PathBuf;

/// The `[editor]` settings of `config.toml`, as the signals the IDE reads.
///
/// [`EditorState::new`] puts these same signals in the editor state and
/// [`EditorPrefs::persist`] saves every change to them, so the file's values
/// apply at start and a toggle made anywhere (Settings, the command palette,
/// a shortcut) is the value that gets saved.
#[derive(Clone, Copy)]
pub struct EditorPrefs {
    pub font_size: RwSignal<u32>,
    pub tab_size: RwSignal<u32>,
    pub auto_save: RwSignal<bool>,
    pub word_wrap: RwSignal<bool>,
    pub relative_line_numbers: RwSignal<bool>,
    pub inlay_hints: RwSignal<bool>,
    pub code_lens: RwSignal<bool>,
    pub organize_imports_on_save: RwSignal<bool>,
    pub ai_completions: RwSignal<bool>,
}

impl EditorPrefs {
    /// Signals holding `cfg`'s values.
    pub fn new(cfg: &EditorSettings) -> Self {
        Self {
            font_size: create_rw_signal(cfg.font_size as u32),
            tab_size: create_rw_signal(cfg.tab_size),
            auto_save: create_rw_signal(cfg.auto_save),
            word_wrap: create_rw_signal(cfg.word_wrap),
            relative_line_numbers: create_rw_signal(cfg.relative_line_numbers),
            inlay_hints: create_rw_signal(cfg.inlay_hints),
            code_lens: create_rw_signal(cfg.code_lens),
            organize_imports_on_save: create_rw_signal(cfg.organize_imports_on_save),
            ai_completions: create_rw_signal(cfg.ai_completions),
        }
    }

    /// The current values. The reads are tracked, so an effect that calls
    /// this re-runs on every change.
    fn values(&self, theme: RwSignal<PhazeTheme>) -> PrefValues {
        PrefValues {
            theme: theme.get().display_name().to_string(),
            font_size: self.font_size.get(),
            tab_size: self.tab_size.get(),
            auto_save: self.auto_save.get(),
            word_wrap: self.word_wrap.get(),
            relative_line_numbers: self.relative_line_numbers.get(),
            inlay_hints: self.inlay_hints.get(),
            code_lens: self.code_lens.get(),
            organize_imports_on_save: self.organize_imports_on_save.get(),
            ai_completions: self.ai_completions.get(),
        }
    }

    /// After every change to these settings or to `theme`, hand `save` an
    /// edit of the `[editor]` table that writes only the values that changed.
    /// `save` applies it to the file as loaded (load-modify-save), so every
    /// other key keeps the value the file has, even one changed there since
    /// start (by hand, or by another PhazeAI window): a value this window
    /// still holds from start is never written back over it. Nothing is
    /// saved at start, when the values have just been read.
    pub fn persist(
        self,
        theme: RwSignal<PhazeTheme>,
        save: impl Fn(Box<dyn FnOnce(&mut EditorSettings) + Send>) + 'static,
    ) {
        create_effect(move |before: Option<PrefValues>| {
            let now = self.values(theme);
            if let Some(before) = before.filter(|before| *before != now) {
                let after = now.clone();
                save(Box::new(move |e| after.write_changes(&before, e)));
            }
            now
        });
    }
}

/// The values [`EditorPrefs::persist`] saves, as they were at one moment.
#[derive(Clone, PartialEq)]
struct PrefValues {
    theme: String,
    font_size: u32,
    tab_size: u32,
    auto_save: bool,
    word_wrap: bool,
    relative_line_numbers: bool,
    inlay_hints: bool,
    code_lens: bool,
    organize_imports_on_save: bool,
    ai_completions: bool,
}

impl PrefValues {
    /// Write to `e` the values that differ from `before`, and nothing else.
    fn write_changes(&self, before: &Self, e: &mut EditorSettings) {
        if self.theme != before.theme {
            e.theme = self.theme.clone();
        }
        if self.font_size != before.font_size {
            e.font_size = self.font_size as f32;
        }
        if self.tab_size != before.tab_size {
            e.tab_size = self.tab_size;
        }
        if self.auto_save != before.auto_save {
            e.auto_save = self.auto_save;
        }
        if self.word_wrap != before.word_wrap {
            e.word_wrap = self.word_wrap;
        }
        if self.relative_line_numbers != before.relative_line_numbers {
            e.relative_line_numbers = self.relative_line_numbers;
        }
        if self.inlay_hints != before.inlay_hints {
            e.inlay_hints = self.inlay_hints;
        }
        if self.code_lens != before.code_lens {
            e.code_lens = self.code_lens;
        }
        if self.organize_imports_on_save != before.organize_imports_on_save {
            e.organize_imports_on_save = self.organize_imports_on_save;
        }
        if self.ai_completions != before.ai_completions {
            e.ai_completions = self.ai_completions;
        }
    }
}

#[derive(Clone)]
pub struct EditorState {
    pub open_file: RwSignal<Option<PathBuf>>,
    pub open_tabs: RwSignal<Vec<PathBuf>>,
    pub active_cursor: RwSignal<Option<(PathBuf, u32, u32)>>,
    pub vim_mode: RwSignal<bool>,
    pub vim_normal_mode: RwSignal<bool>,
    pub vim_pending_key: RwSignal<Option<char>>,
    pub vim_motion: RwSignal<Option<VimMotion>>,
    pub vim_visual_mode: RwSignal<bool>,
    pub vim_visual_line: RwSignal<bool>,
    pub vim_marks: RwSignal<std::collections::HashMap<char, (PathBuf, usize)>>,
    pub vim_last_motion: RwSignal<Option<VimMotion>>,
    pub vim_ex_open: RwSignal<bool>,
    pub vim_ex_input: RwSignal<String>,
    pub font_size: RwSignal<u32>,
    pub tab_size: RwSignal<u32>,
    pub auto_save: RwSignal<bool>,
    pub word_wrap: RwSignal<bool>,
    pub relative_line_numbers: RwSignal<bool>,
    pub line_ending: RwSignal<&'static str>,
    pub active_readonly: RwSignal<bool>,

    // LSP & Intelligence
    pub diagnostics: RwSignal<Vec<DiagEntry>>,
    pub completions: RwSignal<Vec<CompletionEntry>>,
    pub completion_open: RwSignal<bool>,
    pub completion_selected: RwSignal<usize>,
    pub completion_filter_text: RwSignal<String>,
    pub hover_text: RwSignal<Option<String>>,
    pub goto_line: RwSignal<u32>,
    pub goto_definition: RwSignal<Option<DefinitionResult>>,
    pub references: RwSignal<Vec<ReferenceEntry>>,
    pub references_visible: RwSignal<bool>,
    pub code_actions: RwSignal<Vec<CodeAction>>,
    pub code_actions_open: RwSignal<bool>,
    pub doc_symbols: RwSignal<Vec<SymbolEntry>>,
    pub inlay_hints: RwSignal<Vec<InlayHintEntry>>,
    pub code_lens: RwSignal<Vec<CodeLensEntry>>,
    pub folding_ranges: RwSignal<Vec<(u32, u32)>>,

    // Nonces & Actions
    pub comment_toggle_nonce: RwSignal<u64>,
    pub ctrl_d_nonce: RwSignal<u64>,
    pub fold_nonce: RwSignal<u64>,
    pub unfold_nonce: RwSignal<u64>,
    pub fold_all_nonce: RwSignal<u64>,
    pub unfold_all_nonce: RwSignal<u64>,
    pub move_line_up_nonce: RwSignal<u64>,
    pub move_line_down_nonce: RwSignal<u64>,
    pub duplicate_line_nonce: RwSignal<u64>,
    pub delete_line_nonce: RwSignal<u64>,
    pub transform_upper_nonce: RwSignal<u64>,
    pub transform_lower_nonce: RwSignal<u64>,
    pub transform_title_nonce: RwSignal<u64>,
    pub join_line_nonce: RwSignal<u64>,
    pub sort_lines_nonce: RwSignal<u64>,
    pub format_selection_nonce: RwSignal<u64>,
    pub split_editor: RwSignal<bool>,
    pub active_blame: RwSignal<String>,
    pub pending_completion: RwSignal<Option<(String, usize)>>,
    pub yank_ring: RwSignal<Vec<String>>,
    pub yank_ring_idx: RwSignal<usize>,
    pub split_editor_down: RwSignal<bool>,

    // LSP & Overlays
    pub rename_open: RwSignal<bool>,
    pub rename_query: RwSignal<String>,
    pub rename_target: RwSignal<String>,
    pub sig_help: RwSignal<Option<crate::lsp_bridge::SignatureHelpResult>>,
    pub ws_syms_open: RwSignal<bool>,
    pub ws_syms_query: RwSignal<String>,
    pub workspace_symbols: RwSignal<Vec<crate::lsp_bridge::SymbolEntry>>,
    pub goto_overlay_open: RwSignal<bool>,
    pub goto_overlay_input: RwSignal<String>,
    pub peek_def_open: RwSignal<bool>,
    pub peek_def_lines: RwSignal<Vec<String>>,

    // Nonces & Toggles
    pub col_cursor_up_nonce: RwSignal<u64>,
    pub col_cursor_down_nonce: RwSignal<u64>,
    pub sticky_lines: RwSignal<Vec<String>>,
    pub expand_selection_nonce: RwSignal<u64>,
    pub shrink_selection_nonce: RwSignal<u64>,
    pub save_no_format_nonce: RwSignal<u64>,
    pub code_lens_visible: RwSignal<bool>,
    pub organize_imports_on_save: RwSignal<bool>,
    pub inlay_hints_sig: RwSignal<Vec<crate::lsp_bridge::InlayHintEntry>>,
    pub inlay_hints_toggle: RwSignal<bool>,
    /// `[editor] ai_completions`: while false, the editor requests no inline
    /// AI completions (nothing is sent).
    pub ai_completions: RwSignal<bool>,
    pub minimap_visible: RwSignal<bool>,

    // Split View State
    pub split_open_file: RwSignal<Option<PathBuf>>,
    pub split_active_cursor: RwSignal<Option<(PathBuf, u32, u32)>>,
    pub split_open_tabs: RwSignal<Vec<PathBuf>>,
    pub split_down_file: RwSignal<Option<PathBuf>>,
    pub split_down_cursor: RwSignal<Option<(PathBuf, u32, u32)>>,
    pub split_down_tabs: RwSignal<Vec<PathBuf>>,

    // Bumped to request closing the currently active tab (Ctrl+W).
    pub close_active_tab_nonce: RwSignal<u64>,

    // Text currently selected in the active editor ("" when no selection).
    pub selected_text: RwSignal<String>,

    // Last known pointer position (window-relative). Written by PointerMove on the
    // root stack; read by the hover tooltip overlay to position itself near the cursor.
    pub hover_anchor_x: RwSignal<f64>,
    pub hover_anchor_y: RwSignal<f64>,
}

impl EditorState {
    /// The editor state at start. Its settings fields are `prefs`' own
    /// signals, not copies, so they hold the values from `config.toml` and
    /// every change to them is saved (see [`EditorPrefs::persist`]). `lsp`
    /// supplies the language-server results; the other signals were made by
    /// the caller, which wires effects to them first.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        prefs: EditorPrefs,
        lsp: &LspBridgeSignals,
        open_file: RwSignal<Option<PathBuf>>,
        open_tabs: RwSignal<Vec<PathBuf>>,
        vim_mode: RwSignal<bool>,
        goto_line: RwSignal<u32>,
        line_ending: RwSignal<&'static str>,
        active_readonly: RwSignal<bool>,
    ) -> Self {
        Self {
            open_file,
            open_tabs,
            active_cursor: create_rw_signal(None),
            vim_mode,
            vim_normal_mode: create_rw_signal(false),
            vim_pending_key: create_rw_signal(None),
            vim_motion: create_rw_signal(None),
            vim_visual_mode: create_rw_signal(false),
            vim_visual_line: create_rw_signal(false),
            vim_marks: create_rw_signal(std::collections::HashMap::new()),
            vim_last_motion: create_rw_signal(None),
            vim_ex_open: create_rw_signal(false),
            vim_ex_input: create_rw_signal(String::new()),
            font_size: prefs.font_size,
            tab_size: prefs.tab_size,
            auto_save: prefs.auto_save,
            word_wrap: prefs.word_wrap,
            relative_line_numbers: prefs.relative_line_numbers,
            line_ending,
            active_readonly,
            diagnostics: lsp.diagnostics,
            completions: lsp.completions,
            completion_open: create_rw_signal(false),
            completion_selected: create_rw_signal(0),
            completion_filter_text: create_rw_signal(String::new()),
            hover_text: lsp.hover_text,
            goto_line,
            goto_definition: lsp.goto_definition,
            references: lsp.references,
            references_visible: create_rw_signal(false),
            code_actions: lsp.code_actions,
            code_actions_open: create_rw_signal(false),
            doc_symbols: lsp.doc_symbols,
            inlay_hints: lsp.inlay_hints,
            code_lens: lsp.code_lens,
            folding_ranges: lsp.folding_ranges,
            comment_toggle_nonce: create_rw_signal(0),
            ctrl_d_nonce: create_rw_signal(0),
            fold_nonce: create_rw_signal(0),
            unfold_nonce: create_rw_signal(0),
            fold_all_nonce: create_rw_signal(0),
            unfold_all_nonce: create_rw_signal(0),
            move_line_up_nonce: create_rw_signal(0),
            move_line_down_nonce: create_rw_signal(0),
            duplicate_line_nonce: create_rw_signal(0),
            delete_line_nonce: create_rw_signal(0),
            transform_upper_nonce: create_rw_signal(0),
            transform_lower_nonce: create_rw_signal(0),
            transform_title_nonce: create_rw_signal(0),
            join_line_nonce: create_rw_signal(0),
            sort_lines_nonce: create_rw_signal(0),
            format_selection_nonce: create_rw_signal(0),
            split_editor: create_rw_signal(false),
            active_blame: create_rw_signal(String::new()),
            pending_completion: create_rw_signal(None),
            yank_ring: create_rw_signal(Vec::new()),
            yank_ring_idx: create_rw_signal(0),
            split_editor_down: create_rw_signal(false),
            rename_open: create_rw_signal(false),
            rename_query: create_rw_signal(String::new()),
            rename_target: create_rw_signal(String::new()),
            sig_help: lsp.sig_help,
            ws_syms_open: create_rw_signal(false),
            ws_syms_query: create_rw_signal(String::new()),
            workspace_symbols: create_rw_signal(Vec::new()),
            goto_overlay_open: create_rw_signal(false),
            goto_overlay_input: create_rw_signal(String::new()),
            peek_def_open: create_rw_signal(false),
            peek_def_lines: create_rw_signal(Vec::new()),
            col_cursor_up_nonce: create_rw_signal(0),
            col_cursor_down_nonce: create_rw_signal(0),
            sticky_lines: create_rw_signal(Vec::new()),
            expand_selection_nonce: create_rw_signal(0),
            shrink_selection_nonce: create_rw_signal(0),
            save_no_format_nonce: create_rw_signal(0),
            code_lens_visible: prefs.code_lens,
            organize_imports_on_save: prefs.organize_imports_on_save,
            inlay_hints_sig: create_rw_signal(Vec::new()),
            inlay_hints_toggle: prefs.inlay_hints,
            ai_completions: prefs.ai_completions,
            minimap_visible: create_rw_signal(true),
            split_open_file: create_rw_signal(None),
            split_active_cursor: create_rw_signal(None),
            split_open_tabs: create_rw_signal(Vec::new()),
            split_down_file: create_rw_signal(None),
            split_down_cursor: create_rw_signal(None),
            split_down_tabs: create_rw_signal(Vec::new()),
            close_active_tab_nonce: create_rw_signal(0u64),
            selected_text: create_rw_signal(String::new()),
            hover_anchor_x: create_rw_signal(0.0_f64),
            hover_anchor_y: create_rw_signal(0.0_f64),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeVariant;
    use floem::reactive::SignalUpdate;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    /// Language-server signals with no server behind them.
    fn detached_lsp() -> LspBridgeSignals {
        let (cmd_tx, _) = tokio::sync::mpsc::unbounded_channel();
        LspBridgeSignals {
            cmd_tx,
            diagnostics: create_rw_signal(Vec::new()),
            completions: create_rw_signal(Vec::new()),
            goto_definition: create_rw_signal(None),
            hover_text: create_rw_signal(None),
            references: create_rw_signal(Vec::new()),
            code_actions: create_rw_signal(Vec::new()),
            sig_help: create_rw_signal(None),
            doc_symbols: create_rw_signal(Vec::new()),
            workspace_symbols: create_rw_signal(Vec::new()),
            lsp_progress: create_rw_signal(None),
            peek_def_lines: create_rw_signal(Vec::new()),
            code_lens: create_rw_signal(Vec::new()),
            folding_ranges: create_rw_signal(Vec::new()),
            inlay_hints: create_rw_signal(Vec::new()),
        }
    }

    /// The editor state `IdeState::new` builds from `cfg` (the `[editor]`
    /// table of config.toml), and the settings signals it was built from.
    fn editor_from(cfg: &EditorSettings) -> (EditorPrefs, EditorState) {
        let prefs = EditorPrefs::new(cfg);
        let editor = EditorState::new(
            prefs,
            &detached_lsp(),
            create_rw_signal(None),
            create_rw_signal(Vec::new()),
            create_rw_signal(false),
            create_rw_signal(0),
            create_rw_signal("LF"),
            create_rw_signal(false),
        );
        (prefs, editor)
    }

    fn toggles(organize: bool, inlay: bool, lens: bool, ai: bool) -> EditorSettings {
        EditorSettings {
            organize_imports_on_save: organize,
            inlay_hints: inlay,
            code_lens: lens,
            ai_completions: ai,
            ..EditorSettings::default()
        }
    }

    #[test]
    fn the_editor_state_starts_with_the_values_from_config_toml() {
        let (_, editor) = editor_from(&toggles(false, false, false, false));
        assert!(!editor.organize_imports_on_save.get_untracked());
        assert!(!editor.inlay_hints_toggle.get_untracked());
        assert!(!editor.code_lens_visible.get_untracked());
        assert!(!editor.ai_completions.get_untracked());

        // And the other way round, so no hard-coded value passes both.
        let (_, editor) = editor_from(&toggles(true, true, true, true));
        assert!(editor.organize_imports_on_save.get_untracked());
        assert!(editor.inlay_hints_toggle.get_untracked());
        assert!(editor.code_lens_visible.get_untracked());
        assert!(editor.ai_completions.get_untracked());

        // A fresh config: organize imports off, the rest on.
        let (_, editor) = editor_from(&EditorSettings::default());
        assert!(!editor.organize_imports_on_save.get_untracked());
        assert!(editor.inlay_hints_toggle.get_untracked());
        assert!(editor.code_lens_visible.get_untracked());
        assert!(editor.ai_completions.get_untracked());
    }

    #[test]
    fn a_toggle_is_the_value_saved_to_config_toml() {
        let mut on_disk = toggles(false, false, false, true);
        on_disk.show_line_numbers = false; // a key the IDE doesn't hold
        let (prefs, editor) = editor_from(&on_disk);

        let file = Rc::new(RefCell::new(on_disk));
        let saves = Rc::new(Cell::new(0));
        let theme = create_rw_signal(PhazeTheme::from_variant(ThemeVariant::Dracula));
        persist_to(prefs, theme, &file, &saves);
        assert_eq!(saves.get(), 0, "nothing is saved at start");

        // Toggle the way the command palette, the shortcuts and Settings do.
        editor.organize_imports_on_save.update(|v| *v = !*v);
        editor.inlay_hints_toggle.update(|v| *v = !*v);
        editor.code_lens_visible.update(|v| *v = !*v);
        editor.ai_completions.update(|v| *v = !*v);
        theme.set(PhazeTheme::from_variant(ThemeVariant::NordDark));

        assert_eq!(saves.get(), 5, "each change is saved");
        let saved = file.borrow();
        assert!(saved.organize_imports_on_save);
        assert!(saved.inlay_hints);
        assert!(saved.code_lens);
        assert!(!saved.ai_completions);
        assert_eq!(saved.theme, "Nord Dark");
        assert!(!saved.show_line_numbers, "other keys are left as they were");
    }

    #[test]
    fn a_change_here_keeps_settings_changed_elsewhere_since_start() {
        let on_disk = toggles(false, true, true, true);
        let (prefs, editor) = editor_from(&on_disk);
        let file = Rc::new(RefCell::new(on_disk));
        let theme = create_rw_signal(PhazeTheme::from_variant(ThemeVariant::Dracula));
        persist_to(prefs, theme, &file, &Rc::new(Cell::new(0)));

        // Since start, another PhazeAI window (or a hand edit) turned AI
        // completions off and organize imports on. This window still holds
        // the values it started with.
        file.borrow_mut().ai_completions = false;
        file.borrow_mut().organize_imports_on_save = true;
        assert!(editor.ai_completions.get_untracked());

        // An unrelated change here saves that change only.
        let size = editor.font_size.get_untracked();
        editor.font_size.update(|v| *v += 1);

        let saved = file.borrow();
        assert_eq!(saved.font_size, (size + 1) as f32, "the change is saved");
        assert!(!saved.ai_completions, "AI completions stay off");
        assert!(saved.organize_imports_on_save, "organize imports stays on");
    }

    /// Run [`EditorPrefs::persist`] with a stand-in for `save_editor_settings`
    /// that applies each edit to `file` and counts it in `saves`.
    fn persist_to(
        prefs: EditorPrefs,
        theme: RwSignal<PhazeTheme>,
        file: &Rc<RefCell<EditorSettings>>,
        saves: &Rc<Cell<u32>>,
    ) {
        let (file, saves) = (file.clone(), saves.clone());
        prefs.persist(theme, move |edit| {
            edit(&mut file.borrow_mut());
            saves.set(saves.get() + 1);
        });
    }
}
