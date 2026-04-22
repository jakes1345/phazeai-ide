use floem::reactive::RwSignal;

#[derive(Clone)]
pub struct AiState {
    pub provider: RwSignal<String>,
    pub model: RwSignal<String>,
    pub thinking: RwSignal<bool>,
    pub ghost_text: RwSignal<Option<String>>,
    pub pending_chat_inject: RwSignal<Option<String>>,
    pub inline_edit_open: RwSignal<bool>,
    pub inline_edit_query: RwSignal<String>,
    pub token_usage_input: RwSignal<u64>,
    pub token_usage_output: RwSignal<u64>}
