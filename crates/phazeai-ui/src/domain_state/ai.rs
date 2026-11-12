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
    /// Tokens and cost of every chat model call this session, counted as
    /// each call reports them.
    pub token_meter: RwSignal<phazeai_core::cost::Meter>,
}
