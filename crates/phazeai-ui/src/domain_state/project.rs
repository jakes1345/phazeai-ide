use std::path::PathBuf;
use std::sync::Arc;
use floem::reactive::RwSignal;
use phazeai_sidecar::SidecarClient;

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
}
