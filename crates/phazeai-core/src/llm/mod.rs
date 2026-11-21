mod claude;
pub mod context_budget;
pub mod discovery;
pub mod image;
pub mod key_setup;
pub mod model_list;
pub mod model_router;
mod ollama;
pub mod ollama_manager;
pub(crate) mod openai;
pub mod provider;
pub mod retry;
mod stream_cancel;
mod text_only;
mod traits;

pub use claude::{current_model_id, ClaudeClient, EFFORT_LEVELS};
pub use discovery::LocalDiscovery;
pub use image::ImageInput;
pub use model_router::{ModelRoute, ModelRouter, TaskType};
pub use ollama::OllamaClient;
pub use ollama_manager::OllamaManager;
pub use openai::{CacheHints, OpenAIClient};
pub use provider::{
    small_model_for, ModelInfo, ProviderConfig, ProviderId, ProviderRegistry, UsageTracker,
};
pub use text_only::{text_only_reply, text_only_stream, TextReply};
pub use traits::*;
