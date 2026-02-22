mod claude;
pub mod discovery;
pub mod model_router;
mod ollama;
pub mod ollama_manager;
mod openai;
pub mod provider;
mod traits;

pub use claude::ClaudeClient;
pub use discovery::LocalDiscovery;
pub use model_router::{ModelRoute, ModelRouter, TaskType};
pub use ollama::OllamaClient;
pub use ollama_manager::OllamaManager;
pub use openai::OpenAIClient;
pub use provider::{ModelInfo, ProviderConfig, ProviderId, ProviderRegistry, UsageTracker};
pub use traits::*;
