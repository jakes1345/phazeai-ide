use floem::reactive::RwSignal;
use phazeai_sidecar::SidecarClient;
use std::path::PathBuf;
use std::sync::Arc;

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
    pub sidecar_client: Arc<std::sync::Mutex<Option<Arc<SidecarClient>>>>,
    pub sidecar_ready: RwSignal<bool>,
    pub sidecar_status: RwSignal<String>,
    pub sidecar_building: RwSignal<bool>,
    pub sidecar_results: RwSignal<Vec<(String, String)>>,
    pub sidecar_query: RwSignal<String>,
    pub sidecar_build_nonce: RwSignal<u64>,
    pub sidecar_search_nonce: RwSignal<u64>,
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
