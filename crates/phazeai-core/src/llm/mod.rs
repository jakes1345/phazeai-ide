mod traits;
mod claude;
mod openai;
mod ollama;
pub mod provider;
pub mod discovery;
pub mod model_router;
pub mod ollama_manager;

pub use traits::*;
pub use claude::ClaudeClient;
pub use openai::OpenAIClient;
pub use ollama::OllamaClient;
pub use provider::{ProviderId, ProviderConfig, ProviderRegistry, ModelInfo, UsageTracker};
pub use discovery::LocalDiscovery;
pub use model_router::{TaskType, ModelRouter, ModelRoute};
pub use ollama_manager::OllamaManager;

