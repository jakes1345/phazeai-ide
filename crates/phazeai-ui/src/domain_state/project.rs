use crate::code_search::Notice;
use floem::reactive::RwSignal;
use phazeai_core::search::SearchHit;
use phazeai_core::CodeSearch;
use std::path::PathBuf;

/// Debug session state (DAP).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugStatus {
    Idle,
    Running,
    Stopped,
}

#[derive(Clone)]
pub struct ProjectState {
    pub workspace_root: RwSignal<PathBuf>,
    pub git_branch: RwSignal<String>,
    /// In-process code search over the workspace; one per window, shared by
    /// the Search panel, the editor (re-index on save) and the AI tools.
    pub code_search: CodeSearch,
    /// `[search] enabled` at startup. When false nothing is indexed or run.
    pub code_search_enabled: bool,
    /// `[search] auto_index` at startup: index in the background when a
    /// workspace that looks like a project opens.
    pub code_search_auto_index: bool,
    /// One line describing the index ("Indexed 1,204 files · 5,330 chunks").
    pub code_search_status: RwSignal<String>,
    /// A build is running (the Reindex button reads "Reindexing…").
    pub code_search_building: RwSignal<bool>,
    pub code_search_results: RwSignal<Vec<SearchHit>>,
    /// Outcome of the last search or build worth telling the user about.
    pub code_search_notice: RwSignal<Option<Notice>>,
    /// Bumped by every search and workspace switch; a result that comes back
    /// under an older value is stale and dropped.
    pub code_search_seq: RwSignal<u64>,
    pub branch_picker_open: RwSignal<bool>,
    pub branch_list: RwSignal<Vec<String>>,
    pub lsp_progress: RwSignal<Option<String>>,
    pub lsp_cmd: tokio::sync::mpsc::UnboundedSender<crate::lsp_bridge::LspCommand>,
    pub scratch_paths: RwSignal<Vec<std::path::PathBuf>>,
    pub scratch_counter: RwSignal<u32>,
    pub initial_tabs: Vec<PathBuf>,

    // DAP debugger state
    pub debug_status: RwSignal<DebugStatus>,
    pub debug_thread_id: RwSignal<u64>,
    pub debug_output: RwSignal<Vec<String>>,
    pub breakpoints: RwSignal<Vec<(PathBuf, u64)>>,
    /// Where execution is stopped: (file, 1-based line). Drives the editor highlight.
    pub debug_stopped_at: RwSignal<Option<(PathBuf, u64)>>,
    pub debug_frames: RwSignal<Vec<crate::debug_session::FrameRow>>,
    pub debug_vars: RwSignal<Vec<crate::debug_session::VarRow>>,
    /// Command channel into the live session thread (None = no session).
    pub debug_cmd: RwSignal<Option<std::sync::mpsc::Sender<crate::debug_session::DebugCmd>>>,
}

impl ProjectState {
    /// The code search handle for things that consume it (AI tools, the
    /// editor's re-index on save): `None` when the settings turn it off.
    pub fn code_search_if_enabled(&self) -> Option<CodeSearch> {
        self.code_search_enabled.then(|| self.code_search.clone())
    }
}
